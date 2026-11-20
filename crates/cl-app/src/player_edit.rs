//! Direct editing in the player:
//! - click selects the topmost element under the pointer at the playhead (also in the timeline)
//! - dragging moves, snaps to the frame center (turquoise guides)
//! - corner handles scale uniformly around the center
//! - the rotate handle at the bottom rotates, snaps at 0/90/180/270°, angle badge
//! - ✕ at the top left deletes
//! - bars in the middle of the edges (image, video, element) crop
//! - only a preview while dragging; exactly one undo step on release

use cl_core::{ClipContent, ClipId, Crop, EditOp, Transform};
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, Stroke, Vec2};

use crate::app::App;
use crate::theme::{self, icon};

/// Hit area of a visible element in the player (screen coordinates).
#[derive(Clone, Copy)]
pub struct Region {
    pub clip: ClipId,
    pub center: Pos2,
    pub size: Vec2,
    /// Radians, clockwise
    pub angle: f32,
    /// Evaluated transform at the playhead (start value for gestures)
    pub tr: Transform,
    /// Crop (`size` stays the full clip size)
    pub crop: Crop,
}

impl Region {
    fn rot(&self, v: Vec2) -> Vec2 {
        let (s, c) = self.angle.sin_cos();
        vec2(v.x * c - v.y * s, v.x * s + v.y * c)
    }
    pub fn corner(&self, sx: f32, sy: f32) -> Pos2 {
        self.center + self.rot(vec2(sx * self.size.x / 2.0, sy * self.size.y / 2.0))
    }
    /// Corner of the visible (cropped) area, `sx`/`sy` = ±1
    pub fn vis_corner(&self, sx: f32, sy: f32) -> Pos2 {
        let k = self.crop;
        let u = if sx < 0.0 { k.left } else { 1.0 - k.right };
        let v = if sy < 0.0 { k.top } else { 1.0 - k.bottom };
        self.corner(2.0 * u - 1.0, 2.0 * v - 1.0)
    }
    /// Does `p` lie in the visible (cropped) area? 2 px of slack.
    pub fn contains(&self, p: Pos2) -> bool {
        let d = p - self.center;
        let (s, c) = (-self.angle).sin_cos();
        let local = vec2(d.x * c - d.y * s, d.x * s + d.y * c) + self.size / 2.0;
        let k = self.crop;
        let (x0, x1) = (k.left * self.size.x - 2.0, (1.0 - k.right) * self.size.x + 2.0);
        let (y0, y1) = (k.top * self.size.y - 2.0, (1.0 - k.bottom) * self.size.y + 2.0);
        (x0..=x1).contains(&local.x) && (y0..=y1).contains(&local.y)
    }
    /// Rotate handle: 22 px below the center of the visible bottom edge
    pub fn rotate_handle(&self) -> Pos2 {
        self.crop_handle(3) + self.rot(vec2(0.0, 22.0))
    }
    /// Handle of the crop edge `edge` (0 left, 1 right, 2 top, 3 bottom): center of the visible edge.
    pub fn crop_handle(&self, edge: usize) -> Pos2 {
        let k = self.crop;
        let (u0, u1, v0, v1) = (k.left, 1.0 - k.right, k.top, 1.0 - k.bottom);
        let (u, v) = match edge {
            0 => (u0, (v0 + v1) / 2.0),
            1 => (u1, (v0 + v1) / 2.0),
            2 => ((u0 + u1) / 2.0, v0),
            _ => ((u0 + u1) / 2.0, v1),
        };
        self.corner(2.0 * u - 1.0, 2.0 * v - 1.0)
    }
}

/// Crop when dragging edge `edge` from `start` to the spot `p` (screen): the edge
/// follows the pointer in the (rotated) clip, the opposite side leaves at least `Crop::MIN_REST`.
fn crop_drag(r: &Region, edge: usize, start: Crop, p: Pos2) -> Crop {
    let d = p - r.center;
    let (s, c) = r.angle.sin_cos();
    let u = (d.x * c + d.y * s) / r.size.x.max(1.0) + 0.5;
    let v = (-d.x * s + d.y * c) / r.size.y.max(1.0) + 0.5;
    let max = 1.0 - Crop::MIN_REST;
    let mut k = start;
    match edge {
        0 => k.left = u.clamp(0.0, (max - k.right).max(0.0)),
        1 => k.right = (1.0 - u).clamp(0.0, (max - k.left).max(0.0)),
        2 => k.top = v.clamp(0.0, (max - k.bottom).max(0.0)),
        _ => k.bottom = (1.0 - v).clamp(0.0, (max - k.top).max(0.0)),
    }
    k
}

