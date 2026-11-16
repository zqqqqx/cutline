//! Design-System (Stand Phase 2):
//! - Monochrom: fast schwarze Flächen, Weiß als einzige Akzentfarbe (aktive Zustände,
//!   Primäraktion, Auswahl). Farbe kommt nur aus dem Inhalt (Clips, Vorschauen).
//! - Schwebende Karten mit 6-px-Radius (R_CARD), 4 px Abstand, ein Icon-Stil (Phosphor, Linien)
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
    /// Warn-/Hinweistext (≥ 4,5:1 auf `bg_card` und `bg_raised`)
    pub warn_text: Color32,
}

const fn rgb(h: u32) -> Color32 {
    Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

pub const PALETTES: [Palette; 4] = [
    Palette { name: "Dunkel", id: "dark", dark: true, bg_app: rgb(0x0C0C0E), bg_card: rgb(0x17171A), bg_raised: rgb(0x222226), bg_hover: rgb(0x2B2B30), bg_input: rgb(0x101012), border: rgb(0x29292E), text: rgb(0xECECEE), text_dim: rgb(0x8C8C93), text_faint: rgb(0x58585F), accent: rgb(0xF4F4F6), accent_ink: rgb(0x101012), warn_text: WARN },
    Palette { name: "Dunkelgrau", id: "darkgray", dark: true, bg_app: rgb(0x1A1A1C), bg_card: rgb(0x252528), bg_raised: rgb(0x303034), bg_hover: rgb(0x3A3A3F), bg_input: rgb(0x1E1E21), border: rgb(0x37373C), text: rgb(0xEDEDEF), text_dim: rgb(0x9C9CA3), text_faint: rgb(0x68686F), accent: rgb(0xF4F4F6), accent_ink: rgb(0x141416), warn_text: WARN },
    Palette { name: "Grau", id: "gray", dark: true, bg_app: rgb(0x38383C), bg_card: rgb(0x444449), bg_raised: rgb(0x505055), bg_hover: rgb(0x5C5C62), bg_input: rgb(0x313135), border: rgb(0x58585E), text: rgb(0xF4F4F6), text_dim: rgb(0xC8C8CE), text_faint: rgb(0x8A8A91), accent: rgb(0xFFFFFF), accent_ink: rgb(0x202024), warn_text: rgb(0xF5BA88) },
    Palette { name: "Hell", id: "light", dark: false, bg_app: rgb(0xE9E9EC), bg_card: rgb(0xFBFBFC), bg_raised: rgb(0xEFEFF2), bg_hover: rgb(0xE2E2E7), bg_input: rgb(0xF4F4F6), border: rgb(0xD8D8DE), text: rgb(0x18181B), text_dim: rgb(0x5A5A62), text_faint: rgb(0x96969E), accent: rgb(0x18181B), accent_ink: rgb(0xFFFFFF), warn_text: rgb(0x9A5210) },
];

static CURRENT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static ANIMATIONS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_animations(on: bool) {
    ANIMATIONS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Die drei Dauern aller Bewegungen: schnell (Hover, Drücken, Symbolwechsel, Menüs), normal (Wechsel,
/// Ein/Aus, Verschieben, Aufklappen), ruhig (Fenster, große Flächen). Nichts dauert länger.
pub const MOTION_FAST: f32 = 0.09;
pub const MOTION: f32 = 0.16;
pub const MOTION_SLOW: f32 = 0.24;

/// Eine Ladeanzeige erscheint erst nach `LOAD_DELAY` und bleibt dann mindestens `LOAD_MIN` stehen.
pub const LOAD_DELAY: f64 = 0.15;
pub const LOAD_MIN: f64 = 0.30;

/// Dauer einer Bewegung (0, wenn Animationen ausgeschaltet sind). Nur mit den `MOTION`-Konstanten aufrufen.
pub fn anim(secs: f32) -> f32 {
    anim_if(animations_on(), secs)
}

fn anim_if(on: bool, secs: f32) -> f32 {
    if on {
        secs
    } else {
        0.0
    }
}

pub fn animations_on() -> bool {
    ANIMATIONS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Die eine Kurve aller Bewegungen: sanft auslaufend (kubisch), ohne Überschwingen.
pub fn ease(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// Weich ein-/ausblenden (0…1). Beim ersten Aufruf steht der Wert sofort am Ziel.
pub fn fade(ctx: &egui::Context, id: egui::Id, on: bool, secs: f32) -> f32 {
    ctx.animate_bool_with_time_and_easing(id, on, anim(secs), ease)
}

/// Zustand von [`glide`]: Start, Ziel, Startzeit
#[derive(Clone, Copy)]
struct Glide(f32, f32, f64);

/// Stand einer Gleitbewegung von `from` nach `to`, `age` Sekunden nach dem Start.
pub fn glide_at(from: f32, to: f32, age: f32, secs: f32) -> f32 {
    if secs <= 0.0 {
        return to;
    }
    from + (to - from) * ease(age / secs)
}

/// Wert/Position weich zum Ziel gleiten lassen (eine Kurve, Dauer `secs`). Beim ersten Aufruf
/// steht der Wert sofort am Ziel; ein neues Ziel mitten in der Bewegung startet vom aktuellen Stand.
pub fn glide(ctx: &egui::Context, id: egui::Id, to: f32, secs: f32) -> f32 {
    let now = ctx.input(|i| i.time);
    let secs = anim(secs);
    let g = match ctx.data(|d| d.get_temp::<Glide>(id)) {
        Some(g) if g.1 == to => g,
        old => {
            let from = old.map_or(to, |g| glide_at(g.0, g.1, (now - g.2) as f32, secs));
            let n = Glide(from, to, now);
            ctx.data_mut(|d| d.insert_temp(id, n));
            n
        }
    };
    let age = (now - g.2) as f32;
    if age < secs && g.0 != g.1 {
        ctx.request_repaint();
    }
    glide_at(g.0, g.1, age, secs)
}

/// Inhalt nach einem Wechsel (Tab, Reiter) kurz einblenden: Deckkraft 0…1 seit dem letzten Wechsel
/// von `key`. Der erste Aufruf blendet nicht.
pub fn fade_on_change(ctx: &egui::Context, id: egui::Id, key: u64) -> f32 {
    let now = ctx.input(|i| i.time);
    let old = ctx.data(|d| d.get_temp::<(u64, f64)>(id));
    let at = match old {
        Some((k, at)) if k == key => at,
        Some(_) => now,
        None => f64::MIN,
    };
    if old.map(|o| o.0) != Some(key) {
        ctx.data_mut(|d| d.insert_temp(id, (key, at)));
    }
    let secs = anim(MOTION_FAST);
    let age = (now - at) as f32;
    if secs > 0.0 && age < secs {
        ctx.request_repaint();
        ease(age / secs)
    } else {
        1.0
    }
}

/// Ladeanzeige zeigen? `busy_for`: Sekunden seit Beginn, solange noch beschäftigt (`None` = fertig);
/// `shown_for`: Sekunden, seit die Anzeige sichtbar ist. Kurze Wartezeiten bleiben unsichtbar, eine
/// einmal gezeigte Anzeige blitzt nicht nur kurz auf.
pub fn loader_visible(busy_for: Option<f64>, shown_for: Option<f64>) -> bool {
    match (busy_for, shown_for) {
        (_, Some(s)) if s < LOAD_MIN => true,
        (Some(b), _) => b >= LOAD_DELAY,
        (None, _) => false,
    }
}

/// Zustand von [`loading`]: beschäftigt seit, sichtbar seit, zuletzt gefragt (Bildnummer)
#[derive(Clone, Copy, Default)]
struct Loader(Option<f64>, Option<f64>, u64);

/// [`loader_visible`] mit gemerktem Zustand je `id`. Fordert ein Neuzeichnen nur für den Moment an,
/// in dem sich die Antwort ohne neues Ereignis ändert.
pub fn loading(ctx: &egui::Context, id: egui::Id, busy: bool) -> bool {
    let id = id.with("loading");
    let pass = ctx.cumulative_pass_nr();
    let Some(mut s) = ctx.data(|d| d.get_temp::<Loader>(id)).or(busy.then(Loader::default)) else {
        return false;
    };
    // länger nicht gezeichnet (anderer Tab, Fenster zu): neu beginnen
    if s.2 + 1 < pass {
        s = Loader::default();
    }
    let now = ctx.input(|i| i.time);
    s.0 = if busy { Some(s.0.unwrap_or(now)) } else { None };
    let vis = loader_visible(s.0.map(|b| now - b), s.1.map(|v| now - v));
    s.1 = if vis { Some(s.1.unwrap_or(now)) } else { None };
    s.2 = pass;
    if !vis && !busy {
        ctx.data_mut(|d| d.remove::<Loader>(id));
        return false;
    }
    ctx.data_mut(|d| d.insert_temp(id, s));
    let wait = match (vis, s.0, s.1) {
        (false, Some(b), _) => Some(LOAD_DELAY - (now - b)),
        (true, None, Some(v)) => Some(LOAD_MIN - (now - v)),
        _ => None,
    };
    if let Some(w) = wait {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(w.max(0.0) + 0.005));
    }
    vis
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
    // Auswahlrahmen trägt den zuletzt angewandten Akzent → neuer Akzent wendet die Visuals neu an
    let st = ctx.style();
    if CURRENT.swap(i, std::sync::atomic::Ordering::Relaxed) != i
        || st.visuals.panel_fill != pal().bg_app
        || st.visuals.selection.stroke.color != ACCENT()
    {
        apply(ctx);
    }
}

/// Akzent-Vorgaben: (id, Name, Farbe auf dunklen Themes, Farbe auf hellem Theme).
/// Auf dunklen Flächen heller/entsättigt (satte Farben „glühen“ dort), alle ≥ 3:1 gegen
/// `bg_card`; „auto“ ist der neutrale Akzent der Palette.
pub const ACCENTS: [(&str, &str, Color32, Color32); 8] = [
    ("auto", "Weiß", PALETTES[0].accent, PALETTES[3].accent),
    ("blue", "Blau", rgb(0x5B9DFF), rgb(0x2B63D9)),
    ("teal", "Türkis", rgb(0x3CC8C0), rgb(0x0E7C80)),
    ("green", "Grün", rgb(0x5CC98A), rgb(0x1F8048)),
    ("violet", "Violett", rgb(0xA98BFA), rgb(0x7445D6)),
    ("pink", "Pink", rgb(0xF27AB6), rgb(0xC2317A)),
    ("orange", "Orange", rgb(0xF59A55), rgb(0xB85A12)),
    // Koralle statt Reinrot, damit der Akzent nicht wie `DANGER` aussieht
    ("red", "Rot", rgb(0xFF7E6E), rgb(0xBF5039)),
];

/// 0 = auto, 1.. = Vorgabe, CUSTOM|rgb = eigene Farbe
static ACCENT_SPEC: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
const CUSTOM: u32 = 1 << 31;

/// Akzent aus den Einstellungen: „auto“, Vorgabe-id oder „#RRGGBB“. Unbekanntes = auto.
pub fn set_accent(spec: &str) {
    let v = match ACCENTS.iter().position(|a| a.0 == spec) {
        Some(i) => i as u32,
        None => Color32::from_hex(spec).map_or(0, |c| CUSTOM | (u32::from_be_bytes(c.to_array()) >> 8)),
    };
    ACCENT_SPEC.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// Relative Leuchtdichte nach WCAG (0 = schwarz, 1 = weiß)
pub fn luminance(c: Color32) -> f32 {
    let l = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * l(c.r()) + 0.7152 * l(c.g()) + 0.0722 * l(c.b())
}

/// Kontrastverhältnis nach WCAG (1 … 21)
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Gewählter Akzent für Palette `p` (Vorgaben und eigene Farben werden bis 3:1 gegen `bg_card` und `bg_raised` aufgehellt
/// bzw. abgedunkelt, damit aktive Zustände sichtbar bleiben).
pub fn accent_for(p: &Palette) -> Color32 {
    let mut c = match ACCENT_SPEC.load(std::sync::atomic::Ordering::Relaxed) {
        0 => return p.accent,
        s if s & CUSTOM != 0 => rgb(s & 0xFF_FFFF),
        i => {
            let a = &ACCENTS[(i as usize).min(ACCENTS.len() - 1)];
            if p.dark { a.2 } else { a.3 }
        }
    };
    // Vorgaben passen meist schon; im Design „Grau“ werden Blau/Violett so leicht aufgehellt
    let to = if p.dark { Color32::WHITE } else { Color32::BLACK };
    for _ in 0..60 {
        if contrast(c, p.bg_card) >= 3.0 && contrast(c, p.bg_raised) >= 3.0 {
            break;
        }
        c = c.lerp_to_gamma(to, 0.05);
    }
    c
}

/// Schrift auf `accent`: dunkle oder helle Tinte der Palette, je nachdem was stärker kontrastiert
fn ink_for(p: &Palette, accent: Color32) -> Color32 {
    let (dark, light) = if p.dark { (p.accent_ink, p.text) } else { (p.text, p.accent_ink) };
    if contrast(dark, accent) >= contrast(light, accent) { dark } else { light }
}

/// Leichte Überlagerung für Hover/Aktiv: hell auf dunklen Themes, dunkel auf hellem
pub fn overlay(alpha: u8) -> Color32 {
    if pal().dark {
        Color32::from_white_alpha(alpha)
    } else {
        Color32::from_black_alpha((alpha as f32 * 0.6) as u8)
    }
}

/// Fläche hinter Textvorschauen: Texte sind für Video gemacht, deshalb in jedem Design dunkel
pub const STAGE: Color32 = Color32::from_rgb(0x1E, 0x24, 0x2C);

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
/// Nur für Deaktiviertes, Platzhalter in leeren Feldern, Trenner und reine Dekoration
/// (zu wenig Kontrast für Lesetext). Alles, was Information trägt, nimmt `TEXT_DIM()`.
#[allow(non_snake_case)]
pub fn TEXT_FAINT() -> Color32 { pal().text_faint }
#[allow(non_snake_case)]
pub fn ACCENT() -> Color32 { accent_for(pal()) }
#[allow(non_snake_case)]
pub fn ACCENT_INK() -> Color32 { ink_for(pal(), ACCENT()) }

pub const DANGER: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);
pub const SNAP: Color32 = Color32::from_rgb(0xFF, 0xD8, 0x4D);
/// Warnung/Hinweis (fehlende Datei, Lizenz prüfen …)
pub const WARN: Color32 = Color32::from_rgb(0xE0, 0x90, 0x50);
/// Erfolg (Export fertig)
pub const SUCCESS: Color32 = Color32::from_rgb(0x3D, 0xD6, 0x8C);
/// Fläche hinter Erfolgsmeldungen
pub const SUCCESS_BG: Color32 = Color32::from_rgb(0x0B, 0x24, 0x18);

/// Fehler-Toast: getönte Fläche und lesbare Schrift, auf hellem wie dunklem Theme
#[allow(non_snake_case)]
pub fn DANGER_BG() -> Color32 {
    BG_CARD().lerp_to_gamma(DANGER, if pal().dark { 0.18 } else { 0.12 })
}
#[allow(non_snake_case)]
pub fn DANGER_TEXT() -> Color32 { danger_text(pal()) }
fn danger_text(p: &Palette) -> Color32 {
    p.text.lerp_to_gamma(DANGER, 0.35)
}
/// Warn-/Hinweistext; `WARN` selbst ist auf hellen Flächen zu schwach
#[allow(non_snake_case)]
pub fn WARN_TEXT() -> Color32 { pal().warn_text }
/// Füllung roter Knöpfe: weiße Schrift ≥ 4,5:1 (`DANGER` hat nur 3,9:1)
pub const DANGER_FILL: Color32 = rgb(0xC9302C);

pub const RADIUS: u8 = R_CARD as u8;
pub const GAP: f32 = 4.0;

/// Schriftgrößen-Stufen (für die Vereinheitlichung der Oberfläche):
/// Hinweise, Fußnoten, Tastenkappen
pub const FS_HINT: f32 = 10.5;
/// Bedienelemente, Knöpfe, Labels
pub const FS_UI: f32 = 11.5;
/// Fließtext
pub const FS_TEXT: f32 = 12.5;
/// Karten- und Abschnittstitel
pub const FS_TITLE: f32 = 14.0;
/// Seitentitel (Einstellungen, Startseite)
pub const FS_PAGE: f32 = 18.0;
/// Große Überschrift (Willkommen, leere Startseite)
pub const FS_HERO: f32 = 26.0;
/// Radius für Bedienelemente, Chips, Tastenkappen
pub const R_CTRL: f32 = 4.0;
/// Radius für Karten, Kacheln, Clips
pub const R_CARD: f32 = 6.0;
/// Radius für Fenster, Menüs, Toasts
pub const R_WIN: f32 = 10.0;
/// Innenabstand kleiner Karten
pub const PAD_CARD: i8 = 8;
/// Innenabstand großer Karten
pub const PAD_CARD_L: i8 = 12;
/// Innenabstand von Dialogen
pub const PAD_DIALOG: i8 = 16;
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
    v.window_corner_radius = CornerRadius::same(R_WIN as u8);
    v.menu_corner_radius = CornerRadius::same(R_WIN as u8);
    v.extreme_bg_color = BG_INPUT();
    v.faint_bg_color = BG_RAISED();
    v.override_text_color = Some(TEXT());
    // Akzent-Schleier: färbt Fortschrittsbalken/Spinner, markierter Text bleibt lesbar (≥ ~4:1)
    v.selection.bg_fill = ACCENT().gamma_multiply(if pal().dark { 0.30 } else { 0.35 });
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT());
    v.hyperlink_color = ACCENT();
    v.text_cursor.stroke = Stroke::new(1.5_f32, ACCENT());
    let r = CornerRadius::same(R_CTRL as u8);
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
    // nicht Akzent: egui färbt damit auch jeden `.strong()`-Text (Titel, Überschriften)
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, TEXT());
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
        // egui-eigene Bewegungen (Menüs, Tooltips, Popups blenden damit ein)
        s.animation_time = anim(MOTION_FAST);
        s.interaction.tooltip_delay = 0.4;
        s.spacing.item_spacing = vec2(6.0, 4.0);
        s.spacing.button_padding = vec2(8.0, 3.0);
        s.spacing.interact_size = vec2(22.0, 20.0);
        s.spacing.slider_width = 120.0;
        s.spacing.slider_rail_height = 2.0;
        s.spacing.combo_width = 120.0;
        s.spacing.icon_width = 12.0;
        s.spacing.icon_width_inner = 6.0;
        s.spacing.menu_margin = egui::Margin::same(5);
        // Dialogrand (auch für Fenster, die nicht über `dialog::frame` laufen)
        s.spacing.window_margin = egui::Margin::same(PAD_DIALOG);
        s.spacing.scroll = egui::style::ScrollStyle::thin();
        s.visuals.handle_shape = egui::style::HandleShape::Circle;
        s.text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(FS_TEXT));
        s.text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(FS_UI));
        s.text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(FS_HINT));
        s.text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(FS_TITLE));
        s.text_styles
            .insert(egui::TextStyle::Monospace, egui::FontId::monospace(FS_UI));
    });
}

