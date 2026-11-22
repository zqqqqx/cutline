//! Own minimalist controls: slider (2 px track, small knob, value
//! directly editable on the right), toggle, keyframe diamond, segmented control and tiles.

use std::collections::HashMap;
use std::ops::RangeInclusive;

use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText, Sense, Stroke};

use crate::theme::{self, icon};

/// Result of a slider: `changed` = the value moved (preview),
/// `commit` = gesture finished (write to the history).
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
    /// Display factor (e.g. 100 for percent)
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
        // Value field as wide as the longest value („-60.0 dB“); otherwise it grows and pushes the
        // keyframe diamond out of the inspector on the right (looked like a „<“)
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
        // Held knob: if the layout above changes while dragging (e.g. a reset button
        // appears), the slider gets a new ID and egui loses the drag.
        // So the slider remembers the press point and follows the mouse until it is released.
        let grab = egui::Id::new("slider-grab");
        let origin = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started() || resp.is_pointer_button_down_on() {
            if let Some(o) = origin {
                ui.data_mut(|d| d.insert_temp(grab, (o, ui.layer_id())));
            }
        }
        let held = ui.data(|d| d.get_temp::<(egui::Pos2, egui::LayerId)>(grab));
        // only the slider under the press point, in the same layer (no hidden slider
        // behind it) and only for exactly this mouse press
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
            // released without the slider still lying under the press point
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
        // Fill from 0 (bipolar) or from the start
        let zero_n = if a < 0.0 && b > 0.0 {
            self.to_norm(0.0)
        } else {
            0.0
        };
        // values outside the range (e.g. old +20 dB) stay until they are changed
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
        // Value: directly editable
        let mut shown = *self.value * self.display;
        let before = shown;
        let decimals = self.decimals;
        let dv = ui.add_sized(
            [value_w, 18.0],
            egui::DragValue::new(&mut shown)
                .range((a * self.display)..=(b * self.display))
                .clamp_existing_to_range(false)
                .fixed_decimals(self.decimals)
                .custom_formatter(|v, _| crate::theme::dec(v, decimals))
                .custom_parser(crate::theme::parse_dec)
                .suffix(self.suffix)
                .speed(((b - a) * self.display / 300.0).max(0.01)),
        );
        let start = dv.id.with("vor");
        if dv.gained_focus() {
            ui.data_mut(|d| d.insert_temp(start, before));
        }
        if dv.changed() {
            *self.value = shown / self.display;
            out.changed = true;
            ui.data_mut(|d| d.insert_temp(dv.id, shown));
        }
        if dv.drag_stopped() || dv.lost_focus() {
            // Enter only reports the focus loss, the number already came while typing: it also applies to
            // callers that only take their value here (otherwise Enter discarded the input).
            // Esc discards: the value from the start of typing comes back.
            let (typed, began) = ui.data_mut(|d| (d.remove_temp::<f32>(dv.id), d.remove_temp::<f32>(start)));
            let undo = began.filter(|_| field_esc(ui.ctx()));
            if let Some(v) = last_typed(typed, dv.changed(), undo) {
                *self.value = v / self.display;
                out.changed = true;
            }
            out.commit = true;
        }
        out
    }
}

/// Value of a value field on leaving: on Esc the value from the start of typing (`undo`), otherwise the
/// last typed number if it was not reported in this frame anyway.
fn last_typed(typed: Option<f32>, changed_now: bool, undo: Option<f32>) -> Option<f32> {
    undo.or(typed.filter(|_| !changed_now))
}

/// The app assigned Esc to a field in this frame and consumed the key for it (so that the
/// „Auswahl aufheben" shortcut does not fire) – otherwise the field itself would no longer see it.
pub fn mark_field_esc(ctx: &egui::Context) {
    let now = ctx.cumulative_pass_nr(); // before data_mut: both lock the context
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("field_esc"), now));
}

pub(crate) fn field_esc(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.key_pressed(egui::Key::Escape))
        || ctx.data(|d| d.get_temp::<u64>(egui::Id::new("field_esc"))) == Some(ctx.cumulative_pass_nr())
}

/// Toggle (pill). Returns `true` if it was toggled.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 16.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let t = theme::fade(ui.ctx(), resp.id, *on, theme::MOTION);
    let p = ui.painter();
    let bg = if *on { theme::ACCENT() } else { theme::BG_HOVER() };
    // pill: radius = half the height (16 px), fully round on purpose
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

