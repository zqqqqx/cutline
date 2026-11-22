//! Fine cutting: time by number input (start, duration, source start), nudging by frames
//! (Alt+←/→), roll and slip by Alt-dragging, paste attributes (Ctrl+Alt+V) and groups.

use cl_core::{Clip, ClipContent, ClipId, EditOp, MediaKind, Time};
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::theme;

/// What „Attribute einfügen“ takes over
#[derive(Debug, Clone, Copy)]
pub struct PasteAttrs {
    pub look: bool,
    pub transform: bool,
    pub opacity: bool,
    pub anim: bool,
    pub audio: bool,
    pub speed: bool,
    pub text: bool,
}

impl Default for PasteAttrs {
    fn default() -> Self {
        Self {
            look: true,
            transform: false,
            opacity: true,
            anim: true,
            audio: false,
            speed: false,
            text: true,
        }
    }
}

impl PasteAttrs {
    pub fn apply(&self, src: &Clip, dst: &mut Clip) {
        if self.look {
            let mut fx = src.fx.clone();
            fx.keep_bound(&dst.fx);
            dst.fx = fx;
        }
        if self.transform {
            dst.transform = src.transform;
            dst.crop = src.crop;
            dst.keyframes = src.keyframes.clone();
        }
        if self.opacity {
            dst.opacity = src.opacity;
        }
        if self.anim {
            dst.anim = src.anim.clone();
        }
        if self.audio {
            dst.volume_db = src.volume_db;
            dst.audio = src.audio.clone();
            dst.fade_in = src.fade_in;
            dst.fade_out = src.fade_out;
        }
        if self.speed {
            dst.fx.speed_curve = src.fx.speed_curve.clone();
            dst.fx.reverse = src.fx.reverse;
        }
        if self.text {
            if let (ClipContent::Text(s), ClipContent::Text(d)) = (&src.content, &mut dst.content) {
                let text = std::mem::take(&mut d.text);
                *d = cl_core::TextStyle { text, ..s.clone() };
            }
        }
    }
}

/// Limits for a clip's source start (video/audio: length of the source)
pub fn source_limit(app: &App, c: &Clip) -> Option<f64> {
    match &c.content {
        ClipContent::Media {
            media,
            kind: MediaKind::Video | MediaKind::Audio,
        } => {
            let len = app.project.media(*media)?.duration.as_secs_f64();
            Some((len - c.duration.as_secs_f64() * c.speed.max(0.01) as f64).max(0.0))
        }
        _ => None,
    }
}

/// Input in the „Zeit“ section → new values, rounded to frames. `None` if nothing
/// changes or the duration drops below one frame (0, negative: input is discarded).
fn timing_edit(c: &Clip, start: f64, dur: f64, src: f64, fps: f64) -> Option<(Time, Time, Time)> {
    let snap = |v: f64| Time::from_secs_f64((v.max(0.0) * fps).round() / fps);
    let new = (snap(start), snap(dur), snap(src));
    (new.1 > Time::ZERO && new != (c.start, c.duration, c.source_in)).then_some(new)
}

impl App {
    /// „Zeit“ section in the inspector: start, duration and (for media) source start as numbers.
    pub fn timing_section(&mut self, ui: &mut egui::Ui, c: &Clip) {
        theme::section(ui, "Zeit");
        let fps = self.project.settings.fps.as_f64();
        let mut start = c.start.as_secs_f64();
        let mut dur = c.duration.as_secs_f64();
        let mut src = c.source_in.as_secs_f64();
        let limit = source_limit(self, c);
        let fps_r = self.project.settings.fps;
        // the same rounding as the timecode („ab 00:00:03:19“)
        let fmt = move |v: f64, _: std::ops::RangeInclusive<usize>| {
            let (s, f) = Time::from_secs_f64(v).secs_frames(fps_r);
            format!("{}:{:02}.{:02}", s / 60, s % 60, f)
        };
        let step = 1.0 / fps;
        let mut changed = false;
        theme::row(ui, "Start", |ui| {
            changed |= ui
                .add(
                    egui::DragValue::new(&mut start)
                        .range(0.0..=36000.0)
                        .speed(step)
                        .update_while_editing(false)
                        .clamp_existing_to_range(false)
                        .custom_formatter(fmt),
                )
                .on_hover_text(crate::tr("Ziehen oder tippen; Bilder als m:ss.ff"))
                .changed();
        });
        theme::row(ui, "Dauer", |ui| {
            changed |= ui
                .add(
                    egui::DragValue::new(&mut dur)
                        .range(0.0..=36000.0)
                        .speed(step)
                        .update_while_editing(false)
                        .clamp_existing_to_range(false)
                        .custom_formatter(fmt),
                )
                .changed();
        });
        if let Some(max) = limit {
            let max = max + c.source_in.as_secs_f64() - c.source_in.as_secs_f64();
            theme::row(ui, "Quelle ab", |ui| {
                changed |= ui.add(egui::DragValue::new(&mut src).range(0.0..=max.max(0.0)).speed(step).update_while_editing(false).clamp_existing_to_range(false).custom_formatter(fmt)).on_hover_text(crate::tr("Slippen: anderer Ausschnitt der Quelle, Lage und Länge bleiben (Alt+Ziehen im Clip)")).changed();
            });
        }
        if changed {
            if let Some((start, duration, source_in)) = timing_edit(c, start, dur, src, fps) {
                self.apply(EditOp::SetClipTiming { clip: c.id, start, duration, source_in });
            }
        }
    }