/// Tastaturbedienung an? Tab schaltet ein, ein Mausklick wieder aus (dann kein Rahmen um den
/// zuletzt geklickten Knopf).
fn keyboard_nav(was: bool, tab: bool, click: bool) -> bool {
    (was || tab) && !click
}

/// Sichtbarer Fokusrahmen (Akzent) um das Element mit Tastaturfokus, solange mit Tab bedient wird.
pub fn focus_ring(ctx: &egui::Context) {
    let key = egui::Id::new("keyboard-nav");
    let (tab, click) = ctx.input(|i| (i.key_pressed(egui::Key::Tab), i.pointer.any_pressed()));
    let on = keyboard_nav(ctx.data(|d| d.get_temp::<bool>(key)).unwrap_or(false), tab, click);
    ctx.data_mut(|d| d.insert_temp(key, on));
    if let Some(r) = on.then(|| ctx.memory(|m| m.focused())).flatten().and_then(|id| ctx.read_response(id)) {
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("focus-ring"));
        ctx.layer_painter(layer).rect_stroke(
            r.rect.expand(2.0),
            R_CTRL,
            Stroke::new(1.5_f32, ACCENT()),
            egui::StrokeKind::Outside,
        );
    }
}

/// Karte (abgerundet, schwebend). `margin` = Innenabstand.
pub fn card(margin: i8) -> egui::Frame {
    egui::Frame::default()
        .fill(BG_CARD())
        .corner_radius(R_CARD)
        .inner_margin(margin)
        .outer_margin(egui::Margin::same((GAP / 2.0) as i8))
}

