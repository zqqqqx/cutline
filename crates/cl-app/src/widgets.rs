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
    let rects: Vec<Rect> = need
        .iter()
        .map(|n| {
            let cw = if extra >= 0.0 { n + extra } else { n * scale };
            x += cw;
            Rect::from_min_size(pos2(x - cw, rect.top()), vec2(cw, rect.height())).shrink(2.0)
        })
        .collect();
    // Markierung gleitet zum gewählten Feld (Stand als Feldnummer: Größenänderungen gleiten nicht)
    let at = theme::glide(ui.ctx(), ui.id().with(("seg-mark", items.len())), *sel as f32, theme::MOTION);
    if let (Some(a), Some(b)) = (rects.get(at.floor() as usize), rects.get((at.ceil() as usize).min(rects.len().saturating_sub(1)))) {
        let f = at.fract();
        let mark = Rect::from_min_max(a.min.lerp(b.min, f), a.max.lerp(b.max, f));
        ui.painter().rect_filled(mark, theme::R_CTRL, theme::BG_HOVER());
    }
    let mut changed = false;
    for (i, t) in texts.iter().enumerate() {
        let r = rects[i];
        let resp = ui.interact(r, ui.id().with(("seg", i, items[i])), Sense::click());
        let active = *sel == i;
        if !active && resp.hovered() {
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

/// Spalten und Kachelbreite eines Rasters: so viele Spalten, dass jede Kachel mindestens `min_w` breit ist;
/// würde eine Kachel dabei breiter als 1,5 × `min_w` (schmale Leiste, eine Riesenkachel), eine Spalte mehr,
/// solange die Kacheln noch 0,8 × `min_w` breit bleiben.
pub fn grid_cols(avail: f32, min_w: f32, gap: f32) -> (usize, f32) {
    let width = |cols: usize| ((avail - gap * (cols as f32 - 1.0)) / cols as f32).floor();
    let cols = ((avail + gap) / (min_w + gap)).floor().max(1.0) as usize;
    if width(cols) > 1.5 * min_w && width(cols + 1) >= 0.8 * min_w {
        (cols + 1, width(cols + 1))
    } else {
        (cols, width(cols))
    }
}

/// Wirklich nutzbare Breite eines Rasters: höchstens bis zum sichtbaren rechten Rand (`clip_right`).
/// Eine zu breite Zeile davor (Suchleiste, Ordner-Chips) dehnt `available_width` über den Rand,
/// dann würde die letzte Spalte abgeschnitten.
pub fn grid_avail(available: f32, left: f32, clip_right: f32) -> f32 {
    available.min(clip_right - left).max(1.0)
}

/// Kachel-Raster: berechnet Spalten und ruft `tile` für jedes sichtbare Element mit seinem Rechteck.
/// Zeilen außerhalb des sichtbaren Bereichs belegen nur ihren Platz (Pakete mit hunderten Kacheln).
pub fn grid<T>(
    ui: &mut egui::Ui,
    items: &[T],
    min_w: f32,
    aspect: f32,
    label_h: f32,
    mut tile: impl FnMut(&mut egui::Ui, Rect, &T),
) {
    let gap = 8.0;
    // Kacheln zeichnen 2 px über ihren Rand (Hover/Auswahl): so viel vom sichtbaren Rand abziehen
    let avail = grid_avail(ui.available_width(), ui.cursor().left(), ui.clip_rect().right() - 2.0);
    let (cols, w) = grid_cols(avail, min_w, gap);
    let h = (w / aspect).round();
    for chunk in items.chunks(cols) {
        let (row, _) = ui.allocate_exact_size(vec2(avail, h + label_h), Sense::hover());
        let visible = ui.is_rect_visible(row);
        for (i, it) in chunk.iter().enumerate().filter(|_| visible) {
            let r = Rect::from_min_size(
                pos2(row.left() + i as f32 * (w + gap), row.top()),
                vec2(w, h + label_h),
            );
            tile(ui, r, it);
        }
        ui.add_space(gap - ui.spacing().item_spacing.y);
    }
}

/// Fügt dieses Klick-Ereignis einer Kachel ein? `clicks`: 0 = kein Klick, sonst 1/2/3 (Doppel-/
/// Dreifachklick). `plus_only`: Klick auf die Fläche fügt nicht ein (z. B. Sounds: spielt ab), nur
/// „+“ oder Doppelklick. Ein Doppelklick fügt nur EINMAL ein: sein erster Klick wirkt schon.
pub fn adds(clicks: u8, on_plus: bool, plus_only: bool) -> bool {
    match clicks {
        1 => !plus_only || on_plus,
        // zweiter Klick: nur einfügen, wenn der erste es noch nicht tat
        2 => plus_only && !on_plus,
        _ => false,
    }
}

/// [`adds`] für eine Antwort.
pub fn tile_adds(resp: &egui::Response, on_plus: bool, plus_only: bool) -> bool {
    let clicks = if resp.triple_clicked() {
        3
    } else if resp.double_clicked() {
        2
    } else {
        resp.clicked() as u8
    };
    adds(clicks, on_plus, plus_only)
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
    tile_tip(ui, id, r, label, "", selected, draw)
}

/// Wie [`tile`], mit einer zweiten, gedimmten Tooltip-Zeile `sub` (z. B. Ordner) unter dem Namen –
/// beides in EINEM Tooltip.
pub fn tile_tip(
    ui: &mut egui::Ui,
    id: egui::Id,
    r: Rect,
    label: &str,
    sub: &str,
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
    if resp.hovered() && !ui.data(|d| d.get_temp::<bool>(no_plus_id()).unwrap_or(false)) {
        // „+“ zum direkten Hinzufügen (nicht beim Umsortieren)
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
    let mut resp = resp;
    let tip = |resp: egui::Response, name: Option<&str>| {
        if name.is_none() && sub.is_empty() {
            return resp;
        }
        resp.on_hover_ui(|ui| {
            if let Some(n) = name {
                ui.label(n);
            }
            if !sub.is_empty() {
                ui.label(egui::RichText::new(sub).color(theme::TEXT_DIM()));
            }
        })
    };
    if label.is_empty() {
        resp = tip(resp, None);
    } else {
        // eine Zeile, zu lange Namen mit „…“ gekürzt; voller Name im Tooltip
        let full = theme::icon_arrows(&crate::tr(label));
        let font = egui::FontId::proportional(theme::FS_HINT);
        let shown = theme::elide(&full, r.width(), |t| theme::text_width(ui.ctx(), t, font.clone()));
        // Name oben (gekürzt oder mit Zusatzzeile), Zusatz gedimmt darunter: ein Tooltip
        resp = tip(resp, (shown != full || !sub.is_empty()).then_some(full.as_str()));
        let g = p.layout_no_wrap(
            shown,
            font,
            if selected {
                theme::TEXT()
            } else {
                theme::TEXT_DIM()
            },
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

/// Merker „Kacheln ohne „+“ zeichnen“ (Umsortieren in `library_ui`)
pub fn no_plus_id() -> egui::Id {
    egui::Id::new("tile-no-plus")
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

// ------------------------------------------------------------ Ladezustände

/// Lauflinie: Lage des wandernden Stücks (Anfang, Ende als Anteil 0…1) zur Zeit `t`.
/// Ohne Animationen steht die ganze Linie still.
pub fn busy_segment(t: f64, moving: bool) -> (f32, f32) {
    if !moving {
        return (0.0, 1.0);
    }
    const LEN: f32 = 0.3;
    // 1,4 s je Durchlauf, ruhig und gleichmäßig
    let x = (t / 1.4).fract() as f32 * (1.0 + LEN) - LEN;
    (x.max(0.0), (x + LEN).min(1.0))
}

/// Schimmer auf einem Platzhalter: Mitte des hellen Bandes (Anteil, darf über die Ränder laufen)
/// `age` Sekunden nach dem Erscheinen; `None` = ruhige Fläche (noch zu früh, zu lange oder ohne
/// Animationen – ein Bild, das nie kommt, lässt die Oberfläche nicht ewig neu zeichnen).
pub fn shimmer_at(age: f64, moving: bool) -> Option<f32> {
    (moving && (theme::LOAD_DELAY..6.0).contains(&age))
        .then(|| (((age - theme::LOAD_DELAY) / 1.6).fract() as f32) * 1.6 - 0.3)
}

/// Fortschrittslinie (bekannte Dauer): dünn, Akzentfarbe, folgt dem Wert weich.
pub fn progress_line(p: &egui::Painter, id: egui::Id, rect: Rect, frac: f32) {
    let shown = theme::glide(p.ctx(), id.with("progress"), frac.clamp(0.0, 1.0), theme::MOTION_SLOW);
    let r = rect.height() / 2.0;
    p.rect_filled(rect, r, theme::overlay(26));
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * shown, rect.height()));
    p.rect_filled(fill, r, theme::ACCENT());
}

/// Lauflinie (unbekannte Dauer): ein ruhig wanderndes Stück in Akzentfarbe; ohne Animationen
/// eine stehende, gedämpfte Linie.
pub fn busy_line(p: &egui::Painter, rect: Rect) {
    let moving = theme::animations_on();
    let r = rect.height() / 2.0;
    p.rect_filled(rect, r, theme::overlay(26));
    let (a, b) = busy_segment(p.ctx().input(|i| i.time), moving);
    let seg = Rect::from_x_y_ranges(egui::lerp(rect.x_range(), a)..=egui::lerp(rect.x_range(), b), rect.y_range());
    p.rect_filled(seg, r, if moving { theme::ACCENT() } else { theme::ACCENT().gamma_multiply(0.5) });
    if moving {
        p.ctx().request_repaint();
    }
}

/// Platzhalter in Zielgröße, solange Inhalt (Bild, Wellenform …) fehlt: ruhige Fläche mit sehr
/// dezentem Schimmer. `false` = wartet schon lange (Inhalt kommt wohl nicht; Aufrufer zeigt dann
/// sein Ersatz-Symbol).
pub fn skeleton(p: &egui::Painter, id: egui::Id, rect: Rect, radius: f32) -> bool {
    let ctx = p.ctx();
    let id = id.with("skeleton");
    let (now, pass) = (ctx.input(|i| i.time), ctx.cumulative_pass_nr());
    let (since, last) = ctx.data(|d| d.get_temp::<(f64, u64)>(id)).unwrap_or((now, pass));
    // länger nicht gezeichnet: neu beginnen
    let since = if last + 1 < pass { now } else { since };
    ctx.data_mut(|d| d.insert_temp(id, (since, pass)));
    let age = now - since;
    p.rect_filled(rect, radius, theme::BG_RAISED());
    if let Some(x) = shimmer_at(age, theme::animations_on()) {
        let cx = rect.left() + x * rect.width();
        let band = Rect::from_center_size(egui::pos2(cx, rect.center().y), vec2(rect.width() * 0.35, rect.height()));
        // weiche Kanten: drei gestaffelte Streifen
        for (k, a) in [(1.0, 5u8), (0.6, 5), (0.3, 6)] {
            p.with_clip_rect(rect.intersect(p.clip_rect())).rect_filled(
                Rect::from_center_size(band.center(), vec2(band.width() * k, band.height())),
                radius,
                theme::overlay(a),
            );
        }
        ctx.request_repaint();
    } else if age < theme::LOAD_DELAY {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(theme::LOAD_DELAY - age + 0.005));
    }
    age < 6.0
}

/// Inhalt einer Kachel, der gerade eintrifft: Deckkraft 0…1. Was schon beim Öffnen da ist, steht
/// sofort; was nachkommt, blendet einmal ein (Zustand je `id`).
pub fn reveal(ctx: &egui::Context, id: egui::Id, ready: bool) -> f32 {
    theme::fade(ctx, id.with("reveal"), ready, theme::MOTION_FAST)
}

/// Bild einer Kachel: fehlt es noch, ein Platzhalter (nach langer Wartezeit `glyph`); trifft es
/// ein, blendet es über dem Platzhalter ein. `tex`: (Textur, Bildgröße, uv-Ausschnitt) – das Bild
/// füllt die Fläche (Ränder abgeschnitten).
pub fn tile_image(p: &egui::Painter, id: egui::Id, a: Rect, tex: Option<(egui::TextureId, egui::Vec2, Rect)>, glyph: &str) {
    let k = reveal(p.ctx(), id, tex.is_some());
    let Some((tid, ts, src)) = tex else {
        if !skeleton(p, id, a, theme::R_CARD) && !glyph.is_empty() {
            // Icon-Glyphe als Platzhalter
            p.text(a.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(22.0), theme::TEXT_DIM());
        }
        return;
    };
    if k < 1.0 {
        skeleton(p, id, a, theme::R_CARD);
    }
    let s = (a.width() / ts.x).max(a.height() / ts.y);
    let uv = Rect::from_center_size(
        egui::pos2(0.5, 0.5),
        vec2((a.width() / (ts.x * s)).min(1.0), (a.height() / (ts.y * s)).min(1.0)),
    );
    let uv = Rect::from_x_y_ranges(
        egui::lerp(src.x_range(), uv.left())..=egui::lerp(src.x_range(), uv.right()),
        egui::lerp(src.y_range(), uv.top())..=egui::lerp(src.y_range(), uv.bottom()),
    );
    p.add(egui::epaint::RectShape::filled(a, theme::R_CARD, Color32::WHITE.gamma_multiply(k)).with_texture(tid, uv));
}

/// Linie einer laufenden Aufgabe in `rect`: Fortschritt, sobald einer gemeldet wird, sonst Lauflinie.
pub fn task_line(p: &egui::Painter, id: egui::Id, rect: Rect, progress: f32) {
    if progress > 0.0 {
        progress_line(p, id, rect, progress);
    } else {
        busy_line(p, rect);
    }
}

/// Zeile mit Lauflinie unter einem Element (Suchfeld, Liste): belegt ihre Höhe immer (kein
/// Springen), zeigt die Linie erst nach 150 ms Wartezeit und mindestens 300 ms lang.
pub fn busy_row(ui: &mut egui::Ui, id: egui::Id, busy: bool) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 2.0), Sense::hover());
    if theme::loading(ui.ctx(), id, busy) {
        busy_line(ui.painter(), r);
    }
}

#[cfg(test)]
mod tests {
    use super::{grid_avail, grid_cols};
    use eframe::egui::vec2;

    #[test]
    fn a_double_click_inserts_only_once() {
        use super::adds;
        // Ereignisse eines Doppelklicks: erst ein Klick, dann der zweite (Doppel-)Klick
        let count = |on_plus, plus_only| [1u8, 2].iter().filter(|&&c| adds(c, on_plus, plus_only)).count();
        // Klick fügt ein (Effekte, Filter, Übergänge, Texte, Elemente)
        assert_eq!(count(false, false), 1);
        assert_eq!(count(true, false), 1);
        // nur „+“ oder Doppelklick (Sounds, Medien): auf „+“ und auf der Fläche je einmal
        assert_eq!(count(true, true), 1);
        assert_eq!(count(false, true), 1);
        // Einzelklick auf die Fläche fügt dort nicht ein, Dreifachklick nicht noch einmal
        assert!(!adds(1, false, true) && !adds(3, false, false) && !adds(0, true, false));
    }

    #[test]
    fn grid_stays_inside_visible_width() {
        // Raster beginnt bei x = 100, sichtbar bis 400: 300 px, auch wenn die Zeile 320 meldet
        assert_eq!(grid_avail(320.0, 100.0, 400.0), 300.0);
        assert_eq!(grid_avail(250.0, 100.0, 400.0), 250.0);
        for min_w in [70.0, 84.0, 120.0] {
            for vis in (180..=600).map(|v| v as f32) {
                let a = grid_avail(vis + 12.0, 0.0, vis);
                let (c, w) = grid_cols(a, min_w, 8.0);
                assert!(c as f32 * w + (c as f32 - 1.0) * 8.0 <= vis + 0.01, "{min_w} {vis}");
            }
        }
    }

    #[test]
    fn grid_never_makes_giant_tiles() {
        // schmale Leiste (Text-Vorlagen, ~232 px): zwei Spalten statt einer 232er-Kachel
        let (c, w) = grid_cols(232.0, 120.0, 8.0);
        assert_eq!(c, 2);
        assert!(w >= 0.8 * 120.0 && 2.0 * w + 8.0 <= 232.0);
        // normale Breiten bleiben wie bisher
        assert_eq!(grid_cols(400.0, 120.0, 8.0).0, 3);
        assert_eq!(grid_cols(300.0, 70.0, 8.0).0, 3);
        // nichts passt: eine Spalte in voller Breite
        assert_eq!(grid_cols(80.0, 120.0, 8.0), (1, 80.0));
        for avail in (60..900).step_by(7).map(|a| a as f32) {
            let (c, w) = grid_cols(avail, 100.0, 8.0);
            assert!(c as f32 * w + (c as f32 - 1.0) * 8.0 <= avail + 0.01, "{avail}");
        }
    }

    #[test]
    fn grid_builds_only_visible_rows() {
        let ctx = eframe::egui::Context::default();
        let items: Vec<usize> = (0..300).collect();
        let (mut built, mut height) = (Vec::new(), 0.0);
        let input = || eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(Default::default(), vec2(400.0, 600.0))),
            ..Default::default()
        };
        // erstes Bild ist der Mess-Durchgang von egui, gezählt wird das zweite
        let _ = ctx.run(input(), |ctx| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                eframe::egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| super::grid(ui, &items, 70.0, 1.0, 16.0, |_, _, _| {}));
            });
        });
        let _ = ctx.run(input(), |ctx| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_width(240.0);
                let out = eframe::egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    super::grid(ui, &items, 70.0, 1.0, 16.0, |_, _, i| built.push(*i));
                });
                height = out.content_size.y;
            });
        });
        // 300 px hoch, Zeilen ~88 px: nur die paar sichtbaren Zeilen, der Rest belegt nur Platz
        assert!(!built.is_empty() && built.len() <= 30, "{}", built.len());
        assert_eq!(built[0], 0);
        assert!(height > 300.0 / 6.0 * 80.0, "Inhalt muss so hoch bleiben wie alle Zeilen: {height}");
    }
}
