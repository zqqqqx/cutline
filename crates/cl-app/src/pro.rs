//! Phase 3 UI: automatic captions + editor, background removal,
//! chroma key with eyedropper, stabilization, motion tracking, auto reframe,
//! scene detection, audio tools, voice filters, beats and model management.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use cl_core::*;
use eframe::egui::{self, vec2, Color32, Rect, RichText, Stroke};

use crate::app::App;
use crate::player_edit::Region;
use crate::tasks::TaskDone;
use crate::theme::{self, icon};
use crate::widgets::{self, Edit, Slider};

/// Selection in the player for a tool.
#[derive(Clone, Copy, PartialEq)]
pub enum Pick {
    /// Pick a color for the chroma key
    Chroma(ClipId),
    /// Draw a frame around the object to track (overlay, video below)
    Track { clip: ClipId, target: ClipId },
    /// Draw a frame around the subject to censor
    Censor(ClipId),
}

/// Running drag of a censor area in the player (display position at press).
#[derive(Clone, Copy)]
pub struct CensorDrag {
    clip: ClipId,
    i: usize,
    scale: bool,
    press: egui::Pos2,
    start: CensorKey,
}

fn is_video(c: &Clip) -> bool {
    matches!(c.content, ClipContent::Media { kind: MediaKind::Video, .. })
}

/// Source frame ↔ display (mirrored as in the player); the same computation in both directions.
/// Task key of tracking an area (via its ID, not the list position).
fn censor_task(clip: ClipId, id: CensorId) -> String {
    format!("censor:{clip}:{}", id.0)
}

fn censor_flip(c: &Clip, k: CensorKey) -> CensorKey {
    let f = |v: f32, on: bool| if on { 1.0 - v } else { v };
    CensorKey { cx: f(k.cx, c.fx.flip_h), cy: f(k.cy, c.fx.flip_v), ..k }
}

pub const LANGS: [(&str, &str); 8] = [
    ("auto", "Automatisch"),
    ("de", "Deutsch"),
    ("en", "Englisch"),
    ("fr", "Französisch"),
    ("es", "Spanisch"),
    ("it", "Italienisch"),
    ("nl", "Niederländisch"),
    ("pl", "Polnisch"),
];

fn task_bar(ui: &mut egui::Ui, app: &App, key: &str) -> bool {
    if let Some(t) = app.tasks.running(key) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2((ui.available_width() - 36.0).max(20.0), 3.0), egui::Sense::hover());
            widgets::task_line(ui.painter(), egui::Id::new(("task", t.id)), r, t.progress());
            if theme::icon_button(ui, icon::X, "Abbrechen", true, false).clicked() {
                t.cancel();
            }
        });
        ui.label(
            RichText::new(crate::tr(&t.label))
                .small()
                .color(theme::TEXT_DIM()),
        );
        return true;
    }
    false
}

/// Way of cutting out, derived from the clip (not stored). Stage 3 adds `Object` here and in `ALL`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum CutMode {
    Off,
    Person,
    Color,
}

