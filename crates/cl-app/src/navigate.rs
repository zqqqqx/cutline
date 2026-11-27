//! Navigation and range: J/K/L shuttle, in/out points, markers (set, jump,
//! edit, list, YouTube chapters), ripple delete and jump to the next cut.

use cl_core::{EditOp, Marker, Time, Track, TrackId, MARKER_COLORS};
use eframe::egui::{self, Color32, RichText};

use crate::app::App;
use crate::theme::{self, icon};

pub fn marker_color(i: u8) -> Color32 {
    let [r, g, b] = MARKER_COLORS[(i as usize).min(MARKER_COLORS.len() - 1)].1;
    Color32::from_rgb(r, g, b)
}

/// Del on the magnet main track with „Alles mitziehen“: the gap closes on all
/// tracks (like Shift+Del), otherwise overlays and markers would stay shifted.
pub fn delete_ripples_all(magnet: bool, all_tracks: bool, sel_on_main: &[bool]) -> bool {
    magnet && all_tracks && !sel_on_main.is_empty() && sel_on_main.iter().all(|m| *m)
}

/// „1:23“ or „1:02:03“ as in YouTube chapters
pub fn chapter_time(t: Time) -> String {
    let s = t.as_secs_f64().max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Chapters (start, title) as embedded in the export.
fn chapter_list(markers: &[Marker]) -> Vec<(Time, String)> {
    cl_export::chapter_list(markers, Time::ZERO, Time(i64::MAX), &crate::tr("Chapter"))
}

/// Chapter list for the video description.
pub fn chapters(markers: &[Marker]) -> String {
    chapter_list(markers)
        .iter()
        .map(|(t, n)| format!("{} {n}", chapter_time(*t)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Does the list break YouTube's rules (at least 3 chapters, each at least 10 s)?
pub fn chapter_issue(markers: &[Marker], start: Time, end: Time) -> Option<String> {
    let v = cl_export::chapter_list(markers, start, end, &crate::tr("Chapter"));
    v.iter().enumerate().find_map(|(i, (t, name))| {
        let stop = v.get(i + 1).map_or(end - start, |n| n.0);
        let secs = (stop - *t).as_secs_f64();
        (secs < 10.0).then(|| format!("Chapter “{name}” is only {} s long – YouTube only shows chapters of 10 s or more", secs.max(0.0).floor()))
    })
}

/// Warning line from [`chapter_issue`] with „Don't show again“; `true` if it was clicked.
pub fn chapter_warning(ui: &mut egui::Ui, w: &str) -> bool {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{}  {}", icon::WARNING, crate::tr(w))).small().color(theme::WARN_TEXT()));
        // small, dimmed link: hover highlights and underlines
        let g = ui.painter().layout_no_wrap(
            crate::tr("Don't show again"),
            egui::TextStyle::Small.resolve(ui.style()),
            Color32::PLACEHOLDER,
        );
        let (rect, r) = ui.allocate_exact_size(g.size(), egui::Sense::click());
        let col = if r.hovered() { theme::TEXT() } else { theme::TEXT_DIM() };
        if r.hovered() {
            ui.painter().hline(rect.x_range(), rect.bottom() - 1.0, egui::Stroke::new(1.0_f32, col));
        }
        ui.painter().galley(rect.min, g, col);
        r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    })
    .inner
}

impl App {
    /// J/L: play in direction `dir`; pressing again doubles the speed (up to 8×).
    pub fn shuttle(&mut self, dir: f64) {
        self.commit_live();
        let same = self.playing && self.core.rate().signum() == dir.signum();
        let rate = if same {
            (self.core.rate() * 2.0).clamp(-8.0, 8.0)
        } else {
            dir
        };
        if self.playing {
            self.stop_playback();
        }
        if dir > 0.0 && self.playhead >= self.last_frame() {
            self.playhead = Time::ZERO;
        }
        self.start_shuttle(rate);
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

    /// M: set a marker at the playhead; if there already is one, it is edited.
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
        // name it right away; if the name stays empty, it is called „Chapter“ (list and export)
        self.marker_draft = None;
        self.marker_edit = Some(id);
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

    /// ↑/↓: to the previous/next cut (clip start or end on any track).
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

    /// Shift+Del: delete the selection and close the gap (depending on „Alles mitziehen“
    /// only on the same track or on all tracks).
    pub fn ripple_delete(&mut self) {
        self.live = None;
        let sel = self.selection();
        if sel.is_empty() {
            return;
        }
        let ranges: Vec<(TrackId, Time, Time)> = self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| !t.locked) // locked tracks stay untouched instead of failing the batch
            .flat_map(|t| {
                t.clips
                    .iter()
                    .filter(|c| sel.contains(&c.id))
                    .map(move |c| (t.id, c.start, c.end()))
            })
            .collect();
        if ranges.is_empty() {
            return;
        }
        let first = ranges.iter().map(|r| r.1).min();
        self.select_only(None);
        if self.ripple_ranges(ranges, "") {
            if let Some(t) = first {
                self.stop_playback();
                self.playhead = t;
            }
        }
    }

    /// Remove ranges (track, from, to) as one undo step and pull everything behind them up.
    /// Shared path for ripple delete, closing gaps, pauses and filler words.
    /// `label`: name in the history (empty = guessed from the steps).
    pub fn ripple_ranges(&mut self, ranges: Vec<(TrackId, Time, Time)>, label: &str) -> bool {
        let all = self.project.settings.ripple_all_tracks;
        // „Alles mitziehen“ takes along clips of other tracks that lie fully in the range: we say so
        let inside = |tid: TrackId, c: &cl_core::Clip, own: bool| {
            ranges.iter().any(|r| (r.0 == tid) == own && r.1 <= c.start && c.end() <= r.2)
        };
        let extra = self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| all && !t.locked)
            .flat_map(|t| t.clips.iter().map(move |c| (t.id, c)))
            .filter(|(tid, c)| inside(*tid, c, false) && !inside(*tid, c, true))
            .count();
        let ops = cl_core::ripple_ops(ranges, all);
        let ok = if label.is_empty() || ops.is_empty() {
            self.apply_batch(ops)
        } else {
            self.apply(EditOp::Batch { label: label.into(), ops })
        }
        .is_some();
        if ok && extra > 0 {
            self.toast(theme::count_text(extra, "{} clip on other tracks removed as well (move everything)", "{} clips on other tracks removed as well (move everything)"));
        }
        ok
    }

    /// Close gaps of one track or (None) of all tracks. With „Alles mitziehen“ and
    /// all tracks only spots where no track shows anything.
    pub fn close_gaps(&mut self, track: Option<TrackId>) {
        let tracks: Vec<&Track> = self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| !t.locked && track.is_none_or(|id| t.id == id))
            .collect();
        let ranges: Vec<(TrackId, Time, Time)> = match tracks.first() {
            Some(t0) if track.is_none() && self.project.settings.ripple_all_tracks => {
                cl_core::gaps(tracks.iter().flat_map(|t| t.clips.iter()))
                    .into_iter()
                    .map(|(a, b)| (t0.id, a, b))
                    .collect()
            }
            _ => tracks
                .iter()
                .flat_map(|t| cl_core::gaps(&t.clips).into_iter().map(move |(a, b)| (t.id, a, b)))
                .collect(),
        };
        if ranges.is_empty() {
            self.toast("No gaps");
        } else {
            // a single track: only it closes up (otherwise content of other tracks would be lost)
            let all = track.is_none() && self.project.settings.ripple_all_tracks;
            self.apply_batch(cl_core::ripple_ops(ranges, all));
        }
    }

    /// Marker editor (window) and marker list in the project inspector.
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
        // Draft, so that typing does not create an undo step for every letter
        let mut m = self
            .marker_draft
            .clone()
            .filter(|d| d.id == id)
            .unwrap_or_else(|| orig.clone());
        let mut close = false;
        let mut delete = false;
        let first = self.marker_draft.as_ref().is_none_or(|d| d.id != id);
        let modal = crate::dialog::modal(ctx, "marker-edit")
            .show(ctx, |ui| {
                ui.set_width(300.0);
                // Cross like Esc: takes over the draft
                close |= crate::dialog::title(ui, "Markers");
                theme::row(ui, "Name", |ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut m.name)
                            .hint_text(crate::tr("e.g. Intro, Scene 2"))
                            .desired_width(ui.available_width() - 12.0),
                    );
                    if first {
                        r.request_focus();
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        close = true;
                    }
                    r
                });
                theme::row(ui, "Color", |ui| {
                    for (i, _) in MARKER_COLORS.iter().enumerate() {
                        let (r, resp) =
                            ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                        ui.painter()
                            .circle_filled(r.center(), 7.0, marker_color(i as u8));
                        if m.color as usize == i {
                            ui.painter().circle_stroke(
                                r.center(),
                                8.5,
                                egui::Stroke::new(1.5_f32, theme::ACCENT()),
                            );
                        }
                        if resp.on_hover_text(crate::tr(MARKER_COLORS[i].0)).clicked() {
                            m.color = i as u8;
                        }
                    }
                });
                let mut dur = m.duration.as_secs_f64() as f32;
                theme::row(ui, "Duration", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut dur)
                            .range(0.0..=3600.0)
                            .speed(0.05)
                            .custom_formatter(|v, _| crate::theme::dec(v, 2))
                            .custom_parser(crate::theme::parse_dec)
                            .suffix(" s"),
                    )
                });
                m.duration = Time::from_secs_f64(dur as f64);
                ui.label(
                    RichText::new(crate::tr("Note~memo"))
                        .small()
                        .color(theme::TEXT_DIM()),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut m.note)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::ghost(format!("{}  Delete", icon::TRASH)))
                        .clicked()
                    {
                        delete = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::primary("Done")).clicked() {
                            close = true;
                        }
                    });
                });
            });
        if modal.should_close() {
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

    /// Small window for renaming a track.
    pub fn track_rename_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, name)) = &mut self.track_rename else {
            return;
        };
        let id = *id;
        let mut done = None;
        let modal = crate::dialog::modal(ctx, "track-rename")
            .show(ctx, |ui| {
                if crate::dialog::title(ui, "Rename track") {
                    done = Some(false);
                }
                let r = ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text(crate::tr("e.g. Speaker 1, Music, Camera B"))
                        .desired_width(260.0),
                );
                r.request_focus();
                // the name field always has focus: Enter confirms the input
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = Some(true);
                }
                match crate::dialog::buttons(ui, theme::primary("OK"), true) {
                    (true, _) => done = Some(true),
                    (_, true) => done = Some(false),
                    _ => {}
                }
            });
        if modal.should_close() {
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

    /// YouTube chapters from the markers to the clipboard (markers tab, ruler menu).
    pub fn copy_chapters(&mut self, ctx: &egui::Context) {
        ctx.copy_text(chapters(&self.project.timeline.markers));
        self.toast("Chapters copied to the clipboard");
    }

    /// Marker list (inspector tab „Markers“).
    pub fn marker_list(&mut self, ui: &mut egui::Ui) {
        let ms = self.project.timeline.markers.clone();
        if ms.is_empty() {
            ui.label(
                RichText::new(crate::tr("Add markers with M (at the playhead)"))
                    .small()
                    .color(theme::TEXT_DIM()),
            );
            return;
        }
        for m in &ms {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 18.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(r.center(), 4.5, marker_color(m.color));
                if ui
                    .add(theme::ghost(chapter_time(m.at)))
                    .clicked()
                {
                    self.stop_playback();
                    self.playhead = m.at;
                }
                let name = if m.name.is_empty() {
                    crate::tr("Chapter") // as in the export (cl_export::chapter_list)
                } else {
                    m.name.clone()
                };
                let r = ui.add(
                    egui::Label::new(RichText::new(name).size(theme::FS_UI))
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
        if self.settings.export_chapters && self.settings.chapter_hint {
            if let Some(w) = chapter_issue(&ms, Time::ZERO, self.project.duration()) {
                if chapter_warning(ui, &w) {
                    self.settings.chapter_hint = false;
                    self.settings.save();
                }
            }
        }
        if ui
            .add(theme::secondary(format!(
                "{}  Copy chapters",
                icon::COPY
            )))
            .on_hover_text(crate::tr("For the video description"))
            .clicked()
        {
            self.copy_chapters(ui.ctx());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_on_magnet_main_moves_all_tracks_when_linked() {
        assert!(delete_ripples_all(true, true, &[true, true]));
        assert!(!delete_ripples_all(true, false, &[true]));
        assert!(!delete_ripples_all(false, true, &[true]));
        // Overlay in the selection: normal delete
        assert!(!delete_ripples_all(true, true, &[true, false]));
        assert!(!delete_ripples_all(true, true, &[]));
    }

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
            chapters(&[m(75.0, "Boss"), m(3725.0, "End")]),
            "0:00 Intro\n1:15 Boss\n1:02:05 End"
        );
        assert_eq!(chapters(&[m(0.0, "Start")]), "0:00 Start");
        // A marker just after 0:00 (one frame) becomes the first chapter, no extra „Intro“
        assert_eq!(chapters(&[m(0.033, "Start"), m(20.0, "B")]), "0:00 Start\n0:20 B");
    }

    #[test]
    fn youtube_chapter_rules() {
        let m = |s: f64| Marker {
            id: 0,
            at: Time::from_secs_f64(s),
            duration: Time::ZERO,
            name: String::new(),
            note: String::new(),
            color: 0,
        };
        let total = Time::from_secs_f64(60.0);
        let issue = |ms: &[Marker]| chapter_issue(ms, Time::ZERO, total);
        // only report chapters under 10 s, with name and length; the number of chapters does not matter
        assert_eq!(issue(&[m(20.0)]), None);
        assert_eq!(issue(&[m(20.0), m(40.0)]), None);
        assert!(issue(&[m(20.0), m(25.0)]).unwrap().contains("“Chapter” is only 5 s"));
        assert!(issue(&[m(20.0), m(55.0)]).unwrap().contains("only 5 s")); // last one up to the end
        assert!(issue(&[m(4.0), m(30.0)]).unwrap().contains("“Intro” is only 4 s"));
        // in the export range the length counts up to the end of the range
        let s = Time::from_secs_f64;
        assert!(chapter_issue(&[m(20.0)], s(0.0), s(26.0)).unwrap().contains("only 6 s"));
    }
}
