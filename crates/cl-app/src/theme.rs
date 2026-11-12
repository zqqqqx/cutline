//! Design-System (Stand Phase 2):
//! - Monochrom: fast schwarze Flächen, Weiß als einzige Akzentfarbe (aktive Zustände,
//!   Primäraktion, Auswahl). Farbe kommt nur aus dem Inhalt (Clips, Vorschauen).
//! - Schwebende Karten mit 6-px-Radius, 4 px Abstand, ein Icon-Stil (Phosphor, Linien)
//! - 8-px-Raster, feste Label-Spalte im Inspektor, gedämpfte Sekundärtexte
//! - Minimalistische Regler (2-px-Schiene, kleiner Knopf), Schalter statt Häkchen

use cl_core::TrackKind;
use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, Pos2, Rect, RichText, Stroke};

pub use egui_phosphor::regular as icon;

pub const BG_APP: Color32 = Color32::from_rgb(0x0C, 0x0C, 0x0E);
pub const BG_CARD: Color32 = Color32::from_rgb(0x17, 0x17, 0x1A);
pub const BG_RAISED: Color32 = Color32::from_rgb(0x22, 0x22, 0x26);
pub const BG_HOVER: Color32 = Color32::from_rgb(0x2B, 0x2B, 0x30);
pub const BG_INPUT: Color32 = Color32::from_rgb(0x10, 0x10, 0x12);
pub const BORDER: Color32 = Color32::from_rgb(0x29, 0x29, 0x2E);
pub const TEXT: Color32 = Color32::from_rgb(0xEC, 0xEC, 0xEE);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8C, 0x8C, 0x93);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x58, 0x58, 0x5F);
/// Akzent: Weiß
pub const ACCENT: Color32 = Color32::from_rgb(0xF4, 0xF4, 0xF6);
/// Schrift auf Akzentflächen
pub const ACCENT_INK: Color32 = Color32::from_rgb(0x10, 0x10, 0x12);
pub const DANGER: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);
pub const SNAP: Color32 = Color32::from_rgb(0xFF, 0xD8, 0x4D);

pub const RADIUS: u8 = 6;
pub const GAP: f32 = 4.0;
pub const LABEL_W: f32 = 84.0;
pub const TITLE_H: f32 = 36.0;

pub fn clip_color(kind: TrackKind) -> Color32 {
    match kind {
        TrackKind::Video => Color32::from_rgb(0x24, 0x4C, 0x55),
        TrackKind::Audio => Color32::from_rgb(0x25, 0x4D, 0x3C),
        TrackKind::Text => Color32::from_rgb(0x6E, 0x44, 0x36),
        TrackKind::Sticker => Color32::from_rgb(0x6E, 0x58, 0x2C),
        TrackKind::Effect => Color32::from_rgb(0x4A, 0x3F, 0x78),
        TrackKind::Filter => Color32::from_rgb(0x36, 0x44, 0x72),
    }
}

pub fn track_icon(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => icon::FILM_STRIP,
        TrackKind::Audio => icon::MUSIC_NOTES,
        TrackKind::Text => icon::TEXT_T,
        TrackKind::Sticker => icon::STICKER,
        TrackKind::Effect => icon::SPARKLE,
        TrackKind::Filter => icon::CIRCLE_HALF,
    }
}

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG_APP;
    v.window_fill = BG_CARD;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.extreme_bg_color = BG_INPUT;
    v.faint_bg_color = BG_RAISED;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = Color32::from_white_alpha(46);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.hyperlink_color = ACCENT;
    v.text_cursor.stroke = Stroke::new(1.5_f32, ACCENT);
    let r = CornerRadius::same(5);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = r;
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_DIM);
    v.widgets.inactive.weak_bg_fill = BG_RAISED;
    v.widgets.inactive.bg_fill = BG_RAISED;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.hovered.weak_bg_fill = BG_HOVER;
    v.widgets.hovered.bg_fill = BG_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.active.weak_bg_fill = Color32::from_white_alpha(40);
    v.widgets.active.bg_fill = Color32::from_white_alpha(40);
    v.widgets.active.bg_stroke = Stroke::NONE;
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, ACCENT);
    v.widgets.open.weak_bg_fill = BG_HOVER;
    v.window_shadow = egui::epaint::Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(150) };
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(130) };
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(6.0, 4.0);
        s.spacing.button_padding = vec2(8.0, 3.0);
        s.spacing.interact_size = vec2(22.0, 20.0);
        s.spacing.slider_width = 120.0;
        s.spacing.slider_rail_height = 2.0;
        s.spacing.combo_width = 120.0;
        s.spacing.icon_width = 12.0;
        s.spacing.icon_width_inner = 6.0;
        s.spacing.menu_margin = egui::Margin::same(5);
        s.spacing.scroll = egui::style::ScrollStyle::thin();
        s.visuals.handle_shape = egui::style::HandleShape::Circle;
        s.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(12.0));
        s.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(12.0));
        s.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(10.5));
        s.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(15.0));
        s.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(11.0));
    });
}

