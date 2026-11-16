//! Mess-Test ohne Fenster: Player-Kern mit echten FFmpeg-Strömen (Starter-Thread), echtem
//! Einzelbild-Decoder und Ton ohne Gerät (dieselben Ton-Fenster wie in der App, nur nicht hörbar).
//! Je Projekt fünf Stellen, je Stelle `CUTLINE_QA_SECS` (Standard 30) Sekunden abspielen, nach 10 s
//! eine Bearbeitung (Effekt + Lautstärke), nach 20 s ein Klick an eine andere Stelle.
//!
//! Von Hand ausführen (Projekte `PL_einfach/PL_schnitt/PL_gross.cutline` im Ordner):
//! `CUTLINE_QA_DIR=<Ordner> cargo test --release -p cl-app --bin cutline play_measure -- --ignored --nocapture`
//! Proxys werden genutzt, wenn sie im Datenordner (`CUTLINE_HOME`) liegen – wie in der App.
//! Dauerlauf (Speicher): `CUTLINE_QA_SECS=150 CUTLINE_QA_JUMP=8` springt nach dem Klick alle 8 s
//! und gibt die Größe aller Zwischenspeicher nach dem Anlauf und am Ende aus (`mem`).

use std::time::Instant;

use eframe::egui;

use super::*;
use crate::audio::AudioPlayer;
use crate::playcore::{quality_steps, AudioIn, Cmd, Mode, PlayCore, Tick, Want};
use crate::preview::{Decoder, Done, Job, Starter};

/// Messwerte eines Laufs
#[derive(Debug, Default)]
struct Run {
    label: String,
    proxy: bool,
    /// Play → erstes Bild, Play → erstes weitergeschaltetes Bild (ms)
    first_ms: f64,
    moving_ms: f64,
    /// Bilder/s der untersten Video-Ebene im ruhigen Teil (2 s … Bearbeitung)
    fps: f64,
    target_fps: f64,
    /// größte Lücke zwischen zwei Bildern der untersten Video-Ebene nach dem Anlauf (ms)
    gap_ms: f64,
    dropped_per_s: f64,
    /// größter / mittlerer Rückstand des gezeigten Strombilds hinter der Uhr (= Bild-Ton-Versatz)
    lag_max_ms: f64,
    lag_avg_ms: f64,
    underruns: u64,
    underrun_ms: f64,
    catch_ups: u32,
    level_max: usize,
    /// Bearbeitung beim Abspielen: größte Lücke danach, bis das geänderte Bild zu sehen ist (ms)
    edit_gap_ms: f64,
    /// Regler ziehen beim Abspielen: größte Lücke der untersten Ebene (ms)
    drag_gap_ms: f64,
    edit_visible_ms: f64,
    /// Klick beim Abspielen → erstes Bild an der neuen Stelle (ms)
    click_ms: f64,
    clock_audio: bool,
    /// Einzelbilder beim Abspielen: angefordert / gezeigt
    stills_req: u32,
    stills_shown: u32,
    /// größte Zeit ohne neues Bild irgendeiner Ebene bei laufender Uhr (ms)
    freeze_ms: f64,
    /// längste Zeit Auftrag → Einzelbild (ms)
    still_max_ms: f64,
    /// in den 3 s nach Pause → Play an derselben Stelle: geöffnete Ströme, höchste Qualitätsstufe, Aufholen
    resume_opens: u32,
    resume_level: usize,
    resume_catch_up: u32,
    /// gezeigte Bilder je Ebene (Strom oder Einzelbild), Reihenfolge der Ebenen im Projekt
    layer_frames: std::collections::BTreeMap<String, u32>,
    /// Zwischenspeicher nach dem Anlauf und am Ende (Dauerlauf, `CUTLINE_QA_JUMP`)
    mem: Vec<String>,
}

/// Privater Speicher des Prozesses (MB), soweit ermittelbar
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
        // SAFETY: Struktur in der Größe von PROCESS_MEMORY_COUNTERS_EX
        if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) } != 0 {
            return c.private as f64 / 1048576.0;
        }
    }
    f64::NAN
}