/// Handle of the selected element under `p`. The nearest wins: with a small visible
/// cutout, corner handles, crop bars and rotate handle lie close together.
/// Scaling computes around `r.center` (center of the whole clip, around which scaling acts).
fn handle_at(r: &Region, croppable: bool, p: Pos2) -> Option<Mode> {
    let t = r.tr;
    let mut best: Option<(f32, Mode)> = None;
    let mut take = |at: Pos2, tol: f32, m: Mode| {
        let d = p.distance(at);
        if d <= tol && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, m));
        }
    };
    take(r.rotate_handle(), HANDLE + 3.0, Mode::Rotate { start: t, press_angle: (p - r.center).angle() });
    for (sx, sy) in [(1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        take(r.vis_corner(sx, sy), HANDLE + 2.0, Mode::Scale { start: t, press_dist: p.distance(r.center).max(1.0) });
    }
    if croppable {
        for edge in 0..4 {
            take(r.crop_handle(edge), HANDLE + 2.0, Mode::Crop { edge, start: r.crop });
        }
    }
    best.map(|b| b.1)
}

#[derive(Clone, Copy)]
enum Mode {
    Move { press: Pos2, start: Transform },
    Scale { start: Transform, press_dist: f32 },
    Rotate { start: Transform, press_angle: f32 },
    Crop { edge: usize, start: Crop },
}

#[derive(Default)]
pub struct Gizmo {
    drag: Option<(ClipId, Mode)>,
    /// Running change (only a preview, not yet in the history)
    pub live: Option<(ClipId, Transform)>,
    /// Running crop (only a preview)
    pub live_crop: Option<(ClipId, Crop)>,
    /// Active guides (project pixels relative to the frame center): vertical at x, horizontal at y
    guides_x: Vec<f32>,
    guides_y: Vec<f32>,
}

/// Axis-aligned hull of the visible (cropped, rotated) area relative to
/// `r.center` in screen pixels: (min, max).
fn vis_bounds(r: &Region) -> (Vec2, Vec2) {
    let (mut lo, mut hi) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let d = r.vis_corner(sx, sy) - r.center;
        lo = lo.min(d);
        hi = hi.max(d);
    }
    (lo, hi)
}

/// Features for snapping in project pixels when the clip center lies at `at`:
/// edges and center of the visible area, each (x, y).
fn snap_feats(r: &Region, at: Vec2, px_per_unit: f32) -> ([f32; 3], [f32; 3]) {
    let (lo, hi) = vis_bounds(r);
    let (lo, hi) = (at + lo / px_per_unit, at + hi / px_per_unit);
    ([lo.x, (lo.x + hi.x) / 2.0, hi.x], [lo.y, (lo.y + hi.y) / 2.0, hi.y])
}