/// Karte (abgerundet, schwebend). `margin` = Innenabstand.
pub fn card(margin: i8) -> egui::Frame {
    egui::Frame::default().fill(BG_CARD).corner_radius(RADIUS).inner_margin(margin).outer_margin(egui::Margin::same((GAP / 2.0) as i8))
}

/// Kopfzeile einer Karte: Titel links, Aktionen rechts, dünne Linie darunter.
pub fn card_header(ui: &mut egui::Ui, title: &str, actions: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        ui.label(RichText::new(crate::tr(title)).strong().size(12.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), actions);
    });
    let r = ui.max_rect();
    let y = ui.cursor().top();
    ui.painter().line_segment([pos2(r.left() - 12.0, y), pos2(r.right() + 12.0, y)], Stroke::new(1.0_f32, BORDER));
    ui.add_space(6.0);
}

/// Primärknopf (weiße Fläche, dunkle Schrift).
pub fn primary(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(crate::tr(text.into())).size(11.5).color(ACCENT_INK).strong()).fill(ACCENT).corner_radius(5).min_size(vec2(0.0, 24.0))
}

/// Sekundärknopf (dezente Fläche).
pub fn secondary(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(crate::tr(text.into())).size(11.5)).fill(BG_RAISED).corner_radius(5).min_size(vec2(0.0, 24.0))
}

/// Flacher Knopf ohne Fläche (wird beim Überfahren hell).
pub fn ghost(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(crate::tr(text.into())).size(11.5)).frame(false).min_size(vec2(0.0, 22.0))
}

/// Icon-Knopf ohne Rahmen, mit Tooltip. `active` hebt ihn hervor.
pub fn icon_button(ui: &mut egui::Ui, glyph: &str, tip: impl Into<String>, enabled: bool, active: bool) -> egui::Response {
    let color = if !enabled { TEXT_FAINT } else if active { ACCENT } else { TEXT_DIM };
    let btn = egui::Button::new(RichText::new(glyph).size(14.0).color(color))
        .fill(if active { Color32::from_white_alpha(22) } else { Color32::TRANSPARENT })
        .corner_radius(5)
        .min_size(vec2(24.0, 22.0));
    let r = ui.add_enabled(enabled, btn);
    let tip = crate::tr(tip.into());
    if tip.is_empty() {
        r
    } else {
        r.on_hover_text(tip)
    }
}

/// Kleiner umrandeter Chip (Player-Leiste).
pub fn chip(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(crate::tr(text)).size(10.5).color(TEXT_DIM)).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0_f32, BORDER)).corner_radius(5))
}

/// Kategorie-Chip (aktiv = weiß gefüllt).
pub fn cat_chip(ui: &mut egui::Ui, text: &str, active: bool) -> egui::Response {
    let (fg, bg) = if active { (ACCENT_INK, ACCENT) } else { (TEXT_DIM, BG_RAISED) };
    ui.add(egui::Button::new(RichText::new(crate::tr(text)).size(10.5).color(fg)).fill(bg).corner_radius(10).min_size(vec2(0.0, 20.0)))
}

