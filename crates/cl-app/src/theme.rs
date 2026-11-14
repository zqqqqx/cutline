//! Design-System (Stand Phase 2):
//! - Monochrom: fast schwarze Flächen, Weiß als einzige Akzentfarbe (aktive Zustände,
//!   Primäraktion, Auswahl). Farbe kommt nur aus dem Inhalt (Clips, Vorschauen).
//! - Schwebende Karten mit 6-px-Radius, 4 px Abstand, ein Icon-Stil (Phosphor, Linien)
//! - 8-px-Raster, feste Label-Spalte im Inspektor, gedämpfte Sekundärtexte
//! - Minimalistische Regler (2-px-Schiene, kleiner Knopf), Schalter statt Häkchen

use cl_core::TrackKind;
use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, Pos2, Rect, RichText, Stroke};

pub use egui_phosphor::regular as icon;

/// Farbpalette eines Themes. Die Oberfläche liest Farben zur Laufzeit (`theme::TEXT()` …),
/// damit ein Wechsel sofort überall greift.
pub struct Palette {
    pub name: &'static str,
    pub id: &'static str,
    pub dark: bool,
    pub bg_app: Color32,
    pub bg_card: Color32,
    pub bg_raised: Color32,
    pub bg_hover: Color32,
    pub bg_input: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    /// Akzent (aktive Zustände, Primäraktion, Auswahl)
    pub accent: Color32,
    /// Schrift auf Akzentflächen
    pub accent_ink: Color32,
}