/// Alle Zwischenspeicher des Players (Kern, Verteiler, Starter-Warteschlange) und Prozess-Speicher
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

/// Unterste Video-Ebene am Playhead (nicht vorab, keine Element-Fassung)
fn primary(needs: &[Need]) -> Option<ClipId> {
    needs
        .iter()
        .find(|n| n.prefetch.is_none() && n.overlay.is_none() && matches!(n.clip.content, ClipContent::Media { kind: MediaKind::Video, .. }))
        .map(|n| n.id)
}

fn clip_mut(p: &mut Project, id: ClipId) -> Option<&mut Clip> {
    p.timeline.tracks.iter_mut().flat_map(|t| t.clips.iter_mut()).find(|c| c.id == id)
}

#[allow(clippy::too_many_lines)]
fn drive(mut project: Project, label: &str, start: Time, secs: f64) -> Run {
    let ctx = egui::Context::default();
    let decoder = Decoder::new(ctx.clone());
    let starter = Starter::new(ctx);
    let mut core: PlayCore<cl_media::FrameStream> = PlayCore::default();
    // Vergleich „vorher“: ohne gezieltes Aufholen und ohne automatische Qualität
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
    // Bildfolge der untersten Video-Ebene: (Wandzeit)
    let mut prim_frames: Vec<f64> = Vec::new();
    let mut lags: Vec<f64> = Vec::new();
    let mut dropped = 0u32;
    let mut first_any: Option<f64> = None;
    let mut moving: Option<f64> = None;
    let mut warm_until = f64::MAX; // Lücken erst nach dem Anlauf zählen
    let mut any_frame = f64::NAN;
    // Dauerlauf: alle so viele Sekunden ein Sprung (zusätzlich zum Klick)
    let jump_every: Option<f64> = std::env::var("CUTLINE_QA_JUMP").ok().and_then(|s| s.parse().ok());
    let mut last_jump = 0.0;
    let mut starter_queue = 0i64;
    let debug = std::env::var_os("CUTLINE_QA_DEBUG").is_some();
    let mut still_asked: std::collections::HashMap<u64, (f64, f64)> = Default::default();
    let drag_at = (edit_at + click_at) / 2.0;
    // `CUTLINE_QA_PAUSE`: Pause und Play an derselben Stelle (vor der Bearbeitung); danach dürfen
    // weder Ströme neu aufgehen noch die Qualität sinken (verfälscht die Bilder/s des ruhigen Teils)
    let pause_at = if std::env::var_os("CUTLINE_QA_PAUSE").is_some() { edit_at * 0.5 } else { f64::MAX };
    let mut paused_since: Option<f64> = None;
    let mut resumed: Option<f64> = None;
    let mut dragging = false;
    core.play(start, revision, now());
    let t_play = now();
    while now() - t_play < secs {
        let t = now();
        let rel = t - t_play;
        // Bearbeitung: Helligkeit der untersten Ebene + Lautstärke (sichtbar und hörbar)
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
        // Regler ziehen (Inspektor): 1,5 s lang je UI-Bild ein neuer Effektwert und eine neue
        // Größe der untersten Ebene, erst danach ein Schritt im Verlauf
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
        // Klick an eine andere Stelle
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
            break; // Projektende
        }
        run.clock_audio |= matches!(ain, AudioIn::Running(_));
        run.level_max = run.level_max.max(core.quality.level);
        while let Ok((o, f, alias, s)) = starter.rx.try_recv() {
            starter_queue -= 1;
            match s {
                Some(s) => {
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
                Done::Frame { clip, key, .. } => {
                    if let Some((ta, src)) = still_asked.remove(&key) {
                        let ms = (t - ta) * 1000.0;
                        if debug && ms > 700.0 {
                            eprintln!("Einzelbild {ms:.0} ms bis {rel:.2} s (Quelle {src:.2} s)");
                        }
                        run.still_max_ms = run.still_max_ms.max(ms);
                    }
                    if core.still_done(lid(clip), key) {
                        run.stills_shown += 1;
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
                if edited.is_some_and(|e| e.1 == n.id) && edit_key.is_none() {
                    edit_key = Some((n.id, key));
                }
                let alias = stream_alias(c, m, (w, h), sfps, fx_hash_of(c));
                let want = Want { id: lid(n.id), key, start: n.prefetch.unwrap_or(playhead), end: c.end(), prefetch: n.prefetch.is_some(), alias };
                if let Some(o) = core.want(&want, t) {
                    let p = path();
                    run.proxy |= p != m.path;
                    let j = open_job(o, c, m, p, (w, h), sfps, alias);
                    if std::env::var_os("CUTLINE_QA_DEBUG").is_some() {
                        eprintln!("öffnen ({rel:.2} s, Stufe {}) {:.2} s: {} ab {:.3} {}×{} hw={:?} graph={:?}", core.quality.level, o.at.as_secs_f64(), j.path.display(), j.source_at.as_secs_f64(), w, h, j.hw, j.graph);
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
                    decoder.send(Job::Frame { clip: n.id, key, path: path(), at: src, w, h, graph });
                }
            }
        }
        core.visible(&ids);
        // Stillstand: Uhr läuft, aber keine Ebene bekommt ein neues Bild (nicht während Anlauf/Klick)
        let in_click = clicked.is_some_and(|c| c.0 <= t && run.click_ms == 0.0);
        if core.mode() == Mode::Spielen && t >= warm_until && any_frame.is_finite() && !in_click {
            run.freeze_ms = run.freeze_ms.max((t - any_frame) * 1000.0);
        }
        if core.mode() == Mode::Spielen {
            if let Some(l) = core.av_offset_ms() {
                lags.push(l);
            }
        }
        // Klick → erstes Bild an der neuen Stelle
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
    // Effekte und Übergänge der mitgelieferten Pakete (wie beim Start der App)
    let reg = cl_plugin::Registry::scan([std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins"))]);
    cl_fx::catalog::load_packs(crate::contents::catalog_packs(&reg, "de"));
    // künstliche Volllast: so viele rechnende Threads (wie andere Programme auf dem Rechner)
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
        let project = Project::from_json(&json).expect("Projekt lesbar");
        // Messung mit Proxys: fehlende fertig bauen (in `CUTLINE_HOME`, nicht im Datenordner der App)
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
    // großzügige Grenzwerte (Fremdlast auf dem Rechner darf nicht stören); im Dauerlauf mit
    // Sprüngen zählt jeder Anlauf als Rückstand – dort nur die Zwischenspeicher ansehen
    if std::env::var_os("CUTLINE_QA_JUMP").is_some() {
        return;
    }
    for r in &rows {
        assert!(r.first_ms < 3000.0, "{}: Play → Bild {:.0} ms", r.label, r.first_ms);
        assert!(r.lag_max_ms < 1500.0, "{}: Rückstand {:.0} ms", r.label, r.lag_max_ms);
        assert!(r.fps >= 0.5 * r.target_fps.min(30.0), "{}: {:.1} Bilder/s", r.label, r.fps);
        assert!(r.click_ms > 0.0 && r.click_ms < 3000.0, "{}: Klick → Bild {:.0} ms", r.label, r.click_ms);
    }
}

/// Kosten der Player-Logik je UI-Bild ohne Zeichnen (Ebenen, Bedarf, Schlüssel) und je Strombild
/// (Umwandlung für die Textur), an vielen Stellen der Testprojekte.
/// Zwei 4K-Ebenen, die obere mit Tempo `CUTLINE_QA_TEMPO` (Standard 2; `rev` = rückwärts,
/// `kurve` = Tempo-Kurve, `scale` = Größen-Keyframes): beide Ebenen müssen fortlaufend Bilder
/// zeigen (Nachtest K7: die Tempo-2-Ebene blieb ein Standbild).
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