/// Zeile mit fester Label-Spalte (Inspektor).
pub fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        ui.add_sized([LABEL_W, 18.0], egui::Label::new(RichText::new(crate::tr(label)).size(11.5).color(TEXT_DIM)).truncate());
        add(ui)
    })
    .inner
}

/// Abschnittstitel im Inspektor mit Trennlinie davor.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    let r = ui.available_rect_before_wrap();
    ui.painter().line_segment([pos2(r.left(), r.top()), pos2(r.right(), r.top())], Stroke::new(1.0_f32, BORDER));
    ui.add_space(9.0);
    ui.label(RichText::new(crate::tr(title)).size(11.0).color(TEXT).strong());
    ui.add_space(3.0);
}

/// Leerer Zustand: Icon, Titel, Erklärung.
pub fn empty_state(ui: &mut egui::Ui, glyph: &str, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(26.0);
        ui.label(RichText::new(glyph).size(26.0).color(TEXT_FAINT));
        ui.add_space(6.0);
        ui.label(RichText::new(crate::tr(title)).strong());
        ui.label(RichText::new(crate::tr(hint)).small().color(TEXT_DIM));
        ui.add_space(12.0);
    });
}

// ---------------------------------------------------------------- Logo

/// Logo als SVG (für Fenster-Icon): weiße Kachel, drei Clip-Balken, ein Schnitt,
/// rechts davon leicht versetzt.
pub const LOGO_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="15" fill="#F4F4F6"/><g fill="#111113"><rect x="11" y="16" width="20" height="8" rx="4"/><rect x="11" y="28" width="13" height="8" rx="4"/><rect x="11" y="40" width="20" height="8" rx="4"/><rect x="37" y="13" width="16" height="8" rx="4"/><rect x="30" y="25" width="23" height="8" rx="4"/><rect x="37" y="37" width="12" height="8" rx="4"/></g><rect x="32.5" y="8" width="2" height="48" rx="1" fill="#111113"/></svg>"##;

/// Logo direkt zeichnen (gleiche Geometrie wie [`LOGO_SVG`]).
pub fn logo(painter: &egui::Painter, rect: Rect) {
    let s = rect.width().min(rect.height()) / 64.0;
    let o = rect.center() - vec2(32.0 * s, 32.0 * s);
    let r = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(o + vec2(x * s, y * s), vec2(w * s, h * s));
    painter.rect_filled(r(0.0, 0.0, 64.0, 64.0), 15.0 * s, ACCENT);
    let ink = ACCENT_INK;
    for (x, y, w) in [(11.0, 16.0, 20.0), (11.0, 28.0, 13.0), (11.0, 40.0, 20.0), (37.0, 13.0, 16.0), (30.0, 25.0, 23.0), (37.0, 37.0, 12.0)] {
        painter.rect_filled(r(x, y, w, 8.0), 4.0 * s, ink);
    }
    painter.rect_filled(r(32.5, 8.0, 2.0, 48.0), 1.0 * s, ink);
}

/// Logo + Wortmarke.
pub fn wordmark(ui: &mut egui::Ui, size: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    logo(ui.painter(), r);
    ui.label(RichText::new("Cutline").strong().size(size * 0.62).color(TEXT));
}

/// Raute (Keyframe) zeichnen.
pub fn diamond(painter: &egui::Painter, c: Pos2, r: f32, fill: Option<Color32>, stroke: Stroke) {
    let pts = vec![pos2(c.x, c.y - r), pos2(c.x + r, c.y), pos2(c.x, c.y + r), pos2(c.x - r, c.y)];
    painter.add(egui::Shape::convex_polygon(pts, fill.unwrap_or(Color32::TRANSPARENT), stroke));
}

/// Fenster-Icon aus dem Logo.
pub fn window_icon() -> Option<egui::IconData> {
    let size = 64;
    let premul = cl_fx::stickers::render_svg(LOGO_SVG, size)?;
    let mut rgba = premul;
    for px in rgba.chunks_mut(4) {
        let a = px[3] as f32 / 255.0;
        if a > 0.0 {
            for c in &mut px[..3] {
                *c = ((*c as f32 / a).min(255.0)) as u8;
            }
        }
    }
    Some(egui::IconData { rgba, width: size, height: size })
}