/// Kopfzeile einer Karte: Titel links, Aktionen rechts, dünne Linie darunter.
pub fn card_header(ui: &mut egui::Ui, title: &str, actions: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        ui.label(RichText::new(crate::tr(title)).strong().size(FS_TEXT));
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
/// Zweites Feld: Füllfarbe gefüllter Knöpfe (primär/Gefahr); deren Hover tönt die Fläche statt
/// eines Schleiers über der Schrift (der Schleier wäscht die Beschriftung aus).
pub struct Btn(egui::Button<'static>, Option<Color32>);

impl Btn {
    pub fn min_size(self, s: egui::Vec2) -> Self {
        Btn(self.0.min_size(s), self.1)
    }
}

/// Hover-Schleier über einem schon gezeichneten Knopf; beim Drücken etwas kräftiger.
pub fn hover_glow(ui: &egui::Ui, r: &egui::Response, radius: f32) {
    let t = fade(ui.ctx(), r.id.with("hover"), r.enabled() && r.hovered(), MOTION_FAST);
    if t > 0.0 {
        let a = if r.is_pointer_button_down_on() { 44.0 } else { 24.0 };
        ui.painter().rect_filled(r.rect, radius, overlay((a * t) as u8));
    }
}

impl egui::Widget for Btn {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let Some(fill) = self.1 else {
            let r = ui.add(self.0);
            hover_glow(ui, &r, R_CTRL);
            return r;
        };
        // Hover-Zustand vom letzten Frame: die Füllung muss vor dem Zeichnen feststehen
        let id = ui.next_auto_id();
        let last = ui.ctx().read_response(id);
        let hov = last.as_ref().is_some_and(|r| r.enabled() && r.hovered());
        let t = fade(ui.ctx(), id.with("hover"), hov, MOTION_FAST);
        let down = last.is_some_and(|r| r.is_pointer_button_down_on());
        ui.add(self.0.fill(hover_fill(fill, t, down)))
    }
}

/// Knopffüllung beim Überfahren/Drücken; rote Knöpfe dunkeln ab, damit die weiße Schrift lesbar bleibt
fn hover_fill(fill: Color32, t: f32, down: bool) -> Color32 {
    let to = if luminance(fill) > 0.4 || fill == DANGER_FILL { Color32::BLACK } else { Color32::WHITE };
    fill.lerp_to_gamma(to, t * if down { 0.2 } else { 0.1 })
}

pub fn primary(text: impl Into<String>) -> Btn {
    Btn(egui::Button::new(
        RichText::new(crate::tr(text.into()))
            .size(FS_UI)
            .color(ACCENT_INK())
            .strong(),
    )
    .corner_radius(R_CTRL)
    .min_size(vec2(0.0, 24.0)), Some(ACCENT()))
}

/// Sekundärknopf (dezente Fläche).
pub fn secondary(text: impl Into<String>) -> Btn {
    Btn(egui::Button::new(RichText::new(crate::tr(text.into())).size(FS_UI))
        .fill(BG_RAISED())
        .corner_radius(R_CTRL)
        .min_size(vec2(0.0, 24.0)), None)
}

/// Gefährliche Aktion (Löschen): rote Fläche, weiße Schrift.
pub fn danger(text: impl Into<String>) -> Btn {
    Btn(egui::Button::new(RichText::new(crate::tr(text.into())).size(FS_UI).color(Color32::WHITE).strong())
        .corner_radius(R_CTRL)
        .min_size(vec2(0.0, 24.0)), Some(DANGER_FILL))
}

/// Flacher Knopf ohne Fläche (wird beim Überfahren hell).
pub fn ghost(text: impl Into<String>) -> Btn {
    Btn(egui::Button::new(RichText::new(crate::tr(text.into())).size(FS_UI))
        .frame(false)
        .min_size(vec2(0.0, 24.0)), None)
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
            let (text, is_icon) = key_symbol(k).map_or((k, false), |s| (s, true));
            let g = ui.painter().layout_no_wrap(
                text.to_string(),
                // Icon-Glyphe etwas größer als die Schrift, damit sie optisch gleich hoch wirkt
                egui::FontId::proportional(if is_icon { 12.0 } else { FS_HINT }),
                TEXT(),
            );
            let (r, resp) = ui.allocate_exact_size(vec2((g.size().x + 11.0).max(20.0), 19.0), egui::Sense::hover());
            if is_icon {
                resp.on_hover_text(k);
            }
            let p = ui.painter();
            // Kappe mit dunklerer Unterkante (wirkt wie eine Taste)
            p.rect_filled(r, R_CTRL, BORDER());
            let top = egui::Rect::from_min_max(r.min + vec2(1.0, 1.0), r.max - vec2(1.0, 2.5));
            // innere Fläche 1 px eingerückt → Radius 1 px kleiner
            p.rect_filled(top, R_CTRL - 1.0, BG_HOVER());
            p.galley(top.center() - g.size() / 2.0, g, TEXT());
        }
    });
}

