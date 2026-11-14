//! Phase-3-Oberfläche: automatische Untertitel + Editor, Hintergrund entfernen,
//! Chroma-Key mit Pipette, Stabilisieren, Bewegung verfolgen, Auto-Reframe,
//! Szenen erkennen, Ton-Werkzeuge, Stimmfilter, Beats und die Modellverwaltung.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use cl_core::*;
use eframe::egui::{self, vec2, Color32, Rect, RichText, Stroke};

use crate::app::App;
use crate::player_edit::Region;
use crate::tasks::TaskDone;
use crate::theme::{self, icon};
use crate::widgets::{self, Edit, Slider};

/// Auswahl im Player für ein Werkzeug.
#[derive(Clone, Copy, PartialEq)]
pub enum Pick {
    /// Farbe für den Chroma-Key aufnehmen
    Chroma(ClipId),
    /// Rahmen um das zu verfolgende Objekt ziehen (Overlay, Video darunter)
    Track { clip: ClipId, target: ClipId },
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
            ui.add(
                egui::ProgressBar::new(t.progress()).fill(theme::ACCENT())
                    .desired_width((ui.available_width() - 36.0).max(20.0))
                    .desired_height(6.0),
            );
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

fn matte_cache(path: &std::path::Path) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    if let Ok(m) = std::fs::metadata(path) {
        m.len().hash(&mut h);
        m.modified().ok().hash(&mut h);
    }
    // nicht im Cache: das Projekt zeigt dauerhaft auf die Maske
    crate::paths::generated_dir()
        .join("matte")
        .join(format!("{:016x}.mkv", h.finish()))
}

