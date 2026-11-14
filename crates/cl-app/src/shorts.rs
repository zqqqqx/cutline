//! Vom langen Video zum Short: Trefferliste aus „Highlights finden“, je Treffer ein eigenes
//! 9:16-Projekt anlegen, Clips beim Wechsel des Seitenverhältnisses neu einpassen.

use cl_core::*;
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::captions::CapPreset;
use crate::paths;
use crate::tasks::TaskDone;
use crate::theme::{self, icon};

/// Ein gefundener Moment: Projektzeit (s), Stärke (Pegel / Umgebung), gewählt
pub struct Hit {
    pub at: f64,
    pub strength: f32,
    pub on: bool,
}

pub struct ShortsState {
    pub hits: Vec<Hit>,
    /// Länge je Short in Sekunden
    pub len: f32,
    pub captions: bool,
}

/// Wie viel lauter der Moment `t` (s) als seine Umgebung (±5 s) ist.
pub fn strength(env: &[f32], t: f64) -> f32 {
    let r = cl_analyze::audio::ENV_RATE as f64;
    let mean = |a: f64, b: f64| {
        let (a, b) = (((a * r).max(0.0) as usize).min(env.len()), ((b * r) as usize).min(env.len()));
        env[a..b].iter().sum::<f32>() / (b - a).max(1) as f32
    };
    mean(t - 0.25, t + 0.25) / mean(t - 5.0, t + 5.0).max(1e-6)
}

/// Zeitbereich eines Shorts um den Moment `at`: Anlauf davor, im Projekt gehalten.
pub fn short_range(at: f64, len: f64, total: f64) -> (f64, f64) {
    let a = (at - len * 0.6).clamp(0.0, (total - len).max(0.0));
    (a, (a + len).min(total))
}

/// Medien-Clip neu ins Bild `w`×`h` setzen: füllen (beschneiden) oder ganz einpassen.
pub fn fit_clip(c: &mut Clip, m: &MediaRef, w: f32, h: f32, fill: bool) {
    let (sx, sy) = (w / m.width.max(1) as f32, h / m.height.max(1) as f32);
    // Skalierung 1 = eingepasst (wie im Player/Export)
    c.transform.scale = if fill { sx.max(sy) / sx.min(sy) } else { 1.0 };
    c.transform.x = 0.0;
    c.transform.y = 0.0;
    for p in [Prop::X, Prop::Y, Prop::Scale] {
        c.keyframes.0.remove(&p);
    }
}

/// Bildclips (Video/Bild mit bekannter Größe) samt Medium
fn visual<'a>(p: &'a Project, c: &Clip) -> Option<&'a MediaRef> {
    match &c.content {
        ClipContent::Media { media, kind: MediaKind::Video | MediaKind::Image } => {
            p.media(*media).filter(|m| m.width > 0 && m.height > 0)
        }
        _ => None,
    }
}

