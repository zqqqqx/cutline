//! Bewegung zwischen zwei Keyframes formen: fertige Kurven als Kacheln (ein Klick) und
//! darunter „Eigene Kurve“ mit zwei ziehbaren Griffen für alle, die es genau wollen.

use cl_core::{Clip, Ease, Keyframe, Time};
use eframe::egui::{self, pos2, vec2, Pos2, Rect, RichText, Sense, Stroke};

use crate::app::App;
use crate::theme;
use crate::widgets::Edit;

/// Fertige Kurven: (Name, Hinweis, Kurve)
const PRESETS: [(&str, &str, Ease); 8] = [
    ("Linear", "Gleiches Tempo von Anfang bis Ende", Ease::Linear),
    ("Sanft", "Langsam anfahren und abbremsen", Ease::EaseInOut),
    ("Anfahren", "Startet langsam, wird schneller", Ease::EaseIn),
    ("Abbremsen", "Startet schnell, kommt sanft an", Ease::EaseOut),
    ("Schnell", "Kurzer, kräftiger Ruck in der Mitte", Ease::Bezier(0.85, 0.0, 0.15, 1.0)),
    ("Schwung", "Schießt etwas übers Ziel und federt zurück", Ease::Bezier(0.34, 1.56, 0.64, 1.0)),
    ("Ausholen", "Holt kurz in die Gegenrichtung aus", Ease::Bezier(0.36, -0.5, 0.66, 1.0)),
    ("Halten", "Bleibt stehen und springt am nächsten Keyframe", Ease::Hold),
];

/// Index des Keyframes, dessen Abschnitt `t` enthält (braucht einen Keyframe danach)
fn segment(list: &[Keyframe], t: Time) -> Option<usize> {
    if list.len() < 2 {
        return None;
    }
    Some(list[..list.len() - 1].iter().rposition(|k| k.t <= t).unwrap_or(0))
}

/// Kurve in ein Rechteck zeichnen (Zeit nach rechts, Wert nach oben)
fn draw_curve(p: &egui::Painter, r: Rect, e: Ease, stroke: Stroke) {
    let pts: Vec<Pos2> = (0..=24)
        .map(|i| {
            let x = i as f32 / 24.0;
            // „Halten“ springt erst am Ende
            let y = if e == Ease::Hold && i == 24 { 1.0 } else { e.apply(x) };
            pos2(r.left() + x * r.width(), r.bottom() - y * r.height())
        })
        .collect();
    p.add(egui::Shape::line(pts, stroke));
}