const fn rgb(h: u32) -> Color32 {
    Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

pub const PALETTES: [Palette; 4] = [
    Palette { name: "Dunkel", id: "dark", dark: true, bg_app: rgb(0x0C0C0E), bg_card: rgb(0x17171A), bg_raised: rgb(0x222226), bg_hover: rgb(0x2B2B30), bg_input: rgb(0x101012), border: rgb(0x29292E), text: rgb(0xECECEE), text_dim: rgb(0x8C8C93), text_faint: rgb(0x58585F), accent: rgb(0xF4F4F6), accent_ink: rgb(0x101012) },
    Palette { name: "Dunkelgrau", id: "darkgray", dark: true, bg_app: rgb(0x1A1A1C), bg_card: rgb(0x252528), bg_raised: rgb(0x303034), bg_hover: rgb(0x3A3A3F), bg_input: rgb(0x1E1E21), border: rgb(0x37373C), text: rgb(0xEDEDEF), text_dim: rgb(0x9C9CA3), text_faint: rgb(0x68686F), accent: rgb(0xF4F4F6), accent_ink: rgb(0x141416) },
    Palette { name: "Grau", id: "gray", dark: true, bg_app: rgb(0x38383C), bg_card: rgb(0x444449), bg_raised: rgb(0x505055), bg_hover: rgb(0x5C5C62), bg_input: rgb(0x313135), border: rgb(0x58585E), text: rgb(0xF4F4F6), text_dim: rgb(0xB8B8BE), text_faint: rgb(0x8A8A91), accent: rgb(0xFFFFFF), accent_ink: rgb(0x202024) },
    Palette { name: "Hell", id: "light", dark: false, bg_app: rgb(0xE9E9EC), bg_card: rgb(0xFBFBFC), bg_raised: rgb(0xEFEFF2), bg_hover: rgb(0xE2E2E7), bg_input: rgb(0xF4F4F6), border: rgb(0xD8D8DE), text: rgb(0x18181B), text_dim: rgb(0x5A5A62), text_faint: rgb(0x96969E), accent: rgb(0x18181B), accent_ink: rgb(0xFFFFFF) },
];

static CURRENT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static ANIMATIONS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_animations(on: bool) {
    ANIMATIONS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Dauer einer kleinen Animation (0, wenn Animationen ausgeschaltet sind)
pub fn anim(secs: f32) -> f32 {
    if ANIMATIONS.load(std::sync::atomic::Ordering::Relaxed) {
        secs
    } else {
        0.0
    }
}

pub fn pal() -> &'static Palette {
    &PALETTES[CURRENT.load(std::sync::atomic::Ordering::Relaxed).min(PALETTES.len() - 1)]
}

/// Theme aus den Einstellungen („system“, „dark“, „darkgray“, „gray“, „light“). „system“ folgt
/// dem hellen/dunklen Modus des Betriebssystems.
pub fn select(ctx: &egui::Context, id: &str) {
    let id = if id == "system" {
        match ctx.system_theme() {
            Some(egui::Theme::Light) => "light",
            _ => "dark",
        }
    } else {
        id
    };
    let i = PALETTES.iter().position(|p| p.id == id).unwrap_or(0);
    if CURRENT.swap(i, std::sync::atomic::Ordering::Relaxed) != i || ctx.style().visuals.panel_fill != pal().bg_app {
        apply(ctx);
    }
}

/// Leichte Überlagerung für Hover/Aktiv: hell auf dunklen Themes, dunkel auf hellem
pub fn overlay(alpha: u8) -> Color32 {
    if pal().dark {
        Color32::from_white_alpha(alpha)
    } else {
        Color32::from_black_alpha((alpha as f32 * 0.6) as u8)
    }
}

#[allow(non_snake_case)]
pub fn BG_APP() -> Color32 { pal().bg_app }
#[allow(non_snake_case)]
pub fn BG_CARD() -> Color32 { pal().bg_card }
#[allow(non_snake_case)]
pub fn BG_RAISED() -> Color32 { pal().bg_raised }
#[allow(non_snake_case)]
pub fn BG_HOVER() -> Color32 { pal().bg_hover }
#[allow(non_snake_case)]
pub fn BG_INPUT() -> Color32 { pal().bg_input }
#[allow(non_snake_case)]
pub fn BORDER() -> Color32 { pal().border }
#[allow(non_snake_case)]
pub fn TEXT() -> Color32 { pal().text }
#[allow(non_snake_case)]
pub fn TEXT_DIM() -> Color32 { pal().text_dim }
#[allow(non_snake_case)]
pub fn TEXT_FAINT() -> Color32 { pal().text_faint }
#[allow(non_snake_case)]
pub fn ACCENT() -> Color32 { pal().accent }
#[allow(non_snake_case)]
pub fn ACCENT_INK() -> Color32 { pal().accent_ink }

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
    let mut v = if pal().dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = BG_APP();
    v.window_fill = BG_CARD();
    v.window_stroke = Stroke::new(1.0_f32, BORDER());
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.extreme_bg_color = BG_INPUT();
    v.faint_bg_color = BG_RAISED();
    v.override_text_color = Some(TEXT());
    v.selection.bg_fill = overlay(46);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT());
    v.hyperlink_color = ACCENT();
    v.text_cursor.stroke = Stroke::new(1.5_f32, ACCENT());
    let r = CornerRadius::same(5);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER());
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_DIM());
    v.widgets.inactive.weak_bg_fill = BG_RAISED();
    v.widgets.inactive.bg_fill = BG_RAISED();
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT());
    v.widgets.hovered.weak_bg_fill = BG_HOVER();
    v.widgets.hovered.bg_fill = BG_HOVER();
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT());
    v.widgets.active.weak_bg_fill = overlay(40);
    v.widgets.active.bg_fill = overlay(40);
    v.widgets.active.bg_stroke = Stroke::NONE;
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, ACCENT());
    v.widgets.open.weak_bg_fill = BG_HOVER();
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(150),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(130),
    };
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.animation_time = anim(1.0 / 12.0);
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
        s.text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(12.0));
        s.text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(12.0));
        s.text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(10.5));
        s.text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(15.0));
        s.text_styles
            .insert(egui::TextStyle::Monospace, egui::FontId::monospace(11.0));
    });
}

/// Karte (abgerundet, schwebend). `margin` = Innenabstand.
pub fn card(margin: i8) -> egui::Frame {
    egui::Frame::default()
        .fill(BG_CARD())
        .corner_radius(RADIUS)
        .inner_margin(margin)
        .outer_margin(egui::Margin::same((GAP / 2.0) as i8))
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
    ui.painter().line_segment(
        [pos2(r.left() - 12.0, y), pos2(r.right() + 12.0, y)],
        Stroke::new(1.0_f32, BORDER()),
    );
    ui.add_space(6.0);
}

