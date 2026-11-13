//! Navigation und Bereich: J/K/L-Shuttle, In/Out-Punkte, Marker (setzen, springen,
//! bearbeiten, Liste, YouTube-Kapitel), Ripple-Löschen und Sprung zum nächsten Schnitt.

use cl_core::{EditOp, Marker, Time, MARKER_COLORS};
use eframe::egui::{self, Color32, RichText};

use crate::app::App;
use crate::theme::{self, icon};

pub fn marker_color(i: u8) -> Color32 {
    let [r, g, b] = MARKER_COLORS[(i as usize).min(MARKER_COLORS.len() - 1)].1;
    Color32::from_rgb(r, g, b)
}

/// „1:23“ bzw. „1:02:03“ wie in YouTube-Kapiteln
pub fn chapter_time(t: Time) -> String {
    let s = t.as_secs_f64().max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Kapitelliste für die Videobeschreibung. YouTube verlangt ein Kapitel bei 0:00.
pub fn chapters(markers: &[Marker]) -> String {
    let mut v: Vec<(Time, String)> = markers
        .iter()
        .map(|m| {
            (
                m.at,
                if m.name.is_empty() {
                    "Kapitel".to_string()
                } else {
                    m.name.clone()
                },
            )
        })
        .collect();
    v.sort_by_key(|m| m.0);
    if v.first().is_none_or(|m| m.0 > Time::ZERO) {
        v.insert(0, (Time::ZERO, "Intro".into()));
    }
    v.iter()
        .map(|(t, n)| format!("{} {n}", chapter_time(*t)))
        .collect::<Vec<_>>()
        .join("\n")
}

impl App {
    /// J/L: abspielen in Richtung `dir`; nochmal drücken verdoppelt das Tempo (bis 8×).
    pub fn shuttle(&mut self, dir: f64) {
        self.commit_live();
        let same = self.playing && self.play_rate.signum() == dir.signum();
        let rate = if same {
            (self.play_rate * 2.0).clamp(-8.0, 8.0)
        } else {
            dir
        };
        if self.playing {
            self.stop_playback();
        }
        if dir > 0.0 && self.playhead >= self.last_frame() {
            self.playhead = Time::ZERO;
        }
        self.play_rate = rate;
        self.playing = true;
        self.playbacks.clear();
        self.restart_audio();
    }

    pub fn set_range_point(&mut self, is_in: bool) {
        let t = self.playhead.snap_to_frame(self.project.settings.fps);
        if is_in {
            self.range_in = Some(t);
            if self.range_out.is_some_and(|o| o <= t) {
                self.range_out = None;
            }
        } else {
            self.range_out = Some(t);
            if self.range_in.is_some_and(|i| i >= t) {
                self.range_in = None;
            }
        }
    }

    fn next_marker_id(&self) -> u64 {
        self.project
            .timeline
            .markers
            .iter()
            .map(|m| m.id)
            .max()
            .unwrap_or(0)
            + 1
    }

    /// M: Marker am Playhead setzen; liegt dort schon einer, wird er bearbeitet.
    pub fn add_marker(&mut self) {
        let fps = self.project.settings.fps;
        let frame = Time::from_secs_f64(1.0 / fps.as_f64());
        let at = self.playhead.snap_to_frame(fps);
        if let Some(m) = self
            .project
            .timeline
            .markers
            .iter()
            .find(|m| (m.at.0 - at.0).abs() < frame.0.max(1))
        {
            self.marker_edit = Some(m.id);
            return;
        }
        let mut ms = self.project.timeline.markers.clone();
        let id = self.next_marker_id();
        ms.push(Marker {
            id,
            at,
            duration: Time::ZERO,
            name: String::new(),
            note: String::new(),
            color: 0,
        });
        self.apply(EditOp::SetMarkers { markers: ms });
    }

    pub fn jump_marker(&mut self, forward: bool) {
        let ph = self.playhead;
        let eps = 1000;
        let m = if forward {
            self.project
                .timeline
                .markers
                .iter()
                .map(|m| m.at)
                .filter(|t| t.0 > ph.0 + eps)
                .min()
        } else {
            self.project
                .timeline
                .markers
                .iter()
                .map(|m| m.at)
                .filter(|t| t.0 < ph.0 - eps)
                .max()
        };
        if let Some(t) = m {
            self.stop_playback();
            self.playhead = t;
        }
    }

    /// ↑/↓: zum vorherigen/nächsten Schnitt (Clipanfang oder -ende auf irgendeiner Spur).
    pub fn jump_edit(&mut self, forward: bool) {
        let ph = self.playhead.0;
        let eps = 1000;
        let edges = self
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .flat_map(|c| [c.start.0, c.end().0]);
        let t = if forward {
            edges.filter(|t| *t > ph + eps).min()
        } else {
            edges.filter(|t| *t < ph - eps).max()
        };
        if let Some(t) = t {
            self.stop_playback();
            self.playhead = Time(t);
        }
    }

    /// Umschalt+Entf: Auswahl löschen und die Lücke auf derselben Spur schließen.
    pub fn ripple_delete(&mut self) {
        self.live = None;
        let sel = self.selection();
        if sel.is_empty() {
            return;
        }
        let mut ops = Vec::new();
        for t in &self.project.timeline.tracks {
            let mut gone: Vec<(Time, Time)> = t
                .clips
                .iter()
                .filter(|c| sel.contains(&c.id))
                .map(|c| (c.start, c.duration))
                .collect();
            if gone.is_empty() {
                continue;
            }
            gone.sort_by_key(|g| g.0);
            for c in t.clips.iter().filter(|c| sel.contains(&c.id)) {
                ops.push(EditOp::RemoveClip { clip: c.id });
            }
            // Hauptspur mit Magnet schließt die Lücke selbst
            if t.main && self.project.settings.main_track_magnet {
                continue;
            }
            for c in t.clips.iter().filter(|c| !sel.contains(&c.id)) {
                let shift: i64 = gone
                    .iter()
                    .filter(|(s, _)| s.0 < c.start.0)
                    .map(|(_, d)| d.0)
                    .sum();
                if shift > 0 {
                    ops.push(EditOp::MoveClip {
                        clip: c.id,
                        track: Some(t.id),
                        start: Time(c.start.0 - shift),
                    });
                }
            }
        }
        let first = sel
            .iter()
            .filter_map(|c| self.project.find_clip(*c).map(|(_, c)| c.start))
            .min();
        self.select_only(None);
        if self.apply_batch(ops).is_some() {
            if let Some(t) = first {
                self.stop_playback();
                self.playhead = t;
            }
        }
    }

    /// Marker-Editor (Fenster) und Marker-Liste im Projekt-Inspektor.
    pub fn marker_dialog(&mut self, ctx: &egui::Context) {
        let Some(id) = self.marker_edit else { return };
        let Some(orig) = self
            .project
            .timeline
            .markers
            .iter()
            .find(|m| m.id == id)
            .cloned()
        else {
            self.marker_edit = None;
            return;
        };
        // Entwurf, damit Tippen nicht für jeden Buchstaben einen Undo-Schritt erzeugt
        let mut m = self
            .marker_draft
            .clone()
            .filter(|d| d.id == id)
            .unwrap_or_else(|| orig.clone());
        let mut close = false;
        let mut delete = false;
        egui::Window::new(crate::tr("Marker"))
            .id(egui::Id::new("marker-edit"))
            .collapsible(false)
            .resizable(false)
            .default_width(300.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -80.0))
            .show(ctx, |ui| {
                theme::row(ui, "Name", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut m.name)
                            .hint_text(crate::tr("z. B. Intro, Szene 2"))
                            .desired_width(ui.available_width() - 12.0),
                    )
                });
                theme::row(ui, "Farbe", |ui| {
                    for (i, _) in MARKER_COLORS.iter().enumerate() {
                        let (r, resp) =
                            ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                        ui.painter()
                            .circle_filled(r.center(), 7.0, marker_color(i as u8));
                        if m.color as usize == i {
                            ui.painter().circle_stroke(
                                r.center(),
                                8.5,
                                egui::Stroke::new(1.5_f32, theme::ACCENT),
                            );
                        }
                        if resp.on_hover_text(crate::tr(MARKER_COLORS[i].0)).clicked() {
                            m.color = i as u8;
                        }
                    }
                });
                let mut dur = m.duration.as_secs_f64() as f32;
                theme::row(ui, "Dauer", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut dur)
                            .range(0.0..=3600.0)
                            .speed(0.05)
                            .suffix(" s"),
                    )
                });
                m.duration = Time::from_secs_f64(dur as f64);
                ui.label(
                    RichText::new(crate::tr("Notiz"))
                        .small()
                        .color(theme::TEXT_DIM),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut m.note)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::ghost(format!("{}  Löschen", icon::TRASH)))
                        .clicked()
                    {
                        delete = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::primary("Fertig")).clicked() {
                            close = true;
                        }
                    });
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        let mut ms = self.project.timeline.markers.clone();
        if delete {
            ms.retain(|x| x.id != id);
            self.apply(EditOp::SetMarkers { markers: ms });
            self.marker_edit = None;
            self.marker_draft = None;
            return;
        }
        if close {
            if m != orig {
                if let Some(x) = ms.iter_mut().find(|x| x.id == id) {
                    *x = m;
                }
                self.apply(EditOp::SetMarkers { markers: ms });
            }
            self.marker_edit = None;
            self.marker_draft = None;
        } else {
            self.marker_draft = Some(m);
        }
    }

    /// Kleines Fenster zum Umbenennen einer Spur.
    pub fn track_rename_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, name)) = &mut self.track_rename else {
            return;
        };
        let id = *id;
        let mut done = None;
        egui::Window::new(crate::tr("Spur umbenennen"))
            .id(egui::Id::new("track-rename"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -80.0))
            .show(ctx, |ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text(crate::tr("z. B. Sprecher 1, Musik, Kamera B"))
                        .desired_width(260.0),
                );
                r.request_focus();
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = Some(true);
                }
                ui.horizontal(|ui| {
                    if ui.add(theme::primary("OK")).clicked() {
                        done = Some(true);
                    }
                    if ui.add(theme::secondary("Abbrechen")).clicked() {
                        done = Some(false);
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            done = Some(false);
        }
        if let Some(ok) = done {
            let (_, name) = self.track_rename.take().unwrap_or_default();
            if ok {
                self.apply(EditOp::RenameTrack {
                    track: id,
                    name: Some(name),
                });
            }
        }
    }

    /// Marker-Liste (Projekt-Inspektor).
    pub fn marker_list(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "Marker");
        let ms = self.project.timeline.markers.clone();
        if ms.is_empty() {
            ui.label(
                RichText::new(crate::tr("Setze Marker mit M (am Playhead)"))
                    .small()
                    .color(theme::TEXT_DIM),
            );
            return;
        }
        for m in &ms {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 18.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(r.center(), 4.5, marker_color(m.color));
                if ui
                    .add(
                        egui::Button::new(RichText::new(chapter_time(m.at)).monospace().size(11.0))
                            .frame(false),
                    )
                    .clicked()
                {
                    self.stop_playback();
                    self.playhead = m.at;
                }
                let name = if m.name.is_empty() {
                    crate::tr("Ohne Namen")
                } else {
                    m.name.clone()
                };
                let r = ui.add(
                    egui::Label::new(RichText::new(name).size(11.5))
                        .truncate()
                        .sense(egui::Sense::click()),
                );
                if !m.note.is_empty() {
                    r.clone().on_hover_text(&m.note);
                }
                if r.double_clicked() || r.clicked() {
                    self.stop_playback();
                    self.playhead = m.at;
                    self.marker_edit = Some(m.id);
                }
            });
        }
        ui.add_space(4.0);
        if ui
            .add(theme::secondary(format!(
                "{}  YouTube-Kapitel kopieren",
                icon::COPY
            )))
            .on_hover_text(crate::tr(
                "Kapitelliste aus den Markern für die Videobeschreibung",
            ))
            .clicked()
        {
            ui.ctx().copy_text(chapters(&ms));
            self.toast("Kapitel in die Zwischenablage kopiert");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapters_start_at_zero() {
        let m = |s: f64, n: &str| Marker {
            id: 0,
            at: Time::from_secs_f64(s),
            duration: Time::ZERO,
            name: n.into(),
            note: String::new(),
            color: 0,
        };
        assert_eq!(
            chapters(&[m(75.0, "Boss"), m(3725.0, "Ende")]),
            "0:00 Intro\n1:15 Boss\n1:02:05 Ende"
        );
        assert_eq!(chapters(&[m(0.0, "Start")]), "0:00 Start");
    }
}
