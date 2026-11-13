//! Feinschnitt: Zeit per Zahleneingabe (Start, Dauer, Quellbeginn), Verschieben um Bilder
//! (Alt+←/→), Rollen und Slippen per Alt-Ziehen, Attribute einfügen (Strg+Alt+V) und Gruppen.

use cl_core::{Clip, ClipContent, ClipId, EditOp, MediaKind, Time};
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::theme;

/// Was „Attribute einfügen“ übernimmt
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
            let keep = (
                dst.fx.cutout.clone(),
                dst.fx.person.clone(),
                dst.fx.speed_curve.clone(),
                dst.fx.reverse,
            );
            dst.fx = src.fx.clone();
            (
                dst.fx.cutout,
                dst.fx.person,
                dst.fx.speed_curve,
                dst.fx.reverse,
            ) = keep;
        }
        if self.transform {
            dst.transform = src.transform;
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

/// Grenzen für den Quellbeginn eines Clips (Video/Audio: Länge der Quelle)
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

impl App {
    /// Abschnitt „Zeit“ im Inspektor: Start, Dauer und (bei Medien) Quellbeginn als Zahlen.
    pub fn timing_section(&mut self, ui: &mut egui::Ui, c: &Clip) {
        theme::section(ui, "Zeit");
        let fps = self.project.settings.fps.as_f64();
        let mut start = c.start.as_secs_f64();
        let mut dur = c.duration.as_secs_f64();
        let mut src = c.source_in.as_secs_f64();
        let limit = source_limit(self, c);
        let fmt = move |v: f64, _: std::ops::RangeInclusive<usize>| {
            let f = (v * fps).round() as i64;
            format!(
                "{}:{:02}.{:02}",
                f / (fps.round() as i64 * 60),
                f / fps.round() as i64 % 60,
                f % fps.round() as i64
            )
        };
        let step = 1.0 / fps;
        let mut changed = false;
        theme::row(ui, "Start", |ui| {
            changed |= ui
                .add(
                    egui::DragValue::new(&mut start)
                        .range(0.0..=36000.0)
                        .speed(step)
                        .custom_formatter(fmt),
                )
                .on_hover_text(crate::tr("Ziehen oder tippen; Bilder als m:ss.ff"))
                .changed();
        });
        theme::row(ui, "Dauer", |ui| {
            changed |= ui
                .add(
                    egui::DragValue::new(&mut dur)
                        .range(step..=36000.0)
                        .speed(step)
                        .custom_formatter(fmt),
                )
                .changed();
        });
        if let Some(max) = limit {
            let max = max + c.source_in.as_secs_f64() - c.source_in.as_secs_f64();
            theme::row(ui, "Quelle ab", |ui| {
                changed |= ui.add(egui::DragValue::new(&mut src).range(0.0..=max.max(0.0)).speed(step).custom_formatter(fmt)).on_hover_text(crate::tr("Slippen: anderer Ausschnitt der Quelle, Lage und Länge bleiben (Alt+Ziehen im Clip)")).changed();
            });
        }
        if changed {
            let snap = |v: f64| Time::from_secs_f64((v * fps).round() / fps);
            self.apply(EditOp::SetClipTiming {
                clip: c.id,
                start: snap(start),
                duration: snap(dur.max(step)),
                source_in: snap(src.max(0.0)),
            });
        }
    }

    /// Alt+←/→: ausgewählte Clips um Bilder verschieben.
    pub fn nudge(&mut self, frames: i64) {
        self.commit_live();
        let fps = self.project.settings.fps.as_f64();
        let d = Time::from_secs_f64(frames as f64 / fps);
        let ops: Vec<EditOp> = self
            .selection()
            .into_iter()
            .filter_map(|id| {
                self.project.find_clip(id).map(|(t, c)| EditOp::MoveClip {
                    clip: id,
                    track: Some(t.id),
                    start: Time((c.start.0 + d.0).max(0)),
                })
            })
            .collect();
        if !ops.is_empty() {
            self.apply_batch(ops);
        }
    }

    /// Rollen: Schnitt zwischen zwei angrenzenden Clips verschieben (beide passen sich an).
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
        // Quellgrenzen beachten: rechts braucht Material davor, links danach
        let r_src = r.source_in.as_secs_f64() + delta * r.speed as f64;
        if r_src < 0.0 {
            self.toast_err("Am Anfang des rechten Clips ist kein weiteres Material");
            return;
        }
        if let Some(lim) = source_limit(self, &l) {
            let need = l.source_in.as_secs_f64() + (to - l.start.as_secs_f64()) * l.speed as f64;
            let len = lim + l.duration.as_secs_f64() * l.speed as f64;
            if need > len + 1e-3 {
                self.toast_err("Am Ende des linken Clips ist kein weiteres Material");
                return;
            }
        }
        let t = |v: f64| Time::from_secs_f64(v);
        self.apply_batch(vec![
            EditOp::SetClipTiming {
                clip: l.id,
                start: l.start,
                duration: t(to - l.start.as_secs_f64()),
                source_in: l.source_in,
            },
            EditOp::SetClipTiming {
                clip: r.id,
                start: t(to),
                duration: t(r.end().as_secs_f64() - to),
                source_in: t(r_src),
            },
        ]);
    }

    /// Slippen: Quellbeginn verschieben, Lage und Länge bleiben.
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

    /// Dialog „Attribute einfügen“ (Strg+Alt+V).
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
        let (mut ok, mut close) = (false, false);
        egui::Window::new(crate::tr("Attribute einfügen"))
            .id(egui::Id::new("paste-attrs"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -60.0))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(crate::tr(format!(
                        "Von „{}“ auf die Auswahl übertragen:",
                        src.label(&self.project)
                    )))
                    .small()
                    .color(theme::TEXT_DIM),
                );
                ui.add_space(4.0);
                ui.checkbox(
                    &mut a.look,
                    crate::tr("Look (Anpassen, Filter, Effekte, LUT, Maske, Freistellen-Farbe)"),
                );
                ui.checkbox(
                    &mut a.transform,
                    crate::tr("Position, Größe, Drehung und Keyframes"),
                );
                ui.checkbox(&mut a.opacity, crate::tr("Deckkraft"));
                ui.checkbox(&mut a.anim, crate::tr("Animationen"));
                ui.checkbox(
                    &mut a.audio,
                    crate::tr("Ton (Lautstärke, Bearbeitung, Ein-/Ausblenden)"),
                );
                ui.checkbox(&mut a.speed, crate::tr("Tempokurve und Richtung"));
                ui.checkbox(&mut a.text, crate::tr("Textstil (bei Texten)"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(theme::primary("Einfügen")).clicked() {
                        ok = true;
                    }
                    if ui.add(theme::secondary("Abbrechen")).clicked() {
                        close = true;
                    }
                });
            });
        self.paste_attrs = a;
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
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

    /// Strg+G: Auswahl gruppieren (bewegen und auswählen sich gemeinsam), Strg+Umschalt+G löst.
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
        // Bei „lösen“ auch die übrigen Mitglieder der Gruppe
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

    /// Alle Clips derselben Gruppe(n) zur Auswahl hinzufügen.
    pub fn expand_groups(&mut self) {
        let groups: Vec<u64> = self
            .selection()
            .iter()
            .filter_map(|id| self.project.find_clip(*id).and_then(|(_, c)| c.group))
            .collect();
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