/// Primärknopf (weiße Fläche, dunkle Schrift).
/// Knopf mit weich eingeblendetem Hover (egui selbst schaltet hart um, und Knöpfe mit
/// eigener Füllfarbe zeigen sonst gar kein Hover).
pub struct Btn(egui::Button<'static>);

impl Btn {
    pub fn min_size(self, s: egui::Vec2) -> Self {
        Btn(self.0.min_size(s))
    }
}

/// Hover-Schleier über einem schon gezeichneten Knopf; beim Drücken etwas kräftiger.
pub fn hover_glow(ui: &egui::Ui, r: &egui::Response, radius: f32) {
    let t = ui
        .ctx()
        .animate_bool_with_time(r.id.with("hover"), r.enabled() && r.hovered(), anim(0.14));
    if t > 0.0 {
        let a = if r.is_pointer_button_down_on() { 44.0 } else { 24.0 };
        ui.painter().rect_filled(r.rect, radius, overlay((a * t) as u8));
    }
}

impl egui::Widget for Btn {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let r = ui.add(self.0);
        hover_glow(ui, &r, 5.0);
        r
    }
}

pub fn primary(text: impl Into<String>) -> Btn {
    Btn(egui::Button::new(
        RichText::new(crate::tr(text.into()))
            .size(11.5)
            .color(ACCENT_INK())
            .strong(),
    )
    .fill(ACCENT())
    .corner_radius(5)
    .min_size(vec2(0.0, 24.0)))
}

/// Sekundärknopf (dezente Fläche).
pub fn secondary(text: impl Into<String>) -> Btn {
    Btn(egui::Button::new(RichText::new(crate::tr(text.into())).size(11.5))
        .fill(BG_RAISED())
        .corner_radius(5)
        .min_size(vec2(0.0, 24.0)))
}

/// Flacher Knopf ohne Fläche (wird beim Überfahren hell).
pub fn ghost(text: impl Into<String>) -> Btn {
    Btn(egui::Button::new(RichText::new(crate::tr(text.into())).size(11.5))
        .frame(false)
        .min_size(vec2(0.0, 22.0)))
}

/// Tastenkürzel als gemalte Tastenkappen: einheitliche Höhe, Symbole für Sondertasten.
/// `keys` z. B. „Strg+Umschalt+Z“ (wird übersetzt).
pub fn keycaps(ui: &mut egui::Ui, keys: &str) {
    let keys = crate::tr(keys);
    let parts: Vec<&str> = if let Some(k) = keys.strip_suffix("++") {
        k.split('+').chain(["+"]).collect()
    } else {
        keys.split('+').collect()
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for k in parts {
            let k = k.trim();
            let (text, is_icon) = match k {
                "Umschalt" | "Shift" => (icon::ARROW_FAT_UP, true),
                "←" => (icon::ARROW_LEFT, true),
                "→" => (icon::ARROW_RIGHT, true),
                "↑" => (icon::ARROW_UP, true),
                "↓" => (icon::ARROW_DOWN, true),
                "Enter" => (icon::ARROW_ELBOW_DOWN_LEFT, true),
                "Backspace" => (icon::BACKSPACE, true),
                "Tab" => (icon::ARROW_LINE_RIGHT, true),
                _ => (k, false),
            };
            let g = ui.painter().layout_no_wrap(
                text.to_string(),
                egui::FontId::proportional(if is_icon { 12.0 } else { 10.5 }),
                TEXT(),
            );
            let (r, resp) = ui.allocate_exact_size(vec2((g.size().x + 11.0).max(20.0), 19.0), egui::Sense::hover());
            if is_icon {
                resp.on_hover_text(k);
            }
            let p = ui.painter();
            // Kappe mit dunklerer Unterkante (wirkt wie eine Taste)
            p.rect_filled(r, 4.0, BORDER());
            let top = egui::Rect::from_min_max(r.min + vec2(1.0, 1.0), r.max - vec2(1.0, 2.5));
            p.rect_filled(top, 3.0, BG_HOVER());
            p.galley(top.center() - g.size() / 2.0, g, TEXT());
        }
    });
}