/// Symbol für Sondertasten (gemeinsam für [`keycaps`] und [`key_hint`])
fn key_symbol(k: &str) -> Option<&'static str> {
    Some(match k {
        "Umschalt" | "Shift" => icon::ARROW_FAT_UP,
        "←" => icon::ARROW_LEFT,
        "→" => icon::ARROW_RIGHT,
        "↑" => icon::ARROW_UP,
        "↓" => icon::ARROW_DOWN,
        "Enter" => icon::ARROW_ELBOW_DOWN_LEFT,
        "Backspace" => icon::BACKSPACE,
        "Tab" => icon::ARROW_LINE_RIGHT,
        _ => return None,
    })
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
    // Icon-Glyphe: 14 px in Bedienelementen
    let btn = egui::Button::new(RichText::new(glyph).size(14.0).color(color))
        .fill(if active {
            ACCENT().gamma_multiply(0.18)
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(R_CTRL)
        .min_size(vec2(24.0, 24.0));
    let r = ui.add_enabled(enabled, btn);
    hover_glow(ui, &r, R_CTRL);
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
        let galley = ui.painter().layout_no_wrap(self.text, egui::FontId::proportional(FS_TEXT), color);
        let kg = if !self.keys.is_empty() {
            Some(ui.fonts(|f| f.layout_job(key_hint(&self.keys))))
        } else if !self.hint.is_empty() {
            Some(ui.painter().layout_no_wrap(crate::tr(&self.hint), egui::FontId::proportional(FS_HINT), TEXT_DIM()))
        } else {
            None
        };
        let kw = kg.as_ref().map_or(0.0, |g| g.size().x + 20.0);
        let mut w = (galley.size().x + kw + 18.0).max(self.min_w);
        if ui.layout().horizontal_justify() {
            w = w.max(ui.available_width());
        }
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), egui::Sense::click());
        let t = fade(ui.ctx(), resp.id.with("hover"), enabled && resp.hovered(), MOTION_FAST);
        let p = ui.painter();
        if t > 0.001 {
            p.rect_filled(rect, R_CTRL, BG_HOVER().linear_multiply(t));
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

/// Untermenü-Zeile, die wie [`menu_row`] aussieht (Höhe 24, FS_TEXT, Icon in derselben Spalte).
/// egui zeichnet Untermenüs sonst kleiner und enger (Menüstil: Innenabstand 2 px).
pub fn submenu<R>(
    ui: &mut egui::Ui,
    glyph: &str,
    label: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let old = ui.spacing().clone();
    ui.spacing_mut().button_padding = vec2(8.0, 0.0);
    ui.spacing_mut().interact_size.y = 24.0;
    let r = ui.menu_button(RichText::new(crate::tr(format!("{glyph}   {label}"))).size(FS_TEXT), add_contents);
    *ui.spacing_mut() = old;
    r
}

/// Fehler direkt unter einem Eingabefeld (statt Meldung am unteren Rand)
pub fn field_error(ui: &mut egui::Ui, err: Option<&str>) {
    if let Some(e) = err {
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{}  {}", icon::WARNING_CIRCLE, crate::tr(e))).small().color(DANGER_TEXT()));
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
        font_id: egui::FontId::proportional(FS_HINT),
        color: TEXT_FAINT(),
        ..Default::default()
    };
    let cap = TextFormat {
        font_id: egui::FontId::proportional(FS_HINT),
        color: TEXT(),
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
        let k = k.trim();
        let k = key_symbol(k).unwrap_or(k);
        job.append(&format!("\u{2009}{k}\u{2009}"), if i > 0 { 2.0 } else { 0.0 }, cap.clone());
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
        None => r.on_hover_text(tip).on_disabled_hover_text(tip),
    }
}

/// Kleiner umrandeter Chip (Player-Leiste).
pub fn chip(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(crate::tr(text)).size(FS_HINT).color(TEXT_DIM()))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0_f32, BORDER()))
            .corner_radius(R_CTRL)
            .min_size(vec2(0.0, 22.0)),
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
        egui::Button::new(RichText::new(crate::tr(text)).size(FS_HINT).color(fg))
            .fill(bg)
            .corner_radius(R_CTRL)
            .min_size(vec2(0.0, 22.0)),
    )
}