/// Projekt + Zeitbereich → neues 9:16-Projekt (1080×1920, gleiche Bildrate): alle Spuren,
/// Clips auf den Bereich zugeschnitten und auf 0 verschoben, Bildclips füllen das Format.
/// `captions`: Untertitel-Spuren und Stil, der darauf angewendet wird.
pub fn make_short(p: &Project, a: Time, b: Time, name: &str, captions: Option<(&[TrackId], &CapPreset)>) -> Project {
    let mut out = p.clone();
    out.name = name.to_string();
    let old_h = p.settings.height.max(2) as f32;
    out.settings.width = 1080;
    out.settings.height = 1920;
    out.settings.aspect = AspectPreset::Ratio(9, 16);
    out.settings.platform = None;
    let (w, h) = (1080.0, 1920.0);
    for t in &mut out.timeline.tracks {
        let style = captions.filter(|(ids, _)| ids.contains(&t.id)).map(|(_, s)| s);
        let clips = std::mem::take(&mut t.clips);
        for mut c in clips {
            if c.end() <= a || c.start >= b {
                continue;
            }
            let head = if c.start < a { a - c.start } else { Time::ZERO };
            let tail = if c.end() > b { c.end() - b } else { Time::ZERO };
            let src = |d: Time| Time((d.0 as f64 * c.speed.max(0.01) as f64).round() as i64);
            // wie beim Teilen: rückwärts zeigt der Anfang das Ende der Quelle
            if c.fx.reverse {
                c.source_in = c.source_in + src(tail);
            } else {
                c.source_in = c.source_in + src(head);
            }
            if head > Time::ZERO {
                c.keyframes.shift(head);
                c.anim.intro = None;
                c.fade_in = Time::ZERO;
            }
            if tail > Time::ZERO {
                c.anim.outro = None;
                c.transition_out = None;
                c.fade_out = Time::ZERO;
            }
            c.duration = c.duration - head - tail;
            c.start = if c.start > a { c.start - a } else { Time::ZERO };
            if let Some(m) = visual(p, &c) {
                fit_clip(&mut c, m, w, h, true);
            } else if let ClipContent::Text(s) = &mut c.content {
                // gleiche Höhe im Bild wie vorher
                c.transform.y *= h / old_h;
                if let Some(pre) = style {
                    *s = TextStyle { text: std::mem::take(&mut s.text), ..(pre.style)() };
                    c.transform.y = h * pre.y;
                    c.anim.intro = pre.intro.map(|id| AnimRef {
                        id: id.into(),
                        duration: Time::from_secs_f64(0.18f64.min(c.duration.as_secs_f64() / 3.0)),
                    });
                }
            }
            t.clips.push(c);
        }
    }
    out.timeline.markers.retain(|m| m.at >= a && m.at < b);
    for m in &mut out.timeline.markers {
        m.at = m.at - a;
    }
    out
}

