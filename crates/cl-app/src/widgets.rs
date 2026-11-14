//! Eigene, minimalistische Steuerelemente: Regler (2-px-Schiene, kleiner Knopf, Wert
//! rechts direkt editierbar), Schalter, Keyframe-Raute, Segment-Umschalter und Kacheln.

use std::ops::RangeInclusive;

use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText, Sense, Stroke};

use crate::theme::{self, icon};

/// Ergebnis eines Reglers: `changed` = Wert hat sich bewegt (Vorschau),
/// `commit` = Geste abgeschlossen (in den Verlauf schreiben).
#[derive(Default, Clone, Copy)]
pub struct Edit {
    pub changed: bool,
    pub commit: bool,
}

impl std::ops::BitOrAssign for Edit {
    fn bitor_assign(&mut self, o: Self) {
        self.changed |= o.changed;
        self.commit |= o.commit;
    }
}

pub struct Slider<'a> {
    value: &'a mut f32,
    range: RangeInclusive<f32>,
    default: Option<f32>,
    log: bool,
    suffix: &'a str,
    decimals: usize,
    /// Anzeige-Faktor (z. B. 100 für Prozent)
    display: f32,
}

impl<'a> Slider<'a> {
    pub fn new(value: &'a mut f32, range: RangeInclusive<f32>) -> Self {
        Self {
            value,
            range,
            default: None,
            log: false,
            suffix: "",
            decimals: 0,
            display: 1.0,
        }
    }
    pub fn default(mut self, d: f32) -> Self {
        self.default = Some(d);
        self
    }
    pub fn log(mut self) -> Self {
        self.log = true;
        self
    }
    pub fn suffix(mut self, s: &'a str) -> Self {
        self.suffix = s;
        self
    }
    pub fn decimals(mut self, d: usize) -> Self {
        self.decimals = d;
        self
    }
    pub fn percent(mut self) -> Self {
        self.display = 100.0;
        self.suffix = "%";
        self
    }

    fn to_norm(&self, v: f32) -> f32 {
        let (a, b) = (*self.range.start(), *self.range.end());
        if self.log && a > 0.0 {
            ((v.max(a)).ln() - a.ln()) / (b.ln() - a.ln())
        } else {
            (v - a) / (b - a)
        }
        .clamp(0.0, 1.0)
    }
    fn from_norm(&self, n: f32) -> f32 {
        let (a, b) = (*self.range.start(), *self.range.end());
        if self.log && a > 0.0 {
            (a.ln() + n * (b.ln() - a.ln())).exp()
        } else {
            a + n * (b - a)
        }
    }

