//! Measurement test without a window: player core with real FFmpeg streams (starter thread), real
//! single-frame decoder and sound without device (the same audio windows as in the app, just not audible).
//! Five places per project, `CUTLINE_QA_SECS` (default 30) seconds of playback per place, after 10 s
//! an edit (effect + volume), after 20 s a click at another place.
//!
//! Run by hand (projects `PL_einfach/PL_schnitt/PL_gross.cutline` in the folder):
//! `CUTLINE_QA_DIR=<folder> cargo test --release -p cl-app --bin cutline play_measure -- --ignored --nocapture`
//! Proxies are used if they lie in the data folder (`CUTLINE_HOME`) – as in the app.
//! Endurance run (memory): `CUTLINE_QA_SECS=150 CUTLINE_QA_JUMP=8` jumps every 8 s after the click
//! and prints the size of all caches after the startup and at the end (`mem`).

use std::time::Instant;

use eframe::egui;

use super::*;
use crate::audio::AudioPlayer;
use crate::playcore::{quality_steps, AudioIn, Cmd, Mode, PlayCore, Tick, Want};
use crate::preview::{Decoder, Done, Job, Starter};
use crate::proxy::Pick;

/// Measurements of one run
#[derive(Debug, Default)]
struct Run {
    label: String,
    proxy: bool,
    /// Play → first frame, Play → first advanced frame (ms)
    first_ms: f64,
    moving_ms: f64,
    /// Frames/s of the lowest video layer in the quiet part (2 s … edit)
    fps: f64,
    target_fps: f64,
    /// largest gap between two frames of the lowest video layer after the startup (ms)
    gap_ms: f64,
    dropped_per_s: f64,
    /// largest / mean lag of the shown stream frame behind the clock (= picture-sound offset)
    lag_max_ms: f64,
    lag_avg_ms: f64,
    underruns: u64,
    underrun_ms: f64,
    catch_ups: u32,
    level_max: usize,
    /// Edit during playback: largest gap afterwards, until the changed frame is visible (ms)
    edit_gap_ms: f64,
    /// Dragging a slider during playback: largest gap of the lowest layer (ms)
    drag_gap_ms: f64,
    edit_visible_ms: f64,
    /// Click during playback → first frame at the new place (ms)
    click_ms: f64,
    clock_audio: bool,
    /// Single frames during playback: requested / shown
    stills_req: u32,
    stills_shown: u32,
    /// longest time without a new frame on any layer while the clock runs (ms)
    freeze_ms: f64,
    /// longest time job → single frame (ms)
    still_max_ms: f64,
    /// in the 3 s after Pause → Play at the same place: opened streams, highest quality level, catch-ups
    resume_opens: u32,
    resume_level: usize,
    resume_catch_up: u32,
    /// shown frames per layer (stream or single frame), order of the layers in the project
    layer_frames: std::collections::BTreeMap<String, u32>,
    /// Caches after the startup and at the end (endurance run, `CUTLINE_QA_JUMP`)
    mem: Vec<String>,
    /// Frames/s in the quiet part, only single frames with new content (a still frame does not count)
    fps_new: f64,
    /// before the edit: largest lag (ms) and highest quality level
    lag_quiet_ms: f64,
    level_quiet: usize,
}

/// Private memory of the process (MB), where available
fn private_mb() -> f64 {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct Counters {
            cb: u32,
            faults: u32,
            peak_ws: usize,
            ws: usize,
            quota: [usize; 4],
            pagefile: usize,
            peak_pagefile: usize,
            private: usize,
        }
        extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn K32GetProcessMemoryInfo(p: *mut std::ffi::c_void, c: *mut Counters, cb: u32) -> i32;
        }
        let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Default::default() };
        // SAFETY: struct of the size of PROCESS_MEMORY_COUNTERS_EX
        if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) } != 0 {
            return c.private as f64 / 1048576.0;
        }
    }
    f64::NAN
}

/// All caches of the player (core, dispatcher, starter queue) and process memory
fn sizes(core: &PlayCore<cl_media::FrameStream>, decoder: &Decoder, starter_queue: i64, t: f64) -> String {
    use std::sync::atomic::Ordering::Relaxed;
    let h = &decoder.held;
    format!(
        "{t:.0} s: privat {:.0} MB | Kern {} | Einzelbilder {} ({:.0} MB), Animationen {} ({:.0} MB) | Starter wartend {starter_queue}",
        private_mb(),
        core.sizes(),
        h[0].load(Relaxed),
        h[1].load(Relaxed) as f64 / 1048576.0,
        h[2].load(Relaxed),
        h[3].load(Relaxed) as f64 / 1048576.0,
    )
}

/// Lowest video layer at the playhead (not ahead, no element version)
fn primary(needs: &[Need]) -> Option<ClipId> {
    needs
        .iter()
        .find(|n| n.prefetch.is_none() && n.overlay.is_none() && matches!(n.clip.content, ClipContent::Media { kind: MediaKind::Video, .. }))
        .map(|n| n.id)
}

fn clip_mut(p: &mut Project, id: ClipId) -> Option<&mut Clip> {
    p.timeline.tracks.iter_mut().flat_map(|t| t.clips.iter_mut()).find(|c| c.id == id)
}