/// Chip einer Auswahlleiste mit gleitender Markierung ([`chip_marks`]): ohne eigene Fläche, die
/// Schrift wechselt weich zur Farbe auf dem Akzent.
pub fn strip_chip(ui: &mut egui::Ui, text: &str, active: bool) -> egui::Response {
    let t = fade(ui.ctx(), ui.id().with(("strip-chip", text)), active, MOTION);
    ui.add(
        egui::Button::new(RichText::new(crate::tr(text)).size(FS_HINT).color(TEXT_DIM().lerp_to_gamma(ACCENT_INK(), t)))
            .fill(Color32::TRANSPARENT)
            .corner_radius(R_CTRL)
            .min_size(vec2(0.0, 22.0)),
    )
}

/// Flächen der Chips einer Leiste und die Akzent-Markierung, die zum aktiven Chip gleitet; landet in
/// `slot` (vor den Chips mit `painter().add(Shape::Noop)` reserviert, damit sie darunter liegt).
pub fn chip_marks(ui: &egui::Ui, slot: egui::layers::ShapeIdx, id: egui::Id, chips: &[Rect], active: Option<usize>) {
    let mut shapes: Vec<egui::Shape> = chips.iter().map(|r| egui::Shape::rect_filled(*r, R_CTRL, BG_RAISED())).collect();
    if let Some(i) = active.filter(|i| *i < chips.len()) {
        let at = glide(ui.ctx(), id.with("chip-mark"), i as f32, MOTION).clamp(0.0, (chips.len() - 1) as f32);
        let (a, b, f) = (chips[at.floor() as usize], chips[at.ceil() as usize], at.fract());
        shapes.push(egui::Shape::rect_filled(Rect::from_min_max(a.min.lerp(b.min, f), a.max.lerp(b.max, f)), R_CTRL, ACCENT()));
    }
    ui.painter().set(slot, egui::Shape::Vec(shapes));
}

/// Breite eines einzeiligen Textes in der Schrift `font`.
pub fn text_width(ctx: &egui::Context, text: &str, font: egui::FontId) -> f32 {
    ctx.fonts(|f| f.layout_no_wrap(text.to_string(), font, Color32::WHITE).size().x)
}

/// Schräge Pfeile in Namen (↖ ↗ ↙ ↘) als Symbole der Icon-Schrift: die Grundschrift hat nicht
/// alle, der Rückfall auf eine andere Schrift sieht dann ungleich aus.
pub fn icon_arrows(s: &str) -> String {
    s.replace('↖', icon::ARROW_UP_LEFT)
        .replace('↗', icon::ARROW_UP_RIGHT)
        .replace('↙', icon::ARROW_DOWN_LEFT)
        .replace('↘', icon::ARROW_DOWN_RIGHT)
}

/// Dauer in Sekunden für die Oberfläche („6,40 s“ deutsch, „6.40 s“ englisch).
pub fn secs(v: f64, decimals: usize) -> String {
    format!("{} s", dec(v, decimals))
}

/// Dezimalzahl für die Oberfläche („1,79“ deutsch, „1.79“ englisch); die Einheit setzt der
/// Aufrufer dahinter. Nicht für Protokoll, FFmpeg-Argumente oder Dateien.
pub fn dec(v: f64, decimals: usize) -> String {
    local_num(format!("{v:.decimals$}"))
}
/// Schon formatierte Zahl („+1.5“, „05.20“) mit dem Dezimalzeichen der Oberfläche.
pub fn local_num(s: String) -> String {
    dec_mark(s, cl_i18n::ui_lang() == cl_i18n::Lang::De)
}
fn dec_mark(s: String, comma: bool) -> String {
    if comma { s.replace('.', ",") } else { s }
}

/// Getippte Dezimalzahl, mit Komma oder Punkt.
pub fn parse_dec(s: &str) -> Option<f64> {
    s.trim().replace(',', ".").parse().ok()
}

/// Zählertext mit richtiger Einzahl: `one`/`many` sind deutsche Vorlagen mit `{}` für die Zahl
/// (beide stehen im Wörterbuch, `tr` übersetzt das Ergebnis). Nur das erste `{}` wird gefüllt,
/// weitere bleiben für den Aufrufer.
pub fn count_text(n: usize, one: &str, many: &str) -> String {
    (if n == 1 { one } else { many }).replacen("{}", &n.to_string(), 1)
}

/// Kürzt `text` am Ende mit „…“, bis er in `max_w` passt (`width` misst eine Zeichenkette).
pub fn elide(text: &str, max_w: f32, width: impl Fn(&str) -> f32) -> String {
    if width(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len())
        .rev()
        .map(|n| format!("{}…", chars[..n].iter().collect::<String>().trim_end()))
        .find(|s| width(s) <= max_w)
        .unwrap_or_default()
}