    pub fn show(self, ui: &mut egui::Ui) -> Edit {
        let mut out = Edit::default();
        // Wertfeld so breit wie der längste Wert („-60.0 dB“); sonst wächst es und schiebt die
        // Keyframe-Raute rechts aus dem Inspektor (sah aus wie ein „<“)
        let font = egui::TextStyle::Button.resolve(ui.style());
        let text_w = |v: f32| {
            let t = format!("{:.*}{}", self.decimals, v * self.display, self.suffix);
            ui.fonts(|f| f.layout_no_wrap(t, font.clone(), Color32::WHITE).size().x)
        };
        let value_w = (text_w(*self.range.start()).max(text_w(*self.range.end())) + 2.0 * ui.spacing().button_padding.x + 2.0).max(46.0);
        let w = (ui.available_width() - value_w - ui.spacing().item_spacing.x - 1.0).max(30.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::click_and_drag());
        let track = Rect::from_center_size(rect.center(), vec2(rect.width() - 10.0, 2.0));
        if let Some(p) = resp.interact_pointer_pos() {
            if resp.dragged() || resp.clicked() || resp.drag_started() {
                let n = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
                let nv = self.from_norm(n);
                if (nv - *self.value).abs() > f32::EPSILON {
                    *self.value = nv;
                    out.changed = true;
                }
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            out.commit = true;
        }
        // Gehaltener Griff: ändert sich beim Ziehen der Aufbau darüber (z. B. erscheint ein
        // Zurücksetzen-Knopf), bekommt der Regler eine neue ID und egui verliert das Ziehen.
        // Deshalb merkt sich der Regler den Druckpunkt und folgt der Maus, bis sie loslässt.
        let grab = egui::Id::new("slider-grab");
        let origin = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started() || resp.is_pointer_button_down_on() {
            if let Some(o) = origin {
                ui.data_mut(|d| d.insert_temp(grab, (o, ui.layer_id())));
            }
        }
        let held = ui.data(|d| d.get_temp::<(egui::Pos2, egui::LayerId)>(grab));
        // nur der Regler unter dem Druckpunkt, in derselben Ebene (kein verdeckter Regler
        // dahinter) und nur für genau diesen Mausdruck
        let down = ui.input(|i| i.pointer.primary_down());
        if held.is_some_and(|(o, l)| rect.contains(o) && l == ui.layer_id() && (!down || origin == Some(o))) {
            if down {
                if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
                    let nv = self.from_norm(((p.x - track.left()) / track.width()).clamp(0.0, 1.0));
                    if !resp.dragged() && (nv - *self.value).abs() > f32::EPSILON {
                        *self.value = nv;
                        out.changed = true;
                    }
                }
            } else {
                ui.data_mut(|d| d.remove::<(egui::Pos2, egui::LayerId)>(grab));
                out.commit = true;
            }
        } else if held.is_some() && !down {
            // losgelassen, ohne dass der Regler noch unter dem Druckpunkt liegt
            ui.data_mut(|d| d.remove::<(egui::Pos2, egui::LayerId)>(grab));
        }
        if resp.double_clicked() {
            if let Some(d) = self.default {
                *self.value = d;
                out.changed = true;
                out.commit = true;
            }
        }
        let p = ui.painter();
        let (a, b) = (*self.range.start(), *self.range.end());
        p.rect_filled(track, 1.0, theme::BG_HOVER());
        // Füllung ab 0 (bipolar) bzw. ab Anfang
        let zero_n = if a < 0.0 && b > 0.0 {
            self.to_norm(0.0)
        } else {
            0.0
        };
        // Werte außerhalb des Bereichs (z. B. alte +20 dB) bleiben stehen, bis man sie ändert
        let n = self.to_norm(*self.value).clamp(0.0, 1.0);
        let (fa, fb) = (zero_n.min(n), zero_n.max(n));
        let fill = Rect::from_min_max(
            pos2(track.left() + fa * track.width(), track.top()),
            pos2(track.left() + fb * track.width(), track.bottom()),
        );
        p.rect_filled(fill, 1.0, theme::ACCENT());
        if zero_n > 0.0 {
            let zx = track.left() + zero_n * track.width();
            p.line_segment(
                [pos2(zx, track.top() - 3.0), pos2(zx, track.bottom() + 3.0)],
                Stroke::new(1.0_f32, theme::TEXT_FAINT()),
            );
        }
        let knob = pos2(track.left() + n * track.width(), track.center().y);
        let hot = resp.hovered() || resp.dragged();
        p.circle_filled(
            knob + vec2(0.0, 1.0),
            if hot { 6.5 } else { 5.5 },
            Color32::from_black_alpha(90),
        );
        p.circle_filled(knob, if hot { 6.0 } else { 5.0 }, theme::ACCENT());
        if hot {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        // Wert: direkt editierbar
        let mut shown = *self.value * self.display;
        let dv = ui.add_sized(
            [value_w, 18.0],
            egui::DragValue::new(&mut shown)
                .range((a * self.display)..=(b * self.display))
                .clamp_existing_to_range(false)
                .fixed_decimals(self.decimals)
                .suffix(self.suffix)
                .speed(((b - a) * self.display / 300.0).max(0.01)),
        );
        if dv.changed() {
            *self.value = shown / self.display;
            out.changed = true;
        }
        if dv.drag_stopped() || dv.lost_focus() {
            out.commit = true;
        }
        out
    }
}

/// Schalter (Pill). Gibt `true` zurück, wenn umgeschaltet wurde.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 16.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let p = ui.painter();
    let bg = if *on { theme::ACCENT() } else { theme::BG_HOVER() };
    // Pille: Radius = halbe Höhe (16 px), bewusst ganz rund
    p.rect_filled(rect, 8.0, bg);
    let x = egui::lerp(rect.left() + 8.0..=rect.right() - 8.0, t);
    p.circle_filled(
        pos2(x, rect.center().y),
        6.0,
        if *on {
            theme::ACCENT_INK()
        } else {
            theme::TEXT_DIM()
        },
    );
    resp.clicked()
}