impl App {
    /// Fenster mit der Trefferliste von „Highlights finden“
    pub fn shorts_window(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.shorts.take() else {
            return;
        };
        let total = self.project.duration().as_secs_f64();
        let has_caps = !self.caption_clips().is_empty();
        let max = st.hits.iter().map(|h| h.strength).fold(0.0f32, f32::max).max(1e-6);
        let (mut open, mut make, mut markers) = (true, false, false);
        egui::Window::new(crate::tr("Highlights"))
            .id(egui::Id::new("shorts"))
            .open(&mut open)
            .collapsible(false)
            .default_width(400.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for h in &mut st.hits {
                        let (a, b) = short_range(h.at, st.len as f64, total);
                        ui.horizontal(|ui| {
                            crate::widgets::check(ui, &mut h.on, "");
                            if ui
                                .add(theme::ghost(fmt_t(h.at)))
                                .on_hover_text(crate::tr("Zum Moment springen"))
                                .clicked()
                            {
                                self.playhead = Time::from_secs_f64(h.at);
                            }
                            ui.label(RichText::new(format!("{:.0} s", b - a)).color(theme::TEXT_DIM()));
                            ui.label(
                                RichText::new(crate::tr(format!("Stärke {:.0} %", h.strength / max * 100.0)))
                                    .color(theme::TEXT_DIM()),
                            );
                        });
                    }
                });
                theme::section(ui, "Short");
                theme::row(ui, "Länge je Short", |ui| {
                    crate::widgets::Slider::new(&mut st.len, 10.0..=60.0).default(30.0).suffix(" s").show(ui);
                });
                ui.add_enabled_ui(has_caps, |ui| {
                    let mut on = st.captions && has_caps;
                    crate::widgets::check(ui, &mut on, crate::tr("Untertitel-Stil anwenden"));
                    st.captions = on;
                    if on {
                        self.caption_preset_picker(ui);
                    }
                });
                if !has_caps {
                    ui.label(
                        RichText::new(crate::tr("Keine Untertitel im Projekt – erst Untertitel erzeugen"))
                            .small()
                            .color(theme::TEXT_FAINT()),
                    );
                }
                ui.add_space(8.0);
                let n = st.hits.iter().filter(|h| h.on).count();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(n > 0, theme::primary(format!("{}  Als Shorts anlegen", icon::DEVICE_MOBILE))).clicked() {
                            make = true;
                        }
                        if ui.add_enabled(n > 0, theme::secondary(format!("{}  Als Marker setzen", icon::FLAG))).clicked() {
                            markers = true;
                        }
                    });
                });
                if n > 0 && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    make = true;
                }
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        let chosen: Vec<f64> = st.hits.iter().filter(|h| h.on).map(|h| h.at).collect();
        if markers {
            let mut ms = self.project.timeline.markers.clone();
            let first = ms.iter().map(|m| m.id).max().unwrap_or(0) + 1;
            for (i, t) in chosen.iter().enumerate() {
                ms.push(Marker {
                    id: first + i as u64,
                    at: Time::from_secs_f64(*t),
                    duration: Time::ZERO,
                    name: format!("Highlight {}", i + 1),
                    note: String::new(),
                    color: 4,
                });
            }
            self.apply(EditOp::SetMarkers { markers: ms });
            self.toast(format!("{} Highlights als Marker gesetzt", chosen.len()));
            open = false;
        }
        if make {
            self.create_shorts(&chosen, st.len as f64, st.captions && has_caps);
            open = false;
        }
        if open {
            self.shorts = Some(st);
        }
    }

    fn create_shorts(&mut self, hits: &[f64], len: f64, captions: bool) {
        self.commit_live();
        let total = self.project.duration().as_secs_f64();
        let tracks = self.caption_tracks();
        let caps = captions.then(|| (tracks.as_slice(), self.caption_preset()));
        let dir = paths::projects_dir();
        let mut n = 0;
        for (i, at) in hits.iter().enumerate() {
            let (a, b) = short_range(*at, len, total);
            let name = format!("{} – Short {}", self.project.name, i + 1);
            let proj = make_short(&self.project, Time::from_secs_f64(a), Time::from_secs_f64(b), &name, caps);
            let base = paths::safe_name(&name);
            let mut p = dir.join(format!("{}.cutline", base.trim()));
            let mut k = 2;
            while p.exists() {
                p = dir.join(format!("{} ({k}).cutline", base.trim()));
                k += 1;
            }
            let r = std::fs::create_dir_all(&dir).and_then(|_| paths::write_atomic(&p, proj.to_json()));
            match r {
                Ok(()) => {
                    self.settings.add_recent(&p);
                    n += 1;
                }
                Err(e) => self.toast_err(e.to_string()),
            }
        }
        if n > 0 {
            self.settings.save();
            self.home.refresh();
            self.toast(format!("{n} Shorts angelegt in {}", dir.display()));
        }
    }

    /// Nach dem Wechsel des Seitenverhältnisses: Clips neu einpassen? (ein Undo-Schritt)
    pub fn aspect_fit_dialog(&mut self, ctx: &egui::Context) {
        let Some(s) = self.aspect_fit.clone() else {
            return;
        };
        let mut pick: Option<u8> = None;
        egui::Modal::new(egui::Id::new("aspect-fit")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.label(RichText::new(crate::tr("Clips neu einpassen?")).strong().size(theme::FS_TITLE));
            ui.add_space(6.0);
            ui.label(RichText::new(crate::tr("Das Seitenverhältnis ändert sich. Wie sollen vorhandene Bilder ins neue Format?")).color(theme::TEXT_DIM()));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(theme::primary("Füllen")).on_hover_text(crate::tr("Bild füllt das Format, Ränder werden beschnitten")).clicked() {
                        pick = Some(1);
                    }
                    if ui.add(theme::secondary("Einpassen")).on_hover_text(crate::tr("Ganzes Bild sichtbar, mit Balken")).clicked() {
                        pick = Some(2);
                    }
                    if ui.add(theme::secondary("Auto-Reframe")).on_hover_text(crate::tr("Füllen und dem Wichtigen im Bild folgen (Analyse im Hintergrund)")).clicked() {
                        pick = Some(3);
                    }
                    if ui.add(theme::ghost("So lassen")).clicked() {
                        pick = Some(0);
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                pick = Some(1);
            }
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.aspect_fit = None;
            return;
        }
        let Some(mode) = pick else { return };
        self.aspect_fit = None;
        let (w, h) = (s.width as f32, s.height as f32);
        let clips: Vec<(Clip, MediaRef)> = self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| !t.locked)
            .flat_map(|t| t.clips.iter())
            .filter_map(|c| visual(&self.project, c).map(|m| (c.clone(), m.clone())))
            .collect();
        let mut ops = vec![EditOp::SetSettings { settings: s }];
        if mode > 0 {
            for (c, m) in &clips {
                let mut c = c.clone();
                fit_clip(&mut c, m, w, h, mode != 2);
                ops.push(EditOp::ReplaceClip { clip: c });
            }
        }
        self.apply(EditOp::Batch { label: "Seitenverhältnis".into(), ops });
        if mode == 3 {
            for (c, m) in clips.into_iter().filter(|(_, m)| m.kind == MediaKind::Video) {
                let (start, span, id) = (c.source_time(c.start).as_secs_f64(), c.source_span().as_secs_f64(), c.id);
                self.tasks.spawn(ctx, format!("reframe:{id}"), "Auto-Reframe analysiert …", move |p, cancel| {
                    match cl_analyze::reframe::focus_path(&m.path, m.width, m.height, start, span, p, cancel) {
                        Ok(points) => TaskDone::Reframe { clip: id, points },
                        Err(e) => TaskDone::Failed(e),
                    }
                });
            }
        }
    }

    /// Hat das Projekt Bildclips, die beim Formatwechsel neu eingepasst werden können?
    pub fn has_visual_clips(&self) -> bool {
        self.project.timeline.tracks.iter().flat_map(|t| t.clips.iter()).any(|c| visual(&self.project, c).is_some())
    }
}