impl App {
    /// Kurven-Auswahl für den Abschnitt am Playhead; gilt für alle animierten Eigenschaften
    /// des Clips in diesem Abschnitt.
    pub fn curve_ui(&self, ui: &mut egui::Ui, c: &mut Clip) -> Edit {
        let mut out = Edit::default();
        if c.keyframes.is_empty() {
            return out;
        }
        let t = self.rel_time(c);
        theme::section(ui, "Bewegung zwischen Keyframes");
        let cur = c.keyframes.0.values().find_map(|l| segment(l, t).map(|i| (l[i], l[i + 1])));
        let Some((a, b)) = cur else {
            ui.label(
                RichText::new(crate::tr("Setze einen zweiten Keyframe an einer anderen Stelle – dann kannst du hier wählen, wie die Bewegung dazwischen abläuft."))
                    .small()
                    .color(theme::TEXT_DIM()),
            );
            return out;
        };
        let fps = self.project.settings.fps;
        ui.label(
            RichText::new(format!(
                "{} {} – {}",
                crate::tr("Abschnitt"),
                (c.start + a.t).timecode(fps),
                (c.start + b.t).timecode(fps)
            ))
            .small()
            .color(theme::TEXT_FAINT()),
        );
        let mut pick: Option<Ease> = None;
        // Einsteiger: Kacheln
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
            for (name, tip, e) in PRESETS {
                let (r, resp) = ui.allocate_exact_size(vec2(58.0, 50.0), Sense::click());
                let on = a.ease == e;
                let p = ui.painter();
                p.rect_filled(r, theme::R_CARD, if on { theme::BG_HOVER() } else { theme::BG_RAISED() });
                if on {
                    p.rect_stroke(r, theme::R_CARD, Stroke::new(1.5_f32, theme::ACCENT()), egui::StrokeKind::Inside);
                }
                theme::hover_glow(ui, &resp, theme::R_CARD);
                let plot = Rect::from_min_max(r.min + vec2(12.0, 9.0), pos2(r.right() - 12.0, r.bottom() - 19.0));
                draw_curve(p, plot, e, Stroke::new(1.5_f32, if on { theme::ACCENT() } else { theme::TEXT_DIM() }));
                p.text(
                    pos2(r.center().x, r.bottom() - 9.0),
                    egui::Align2::CENTER_CENTER,
                    crate::tr(name),
                    egui::FontId::proportional(theme::FS_HINT),
                    if on { theme::TEXT() } else { theme::TEXT_DIM() },
                );
                if resp.on_hover_text(crate::tr(tip)).clicked() {
                    pick = Some(e);
                    out.commit = true;
                }
            }
        });
        // Fortgeschrittene: Griffe ziehen
        egui::CollapsingHeader::new(RichText::new(crate::tr("Eigene Kurve")).size(theme::FS_UI).color(theme::TEXT_DIM()))
            .id_salt("curve-adv")
            .show(ui, |ui| {
                let Some((mut x1, mut y1, mut x2, mut y2)) = a.ease.bezier() else {
                    ui.label(RichText::new(crate::tr("„Halten“ hat keine Kurve. Wähle oben eine andere Form.")).small().color(theme::TEXT_DIM()));
                    return;
                };
                let w = ui.available_width().min(230.0);
                let (r, _) = ui.allocate_exact_size(vec2(w, 150.0), Sense::hover());
                // Wertebereich −0,5 … 1,5, damit Überschwingen Platz hat
                let plot = Rect::from_min_max(r.min + vec2(8.0, 8.0), r.max - vec2(8.0, 8.0));
                let to = |x: f32, y: f32| pos2(plot.left() + x * plot.width(), plot.bottom() - (y + 0.5) / 2.0 * plot.height());
                let p = ui.painter().with_clip_rect(r);
                p.rect_filled(r, theme::R_CARD, theme::BG_APP());
                let grid = Stroke::new(1.0_f32, theme::BORDER());
                p.line_segment([to(0.0, 0.0), to(1.0, 0.0)], grid);
                p.line_segment([to(0.0, 1.0), to(1.0, 1.0)], grid);
                let mut dragged = false;
                for (i, (x, y, anchor)) in [(&mut x1, &mut y1, to(0.0, 0.0)), (&mut x2, &mut y2, to(1.0, 1.0))].into_iter().enumerate() {
                    let h = to(*x, *y);
                    let resp = ui.interact(Rect::from_center_size(h, vec2(18.0, 18.0)), ui.id().with(("curve-h", i)), Sense::drag());
                    if let Some(m) = resp.interact_pointer_pos().filter(|_| resp.dragged()) {
                        *x = ((m.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
                        *y = (((plot.bottom() - m.y) / plot.height()) * 2.0 - 0.5).clamp(-0.5, 1.5);
                        out.changed = true;
                        dragged = true;
                    }
                    out.commit |= resp.drag_stopped();
                    let h = to(*x, *y);
                    p.line_segment([anchor, h], Stroke::new(1.0_f32, theme::TEXT_FAINT()));
                    p.circle_filled(h, if resp.hovered() || resp.dragged() { 6.0 } else { 5.0 }, theme::ACCENT());
                    resp.on_hover_cursor(egui::CursorIcon::Grab);
                }
                let e = Ease::Bezier(x1, y1, x2, y2);
                let curve = Rect::from_min_max(to(0.0, 1.0), to(1.0, 0.0));
                draw_curve(&p, curve, e, Stroke::new(2.0_f32, theme::TEXT()));
                if dragged {
                    pick = Some(e);
                }
                ui.label(
                    RichText::new(format!("{x1:.2} · {y1:.2} · {x2:.2} · {y2:.2}"))
                        .small()
                        .monospace()
                        .color(theme::TEXT_FAINT()),
                )
                .on_hover_text(crate::tr("Griffe ziehen: links der Start, rechts die Ankunft. Über die Linien hinaus = Ausholen bzw. Überschwingen."));
            });
        if let Some(e) = pick {
            for l in c.keyframes.0.values_mut() {
                if let Some(i) = segment(l, t) {
                    l[i].ease = e;
                }
            }
            out.changed = true;
        }
        out
    }
}

#[test]
fn segment_picks_the_keyframe_before_the_playhead() {
    let k = |s: f64| Keyframe { t: Time::from_secs_f64(s), v: 0.0, ease: Ease::Linear };
    let l = [k(1.0), k(2.0), k(4.0)];
    assert_eq!(segment(&l, Time::from_secs_f64(0.0)), Some(0));
    assert_eq!(segment(&l, Time::from_secs_f64(2.5)), Some(1));
    assert_eq!(segment(&l, Time::from_secs_f64(9.0)), Some(1), "hinter dem letzten: letzter Abschnitt");
    assert_eq!(segment(&l[..1], Time::ZERO), None);
}