impl CutMode {
    const ALL: [(CutMode, &'static str); 3] = [(CutMode::Off, "Aus"), (CutMode::Person, "Person"), (CutMode::Color, "Farbe")];

    /// `matte_running`: a mask job runs for the clip. Older projects with mask and color key = Person.
    fn of(fx: &ClipFx, matte_running: bool) -> Self {
        if fx.cutout.is_some() || matte_running {
            CutMode::Person
        } else if fx.chroma.is_some() {
            CutMode::Color
        } else {
            CutMode::Off
        }
    }

    /// Choose this way: the fields of the other ways go (one undo step). Person: the mask
    /// comes from `App::start_matte`; Off also drops background effect and edge.
    fn apply(self, fx: &mut ClipFx) {
        match self {
            CutMode::Off => {
                fx.cutout = None;
                fx.chroma = None;
                fx.person = None;
                fx.edge = MatteEdge::default();
            }
            CutMode::Person => fx.chroma = None,
            CutMode::Color => {
                fx.cutout = None;
                fx.chroma.get_or_insert_with(ChromaKey::default);
            }
        }
    }
}

fn matte_cache(path: &std::path::Path) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    if let Ok(m) = std::fs::metadata(path) {
        m.len().hash(&mut h);
        m.modified().ok().hash(&mut h);
    }
    // not in the cache: the project points to the mask permanently
    crate::paths::generated_dir()
        .join("matte")
        .join(format!("{:016x}.mkv", h.finish()))
}

/// Selectable speech recognizers (ID, name) and the valid choice: „auto“ and every
/// installed recognizer (Whisper only with the FFmpeg filter); a remembered choice that no
/// longer exists counts as „auto“.
pub(crate) fn asr_offer(
    parakeet: bool,
    whisper: Option<String>,
    filter: bool,
    saved: &str,
) -> (Vec<(&'static str, String)>, &'static str) {
    let mut v = vec![("auto", "Automatisch".to_string())];
    if parakeet {
        v.push(("parakeet", cl_analyze::asr::Engine::Parakeet.name().to_string()));
    }
    if let Some(w) = whisper.filter(|_| filter) {
        v.push(("whisper", w));
    }
    let cur = v.iter().find(|c| c.0 == saved).map_or("auto", |c| c.0);
    (v, cur)
}

/// Chosen recognizer as the default for [`cl_analyze::asr::transcribe`] (`auto` = the best).
pub(crate) fn asr_force(id: &str) -> Option<cl_analyze::asr::Engine> {
    match id {
        "parakeet" => Some(cl_analyze::asr::Engine::Parakeet),
        "whisper" => Some(cl_analyze::asr::Engine::Whisper),
        _ => None,
    }
}

impl App {
    pub fn models(&self) -> cl_analyze::Models {
        cl_analyze::Models::new(crate::paths::models_dir())
    }

    /// Speech recognition offer for this machine (see [`asr_offer`]).
    fn asr_choices(&self, ctx: &egui::Context) -> (Vec<(&'static str, String)>, &'static str) {
        let m = self.models();
        let filter = cl_analyze::asr::whisper_filter_available();
        if filter.is_none() {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
        asr_offer(
            m.installed("parakeet") && m.installed("onnxruntime"),
            m.whisper().map(|p| cl_analyze::asr::whisper_label(&p)),
            filter.unwrap_or(false),
            &self.settings.asr_engine,
        )
    }

    fn need_models(&mut self, ids: &[&str]) -> bool {
        let m = self.models();
        let missing: Vec<&str> = ids.iter().copied().filter(|i| !m.installed(i)).collect();
        if missing.is_empty() {
            return true;
        }
        let names: Vec<&str> = missing
            .iter()
            .filter_map(|i| {
                cl_analyze::CATALOG
                    .iter()
                    .find(|c| c.id == *i)
                    .map(|c| c.name)
            })
            .collect();
        self.toast_err(format!(
            "Benötigt: {} (Einstellungen → Modelle)",
            names.join(", ")
        ));
        self.settings_section = crate::settings::Section::Models;
        self.settings_open = true;
        false
    }

    // ------------------------------------------------------------ results

    pub fn poll_tasks(&mut self) {
        for (_, done) in self.tasks.poll() {
            match done {
                TaskDone::Failed(e) => self.toast_err(e),
                TaskDone::Run(f) => f(self),
                // done: nothing new – streams switch via their key (see `stream_pick`),
                // without discarding the current frame
                TaskDone::Proxy { media, ok } => {
                    self.proxy_finished(media, ok);
                    if !ok {
                        self.proxy_failed.insert(media);
                    }
                }
                TaskDone::Model { id } => {
                    let n = cl_analyze::CATALOG
                        .iter()
                        .find(|c| c.id == id)
                        .map(|c| c.name)
                        .unwrap_or(id);
                    self.toast(format!("{n} installiert"));
                }
                TaskDone::Captions {
                    segments,
                    offset,
                    engine,
                } => self.insert_captions(&segments, offset, engine),
                TaskDone::Matte { clip, out } => {
                    if let Some(mut c) = self.clip_view(clip) {
                        c.fx.cutout = Some(out.to_string_lossy().into_owned());
                        self.live = None;
                        self.apply(EditOp::ReplaceClip { clip: c });
                        self.toast("Hintergrund entfernt");
                    }
                }
                TaskDone::Scenes { clip, times } => {
                    let Some(c) = self.clip_view(clip) else {
                        continue;
                    };
                    let mut cuts: Vec<Time> = times
                        .iter()
                        .map(|t| {
                            (c.start + Time::from_secs_f64(t / c.speed.max(0.01) as f64))
                                .snap_to_frame(self.project.settings.fps)
                        })
                        .filter(|t| *t > c.start && *t < c.end())
                        .collect();
                    cuts.sort();
                    cuts.dedup();
                    if cuts.is_empty() {
                        self.toast("Keine Szenenwechsel gefunden");
                        continue;
                    }
                    let n = cuts.len();
                    let ops = cuts
                        .into_iter()
                        .rev()
                        .map(|at| EditOp::SplitClip { clip, at })
                        .collect();
                    self.apply_batch(ops);
                    self.toast(format!("{} Szenen erkannt und geteilt", n + 1));
                }
                TaskDone::Beats { clip, times, bpm } => {
                    if let Some(mut c) = self.clip_view(clip) {
                        let base = c.source_time(c.start);
                        c.beats = times
                            .iter()
                            .map(|t| base + Time::from_secs_f64(*t))
                            .collect();
                        let n = c.beats.len();
                        self.live = None;
                        self.apply(EditOp::ReplaceClip { clip: c });
                        self.toast(format!("{n} Beats erkannt ({bpm:.0} BPM)"));
                    }
                }
                TaskDone::Track {
                    clip,
                    target,
                    points,
                    from,
                } => self.apply_tracking(clip, target, &points, from),
                TaskDone::Reframe { clip, points } => self.apply_reframe(clip, &points),
            }
        }
    }

    fn insert_captions(&mut self, segs: &[cl_analyze::asr::Segment], offset: Time, engine: &str) {
        if segs.is_empty() {
            self.toast_err("Keine Sprache erkannt");
            return;
        }
        // Whisper returns finished lines: bring them to the preset's length
        let segs = if engine == "Whisper" {
            let p = self.caption_preset();
            cl_analyze::asr::resegment(segs, p.max_chars, p.max_dur)
        } else {
            segs.to_vec()
        };
        let n = self.add_caption_segments(&segs, offset);
        self.toast(format!("{n} Untertitel erzeugt ({engine})"));
    }

    fn apply_tracking(
        &mut self,
        clip: ClipId,
        target: ClipId,
        pts: &[(f64, f32, f32)],
        from: Time,
    ) {
        let (Some(mut c), Some(tc)) = (self.clip_view(clip), self.clip_view(target)) else {
            return;
        };
        let ClipContent::Media { media, .. } = tc.content else {
            return;
        };
        let Some(m) = self.project.media(media) else {
            return;
        };
        let (w, h) = (
            self.project.settings.width as f32,
            self.project.settings.height as f32,
        );
        let k = (w / m.width.max(1) as f32).min(h / m.height.max(1) as f32);
        let ev = tc.eval_at(Time((from - tc.start).0.max(0)), w, h);
        let (fw, fh) = (
            m.width as f32 * k * ev.scale,
            m.height as f32 * k * ev.scale,
        );
        let to_canvas = |nx: f32, ny: f32| (ev.x + (nx - 0.5) * fw, ev.y + (ny - 0.5) * fh);
        let Some(first) = pts.first() else { return };
        let (cx0, cy0) = to_canvas(first.1, first.2);
        let start_rel = Time((from - c.start).0.max(0));
        let e0 = c.eval_at(start_rel, w, h);
        let (ox, oy) = (e0.x - cx0, e0.y - cy0);
        c.keyframes.0.remove(&Prop::X);
        c.keyframes.0.remove(&Prop::Y);
        for (i, (t, nx, ny)) in pts.iter().enumerate() {
            if i % 2 != 0 && i + 1 != pts.len() {
                continue;
            }
            let tl = start_rel + Time::from_secs_f64(t / tc.speed.max(0.01) as f64);
            if tl > c.duration {
                break;
            }
            let (x, y) = to_canvas(*nx, *ny);
            c.keyframes.set(Prop::X, tl, x + ox);
            c.keyframes.set(Prop::Y, tl, y + oy);
        }
        for list in c.keyframes.0.values_mut() {
            for kf in list.iter_mut() {
                kf.ease = Ease::Linear;
            }
        }
        self.live = None;
        self.apply(EditOp::ReplaceClip { clip: c });
        self.toast(format!("Bewegung verfolgt ({} Punkte)", pts.len()));
    }

    fn apply_reframe(&mut self, clip: ClipId, pts: &[(f64, f32, f32)]) {
        self.apply_reframes(&[(clip, pts.to_vec())]);
    }

    /// Apply the results of several reframe analyses as one undo step.
    pub fn apply_reframes(&mut self, list: &[(ClipId, Vec<(f64, f32, f32)>)]) {
        let ops: Vec<EditOp> = list
            .iter()
            .filter_map(|(id, pts)| self.reframed(*id, pts))
            .map(|clip| EditOp::ReplaceClip { clip })
            .collect();
        if ops.is_empty() {
            return;
        }
        self.live = None;
        self.apply_batch(ops);
        self.toast("Auto-Reframe angewendet");
    }

    fn reframed(&self, clip: ClipId, pts: &[(f64, f32, f32)]) -> Option<Clip> {
        let mut c = self.clip_view(clip)?;
        let ClipContent::Media { media, .. } = c.content else {
            return None;
        };
        let m = self.project.media(media)?;
        let (w, h) = (
            self.project.settings.width as f32,
            self.project.settings.height as f32,
        );
        let k = (w / m.width.max(1) as f32).min(h / m.height.max(1) as f32);
        let (fw, fh) = (m.width as f32 * k, m.height as f32 * k);
        c.transform.scale = (w / fw).max(h / fh);
        c.keyframes.0.remove(&Prop::Scale);
        crate::shorts::follow_keys(&mut c, m, (w, h), pts);
        Some(c)
    }

    // ------------------------------------------------------------ starting

    pub fn start_captions(&mut self, ctx: &egui::Context, only_clip: Option<ClipId>) {
        if !self.models().installed("parakeet") && !self.models().installed("whisper") {
            self.need_models(&["parakeet"]);
            return;
        }
        if self.models().installed("parakeet")
            && !self.models().installed("onnxruntime")
            && !self.models().installed("whisper")
        {
            self.need_models(&["onnxruntime"]);
            return;
        }
        let models = self.models();
        let pre = self.caption_preset();
        let opt = cl_analyze::asr::Options {
            language: self.caption_lang.clone(),
            max_chars: pre.max_chars,
            max_dur: pre.max_dur,
            force: asr_force(self.asr_choices(ctx).1),
        };
        match only_clip.and_then(|c| self.clip_view(c)) {
            Some(c) => {
                let ClipContent::Media { media, .. } = c.content else {
                    return;
                };
                let Some(m) = self.project.media(media).cloned() else {
                    return;
                };
                let (start, span, offset, speed) = (
                    c.source_time(c.start).as_secs_f64(),
                    c.source_span().as_secs_f64(),
                    c.start,
                    c.speed.max(0.01) as f64,
                );
                self.tasks.spawn(
                    ctx,
                    "captions",
                    "Untertitel werden erkannt …",
                    move |p, cancel| match cl_analyze::asr::transcribe(
                        &models, &m.path, start, span, &opt, p, cancel,
                    ) {
                        Ok((eng, mut segs)) => {
                            for s in &mut segs {
                                s.start /= speed;
                                s.end /= speed;
                                s.words.iter_mut().for_each(|w| *w /= speed);
                            }
                            TaskDone::Captions {
                                segments: segs,
                                offset,
                                engine: eng.name(),
                            }
                        }
                        Err(e) => TaskDone::Failed(format!("Untertitel: {e}")),
                    },
                );
            }
            None => {
                let project = self.project.clone();
                if project.duration() <= Time::ZERO {
                    self.toast_err("Das Projekt ist leer");
                    return;
                }
                self.tasks.spawn(
                    ctx,
                    "captions",
                    "Untertitel werden erkannt …",
                    move |p, cancel| {
                        // mix the audio of the whole timeline
                        let dir = std::env::temp_dir()
                            .join(format!("cutline-asr-{}", std::process::id()));
                        let _ = std::fs::create_dir_all(&dir);
                        let wav = dir.join("mix.wav");
                        let mut s = cl_export::ExportSettings::from_project(&project);
                        s.format = cl_export::Format::Wav;
                        let none = |_: Option<&str>| None;
                        let res = cl_export::build(&project, &s, &none, &wav)
                            .map_err(|e| e.to_string())
                            .and_then(|job| {
                                cl_export::run(
                                    &job,
                                    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                                    |x| p(x * 0.1),
                                )
                                .map_err(|e| e.to_string())
                            })
                            .and_then(|_| {
                                let dur = project.duration().as_secs_f64();
                                cl_analyze::asr::transcribe(
                                    &models,
                                    &wav,
                                    0.0,
                                    dur,
                                    &opt,
                                    &|x| p(0.1 + 0.9 * x),
                                    cancel,
                                )
                            });
                        let _ = std::fs::remove_dir_all(&dir);
                        match res {
                            Ok((eng, segs)) => TaskDone::Captions {
                                segments: segs,
                                offset: Time::ZERO,
                                engine: eng.name(),
                            },
                            Err(e) => TaskDone::Failed(format!("Untertitel: {e}")),
                        }
                    },
                );
            }
        }
    }

    /// Person mask: a finished one from the cache goes into `c` (the caller commits), otherwise
    /// the job starts and sets it when done.
    fn start_matte(&mut self, ctx: &egui::Context, c: &mut Clip) {
        let ClipContent::Media {
            media,
            kind: MediaKind::Video,
        } = c.content
        else {
            self.toast_err("Hintergrund entfernen geht bei Videoclips");
            return;
        };
        let Some(m) = self.project.media(media).cloned() else {
            return;
        };
        let out = matte_cache(&m.path);
        if out.is_file() {
            c.fx.cutout = Some(out.to_string_lossy().into_owned());
            return;
        }
        if !self.need_models(&["onnxruntime", "rvm"]) {
            return;
        }
        let models = self.models();
        let id = c.id;
        self.tasks.spawn(
            ctx,
            format!("matte:{id}"),
            format!(
                "Hintergrund wird entfernt: {}",
                m.path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            move |p, cancel| match cl_analyze::matte::matte(&models, &m.path, &out, p, cancel) {
                Ok(()) => TaskDone::Matte { clip: id, out },
                Err(e) => TaskDone::Failed(format!("Freistellen: {e}")),
            },
        );
    }

    fn start_simple_analysis(&mut self, ctx: &egui::Context, c: &Clip, what: &'static str) {
        let ClipContent::Media { media, .. } = c.content else {
            return;
        };
        let Some(m) = self.project.media(media).cloned() else {
            return;
        };
        let (start, span, id) = (
            c.source_time(c.start).as_secs_f64(),
            c.source_span().as_secs_f64(),
            c.id,
        );
        let (key, label) = match what {
            "scenes" => (format!("scenes:{id}"), "Szenen werden erkannt …"),
            "beats" => (format!("beats:{id}"), "Beats werden erkannt …"),
            _ => (format!("reframe:{id}"), "Auto-Reframe analysiert …"),
        };
        self.tasks
            .spawn(ctx, key, label, move |p, cancel| match what {
                "scenes" => match cl_analyze::scenes::detect(&m.path, start, span, 0.5) {
                    Ok(times) => TaskDone::Scenes { clip: id, times },
                    Err(e) => TaskDone::Failed(e),
                },
                "beats" => match cl_analyze::beats::detect(&m.path, start, span) {
                    Ok(b) => TaskDone::Beats {
                        clip: id,
                        times: b.times,
                        bpm: b.bpm,
                    },
                    Err(e) => TaskDone::Failed(e),
                },
                _ => match cl_analyze::reframe::focus_path(
                    &m.path, m.width, m.height, start, span, p, cancel,
                ) {
                    Ok(points) => TaskDone::Reframe { clip: id, points },
                    Err(e) => TaskDone::Failed(e),
                },
            });
    }

    /// Video clip under an overlay at the playhead (for „Bewegung verfolgen“).
    fn video_below(&self, clip: ClipId) -> Option<ClipId> {
        let ti = self
            .project
            .timeline
            .tracks
            .iter()
            .position(|t| t.clips.iter().any(|c| c.id == clip))?;
        self.project.timeline.tracks[..ti]
            .iter()
            .rev()
            .filter(|t| t.kind == TrackKind::Video && !t.hidden)
            .flat_map(|t| t.clips.iter())
            .find(|c| {
                c.contains(self.playhead)
                    && matches!(
                        c.content,
                        ClipContent::Media {
                            kind: MediaKind::Video,
                            ..
                        }
                    )
            })
            .map(|c| c.id)
    }

    // ------------------------------------------------------------ inspector

    /// „Smart“ tab (video/image); for text only „Bewegung verfolgen“ in the „Basis“ tab.
    pub fn pro_visual(&mut self, ui: &mut egui::Ui, c: &mut Clip, e: &mut Edit) {
        let ctx = ui.ctx().clone();
        let is_video = matches!(
            c.content,
            ClipContent::Media {
                kind: MediaKind::Video,
                ..
            }
        );
        let is_media = matches!(c.content, ClipContent::Media { .. });
        let on_main = self
            .project
            .timeline
            .tracks
            .iter()
            .any(|t| t.main && t.clips.iter().any(|x| x.id == c.id));
        if is_media {
            theme::section(ui, "Freistellen");
            let task = format!("matte:{}", c.id);
            let mode = CutMode::of(&c.fx, self.tasks.running(&task).is_some());
            let mut sel = CutMode::ALL.iter().position(|m| m.0 == mode).unwrap_or(0);
            let names = CutMode::ALL.map(|m| m.1);
            let enabled = CutMode::ALL.map(|m| m.0 != CutMode::Person || is_video);
            if widgets::segmented_some(ui, &names, &mut sel, &enabled) && CutMode::ALL[sel].0 != mode {
                let new = CutMode::ALL[sel].0;
                if let Some(t) = self.tasks.running(&task).filter(|_| new != CutMode::Person) {
                    t.cancel();
                }
                new.apply(&mut c.fx);
                if new == CutMode::Person {
                    self.start_matte(&ctx, c);
                }
                e.commit = true;
            }
            if mode == CutMode::Person {
                task_bar(ui, self, &task);
                // older projects: mask and color key at once
                if c.fx.chroma.is_some() {
                    let x = theme::row(ui, "Farbe zusätzlich aktiv", |ui| {
                        theme::icon_button(ui, icon::X, "Farbschlüssel entfernen", true, false).clicked()
                    });
                    if x {
                        c.fx.chroma = None;
                        e.commit = true;
                    }
                }
            }
            if let Some(k) = c.fx.chroma.as_mut().filter(|_| mode == CutMode::Color) {
                theme::row(ui, "Farbe", |ui| {
                    let mut rgba = [k.color[0], k.color[1], k.color[2], 255];
                    if widgets::color_button(ui, &mut rgba) {
                        k.color = [rgba[0], rgba[1], rgba[2]];
                        e.commit = true;
                    }
                    let picking = self.pick == Some(Pick::Chroma(c.id));
                    if theme::icon_button(
                        ui,
                        icon::EYEDROPPER,
                        "Farbe im Player aufnehmen",
                        true,
                        picking,
                    )
                    .clicked()
                    {
                        self.pick = if picking {
                            None
                        } else {
                            Some(Pick::Chroma(c.id))
                        };
                    }
                });
                *e |= theme::row(ui, "Toleranz", |ui| {
                    Slider::new(&mut k.similarity, 0.01..=0.6)
                        .percent()
                        .default(0.18)
                        .show(ui)
                });
                *e |= theme::row(ui, "Weiche Kante", |ui| {
                    Slider::new(&mut k.blend, 0.0..=0.5)
                        .percent()
                        .default(0.08)
                        .show(ui)
                });
                *e |= theme::row(ui, "Farbsaum", |ui| {
                    Slider::new(&mut k.spill, 0.0..=1.0)
                        .percent()
                        .default(0.5)
                        .show(ui)
                });
            }
            if mode != CutMode::Off {
                // shared by all ways: what happens to the background, then the mask edge
                let mut sel = c.fx.person.as_ref().map(|p| p.index() + 1).unwrap_or(0);
                let items = [
                    "Transparent",
                    PersonFx::KINDS[0],
                    PersonFx::KINDS[1],
                    PersonFx::KINDS[2],
                ];
                theme::row(ui, "Hintergrund", |ui| {
                    egui::ComboBox::from_id_salt(("person", c.id))
                        .selected_text(crate::tr(items[sel]))
                        .width((ui.available_width() - 12.0).max(60.0))
                        .show_ui(ui, |ui| {
                            for (i, it) in items.iter().enumerate() {
                                if ui.selectable_value(&mut sel, i, crate::tr(*it)).changed() {
                                    c.fx.person = (i > 0).then(|| PersonFx::from_index(i - 1));
                                    e.commit = true;
                                }
                            }
                        });
                });
                match &mut c.fx.person {
                    Some(PersonFx::BlurBackground { amount }) => {
                        *e |= theme::row(ui, "Unschärfe", |ui| {
                            Slider::new(amount, 0.0..=1.0)
                                .percent()
                                .default(0.6)
                                .show(ui)
                        });
                    }
                    Some(PersonFx::ColorBackground { color }) => {
                        theme::row(ui, "Farbe", |ui| {
                            let mut rgba = [color[0], color[1], color[2], 255];
                            if widgets::color_button(ui, &mut rgba) {
                                *color = [rgba[0], rgba[1], rgba[2]];
                                e.commit = true;
                            }
                        });
                    }
                    Some(PersonFx::Outline { color, width, transparent }) => {
                        theme::row(ui, "Farbe", |ui| {
                            let mut rgba = [color[0], color[1], color[2], 255];
                            if widgets::color_button(ui, &mut rgba) {
                                *color = [rgba[0], rgba[1], rgba[2]];
                                e.commit = true;
                            }
                        });
                        *e |= theme::row(ui, "Breite", |ui| {
                            Slider::new(width, 0.0..=1.0)
                                .percent()
                                .default(0.4)
                                .show(ui)
                        });
                        if widgets::toggle_row(ui, "Ohne Hintergrund", transparent) {
                            e.commit = true;
                        }
                    }
                    None => {}
                }
                *e |= theme::row(ui, "Weiche Kante", |ui| {
                    Slider::new(&mut c.fx.edge.feather, 0.0..=1.0)
                        .percent()
                        .default(0.0)
                        .show(ui)
                });
                *e |= theme::row(ui, "Kante verschieben", |ui| {
                    Slider::new(&mut c.fx.edge.shift, -1.0..=1.0)
                        .percent()
                        .default(0.0)
                        .show(ui)
                });
                if widgets::toggle_row(ui, "Umkehren", &mut c.fx.edge.invert) {
                    e.commit = true;
                }
            }
        }
        if is_video {
            theme::section(ui, "Bildqualität");
            *e |= theme::row(ui, "Entrauschen", |ui| {
                Slider::new(&mut c.fx.denoise, 0.0..=1.0)
                    .percent()
                    .default(0.0)
                    .show(ui)
            });
            if widgets::toggle_row(ui, "Flackern ausgleichen", &mut c.fx.deflicker) {
                e.commit = true;
            }
            theme::section(ui, "Bewegung");
            *e |= theme::row(ui, "Stabilisieren", |ui| {
                Slider::new(&mut c.fx.stabilize, 0.0..=1.0)
                    .percent()
                    .default(0.0)
                    .show(ui)
            });
            if c.fx.stabilize > 0.0 && c.fx.cutout.is_some() {
                ui.label(
                    RichText::new(crate::tr(
                        "Bei entferntem Hintergrund wird nicht stabilisiert",
                    ))
                    .small()
                    .color(theme::TEXT_DIM()),
                );
            }
            ui.horizontal_wrapped(|ui| {
                if ui.add(theme::secondary(format!("{}  Auto-Reframe", icon::CROP))).on_hover_text(crate::tr("Hält das Wichtige im aktuellen Seitenverhältnis im Bild (setzt Keyframes)")).clicked() {
                    let (w, h) = (self.project.settings.width as f32, self.project.settings.height as f32);
                    let same = match &c.content {
                        ClipContent::Media { media, .. } => self.project.media(*media).is_some_and(|m| ((m.width as f32 / m.height.max(1) as f32) - w / h).abs() < 0.02),
                        _ => true,
                    };
                    if same {
                        self.toast_err("Das Format passt schon – erst ein anderes Seitenverhältnis wählen (z. B. 9:16)");
                    } else {
                        let cc = c.clone();
                        self.start_simple_analysis(&ctx, &cc, "reframe");
                    }
                }
                if ui.add(theme::secondary(format!("{}  Szenen teilen", icon::FILM_STRIP))).on_hover_text(crate::tr("Szenenwechsel erkennen und den Clip dort teilen")).clicked() {
                    let cc = c.clone();
                    self.start_simple_analysis(&ctx, &cc, "scenes");
                }
            });
            task_bar(ui, self, &format!("reframe:{}", c.id));
            task_bar(ui, self, &format!("scenes:{}", c.id));
        }
        if is_media {
            self.censor_ui(ui, c, e);
        }
        if !on_main {
            theme::section(ui, "Bewegung verfolgen");
            let picking = matches!(self.pick, Some(Pick::Track { clip, .. }) if clip == c.id);
            if task_bar(ui, self, &format!("track:{}", c.id)) {
            } else if picking {
                ui.label(RichText::new(crate::tr("Ziehe im Player einen Rahmen um das Objekt, dem dieser Clip folgen soll (Esc bricht ab)")).small().color(theme::TEXT()));
            } else if ui
                .add(theme::secondary(format!(
                    "{}  Objekt verfolgen",
                    icon::CROSSHAIR
                )))
                .on_hover_text(crate::tr(
                    "Clip folgt einem Objekt im Video darunter (ab Playhead)",
                ))
                .clicked()
            {
                match self.video_below(c.id) {
                    Some(t) => {
                        self.pick = Some(Pick::Track {
                            clip: c.id,
                            target: t,
                        })
                    }
                    None => self.toast_err("Unter diesem Clip liegt am Playhead kein Video"),
                }
            }
        }
    }

    /// Extras in the „Audio“ tab (noise, sound, audio filters, voice filters, beats).
    pub fn pro_audio(&mut self, ui: &mut egui::Ui, c: &mut Clip, e: &mut Edit) {
        let ctx = ui.ctx().clone();
        *e |= audio_fx_ui(ui, &mut c.audio);
        // Beats: in the last section, no separate section for one button
        ui.add_space(6.0);
        if !task_bar(ui, self, &format!("beats:{}", c.id)) {
            ui.horizontal(|ui| {
                if ui
                    .add(theme::secondary(format!(
                        "{}  Beats erkennen",
                        icon::METRONOME
                    )))
                    .on_hover_text(crate::tr(
                        "Markiert die Schläge auf dem Clip; Clips und Playhead rasten daran ein",
                    ))
                    .clicked()
                {
                    let cc = c.clone();
                    self.start_simple_analysis(&ctx, &cc, "beats");
                }
                if !c.beats.is_empty() {
                    ui.label(
                        RichText::new(crate::tr(format!("{} Beats", c.beats.len())))
                            .small()
                            .color(theme::TEXT_DIM()),
                    );
                    if theme::icon_button(ui, icon::X, "Beat-Markierungen entfernen", true, false)
                        .clicked()
                    {
                        c.beats.clear();
                        e.commit = true;
                    }
                }
            });
        }
    }

    // ------------------------------------------------------------ captions

    pub fn captions_pro(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let m = self.models();
        let (choices, cur) = self.asr_choices(&ctx);
        let engine = if let Some(c) = choices.iter().find(|c| c.0 == cur && cur != "auto") {
            Some(c.1.clone())
        } else if m.installed("parakeet") && m.installed("onnxruntime") {
            Some("Parakeet TDT 0.6B v3".to_string())
        } else if m.installed("whisper") {
            Some("Whisper".to_string())
        } else {
            None
        };
        egui::Frame::default()
            .fill(theme::BG_RAISED())
            .corner_radius(theme::R_CARD)
            .inner_margin(theme::PAD_CARD_L)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(crate::tr(format!(
                        "{}  Automatische Untertitel",
                        icon::WAVEFORM
                    )))
                    .strong(),
                );
                ui.label(
                    RichText::new(crate::tr(match &engine {
                        Some(e) => format!("Lokal mit {e} – nichts verlässt deinen Rechner"),
                        None => "Noch kein Spracherkennungsmodell installiert".into(),
                    }))
                    .small()
                    .color(theme::TEXT_DIM()),
                );
                ui.add_space(6.0);
                if engine.is_none() {
                    if ui.add(theme::primary("Modell laden …")).clicked() {
                        self.settings_section = crate::settings::Section::Models;
                        self.settings_open = true;
                    }
                    return;
                }
                theme::row(ui, "Spracherkennung", |ui| {
                    let name = |id: &str| choices.iter().find(|c| c.0 == id).map_or("", |c| c.1.as_str());
                    egui::ComboBox::from_id_salt("cap-engine")
                        .selected_text(crate::tr(name(cur)))
                        .width((ui.available_width() - 12.0).max(60.0))
                        .show_ui(ui, |ui| {
                            for (id, n) in &choices {
                                if ui.selectable_label(*id == cur, crate::tr(n.as_str())).clicked() {
                                    self.settings.asr_engine = id.to_string();
                                    self.settings.save();
                                }
                            }
                        });
                });
                theme::row(ui, "Sprache", |ui| {
                    let cur = LANGS
                        .iter()
                        .find(|l| l.0 == self.caption_lang)
                        .map(|l| l.1)
                        .unwrap_or("Automatisch");
                    egui::ComboBox::from_id_salt("cap-lang")
                        .selected_text(crate::tr(cur))
                        .width((ui.available_width() - 12.0).max(60.0))
                        .show_ui(ui, |ui| {
                            for (code, name) in LANGS {
                                if ui
                                    .selectable_label(self.caption_lang == code, crate::tr(name))
                                    .clicked()
                                {
                                    self.caption_lang = code.to_string();
                                }
                            }
                        });
                });
                let sel_audio = self
                    .selected
                    .and_then(|s| self.project.find_clip(s))
                    .filter(|(_, c)| {
                        matches!(
                            c.content,
                            ClipContent::Media {
                                kind: MediaKind::Video | MediaKind::Audio,
                                ..
                            }
                        )
                    })
                    .map(|(_, c)| c.id);
                let mut src = if self.caption_from_clip && sel_audio.is_some() {
                    1
                } else {
                    0
                };
                theme::row(ui, "Quelle", |ui| {
                    if widgets::segmented(ui, &["Ganzes Projekt", "Ausgewählter Clip"], &mut src) {
                        self.caption_from_clip = src == 1;
                    }
                });
                ui.add_space(6.0);
                ui.label(RichText::new(crate::tr("Stil")).small().color(theme::TEXT_DIM()));
                self.caption_preset_picker(ui);
                if !task_bar(ui, self, "captions") {
                    if ui
                        .add_sized(
                            [ui.available_width(), 28.0],
                            theme::primary(format!("{}  Untertitel erzeugen", icon::SUBTITLES)),
                        )
                        .clicked()
                    {
                        if src == 1 && sel_audio.is_none() {
                            self.toast_err("Wähle einen Video- oder Audioclip");
                        } else {
                            self.start_captions(&ctx, if src == 1 { sel_audio } else { None });
                        }
                    }
                }
            });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(theme::secondary(format!(
                    "{}  SRT importieren",
                    icon::FILE_ARROW_DOWN
                )))
                .clicked()
            {
                if let Some(f) = crate::dialog()
                    .add_filter("Untertitel", &["srt", "vtt"])
                    .pick_file()
                {
                    self.import_srt(&f);
                }
            }
            if ui
                .add(theme::secondary(format!(
                    "{}  SRT exportieren",
                    icon::EXPORT
                )))
                .clicked()
            {
                if let Some(f) = crate::dialog()
                    .add_filter("SubRip", &["srt"])
                    .add_filter("WebVTT", &["vtt"])
                    .set_file_name(format!("{}.srt", self.project.name))
                    .save_file()
                {
                    match std::fs::write(&f, cl_export::subtitles_for(&self.project, &f)) {
                        Ok(()) => self.toast("Untertitel gespeichert"),
                        Err(e) => self.toast_err(e.to_string()),
                    }
                }
            }
        });
        ui.add_space(6.0);
        self.captions_editor(ui);
    }

