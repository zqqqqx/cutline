//! Direkte Bearbeitung im Player:
//! - Klick wählt das oberste Element unter dem Zeiger am Playhead (auch in der Zeitleiste)
//! - Ziehen verschiebt, rastet an der Bildmitte ein (türkise Hilfslinien)
//! - Eckgriffe skalieren gleichmäßig um den Mittelpunkt
//! - Drehgriff unten dreht, rastet bei 0/90/180/270° ein, Winkel-Badge
//! - ✕ oben links löscht
//! - Während des Ziehens nur Vorschau; beim Loslassen genau ein Undo-Schritt

use cl_core::{ClipContent, ClipId, Transform};
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Stroke, Vec2};

use crate::app::App;
use crate::theme::{self, icon};

/// Trefferfläche eines sichtbaren Elements im Player (Bildschirmkoordinaten).
#[derive(Clone, Copy)]
pub struct Region {
    pub clip: ClipId,
    pub center: Pos2,
    pub size: Vec2,
    /// Radiant, im Uhrzeigersinn
    pub angle: f32,
    /// Ausgewertete Transformation am Playhead (Startwert für Gesten)
    pub tr: Transform,
}

impl Region {
    fn rot(&self, v: Vec2) -> Vec2 {
        let (s, c) = self.angle.sin_cos();
        vec2(v.x * c - v.y * s, v.x * s + v.y * c)
    }
    pub fn corner(&self, sx: f32, sy: f32) -> Pos2 {
        self.center + self.rot(vec2(sx * self.size.x / 2.0, sy * self.size.y / 2.0))
    }
    pub fn contains(&self, p: Pos2) -> bool {
        let d = p - self.center;
        let (s, c) = (-self.angle).sin_cos();
        let local = vec2(d.x * c - d.y * s, d.x * s + d.y * c);
        local.x.abs() <= self.size.x / 2.0 + 2.0 && local.y.abs() <= self.size.y / 2.0 + 2.0
    }
    pub fn rotate_handle(&self) -> Pos2 {
        self.center + self.rot(vec2(0.0, self.size.y / 2.0 + 22.0))
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Move { press: Pos2, start: Transform },
    Scale { start: Transform, press_dist: f32 },
    Rotate { start: Transform, press_angle: f32 },
}

#[derive(Default)]
pub struct Gizmo {
    drag: Option<(ClipId, Mode)>,
    /// Laufende Änderung (nur Vorschau, noch nicht im Verlauf)
    pub live: Option<(ClipId, Transform)>,
    /// Aktive Hilfslinien (Projektpixel relativ zur Bildmitte): senkrecht bei x, waagrecht bei y
    guides_x: Vec<f32>,
    guides_y: Vec<f32>,
}

/// Halbe Breite/Höhe der achsparallelen Hülle eines (gedrehten) Elements in Bildschirmpixeln.
fn half_extent(r: &Region) -> Vec2 {
    let pts = [r.corner(-1.0, -1.0), r.corner(1.0, -1.0), r.corner(1.0, 1.0), r.corner(-1.0, 1.0)];
    let (mut w, mut h) = (0.0f32, 0.0f32);
    for p in pts {
        w = w.max((p.x - r.center.x).abs());
        h = h.max((p.y - r.center.y).abs());
    }
    vec2(w, h)
}

/// Bester Versatz, der eines der Merkmale `feats` an einen Kandidaten `cands` bringt
/// (innerhalb `tol`). Gibt (Versatz, Linie) zurück.
fn best_snap(feats: &[f32], cands: &[f32], tol: f32) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32)> = None;
    for f in feats {
        for c in cands {
            let d = c - f;
            if d.abs() <= tol && best.is_none_or(|(b, _)| d.abs() < b.abs()) {
                best = Some((d, *c));
            }
        }
    }
    best
}

const HANDLE: f32 = 9.0;
const SNAP_PX: f32 = 10.0;

impl App {
    /// Verarbeitet Maus-Eingaben im Player und zeichnet den Bearbeitungsrahmen.
    /// `regions` von unten nach oben; `px_per_unit` = Bildschirmpixel pro Projektpixel.
    pub fn player_interact(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        canvas: Rect,
        px_per_unit: f32,
        regions: &[Region],
    ) {
        let painter = ui.painter_at(resp.rect);
        let sel = self
            .selected
            .and_then(|s| regions.iter().find(|r| r.clip == s).copied());
        let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());