impl App {
    pub fn models(&self) -> cl_analyze::Models {
        cl_analyze::Models::new(crate::paths::models_dir())
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

    // ------------------------------------------------------------ Ergebnisse

    pub fn poll_tasks(&mut self) {
        for (_, done) in self.tasks.poll() {
            match done {
                TaskDone::Failed(e) => self.toast_err(e),
                TaskDone::Run(f) => f(self),
                TaskDone::Proxy { media, ok } => {
                    if ok {
                        self.invalidate_preview();
                    } else {
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
        // Whisper liefert fertige Zeilen: auf die Länge der Vorlage bringen
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
        let Some(mut c) = self.clip_view(clip) else {
            return;
        };
        let ClipContent::Media { media, .. } = c.content else {
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
        let (fw, fh) = (m.width as f32 * k, m.height as f32 * k);
        let s = (w / fw).max(h / fh);
        let (dw, dh) = (fw * s, fh * s);
        let (mx, my) = (((dw - w) / 2.0).max(0.0), ((dh - h) / 2.0).max(0.0));
        c.transform.scale = s;
        for p in [Prop::X, Prop::Y, Prop::Scale] {
            c.keyframes.0.remove(&p);
        }
        for (i, (t, fx, fy)) in pts.iter().enumerate() {
            if i % 2 != 0 && i + 1 != pts.len() {
                continue;
            }
            let tl = Time::from_secs_f64(t / c.speed.max(0.01) as f64);
            if tl > c.duration {
                break;
            }
            if mx > 0.5 {
                c.keyframes
                    .set(Prop::X, tl, ((0.5 - fx) * dw).clamp(-mx, mx));
            }
            if my > 0.5 {
                c.keyframes
                    .set(Prop::Y, tl, ((0.5 - fy) * dh).clamp(-my, my));
            }
        }
        self.live = None;
        self.apply(EditOp::ReplaceClip { clip: c });
        self.toast("Auto-Reframe angewendet");
    }

    // ------------------------------------------------------------ Starten

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
            ..Default::default()
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
                        // Ton der ganzen Zeitleiste mischen
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

    fn start_matte(&mut self, ctx: &egui::Context, c: &Clip) {
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
            let mut c2 = c.clone();
            c2.fx.cutout = Some(out.to_string_lossy().into_owned());
            self.live = None;
            self.apply(EditOp::ReplaceClip { clip: c2 });
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

    /// Videoclip unter einem Overlay am Playhead (für „Bewegung verfolgen“).
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

    // ------------------------------------------------------------ Inspektor

    /// Reiter „Smart“ (Video/Bild); bei Text nur „Bewegung verfolgen“ im Reiter „Basis“.
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
            if is_video {
                let running = self.tasks.running(&format!("matte:{}", c.id)).is_some();
                let mut on = c.fx.cutout.is_some() || running;
                if widgets::toggle_row(ui, "Hintergrund entfernen", &mut on) {
                    if running {
                        if let Some(t) = self.tasks.running(&format!("matte:{}", c.id)) {
                            t.cancel();
                        }
                    } else if on {
                        self.start_matte(&ctx, c);
                    } else {
                        c.fx.cutout = None;
                        e.commit = true;
                    }
                }
                task_bar(ui, self, &format!("matte:{}", c.id));
                if c.fx.cutout.is_some() {
                    // Personen-Effekte: was mit dem Hintergrund passiert
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
                        Some(PersonFx::Outline { color, width }) => {
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
                        }
                        None => {}
                    }
                }
            }
            let mut ck = c.fx.chroma.is_some();
            if widgets::toggle_row(ui, "Chroma-Key (Greenscreen)", &mut ck) {
                c.fx.chroma = ck.then(ChromaKey::default);
                e.commit = true;
            }
            if let Some(k) = &mut c.fx.chroma {
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

    /// Zusätze im Reiter „Audio“ (Störgeräusche, Klang, Audiofilter, Stimmfilter, Beats).
    pub fn pro_audio(&mut self, ui: &mut egui::Ui, c: &mut Clip, e: &mut Edit) {
        let ctx = ui.ctx().clone();
        *e |= audio_fx_ui(ui, &mut c.audio);
        // Beats: im letzten Abschnitt, kein eigener Abschnitt für einen Knopf
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

    // ------------------------------------------------------------ Untertitel

    pub fn captions_pro(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let m = self.models();
        let engine = if m.installed("parakeet") && m.installed("onnxruntime") {
            Some("Parakeet TDT 0.6B v3")
        } else if m.installed("whisper") {
            Some("Whisper")
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
                    RichText::new(crate::tr(match engine {
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
        ui.horizontal(|ui| {
            if ui
                .add(theme::secondary(format!(
                    "{}  SRT importieren",
                    icon::FILE_ARROW_DOWN
                )))
                .clicked()
            {
                if let Some(f) = crate::dialog()
                    .add_filter("Untertitel", &["srt"])
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

    // ------------------------------------------------------------ Player-Auswahl

    /// Pipette bzw. Tracking-Rahmen im Player. Gibt `true` zurück, wenn die Eingabe verbraucht wurde.
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
            Pick::Track { .. } => "Ziehe einen Rahmen um das Objekt",
        };
        let g = painter.layout_no_wrap(
            format!(
                "{}  {hint}  ·  Esc",
                match pick {
                    Pick::Chroma(_) => icon::EYEDROPPER,
                    Pick::Track { .. } => icon::CROSSHAIR,
                }
            ),
            egui::FontId::proportional(theme::FS_TEXT),
            theme::ACCENT_INK(),
        );
        let br = Rect::from_center_size(
            resp.rect.center_top() + vec2(0.0, 22.0),
            g.size() + vec2(20.0, 10.0),
        );
        painter.rect_filled(br, 12.0, theme::ACCENT()); // Pille: Radius = halbe Höhe
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
            Pick::Track { clip, target } => {
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
                            self.start_tracking(ui.ctx(), clip, target, bx);
                        } else {
                            self.toast_err("Das Video darunter ist am Playhead nicht sichtbar");
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

    /// Laufende Aufgaben in der Titelleiste.
    pub fn tasks_badge(&mut self, ui: &mut egui::Ui) {
        if let Some(t) = self.tasks.list.first() {
            let more = self.tasks.list.len() - 1;
            let txt = format!(
                "{}  {} {:.0} %{}",
                icon::SPINNER_GAP,
                t.label.trim_end_matches(" …"),
                t.progress() * 100.0,
                if more > 0 {
                    format!(" (+{more})")
                } else {
                    String::new()
                }
            );
            let r = ui.label(RichText::new(txt).size(theme::FS_UI).color(theme::TEXT_DIM()));
            if r.on_hover_text(crate::tr("Klicken bricht ab"))
                .interact(egui::Sense::click())
                .clicked()
            {
                t.cancel();
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    // ------------------------------------------------------------ Modelle

    pub fn models_settings(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let m = self.models();
        ui.label(RichText::new(crate::tr("Läuft lokal auf deinem Rechner, es werden keine Daten hochgeladen.")).small().color(theme::TEXT_DIM()));
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
                            // Platz für Knopf/Status rechts lassen, sonst läuft der Text darunter
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

/// Ton-Werkzeuge für Clips und Einsprechen: Störgeräusche, Klang, Audiofilter, Stimmfilter.
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