/// Schalter mit Text rechts daneben, statt Checkbox (deren leeres Kästchen ist auf den
/// Karten kaum zu sehen). Der Text schaltet mit; `changed()` wie bei `ui.checkbox`.
pub fn check(ui: &mut egui::Ui, on: &mut bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.horizontal(|ui| {
        let flipped = toggle(ui, on);
        let mut r = ui.add(egui::Label::new(text).sense(Sense::click()));
        if r.clicked() {
            *on = !*on;
        }
        if flipped || r.clicked() {
            r.mark_changed();
        }
        r
    })
    .inner
}

/// Zeile: Label links, Schalter rechts.
pub fn toggle_row(ui: &mut egui::Ui, label: &str, on: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        ui.label(
            RichText::new(crate::tr(label))
                .size(theme::FS_UI)
                .color(theme::TEXT_DIM()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = toggle(ui, on);
        });
    });
    changed
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kf {
    /// Eigenschaft hat keine Keyframes
    None,
    /// Keyframes vorhanden, aber nicht am Playhead
    Elsewhere,
    /// Keyframe genau am Playhead
    Here,
}

/// Keyframe-Raute; Klick setzt bzw. entfernt den Keyframe am Playhead.
pub fn kf_button(ui: &mut egui::Ui, state: Kf) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
    let c = rect.center();
    let col = if resp.hovered() {
        theme::ACCENT()
    } else {
        theme::TEXT_DIM()
    };
    match state {
        Kf::Here => theme::diamond(
            ui.painter(),
            c,
            5.0,
            Some(theme::ACCENT()),
            Stroke::new(1.0_f32, theme::ACCENT()),
        ),
        Kf::Elsewhere => theme::diamond(
            ui.painter(),
            c,
            5.0,
            None,
            Stroke::new(1.3_f32, theme::ACCENT()),
        ),
        Kf::None => theme::diamond(ui.painter(), c, 4.5, None, Stroke::new(1.0_f32, col)),
    }
    resp.on_hover_text(crate::tr(match state {
        Kf::Here => "Keyframe entfernen",
        _ => "Keyframe am Playhead setzen",
    }))
}

/// Segment-Umschalter (gleich breite Felder). Gibt `true` bei Änderung zurück.
pub fn segmented(ui: &mut egui::Ui, items: &[&str], sel: &mut usize) -> bool {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    ui.painter().rect_filled(rect, theme::R_CARD, theme::BG_INPUT());
    let font = egui::FontId::proportional(theme::FS_UI);
    let texts: Vec<String> = items.iter().map(|it| crate::tr(*it)).collect();
    // Breite je Segment nach Textlänge; Restplatz gleichmäßig verteilt
    let need: Vec<f32> = texts
        .iter()
        .map(|t| ui.fonts(|f| f.layout_no_wrap(t.clone(), font.clone(), Color32::WHITE).size().x) + 14.0)
        .collect();
    let total: f32 = need.iter().sum();
    let extra = (rect.width() - total) / items.len().max(1) as f32;
    let scale = (rect.width() / total).min(1.0);
    let mut x = rect.left();
    let mut changed = false;
    for (i, t) in texts.iter().enumerate() {
        let cw = if extra >= 0.0 { need[i] + extra } else { need[i] * scale };
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(cw, rect.height())).shrink(2.0);
        x += cw;
        let resp = ui.interact(r, ui.id().with(("seg", i, items[i])), Sense::click());
        let active = *sel == i;
        if active {
            ui.painter().rect_filled(r, theme::R_CTRL, theme::BG_HOVER());
        } else if resp.hovered() {
            ui.painter().rect_filled(r, theme::R_CTRL, theme::BG_RAISED());
        }
        ui.painter_at(r).text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            t,
            font.clone(),
            // aktiv: neutrale Fläche + heller Text (kein Akzent-Text)
            if active { theme::TEXT() } else { theme::TEXT_DIM() },
        );
        if extra < 0.0 {
            resp.clone().on_hover_text(t);
        }
        if resp.clicked() && !active {
            *sel = i;
            changed = true;
        }
    }
    changed
}