/// Best offset that brings one of the features `feats` to a candidate `cands`
/// (within `tol`). Returns (offset, line).
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
    /// Handles mouse input in the player and draws the edit frame.
    /// `regions` from bottom to top; `px_per_unit` = screen pixels per project pixel.
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

        // What lies under the pointer? (handles of the selected element take precedence)
        let croppable = sel.is_some_and(|r| {
            self.project.find_clip(r.clip).is_some_and(|(_, c)| matches!(c.content, ClipContent::Media { .. }))
        });
        let hit_handle = move |p: Pos2| sel.and_then(|r| handle_at(&r, croppable, p));
        let delete_btn = sel.map(|r| r.vis_corner(-1.0, -1.0));

        if let Some(p) = resp.hover_pos() {
            if let Some(m) = hit_handle(p) {
                ui.ctx().set_cursor_icon(match m {
                    Mode::Rotate { .. } => egui::CursorIcon::Alias,
                    Mode::Crop { edge: 0 | 1, .. } => egui::CursorIcon::ResizeHorizontal,
                    Mode::Crop { .. } => egui::CursorIcon::ResizeVertical,
                    _ => egui::CursorIcon::ResizeNwSe,
                });
            } else if regions.iter().any(|r| r.contains(p)) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
            }
        }

        // Click: ✕ deletes, otherwise select the topmost element or clear the selection
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
                Mode::Crop { .. } => reg.map(|r| r.tr).unwrap_or_default(),
            };
            self.gizmo.guides_x.clear();
            self.gizmo.guides_y.clear();
            let (pw, ph) = (canvas.width() / px_per_unit, canvas.height() / px_per_unit);
            let free = ui.input(|i| i.modifiers.alt); // Alt = without snapping
            let tol = SNAP_PX / px_per_unit;
            match mode {
                Mode::Move { press, start } => {
                    let d = (p - press) / px_per_unit;
                    t.x = start.x + d.x;
                    t.y = start.y + d.y;
                    if let (Some(r), false) = (reg, free) {
                        // edges and center to the frame edge, center, thirds and other elements
                        let (fx, fy) = snap_feats(&r, vec2(t.x, t.y), px_per_unit);
                        let mut cx = vec![-pw / 2.0, -pw / 6.0, 0.0, pw / 6.0, pw / 2.0];
                        let mut cy = vec![-ph / 2.0, -ph / 6.0, 0.0, ph / 6.0, ph / 2.0];
                        for o in regions.iter().filter(|o| o.clip != clip) {
                            let (ox, oy) = snap_feats(o, (o.center - canvas.center()) / px_per_unit, px_per_unit);
                            cx.extend(ox);
                            cy.extend(oy);
                        }
                        if let Some((dx, line)) = best_snap(&fx, &cx, tol) {
                            t.x += dx;
                            self.gizmo.guides_x.push(line);
                        }
                        if let Some((dy, line)) = best_snap(&fy, &cy, tol) {
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
                            // snap edges to the frame edge, also at 100 %
                            let cur = self.gizmo.live.map(|l| l.1.scale).unwrap_or(start.scale).max(0.001);
                            let (lo, hi) = vis_bounds(&r);
                            let (lo, hi) = (lo / px_per_unit / cur, hi / px_per_unit / cur);
                            let reach = (-lo).max(hi).max_elem();
                            let raw = t.scale;
                            let mut best: Option<(f32, Option<(bool, f32)>)> = None;
                            let mut consider = |sc: f32, line: Option<(bool, f32)>| {
                                let near = ((sc - raw) * reach * px_per_unit).abs() <= SNAP_PX;
                                if sc > 0.02 && near && best.is_none_or(|(b, _)| (sc - raw).abs() < (b - raw).abs()) {
                                    best = Some((sc, line));
                                }
                            };
                            if hi.x > 0.0 {
                                consider((pw / 2.0 - t.x) / hi.x, Some((true, pw / 2.0)));
                            }
                            if lo.x < 0.0 {
                                consider((pw / 2.0 + t.x) / -lo.x, Some((true, -pw / 2.0)));
                            }
                            if hi.y > 0.0 {
                                consider((ph / 2.0 - t.y) / hi.y, Some((false, ph / 2.0)));
                            }
                            if lo.y < 0.0 {
                                consider((ph / 2.0 + t.y) / -lo.y, Some((false, -ph / 2.0)));
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
                Mode::Crop { edge, start } => {
                    if let Some(r) = reg {
                        let k = crop_drag(&r, edge, start, p);
                        let v = [k.left, k.right, k.top, k.bottom][edge];
                        badge = Some(format!("{:.0} %", v * 100.0));
                        self.gizmo.live_crop = Some((clip, k));
                    }
                }
            }
            if !matches!(mode, Mode::Crop { .. }) {
                self.gizmo.live = Some((clip, t));
            }
            ui.ctx().request_repaint();
        }
        if resp.drag_stopped() {
            if let (Some((clip, _)), Some((_, t))) =
                (self.gizmo.drag.take(), self.gizmo.live.take())
            {
                self.commit_transform(clip, t);
            }
            if let Some((clip, crop)) = self.gizmo.live_crop.take() {
                self.commit_live();
                self.apply(EditOp::SetCrop { clip, crop });
            }
            self.gizmo.guides_x.clear();
            self.gizmo.guides_y.clear();
        }

        // When moving/scaling: subtle grid (thirds, center) and active guides
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

        // Frame + handles of the selected element: only while the pointer is in the player
        // (or dragging), otherwise fade out softly – the selection itself stays
        let show = theme::fade(
            ui.ctx(),
            egui::Id::new("gizmo-visible"),
            resp.contains_pointer() || self.gizmo.drag.is_some(),
            theme::MOTION,
        );
        let mut painter = painter;
        painter.multiply_opacity(show);
        if let Some(r) = sel.filter(|_| show > 0.01) {
            // frame and handles on the visible (cropped) area
            let pts = [
                r.vis_corner(-1.0, -1.0),
                r.vis_corner(1.0, -1.0),
                r.vis_corner(1.0, 1.0),
                r.vis_corner(-1.0, 1.0),
            ];
            let frame = Stroke::new(1.0_f32, Color32::from_white_alpha(230));
            for i in 0..4 {
                painter.line_segment([pts[i], pts[(i + 1) % 4]], frame);
            }
            for p in &pts[1..] {
                painter.circle(
                    *p,
                    4.5,
                    theme::ACCENT(),
                    Stroke::new(1.0_f32, theme::ACCENT_INK()),
                );
            }
            // Crop: the whole clip subtly and bars in the middle of the visible edges
            if croppable {
                if !r.crop.is_none() {
                    let full = [r.corner(-1.0, -1.0), r.corner(1.0, -1.0), r.corner(1.0, 1.0), r.corner(-1.0, 1.0)];
                    for i in 0..4 {
                        painter.line_segment([full[i], full[(i + 1) % 4]], Stroke::new(1.0_f32, Color32::from_white_alpha(60)));
                    }
                }
                for e in 0..4 {
                    let along = r.rot(if e < 2 { vec2(0.0, 7.0) } else { vec2(7.0, 0.0) });
                    let h = r.crop_handle(e);
                    painter.line_segment([h - along, h + along], Stroke::new(4.0_f32, theme::ACCENT()));
                }
            }
            // ✕ delete
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
                egui::FontId::proportional(9.0), // icon glyph in the small handle circle
                Color32::WHITE,
            );
            // Rotate handle
            let rh = r.rotate_handle();
            painter.line_segment(
                [r.crop_handle(3), rh],
                Stroke::new(1.0_f32, Color32::from_white_alpha(140)),
            );
            painter.circle(
                rh,
                8.0,
                theme::ACCENT(),
                Stroke::new(1.0_f32, theme::ACCENT_INK()),
            );
            painter.text(
                rh,
                egui::Align2::CENTER_CENTER,
                icon::ARROWS_CLOCKWISE,
                egui::FontId::proportional(10.0), // icon glyph in the rotate handle
                theme::ACCENT_INK(),
            );
            if let Some(b) = badge {
                let g = painter.layout_no_wrap(b, egui::FontId::proportional(theme::FS_UI), Color32::BLACK);
                let top = pts.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
                let br = Rect::from_center_size(
                    pos2((pts[0].x + pts[2].x) / 2.0, top - 16.0),
                    g.size() + vec2(12.0, 6.0),
                );
                painter.rect_filled(br, 9.0, Color32::from_white_alpha(235)); // pill: radius = half the height
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
            crop: Crop::default(),
        };
        // rotated by 90°: now tall and narrow
        assert!(r.contains(pos2(100.0, 145.0)));
        assert!(!r.contains(pos2(145.0, 100.0)));
        let c = r.corner(1.0, 1.0);
        assert!(
            (c.x - 90.0).abs() < 0.01 && (c.y - 150.0).abs() < 0.01,
            "{c:?}"
        );
    }

    fn region(center: Pos2, size: Vec2, angle: f32, crop: Crop) -> Region {
        Region { clip: ClipId::new(), center, size, angle, tr: Transform::default(), crop }
    }

    /// Split screen „Nebeneinander“: the right (upper) clip reaches invisibly into the left half.
    /// Hits only in the visible part, ✕ at the visible corner (on the stage).
    #[test]
    fn hits_and_handles_follow_the_crop() {
        let k = Crop { left: 0.25, right: 0.25, top: 0.0, bottom: 0.0 };
        let regions = [region(pos2(50.0, 50.0), vec2(200.0, 100.0), 0.0, k), region(pos2(150.0, 50.0), vec2(200.0, 100.0), 0.0, k)];
        let top = |p: Pos2| regions.iter().rev().position(|r| r.contains(p)).map(|i| 1 - i);
        assert_eq!(top(pos2(60.0, 50.0)), Some(0));
        assert_eq!(top(pos2(140.0, 50.0)), Some(1));
        let near = |a: Pos2, b: Pos2| a.distance(b) < 0.01;
        assert!(near(regions[0].vis_corner(-1.0, -1.0), pos2(0.0, 0.0)), "{:?}", regions[0].vis_corner(-1.0, -1.0));
        assert!(near(regions[1].vis_corner(1.0, 1.0), pos2(200.0, 100.0)));
        // rotated (90°) and twice as large, the left half cut away: only the lower half is visible
        let r = region(pos2(100.0, 100.0), vec2(400.0, 200.0), std::f32::consts::FRAC_PI_2, Crop { left: 0.5, ..Default::default() });
        assert!(r.contains(pos2(100.0, 250.0)) && r.contains(pos2(0.0, 150.0)));
        assert!(!r.contains(pos2(100.0, 50.0)) && !r.contains(pos2(100.0, 310.0)));
        // rotate handle below the visible bottom edge
        let r = region(pos2(100.0, 100.0), vec2(200.0, 100.0), 0.0, Crop { bottom: 0.5, ..Default::default() });
        assert!(near(r.rotate_handle(), pos2(100.0, 122.0)), "{:?}", r.rotate_handle());
    }

    /// Narrow visible cutout: every handle stays grabbable at its spot; scaling
    /// computes from the center of the whole clip.
    #[test]
    fn close_handles_pick_the_nearest() {
        let r = region(pos2(100.0, 100.0), vec2(200.0, 100.0), 0.0, Crop { left: 0.45, right: 0.45, top: 0.0, bottom: 0.0 });
        assert!(matches!(handle_at(&r, true, r.crop_handle(2)), Some(Mode::Crop { edge: 2, .. })));
        assert!(matches!(handle_at(&r, true, r.crop_handle(3)), Some(Mode::Crop { edge: 3, .. })));
        assert!(matches!(handle_at(&r, true, r.crop_handle(1)), Some(Mode::Crop { edge: 1, .. })));
        assert!(matches!(handle_at(&r, true, r.rotate_handle()), Some(Mode::Rotate { .. })));
        let c = r.vis_corner(1.0, -1.0);
        assert!(near_pos(c, pos2(110.0, 50.0)));
        match handle_at(&r, true, c) {
            Some(Mode::Scale { press_dist, .. }) => assert!((press_dist - c.distance(r.center)).abs() < 1e-3),
            _ => panic!("corner handle"),
        }
        assert!(matches!(handle_at(&r, false, r.crop_handle(2)), Some(Mode::Scale { .. })));
    }

    /// Snapping by the visible part: left 50 % cut away, visible left edge to the frame edge,
    /// visible center to the frame center; without a crop edges at ±half the width. Also rotated.
    #[test]
    fn move_snaps_to_the_visible_part() {
        let cands = [-200.0, 0.0, 200.0]; // frame 400 wide, 1 screen pixel = 1 project pixel
        let half = Crop { left: 0.5, ..Default::default() };
        let r = region(pos2(0.0, 0.0), vec2(200.0, 100.0), 0.0, half);
        // visible: 0..100 right of the clip center
        let (fx, fy) = snap_feats(&r, vec2(-198.0, 0.0), 1.0);
        assert_eq!(fx, [-198.0, -148.0, -98.0]);
        assert_eq!(fy, [-50.0, 0.0, 50.0]);
        assert_eq!(best_snap(&fx, &cands, 10.0), Some((-2.0, -200.0)));
        let (fx, _) = snap_feats(&r, vec2(-48.0, 0.0), 1.0);
        assert_eq!(best_snap(&fx, &cands, 10.0), Some((-2.0, 0.0)));
        // without a crop as before
        let full = region(pos2(0.0, 0.0), vec2(200.0, 100.0), 0.0, Crop::default());
        let (fx, fy) = snap_feats(&full, vec2(-98.0, 3.0), 1.0);
        assert_eq!(fx, [-198.0, -98.0, 2.0]);
        assert_eq!(fy, [-47.0, 3.0, 53.0]);
        // rotated by 90° and double screen size: the lower half is visible (y 0..200 px)
        let rot = region(pos2(0.0, 0.0), vec2(400.0, 200.0), std::f32::consts::FRAC_PI_2, half);
        let (fx, fy) = snap_feats(&rot, vec2(0.0, 0.0), 2.0);
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
        assert!(close(fx, [-50.0, 0.0, 50.0]), "{fx:?}");
        assert!(close(fy, [0.0, 50.0, 100.0]), "{fy:?}");
    }

    fn near_pos(a: Pos2, b: Pos2) -> bool {
        a.distance(b) < 0.01
    }

    /// Crop handles: lie in the middle of the visible edges, dragging follows the pointer even
    /// when rotated, the opposite side leaves 2 %.
    #[test]
    fn crop_handles_follow_the_pointer() {
        let mut r = Region {
            clip: ClipId::new(),
            center: pos2(100.0, 100.0),
            size: vec2(200.0, 100.0),
            angle: 0.0,
            tr: Transform::default(),
            crop: Crop { left: 0.25, ..Default::default() },
        };
        let near = |a: Pos2, b: Pos2| a.distance(b) < 0.01;
        assert!(near(r.crop_handle(0), pos2(50.0, 100.0)), "{:?}", r.crop_handle(0));
        assert!(near(r.crop_handle(2), pos2(125.0, 50.0)), "{:?}", r.crop_handle(2));
        // drag the left to 40 %, the right beyond the left edge: 2 % remain
        let k = crop_drag(&r, 0, r.crop, pos2(80.0, 130.0));
        assert!((k.left - 0.4).abs() < 1e-5, "{k:?}");
        let k = crop_drag(&r, 1, k, pos2(0.0, 100.0));
        assert!((k.right - 0.58).abs() < 1e-5, "{k:?}");
        // rotated by 90°: the top edge now lies on the right
        r.angle = std::f32::consts::FRAC_PI_2;
        let k = crop_drag(&r, 2, Crop::default(), pos2(125.0, 100.0));
        assert!((k.top - 0.25).abs() < 1e-5, "{k:?}");
    }
}

// ---------------------------------------------------------------- player zoom

/// Zoom levels relative to „Ganzes Bild“
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 16.0;

impl App {
    /// Ctrl+mouse wheel: zoom around the mouse pointer; mouse wheel/middle button: pan the view;
    /// Alt+Shift+Plus/Minus/Z.
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
        // never push the canvas fully out of view
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
        // the point under the pointer stays put
        let c = area.center() + self.player_pan;
        self.player_pan += (anchor - c) * (1.0 - new / old);
        self.player_zoom = new;
    }

    /// Chip in the player bar with a popover (−, slider, +, Ganzes Bild, 1:1).
    pub fn player_zoom_chip(&mut self, ui: &mut egui::Ui, compact: bool) {
        let pct = format!("{:.0} %", self.player_zoom * 100.0);
        let label = if compact {
            icon::MAGNIFYING_GLASS.to_string()
        } else if (self.player_zoom - 1.0).abs() < 1e-3 {
            format!("{} Ganzes Bild", icon::MAGNIFYING_GLASS)
        } else {
            format!("{} {pct}", icon::MAGNIFYING_GLASS)
        };
        let resp = theme::chip(ui, &label).on_hover_text(format!(
            "Player-Zoom {pct} (Strg+Mausrad, Alt+Umschalt+Z: ganzes Bild)"
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
                    if ui.add(theme::secondary("Ganzes Bild")).clicked() {
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