/// Kürzt `text` in der Mitte („C:/Users/…/Videos“), bis er in `max_w` passt.
pub fn elide_middle(text: &str, max_w: f32, width: impl Fn(&str) -> f32) -> String {
    if width(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    (1..chars.len())
        .rev()
        .map(|n| {
            let (a, b) = (n.div_ceil(2), n / 2);
            format!("{}…{}", chars[..a].iter().collect::<String>(), chars[chars.len() - b..].iter().collect::<String>())
        })
        .find(|s| width(s) <= max_w)
        .unwrap_or_default()
}

/// Schriftgröße, mit der ein bei `size` `w` breiter Text in `max_w` passt (nie größer als `size`).
pub fn fit_size(size: f32, w: f32, max_w: f32) -> f32 {
    if w <= max_w || w <= 0.0 {
        size
    } else {
        size * max_w.max(0.0) / w
    }
}

/// Ränder einer blätterbaren Leiste. `chips` = (links, rechts) je Eintrag im Inhalt, `total` = Breite des
/// Inhalts (mit Rand hinter dem letzten Eintrag), `view` = sichtbare Breite, `off` = Versatz, `arrow` = Breite
/// des Pfeil-Knopfs. Je Seite: (Kante der Abdeckung im Inhalt,
/// Versatz nach dem Klick). Die Abdeckung reicht bis an den nächsten ganz sichtbaren Eintrag, damit nie
/// ein halber Eintrag neben dem Pfeil steht.
pub struct StripEdges {
    pub left: Option<(f32, f32)>,
    pub right: Option<(f32, f32)>,
}

pub fn strip_edges(chips: &[(f32, f32)], total: f32, view: f32, off: f32, arrow: f32) -> StripEdges {
    // nie kürzer als bis zum letzten Eintrag
    let total = chips.last().map_or(total, |c| c.1.max(total));
    let max = (total - view).max(0.0);
    let right = (max - off > 0.5).then(|| {
        let limit = off + view - arrow;
        // erster Eintrag, der nicht mehr ganz vor den Pfeil passt
        let c = chips.iter().find(|c| c.1 > limit).copied().unwrap_or((limit, limit));
        let next = (c.0 - arrow - 2.0).min(max);
        // Eintrag breiter als die Leiste: trotzdem weiterkommen
        let next = if next > off + 1.0 { next } else { (off + view * 0.7).min(max) };
        // fast am Ende: ganz ans Ende, sonst bleibt der letzte Eintrag ohne Pfeil angeschnitten
        let next = if max - next <= 0.5 { max } else { next };
        ((c.0 - 2.0).clamp(off, limit), next)
    });
    let left = (off > 1.0).then(|| {
        let limit = off + arrow;
        let cover = chips.iter().find(|c| c.0 < limit && c.1 > limit).map_or(limit, |c| c.1 + 2.0);
        // eine Seite zurück, ausgerichtet am Anfang eines Eintrags
        let start = off - (view - 2.0 * arrow);
        let prev = chips.iter().find(|c| c.0 >= start + arrow).map_or(0.0, |c| c.0 - arrow - 2.0);
        let prev = if prev < arrow + 6.0 || prev >= off - 1.0 { 0.0 } else { prev };
        (cover.min(off + view), prev)
    });
    StripEdges { left, right }
}

/// Pfeil-Knöpfe an waagrecht scrollbaren Zeilen auf Kartengrund: zeigen, dass links bzw. rechts
/// noch mehr kommt, und blättern per Klick weiter. `chips` = Flächen der Einträge (Bildschirm).
pub fn edge_arrows<R>(ui: &mut egui::Ui, out: &mut egui::scroll_area::ScrollAreaOutput<R>, chips: &[Rect]) {
    // `inner_rect` reicht bei `auto_shrink(false)` über die Zeile hinaus: Höhe aus dem Inhalt nehmen
    let (mut r, x) = (out.inner_rect, out.state.offset.x);
    r.set_height(out.content_size.y);
    let x0 = r.left() - x;
    let rel: Vec<(f32, f32)> = chips.iter().map(|c| (c.left() - x0, c.right() - x0)).collect();
    if out.content_size.x - r.width() <= 0.5 && x != 0.0 {
        // alles passt wieder (Bereich breiter gezogen): Versatz verwerfen
        out.state.offset.x = 0.0;
        out.state.store(ui.ctx(), out.id);
        ui.ctx().request_repaint();
        return;
    }
    let w = 20.0;
    let e = strip_edges(&rel, out.content_size.x, r.width(), x, w);
    let sides = [
        // 4 px über die Leiste hinaus: dort schaut sonst der Rand eines Eintrags hervor
        e.left.map(|(cover, to)| (Rect::from_min_max(r.min - vec2(4.0, 0.0), pos2(x0 + cover, r.bottom())), r.left() + w / 2.0, icon::CARET_LEFT, to)),
        e.right.map(|(cover, to)| (Rect::from_min_max(pos2(x0 + cover, r.top()), r.max + vec2(4.0, 0.0)), r.right() - w / 2.0, icon::CARET_RIGHT, to)),
    ];
    for (b, cx, glyph, to) in sides.into_iter().flatten() {
        ui.painter().rect_filled(b, 0.0, BG_CARD());
        let resp = ui.interact(b, out.id.with(glyph), egui::Sense::click());
        let col = if resp.hovered() { TEXT() } else { TEXT_DIM() };
        // Icon-Glyphe: 14 px wie in `icon_button`
        ui.painter().text(pos2(cx, b.center().y), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(14.0), col);
        if resp.clicked() {
            out.state.offset.x = to;
            out.state.store(ui.ctx(), out.id);
        }
    }
}

/// Zeile mit fester Label-Spalte (Inspektor).
pub fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        // linksbündig in der Label-Spalte (add_sized zentriert, das wirkte wie im Feld daneben)
        ui.allocate_ui_with_layout(vec2(LABEL_W, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(LABEL_W);
            ui.add(egui::Label::new(RichText::new(crate::tr(label)).size(FS_UI).color(TEXT_DIM())).truncate());
        });
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
            .size(FS_UI)
            .color(TEXT())
            .strong(),
    );
    ui.add_space(3.0);
}