fn fmt_t(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_is_trimmed_shifted_and_vertical() {
        let mut p = Project::new("Lang");
        p.settings.width = 1920;
        p.settings.height = 1080;
        let m = MediaRef {
            id: MediaId::new(),
            path: "a.mp4".into(),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(60.0),
            width: 1920,
            height: 1080,
            fps: None,
            has_audio: true,
            video_codec: None,
            audio_codec: None,
        };
        let mid = m.id;
        p.media.push(m);
        let mut t = Track::new(TrackKind::Video);
        let mut c = Clip::new(ClipContent::Media { media: mid, kind: MediaKind::Video }, Time::from_secs_f64(5.0), Time::from_secs_f64(20.0));
        c.source_in = Time::from_secs_f64(1.0);
        t.clips.push(c);
        // liegt ganz außerhalb
        t.clips.push(Clip::new(ClipContent::Media { media: mid, kind: MediaKind::Video }, Time::from_secs_f64(40.0), Time::from_secs_f64(5.0)));
        p.timeline.tracks.push(t);
        let s = make_short(&p, Time::from_secs_f64(10.0), Time::from_secs_f64(20.0), "Lang – Short 1", None);
        assert_eq!((s.settings.width, s.settings.height), (1080, 1920));
        assert_eq!(s.settings.fps, p.settings.fps);
        let cs = &s.timeline.tracks[0].clips;
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].start, Time::ZERO);
        assert!((cs[0].duration.as_secs_f64() - 10.0).abs() < 1e-3);
        assert!((cs[0].source_in.as_secs_f64() - 6.0).abs() < 1e-3);
        // 16:9 füllt 9:16: (1920/1080)² ≈ 3,16
        assert!((cs[0].transform.scale - 3.16).abs() < 0.01, "{}", cs[0].transform.scale);
        assert_eq!(short_range(5.0, 30.0, 100.0), (0.0, 30.0));
        assert_eq!(short_range(95.0, 30.0, 100.0), (70.0, 100.0));
        let env: Vec<f32> = (0..1000).map(|i| if (495..505).contains(&i) { 1.0 } else { 0.1 }).collect();
        assert!(strength(&env, 5.0) > strength(&env, 2.0));
    }
}