/// Kachel-Raster: berechnet Spalten und ruft `tile` für jedes Element mit seinem Rechteck.
pub fn grid<T>(
    ui: &mut egui::Ui,
    items: &[T],
    min_w: f32,
    aspect: f32,
    label_h: f32,
    mut tile: impl FnMut(&mut egui::Ui, Rect, &T),
) {
    let gap = 8.0;
    let avail = ui.available_width();
    let cols = ((avail + gap) / (min_w + gap)).floor().max(1.0) as usize;
    let w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).floor();
    let h = (w / aspect).round();
    for chunk in items.chunks(cols) {
        let (row, _) = ui.allocate_exact_size(vec2(avail, h + label_h), Sense::hover());
        for (i, it) in chunk.iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(row.left() + i as f32 * (w + gap), row.top()),
                vec2(w, h + label_h),
            );
            tile(ui, r, it);
        }
        ui.add_space(gap - ui.spacing().item_spacing.y);
    }
}

/// Standard-Kachel: Fläche + Name darunter; zeichnet Hover/Auswahl. `draw` malt den Inhalt.
pub fn tile(
    ui: &mut egui::Ui,
    id: egui::Id,
    r: Rect,
    label: &str,
    selected: bool,
    draw: impl FnOnce(&egui::Painter, Rect),
) -> egui::Response {
    let label_h = if label.is_empty() { 0.0 } else { 16.0 };
    let area = Rect::from_min_max(r.min, pos2(r.right(), r.bottom() - label_h));
    let resp = ui.interact(r, id, Sense::click_and_drag());
    let p = ui.painter_at(r.expand(2.0));
    p.rect_filled(area, theme::R_CARD, theme::BG_RAISED());
    draw(&p.with_clip_rect(area), area);
    if selected {
        p.rect_stroke(
            area,
            theme::R_CARD,
            Stroke::new(1.5_f32, theme::ACCENT()),
            egui::StrokeKind::Inside,
        );
    } else if resp.hovered() {
        p.rect_stroke(
            area,
            theme::R_CARD,
            Stroke::new(1.0_f32, theme::overlay(110)),
            egui::StrokeKind::Inside,
        );
    }
    if resp.hovered() {
        // „+“ zum direkten Hinzufügen
        let plus = Rect::from_min_size(area.right_bottom() - vec2(22.0, 22.0), vec2(18.0, 18.0));
        p.circle_filled(plus.center(), 9.0, theme::ACCENT());
        p.text(
            plus.center(),
            egui::Align2::CENTER_CENTER,
            icon::PLUS,
            // Icon-Glyphe im 18-px-Kreis
            egui::FontId::proportional(11.0),
            theme::ACCENT_INK(),
        );
    }
    if !label.is_empty() {
        let g = p.layout(
            crate::tr(label),
            egui::FontId::proportional(theme::FS_HINT),
            if selected {
                theme::TEXT()
            } else {
                theme::TEXT_DIM()
            },
            r.width(),
        );
        let pos = pos2(
            r.center().x - g.size().x.min(r.width()) / 2.0,
            area.bottom() + 3.0,
        );
        p.with_clip_rect(Rect::from_min_max(pos2(r.left(), area.bottom()), r.max))
            .galley(pos, g, theme::TEXT_DIM());
    }
    resp
}

/// Kachel-Klick traf das „+“ unten rechts?
pub fn hit_plus(resp: &egui::Response, r: Rect, label_h: f32) -> bool {
    let area_br = pos2(r.right(), r.bottom() - label_h);
    resp.interact_pointer_pos()
        .is_some_and(|p| p.x > area_br.x - 26.0 && p.y > area_br.y - 26.0 && p.y <= area_br.y)
}

/// Farbfeld mit Popup-Wähler.
pub fn color_button(ui: &mut egui::Ui, rgba: &mut [u8; 4]) -> bool {
    let mut c = Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);
    let r = egui::widgets::color_picker::color_edit_button_srgba(
        ui,
        &mut c,
        egui::widgets::color_picker::Alpha::OnlyBlend,
    );
    if r.changed() {
        *rgba = c.to_srgba_unmultiplied();
        return true;
    }
    false
}