        // Was liegt unter dem Zeiger? (Griffe des ausgewählten Elements haben Vorrang)
        let sel_tr = sel.map(|r| r.tr);
        let hit_handle = move |p: Pos2| -> Option<Mode> {
            let r = sel?;
            let t = sel_tr?;
            if p.distance(r.rotate_handle()) <= HANDLE + 3.0 {
                let a = (p - r.center).angle();
                return Some(Mode::Rotate {
                    start: t,
                    press_angle: a,
                });
            }
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                if (sx, sy) != (-1.0, -1.0) && p.distance(r.corner(sx, sy)) <= HANDLE + 2.0 {
                    return Some(Mode::Scale {
                        start: t,
                        press_dist: p.distance(r.center).max(1.0),
                    });
                }
            }
            None
        };
        let delete_btn = sel.map(|r| r.corner(-1.0, -1.0));

        if let Some(p) = resp.hover_pos() {
            if let Some(m) = hit_handle(p) {
                ui.ctx().set_cursor_icon(match m {
                    Mode::Rotate { .. } => egui::CursorIcon::Alias,
                    _ => egui::CursorIcon::ResizeNwSe,
                });
            } else if regions.iter().any(|r| r.contains(p)) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
            }
        }

        // Klick: ✕ löscht, sonst oberstes Element wählen bzw. Auswahl aufheben
        if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if delete_btn.is_some_and(|d| p.distance(d) <= HANDLE + 2.0) {
                    self.delete_selected();
                    return;
                }
                let hit = regions.iter().rev().find(|r| r.contains(p)).map(|r| r.clip);
                if ui.input(|i| i.modifiers.command || i.modifiers.shift) {
                    if let Some(h) = hit {
                        self.toggle_select(h);
                    }
                } else {
                    self.select_only(hit);
                }
            }
        }
        if resp.double_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(r) = regions.iter().rev().find(|r| r.contains(p)) {
                    if self
                        .project
                        .find_clip(r.clip)
                        .is_some_and(|(_, c)| matches!(c.content, ClipContent::Text(_)))
                    {
                        self.select_only(Some(r.clip));
                        self.clip_tab = crate::app::ClipTab::Text;
                        self.focus_text = true;
                    }
                }
            }
        }

        if resp.drag_started() {
            if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                if let Some(m) = hit_handle(p) {
                    self.gizmo.drag = Some((sel.expect("Griff gehört zur Auswahl").clip, m));
                } else if let Some(r) = regions.iter().rev().find(|r| r.contains(p)) {
                    if !self.is_selected(r.clip) {
                        self.select_only(Some(r.clip));
                    }
                    self.commit_live();
                    self.gizmo.drag = Some((
                        r.clip,
                        Mode::Move {
                            press: p,
                            start: r.tr,
                        },
                    ));
                }
            }
        }

        let mut badge: Option<String> = None;
        if let (Some((clip, mode)), Some(p)) = (self.gizmo.drag, pointer) {
            let reg = regions.iter().find(|r| r.clip == clip).copied();
            let mut t = match mode {
                Mode::Move { start, .. }
                | Mode::Scale { start, .. }
                | Mode::Rotate { start, .. } => start,
            };
            self.gizmo.guides_x.clear();
            self.gizmo.guides_y.clear();
            let (pw, ph) = (canvas.width() / px_per_unit, canvas.height() / px_per_unit);
            let free = ui.input(|i| i.modifiers.alt); // Alt = ohne Einrasten
            let tol = SNAP_PX / px_per_unit;
            match mode {
                Mode::Move { press, start } => {
                    let d = (p - press) / px_per_unit;
                    t.x = start.x + d.x;
                    t.y = start.y + d.y;
                    if let (Some(r), false) = (reg, free) {
                        // Kanten und Mitte an Bildrand, Mitte, Dritteln und anderen Elementen
                        let he = half_extent(&r) / px_per_unit;
                        let mut cx = vec![-pw / 2.0, -pw / 6.0, 0.0, pw / 6.0, pw / 2.0];
                        let mut cy = vec![-ph / 2.0, -ph / 6.0, 0.0, ph / 6.0, ph / 2.0];
                        for o in regions.iter().filter(|o| o.clip != clip) {
                            let oc = (o.center - canvas.center()) / px_per_unit;
                            let oe = half_extent(o) / px_per_unit;
                            cx.extend([oc.x - oe.x, oc.x, oc.x + oe.x]);
                            cy.extend([oc.y - oe.y, oc.y, oc.y + oe.y]);
                        }
                        if let Some((dx, line)) = best_snap(&[t.x - he.x, t.x, t.x + he.x], &cx, tol) {
                            t.x += dx;
                            self.gizmo.guides_x.push(line);
                        }
                        if let Some((dy, line)) = best_snap(&[t.y - he.y, t.y, t.y + he.y], &cy, tol) {
                            t.y += dy;
                            self.gizmo.guides_y.push(line);
                        }
                    }
                }
                Mode::Scale { start, press_dist } => {
                    if let Some(r) = reg {
                        t.scale =
                            (start.scale * p.distance(r.center) / press_dist).clamp(0.02, 20.0);
                        if !free {
                            // Kanten am Bildrand einrasten, außerdem bei 100 %
                            let cur = self.gizmo.live.map(|l| l.1.scale).unwrap_or(start.scale).max(0.001);
                            let base = half_extent(&r) / px_per_unit / cur;
                            let raw = t.scale;
                            let mut best: Option<(f32, Option<(bool, f32)>)> = None;
                            let mut consider = |sc: f32, line: Option<(bool, f32)>| {
                                let near = ((sc - raw) * base.x.max(base.y) * px_per_unit).abs() <= SNAP_PX;
                                if sc > 0.02 && near && best.is_none_or(|(b, _)| (sc - raw).abs() < (b - raw).abs()) {
                                    best = Some((sc, line));
                                }
                            };
                            if base.x > 0.0 {
                                consider((pw / 2.0 - t.x) / base.x, Some((true, pw / 2.0)));
                                consider((pw / 2.0 + t.x) / base.x, Some((true, -pw / 2.0)));
                            }
                            if base.y > 0.0 {
                                consider((ph / 2.0 - t.y) / base.y, Some((false, ph / 2.0)));
                                consider((ph / 2.0 + t.y) / base.y, Some((false, -ph / 2.0)));
                            }
                            consider(1.0, None);
                            if let Some((sc, line)) = best {
                                t.scale = sc;
                                match line {
                                    Some((true, x)) => self.gizmo.guides_x.push(x),
                                    Some((false, y)) => self.gizmo.guides_y.push(y),
                                    None => {}
                                }
                            }
                        }
                        badge = Some(format!("{:.0} %", t.scale * 100.0));
                    }
                }
                Mode::Rotate { start, press_angle } => {
                    if let Some(r) = reg {
                        let mut deg = start.rotation_deg
                            + ((p - r.center).angle() - press_angle).to_degrees();
                        deg = ((deg + 180.0).rem_euclid(360.0)) - 180.0;
                        for snap in [-180.0f32, -90.0, 0.0, 90.0, 180.0] {
                            if (deg - snap).abs() < 3.0 {
                                deg = snap;
                            }
                        }
                        t.rotation_deg = deg;
                        badge = Some(format!("{deg:.1}°"));
                    }
                }
            }
            self.gizmo.live = Some((clip, t));
            ui.ctx().request_repaint();
        }
        if resp.drag_stopped() {
            if let (Some((clip, _)), Some((_, t))) =
                (self.gizmo.drag.take(), self.gizmo.live.take())
            {
                self.commit_transform(clip, t);
            }
            self.gizmo.guides_x.clear();
            self.gizmo.guides_y.clear();
        }

        // Beim Verschieben/Skalieren: dezentes Raster (Drittel, Mitte) und aktive Hilfslinien
        if self.gizmo.drag.is_some_and(|(_, m)| !matches!(m, Mode::Rotate { .. })) {
            let grid = Stroke::new(1.0_f32, Color32::from_white_alpha(28));
            for k in [1.0 / 3.0, 0.5, 2.0 / 3.0] {
                let x = canvas.left() + canvas.width() * k;
                let y = canvas.top() + canvas.height() * k;
                painter.line_segment([pos2(x, canvas.top()), pos2(x, canvas.bottom())], grid);
                painter.line_segment([pos2(canvas.left(), y), pos2(canvas.right(), y)], grid);
            }
        }
        let guide = Stroke::new(1.0_f32, theme::SNAP);
        for x in &self.gizmo.guides_x {
            let sx = canvas.center().x + x * px_per_unit;
            painter.line_segment([pos2(sx, canvas.top() - 6.0), pos2(sx, canvas.bottom() + 6.0)], guide);
        }
        for y in &self.gizmo.guides_y {
            let sy = canvas.center().y + y * px_per_unit;
            painter.line_segment([pos2(canvas.left() - 6.0, sy), pos2(canvas.right() + 6.0, sy)], guide);
        }

        // Rahmen + Griffe des ausgewählten Elements
        if let Some(r) = sel {
            let pts = [
                r.corner(-1.0, -1.0),
                r.corner(1.0, -1.0),
                r.corner(1.0, 1.0),
                r.corner(-1.0, 1.0),
            ];
            let frame = Stroke::new(1.0_f32, Color32::from_white_alpha(230));
            for i in 0..4 {
                painter.line_segment([pts[i], pts[(i + 1) % 4]], frame);
            }
            for p in &pts[1..] {
                painter.circle(
                    *p,
                    4.5,
                    Color32::WHITE,
                    Stroke::new(1.0_f32, Color32::from_black_alpha(120)),
                );
            }
            // ✕ löschen
            painter.circle(
                pts[0],
                7.5,
                Color32::from_rgb(0x30, 0x30, 0x34),
                Stroke::new(1.0_f32, Color32::from_white_alpha(200)),
            );
            painter.text(
                pts[0],
                egui::Align2::CENTER_CENTER,
                icon::X,
                egui::FontId::proportional(9.0),
                Color32::WHITE,
            );
            // Drehgriff
            let rh = r.rotate_handle();
            painter.line_segment(
                [r.center + r.rot(vec2(0.0, r.size.y / 2.0)), rh],
                Stroke::new(1.0_f32, Color32::from_white_alpha(140)),
            );
            painter.circle(
                rh,
                8.0,
                Color32::from_rgb(0x30, 0x30, 0x34),
                Stroke::new(1.0_f32, Color32::WHITE),
            );
            painter.text(
                rh,
                egui::Align2::CENTER_CENTER,
                icon::ARROW_CLOCKWISE,
                egui::FontId::proportional(10.0),
                Color32::WHITE,
            );
            if let Some(b) = badge {
                let g = painter.layout_no_wrap(b, egui::FontId::proportional(11.0), Color32::BLACK);
                let top = pts.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
                let br = Rect::from_center_size(
                    pos2(r.center.x, top - 16.0),
                    g.size() + vec2(12.0, 6.0),
                );
                painter.rect_filled(br, 9.0, Color32::from_white_alpha(235));
                painter.galley(br.min + vec2(6.0, 3.0), g, Color32::BLACK);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_region_hit_test() {
        let id = ClipId::new();
        let r = Region {
            clip: id,
            center: pos2(100.0, 100.0),
            size: vec2(100.0, 20.0),
            angle: std::f32::consts::FRAC_PI_2,
            tr: Transform::default(),
        };
        // um 90° gedreht: jetzt hoch und schmal
        assert!(r.contains(pos2(100.0, 145.0)));
        assert!(!r.contains(pos2(145.0, 100.0)));
        let c = r.corner(1.0, 1.0);
        assert!(
            (c.x - 90.0).abs() < 0.01 && (c.y - 150.0).abs() < 0.01,
            "{c:?}"
        );
    }
}

// ---------------------------------------------------------------- Player-Zoom

/// Zoomstufen relativ zu „Einpassen“
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 16.0;

impl App {
    /// Strg+Mausrad: Zoom um den Mauszeiger; Mausrad/mittlere Taste: Ansicht verschieben;
    /// Alt+Umschalt+Plus/Minus/Z.
    pub fn player_zoom_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        area: Rect,
        fit_scale: f32,
        pw: f32,
        ph: f32,
    ) {
        let (scroll, ctrl, zoom_delta, mid_drag, pointer) = ui.input(|i| {
            (
                i.raw_scroll_delta,
                i.modifiers.command,
                i.zoom_delta(),
                i.pointer.middle_down(),
                i.pointer.hover_pos(),
            )
        });
        if resp.hovered() {
            let factor = if ctrl && scroll.y != 0.0 {
                (1.0 + scroll.y * 0.0018).clamp(0.5, 2.0)
            } else {
                zoom_delta
            };
            if (factor - 1.0).abs() > 1e-4 {
                let anchor = pointer.unwrap_or(area.center());
                self.zoom_player_around(anchor, area, factor);
            } else if !ctrl && (scroll.x != 0.0 || scroll.y != 0.0) && self.player_zoom > 1.0 {
                self.player_pan += scroll;
            }
            if mid_drag {
                self.player_pan += ui.input(|i| i.pointer.delta());
            }
        }
        let k = |key: egui::Key| {
            ui.ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::ALT | egui::Modifiers::SHIFT, key))
        };
        if k(egui::Key::Plus) || k(egui::Key::Equals) {
            self.zoom_player_around(area.center(), area, 1.25);
        }
        if k(egui::Key::Minus) {
            self.zoom_player_around(area.center(), area, 0.8);
        }
        if k(egui::Key::Z) {
            self.player_zoom = 1.0;
            self.player_pan = egui::Vec2::ZERO;
        }
        // Leinwand nie ganz aus dem Blick schieben
        let half = vec2(pw, ph) * fit_scale * self.player_zoom / 2.0 + area.size() / 2.0
            - vec2(40.0, 40.0);
        self.player_pan = self
            .player_pan
            .clamp(-half.max(Vec2::ZERO), half.max(Vec2::ZERO));
        if self.player_zoom <= 1.0 {
            self.player_pan = egui::Vec2::ZERO;
        }
    }

    fn zoom_player_around(&mut self, anchor: Pos2, area: Rect, factor: f32) {
        let old = self.player_zoom;
        let new = (old * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        // Punkt unter dem Zeiger bleibt stehen
        let c = area.center() + self.player_pan;
        self.player_pan += (anchor - c) * (1.0 - new / old);
        self.player_zoom = new;
    }

    /// Chip in der Player-Leiste mit Popover (−, Regler, +, Einpassen, 1:1).
    pub fn player_zoom_chip(&mut self, ui: &mut egui::Ui, compact: bool) {
        let pct = format!("{:.0} %", self.player_zoom * 100.0);
        let label = if compact {
            icon::MAGNIFYING_GLASS.to_string()
        } else if (self.player_zoom - 1.0).abs() < 1e-3 {
            format!("{} Einpassen", icon::MAGNIFYING_GLASS)
        } else {
            format!("{} {pct}", icon::MAGNIFYING_GLASS)
        };
        let resp = theme::chip(ui, &label).on_hover_text(format!(
            "Player-Zoom {pct} (Strg+Mausrad, Alt+Umschalt+Z einpassen)"
        ));
        let id = ui.make_persistent_id("player-zoom");
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(id));
        }
        egui::popup_above_or_below_widget(
            ui,
            id,
            &resp,
            egui::AboveOrBelow::Above,
            egui::PopupCloseBehavior::CloseOnClickOutside,
            |ui| {
                ui.set_min_width(220.0);
                ui.horizontal(|ui| {
                    if theme::icon_button(
                        ui,
                        icon::MAGNIFYING_GLASS_MINUS,
                        "Herauszoomen (Alt+Umschalt+−)",
                        true,
                        false,
                    )
                    .clicked()
                    {
                        self.player_zoom = (self.player_zoom * 0.8).max(ZOOM_MIN);
                    }
                    let mut z = self.player_zoom;
                    ui.allocate_ui(vec2(140.0, 20.0), |ui| {
                        if crate::widgets::Slider::new(&mut z, ZOOM_MIN..=ZOOM_MAX)
                            .log()
                            .percent()
                            .default(1.0)
                            .show(ui)
                            .changed
                        {
                            self.player_zoom = z;
                        }
                    });
                    if theme::icon_button(
                        ui,
                        icon::MAGNIFYING_GLASS_PLUS,
                        "Heranzoomen (Alt+Umschalt++)",
                        true,
                        false,
                    )
                    .clicked()
                    {
                        self.player_zoom = (self.player_zoom * 1.25).min(ZOOM_MAX);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add(theme::secondary("Einpassen")).clicked() {
                        self.player_zoom = 1.0;
                        self.player_pan = egui::Vec2::ZERO;
                    }
                    for z in [0.5, 2.0, 4.0] {
                        if ui
                            .add(theme::secondary(format!("{:.0} %", z * 100.0)))
                            .clicked()
                        {
                            self.player_zoom = z;
                        }
                    }
                });
            },
        );
    }
}