    /// Alt+←/→: nudge selected clips by frames.
    pub fn nudge(&mut self, frames: i64) {
        self.commit_live();
        let fps = self.project.settings.fps.as_f64();
        let d = Time::from_secs_f64(frames as f64 / fps);
        let ids: Vec<(ClipId, f64)> =
            self.selection().into_iter().filter_map(|id| self.project.find_clip(id).map(|(_, c)| (id, c.start.as_secs_f64()))).collect();
        let Some(&(first, st)) = ids.first() else {
            return;
        };
        let moves = crate::timeline::group_moves(&ids, first, Time(Time::from_secs_f64(st).0 + d.0));
        match crate::timeline::group_in_time(&self.project, &moves) {
            Ok(op) => {
                self.apply(op);
            }
            Err((row, upper)) => {
                self.clips_to_new_tracks(row, upper, &moves);
            }
        }
    }

    /// Clips (id, new start) onto new tracks next to display row `row` – one history step; false
    /// (with a message) if that is not possible
    pub fn clips_to_new_tracks(&mut self, row: usize, upper: bool, moves: &[(ClipId, Time)]) -> bool {
        let r = cl_core::move_to_new_tracks(&mut self.doc.lock().unwrap_or_else(|e| e.into_inner()), row, upper, moves);
        match r {
            Ok(()) => {
                self.sync_project();
                true
            }
            Err(e) => {
                self.toast_err(e.to_string());
                false
            }
        }
    }

    /// Roll: move the cut between two adjacent clips (both adapt).
    pub fn roll_edit(&mut self, left: ClipId, right: ClipId, to: f64) {
        let (Some((_, l)), Some((_, r))) =
            (self.project.find_clip(left), self.project.find_clip(right))
        else {
            return;
        };
        let (l, r) = (l.clone(), r.clone());
        let fps = self.project.settings.fps.as_f64();
        let to = (to * fps).round() / fps;
        let min = l.start.as_secs_f64() + 1.0 / fps;
        let max = r.end().as_secs_f64() - 1.0 / fps;
        let to = to.clamp(min, max);
        let delta = to - r.start.as_secs_f64();
        // Respect source limits: the right one needs material before, the left one after. Only video/audio
        // have a time-bound source (like TrimClip in the core); text, image, effect do not
        let timed = matches!(r.content, ClipContent::Media { kind: MediaKind::Video | MediaKind::Audio, .. });
        let r_src = r.source_in.as_secs_f64() + if timed { delta * r.speed as f64 } else { 0.0 };
        if r_src < 0.0 {
            self.toast_err("Am Anfang des rechten Clips gibt es keine weiteren Medien");
            return;
        }
        if let Some(lim) = source_limit(self, &l) {
            let need = l.source_in.as_secs_f64() + (to - l.start.as_secs_f64()) * l.speed as f64;
            let len = lim + l.duration.as_secs_f64() * l.speed as f64;
            if need > len + 1e-3 {
                self.toast_err("Am Ende des linken Clips gibt es keine weiteren Medien");
                return;
            }
        }
        let t = |v: f64| Time::from_secs_f64(v);
        let cut = t(to);
        let mut ops = vec![
            EditOp::SetClipTiming {
                clip: l.id,
                start: l.start,
                duration: cut - l.start,
                source_in: l.source_in,
            },
            EditOp::SetClipTiming {
                clip: r.id,
                start: cut,
                duration: r.end() - cut,
                source_in: t(r_src),
            },
        ];
        // The clip that gets shorter first, otherwise both overlap briefly (rejected)
        if cut > r.start {
            ops.reverse();
        }
        self.apply_batch(ops);
    }