fn drive(project: Project, label: &str, start: Time, secs: f64) -> Run {
    drive_parts(project, label, start, secs, &Default::default())
}

/// Source of a stream from proxy parts as in `App::stream_pick`/`stream_source`: `parts` maps a
/// media path to its parts folder (frame rate of the parts = source rate, as for 60 fps media in a
/// 60 fps project). Result: choice, file and its start in the source.
fn part_pick(prev: &mut std::collections::HashMap<ClipId, Pick>, parts: &std::collections::HashMap<std::path::PathBuf, std::path::PathBuf>, id: ClipId, c: &Clip, m: &MediaRef, from: Time) -> Option<(Pick, std::path::PathBuf, Time)> {
    use crate::proxy::parts;
    let dir = parts.get(&m.path)?;
    let pt = parts::Parting::new(m.duration.as_secs_f64(), m.fps?, parts::PART_SECS)?;
    let p = c.source_time(Time(from.0.clamp(c.start.0, c.end().0))).as_secs_f64();
    let end = (c.source_in + c.source_span()).as_secs_f64().min(m.duration.as_secs_f64());
    let pick = parts::choose_stream(prev.get(&id).copied(), &parts::scan_ready(dir, pt.count), &pt, p, end, parts::SWITCH_MARGIN * c.speed.max(0.01) as f64);
    prev.insert(id, pick);
    Some(match pick {
        Pick::Parts { first, end } => (pick, parts::concat_list(dir, first, end).ok()?, Time::from_secs_f64(pt.start(first))),
        _ => (pick, m.path.clone(), Time::ZERO),
    })
}