/// Icon-Knopf ohne Rahmen, mit Tooltip. `active` hebt ihn hervor.
pub fn icon_button(
    ui: &mut egui::Ui,
    glyph: &str,
    tip: impl Into<String>,
    enabled: bool,
    active: bool,
) -> egui::Response {
    let color = if !enabled {
        TEXT_FAINT()
    } else if active {
        ACCENT()
    } else {
        TEXT_DIM()
    };
    let btn = egui::Button::new(RichText::new(glyph).size(14.0).color(color))
        .fill(if active {
            overlay(22)
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(5)
        .min_size(vec2(24.0, 22.0));
    let r = ui.add_enabled(enabled, btn);
    hover_glow(ui, &r, 5.0);
    let tip = crate::tr(tip.into());
    if tip.is_empty() {
        r
    } else {
        tip_with_keys(r, &tip)
    }
}

/// Menüeintrag mit weich eingeblendetem Hover-Hintergrund und Tastenkappen rechts.
/// Text wird nicht übersetzt (Aufrufer übergeben ihn schon übersetzt).
pub struct MenuRow {
    text: String,
    keys: String,
    hint: String,
    min_w: f32,
}

pub fn menu_row(text: impl Into<String>) -> MenuRow {
    MenuRow {
        text: text.into(),
        keys: String::new(),
        hint: String::new(),
        min_w: 0.0,
    }
}

impl MenuRow {
    pub fn keys(mut self, keys: &str) -> Self {
        self.keys = keys.to_string();
        self
    }
    /// Schlichter Zusatz rechts (z. B. Uhrzeit), statt Tastenkappen
    pub fn hint(mut self, hint: &str) -> Self {
        self.hint = hint.to_string();
        self
    }
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_w = w;
        self
    }
}

impl egui::Widget for MenuRow {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let enabled = ui.is_enabled();
        let color = if enabled { TEXT() } else { TEXT_FAINT() };
        let galley = ui.painter().layout_no_wrap(self.text, egui::FontId::proportional(12.0), color);
        let kg = if !self.keys.is_empty() {
            Some(ui.fonts(|f| f.layout_job(key_hint(&self.keys))))
        } else if !self.hint.is_empty() {
            Some(ui.painter().layout_no_wrap(crate::tr(&self.hint), egui::FontId::proportional(10.0), TEXT_FAINT()))
        } else {
            None
        };
        let kw = kg.as_ref().map_or(0.0, |g| g.size().x + 20.0);
        let mut w = (galley.size().x + kw + 18.0).max(self.min_w);
        if ui.layout().horizontal_justify() {
            w = w.max(ui.available_width());
        }
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), egui::Sense::click());
        let t = ui
            .ctx()
            .animate_bool_with_time(resp.id.with("hover"), enabled && resp.hovered(), anim(0.12));
        let p = ui.painter();
        if t > 0.001 {
            p.rect_filled(rect, 5.0, BG_HOVER().linear_multiply(t));
        }
        p.galley(
            pos2(rect.left() + 8.0 + 2.0 * t, rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
        if let Some(g) = kg {
            p.galley(
                pos2(rect.right() - 8.0 - g.size().x, rect.center().y - g.size().y / 2.0),
                g,
                TEXT_DIM(),
            );
        }
        resp
    }
}

/// Fehler direkt unter einem Eingabefeld (statt Meldung am unteren Rand)
pub fn field_error(ui: &mut egui::Ui, err: Option<&str>) {
    if let Some(e) = err {
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{}  {}", icon::WARNING_CIRCLE, crate::tr(e))).small().color(DANGER));
    }
}

/// Tastenkürzel als kleine Tastenkappen, z. B. „Strg+B“ → [Ctrl] + [B] (Text wird übersetzt).
pub fn key_hint(keys: &str) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let mut job = LayoutJob::default();
    let keys = crate::tr(keys);
    if keys.trim().is_empty() {
        return job;
    }
    let plus = TextFormat {
        font_id: egui::FontId::proportional(9.5),
        color: TEXT_FAINT(),
        ..Default::default()
    };
    let cap = TextFormat {
        font_id: egui::FontId::proportional(10.0),
        color: TEXT_DIM(),
        background: BG_HOVER(),
        ..Default::default()
    };
    // „Strg++“ (Plus-Taste) nicht am Pluszeichen trennen
    let parts: Vec<&str> = if let Some(k) = keys.strip_suffix("++") {
        k.split('+').chain(["+"]).collect()
    } else {
        keys.split('+').collect()
    };
    for (i, k) in parts.iter().enumerate() {
        if i > 0 {
            job.append("+", 2.0, plus.clone());
        }
        job.append(&format!("\u{2009}{}\u{2009}", k.trim()), if i > 0 { 2.0 } else { 0.0 }, cap.clone());
    }
    job
}