    // ------------------------------------------------------------ player selection

    /// Eyedropper or tracking frame in the player. Returns `true` if the input was consumed.
    pub fn player_pick(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        regions: &[Region],
    ) -> bool {
        let Some(pick) = self.pick else { return false };
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.pick = None;
            return true;
        }
        let painter = ui.painter_at(resp.rect);
        let hint = match pick {
            Pick::Chroma(_) => "Klicke auf die Farbe, die verschwinden soll",
            Pick::Track { .. } | Pick::Censor(_) => "Ziehe einen Rahmen um das Objekt",
        };
        let g = painter.layout_no_wrap(
            format!(
                "{}  {hint}  ·  Esc",
                match pick {
                    Pick::Chroma(_) => icon::EYEDROPPER,
                    Pick::Track { .. } | Pick::Censor(_) => icon::CROSSHAIR,
                }
            ),
            egui::FontId::proportional(theme::FS_TEXT),
            theme::ACCENT_INK(),
        );
        let br = Rect::from_center_size(
            resp.rect.center_top() + vec2(0.0, 22.0),
            g.size() + vec2(20.0, 10.0),
        );
        painter.rect_filled(br, 12.0, theme::ACCENT()); // pill: radius = half the height
        painter.galley(br.min + vec2(10.0, 5.0), g, theme::ACCENT_INK());
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        let local = |r: &Region, p: egui::Pos2| -> (f32, f32) {
            let d = p - r.center;
            let (s, c) = (-r.angle).sin_cos();
            let v = vec2(d.x * c - d.y * s, d.x * s + d.y * c);
            (v.x / r.size.x + 0.5, v.y / r.size.y + 0.5)
        };
        match pick {
            Pick::Chroma(clip) => {
                if resp.clicked() {
                    if let (Some(p), Some(r)) = (
                        resp.interact_pointer_pos(),
                        regions.iter().find(|r| r.clip == clip),
                    ) {
                        let (u, v) = local(r, p);
                        if let Some(f) = self
                            .frame_cpu
                            .as_ref()
                            .filter(|(id, _)| *id == clip)
                            .map(|(_, f)| f)
                        {
                            let x =
                                ((u * f.width as f32) as i64).clamp(0, f.width as i64 - 1) as usize;
                            let y = ((v * f.height as f32) as i64).clamp(0, f.height as i64 - 1)
                                as usize;
                            let i = (y * f.width as usize + x) * 4;
                            let col = [f.data[i], f.data[i + 1], f.data[i + 2]];
                            if let Some(mut c) = self.clip_view(clip) {
                                let mut k = c.fx.chroma.unwrap_or_default();
                                k.color = col;
                                c.fx.chroma = Some(k);
                                self.live = None;
                                self.apply(EditOp::ReplaceClip { clip: c });
                                self.toast(format!(
                                    "Farbe #{:02X}{:02X}{:02X} freigestellt",
                                    col[0], col[1], col[2]
                                ));
                            }
                        }
                    }
                    self.pick = None;
                }
            }
            Pick::Track { .. } | Pick::Censor(_) => {
                let (clip, target) = match pick {
                    Pick::Track { clip, target } => (clip, target),
                    Pick::Censor(c) | Pick::Chroma(c) => (c, c),
                };
                if resp.drag_started() {
                    self.pick_origin = ui
                        .input(|i| i.pointer.press_origin())
                        .or(resp.interact_pointer_pos());
                }
                let cur = ui.input(|i| i.pointer.latest_pos());
                if let (Some(a), Some(b)) = (self.pick_origin, cur) {
                    if resp.dragged() || resp.drag_stopped() {
                        let r = Rect::from_two_pos(a, b);
                        painter.rect_stroke(
                            r,
                            2.0,
                            Stroke::new(1.5_f32, theme::ACCENT()),
                            egui::StrokeKind::Inside,
                        );
                        painter.rect_filled(r, 2.0, Color32::from_white_alpha(20));
                    }
                    if resp.drag_stopped() {
                        self.pick_origin = None;
                        if let Some(reg) = regions.iter().find(|r| r.clip == target) {
                            let (u0, v0) = local(reg, a);
                            let (u1, v1) = local(reg, b);
                            let bx = cl_analyze::track::BoxN {
                                cx: ((u0 + u1) / 2.0).clamp(0.0, 1.0),
                                cy: ((v0 + v1) / 2.0).clamp(0.0, 1.0),
                                w: (u1 - u0).abs().max(0.02),
                                h: (v1 - v0).abs().max(0.02),
                            };
                            if matches!(pick, Pick::Censor(_)) {
                                self.add_censor(ui.ctx(), clip, bx);
                            } else {
                                self.start_tracking(ui.ctx(), clip, target, bx);
                            }
                        } else {
                            self.toast_err(if matches!(pick, Pick::Censor(_)) {
                                "Der Clip ist am Playhead nicht sichtbar"
                            } else {
                                "Das Video darunter ist am Playhead nicht sichtbar"
                            });
                        }
                        self.pick = None;
                    }
                }
            }
        }
        true
    }

    fn start_tracking(
        &mut self,
        ctx: &egui::Context,
        clip: ClipId,
        target: ClipId,
        bx: cl_analyze::track::BoxN,
    ) {
        let (Some(c), Some(t)) = (self.clip_view(clip), self.clip_view(target)) else {
            return;
        };
        let ClipContent::Media { media, .. } = t.content else {
            return;
        };
        let Some(m) = self.project.media(media).cloned() else {
            return;
        };
        let from = self.playhead.max(c.start).max(t.start);
        let until = c.end().min(t.end());
        if until <= from {
            return;
        }
        let src_start = t.source_time(from).as_secs_f64();
        let src_dur = (until - from).as_secs_f64() * t.speed.max(0.01) as f64;
        self.tasks.spawn(
            ctx,
            format!("track:{clip}"),
            "Bewegung wird verfolgt …",
            move |p, cancel| match cl_analyze::track::track(
                &m.path, m.width, m.height, src_start, src_dur, bx, p, cancel,
            ) {
                Ok(points) => TaskDone::Track {
                    clip,
                    target,
                    points,
                    from,
                },
                Err(e) => TaskDone::Failed(e),
            },
        );
    }

    // ------------------------------------------------------------ censoring

    /// Create a censor area from the drawn frame (`bx` in display position, i.e. mirrored as in the
    /// player): kind, shape, strength and „Verfolgen“ like the last created area.
    /// The area is `CENSOR_PAD` larger than the frame; for videos the subject is tracked.
    fn add_censor(&mut self, ctx: &egui::Context, clip: ClipId, bx: cl_analyze::track::BoxN) {
        let Some(mut c) = self.clip_view(clip) else { return };
        let t = c.source_time(self.playhead.clamp(c.start, c.end()));
        let k = CensorKey { t, cx: bx.cx, cy: bx.cy, w: bx.w, h: bx.h };
        let mut cs = c.fx.censor.last().cloned().unwrap_or(Censor { track: true, ..Default::default() });
        cs.path = vec![censor_flip(&c, k).grown(CENSOR_PAD)];
        cs.id = CensorId::default();
        let (track, id) = (cs.track && is_video(&c), cs.id);
        c.fx.censor.push(cs);
        self.live = None;
        self.apply(EditOp::ReplaceClip { clip: c });
        if track {
            self.start_censor_tracking(ctx, clip, id);
        }
    }

    /// Track the subject of area `id` from its position at the playhead over the whole clip.
    fn start_censor_tracking(&mut self, ctx: &egui::Context, clip: ClipId, id: CensorId) {
        let Some(c) = self.clip_view(clip) else { return };
        let ClipContent::Media { media, .. } = c.content else { return };
        let cs = c.fx.censor.iter().find(|cs| cs.id.0 == id.0);
        let (Some(m), Some(cs)) = (self.project.media(media).cloned(), cs) else {
            return;
        };
        let at = c.source_time(self.playhead.clamp(c.start, c.end()));
        let k = cs.at(at);
        let init = cl_analyze::track::BoxN { cx: k.cx, cy: k.cy, w: k.w / CENSOR_PAD, h: k.h / CENSOR_PAD };
        let (a, b) = (c.source_time(c.start), c.source_time(c.end()));
        let (s0, s1) = (a.min(b).as_secs_f64(), a.max(b).as_secs_f64());
        self.tasks.spawn(ctx, censor_task(clip, id), "Zensur wird verfolgt …", move |p, cancel| {
            match cl_analyze::track::track_box(&m.path, m.width, m.height, s0, s1 - s0, at.as_secs_f64(), init, p, cancel) {
                Ok(_) if cancel.load(std::sync::atomic::Ordering::Relaxed) => TaskDone::Run(Box::new(|_| {})),
                Ok(boxes) => {
                    let keys: Vec<CensorKey> = boxes
                        .iter()
                        .map(|(t, b)| CensorKey { t: Time::from_secs_f64(*t), cx: b.cx, cy: b.cy, w: b.w, h: b.h })
                        .collect();
                    let path = censor_simplify(&keys);
                    TaskDone::Run(Box::new(move |app: &mut App| app.censor_tracked(media, id, path)))
                }
                Err(e) => TaskDone::Failed(e),
            }
        });
    }

    /// Enter the tracking result of area `id` – in every clip of this medium that (still) has it
    /// and tracks it: after a split it belongs to both halves, the path lies in
    /// source time. If the area no longer exists (removed, undone, clip deleted) or
    /// „Verfolgen“ is off, the result is discarded.
    fn censor_tracked(&mut self, media: MediaId, id: CensorId, path: Vec<CensorKey>) {
        if path.is_empty() {
            return;
        }
        let ids: Vec<ClipId> = self
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| matches!(c.content, ClipContent::Media { media: m, .. } if m == media))
            .map(|c| c.id)
            .collect();
        let ops: Vec<EditOp> = ids
            .into_iter()
            .filter_map(|clip| {
                let mut c = self.clip_view(clip)?;
                let cs = c.fx.censor.iter_mut().find(|cs| cs.id.0 == id.0 && cs.track)?;
                cs.path = path.clone();
                Some(EditOp::ReplaceClip { clip: c })
            })
            .collect();
        if ops.is_empty() {
            return;
        }
        self.live = None;
        self.apply(EditOp::Batch { label: "Zensur verfolgt".into(), ops });
        self.toast(format!("Zensur verfolgt ({} Keyframes)", path.len()));
    }

    /// „Zensur“ section in the „Smart“ tab: settings per area and „Zensieren …“.
    fn censor_ui(&mut self, ui: &mut egui::Ui, c: &mut Clip, e: &mut Edit) {
        theme::section(ui, "Zensur");
        let video = is_video(c);
        let now = c.source_time(self.playhead.clamp(c.start, c.end()));
        let (mut remove, mut track) = (None, None);
        for (i, cs) in c.fx.censor.iter_mut().enumerate() {
            let key = censor_task(c.id, cs.id);
            if i > 0 {
                ui.add_space(8.0);
            }
            let mut kind = cs.blur as usize;
            theme::row(ui, "Art", |ui| {
                if widgets::segmented(ui, &["Verpixeln", "Weichzeichnen"], &mut kind) {
                    cs.blur = kind == 1;
                    e.commit = true;
                }
            });
            let mut shape = cs.ellipse as usize;
            theme::row(ui, "Form", |ui| {
                if widgets::segmented(ui, &["Rechteck", "Ellipse"], &mut shape) {
                    cs.ellipse = shape == 1;
                    e.commit = true;
                }
            });
            *e |= theme::row(ui, "Stärke", |ui| {
                Slider::new(&mut cs.strength, 0.0..=1.0).percent().default(0.0).show(ui)
            });
            if video {
                let mut on = cs.track;
                if widgets::toggle_row(ui, "Verfolgen", &mut on) {
                    cs.track = on;
                    if on {
                        track = Some(cs.id);
                    } else {
                        if let Some(t) = self.tasks.running(&key) {
                            t.cancel();
                        }
                        cs.path = vec![CensorKey { t: now, ..cs.at(now) }];
                    }
                    e.commit = true;
                }
                task_bar(ui, self, &key);
            }
            ui.horizontal(|ui| {
                ui.add_space(theme::LABEL_W + 6.0);
                if ui.add(theme::ghost(format!("{}  Entfernen", icon::X))).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            if let Some(t) = self.tasks.running(&censor_task(c.id, c.fx.censor[i].id)) {
                t.cancel();
            }
            c.fx.censor.remove(i);
            e.commit = true;
        }
        if let Some(id) = track {
            // reads the area from the project; the result only applies if „Verfolgen“ is on then
            let ctx = ui.ctx().clone();
            self.start_censor_tracking(&ctx, c.id, id);
        }
        ui.add_space(4.0);
        let picking = self.pick == Some(Pick::Censor(c.id));
        if ui.add(theme::secondary(format!("{}  Zensieren …", icon::EYE_SLASH))).clicked() {
            self.pick = if picking { None } else { Some(Pick::Censor(c.id)) };
        }
    }

    /// Move areas of the selected clip in the player (drag inside) or scale them (drag
    /// a corner) while the „Smart“ tab is open. Affects the whole path.
    /// Returns `true` if the input was consumed.
    pub fn censor_gizmo(&mut self, ui: &egui::Ui, resp: &egui::Response, regions: &[Region]) -> bool {
        if self.pick.is_some() || self.clip_tab != crate::app::ClipTab::Smart {
            self.censor_drag = None;
            return false;
        }
        let Some((c, reg)) = self
            .selected
            .and_then(|s| self.clip_view(s))
            .filter(|c| !c.fx.censor.is_empty())
            .and_then(|c| regions.iter().find(|r| r.clip == c.id).map(|r| (c, *r)))
        else {
            return false;
        };
        let now = c.source_time(self.playhead.clamp(c.start, c.end()));
        // display position (mirrored as in the player) ↔ screen
        let screen = |u: f32, v: f32| reg.corner(2.0 * u - 1.0, 2.0 * v - 1.0);
        let local = |p: egui::Pos2| {
            let d = p - reg.center;
            let (s, co) = (-reg.angle).sin_cos();
            ((d.x * co - d.y * s) / reg.size.x + 0.5, (d.x * s + d.y * co) / reg.size.y + 0.5)
        };
        let shown = |i: usize| censor_flip(&c, c.fx.censor[i].at(now));
        let painter = ui.painter_at(resp.rect);
        let pointer = ui.input(|i| i.pointer.latest_pos());
        // running drag: new position of the area (display position)
        let dragged = |d: &CensorDrag, p: egui::Pos2| {
            let (u, v) = local(p);
            let (u0, v0) = local(d.press);
            if d.scale {
                CensorKey { w: (2.0 * (u - d.start.cx).abs()).max(0.02), h: (2.0 * (v - d.start.cy).abs()).max(0.02), ..d.start }
            } else {
                CensorKey { cx: d.start.cx + u - u0, cy: d.start.cy + v - v0, ..d.start }
            }
        };
        let hit = |p: egui::Pos2| -> Option<(usize, bool)> {
            (0..c.fx.censor.len()).rev().find_map(|i| {
                let k = shown(i);
                let corner = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                    .iter()
                    .any(|(sx, sy)| p.distance(screen(k.cx + sx * k.w / 2.0, k.cy + sy * k.h / 2.0)) <= 10.0);
                let (u, v) = local(p);
                let inside = (u - k.cx).abs() <= k.w / 2.0 && (v - k.cy).abs() <= k.h / 2.0;
                (corner || inside).then_some((i, corner))
            })
        };
        if resp.drag_started() {
            let press = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
            if let Some((p, (i, scale))) = press.and_then(|p| hit(p).map(|h| (p, h))) {
                self.censor_drag = Some(CensorDrag { clip: c.id, i, scale, press: p, start: shown(i) });
            }
        }
        let drag = self.censor_drag.filter(|d| d.clip == c.id && d.i < c.fx.censor.len());
        for i in 0..c.fx.censor.len() {
            let k = match (drag, pointer) {
                (Some(d), Some(p)) if d.i == i => dragged(&d, p),
                _ => shown(i),
            };
            let pts: Vec<egui::Pos2> = if c.fx.censor[i].ellipse {
                (0..48)
                    .map(|j| {
                        let a = j as f32 / 48.0 * std::f32::consts::TAU;
                        screen(k.cx + a.cos() * k.w / 2.0, k.cy + a.sin() * k.h / 2.0)
                    })
                    .collect()
            } else {
                [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                    .iter()
                    .map(|(sx, sy)| screen(k.cx + sx * k.w / 2.0, k.cy + sy * k.h / 2.0))
                    .collect()
            };
            painter.add(egui::Shape::closed_line(pts, Stroke::new(1.5_f32, theme::ACCENT())));
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                painter.circle_filled(screen(k.cx + sx * k.w / 2.0, k.cy + sy * k.h / 2.0), 4.0, theme::ACCENT());
            }
        }
        if let Some(d) = drag {
            if resp.drag_stopped() {
                self.censor_drag = None;
                if let Some(p) = pointer {
                    let (a, b) = (d.start, dragged(&d, p));
                    let mut c = c.clone();
                    let flip = |v: f32, f: bool| if f { -v } else { v };
                    let (dx, dy) = (flip(b.cx - a.cx, c.fx.flip_h), flip(b.cy - a.cy, c.fx.flip_v));
                    let (kx, ky) = (b.w / a.w.max(1e-4), b.h / a.h.max(1e-4));
                    for k in &mut c.fx.censor[d.i].path {
                        (k.cx, k.cy, k.w, k.h) = (k.cx + dx, k.cy + dy, k.w * kx, k.h * ky);
                    }
                    self.live = None;
                    self.apply(EditOp::ReplaceClip { clip: c });
                }
            }
            return true;
        }
        // do not interrupt a drag started elsewhere (moving a clip)
        if let Some(p) = resp.hover_pos().filter(|_| !resp.dragged() && !resp.drag_stopped()) {
            if let Some((_, corner)) = hit(p) {
                ui.ctx().set_cursor_icon(if corner { egui::CursorIcon::ResizeNwSe } else { egui::CursorIcon::Move });
                return true;
            }
        }
        false
    }

    // ------------------------------------------------------------ models

    pub fn models_settings(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let m = self.models();
        ui.add_space(6.0);
        ui.add_space(8.0);
        for info in cl_analyze::CATALOG {
            let installed = m.installed(info.id);
            egui::Frame::default()
                .fill(theme::BG_RAISED())
                .corner_radius(theme::R_CARD)
                .inner_margin(theme::PAD_CARD_L)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            // leave room for button/status on the right, otherwise the text runs underneath
                            ui.set_max_width(ui.available_width() - 150.0);
                            ui.label(RichText::new(crate::tr(info.name)).strong());
                            ui.label(
                                RichText::new(crate::tr(info.about))
                                    .small()
                                    .color(theme::TEXT_DIM()),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let key = format!("model:{}", info.id);
                            if self.tasks.running(&key).is_some() {
                                return;
                            }
                            if installed {
                                ui.label(
                                    RichText::new(crate::tr(format!(
                                        "{}  installiert",
                                        icon::CHECK_CIRCLE
                                    )))
                                    .small()
                                    .color(theme::TEXT_DIM()),
                                );
                            } else if info.id == "onnxruntime" && !cfg!(windows) {
                                ui.label(
                                    RichText::new(crate::tr("Im Programmpaket enthalten"))
                                        .small()
                                        .color(theme::TEXT_DIM()),
                                );
                            } else if ui
                                .add(theme::primary(format!("Laden · {} MB", info.size_mb)))
                                .clicked()
                            {
                                self.download_model(&ctx, info.id);
                            }
                        });
                    });
                    task_bar(ui, self, &format!("model:{}", info.id));
                });
            ui.add_space(6.0);
        }
        ui.horizontal(|ui| {
            if ui
                .add(theme::ghost(format!(
                    "{}  Modellordner öffnen",
                    icon::FOLDER_OPEN
                )))
                .clicked()
            {
                let d = crate::paths::models_dir();
                let _ = std::fs::create_dir_all(&d);
                crate::home::reveal(&d);
            }
        });
    }

    pub fn download_model(&mut self, ctx: &egui::Context, id: &'static str) {
        let Some(info) = cl_analyze::CATALOG.iter().find(|c| c.id == id) else {
            return;
        };
        let dir = crate::paths::models_dir();
        let files: Vec<(String, String)> = info
            .files
            .iter()
            .map(|(f, u)| (f.to_string(), u.to_string()))
            .collect();
        self.tasks.spawn(
            ctx,
            format!("model:{id}"),
            format!("{} wird geladen …", info.name),
            move |p, cancel| {
                let n = files.len() as f32;
                for (i, (rel, url)) in files.iter().enumerate() {
                    let dest = dir.join(rel);
                    if dest.is_file() {
                        continue;
                    }
                    let r = cl_update::download_to(
                        url,
                        &dest,
                        |d, t| p((i as f32 + if t > 0 { d as f32 / t as f32 } else { 0.0 }) / n),
                        cancel,
                    );
                    if let Err(e) = r {
                        return TaskDone::Failed(format!("Download: {e}"));
                    }
                    if rel.ends_with(".zip") {
                        let r = cl_update::extract_zip(&dest, &dir.join("ort"));
                        let _ = std::fs::remove_file(&dest);
                        if let Err(e) = r {
                            return TaskDone::Failed(e);
                        }
                    }
                }
                TaskDone::Model { id }
            },
        );
    }
}