#[allow(clippy::too_many_lines)]
fn drive_parts(mut project: Project, label: &str, start: Time, secs: f64, parts: &std::collections::HashMap<std::path::PathBuf, std::path::PathBuf>) -> Run {
    let mut picks = std::collections::HashMap::new();
    let mut shown_key: std::collections::HashMap<ClipId, u64> = Default::default();
    let ctx = egui::Context::default();
    let decoder = Decoder::new(ctx.clone());
    let starter = Starter::new(ctx);
    let mut core: PlayCore<cl_media::FrameStream> = PlayCore::default();
    // comparison "before": without targeted catching up and without automatic quality
    let baseline = std::env::var_os("CUTLINE_QA_BASELINE").is_some();
    core.catch_up = !baseline;
    let t0 = Instant::now();
    let now = || t0.elapsed().as_secs_f64();
    let fps = project.settings.fps;
    let steps = quality_steps(1, fps.as_f64());
    let canvas = egui::vec2(1280.0, 720.0);
    let proxy_h = crate::settings::AppSettings::default().proxy_h;
    let mut audio: Option<AudioPlayer> = None;
    let mut playhead = start;
    let mut revision = 0u64;
    let mut run = Run { label: label.into(), target_fps: fps.as_f64(), ..Default::default() };
    let (edit_at, click_at) = (10.0f64.min(secs * 0.4), 20.0f64.min(secs * 0.7));
    let (mut edited, mut clicked) = (None::<(f64, ClipId)>, None::<(f64, Time)>);
    let mut edit_key: Option<(ClipId, u64)> = None;
    // frame sequence of the lowest video layer: (wall time)
    let mut prim_frames: Vec<f64> = Vec::new();
    let (mut prim_new, mut last_data): (Vec<f64>, Vec<u8>) = (Vec::new(), Vec::new());
    let mut lags: Vec<f64> = Vec::new();
    let mut dropped = 0u32;
    let mut first_any: Option<f64> = None;
    let mut moving: Option<f64> = None;
    let mut warm_until = f64::MAX; // count gaps only after the startup
    let mut any_frame = f64::NAN;
    // endurance run: a jump every this many seconds (in addition to the click)
    let jump_every: Option<f64> = std::env::var("CUTLINE_QA_JUMP").ok().and_then(|s| s.parse().ok());
    let mut last_jump = 0.0;
    let mut starter_queue = 0i64;
    let debug = std::env::var_os("CUTLINE_QA_DEBUG").is_some();
    let mut still_asked: std::collections::HashMap<u64, (f64, f64)> = Default::default();
    let drag_at = (edit_at + click_at) / 2.0;
    // `CUTLINE_QA_PAUSE`: Pause and Play at the same place (before the edit); afterwards
    // no streams may open anew nor the quality drop (would falsify the frames/s of the quiet part)
    let pause_at = if std::env::var_os("CUTLINE_QA_PAUSE").is_some() { edit_at * 0.5 } else { f64::MAX };
    let mut paused_since: Option<f64> = None;
    let mut resumed: Option<f64> = None;
    let mut dragging = false;
    // `CUTLINE_QA_PREROLL`: preroll as when the app is stopped (`App::update_preroll`) – open the stream of the
    // lowest layer ahead, let it rest, attach on Play
    let mut preroll: Option<(PrerollSpec, f64, cl_media::FrameStream)> = None;
    if std::env::var_os("CUTLINE_QA_PREROLL").is_some() {
        let ns = needs(&project, &layers_in(&project, None, start), start, false, |_| {});
        if let Some(n) = ns.iter().find(|n| n.prefetch.is_none() && n.overlay.is_none() && streamable(&n.clip)) {
            let (div, half) = steps[0];
            let sfps = if half { Rational::new(fps.num, fps.den * 2) } else { fps };
            let fxh = fx_hash_of(&n.clip);
            let (key, w, h) = stream_spec(n.id, &n.clip, &n.media, canvas, div as f32, sfps, fxh);
            let alias = stream_alias(&n.clip, &n.media, (w, h), sfps, fxh);
            let spec = PrerollSpec { id: lid(n.id), key, alias, at: start, revision };
            let o = crate::playcore::Open { id: spec.id, key, at: start, gen: u64::MAX, seq: 0 };
            starter.send(open_job(o, &n.clip, &n.media, n.media.path.clone(), (w, h), sfps, alias));
            let (_, f, _, s) = starter.rx.recv().expect("Starter");
            std::thread::sleep(std::time::Duration::from_millis(1500));
            preroll = s.map(|s| (spec, f, s));
        }
    }
    core.play(start, revision, now());
    let t_play = now();
    while now() - t_play < secs {
        let t = now();
        let rel = t - t_play;
        // edit: brightness of the lowest layer + volume (visible and audible)
        if edited.is_none() && rel >= edit_at {
            let layers = layers_in(&project, None, playhead);
            if let Some(id) = primary(&needs(&project, &layers, playhead, true, |_| {})) {
                if let Some(c) = clip_mut(&mut project, id) {
                    c.fx.adjust.brightness = (c.fx.adjust.brightness + 0.1).min(1.0);
                    c.volume_db -= 1.0;
                }
                revision += 1;
                core.forget_stills();
                core.edit(true, t);
                edited = Some((t, id));
            }
        }
        // dragging a slider (inspector): for 1.5 s a new effect value and a new
        // size of the lowest layer per UI frame, only then one step in the history
        if paused_since.is_none() && resumed.is_none() && rel >= pause_at && core.mode() == Mode::Spielen {
            let running = audio.as_ref().is_some_and(|a| a.started());
            core.pause(revision, running, true);
            paused_since = Some(rel);
            if debug {
                eprintln!("PAUSE {rel:.2} s pos {:.2} Stufe {}", playhead.as_secs_f64(), core.quality.level);
            }
        } else if paused_since.is_some_and(|p| rel - p >= 1.5) {
            paused_since = None;
            core.play(playhead, revision, t);
            resumed = Some(t);
            if debug {
                eprintln!("PLAY {rel:.2} s pos {:.2} Stufe {} {}", playhead.as_secs_f64(), core.quality.level, core.sizes());
            }
        }
        if (drag_at..drag_at + 1.5).contains(&rel) {
            let layers = layers_in(&project, None, playhead);
            if let Some(id) = primary(&needs(&project, &layers, playhead, true, |_| {})) {
                if let Some(c) = clip_mut(&mut project, id) {
                    c.fx.adjust.contrast = 0.2 * ((rel - drag_at) * 7.0).sin() as f32;
                    c.transform.scale = 1.0 + 0.3 * (rel - drag_at) as f32;
                }
            }
            dragging = true;
        } else if dragging {
            dragging = false;
            revision += 1;
            core.edit(false, t);
        }
        // click at another place
        if clicked.is_none() && rel >= click_at {
            let d = project.duration().as_secs_f64();
            let to = Time::from_secs_f64((playhead.as_secs_f64() + 37.0) % (d - 5.0).max(1.0));
            playhead = to;
            clicked = Some((t, to));
        }
        if let Some(j) = jump_every.filter(|j| rel - last_jump >= *j && rel > click_at + 3.0) {
            let d = project.duration().as_secs_f64();
            playhead = Time::from_secs_f64((playhead.as_secs_f64() + 37.0 + j) % (d - 5.0).max(1.0));
            last_jump = rel;
        }
        if run.mem.is_empty() && rel >= 3.0 {
            run.mem.push(sizes(&core, &decoder, starter_queue, rel));
        }
        let ain = match &audio {
            None => AudioIn::None,
            Some(a) if a.failed() => AudioIn::Failed,
            Some(a) if a.started() && a.finished() => AudioIn::Finished,
            Some(a) if a.started() => AudioIn::Running(a.position()),
            Some(a) if !a.ready() => AudioIn::Loading,
            Some(_) => AudioIn::Pending,
        };
        if let Some(a) = &audio {
            let (n, d) = a.take_underruns();
            run.underruns += n;
            run.underrun_ms += d.as_secs_f64() * 1000.0;
        }
        let tick = Tick {
            now: t,
            playhead,
            revision,
            audio: ain,
            end: project.duration(),
            fps: fps.as_f64(),
            loop_range: None,
            recording: false,
            max_quality_level: if baseline { 0 } else { steps.len() - 1 },
        };
        playhead = core.tick(&tick);
        for c in core.cmds() {
            match c {
                Cmd::StartAudio { at, anchor } => {
                    audio = AudioPlayer::start_null(&project, at).ok();
                    if anchor {
                        if let Some(a) = &audio {
                            a.anchor(Instant::now());
                        }
                    }
                }
                Cmd::AnchorAudio => {
                    if let Some(a) = &audio {
                        a.anchor(Instant::now());
                    }
                }
                Cmd::StopAudio => audio = None,
                Cmd::PauseAudio => {
                    if !audio.as_ref().is_some_and(|a| a.pause()) {
                        core.stop();
                    }
                }
                Cmd::ResumeAudio => {
                    if let Some(a) = &audio {
                        a.resume();
                    }
                }
            }
        }
        if core.mode() == Mode::Stand {
            break; // project end
        }
        run.clock_audio |= matches!(ain, AudioIn::Running(_));
        run.level_max = run.level_max.max(core.quality.level);
        if rel < edit_at {
            run.level_quiet = run.level_quiet.max(core.quality.level);
        }
        while let Ok((o, f, alias, s)) = starter.rx.try_recv() {
            starter_queue -= 1;
            match s {
                Some(s) => {
                    if debug {
                        eprintln!("angehängt ({rel:.2} s, pos {:.2}) {:04x} ab {:.2}", playhead.as_secs_f64(), o.id & 0xffff, o.at.as_secs_f64());
                    }
                    core.attach_alias(o, f, s, t, alias);
                }
                None => core.open_failed(o, t),
            }
        }
        let layers = layers_in(&project, None, playhead);
        let needs = needs(&project, &layers, playhead, true, |_| {});
        let prim = primary(&needs);
        macro_rules! got {
            ($id:expr, $key:expr) => {{
                let (id, key): (ClipId, u64) = ($id, $key);
                first_any.get_or_insert(t);
                if debug && shown_key.insert(id, key).is_some_and(|k| k != key) {
                    eprintln!("neuer Schlüssel sichtbar ({rel:.2} s, pos {:.2}) {:04x}, Rückstand {:.0} ms", playhead.as_secs_f64(), lid(id) & 0xffff, core.av_offset_ms().unwrap_or(0.0));
                }
                *run.layer_frames.entry(format!("{:04x}", lid(id) & 0xffff)).or_insert(0) += 1;
                if Some(id) == prim {
                    if debug && t >= warm_until && prim_frames.last().is_some_and(|l| t - l > 0.15) {
                        let what: Vec<String> = needs
                            .iter()
                            .map(|n| format!("{}{}{}{}", format!("{:04x}", lid(n.id) & 0xffff), if streamable(&n.clip) { " Strom" } else { " Einzelbild" }, if n.prefetch.is_some() { " vorab" } else { "" }, if n.overlay.is_some() { " Element" } else { "" }))
                            .collect();
                        eprintln!("Lücke {:.0} ms bis {rel:.2} s (pos {:.2}): Hauptebene {} – {what:?} | {}", (t - prim_frames.last().unwrap()) * 1000.0, playhead.as_secs_f64(), format!("{:04x}", lid(id) & 0xffff), core.sizes());
                    }
                    prim_frames.push(t);
                }
                if edit_key.is_some_and(|(e, k)| e == id && k == key) {
                    if let Some((te, _)) = edited {
                        if run.edit_visible_ms == 0.0 {
                            run.edit_visible_ms = (t - te) * 1000.0;
                        }
                    }
                }
            }};
        }
        while let Ok(d) = decoder.rx.try_recv() {
            match d {
                Done::Frame { clip, key, frame } => {
                    if let Some((ta, src)) = still_asked.remove(&key) {
                        let ms = (t - ta) * 1000.0;
                        if debug && ms > 700.0 {
                            eprintln!("Einzelbild {ms:.0} ms bis {rel:.2} s (Quelle {src:.2} s)");
                        }
                        run.still_max_ms = run.still_max_ms.max(ms);
                    }
                    if core.still_done(lid(clip), key) {
                        run.stills_shown += 1;
                        if Some(clip) == prim && frame.data != last_data {
                            prim_new.push(t);
                            last_data = frame.data;
                        }
                        any_frame = t;
                        got!(clip, key);
                    }
                }
                Done::FrameFailed { clip, key, .. } => core.still_failed(lid(clip), key),
                _ => {}
            }
        }
        let (div, half) = steps[core.quality.level.min(steps.len() - 1)];
        let sfps = if half { Rational::new(fps.num, fps.den * 2) } else { fps };
        let mut ids = Vec::new();
        for n in &needs {
            ids.push(lid(n.id));
            let (c, m) = (&n.clip, &n.media);
            let path = || {
                crate::proxy::existing_proxy(m, proxy_h, fps).map(|p| p.0).unwrap_or_else(|| m.path.clone())
            };
            if core.streaming() && streamable(c) {
                let (key, w, h) = stream_spec(n.id, c, m, canvas, div as f32, sfps, fx_hash_of(c));
                let src = part_pick(&mut picks, parts, n.id, c, m, n.prefetch.unwrap_or(playhead));
                let pick = src.as_ref().map_or(Pick::Whole, |s| s.0);
                let key = key ^ pick.tag();
                if edited.is_some_and(|e| e.1 == n.id) && edit_key.is_none() {
                    edit_key = Some((n.id, key));
                }
                let alias = stream_alias(c, m, (w, h), sfps, fx_hash_of(c)) ^ pick.tag();
                let want = Want { id: lid(n.id), key, start: want_start(c, pick, playhead, n.prefetch), end: c.end(), prefetch: n.prefetch.is_some(), alias };
                let o = core.want(&want, t);
                let pre = o.and_then(|o| preroll.take_if(|p| preroll_fits(&p.0, &o, alias, revision)));
                if let (Some(o), Some((_, f, s))) = (o, pre) {
                    core.attach_alias(o, f, s, t, alias);
                } else if let Some(o) = o {
                    let (p, off) = src.map_or_else(|| (path(), Time::ZERO), |s| (s.1, s.2));
                    run.proxy |= p != m.path;
                    let mut j = open_job(o, c, m, p, (w, h), sfps, alias);
                    j.source_at = Time((j.source_at - off).0.max(0));
                    if std::env::var_os("CUTLINE_QA_DEBUG").is_some() {
                        eprintln!("öffnen ({rel:.2} s, pos {:.2}, Stufe {}) {:04x} {:.2} s: {} ab {:.3} {}×{} hw={:?} graph={:?}", playhead.as_secs_f64(), core.quality.level, o.id & 0xffff, o.at.as_secs_f64(), j.path.display(), j.source_at.as_secs_f64(), w, h, j.hw, j.graph);
                    }
                    starter.send(j);
                    starter_queue += 1;
                }
                if n.prefetch.is_none() && core.pull(lid(n.id), t, n.overlay.is_none()).is_some() {
                    any_frame = t;
                    got!(n.id, key);
                    if moving.is_none() && core.mode() == Mode::Spielen && playhead > start {
                        moving = Some(t);
                        warm_until = t;
                    }
                }
            } else if n.prefetch.is_none() {
                let (w, h) = decode_size(c, m, canvas, div as f32);
                let src = still_src(c, m, n.layer.t_rel, fps);
                let key = App::frame_key(n.id, c, src, w, h, n.layer.t_rel, false, fx_hash_of(c), fps);
                if core.still(lid(n.id), key, t) {
                    run.stills_req += 1;
                    still_asked.insert(key, (t, src.as_secs_f64()));
                    let graph = cl_fx::preview_graph(c, w, h, n.layer.t_rel.as_secs_f64(), None, None);
                    let p = path();
                    let window = reverse_window(c, m, p == m.path, graph.is_none(), core.playing() && core.rate() > 0.0, fps);
                    decoder.send(Job::Frame { clip: n.id, key, path: p, at: src, w, h, graph, window });
                }
            }
        }
        core.visible(&ids);
        // standstill: clock runs, but no layer gets a new frame (not during startup/click)
        let in_click = clicked.is_some_and(|c| c.0 <= t && run.click_ms == 0.0);
        if core.mode() == Mode::Spielen && t >= warm_until && any_frame.is_finite() && !in_click {
            run.freeze_ms = run.freeze_ms.max((t - any_frame) * 1000.0);
        }
        if core.mode() == Mode::Spielen {
            if let Some(l) = core.av_offset_ms() {
                if debug && l > 100.0 && (rel * 10.0).fract() < 0.03 {
                    eprintln!("Rückstand {l:.0} ms bei {rel:.2} s (pos {:.2}) {}", playhead.as_secs_f64(), core.sizes());
                }
                lags.push(l);
                if rel < edit_at {
                    run.lag_quiet_ms = run.lag_quiet_ms.max(l);
                }
            }
        }
        // click → first frame at the new place
        if let Some((tc, to)) = clicked {
            if run.click_ms == 0.0 && core.mode() == Mode::Spielen && playhead >= to && prim_frames.last().is_some_and(|f| *f >= tc) {
                run.click_ms = (t - tc) * 1000.0;
                warm_until = t;
            }
        }
        let st = core.take_stats();
        if let Some(r) = resumed.filter(|r| t - r < 3.0) {
            run.resume_opens += st.opens;
            run.resume_level = run.resume_level.max(core.quality.level);
            run.resume_catch_up += st.catch_ups;
            let _ = r;
        }
        if std::env::var_os("CUTLINE_QA_DEBUG").is_some() && (rel * 2.0).fract() < 0.01 {
            eprintln!("{rel:.1} s: pos {:.2} {:?} Ströme {} Ebenen {} prim {:?} Bilder {} {st:?}", playhead.as_secs_f64(), core.mode(), core.stream_count(), needs.len(), prim.is_some(), prim_frames.len());
        }
        dropped += st.dropped;
        run.catch_ups += st.catch_ups;
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    run.mem.push(sizes(&core, &decoder, starter_queue, now() - t_play));
    run.first_ms = first_any.map_or(f64::NAN, |f| (f - t_play) * 1000.0);
    run.moving_ms = moving.map_or(f64::NAN, |f| (f - t_play) * 1000.0);
    let ruhig: Vec<f64> = prim_frames.iter().copied().filter(|f| *f - t_play >= 2.0 && *f - t_play < edit_at).collect();
    if ruhig.len() > 1 {
        run.fps = (ruhig.len() - 1) as f64 / (ruhig[ruhig.len() - 1] - ruhig[0]);
    }
    let neu: Vec<f64> = prim_new.iter().copied().filter(|f| *f - t_play >= 2.0 && *f - t_play < edit_at).collect();
    if neu.len() > 1 {
        run.fps_new = (neu.len() - 1) as f64 / (neu[neu.len() - 1] - neu[0]);
    }
    let click_t = clicked.map_or(f64::MAX, |c| c.0);
    let after_click = click_t + run.click_ms / 1000.0;
    run.gap_ms = prim_frames
        .windows(2)
        .filter(|w| w[0] >= warm_until.min(t_play + 2.0) && !(w[1] > click_t && w[0] < after_click))
        .map(|w| (w[1] - w[0]) * 1000.0)
        .fold(0.0, f64::max);
    if let Some((te, _)) = edited {
        let d0 = t_play + drag_at;
        run.drag_gap_ms = prim_frames.windows(2).filter(|w| w[1] >= d0 && w[0] <= d0 + 2.5).map(|w| (w[1] - w[0]) * 1000.0).fold(0.0, f64::max);
        run.edit_gap_ms = prim_frames.windows(2).filter(|w| w[1] >= te && w[0] <= te + 3.0 && w[1] < click_t).map(|w| (w[1] - w[0]) * 1000.0).fold(0.0, f64::max);
    }
    run.dropped_per_s = dropped as f64 / secs;
    run.lag_max_ms = lags.iter().copied().fold(0.0, f64::max);
    run.lag_avg_ms = if lags.is_empty() { 0.0 } else { lags.iter().sum::<f64>() / lags.len() as f64 };
    run
}

#[test]
#[ignore]
fn play_measure() {
    let Some(dir) = std::env::var_os("CUTLINE_QA_DIR").map(std::path::PathBuf::from) else {
        eprintln!("CUTLINE_QA_DIR fehlt – übersprungen");
        return;
    };
    if !cl_media::ffmpeg_available() {
        return;
    }
    // effects and transitions of the bundled packages (as at app start)
    let reg = cl_plugin::Registry::scan([std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins"))]);
    cl_fx::catalog::load_packs(crate::contents::catalog_packs(&reg, "de"));
    // artificial full load: this many computing threads (like other programs on the computer)
    let stress: usize = std::env::var("CUTLINE_QA_STRESS").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let workers: Vec<_> = (0..stress)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut x = 1u64;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    for _ in 0..100_000 {
                        x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
                    }
                }
            })
        })
        .collect();
    let secs: f64 = std::env::var("CUTLINE_QA_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let only = std::env::var("CUTLINE_QA_ONLY").ok();
    let plan: [(&str, [f64; 5]); 3] = [
        ("PL_einfach", [37.0, 109.0, 335.0, 600.0, 1222.0]),
        ("PL_schnitt", [3.0, 37.0, 128.0, 335.0, 571.0]),
        ("PL_gross", [0.0, 40.0, 120.0, 335.0, 475.0]),
    ];
    let mut rows = Vec::new();
    for (name, at) in plan {
        if only.as_deref().is_some_and(|o| !name.contains(o)) {
            continue;
        }
        let Ok(json) = std::fs::read_to_string(dir.join(format!("{name}.cutline"))) else {
            eprintln!("{name}: fehlt – übersprungen");
            continue;
        };
        let project = Project::from_json(&json).expect("project readable");
        // measurement with proxies: finish building missing ones (in `CUTLINE_HOME`, not in the app's data folder)
        if std::env::var_os("CUTLINE_QA_PROXY").is_some() && std::env::var_os("CUTLINE_HOME").is_some() {
            let (h, fps) = (crate::settings::AppSettings::default().proxy_h, project.settings.fps);
            for m in project.media.iter().filter(|m| crate::proxy::wants_proxy(m) && m.path.is_file()) {
                if crate::proxy::existing_proxy(m, h, fps).is_none() {
                    let t = Instant::now();
                    let out = crate::proxy::proxy_path(m, h, fps);
                    let _ = std::fs::create_dir_all(out.parent().unwrap());
                    let r = crate::proxy::build(m, &out, &crate::proxy::proxy_spec(m, h, fps), &|_| {}, &std::sync::atomic::AtomicBool::new(false));
                    eprintln!("Proxy {} in {:.0} s: {}", m.path.display(), t.elapsed().as_secs_f64(), r.is_ok());
                }
            }
        }
        let at: Vec<f64> = match std::env::var("CUTLINE_QA_AT").ok().and_then(|a| a.parse().ok()) {
            Some(a) => vec![a],
            None => at.to_vec(),
        };
        for a in at {
            let r = drive(project.clone(), &format!("{name} {}:{:02}", a as u64 / 60, a as u64 % 60), Time::from_secs_f64(a), secs);
            eprintln!("{r:?}");
            rows.push(r);
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for w in workers {
        let _ = w.join();
    }
    eprintln!("\nkünstliche Last: {stress} rechnende Threads");
    eprintln!(
        "\n| Lauf | Proxy | Play→Bild | Play→bewegt | Bilder/s (Soll) | größte Lücke | verworfen/s | Rückstand max/Ø | Ton-Aussetzer | aufgeholt | Stufe | Bearb.: Lücke / sichtbar | Regler: Lücke | Klick→Bild | Stillstand max | Einzelbilder angef./gezeigt |"
    );
    eprintln!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for r in &rows {
        eprintln!(
            "| {} | {} | {:.0} ms | {:.0} ms | {:.1} ({:.0}) | {:.0} ms | {:.1} | {:.0} / {:.0} ms | {}× {:.0} ms | {} | {} | {:.0} / {:.0} ms | {:.0} ms | {:.0} ms | {:.0} ms | {} / {} |",
            r.label,
            if r.proxy { "ja" } else { "nein" },
            r.first_ms,
            r.moving_ms,
            r.fps,
            r.target_fps,
            r.gap_ms,
            r.dropped_per_s,
            r.lag_max_ms,
            r.lag_avg_ms,
            r.underruns,
            r.underrun_ms,
            r.catch_ups,
            r.level_max,
            r.edit_gap_ms,
            r.edit_visible_ms,
            r.drag_gap_ms,
            r.click_ms,
            r.freeze_ms,
            r.stills_req,
            r.stills_shown
        );
    }
    // generous limits (outside load on the computer must not interfere); in the endurance run with
    // jumps every startup counts as lag – there only look at the caches
    if std::env::var_os("CUTLINE_QA_JUMP").is_some() {
        return;
    }
    for r in &rows {
        assert!(r.first_ms < 3000.0, "{}: Play → frame {:.0} ms", r.label, r.first_ms);
        assert!(r.lag_max_ms < 1500.0, "{}: lag {:.0} ms", r.label, r.lag_max_ms);
        assert!(r.fps >= 0.5 * r.target_fps.min(30.0), "{}: {:.1} frames/s", r.label, r.fps);
        assert!(r.click_ms > 0.0 && r.click_ms < 3000.0, "{}: click → frame {:.0} ms", r.label, r.click_ms);
    }
}

/// A project with proxy parts (as in the app with an unfinished proxy build): `CUTLINE_QA_PARTS=
/// <media path>=<parts folder>;…` (folders are written to: concat lists), project
/// `CUTLINE_QA_DIR/CUTLINE_QA_ONLY.cutline`, places `CUTLINE_QA_AT=a,b,…`, `CUTLINE_QA_SECS`.
/// `cargo test --release -p cl-app --bin cutline parts_measure -- --ignored --nocapture`
#[test]
#[ignore]
fn parts_measure() {
    let (Some(dir), Ok(spec)) = (std::env::var_os("CUTLINE_QA_DIR"), std::env::var("CUTLINE_QA_PARTS")) else {
        return;
    };
    let name = std::env::var("CUTLINE_QA_ONLY").unwrap_or_else(|_| "PL_gross".into());
    let project = Project::from_json(&std::fs::read_to_string(std::path::Path::new(&dir).join(format!("{name}.cutline"))).unwrap()).unwrap();
    let parts = spec.split(';').filter_map(|s| s.split_once('=')).map(|(m, d)| (m.into(), d.into())).collect();
    let secs: f64 = std::env::var("CUTLINE_QA_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(15.0);
    for a in std::env::var("CUTLINE_QA_AT").unwrap_or_else(|_| "1.0".into()).split(',').filter_map(|a| a.parse::<f64>().ok()) {
        let r = drive_parts(project.clone(), &format!("{name} ab {a}"), Time::from_secs_f64(a), secs, &parts);
        eprintln!("PARTS | {} | Play→bewegt {:.0} ms | vor der Bearbeitung: Rückstand max {:.0} ms, Stufe {} | ganzer Lauf: Rückstand max {:.0} ms, Lücke {:.0} ms, verworfen/s {:.1}, Stufe {}", r.label, r.moving_ms, r.lag_quiet_ms, r.level_quiet, r.lag_max_ms, r.gap_ms, r.dropped_per_s, r.level_max);
    }
}

/// Cost of the player logic per UI frame without drawing (layers, demand, keys) and per stream frame
/// (conversion for the texture), at many places of the test projects.
/// Two 4K layers, the upper one with speed `CUTLINE_QA_TEMPO` (default 2; `rev` = reverse,
/// `kurve` = speed curve, `scale` = size keyframes): both layers must show frames continuously
/// (follow-up test K7: the speed-2 layer stayed a still image).
/// `cargo test --release -p cl-app --bin cutline tempo_layers -- --ignored --nocapture`
#[test]
#[ignore]
fn tempo_layers() {
    let (a, b) = (std::path::Path::new("F:/cutline-testmedien/4k60_h264_80mbit.mp4"), std::path::Path::new("F:/cutline-testmedien/4k60_hevc10_50mbit.mp4"));
    if !cl_media::ffmpeg_available() || !a.is_file() || !b.is_file() {
        return;
    }
    let mode = std::env::var("CUTLINE_QA_TEMPO").unwrap_or_else(|_| "2".into());
    let (ma, mb) = (cl_media::probe(a).unwrap(), cl_media::probe(b).unwrap());
    let mut p = Project::new("tempo");
    p.settings.fps = Rational::new(60, 1);
    p.settings.width = 1920;
    p.settings.height = 1080;
    let mut v1 = cl_core::Track::new(TrackKind::Video);
    v1.main = true;
    v1.clips.push(Clip::new(ClipContent::Media { media: ma.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(120.0)));
    let mut v2 = cl_core::Track::new(TrackKind::Video);
    let mut c = Clip::new(ClipContent::Media { media: mb.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(100.0));
    match mode.as_str() {
        "rev" => c.fx.reverse = true,
        "kurve" => c.fx.speed_curve = Some("montage".into()),
        "scale" => {
            for (i, v) in [0.5f32, 1.5, 0.5, 1.5].iter().enumerate() {
                c.keyframes.set(Prop::Scale, Time::from_secs_f64(20.0 + i as f64 * 5.0), *v);
            }
        }
        t => {
            c.speed = t.parse().unwrap_or(2.0);
            c.duration = Time::from_secs_f64(100.0 / c.speed as f64);
        }
    }
    v2.clips.push(c);
    p.timeline.tracks = vec![v1, v2];
    p.media = vec![ma, mb];
    let secs: f64 = std::env::var("CUTLINE_QA_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(12.0);
    for at in [20.0, 33.0] {
        let r = drive(p.clone(), &format!("Tempo {mode} ab {at}"), Time::from_secs_f64(at), secs);
        eprintln!("{}: Bilder je Ebene {:?}, Bilder/s unten {:.1}, Einzelbilder {}/{}", r.label, r.layer_frames, r.fps, r.stills_req, r.stills_shown);
        assert!(r.layer_frames.len() == 2 && r.layer_frames.values().all(|n| *n as f64 >= secs * 10.0), "{:?}", r.layer_frames);
    }
}

#[test]
#[ignore]
fn player_cost() {
    let Some(dir) = std::env::var_os("CUTLINE_QA_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    for name in ["PL_schnitt", "PL_gross"] {
        let Ok(json) = std::fs::read_to_string(dir.join(format!("{name}.cutline"))) else { continue };
        let project = Project::from_json(&json).unwrap();
        let d = project.duration().as_secs_f64();
        let n = 2000;
        let (mut t_json, mut t_cache) = (0.0, 0.0);
        let mut cache: std::collections::HashMap<ClipId, u64> = Default::default();
        for i in 0..n {
            let at = Time::from_secs_f64(d * i as f64 / n as f64);
            for pass in 0..2 {
                let t0 = Instant::now();
                let layers = layers_in(&project, None, at);
                let needs = needs(&project, &layers, at, true, |_| {});
                for nd in &needs {
                    let fxh = if pass == 0 { fx_hash_of(&nd.clip) } else { *cache.entry(nd.id).or_insert_with(|| fx_hash_of(&nd.clip)) };
                    std::hint::black_box(stream_spec(nd.id, &nd.clip, &nd.media, egui::vec2(1280.0, 720.0), 1.0, project.settings.fps, fxh));
                }
                let el = t0.elapsed().as_secs_f64() * 1e3;
                if pass == 0 { t_json += el } else { t_cache += el }
            }
        }
        eprintln!("{name}: Ebenen+Bedarf+Schlüssel je UI-Bild: mit JSON {:.3} ms, mit Zwischenspeicher {:.3} ms", t_json / n as f64, t_cache / n as f64);
    }
    let f = cl_media::RgbaFrame { width: 1280, height: 720, data: [30u8, 60, 90, 255].repeat(1280 * 720) };
    let t0 = Instant::now();
    for _ in 0..50 {
        std::hint::black_box(crate::preview::to_color_image(&f));
    }
    eprintln!("Strombild 1280×720 → Textur-Bild: {:.2} ms", t0.elapsed().as_secs_f64() * 1e3 / 50.0);
}

/// One clip without proxy (`CUTLINE_HOME` on an empty folder): reverse (frames/s, `neu` =
/// frames with new content) or forwards (Play → first frame). `CUTLINE_H4_FILES=a;b`,
/// `CUTLINE_H4_MODE=rev|fwd`, `CUTLINE_QA_AT`, `CUTLINE_QA_SECS` (for frames/s ≥ 25: quiet part 2–10 s)
/// `cargo test --release -p cl-app --bin cutline h4_measure -- --ignored --nocapture`
#[test]
#[ignore]
fn h4_measure() {
    let files = std::env::var("CUTLINE_H4_FILES").unwrap_or_default();
    let rev = std::env::var("CUTLINE_H4_MODE").as_deref() == Ok("rev");
    let secs: f64 = std::env::var("CUTLINE_QA_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(8.0);
    let at: f64 = std::env::var("CUTLINE_QA_AT").ok().and_then(|s| s.parse().ok()).unwrap_or(5.0);
    for f in files.split(';').filter(|s| !s.is_empty()) {
        let m = cl_media::probe(std::path::Path::new(f)).unwrap();
        let mut p = Project::new("h4");
        p.settings.fps = Rational::new(60, 1);
        p.settings.width = 1920;
        p.settings.height = 1080;
        let mut v1 = cl_core::Track::new(TrackKind::Video);
        v1.main = true;
        let d = m.duration.as_secs_f64().min(60.0);
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(d));
        c.fx.reverse = rev;
        v1.clips.push(c);
        p.timeline.tracks = vec![v1];
        p.media = vec![m];
        let r = drive(p, &format!("{} {f}", if rev { "rev" } else { "fwd" }), Time::from_secs_f64(at), secs);
        eprintln!(
            "H4 | {} | proxy {} | Play→Bild {:.0} ms | Play→bewegt {:.0} ms | fps {:.1} neu {:.1} | Lücke {:.0} ms | Einzelbilder {}/{} | Einzelbild max {:.0} ms | Stufe {} | Klick→Bild {:.0} ms",
            r.label, r.proxy, r.first_ms, r.moving_ms, r.fps, r.fps_new, r.gap_ms, r.stills_req, r.stills_shown, r.still_max_ms, r.level_max, r.click_ms
        );
    }
}