/// Tooltip „Name  (Taste)“ mit Tastenkappen statt Klammertext.
pub fn tip_with_keys(r: egui::Response, tip: &str) -> egui::Response {
    match tip.rsplit_once("  (").filter(|(_, k)| k.ends_with(')')) {
        Some((name, keys)) => {
            let (name, keys) = (name.to_string(), keys.trim_end_matches(')').to_string());
            r.on_hover_ui(move |ui| {
                ui.horizontal(|ui| {
                    ui.label(&name);
                    ui.add_space(6.0);
                    ui.label(key_hint(&keys));
                });
            })
        }
        None => r.on_hover_text(tip),
    }
}

/// Kleiner umrandeter Chip (Player-Leiste).
pub fn chip(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(crate::tr(text)).size(10.5).color(TEXT_DIM()))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0_f32, BORDER()))
            .corner_radius(5),
    )
}

/// Kategorie-Chip (aktiv = weiß gefüllt).
pub fn cat_chip(ui: &mut egui::Ui, text: &str, active: bool) -> egui::Response {
    let (fg, bg) = if active {
        (ACCENT_INK(), ACCENT())
    } else {
        (TEXT_DIM(), BG_RAISED())
    };
    ui.add(
        egui::Button::new(RichText::new(crate::tr(text)).size(10.5).color(fg))
            .fill(bg)
            .corner_radius(10)
            .min_size(vec2(0.0, 20.0)),
    )
}

/// Zeile mit fester Label-Spalte (Inspektor).
pub fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        ui.add_sized(
            [LABEL_W, 18.0],
            egui::Label::new(RichText::new(crate::tr(label)).size(11.5).color(TEXT_DIM())).truncate(),
        );
        add(ui)
    })
    .inner
}

/// Abschnittstitel im Inspektor mit Trennlinie davor.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    let r = ui.available_rect_before_wrap();
    ui.painter().line_segment(
        [pos2(r.left(), r.top()), pos2(r.right(), r.top())],
        Stroke::new(1.0_f32, BORDER()),
    );
    ui.add_space(9.0);
    ui.label(
        RichText::new(crate::tr(title))
            .size(11.0)
            .color(TEXT())
            .strong(),
    );
    ui.add_space(3.0);
}

/// Leerer Zustand: Icon, Titel, Erklärung.
pub fn empty_state(ui: &mut egui::Ui, glyph: &str, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(26.0);
        ui.label(RichText::new(glyph).size(26.0).color(TEXT_FAINT()));
        ui.add_space(6.0);
        ui.label(RichText::new(crate::tr(title)).strong());
        ui.label(RichText::new(crate::tr(hint)).small().color(TEXT_DIM()));
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
    let r = |x: f32, y: f32, w: f32, h: f32| {
        Rect::from_min_size(o + vec2(x * s, y * s), vec2(w * s, h * s))
    };
    painter.rect_filled(r(0.0, 0.0, 64.0, 64.0), 15.0 * s, ACCENT());
    let ink = ACCENT_INK();
    for (x, y, w) in [
        (11.0, 16.0, 20.0),
        (11.0, 28.0, 13.0),
        (11.0, 40.0, 20.0),
        (37.0, 13.0, 16.0),
        (30.0, 25.0, 23.0),
        (37.0, 37.0, 12.0),
    ] {
        painter.rect_filled(r(x, y, w, 8.0), 4.0 * s, ink);
    }
    painter.rect_filled(r(32.5, 8.0, 2.0, 48.0), 1.0 * s, ink);
}

/// Logo + Wortmarke.
pub fn wordmark(ui: &mut egui::Ui, size: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    logo(ui.painter(), r);
    ui.label(
        RichText::new("Cutline")
            .strong()
            .size(size * 0.62)
            .color(TEXT()),
    );
}

/// Raute (Keyframe) zeichnen.
pub fn diamond(painter: &egui::Painter, c: Pos2, r: f32, fill: Option<Color32>, stroke: Stroke) {
    let pts = vec![
        pos2(c.x, c.y - r),
        pos2(c.x + r, c.y),
        pos2(c.x, c.y + r),
        pos2(c.x - r, c.y),
    ];
    painter.add(egui::Shape::convex_polygon(
        pts,
        fill.unwrap_or(Color32::TRANSPARENT),
        stroke,
    ));
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
    Some(egui::IconData {
        rgba,
        width: size,
        height: size,
    })
}