    /// Slip: move the source start, position and length stay.
    pub fn slip_edit(&mut self, clip: ClipId, src_secs: f64) {
        let Some((_, c)) = self.project.find_clip(clip).map(|(t, c)| (t.id, c.clone())) else {
            return;
        };
        let Some(lim) = source_limit(self, &c) else {
            return;
        };
        let fps = self.project.settings.fps.as_f64();
        let v = ((src_secs.clamp(0.0, lim)) * fps).round() / fps;
        self.apply(EditOp::SetClipTiming {
            clip,
            start: c.start,
            duration: c.duration,
            source_in: Time::from_secs_f64(v),
        });
    }

    /// „Attribute einfügen“ dialog (Ctrl+Alt+V).
    pub fn paste_attrs_dialog(&mut self, ctx: &egui::Context) {
        if !self.paste_attrs_open {
            return;
        }
        let Some(src) = self.clipboard.first().cloned() else {
            self.paste_attrs_open = false;
            self.toast_err("Erst einen Clip kopieren (Strg+C)");
            return;
        };
        let mut a = self.paste_attrs;
        // Esc closes, Enter confirms like „Einfügen“
        let (mut close, mut ok) = crate::dialog::keys(ctx);
        crate::dialog::window(ctx, crate::tr("Attribute einfügen"), "paste-attrs")
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -60.0))
            .show(ctx, |ui| {
                close |= crate::dialog::title(ui, "Attribute einfügen");
                ui.label(
                    RichText::new(crate::tr(format!(
                        "Von „{}“ auf die Auswahl übertragen:",
                        src.label(&self.project)
                    )))
                    .small()
                    .color(theme::TEXT_DIM()),
                );
                ui.add_space(4.0);
                crate::widgets::check(ui, 
                    &mut a.look,
                    crate::tr("Look (Anpassen, Filter, Effekte, LUT, Maske, Freistellen-Farbe)"),
                );
                crate::widgets::check(ui, 
                    &mut a.transform,
                    crate::tr("Position, Größe, Drehung und Keyframes"),
                );
                crate::widgets::check(ui, &mut a.opacity, crate::tr("Deckkraft"));
                crate::widgets::check(ui, &mut a.anim, crate::tr("Animationen"));
                crate::widgets::check(ui, 
                    &mut a.audio,
                    crate::tr("Ton (Lautstärke, Bearbeitung, Ein-/Ausblenden)"),
                );
                crate::widgets::check(ui, &mut a.speed, crate::tr("Tempokurve und Richtung"));
                crate::widgets::check(ui, &mut a.text, crate::tr("Textstil (bei Texten)"));
                let (m, c) = crate::dialog::buttons(ui, theme::primary("Einfügen"), true);
                ok |= m;
                close |= c;
            });
        self.paste_attrs = a;
        if ok {
            let src2 = src.clone();
            self.modify_selection(|c| a.apply(&src2, c));
            self.toast("Attribute eingefügt");
            close = true;
        }
        if close {
            self.paste_attrs_open = false;
        }
    }

    /// Ctrl+G: group the selection (move and select together), Ctrl+Shift+G ungroups.
    pub fn group_selection(&mut self, on: bool) {
        let sel = self.selection();
        if on && sel.len() < 2 {
            self.toast_err("Für eine Gruppe mindestens zwei Clips auswählen");
            return;
        }
        let gid = if on {
            Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(1),
            )
        } else {
            None
        };
        // When ungrouping, also the other members of the group
        let groups: Vec<u64> = sel
            .iter()
            .filter_map(|id| self.project.find_clip(*id).and_then(|(_, c)| c.group))
            .collect();
        let ids: Vec<ClipId> = if on {
            sel
        } else {
            self.project
                .timeline
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter())
                .filter(|c| c.group.is_some_and(|g| groups.contains(&g)))
                .map(|c| c.id)
                .collect()
        };
        let ops: Vec<EditOp> = ids
            .iter()
            .filter_map(|id| self.project.find_clip(*id).map(|(_, c)| c.clone()))
            .map(|mut c| {
                c.group = gid;
                EditOp::ReplaceClip { clip: c }
            })
            .collect();
        let n = ops.len();
        self.apply_batch(ops);
        self.toast(if on {
            format!("{n} Clips gruppiert")
        } else {
            "Gruppe aufgelöst".to_string()
        });
    }

    /// Add all clips of the same group(s) to the selection.
    pub fn expand_groups(&mut self) {
        // one pass over the project with hash sets: a selection of thousands of clips stays cheap
        let sel: std::collections::HashSet<ClipId> = self.selection().into_iter().collect();
        let groups: std::collections::HashSet<u64> =
            self.project.timeline.tracks.iter().flat_map(|t| &t.clips).filter(|c| sel.contains(&c.id)).filter_map(|c| c.group).collect();
        if groups.is_empty() {
            return;
        }
        let members: Vec<ClipId> = self
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.group.is_some_and(|g| groups.contains(&g)))
            .map(|c| c.id)
            .collect();
        for m in members {
            if Some(m) != self.selected {
                self.sel_extra.insert(m);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The crop goes with the transform attributes, not with the look.
    #[test]
    fn paste_attrs_carries_crop_with_transform() {
        let m = ClipContent::Media { media: cl_core::MediaId::new(), kind: MediaKind::Video };
        let mut src = Clip::new(m, Time::ZERO, Time::from_secs_f64(1.0));
        src.crop.left = 0.25;
        let mut dst = src.clone();
        dst.crop = Default::default();
        PasteAttrs::default().apply(&src, &mut dst);
        assert!(dst.crop.is_none());
        PasteAttrs { transform: true, ..Default::default() }.apply(&src, &mut dst);
        assert_eq!(dst.crop.left, 0.25);
    }

    /// The look carries chroma and filters; mask, person effect and edge stay with the target clip.
    #[test]
    fn paste_look_keeps_the_clip_bound_cutout() {
        let m = ClipContent::Media { media: cl_core::MediaId::new(), kind: MediaKind::Video };
        let mut src = Clip::new(m, Time::ZERO, Time::from_secs_f64(1.0));
        src.fx.chroma = Some(Default::default());
        src.fx.cutout = Some("src.mkv".into());
        src.fx.edge.feather = 0.5;
        let mut dst = src.clone();
        dst.fx = Default::default();
        dst.fx.cutout = Some("dst.mkv".into());
        dst.fx.edge.invert = true;
        PasteAttrs::default().apply(&src, &mut dst);
        assert_eq!(dst.fx.chroma, src.fx.chroma);
        assert_eq!(dst.fx.cutout.as_deref(), Some("dst.mkv"));
        assert_eq!(dst.fx.edge, cl_core::MatteEdge { invert: true, ..Default::default() });
    }

    #[test]
    fn timing_edit_writes_only_real_changes() {
        let m = ClipContent::Media { media: cl_core::MediaId::new(), kind: MediaKind::Video };
        let mut c = Clip::new(m, Time::ZERO, Time::from_secs_f64(1.0 / 30.0));
        c.source_in = Time::from_secs_f64(2.0);
        let (s, d, src) = (0.0, c.duration.as_secs_f64(), 2.0);
        // the same value (also after rounding to frames) writes nothing
        assert_eq!(timing_edit(&c, s, d, src, 30.0), None);
        assert_eq!(timing_edit(&c, s, d - 1e-9, src, 30.0), None);
        // 0 and negative: discarded
        assert_eq!(timing_edit(&c, s, 0.0, src, 30.0), None);
        assert_eq!(timing_edit(&c, s, -3.0, src, 30.0), None);
        // real change: one value
        let t = Time::from_secs_f64;
        assert_eq!(timing_edit(&c, s, 2.0, src, 30.0), Some((Time::ZERO, t(2.0), t(2.0))));
    }

    /// Roll in both directions: the cut moves, nothing overlaps (the core rejects
    /// overlap, so the clip that gets shorter must be set first). Text has
    /// no source: rolling left works even without material before the right clip.
    #[test]
    fn roll_edit_moves_the_cut_both_ways() {
        let mut app = crate::saving::tests::test_app();
        let t = Time::from_secs_f64;
        let mut p = cl_core::Project::new("R");
        let mut tr = cl_core::Track::new(cl_core::TrackKind::Text);
        let text = || ClipContent::Text(cl_core::TextStyle::default());
        let (a, b) = (Clip::new(text(), t(0.0), t(2.0)), Clip::new(text(), t(2.0), t(2.0)));
        let (ia, ib) = (a.id, b.id);
        tr.clips = vec![a, b];
        p.timeline.tracks.push(tr);
        app.set_doc(p);
        for to in [3.0, 1.0] {
            app.roll_edit(ia, ib, to);
            app.sync_project();
            let (l, r) = (app.project.find_clip(ia).unwrap().1, app.project.find_clip(ib).unwrap().1);
            assert_eq!((l.end(), r.start, r.end()), (t(to), t(to), t(4.0)), "cut to {to}");
        }
    }
}