/// Audio tools for clips and voiceover: noise, sound, audio filters, voice filters.
pub fn audio_fx_ui(ui: &mut egui::Ui, a: &mut AudioFx) -> Edit {
    let mut e = Edit::default();
    theme::row(ui, "Kanäle", |ui| {
        let label = a.channels.map(|c| c.name()).unwrap_or("Projekt-Standard");
        egui::ComboBox::from_id_salt("clip-channels")
            .selected_text(crate::tr(label))
            .show_ui(ui, |ui| {
                if ui.selectable_label(a.channels.is_none(), crate::tr("Projekt-Standard")).clicked() {
                    a.channels = None;
                    e.commit = true;
                }
                for (c, n) in cl_core::Channels::ALL {
                    if ui.selectable_label(a.channels == Some(c), crate::tr(n)).clicked() {
                        a.channels = Some(c);
                        e.commit = true;
                    }
                }
            })
            .response
            .on_hover_text(crate::tr("„Automatisch“ legt Ton, der nur links oder rechts aufgenommen wurde, auf beide Ohren"));
    });
    theme::section(ui, "Störgeräusche");
    e |= theme::row(ui, "Entrauschen", |ui| {
        Slider::new(&mut a.denoise, 0.0..=1.0).percent().default(0.0).show(ui)
    });
    let toggles: [(&str, &str, &mut bool); 4] = [
        ("Brummen", "Netzbrummen (50/60 Hz) entfernen", &mut a.hum),
        ("Rumpeln & Wind", "Tiefe Störgeräusche unter 90 Hz entfernen", &mut a.rumble),
        ("Klicks", "Klicks und Knackser entfernen", &mut a.declick),
        ("Stimme hervorheben", "Sprache betonen, Hintergrund absenken", &mut a.voice_isolate),
    ];
    for (label, tip, v) in toggles {
        let r = ui.scope(|ui| widgets::toggle_row(ui, label, v));
        if r.inner {
            e.commit = true;
        }
        r.response.on_hover_text(crate::tr(tip));
    }
    theme::section(ui, "Klang");
    e |= theme::row(ui, "Kompressor", |ui| {
        Slider::new(&mut a.compress, 0.0..=1.0).percent().default(0.0).show(ui)
    });
    e |= theme::row(ui, "Bass", |ui| {
        Slider::new(&mut a.bass, -12.0..=12.0).suffix(" dB").default(0.0).show(ui)
    });
    e |= theme::row(ui, "Höhen", |ui| {
        Slider::new(&mut a.treble, -12.0..=12.0).suffix(" dB").default(0.0).show(ui)
    });
    if widgets::toggle_row(ui, "Lautheit angleichen (−14 LUFS)", &mut a.normalize) {
        e.commit = true;
    }
    for (title, list, slot, salt) in [
        ("Audiofilter", cl_fx::audio::FILTERS, &mut a.filter, "afilter"),
        ("Stimmfilter", cl_fx::audio::VOICES, &mut a.voice, "voice"),
    ] {
        theme::section(ui, title);
        let mut items: Vec<(&str, &str)> = vec![("", "Kein")];
        items.extend(list.iter().map(|v| (v.0, v.1)));
        let cur = slot.clone().unwrap_or_default();
        let mut chosen = None;
        widgets::grid(ui, &items, 70.0, 2.4, 0.0, |ui, r, (id, name)| {
            let resp = widgets::tile(ui, egui::Id::new((salt, *id)), r, "", cur == *id, |p, area| {
                p.text(
                    area.center(),
                    egui::Align2::CENTER_CENTER,
                    crate::tr(*name),
                    egui::FontId::proportional(theme::FS_UI),
                    theme::TEXT(),
                );
            });
            if resp.clicked() {
                chosen = Some(id.to_string());
            }
        });
        if let Some(v) = chosen {
            *slot = (!v.is_empty()).then_some(v);
            e.commit = true;
        }
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_analyze::asr::Engine;

    #[test]
    fn cut_mode_is_derived_from_clip_and_task() {
        let k = Some(ChromaKey::default());
        let m = Some("m.mkv".to_string());
        let fx = |cutout: &Option<String>, chroma: &Option<ChromaKey>| ClipFx { cutout: cutout.clone(), chroma: *chroma, ..Default::default() };
        for running in [false, true] {
            let busy = |m: CutMode| if running { CutMode::Person } else { m };
            assert_eq!(CutMode::of(&fx(&None, &None), running), busy(CutMode::Off));
            assert_eq!(CutMode::of(&fx(&None, &k), running), busy(CutMode::Color));
            assert_eq!(CutMode::of(&fx(&m, &None), running), CutMode::Person);
            // older project with both: Person (plus the status row for the key)
            assert_eq!(CutMode::of(&fx(&m, &k), running), CutMode::Person);
        }
        // a person effect or edge alone is no way of its own
        let only = ClipFx { person: Some(PersonFx::from_index(1)), edge: MatteEdge { invert: true, ..Default::default() }, ..Default::default() };
        assert_eq!(CutMode::of(&only, false), CutMode::Off);
    }

    /// Each choice is one clip replacement (one undo step) with the fields of the other ways gone.
    #[test]
    fn cut_mode_choice_is_one_undo_step() {
        let mut p = Project::new("Freistellen");
        let mut t = Track::new(TrackKind::Video);
        // fields only: the content does not matter for the choice
        let mut c = Clip::new(ClipContent::Text(Default::default()), Time::ZERO, Time::from_secs_f64(1.0));
        let custom = ChromaKey { similarity: 0.3, ..Default::default() };
        c.fx.cutout = Some("m.mkv".into());
        c.fx.chroma = Some(custom);
        c.fx.person = Some(PersonFx::from_index(2));
        c.fx.edge.shift = 0.5;
        c.fx.adjust.brightness = 0.2;
        t.clips.push(c.clone());
        p.timeline.tracks.push(t);
        let mut doc = Document::new(p);
        let choose = |doc: &mut Document, m: CutMode| {
            let mut c = doc.project().timeline.tracks[0].clips[0].clone();
            m.apply(&mut c.fx);
            let before = doc.undo_entries().len();
            doc.apply(EditOp::ReplaceClip { clip: c }, OpSource::Ui).unwrap();
            assert_eq!(doc.undo_entries().len(), before + 1);
            doc.project().timeline.tracks[0].clips[0].fx.clone()
        };
        // Color: mask goes, the existing key stays, background effect and edge stay
        let fx = choose(&mut doc, CutMode::Color);
        assert_eq!((fx.cutout, fx.chroma), (None, Some(custom)));
        assert_eq!((fx.person.is_some(), fx.edge.shift), (true, 0.5));
        doc.undo().unwrap();
        // Person: key goes, the mask stays
        let fx = choose(&mut doc, CutMode::Person);
        assert_eq!((fx.cutout.as_deref(), fx.chroma), (Some("m.mkv"), None));
        assert_eq!(CutMode::of(&fx, false), CutMode::Person);
        // Off: everything of the cutout goes, the rest of the look stays
        let fx = choose(&mut doc, CutMode::Off);
        assert!(fx.cutout.is_none() && fx.chroma.is_none() && fx.person.is_none() && fx.edge.is_identity());
        assert_eq!(fx.adjust.brightness, 0.2);
        // Color from nothing: default key
        let fx = choose(&mut doc, CutMode::Color);
        assert_eq!(fx.chroma, Some(ChromaKey::default()));
        // one undo brings back the previous state as a whole
        doc.undo().unwrap();
        doc.undo().unwrap();
        assert_eq!(doc.project().timeline.tracks[0].clips[0].fx.chroma, None);
        assert_eq!(doc.project().timeline.tracks[0].clips[0].fx.cutout.as_deref(), Some("m.mkv"));
    }

    #[test]
    fn asr_offer_lists_installed_engines_and_forces_the_choice() {
        let ids = |v: &[(&'static str, String)]| v.iter().map(|c| c.0).collect::<Vec<_>>();
        // both installed, filter present: Whisper selectable and passed as the default to recognition
        let (v, cur) = asr_offer(true, Some("Whisper Base".into()), true, "whisper");
        assert_eq!(ids(&v), ["auto", "parakeet", "whisper"]);
        assert_eq!(v[2].1, "Whisper Base");
        assert_eq!(asr_force(cur), Some(Engine::Whisper));
        // no Whisper without the FFmpeg filter; the remembered choice silently counts as automatic
        let (v, cur) = asr_offer(true, Some("Whisper Base".into()), false, "whisper");
        assert_eq!(ids(&v), ["auto", "parakeet"]);
        assert_eq!((cur, asr_force(cur)), ("auto", None));
        // Parakeet missing
        let (v, cur) = asr_offer(false, Some("Whisper Base".into()), true, "parakeet");
        assert_eq!(ids(&v), ["auto", "whisper"]);
        assert_eq!(cur, "auto");
        let (_, cur) = asr_offer(true, None, true, "parakeet");
        assert_eq!(asr_force(cur), Some(Engine::Parakeet));
        // unknown value
        assert_eq!(asr_offer(true, None, true, "gibts-nicht").1, "auto");
    }

    fn video(dir: &std::path::Path, name: &str, args: &[&str]) -> MediaRef {
        let path = dir.join(name);
        let st = cl_media::ffmpeg_command().unwrap().args(["-v", "error", "-y", "-threads", "2"]).args(args).arg(&path).status().unwrap();
        assert!(st.success());
        cl_media::probe(&path).unwrap()
    }

    /// „Zensieren …“ only with a selected video/image clip; the frame from the player becomes
    /// (mirrored back into the source frame) the area, 15 % larger; further ones take over the kind.
    #[test]
    fn censor_action_and_area_from_the_player_box() {
        let mut app = crate::saving::tests::test_app();
        let ctx = egui::Context::default();
        let img = MediaRef {
            id: MediaId::new(),
            path: "bild.png".into(),
            kind: MediaKind::Image,
            duration: Time::ZERO,
            width: 640,
            height: 360,
            fps: None,
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
        };
        let mut p = Project::new("Zensur");
        let mut t = Track::new(TrackKind::Video);
        let mut c = Clip::new(ClipContent::Media { media: img.id, kind: MediaKind::Image }, Time::ZERO, Time::from_secs_f64(3.0));
        c.fx.flip_h = true;
        let (id, text) = (c.id, Clip::new(ClipContent::Text(TextStyle::default()), Time::ZERO, Time::from_secs_f64(1.0)));
        let text_id = text.id;
        t.clips.push(c);
        let mut t2 = Track::new(TrackKind::Text);
        t2.clips.push(text);
        p.media.push(img);
        p.timeline.tracks.extend([t, t2]);
        app.set_doc(p);
        assert!(!app.action_enabled("censor"));
        app.select_only(Some(text_id));
        assert!(!app.action_enabled("censor"));
        app.select_only(Some(id));
        assert!(app.action_enabled("censor"));
        app.run_action(&ctx, "censor");
        assert!(app.pick == Some(Pick::Censor(id)));
        app.playhead = Time::from_secs_f64(1.0);
        app.add_censor(&ctx, id, cl_analyze::track::BoxN { cx: 0.2, cy: 0.4, w: 0.2, h: 0.1 });
        let cs = &app.clip_view(id).unwrap().fx.censor;
        assert_eq!(cs.len(), 1);
        let k = cs[0].path[0];
        assert!((k.cx - 0.8).abs() < 1e-6 && (k.cy - 0.4).abs() < 1e-6, "{k:?}");
        assert!((k.w - 0.23).abs() < 1e-6 && (k.h - 0.115).abs() < 1e-6, "{k:?}");
        assert!(cs[0].track && !cs[0].blur && cs[0].strength == 0.0);
        // the second area takes over kind and shape of the last one
        let mut c = app.clip_view(id).unwrap();
        (c.fx.censor[0].blur, c.fx.censor[0].ellipse) = (true, true);
        app.apply(EditOp::ReplaceClip { clip: c });
        app.add_censor(&ctx, id, cl_analyze::track::BoxN { cx: 0.6, cy: 0.6, w: 0.1, h: 0.1 });
        let cs = &app.clip_view(id).unwrap().fx.censor;
        assert_eq!(cs.len(), 2);
        assert!(cs[1].blur && cs[1].ellipse && (cs[1].path[0].cx - 0.4).abs() < 1e-6);
        // Take over/paste the look: areas belong to the clip
        let look = crate::presets::Preset::from_clip("x", &app.clip_view(id).unwrap());
        assert!(look.fx.censor.is_empty());
        let mut other = app.clip_view(id).unwrap();
        look.apply(&mut other);
        assert_eq!(other.fx.censor.len(), 2);
    }

    /// A tracking result finds its area via the ID, not the list position:
    /// removing before/after, undo, splitting and deleting the clip during tracking.
    #[test]
    fn censor_tracking_result_goes_to_its_own_area() {
        let mut app = crate::saving::tests::test_app();
        let ctx = egui::Context::default();
        let m = MediaRef {
            id: MediaId::new(),
            path: "film.mp4".into(),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(10.0),
            width: 640,
            height: 360,
            fps: None,
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
        };
        let media = m.id;
        let c = Clip::new(ClipContent::Media { media, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(4.0));
        let clip = c.id;
        let mut p = Project::new("Zensur");
        let mut t = Track::new(TrackKind::Video);
        t.clips.push(c);
        p.media.push(m);
        p.timeline.tracks.push(t);
        let key = |x: f32| CensorKey { t: Time::ZERO, cx: x, cy: 0.5, w: 0.1, h: 0.1 };
        let tracked = vec![key(0.9), CensorKey { t: Time::from_secs_f64(2.0), ..key(0.95) }];
        let fresh = |app: &mut App| {
            let mut p = p.clone();
            p.timeline.tracks[0].clips[0].fx.censor = [0.1, 0.2, 0.3]
                .map(|x| Censor { track: true, path: vec![key(x)], ..Default::default() })
                .to_vec();
            app.set_doc(p);
            app.clip_view(clip).unwrap().fx.censor.iter().map(|cs| cs.id).collect::<Vec<_>>()
        };
        let xs = |app: &App, clip: ClipId| {
            app.clip_view(clip).unwrap().fx.censor.iter().map(|cs| (cs.path.len(), cs.path[0].cx)).collect::<Vec<_>>()
        };
        let remove = |app: &mut App, i: usize| {
            let mut c = app.clip_view(clip).unwrap();
            c.fx.censor.remove(i);
            app.apply(EditOp::ReplaceClip { clip: c });
        };
        // B is tracked, A before it removed: B (now position 0) gets the path, C stays
        let ids = fresh(&mut app);
        remove(&mut app, 0);
        app.censor_tracked(media, ids[1], tracked.clone());
        assert_eq!(xs(&app, clip), vec![(2, 0.9), (1, 0.3)]);
        // C after it removed
        let ids = fresh(&mut app);
        remove(&mut app, 2);
        app.censor_tracked(media, ids[1], tracked.clone());
        assert_eq!(xs(&app, clip), vec![(1, 0.1), (2, 0.9)]);
        // B itself removed: result discarded, no history step
        let ids = fresh(&mut app);
        remove(&mut app, 1);
        let rev = app.doc.lock().unwrap().revision();
        app.censor_tracked(media, ids[1], tracked.clone());
        assert_eq!((xs(&app, clip), app.doc.lock().unwrap().revision()), (vec![(1, 0.1), (1, 0.3)], rev));
        // A removed, then undone: B is at position 1 again and gets the path
        let ids = fresh(&mut app);
        remove(&mut app, 0);
        app.undo();
        app.sync_project();
        app.censor_tracked(media, ids[1], tracked.clone());
        assert_eq!(xs(&app, clip), vec![(1, 0.1), (2, 0.9), (1, 0.3)]);
        // „Verfolgen“ off: discarded
        let ids = fresh(&mut app);
        let mut c = app.clip_view(clip).unwrap();
        c.fx.censor[1].track = false;
        app.apply(EditOp::ReplaceClip { clip: c });
        app.censor_tracked(media, ids[1], tracked.clone());
        assert_eq!(xs(&app, clip)[1], (1, 0.2));
        // a new area via „Zensieren“ gets its own ID, even if it takes over the last one
        let ids = fresh(&mut app);
        app.add_censor(&ctx, clip, cl_analyze::track::BoxN { cx: 0.5, cy: 0.5, w: 0.1, h: 0.1 });
        let all = app.clip_view(clip).unwrap().fx.censor;
        assert!(all[3].id.0 != ids[2].0);
        app.censor_tracked(media, ids[2], tracked.clone());
        assert_eq!(xs(&app, clip)[2..].to_vec(), vec![(2, 0.9), (1, all[3].path[0].cx)]);
        // clip split: both halves get the path (it lies in source time)
        let ids = fresh(&mut app);
        let Some(OpOutput::Clip(right)) = app.apply(EditOp::SplitClip { clip, at: Time::from_secs_f64(2.0) }) else {
            panic!()
        };
        app.censor_tracked(media, ids[1], tracked.clone());
        assert_eq!(xs(&app, clip)[1], (2, 0.9));
        assert_eq!(xs(&app, right)[1], (2, 0.9));
        // clip deleted: discarded
        let ids = fresh(&mut app);
        app.apply(EditOp::RemoveClip { clip });
        let rev = app.doc.lock().unwrap().revision();
        app.censor_tracked(media, ids[1], tracked);
        assert_eq!(app.doc.lock().unwrap().revision(), rev);
    }

    /// Measurement on the substitute medium: a box moves on a curve across the frame (640×360, 30 fps,
    /// 4 s), frame drawn at 1 s. The area covers it in every frame; the number of keyframes and
    /// the tracking time are printed.
    #[test]
    fn censor_tracking_covers_a_moving_object_in_every_frame() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let m = video(dir.path(), "kurve.mp4", &[
            "-f", "lavfi", "-i", "color=0x203040:s=640x360:r=30:d=4",
            "-f", "lavfi", "-i", "testsrc2=s=64x64:r=30:d=4",
            "-filter_complex", "[0][1]overlay=x='60+110*t':y='150+70*sin(1.5*t)'",
            "-pix_fmt", "yuv420p", "-c:v", "libx264", "-crf", "18",
        ]);
        let obj = |t: f64| {
            let (x, y) = ((60.0 + 110.0 * t).floor(), (150.0 + 70.0 * (1.5 * t).sin()).floor());
            CensorKey { t: Time::from_secs_f64(t), cx: ((x + 32.0) / 640.0) as f32, cy: ((y + 32.0) / 360.0) as f32, w: 64.0 / 640.0, h: 64.0 / 360.0 }
        };
        let o = obj(1.0);
        let init = cl_analyze::track::BoxN { cx: o.cx, cy: o.cy, w: o.w, h: o.h };
        let t0 = std::time::Instant::now();
        let boxes = cl_analyze::track::track_box(&m.path, 640, 360, 0.0, 4.0, 1.0, init, &|_| {}, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        let secs = t0.elapsed().as_secs_f64();
        let keys: Vec<CensorKey> = boxes.iter().map(|(t, b)| CensorKey { t: Time::from_secs_f64(*t), cx: b.cx, cy: b.cy, w: b.w, h: b.h }).collect();
        let cs = Censor { path: censor_simplify(&keys), ..Default::default() };
        eprintln!(
            "Verfolgung: {} Rahmen, {} Keyframes, {:.2} s Rechenzeit je Sekunde Video",
            boxes.len(),
            cs.path.len(),
            secs / 4.0
        );
        let mut missed = Vec::new();
        for n in 0..120 {
            let t = n as f64 / 30.0;
            if !cs.at(Time::from_secs_f64(t)).covers(&obj(t)) {
                missed.push(n);
            }
        }
        assert!(missed.is_empty(), "not covered in frame {missed:?}");
        assert!(cs.path.len() < boxes.len() / 2, "{} Keyframes", cs.path.len());
    }
}