/// Leerer Zustand: Icon, Titel, Erklärung.
pub fn empty_state(ui: &mut egui::Ui, glyph: &str, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(26.0);
        // Icon-Glyphe: leere Zustände nutzen 26 px
        ui.label(RichText::new(glyph).size(26.0).color(TEXT_FAINT()));
        ui.add_space(6.0);
        ui.label(RichText::new(crate::tr(title)).strong());
        if !hint.is_empty() {
            ui.label(RichText::new(crate::tr(hint)).small().color(TEXT_DIM()));
        }
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
    // Logo-Geometrie skaliert mit `s` (keine UI-Radien)
    // Logo bleibt neutral: Palettenakzent statt gewählter Akzentfarbe
    painter.rect_filled(r(0.0, 0.0, 64.0, 64.0), 15.0 * s, pal().accent);
    let ink = pal().accent_ink;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagonal_arrows_use_icon_font() {
        let s = icon_arrows("Wischen ↖ ↗ ↙ ↘");
        assert!(!s.contains(['↖', '↗', '↙', '↘']) && s.starts_with("Wischen "), "{s}");
    }

    #[test]
    fn seconds_use_language_decimal_mark() {
        assert_eq!(dec_mark(format!("{:.2}", 6.4), true), "6,40");
        assert_eq!(dec_mark(format!("{:.1}", 6.4), false), "6.4");
        assert_eq!((parse_dec(" 1,5 "), parse_dec("1.5"), parse_dec("x")), (Some(1.5), Some(1.5), None));
    }

    #[test]
    fn shown_numbers_use_the_decimal_comma_in_german() {
        cl_i18n::set_ui_lang(cl_i18n::Lang::De);
        assert_eq!(crate::props::fmt_bytes(1_920_000_000), "1,79 GB");
        assert_eq!(crate::props::fmt_bytes(5 << 20), "5,0 MB");
        assert_eq!(secs(1.0, 1), "1,0 s");
    }

    #[test]
    fn count_text_uses_singular_for_one() {
        assert_eq!(count_text(1, "{} Spur", "{} Spuren"), "1 Spur");
        assert_eq!(count_text(0, "{} Spur", "{} Spuren"), "0 Spuren");
        assert_eq!(count_text(2, "{} Highlight als Marker gesetzt", "{} Highlights als Marker gesetzt"), "2 Highlights als Marker gesetzt");
        assert_eq!(count_text(1, "{} Inhalt fehlt: {}", "{} Inhalte fehlen: {}"), "1 Inhalt fehlt: {}");
    }

    #[test]
    fn focus_ring_only_while_using_keyboard() {
        assert!(!keyboard_nav(false, false, false));
        assert!(keyboard_nav(false, true, false));
        assert!(keyboard_nav(true, false, false), "bleibt an bis zum Klick");
        assert!(!keyboard_nav(true, false, true));
    }

    #[test]
    fn elide_and_fit_stay_inside() {
        let w = |s: &str| s.chars().count() as f32 * 7.0;
        assert_eq!(elide("Daumen", 100.0, w), "Daumen");
        let e = elide("Subscribe, German (red)", 80.0, w);
        assert!(e.ends_with('…') && w(&e) <= 80.0 && e.starts_with("Subscribe"), "{e}");
        // Leerzeichen vor „…“ entfällt
        assert_eq!(elide("ab cdef", 28.0, w), "ab…");
        assert_eq!(elide("abc", 3.0, w), "");
        let m = elide_middle("C:/Users/zero/Videos/Urlaub", 70.0, w);
        assert!(m.starts_with("C:/U") && m.ends_with("laub") && m.contains('…') && w(&m) <= 70.0, "{m}");
        assert_eq!(elide_middle("kurz", 70.0, w), "kurz");
        assert_eq!(fit_size(20.0, 100.0, 150.0), 20.0);
        let s = fit_size(20.0, 200.0, 150.0);
        assert!((s - 15.0).abs() < 1e-4 && 200.0 * s / 20.0 <= 150.0 + 1e-3);
    }

    /// Blättern zeigt nie einen halben Eintrag neben dem Pfeil und erreicht jeden Eintrag.
    #[test]
    fn strip_never_shows_half_entries() {
        let (view, arrow) = (200.0, 20.0);
        for widths in [vec![50.0; 10], vec![30.0, 26.0, 26.0, 70.0, 64.0, 90.0, 58.0, 44.0], vec![260.0, 40.0, 40.0]] {
            let mut chips = Vec::new();
            let mut x = 0.0;
            for w in &widths {
                chips.push((x, x + w));
                x += w + 4.0;
            }
            // Inhalt: 2 px Rand + Abstand hinter dem letzten Eintrag (wie in der Leiste)
            let total = chips.last().unwrap().1 + 6.0;
            let whole = |off: f32| {
                let e = strip_edges(&chips, total, view, off, arrow);
                let lo = e.left.map_or(off, |l| l.0);
                let hi = e.right.map_or(off + view, |r| r.0);
                // jeder Eintrag liegt ganz im freien Bereich oder ganz unter einer Abdeckung
                for c in chips.iter().filter(|c| c.1 - c.0 <= view - 2.0 * arrow) {
                    assert!((c.0 >= lo - 0.1 && c.1 <= hi + 0.1) || c.1 <= lo + 2.1 || c.0 >= hi - 0.1, "{c:?} bei {off} ({lo}..{hi})");
                }
                e
            };
            let (mut off, mut seen, mut steps) = (0.0, Vec::new(), 0);
            loop {
                let e = whole(off);
                let lo = e.left.map_or(off, |l| l.0);
                let hi = e.right.map_or(off + view, |r| r.0);
                seen.extend(chips.iter().filter(|c| c.0 >= lo - 0.1 && c.1 <= hi + 0.1).map(|c| c.0 as i32));
                let Some((_, next)) = e.right else { break };
                assert!(next > off && next <= total - view + 0.1);
                off = next;
                steps += 1;
                assert!(steps < 50);
            }
            for c in chips.iter().filter(|c| c.1 - c.0 <= view - 2.0 * arrow) {
                assert!(seen.contains(&(c.0 as i32)), "{c:?} nie ganz sichtbar");
            }
            // zurück bis an den Anfang, auch von krummen Versätzen (Mausrad) aus
            for start in [off, 37.0, 111.5] {
                let mut off = start.min(total - view).max(0.0);
                while let Some((_, prev)) = whole(off).left {
                    assert!(prev < off);
                    off = prev;
                }
                assert_eq!(off, 0.0);
            }
        }
        // alles passt: keine Pfeile
        let e = strip_edges(&[(0.0, 50.0), (54.0, 104.0)], 110.0, 200.0, 0.0, 20.0);
        assert!(e.left.is_none() && e.right.is_none());
        // am Ende (per Pfeil oder Mausrad bis knapp davor) liegt der letzte Eintrag ganz im freien Bereich
        let mut x: u32 = 0x9e37_79b9;
        let mut rnd = |lo: f32, hi: f32| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            lo + (x % 10_000) as f32 / 10_000.0 * (hi - lo)
        };
        for _ in 0..3000 {
            let view = rnd(150.0, 350.0);
            let mut chips = Vec::new();
            let mut at = 0.0;
            for _ in 0..(rnd(2.0, 12.0) as usize) {
                let w = rnd(22.0, 120.0);
                chips.push((at, at + w));
                at += w + 4.0;
            }
            let last = *chips.last().unwrap();
            let total = last.1 + 6.0;
            if total <= view {
                continue;
            }
            let mut off = 0.0;
            for _ in 0..100 {
                match strip_edges(&chips, total, view, off, 20.0).right {
                    Some((_, next)) => off = next,
                    None => break,
                }
            }
            for o in [off, total - view - 0.4, total - view] {
                let e = strip_edges(&chips, total, view, o, 20.0);
                let hi = e.right.map_or(o + view, |r| r.0);
                assert!(last.1 <= hi + 0.01 || last.0 >= hi - 0.1, "letzter Eintrag {last:?} bei {o} angeschnitten (bis {hi})");
            }
        }
    }

    // Ein Test, weil der Akzent global ist (parallele Tests würden sich überschreiben)
    /// Vertrag des Design-Systems: 6 Schriftstufen, 3 Radien, 3 Abstände, jeweils aufsteigend
    #[test]
    fn tokens_match_brand_guide() {
        let fs = [FS_HINT, FS_UI, FS_TEXT, FS_TITLE, FS_PAGE, FS_HERO];
        assert_eq!(fs, [10.5, 11.5, 12.5, 14.0, 18.0, 26.0]);
        assert!(fs.windows(2).all(|w| w[0] < w[1]));
        let r = [R_CTRL, R_CARD, R_WIN];
        assert_eq!(r, [4.0, 6.0, 10.0]);
        assert!(r.windows(2).all(|w| w[0] < w[1]));
        let pad = [PAD_CARD, PAD_CARD_L, PAD_DIALOG];
        assert_eq!(pad, [8, 12, 16]);
        assert!(pad.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn accents_readable_on_all_palettes() {
        for (id, ..) in ACCENTS {
            set_accent(id);
            for p in &PALETTES {
                let a = accent_for(p);
                let (c, ci) = (contrast(a, p.bg_card), contrast(ink_for(p, a), a));
                let cr = contrast(a, p.bg_raised);
                println!("{id:7} {:10} {} card {c:.2} raised {cr:.2} ink {ci:.2}", p.id, a.to_hex());
                assert!(c >= 3.0, "{id} auf {}: {c}", p.id);
                assert!(cr >= 3.0, "{id} auf {} (raised): {cr}", p.id);
                assert!(ci >= 4.5, "Tinte {id} auf {}: {ci}", p.id);
            }
        }
        set_accent("#101010");
        for p in PALETTES.iter().filter(|p| p.dark) {
            assert!(contrast(accent_for(p), p.bg_card) >= 3.0, "{}", p.id);
            assert!(contrast(accent_for(p), p.bg_raised) >= 3.0, "{}", p.id);
        }
        set_accent("#FFFFFF");
        assert!(contrast(accent_for(&PALETTES[3]), PALETTES[3].bg_card) >= 3.0);
        set_accent("auto");
        assert_eq!(accent_for(&PALETTES[0]), PALETTES[0].accent);
        assert_eq!(ink_for(&PALETTES[3], PALETTES[3].accent), PALETTES[3].accent_ink);
    }

    /// Lesetext (`text_dim`) und Statustexte erreichen WCAG AA (4,5:1) in allen Designs
    #[test]
    fn text_readable_on_all_palettes() {
        for p in &PALETTES {
            for (bg, name) in [(p.bg_app, "app"), (p.bg_card, "card"), (p.bg_input, "input"), (p.bg_raised, "raised")] {
                let c = contrast(p.text_dim, bg);
                assert!(c >= 4.5, "text_dim auf {name} in {}: {c:.2}", p.id);
            }
            for (col, name) in [(danger_text(p), "danger"), (p.warn_text, "warn")] {
                for bg in [p.bg_card, p.bg_raised] {
                    let c = contrast(col, bg);
                    assert!(c >= 4.5, "{name} in {}: {c:.2}", p.id);
                }
            }
        }
    }

    /// Roter Knopf: weiße Schrift ≥ 4,5:1 in Ruhe, beim Überfahren und Drücken
    #[test]
    fn danger_button_readable() {
        for (t, down) in [(0.0, false), (1.0, false), (1.0, true)] {
            let c = contrast(Color32::WHITE, hover_fill(DANGER_FILL, t, down));
            assert!(c >= 4.5, "t {t} down {down}: {c:.2}");
        }
    }

    #[test]
    fn the_one_curve_eases_out_without_overshoot() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert_eq!(ease(-1.0), 0.0);
        assert_eq!(ease(2.0), 1.0);
        let mut last = 0.0;
        for i in 1..=100 {
            let v = ease(i as f32 / 100.0);
            assert!(v >= last && v <= 1.0, "{i}: {v}");
            last = v;
        }
        // sanft auslaufend: die erste Hälfte legt mehr als die Hälfte des Wegs zurück
        assert!(ease(0.5) > 0.8);
        // Gleiten: Anfang, Ende, nichts über das Ziel hinaus (auch rückwärts)
        assert_eq!(glide_at(10.0, 20.0, 0.0, MOTION), 10.0);
        assert_eq!(glide_at(10.0, 20.0, MOTION, MOTION), 20.0);
        assert_eq!(glide_at(10.0, 20.0, 5.0, MOTION), 20.0);
        for i in 0..=20 {
            let v = glide_at(20.0, 10.0, i as f32 / 100.0, MOTION);
            assert!((10.0..=20.0).contains(&v));
        }
    }

    #[test]
    fn three_durations_and_off_means_instant() {
        assert!(MOTION_FAST < MOTION && MOTION < MOTION_SLOW && MOTION_SLOW <= 0.24);
        // ohne den globalen Schalter umzulegen (parallele Tests lesen ihn)
        assert_eq!([MOTION_FAST, MOTION, MOTION_SLOW].map(|s| anim_if(false, s)), [0.0; 3]);
        assert_eq!(glide_at(0.0, 5.0, 0.0, anim_if(false, MOTION)), 5.0);
        // ohne Animationen: Lauflinie steht, Platzhalter ohne Schimmer
        assert_eq!(crate::widgets::busy_segment(0.7, false), (0.0, 1.0));
        assert_eq!(crate::widgets::busy_segment(3.1, false), crate::widgets::busy_segment(0.2, false));
        assert_eq!(crate::widgets::shimmer_at(1.0, false), None);
        assert_eq!(anim_if(true, MOTION), MOTION);
    }

    #[test]
    fn loader_waits_150_ms_then_stays_300_ms() {
        // kurze Arbeit bleibt unsichtbar
        assert!(!loader_visible(Some(0.0), None));
        assert!(!loader_visible(Some(0.149), None));
        assert!(!loader_visible(None, None));
        // ab 150 ms sichtbar
        assert!(loader_visible(Some(0.15), None));
        assert!(loader_visible(Some(2.0), Some(1.85)));
        // fertig kurz nach dem Erscheinen: steht noch bis 300 ms
        assert!(loader_visible(None, Some(0.05)));
        assert!(loader_visible(None, Some(0.299)));
        assert!(!loader_visible(None, Some(0.3)));
    }

    #[test]
    fn loader_state_over_frames() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("t");
        let at = |t: f64, busy: bool| {
            let mut vis = false;
            let input = egui::RawInput { time: Some(t), ..Default::default() };
            let _ = ctx.run(input, |ctx| vis = loading(ctx, id, busy));
            vis
        };
        // 100 ms Arbeit: nie sichtbar
        assert!(!at(0.0, true) && !at(0.05, true) && !at(0.1, false) && !at(0.2, false));
        // lange Arbeit: ab 150 ms sichtbar, nach dem Ende noch bis 300 ms nach dem Erscheinen
        assert!(!at(1.0, true));
        assert!(at(1.16, true));
        assert!(at(1.2, false));
        assert!(at(1.4, false));
        assert!(!at(1.47, false));
    }

    #[test]
    fn busy_line_and_shimmer_move_calmly() {
        use crate::widgets::{busy_segment, shimmer_at};
        for i in 0..200 {
            let (a, b) = busy_segment(i as f64 * 0.013, true);
            assert!((0.0..=1.0).contains(&a) && (0.0..=1.0).contains(&b) && a <= b, "{i}: {a} {b}");
        }
        // Stück wandert nach rechts
        assert!(busy_segment(0.5, true).0 > busy_segment(0.2, true).0);
        // Schimmer erst nach 150 ms, nach einigen Sekunden ruhige Fläche
        assert_eq!(shimmer_at(0.1, true), None);
        assert!(shimmer_at(0.5, true).is_some());
        assert_eq!(shimmer_at(10.0, true), None);
    }
}