/// Toggle with text to its right, instead of a checkbox (whose empty box is hardly visible
/// on the cards). The text toggles too; `changed()` as with `ui.checkbox`.
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

/// Row: label on the left, toggle on the right.
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
    /// The property has no keyframes
    None,
    /// Keyframes present, but not at the playhead
    Elsewhere,
    /// Keyframe exactly at the playhead
    Here,
}

/// Keyframe diamond; a click sets or removes the keyframe at the playhead.
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

/// Segmented control (equally wide fields). Returns `true` on change.
pub fn segmented(ui: &mut egui::Ui, items: &[&str], sel: &mut usize) -> bool {
    segmented_some(ui, items, sel, &[])
}

/// Like [`segmented`]; fields with `false` in `enabled` are grayed out and not selectable.
pub fn segmented_some(ui: &mut egui::Ui, items: &[&str], sel: &mut usize, enabled: &[bool]) -> bool {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    ui.painter().rect_filled(rect, theme::R_CARD, theme::BG_INPUT());
    let font = egui::FontId::proportional(theme::FS_UI);
    let texts: Vec<String> = items.iter().map(|it| crate::tr(*it)).collect();
    // Width per segment by text length; the remaining room spread evenly
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
    // The marker glides to the chosen field (state as field number: size changes do not glide)
    let at = theme::glide(ui.ctx(), ui.id().with(("seg-mark", items.len())), *sel as f32, theme::MOTION);
    if let (Some(a), Some(b)) = (rects.get(at.floor() as usize), rects.get((at.ceil() as usize).min(rects.len().saturating_sub(1)))) {
        let f = at.fract();
        let mark = Rect::from_min_max(a.min.lerp(b.min, f), a.max.lerp(b.max, f));
        ui.painter().rect_filled(mark, theme::R_CTRL, theme::BG_HOVER());
    }
    let mut changed = false;
    for (i, t) in texts.iter().enumerate() {
        let r = rects[i];
        let on = enabled.get(i).copied().unwrap_or(true);
        let resp = ui.interact(r, ui.id().with(("seg", i, items[i])), if on { Sense::click() } else { Sense::hover() });
        let active = *sel == i;
        if !active && resp.hovered() {
            ui.painter().rect_filled(r, theme::R_CTRL, theme::BG_RAISED());
        }
        ui.painter_at(r).text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            t,
            font.clone(),
            // active: neutral area + light text (no accent text)
            if active { theme::TEXT() } else if on { theme::TEXT_DIM() } else { theme::TEXT_FAINT() },
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

/// Columns and tile width of a grid: so many columns that each tile is at least `min_w` wide;
/// if a tile would then be wider than 1.5 × `min_w` (narrow bar, one giant tile), one more column,
/// as long as the tiles stay 0.8 × `min_w` wide.
pub fn grid_cols(avail: f32, min_w: f32, gap: f32) -> (usize, f32) {
    let width = |cols: usize| ((avail - gap * (cols as f32 - 1.0)) / cols as f32).floor();
    let cols = ((avail + gap) / (min_w + gap)).floor().max(1.0) as usize;
    if width(cols) > 1.5 * min_w && width(cols + 1) >= 0.8 * min_w {
        (cols + 1, width(cols + 1))
    } else {
        (cols, width(cols))
    }
}

/// Really usable width of a grid: at most up to the visible right edge (`clip_right`).
/// A too wide row before it (search bar, folder chips) stretches `available_width` beyond the edge,
/// then the last column would be cut off.
pub fn grid_avail(available: f32, left: f32, clip_right: f32) -> f32 {
    available.min(clip_right - left).max(1.0)
}

/// Tile grid: computes the columns and calls `tile` for each visible element with its rect.
/// Rows outside the visible area only take up their space (packs with hundreds of tiles).
pub fn grid<T>(
    ui: &mut egui::Ui,
    items: &[T],
    min_w: f32,
    aspect: f32,
    label_h: f32,
    mut tile: impl FnMut(&mut egui::Ui, Rect, &T),
) {
    let gap = 8.0;
    // tiles draw 2 px beyond their edge (hover/selection): subtract that much from the visible edge
    let avail = grid_avail(ui.available_width(), ui.cursor().left(), ui.clip_rect().right() - 2.0);
    let (cols, w) = grid_cols(avail, min_w, gap);
    let h = (w / aspect).round();
    let order = ui
        .data_mut(|d| d.remove_temp::<GridOrder>(grid_order_id()))
        .filter(|o| o.keys.len() == items.len());
    let Some(order) = order else {
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
        return;
    };
    // With an order: first allocate all rows, then each tile at its place (gliding)
    let rows: Vec<Rect> = items
        .chunks(cols)
        .map(|_| {
            let (row, _) = ui.allocate_exact_size(vec2(avail, h + label_h), Sense::hover());
            ui.add_space(gap - ui.spacing().item_spacing.y);
            row
        })
        .collect();
    let Some(first) = rows.first().copied() else {
        return;
    };
    let cell = vec2(w + gap, rows.get(1).map_or(h + label_h + gap, |r| r.top() - first.top()));
    let slot_pos = |s: usize| vec2((s % cols) as f32 * cell.x, (s / cols) as f32 * cell.y);
    // Dragging in edit mode: the others move aside, the dragged one lies faded in the gap
    let held = egui::DragAndDrop::payload::<ReorderPayload>(ui.ctx());
    let from = held.and_then(|p| order.keys[..order.movable].iter().position(|k| *k == p.0)).filter(|_| order.editing);
    let area = Rect::from_min_max(first.min, rows.last().map_or(first.max, |r| r.max));
    let pointer = ui.input(|i| i.pointer.latest_pos()).filter(|p| area.expand(gap).contains(*p));
    let slots = match (from, pointer) {
        (Some(from), Some(p)) => {
            let to = slot_at(p - first.min, cols, cell, order.movable);
            let before = gap_before(&order.keys[..order.movable], from, to);
            ui.data_mut(|d| d.insert_temp(grid_drop_id(), GridDrop(order.keys[from].clone(), before)));
            gap_slots(items.len(), from, to)
        }
        _ => (0..items.len()).collect(),
    };
    let now = ui.input(|i| i.time);
    let store = ui.id().with("grid-glide");
    let mut glides = ui.data_mut(|d| d.get_temp::<GridGlides>(store)).unwrap_or_default();
    // another column count (width changed): rewrap at once, do not glide
    let fresh = glides.cols != cols;
    let mut next = GridGlides { cols, at: HashMap::with_capacity(items.len()) };
    let mut moving = false;
    for (i, it) in items.iter().enumerate() {
        let to = slot_pos(slots[i]);
        let g = match glides.at.remove(&order.keys[i]) {
            Some(g) if !fresh && g.to == to => g,
            Some(g) if !fresh => SlotGlide { from: g.at(now), to, t0: now },
            _ => SlotGlide { from: to, to, t0: now },
        };
        let at = g.at(now);
        moving |= at != to;
        next.at.insert(order.keys[i].clone(), g);
        let r = Rect::from_min_size(first.min + at, vec2(w, h + label_h));
        if ui.is_rect_visible(r) {
            tile(ui, r, it);
        }
    }
    if moving {
        ui.ctx().request_repaint();
    }
    ui.data_mut(|d| d.insert_temp(store, next));
}

/// For the next [`grid`]: key per entry (in display order), of which the first
/// `movable` can be moved; `editing` = edit mode (dragging makes room). With keys,
/// tiles glide to their new place when the order changes (drop, pin).
#[derive(Clone, Default)]
pub struct GridOrder {
    pub keys: std::sync::Arc<Vec<String>>,
    pub movable: usize,
    pub editing: bool,
}

pub fn grid_order_id() -> egui::Id {
    egui::Id::new("grid-order")
}

/// Payload when reordering (key of the entry); the timeline does not accept it.
pub struct ReorderPayload(pub String);

/// Target when dragging, set by [`grid`]: the dragged key lands before this one (or at the end)
#[derive(Clone, Default)]
pub struct GridDrop(pub String, pub Option<String>);

pub fn grid_drop_id() -> egui::Id {
    egui::Id::new("grid-drop")
}

/// Gliding of a tile between two places (relative to the first tile)
#[derive(Clone, Copy, Debug, PartialEq)]
struct SlotGlide {
    from: egui::Vec2,
    to: egui::Vec2,
    t0: f64,
}

impl SlotGlide {
    fn at(&self, now: f64) -> egui::Vec2 {
        let age = (now - self.t0) as f32;
        let secs = theme::anim(theme::MOTION);
        vec2(theme::glide_at(self.from.x, self.to.x, age, secs), theme::glide_at(self.from.y, self.to.y, age, secs))
    }
}

#[derive(Clone, Default)]
struct GridGlides {
    cols: usize,
    at: HashMap<String, SlotGlide>,
}

/// Display place per entry while entry `from` is dragged to place `to`: the others move
/// together and leave the gap there (in which the dragged entry lies).
pub fn gap_slots(n: usize, from: usize, to: usize) -> Vec<usize> {
    (0..n)
        .map(|i| {
            if i == from {
                return to;
            }
            let j = if i > from { i - 1 } else { i };
            if j >= to {
                j + 1
            } else {
                j
            }
        })
        .collect()
}

/// Place in the grid under the pointer (`p` relative to the first tile), clamped to `0..n`
pub fn slot_at(p: egui::Vec2, cols: usize, cell: egui::Vec2, n: usize) -> usize {
    let col = ((p.x / cell.x).floor().max(0.0) as usize).min(cols.max(1) - 1);
    let row = (p.y / cell.y).floor().max(0.0) as usize;
    (row * cols + col).min(n.saturating_sub(1))
}

/// Drop: before which key does `keys[from]` land at place `to` (`None` = at the end)?
pub fn gap_before(keys: &[String], from: usize, to: usize) -> Option<String> {
    let mut v: Vec<&String> = keys.iter().collect();
    let k = v.remove(from);
    v.insert(to.min(v.len()), k);
    v.get(to + 1).map(|s| s.to_string())
}

/// Does this click event of a tile insert? `clicks`: 0 = no click, otherwise 1/2/3 (double/
/// triple click). `plus_only`: a click on the area does not insert (e.g. sounds: plays), only
/// „+“ or a double-click. A double-click inserts only ONCE: its first click already acts.
pub fn adds(clicks: u8, on_plus: bool, plus_only: bool) -> bool {
    match clicks {
        1 => !plus_only || on_plus,
        // second click: only insert if the first did not already
        2 => plus_only && !on_plus,
        _ => false,
    }
}

/// [`adds`] for a response.
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

/// Default tile: area + name below; draws hover/selection. `draw` paints the content.
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

/// Like [`tile`], with a second, dimmed tooltip line `sub` (e.g. folder) below the name –
/// both in ONE tooltip.
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
        // „+“ for adding directly (not while reordering)
        let plus = Rect::from_min_size(area.right_bottom() - vec2(22.0, 22.0), vec2(18.0, 18.0));
        p.circle_filled(plus.center(), 9.0, theme::ACCENT());
        p.text(
            plus.center(),
            egui::Align2::CENTER_CENTER,
            icon::PLUS,
            // icon glyph in the 18 px circle
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
        // one line, too long names shortened with „…“; full name in the tooltip
        let full = theme::icon_arrows(&crate::tr(label));
        let font = egui::FontId::proportional(theme::FS_HINT);
        let shown = theme::elide(&full, r.width(), |t| theme::text_width(ui.ctx(), t, font.clone()));
        // name on top (shortened or with an extra line), extra dimmed below: one tooltip
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

/// Flag: draw tiles without „+“ (reordering in `library_ui`)
pub fn no_plus_id() -> egui::Id {
    egui::Id::new("tile-no-plus")
}

/// Did the tile click hit the „+“ at the bottom right?
pub fn hit_plus(resp: &egui::Response, r: Rect, label_h: f32) -> bool {
    let area_br = pos2(r.right(), r.bottom() - label_h);
    resp.interact_pointer_pos()
        .is_some_and(|p| p.x > area_br.x - 26.0 && p.y > area_br.y - 26.0 && p.y <= area_br.y)
}

/// Color field with a popup picker; if the Brand Kit has colors, they stand next to it as a row (a click
/// takes over the color, the opacity stays).
pub fn color_button(ui: &mut egui::Ui, rgba: &mut [u8; 4]) -> bool {
    let brand = crate::brand::colors(ui.ctx());
    if brand.is_empty() {
        return color_picker(ui, rgba);
    }
    ui.horizontal(|ui| {
        let mut changed = color_picker(ui, rgba);
        ui.spacing_mut().item_spacing.x = 3.0;
        for b in brand.iter() {
            let (r, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
            let p = ui.painter();
            p.rect_filled(r, theme::R_CTRL, Color32::from_rgb(b.rgb[0], b.rgb[1], b.rgb[2]));
            let stroke = if rgba[..3] == b.rgb {
                Stroke::new(1.5_f32, theme::ACCENT())
            } else if resp.hovered() {
                Stroke::new(1.0_f32, theme::TEXT_DIM())
            } else {
                Stroke::new(1.0_f32, theme::BORDER())
            };
            p.rect_stroke(r, theme::R_CTRL, stroke, egui::StrokeKind::Outside);
            let resp = if b.name.is_empty() { resp } else { resp.on_hover_text(&b.name) };
            if resp.clicked() {
                rgba[..3].copy_from_slice(&b.rgb);
                changed = true;
            }
        }
        changed
    })
    .inner
}

fn color_picker(ui: &mut egui::Ui, rgba: &mut [u8; 4]) -> bool {
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

// ------------------------------------------------------------ loading states

/// Running line: position of the moving piece (start, end as fraction 0…1) at time `t`.
/// Without animations the whole line stands still.
pub fn busy_segment(t: f64, moving: bool) -> (f32, f32) {
    if !moving {
        return (0.0, 1.0);
    }
    const LEN: f32 = 0.3;
    // 1.4 s per pass, calm and even
    let x = (t / 1.4).fract() as f32 * (1.0 + LEN) - LEN;
    (x.max(0.0), (x + LEN).min(1.0))
}

/// Shimmer on a placeholder: center of the light band (fraction, may run over the edges)
/// `age` seconds after appearing; `None` = calm area (still too early, too long or without
/// animations – an image that never comes does not make the UI repaint forever).
pub fn shimmer_at(age: f64, moving: bool) -> Option<f32> {
    (moving && (theme::LOAD_DELAY..6.0).contains(&age))
        .then(|| (((age - theme::LOAD_DELAY) / 1.6).fract() as f32) * 1.6 - 0.3)
}

/// Progress line (known duration): thin, accent color, follows the value smoothly.
pub fn progress_line(p: &egui::Painter, id: egui::Id, rect: Rect, frac: f32) {
    let shown = theme::glide(p.ctx(), id.with("progress"), frac.clamp(0.0, 1.0), theme::MOTION_SLOW);
    let r = rect.height() / 2.0;
    p.rect_filled(rect, r, theme::overlay(26));
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * shown, rect.height()));
    p.rect_filled(fill, r, theme::ACCENT());
}

/// Running line (unknown duration): a calmly moving piece in accent color; without animations
/// a still, muted line.
pub fn busy_line(p: &egui::Painter, rect: Rect) {
    let moving = theme::animations_on();
    let r = rect.height() / 2.0;
    p.rect_filled(rect, r, theme::overlay(26));
    let (a, b) = busy_segment(p.ctx().input(|i| i.time), moving);
    let seg = Rect::from_x_y_ranges(egui::lerp(rect.x_range(), a)..=egui::lerp(rect.x_range(), b), rect.y_range());
    p.rect_filled(seg, r, if moving { theme::ACCENT() } else { theme::ACCENT().gamma_multiply(0.5) });
    let visible = rect.width() > 0.0 && p.clip_rect().intersects(rect);
    if let Some(gap) = line_repaint_gap(true, moving && visible) {
        p.ctx().request_repaint_after(gap);
    }
}

/// Time until the next frame for a running task: running line smooth (only while it is
/// visible and animated), progress changes slowly (~10 frames/s are enough).
pub fn line_repaint_gap(busy: bool, animating: bool) -> Option<std::time::Duration> {
    animating.then(|| std::time::Duration::from_millis(if busy { 16 } else { 100 }))
}

/// Placeholder at target size while content (image, waveform …) is missing: calm area with a very
/// subtle shimmer. `false` = has been waiting long (content probably will not come; the caller then shows
/// its substitute icon).
pub fn skeleton(p: &egui::Painter, id: egui::Id, rect: Rect, radius: f32) -> bool {
    let ctx = p.ctx();
    let id = id.with("skeleton");
    let (now, pass) = (ctx.input(|i| i.time), ctx.cumulative_pass_nr());
    let (since, last) = ctx.data(|d| d.get_temp::<(f64, u64)>(id)).unwrap_or((now, pass));
    // not drawn for a while: start over
    let since = if last + 1 < pass { now } else { since };
    ctx.data_mut(|d| d.insert_temp(id, (since, pass)));
    let age = now - since;
    p.rect_filled(rect, radius, theme::BG_RAISED());
    if let Some(x) = shimmer_at(age, theme::animations_on()) {
        let cx = rect.left() + x * rect.width();
        let band = Rect::from_center_size(egui::pos2(cx, rect.center().y), vec2(rect.width() * 0.35, rect.height()));
        // soft edges: three staggered stripes
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

/// Content of a tile that is just arriving: opacity 0…1. What is already there on opening shows
/// at once; what comes later fades in once (state per `id`).
pub fn reveal(ctx: &egui::Context, id: egui::Id, ready: bool) -> f32 {
    theme::fade(ctx, id.with("reveal"), ready, theme::MOTION_FAST)
}

/// Image of a tile: if still missing, a placeholder (after a long wait `glyph`); when it
/// arrives, it fades in above the placeholder. `tex`: (texture, image size, uv cutout) – the image
/// fills the area (edges cut off).
pub fn tile_image(p: &egui::Painter, id: egui::Id, a: Rect, tex: Option<(egui::TextureId, egui::Vec2, Rect)>, glyph: &str) {
    let k = reveal(p.ctx(), id, tex.is_some());
    let Some((tid, ts, src)) = tex else {
        if !skeleton(p, id, a, theme::R_CARD) && !glyph.is_empty() {
            // icon glyph as a placeholder
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

/// Line of a running task in `rect`: progress as soon as one is reported, otherwise a running line.
pub fn task_line(p: &egui::Painter, id: egui::Id, rect: Rect, progress: f32) {
    if progress > 0.0 {
        progress_line(p, id, rect, progress);
    } else {
        busy_line(p, rect);
    }
}

/// Row with a running line below an element (search field, list): always takes its height (no
/// jumping), shows the line only after 150 ms of waiting and for at least 300 ms.
pub fn busy_row(ui: &mut egui::Ui, id: egui::Id, busy: bool) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 2.0), Sense::hover());
    if theme::loading(ui.ctx(), id, busy) {
        busy_line(ui.painter(), r);
    }
}

#[cfg(test)]
mod tests {
    use super::{grid_avail, grid_cols, line_repaint_gap};
    use eframe::egui::vec2;

    #[test]
    fn task_lines_repaint_only_when_needed_and_progress_slowly() {
        let ms = |d: Option<std::time::Duration>| d.map(|d| d.as_millis());
        assert_eq!(ms(line_repaint_gap(true, true)), Some(16));
        assert_eq!(ms(line_repaint_gap(false, true)), Some(100));
        assert_eq!(line_repaint_gap(true, false), None);
    }

    #[test]
    fn a_double_click_inserts_only_once() {
        use super::adds;
        // events of a double-click: first a click, then the second (double) click
        let count = |on_plus, plus_only| [1u8, 2].iter().filter(|&&c| adds(c, on_plus, plus_only)).count();
        // a click inserts (effects, filters, transitions, texts, elements)
        assert_eq!(count(false, false), 1);
        assert_eq!(count(true, false), 1);
        // only „+“ or double-click (sounds, media): once each on „+“ and on the area
        assert_eq!(count(true, true), 1);
        assert_eq!(count(false, true), 1);
        // a single click on the area does not insert there, a triple click does not insert again
        assert!(!adds(1, false, true) && !adds(3, false, false) && !adds(0, true, false));
    }

    #[test]
    fn grid_stays_inside_visible_width() {
        // grid starts at x = 100, visible up to 400: 300 px, even if the row reports 320
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
        // narrow bar (text templates, ~232 px): two columns instead of one 232 px tile
        let (c, w) = grid_cols(232.0, 120.0, 8.0);
        assert_eq!(c, 2);
        assert!(w >= 0.8 * 120.0 && 2.0 * w + 8.0 <= 232.0);
        // normal widths stay as before
        assert_eq!(grid_cols(400.0, 120.0, 8.0).0, 3);
        assert_eq!(grid_cols(300.0, 70.0, 8.0).0, 3);
        // nothing fits: one column at full width
        assert_eq!(grid_cols(80.0, 120.0, 8.0), (1, 80.0));
        for avail in (60..900).step_by(7).map(|a| a as f32) {
            let (c, w) = grid_cols(avail, 100.0, 8.0);
            assert!(c as f32 * w + (c as f32 - 1.0) * 8.0 <= avail + 0.01, "{avail}");
        }
    }

    /// Measurement (by hand): UI frame time with 300 placeholder tiles (shimmer running)
    /// and a running line, incl. tessellation. `cargo test --release -p cl-app -- --ignored placeholder_frame_time --nocapture`
    #[test]
    #[ignore]
    fn placeholder_frame_time() {
        let ctx = eframe::egui::Context::default();
        let items: Vec<usize> = (0..300).collect();
        let mut times = Vec::new();
        for f in 0..120 {
            let input = eframe::egui::RawInput {
                screen_rect: Some(eframe::egui::Rect::from_min_size(Default::default(), vec2(1600.0, 1000.0))),
                time: Some(f as f64 / 60.0),
                ..Default::default()
            };
            let t0 = std::time::Instant::now();
            let out = ctx.run(input, |ctx| {
                eframe::egui::CentralPanel::default().show(ctx, |ui| {
                    super::busy_row(ui, eframe::egui::Id::new("b"), true);
                    super::grid(ui, &items, 84.0, 16.0 / 9.0, 16.0, |ui, r, i| {
                        super::tile(ui, eframe::egui::Id::new(("t", *i)), r, "Kachel", false, |p, a| {
                            super::tile_image(p, eframe::egui::Id::new(("t", *i)), a, None, "")
                        });
                    });
                });
            });
            let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
            if f >= 20 {
                times.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
        }
        times.sort_by(f64::total_cmp);
        let (med, max) = (times[times.len() / 2], times[times.len() - 1]);
        println!("Platzhalter-Raster 300 Kacheln: Median {med:.2} ms, Max {max:.2} ms je Bild");
        assert!(med < 8.0, "{med}");
    }

    #[test]
    fn dragging_makes_room_at_the_target() {
        use super::{gap_before, gap_slots, slot_at};
        // without movement (to its own place): everything stays
        assert_eq!(gap_slots(5, 2, 2), vec![0, 1, 2, 3, 4]);
        // entry 0 to place 3: 1..3 move forward, the dragged one lies in the gap
        assert_eq!(gap_slots(5, 0, 3), vec![3, 0, 1, 2, 4]);
        // entry 4 to the front: all before it move one place on
        assert_eq!(gap_slots(5, 4, 0), vec![1, 2, 3, 4, 0]);
        // every place taken exactly once
        for (from, to) in [(0, 4), (3, 1), (2, 0), (1, 1)] {
            let mut s = gap_slots(5, from, to);
            assert_eq!(s[from], to);
            s.sort();
            assert_eq!(s, vec![0, 1, 2, 3, 4]);
        }
        // dropping lands before the successor in the new order, at the end without a successor
        let k: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(gap_before(&k, 0, 2).as_deref(), Some("d"));
        assert_eq!(gap_before(&k, 3, 0).as_deref(), Some("a"));
        assert_eq!(gap_before(&k, 1, 3), None);
        assert_eq!(gap_before(&k, 2, 2).as_deref(), Some("d"), "to its own place");
        // pointer → place: columns, rows, edges and end clamped
        let cell = vec2(100.0, 80.0);
        assert_eq!(slot_at(vec2(150.0, 10.0), 3, cell, 10), 1);
        assert_eq!(slot_at(vec2(250.0, 90.0), 3, cell, 10), 5);
        assert_eq!(slot_at(vec2(-30.0, -5.0), 3, cell, 10), 0);
        assert_eq!(slot_at(vec2(900.0, 10.0), 3, cell, 10), 2, "right of the last column");
        assert_eq!(slot_at(vec2(50.0, 900.0), 3, cell, 4), 3, "after the last entry");
    }

    #[test]
    fn grid_tiles_glide_to_their_new_place() {
        use super::SlotGlide;
        let g = SlotGlide { from: vec2(0.0, 0.0), to: vec2(100.0, 80.0), t0: 1.0 };
        if crate::theme::animations_on() {
            assert_eq!(g.at(1.0), vec2(0.0, 0.0), "start");
            let mid = g.at(1.0 + crate::theme::MOTION as f64 / 2.0);
            assert!(mid.x > 50.0 && mid.x < 100.0 && mid.y > 40.0 && mid.y < 80.0, "middle (easing out): {mid:?}");
        }
        assert_eq!(g.at(1.0 + crate::theme::MOTION as f64), vec2(100.0, 80.0), "end");
    }

    /// Edit mode in the grid: dragging „a“ over the third tile makes the others
    /// move up (target comes from the grid), without dragging all stay in place.
    #[test]
    fn grid_with_order_places_tiles_by_drag_target() {
        use super::{grid, grid_drop_id, grid_order_id, GridDrop, GridOrder, ReorderPayload};
        let ctx = eframe::egui::Context::default();
        let keys: Vec<String> = (0..6).map(|i| format!("k{i}")).collect();
        let mut placed = Vec::new();
        let frame = |drag: bool, placed: &mut Vec<(String, eframe::egui::Pos2)>| {
            placed.clear();
            let input = eframe::egui::RawInput {
                screen_rect: Some(eframe::egui::Rect::from_min_size(Default::default(), vec2(400.0, 600.0))),
                // pointer over the third tile of the first row
                events: vec![eframe::egui::Event::PointerMoved(eframe::egui::pos2(300.0, 40.0))],
                ..Default::default()
            };
            if drag {
                eframe::egui::DragAndDrop::set_payload(&ctx, ReorderPayload("k0".into()));
            }
            let _ = ctx.run(input, |ctx| {
                eframe::egui::CentralPanel::default().show(ctx, |ui| {
                    let order = GridOrder { keys: std::sync::Arc::new(keys.clone()), movable: keys.len(), editing: true };
                    ui.data_mut(|d| d.insert_temp(grid_order_id(), order));
                    grid(ui, &keys, 100.0, 1.0, 0.0, |_, r, k| placed.push((k.clone(), r.min)));
                });
            });
        };
        frame(false, &mut placed);
        let home = placed.clone();
        assert!(ctx.data(|d| d.get_temp::<GridDrop>(grid_drop_id())).is_none());
        frame(true, &mut placed);
        let drop = ctx.data(|d| d.get_temp::<GridDrop>(grid_drop_id())).expect("target");
        assert_eq!(drop.0, "k0");
        let target = home.iter().position(|(_, p)| p.x <= 300.0 && p.x + 100.0 >= 300.0 && p.y <= 40.0).expect("place under the pointer");
        assert_eq!(drop.1, keys.get(target + 1).cloned(), "lands before the successor");
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
        // the first frame is egui's measuring pass, the second one counts
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
        // 300 px high, rows ~88 px: only the few visible rows, the rest only takes space
        assert!(!built.is_empty() && built.len() <= 30, "{}", built.len());
        assert_eq!(built[0], 0);
        assert!(height > 300.0 / 6.0 * 80.0, "content must stay as high as all rows: {height}");
    }

    /// Enter in a value field: the typed number applies on leaving, unless it already came in the same frame.
    #[test]
    fn enter_keeps_the_typed_number() {
        assert_eq!(super::last_typed(Some(1.5), false, None), Some(1.5));
        assert_eq!(super::last_typed(Some(1.5), true, None), None);
        assert_eq!(super::last_typed(None, false, None), None);
    }

    /// Esc in a value field: the value from the start of typing comes back – even if the app already
    /// consumed the key (then the field only sees it via the marker).
    #[test]
    fn esc_discards_the_typed_number() {
        assert_eq!(super::last_typed(Some(7.0), false, Some(100.0)), Some(100.0));
        assert_eq!(super::last_typed(None, false, Some(100.0)), Some(100.0));
        let ctx = super::egui::Context::default();
        assert!(!super::field_esc(&ctx));
        super::mark_field_esc(&ctx);
        assert!(super::field_esc(&ctx));
        let _ = ctx.run(Default::default(), |_| {});
        let _ = ctx.run(Default::default(), |ctx| assert!(!super::field_esc(ctx), "applies to one frame only"));
    }
}
