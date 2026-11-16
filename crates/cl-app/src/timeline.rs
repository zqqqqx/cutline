//! Zeitleiste:
//! - kurzer Klick ins Leere setzt den Playhead (begrenzt aufs Projektende), gedrückt halten
//!   und ziehen zieht einen Auswahlrahmen auf; Lineal und Playhead-Griff scrubben
//! - Playhead und Clips rasten an Clipkanten (alle Spuren) und am Playhead ein
//! - Clips ziehen: auf andere Spuren derselben Art, ins Leere darüber/darunter = neue Spur; über
//!   einer unpassenden oder gesperrten Spur entsteht daneben eine neue (Einfüge-Linie)
//! - Spurkopf gedrückt halten und ziehen ordnet die Spuren um; „+“ über den Spurköpfen legt eine an
//! - Strg/Umschalt + Klick = Mehrfachauswahl, Rechtsklick-Menüs für Clip, Spur und Leerraum
//! - Übergänge als Block auf dem Schnitt, Keyframe-Rauten, Live-Waveform beim Einsprechen
//! - Mausrad scrollt die Spuren senkrecht, Umschalt+Rad waagerecht, Strg+Rad zoomt

use std::collections::{HashMap, HashSet};

use cl_core::*;
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect, RichText, Sense, Stroke};

use crate::app::{App, DragAsset};
use crate::keymap::with_key;

use crate::preview::WAVE_PER_SEC;
use crate::theme::{self, icon};
use crate::widgets::Slider;

const HEADER_W: f32 = 172.0;
const RULER_H: f32 = 22.0;
const ROW_H: f32 = 34.0;
const MAIN_ROW_H: f32 = 58.0;
const AUDIO_ROW_H: f32 = 42.0;
/// Höhe einer eingeklappten Spur
const COLLAPSED_H: f32 = 16.0;
const ROW_GAP: f32 = 5.0;
const EDGE_PX: f32 = 7.0;
const SNAP_PX: f32 = 9.0;
const BAND_H: f32 = 14.0;
const REC_ROW: usize = usize::MAX;

#[derive(Clone, PartialEq)]
enum Drag {
    None,
    Scrub,
    /// `ids`: alle bewegten Clips mit Startzeit; `grab` = Abstand Zeiger ↔ Clipanfang
    Move {
        clip: ClipId,
        grab: f64,
        ids: Vec<(ClipId, f64)>,
    },
    Trim {
        clip: ClipId,
        edge: Edge,
    },
    /// Alt+Kante zwischen zwei Clips: Schnitt verschieben
    Roll {
        left: ClipId,
        right: ClipId,
    },
    /// Alt+Clip: Quellausschnitt verschieben (`src` = Quellbeginn beim Start, `x` = Startposition)
    Slip {
        clip: ClipId,
        src: f64,
        x: f32,
    },
    Marquee {
        origin: Pos2,
        additive: bool,
    },
    /// Fade-Griff oder Lautstärkelinie auf einem Clip mit Ton (`value`: Sekunden bzw. dB)
    Audio {
        clip: ClipId,
        handle: AudioHandle,
        value: f32,
    },
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum AudioHandle {
    FadeIn,
    FadeOut,
    Volume,
}

/// Lautstärke −60…+12 dB (Zeitleiste und Inspektor). Die Linie ist nicht linear: die oberen
/// zwei Drittel decken −12…+12 dB ab (dort wird fein gemischt), das untere Drittel −60…−12 dB.
const VOL_MIN: f32 = -60.0;
pub const VOL_MAX: f32 = 12.0;
const VOL_KNEE: f32 = -12.0;
/// dB → Anteil der Höhe (0 = unten, 1 = oben)
fn vol_norm(db: f32) -> f32 {
    let db = db.clamp(VOL_MIN, VOL_MAX);
    if db >= VOL_KNEE {
        1.0 / 3.0 + (db - VOL_KNEE) / (VOL_MAX - VOL_KNEE) * 2.0 / 3.0
    } else {
        (db - VOL_MIN) / (VOL_KNEE - VOL_MIN) / 3.0
    }
}
/// Anteil der Höhe → dB (Umkehrung von `vol_norm`)
fn norm_vol(n: f32) -> f32 {
    let n = n.clamp(0.0, 1.0);
    if n >= 1.0 / 3.0 {
        VOL_KNEE + (n - 1.0 / 3.0) * 1.5 * (VOL_MAX - VOL_KNEE)
    } else {
        VOL_MIN + n * 3.0 * (VOL_KNEE - VOL_MIN)
    }
}
fn vol_y(body: Rect, db: f32) -> f32 {
    body.bottom() - 3.0 - vol_norm(db) * (body.height() - 6.0)
}
/// Beschriftung eines Hauptstrichs im Lineal: mm:ss, bei Schritten unter 1 s mit so vielen
/// Nachkommastellen, wie der Schritt braucht (0,25 s → zwei). Sekunden abgeschnitten, nicht gerundet.
fn ruler_label(t: f64, step: f64) -> String {
    let digits = if step >= 1.0 { 0 } else if (step * 10.0).fract().abs() < 1e-9 { 1 } else { 2 };
    let unit = 10i64.pow(digits);
    let n = (t.max(0.0) * unit as f64).round() as i64;
    let secs = n / unit;
    // ab einer Stunde h:mm:ss
    let mm_ss = if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{:02}:{:02}", secs / 60, secs % 60)
    };
    match digits {
        0 => mm_ss,
        d => format!("{mm_ss}.{:0w$}", n % unit, w = d as usize),
    }
}

/// Schritte der Hauptstriche im Lineal (s): bis zu Stunden für weit herausgezoomte, lange Projekte
const RULER_STEPS: [f64; 22] = [
    0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0,
    10_800.0, 21_600.0, 43_200.0, 86_400.0, 172_800.0,
];

/// Kleinster Schritt, dessen Beschriftungen bei `pps` mindestens 70 px auseinanderliegen
fn ruler_step(pps: f32) -> f64 {
    RULER_STEPS.into_iter().find(|s| *s as f32 * pps >= 70.0).unwrap_or(RULER_STEPS[RULER_STEPS.len() - 1])
}

/// Größter Maßstab (px je Sekunde)
const PPS_MAX: f32 = 400.0;

/// Kleinster Maßstab bei sichtbarer Breite `width` (px) und Projektlänge `dur` (s): das ganze
/// Projekt (leer bzw. kurz: 5 Minuten) belegt höchstens die Hälfte der Breite, rechts bleibt
/// freier Platz. Nie enger als der frühere feste Wert von 2 px/s.
fn min_pps(dur: f64, width: f32) -> f32 {
    (0.5 * width.max(1.0) / dur.max(300.0) as f32).min(2.0)
}

/// „Ganze Zeitleiste zeigen“: (Maßstab, Bildlauf) mit 5 % Rand links und rechts
fn fit_all(dur: f64, width: f32, lo: f32) -> (f32, f64) {
    let d = dur.max(1.0);
    let pps = (width.max(1.0) / (d * 1.1) as f32).clamp(lo, PPS_MAX);
    (pps, -0.05 * d)
}

/// Zoom-Regler: Maßstab → Stellung 0…1 (logarithmisch zwischen `lo` und `PPS_MAX`)
fn zoom_norm(pps: f32, lo: f32) -> f32 {
    ((pps.ln() - lo.ln()) / (PPS_MAX.ln() - lo.ln())).clamp(0.0, 1.0)
}

/// Zoom-Regler: Stellung 0…1 → Maßstab
fn norm_zoom(n: f32, lo: f32) -> f32 {
    (lo.ln() + n.clamp(0.0, 1.0) * (PPS_MAX.ln() - lo.ln())).exp()
}

/// Was beim weichen Zoomen fest bleibt
#[derive(Clone, Copy, Debug, PartialEq)]
enum Anchor {
    /// Zeit `t` bleibt `x` px rechts der Spurkante (unter dem Zeiger); Bildlauf nicht unter `floor`
    At { t: f64, x: f32, floor: f64 },
    /// Bildlauf gleitet von → nach
    Scroll(f64, f64),
}

/// Weicher Wechsel von Maßstab und Bildlauf (Zoom, „Ganze Zeitleiste zeigen“, Playhead folgen)
#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewGlide {
    pps: (f32, f32),
    anchor: Anchor,
    t0: f64,
}

impl ViewGlide {
    /// (Maßstab, Bildlauf, fertig) `age` s nach dem Start; Maßstab gleitet logarithmisch
    fn at(&self, age: f32, secs: f32) -> (f32, f64, bool) {
        let k = if secs <= 0.0 { 1.0 } else { theme::ease(age / secs) };
        let pps = if k >= 1.0 || self.pps.0 == self.pps.1 { self.pps.1 } else { (self.pps.0.ln() + (self.pps.1.ln() - self.pps.0.ln()) * k).exp() };
        let scroll = match self.anchor {
            Anchor::At { t, x, floor } => (t - (x / pps) as f64).max(floor),
            Anchor::Scroll(a, b) => a + (b - a) * k as f64,
        };
        (pps, scroll, k >= 1.0)
    }
}

// ---------- Bewegung der Clips und Spuren ----------

/// Höchstens so viele Clips gleiten (bzw. blenden ein/aus) gleichzeitig; bei mehr springen sie
/// sofort an ihren Platz, damit die Zeitleiste auch dann unter 8 ms je Bild bleibt.
const MAX_GLIDES: usize = 300;

/// Lage eines Clips im Modell
#[derive(Clone, Copy, PartialEq, Debug)]
struct Place {
    s: f64,
    e: f64,
    track: TrackId,
}

/// Gleiten eines Clips von seiner alten zur neuen Lage: Start (Beginn, Ende), senkrechter Versatz
/// am Start (Spurwechsel, px; gleitet zu 0) und Startzeit.
#[derive(Clone, Copy, PartialEq, Debug)]
struct ClipGlide {
    s0: f64,
    e0: f64,
    dy0: f32,
    t0: f64,
}

impl ClipGlide {
    /// Angezeigte Lage `age` s nach dem Start: (Beginn, Ende, Versatz); `secs` = 0 → sofort am Ziel
    fn at(&self, to: Place, age: f32, secs: f32) -> (f64, f64, f32) {
        if secs <= 0.0 || age >= secs {
            return (to.s, to.e, 0.0);
        }
        let k = theme::ease(age / secs);
        let k64 = k as f64;
        (self.s0 + (to.s - self.s0) * k64, self.e0 + (to.e - self.e0) * k64, self.dy0 * (1.0 - k))
    }
}

/// Was sich zwischen zwei Projektständen bewegt hat (je sortiert)
#[derive(Default, Debug, PartialEq)]
struct Changes {
    moved: Vec<ClipId>,
    born: Vec<ClipId>,
    gone: Vec<ClipId>,
}

fn places(tracks: &[Track]) -> HashMap<ClipId, Place> {
    tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(move |c| (c.id, Place { s: c.start.as_secs_f64(), e: c.end().as_secs_f64(), track: t.id })))
        .collect()
}

/// Bewegte, neue und gelöschte Clips. Nur der Beginn oder die Spur zählt als „bewegt“ (eine
/// geänderte Länge bei gleichem Beginn, etwa beim Teilen, springt). Neue Clips an einer Stelle, wo
/// vorher ein Clip derselben Spur lag (Teilen, Ersetzen), blenden nicht ein. Gar nichts, wenn
/// es ein anderes Projekt ist (kein gemeinsamer Clip); je Art nichts über `MAX_GLIDES`.
fn changes(prev: &HashMap<ClipId, Place>, next: &HashMap<ClipId, Place>, skip: &HashSet<ClipId>) -> Changes {
    if prev.is_empty() || (!next.is_empty() && !next.keys().any(|k| prev.contains_key(k))) {
        return Changes::default();
    }
    let mut ch = Changes::default();
    for (id, p) in next {
        match prev.get(id) {
            Some(q) if ((q.s - p.s).abs() > 1e-6 || q.track != p.track) && !skip.contains(id) => ch.moved.push(*id),
            Some(_) => {}
            None => ch.born.push(*id),
        }
    }
    ch.gone = prev.keys().filter(|k| !next.contains_key(k)).copied().collect();
    if ch.born.len() <= MAX_GLIDES {
        ch.born.retain(|id| {
            let p = next[id];
            !prev.values().any(|q| q.track == p.track && q.s < p.e && p.s < q.e)
        });
    }
    for v in [&mut ch.moved, &mut ch.born, &mut ch.gone] {
        if v.len() > MAX_GLIDES {
            v.clear();
        }
        v.sort();
    }
    ch
}

/// Gelöschter Clip, der mit seinem letzten Aussehen kurz ausblendet
struct GoneClip {
    clip: Clip,
    /// Spur (ohne Clips) für Farbe/Sperre
    track: Track,
    s: f64,
    e: f64,
    top: f32,
    h: f32,
    t0: f64,
}

/// Höhe einer Spur, die gleitet: (von, nach, Start)
type HeightGlide = (f32, f32, f64);

/// Bewegungszustand der Zeitleiste
#[derive(Default)]
struct Motion {
    places: HashMap<ClipId, Place>,
    glides: HashMap<ClipId, ClipGlide>,
    born: HashMap<ClipId, f64>,
    gone: Vec<GoneClip>,
    /// vom Nutzer selbst gezogen/getrimmt: folgen dem Zeiger, gleiten nach dem Loslassen nicht
    skip: HashSet<ClipId>,
    /// Oberkante (Bildschirm) und Höhe je Spur im letzten Bild
    rows: HashMap<TrackId, (f32, f32)>,
    /// Spuren im letzten Bild in Anzeigereihenfolge
    order: Vec<TrackId>,
    heights: HashMap<TrackId, HeightGlide>,
    /// weggefallene Spur: Platz über der Spur `anchor` (`None` = unten), schrumpft von `h` auf 0
    spacers: Vec<(Option<TrackId>, f32, f64)>,
}

impl Motion {
    /// Neuer Projektstand `next` (vorher `prev`); `row_top` = Oberkante je Spur in diesem Bild
    fn update(&mut self, prev: &[Track], next: &[Track], row_top: &HashMap<TrackId, f32>, now: f64) {
        let target = places(next);
        let ch = changes(&self.places, &target, &self.skip);
        let (secs, fast) = (theme::anim(theme::MOTION), theme::anim(theme::MOTION_FAST));
        if ch == Changes::default() && !self.places.is_empty() && !target.keys().any(|k| self.places.contains_key(k)) {
            // anderes Projekt: laufende Bewegungen verwerfen
            self.glides.clear();
            self.born.clear();
            self.gone.clear();
        }
        // angezeigte Lage jetzt (läuft schon eine Bewegung, startet die neue dort)
        let shown = |m: &Self, id: ClipId, p: Place| match m.glides.get(&id) {
            Some(g) => g.at(p, (now - g.t0) as f32, secs),
            None => (p.s, p.e, 0.0),
        };
        if secs > 0.0 {
            for id in &ch.moved {
                let (from, to) = (self.places[id], target[id]);
                let (s0, e0, dy) = shown(self, *id, from);
                let old_top = self.rows.get(&from.track).map_or(0.0, |r| r.0);
                let new_top = row_top.get(&to.track).copied().unwrap_or(old_top);
                let dy0 = if from.track != to.track { old_top + dy - new_top } else { dy };
                self.glides.insert(*id, ClipGlide { s0, e0, dy0, t0: now });
            }
        }
        if fast > 0.0 {
            for id in &ch.born {
                self.born.insert(*id, now);
            }
            for id in &ch.gone {
                let from = self.places[id];
                let Some((t, c)) = prev.iter().find_map(|t| t.clips.iter().find(|c| c.id == *id).map(|c| (t, c))) else {
                    continue;
                };
                let (s, e, dy) = shown(self, *id, from);
                let (top, h) = self.rows.get(&t.id).copied().unwrap_or((0.0, 0.0));
                let track = Track { clips: Vec::new(), ..t.clone() };
                self.gone.push(GoneClip { clip: c.clone(), track, s, e, top: top + dy, h, t0: now });
            }
        }
        for id in &ch.gone {
            self.glides.remove(id);
        }
        self.places = target;
        self.skip.clear();
    }
}

/// Angezeigte Höhe einer Spur mit Ziel `to`: neues Ziel startet am aktuellen Stand; eine neue Spur
/// (`appear`) wächst von 0. Gibt (Zustand, Höhe) zurück.
fn height_at(state: Option<HeightGlide>, to: f32, appear: bool, now: f64, secs: f32) -> (HeightGlide, f32) {
    let cur = |g: HeightGlide| theme::glide_at(g.0, g.1, (now - g.2) as f32, secs);
    let g = match state {
        Some(g) if g.1 == to => g,
        Some(g) => (cur(g), to, now),
        None if appear => (0.0, to, now),
        None => (to, to, now),
    };
    (g, cur(g))
}

/// Alpha eines ein- bzw. ausblendenden Clips `age` s nach dem Start
fn fade_alpha(age: f32, secs: f32, appear: bool) -> f32 {
    let k = if secs <= 0.0 { 1.0 } else { theme::ease(age / secs) };
    if appear {
        k
    } else {
        1.0 - k
    }
}

/// Zeiger greift die Lautstärkelinie nur dicht daran; auf flachen Clips (Tonspur) liegt die
/// Linie sonst mitten im Clip und Ziehen in der Mitte verschiebt den Clip nicht mehr.
fn vol_line_hit(body: Rect, db: f32, y: f32) -> bool {
    (y - vol_y(body, db)).abs() <= (body.height() / 10.0).clamp(1.5, 4.0)
}
/// Text im Clipkopf (Vorbild von `clip_head_measured`): Name, Kennzeichen, Dauer. Wird es eng,
/// fallen zuerst die Kennzeichen weg,
/// dann wird der Name mit „…“ gekürzt; die Dauer bleibt so lange wie möglich sichtbar.
#[cfg(test)]
fn clip_head(name: &str, tags: &str, dur: &str, max_w: f32, width: impl Fn(&str) -> f32) -> String {
    let sep = if dur.is_empty() { "" } else { "  " };
    let full = format!("{name}{tags}{sep}{dur}");
    if width(&full) <= max_w {
        return full;
    }
    let tail = format!("{sep}{dur}");
    let short = theme::elide(name, max_w - width(&tail), &width);
    if !short.is_empty() {
        return format!("{short}{tail}");
    }
    theme::elide(name, max_w, &width)
}

/// Text mit vorab gemessenen Zeichenbreiten: kürzt wie `theme::elide` bzw. `clip_head`, ohne je
/// Versuch neu zu setzen. Rechnet wie egui: Unterschneidung zum vorigen Zeichen, Zeichen auf ganze
/// Pixel gerundet, die Breite endet mit dem ungerundeten Vorschub des letzten Zeichens.
#[derive(Clone, Debug, PartialEq)]
struct Measured {
    chars: Vec<char>,
    /// Breite der ersten k Zeichen (k = 0…n)
    ends: Vec<f32>,
    /// Breite der ersten k Zeichen mit „…“ dahinter
    ell_at: Vec<f32>,
}

impl Measured {
    fn new(text: &str, ppp: f32, adv: impl Fn(char) -> f32, mut kern: impl FnMut(char, char) -> f32) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let ell = adv('…');
        let (mut ends, mut ell_at) = (Vec::with_capacity(chars.len() + 1), Vec::with_capacity(chars.len() + 1));
        ends.push(0.0);
        ell_at.push(ell);
        let mut x = 0.0;
        for (i, &c) in chars.iter().enumerate() {
            if i > 0 {
                x += kern(chars[i - 1], c);
            }
            let a = adv(c);
            ends.push(x + a);
            x = round_px(x + a, ppp);
            ell_at.push(x + kern(c, '…') + ell);
        }
        Self { chars, ends, ell_at }
    }
    fn width(&self) -> f32 {
        *self.ends.last().unwrap_or(&0.0)
    }
    fn text(&self) -> String {
        self.chars.iter().collect()
    }
    /// Wie `theme::elide` für die ersten `n` Zeichen
    fn cut(&self, n: usize, max_w: f32) -> String {
        if self.ends[n] <= max_w {
            return self.chars[..n].iter().collect();
        }
        (0..n)
            .rev()
            .map(|m| m - self.chars[..m].iter().rev().take_while(|c| c.is_whitespace()).count())
            .find(|k| self.ell_at[*k] <= max_w)
            .map(|k| self.chars[..k].iter().chain(['…'].iter()).collect())
            .unwrap_or_default()
    }
}

/// Unterschneidung je Zeichenpaar und Schrift, einmal gemessen (egui gibt sie nicht heraus):
/// Breite von „ab“ minus gerundeter Vorschub von a minus Vorschub von b.
#[derive(Default)]
struct Kerning(HashMap<(u32, egui::FontFamily, char, char), f32>);

/// Höchstens so viele Zeichenpaare (darüber: neu beginnen)
const KERN_CAP: usize = 50_000;

impl Kerning {
    fn measured(&mut self, f: &egui::epaint::Fonts, font: &egui::FontId, text: &str) -> Measured {
        let ppp = f.pixels_per_point();
        if self.0.len() > KERN_CAP {
            self.0.clear();
        }
        Measured::new(text, ppp, |c| f.glyph_width(font, c), |a, b| {
            *self.0.entry((font.size.to_bits(), font.family.clone(), a, b)).or_insert_with(|| {
                let w = f.layout_no_wrap(format!("{a}{b}"), font.clone(), Color32::WHITE).size().x;
                w - round_px(f.glyph_width(font, a), ppp) - f.glyph_width(font, b)
            })
        })
    }
}

fn round_px(x: f32, ppp: f32) -> f32 {
    (x * ppp).round() / ppp
}

/// Wellenform eines Clips: Lage, Ausschnitt, Zoom, Lautstärke und Daten, für die das Netz gilt
#[derive(Clone, Copy, PartialEq, Debug)]
struct WaveKey {
    geo: [u32; 12],
    peaks: (usize, usize),
    /// Spur stumm: Wellenform blass (gehört zum Aussehen des Netzes)
    muted: bool,
}

/// Wellenform-Netze je Clip (aus dem Zwischenspeicher geliehen) und ein Tessellierer für dieses Bild
struct WaveMeshes {
    map: HashMap<ClipId, (WaveKey, std::sync::Arc<egui::Mesh>)>,
    tess: Option<egui::epaint::Tessellator>,
}

impl WaveMeshes {
    /// Formen genau so zu einem Netz machen, wie es egui beim Zeichnen täte
    fn tessellate(&mut self, ctx: &egui::Context, clip: Rect, shapes: Vec<egui::Shape>) -> std::sync::Arc<egui::Mesh> {
        let t = self.tess.get_or_insert_with(|| {
            let opts = ctx.tessellation_options(|o| *o);
            let (size, discs) = ctx.fonts(|f| (f.font_image_size(), f.texture_atlas().lock().prepared_discs()));
            egui::epaint::Tessellator::new(ctx.pixels_per_point(), opts, size, discs)
        });
        t.set_clip_rect(clip);
        let mut mesh = egui::Mesh::default();
        for s in shapes {
            t.tessellate_shape(s, &mut mesh);
        }
        std::sync::Arc::new(mesh)
    }
}

/// Projektstand, für den der Zwischenspeicher gilt: Änderung am Projekt, Sprache, Skalierung
#[derive(Clone, Copy, PartialEq, Debug)]
struct CacheKey {
    epoch: u64,
    lang: cl_i18n::Lang,
    ppp: f32,
}

/// Höchstens so viele Clip-Texte im Zwischenspeicher (darüber: neu beginnen)
const HEAD_CAP: usize = 20_000;

/// Texte eines Clips: Kopfzeile und (Textclips) Text im Clip, gemessen; dazu der zuletzt
/// gezeigte, fertig gesetzte Stand je Breite.
struct Head {
    full: Measured,
    name_n: usize,
    tail: Measured,
    body: Option<(egui::FontFamily, Measured)>,
    shown: Option<(f32, std::sync::Arc<egui::Galley>)>,
    body_shown: Option<(f32, std::sync::Arc<egui::Galley>)>,
}

/// Zwischenspeicher der Zeitleiste für einen Projektstand: Spuren (statt Klon je Bild),
/// fehlende Dateien (statt Dateiprüfung je Clip und Bild), Clip-Texte (statt Setzen je Bild).
#[derive(Default)]
struct ViewCache {
    key: Option<CacheKey>,
    tracks: std::sync::Arc<Vec<Track>>,
    missing: HashSet<MediaId>,
    heads: HashMap<ClipId, Head>,
    kern: Kerning,
    waves: HashMap<ClipId, (WaveKey, std::sync::Arc<egui::Mesh>)>,
}

/// Fehlende Medien laut Hintergrundprüfung ([`crate::loading::Presence`]): keine Dateizugriffe je Bild
fn missing_media(media: &[MediaRef], presence: &crate::loading::Presence) -> HashSet<MediaId> {
    media.iter().filter(|m| !presence.get(&m.path)).map(|m| m.id).collect()
}

impl ViewCache {
    /// Auf Stand `key` bringen; `true`, wenn sich der Stand geändert hat (alles neu).
    fn sync(&mut self, key: CacheKey, tracks: impl FnOnce() -> Vec<Track>) -> bool {
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        self.tracks = std::sync::Arc::new(tracks());
        self.heads.clear();
        self.kern.0.clear();
        self.waves.clear();
        true
    }
    /// Eintrag für `id` (neu mit `make`); bei voller Obergrenze wird zuerst geleert.
    fn head(&mut self, id: ClipId, make: impl FnOnce(&mut Kerning) -> Head) -> &mut Head {
        if self.heads.len() >= HEAD_CAP && !self.heads.contains_key(&id) {
            self.heads.clear();
        }
        let Self { heads, kern, .. } = self;
        heads.entry(id).or_insert_with(|| make(kern))
    }
}

/// `clip_head` über vorab gemessene Teile: `full` = Name + Kennzeichen + Abstand + Dauer,
/// `name_n` = Zeichen des Namens, `tail` = Abstand + Dauer.
fn clip_head_measured(full: &Measured, name_n: usize, tail: &Measured, max_w: f32) -> String {
    if full.width() <= max_w {
        return full.text();
    }
    let short = full.cut(name_n, max_w - tail.width());
    if !short.is_empty() {
        return short + &tail.text();
    }
    full.cut(name_n, max_w)
}

fn y_vol(body: Rect, y: f32) -> f32 {
    let db = norm_vol((body.bottom() - 3.0 - y) / (body.height() - 6.0).max(1.0));
    // bei 0 dB leicht einrasten
    if db.abs() < 0.8 { 0.0 } else { (db * 2.0).round() / 2.0 }
}

/// Längste erlaubte Blende: Cliplänge minus die andere Blende (Zeitleiste und Inspektor).
pub fn fade_max(duration: f64, other_fade: f64) -> f64 {
    (duration - other_fade).max(0.0)
}

/// Werkzeug in der Zeitleiste (wie in CapCut: A = Auswählen, B = Klinge)
#[derive(Clone, Copy, PartialEq, Default)]
pub enum Tool {
    #[default]
    Select,
    Blade,
    SelectLeft,
    SelectRight,
}

impl Tool {
    pub const ALL: [(Tool, &'static str, &'static str, &'static str); 4] = [
        (Tool::Select, icon::CURSOR, "Auswählen", "A"),
        (Tool::Blade, icon::KNIFE, "Klinge", "B"),
        (Tool::SelectLeft, icon::ARROW_FAT_LINES_LEFT, "Alle Clips links auswählen", ""),
        (Tool::SelectRight, icon::ARROW_FAT_LINES_RIGHT, "Alle Clips rechts auswählen", ""),
    ];
    fn icon(self) -> &'static str {
        Self::ALL.iter().find(|t| t.0 == self).map(|t| t.1).unwrap_or(icon::CURSOR)
    }
}

/// Spurarten zum Hinzufügen (Symbol, Name)
pub const TRACK_KINDS: [(TrackKind, &str); 5] = [
    (TrackKind::Video, "Videospur"),
    (TrackKind::Text, "Textspur"),
    (TrackKind::Effect, "Effektspur"),
    (TrackKind::Filter, "Filterspur"),
    (TrackKind::Audio, "Audiospur"),
];

#[derive(Clone, Copy, PartialEq)]
enum CtxTarget {
    Clip(ClipId),
    Track(TrackId),
    Empty(Option<TrackId>),
    Transition(ClipId),
    /// Lineal; mit Marker-ID, wenn der Rechtsklick einen Marker traf
    Ruler(Option<u64>),
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    Same,
    Track(usize),
    /// Neue Spur(en) neben Anzeige-Zeile (Zeile, obere Hälfte)
    New(usize, bool),
}

/// Spurkopf gedrückt: Spur, Druckpunkt, Zeitpunkt, schon angehoben?
#[derive(Clone, Copy)]
struct TrackDrag {
    track: TrackId,
    origin: Pos2,
    t0: f64,
    lifted: bool,
}

/// Spurkopf gedrückt gehalten: ab dieser Bewegung (px) oder Haltezeit (s) hebt sich die Spur
/// zum Verschieben; darunter bleibt es ein Klick.
fn track_lifted(moved: f32, held: f64) -> bool {
    moved >= 4.0 || held >= 0.15
}

/// Ziehen an einem schon ausgewählten Clip: er wird Hauptauswahl, die bisherige Hauptauswahl
/// bleibt ausgewählt (sonst fiele sie aus der Auswahl und bliebe beim Verschieben liegen).
fn make_primary(selected: &mut Option<ClipId>, extra: &mut HashSet<ClipId>, c: ClipId) {
    if *selected == Some(c) {
        return;
    }
    if let Some(old) = selected.replace(c) {
        extra.insert(old);
    }
    extra.remove(&c);
}

/// Wie weit (px) muss senkrecht gerollt werden, damit `row` ganz in `lanes` liegt (4 px Rand)?
fn reveal_delta(row: Rect, lanes: Rect) -> f32 {
    if row.top() < lanes.top() + 4.0 {
        row.top() - lanes.top() - 4.0
    } else if row.bottom() > lanes.bottom() - 4.0 {
        (row.bottom() - lanes.bottom() + 4.0).min(row.top() - lanes.top() - 4.0)
    } else {
        0.0
    }
}

/// Zeit (s) an der Fensterstelle `x`: Spurkante `x0`, Bildlauf `scroll` (s), Maßstab `pps` (px/s)
fn time_at(x: f32, x0: f32, scroll: f64, pps: f32) -> f64 {
    (((x - x0) / pps) as f64 + scroll).max(0.0)
}

/// Spurart, die ein Element aus dem Medien-Menü braucht (`None`: kein eigener Clip, z. B. Übergang).
fn asset_kind(p: &Project, a: &DragAsset) -> Option<TrackKind> {
    match a {
        DragAsset::Media(m) => p.media(*m).map(|m| if m.kind == MediaKind::Audio { TrackKind::Audio } else { TrackKind::Video }),
        DragAsset::Text(_) => Some(TrackKind::Text),
        DragAsset::Sticker(_) | DragAsset::OwnSticker(_) => Some(TrackKind::Video),
        DragAsset::Sfx(_) | DragAsset::File(_) => Some(TrackKind::Audio),
        DragAsset::Effect(_) => Some(TrackKind::Effect),
        DragAsset::Filter(_) => Some(TrackKind::Filter),
        DragAsset::Transition(_) => None,
    }
}

/// Anzahl der Schalter im Spurkopf (Sperren, Auge, Ton, Solo); eingeklappt keine.
fn toggle_count(t: &Track, collapsed: bool) -> usize {
    if collapsed {
        return 0;
    }
    1 + usize::from(t.kind != TrackKind::Audio) + 2 * usize::from(matches!(t.kind, TrackKind::Video | TrackKind::Audio))
}

/// Abstand des Spurnamens vom linken Rand des Spurkopfs: für alle Spurarten (auch eingeklappt)
/// gleich – Platz für die größte Schalterzahl bleibt frei, fehlende Schalter bleiben leer.
fn name_column() -> f32 {
    let most = TRACK_KINDS.iter().map(|(k, _)| toggle_count(&Track::new(*k), false)).max().unwrap_or(0);
    32.0 + most as f32 * 19.0 + 4.0
}

pub struct TimelineState {
    pub pps: f32,
    pub scroll: f64,
    pub vscroll: f32,
    drag: Drag,
    ctx: Option<CtxTarget>,
    /// Höhenfaktor je Spur (Rechtsklick → Spurhöhe)
    pub heights: HashMap<TrackId, f32>,
    /// Eingeklappte Spuren (schmal, bleiben aber sichtbar)
    pub collapsed: std::collections::HashSet<TrackId>,
    last_click: Option<Pos2>,
    /// Playhead im letzten Bild (Sprünge per Taste → Ansicht folgt)
    last_playhead: Option<Time>,
    pub tool: Tool,
    track_drag: Option<TrackDrag>,
    cache: ViewCache,
    /// laufender weicher Zoom/Bildlauf
    view_glide: Option<ViewGlide>,
    /// sichtbare Breite der Spuren im letzten Bild (für die Zoom-Grenzen der Werkzeugleiste)
    view_w: f32,
    motion: Motion,
    /// neue Spur, die sichtbar gerollt wird: (Spur, Start, senkrechter Bildlauf beim Start)
    reveal: Option<(TrackId, f64, f32)>,
    /// Zeit (egui) des letzten Bilds
    now: f64,
}

impl TimelineState {
    /// Maßstab, auf den gerade gezoomt wird (Ziel einer laufenden Bewegung)
    fn target_pps(&self) -> f32 {
        self.view_glide.map_or(self.pps, |g| g.pps.1)
    }
    /// Maßstab/Bildlauf weich ändern (Animationen aus: sofort)
    fn glide_view(&mut self, pps: f32, anchor: Anchor, now: f64) {
        let g = ViewGlide { pps: (self.pps, pps), anchor, t0: now };
        self.view_glide = Some(g);
        self.step_view(now);
    }
    /// Zoomen um den Punkt `x` px rechts der Spurkante
    fn zoom_to(&mut self, pps: f32, x: f32, now: f64) {
        let t = self.scroll + (x / self.pps) as f64;
        self.glide_view(pps, Anchor::At { t, x, floor: self.scroll.min(0.0) }, now);
    }
    /// „Ganze Zeitleiste zeigen“: Projekt der Länge `dur` mit Rand einpassen (weich)
    fn show_all(&mut self, dur: f64, now: f64) {
        let (pps, scroll) = fit_all(dur, self.view_w, min_pps(dur, self.view_w));
        self.glide_view(pps, Anchor::Scroll(self.scroll, scroll), now);
    }
    /// Bildlauf weich zu `scroll` (Maßstab bleibt)
    fn scroll_to(&mut self, scroll: f64, now: f64) {
        self.glide_view(self.pps, Anchor::Scroll(self.scroll, scroll), now);
    }
    /// Neue Spur weich sichtbar rollen
    pub(crate) fn reveal_track(&mut self, t: TrackId) {
        self.reveal = Some((t, self.now, self.vscroll));
    }
    /// Laufende Bewegung fortschreiben; `true` = läuft noch
    fn step_view(&mut self, now: f64) -> bool {
        let Some(g) = self.view_glide else {
            return false;
        };
        let (pps, scroll, done) = g.at((now - g.t0) as f32, theme::anim(theme::MOTION));
        self.pps = pps;
        self.scroll = scroll;
        if done {
            self.view_glide = None;
        }
        !done
    }
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            pps: 60.0,
            scroll: 0.0,
            vscroll: 0.0,
            drag: Drag::None,
            ctx: None,
            heights: HashMap::new(),
            collapsed: Default::default(),
            last_click: None,
            last_playhead: None,
            tool: Tool::Select,
            track_drag: None,
            cache: ViewCache::default(),
            view_glide: None,
            view_w: 1000.0,
            motion: Motion::default(),
            reveal: None,
            now: 0.0,
        }
    }
}

struct Row {
    track: usize,
    rect: Rect,
}

pub(crate) fn menu_item(ui: &mut egui::Ui, glyph: &str, text: &str, keys: &str) -> bool {
    let r = ui.add(
        theme::menu_row(crate::tr(format!("{glyph}   {text}")))
            .keys(keys)
            .min_width(210.0),
    );
    if r.clicked() {
        ui.close_menu();
        return true;
    }
    false
}

fn set_audio_handle(c: &mut Clip, h: AudioHandle, v: f32) {
    let dur = c.duration.as_secs_f64();
    match h {
        AudioHandle::FadeIn => {
            c.fade_in = Time::from_secs_f64((v as f64).clamp(0.0, fade_max(dur, c.fade_out.as_secs_f64())))
        }
        AudioHandle::FadeOut => {
            c.fade_out = Time::from_secs_f64((v as f64).clamp(0.0, fade_max(dur, c.fade_in.as_secs_f64())))
        }
        AudioHandle::Volume => c.volume_db = v,
    }
}

/// Liegt der Clip (Fläche `cr`) sichtbar in der Spurfläche `view`? Unsichtbare werden gar nicht
/// erst gebaut – waagrecht wie senkrecht (Spuren außerhalb des Bildlaufs).
fn clip_visible(cr: Rect, view: Rect) -> bool {
    cr.width() >= 1.0 && cr.right() >= view.left() && cr.left() <= view.right() && cr.bottom() >= view.top() && cr.top() <= view.bottom()
}

/// Schmaler als das (px): nur ein schlichter Balken ohne Text, Filmstreifen und Wellenform
const TINY_PX: f32 = 3.0;
/// Mindestbreite eines Clips bzw. Balkens (px), damit er weit herausgezoomt sichtbar bleibt
const BAR_MIN: f32 = 1.5;

/// Schmalen Clip `r` an den laufenden Balken hängen (berührt ihn und gleiche Farbe), sonst den
/// alten Balken zum Zeichnen zurückgeben und mit `r` neu beginnen.
fn merge_bar(bar: &mut Option<(Rect, Color32)>, r: Rect, col: Color32) -> Option<(Rect, Color32)> {
    match bar {
        Some((b, c)) if *c == col && r.left() <= b.right() + 1.0 => {
            b.max.x = b.max.x.max(r.right());
            None
        }
        _ => bar.replace((r, col)),
    }
}

/// Farbe eines Clip-Balkens (wie die Clipfläche in `draw_clip`)
fn bar_color(tr: &Track, c: &Clip) -> Color32 {
    let col = theme::clip_color(c.kind());
    if tr.hidden || (tr.muted && c.kind() == TrackKind::Audio) || c.disabled || tr.locked {
        col.gamma_multiply(0.4)
    } else {
        col
    }
}

/// Lage der Clip-Texte: (x der Kopfzeile, Breite der Kopfzeile, Breite des Texts im Clip)
fn clip_rooms(cr: Rect, lanes_clip: Rect, missing: bool) -> (f32, f32, f32) {
    let x = cr.left().max(lanes_clip.left()) + 6.0 + if missing { 14.0 } else { 0.0 };
    let right = cr.right().min(lanes_clip.right()) - 4.0;
    (x, right - x, right - (cr.left() + 8.0))
}

/// Texte eines Clips messen (einmal je Projektstand): Kopfzeile (Name, Kennzeichen, Dauer) und
/// bei Textclips der Text im Clip in seiner Schrift.
fn make_head(ctx: &egui::Context, kern: &mut Kerning, c: &Clip, project: &Project, fam: Option<egui::FontFamily>) -> Head {
    let mut tags = String::new();
    if c.speed != 1.0 || c.fx.speed_curve.is_some() {
        tags.push_str(&format!(" {} {:.2}x", icon::GAUGE, c.speed));
    }
    if c.fx.reverse {
        tags.push_str(&format!(" {}", icon::ARROWS_LEFT_RIGHT));
    }
    if !c.fx.effects.is_empty() || c.fx.filter.is_some() || !c.fx.adjust.is_identity() {
        tags.push_str(&format!(" {}", icon::SPARKLE));
    }
    if c.fx.mask.is_some() {
        tags.push_str(&format!(" {}", icon::CIRCLE_DASHED));
    }
    if !c.anim.is_empty() {
        tags.push_str(&format!(" {}", icon::FILM_REEL));
    }
    if c.disabled {
        tags.push_str(&format!(" {}", icon::EYE_SLASH));
    }
    let (name, dur) = match &c.content {
        ClipContent::Text(_) => (icon::TEXT_T.to_string(), String::new()),
        ClipContent::Effect(r) => (format!("{}  {}", icon::SPARKLE, crate::tr(cl_fx::fx_name(&r.id))), String::new()),
        ClipContent::Filter(r) => (format!("{}  {}", icon::CIRCLE_HALF, crate::tr(cl_fx::fx_name(&r.id))), String::new()),
        _ => (crate::contents::clip_title(c, project), theme::secs(c.duration.as_secs_f64(), 1)),
    };
    let sep = if dur.is_empty() { "" } else { "  " };
    let font = egui::FontId::proportional(theme::FS_HINT);
    ctx.fonts(|f| {
        let body = match (&c.content, fam) {
            (ClipContent::Text(st), Some(fam)) => {
                let bf = egui::FontId::new(11.5, fam.clone());
                Some((fam, kern.measured(f, &bf, &st.text)))
            }
            _ => None,
        };
        Head {
            full: kern.measured(f, &font, &format!("{name}{tags}{sep}{dur}")),
            name_n: name.chars().count(),
            tail: kern.measured(f, &font, &format!("{sep}{dur}")),
            body,
            shown: None,
            body_shown: None,
        }
    })
}

/// Kopfzeile und Text im Clip für die Breiten `room`/`body_room`; neu gesetzt wird nur, wenn sich
/// die Breite seit dem letzten Bild geändert hat.
fn head_galleys(h: &mut Head, ctx: &egui::Context, room: f32, body_room: f32) -> (Option<std::sync::Arc<egui::Galley>>, Option<std::sync::Arc<egui::Galley>>) {
    let col = Color32::from_white_alpha(230);
    if h.shown.as_ref().is_none_or(|(w, _)| *w != room) {
        let text = clip_head_measured(&h.full, h.name_n, &h.tail, room);
        let g = ctx.fonts(|f| f.layout_no_wrap(text, egui::FontId::proportional(theme::FS_HINT), col));
        h.shown = Some((room, g));
    }
    if let Some((fam, m)) = &h.body {
        if h.body_shown.as_ref().is_none_or(|(w, _)| *w != body_room) {
            let text = m.cut(m.chars.len(), body_room);
            let g = ctx.fonts(|f| f.layout_no_wrap(text, egui::FontId::new(11.5, fam.clone()), col));
            h.body_shown = Some((body_room, g));
        }
    }
    // leere Texte gar nicht erst als Form ausgeben
    let some = |g: &std::sync::Arc<egui::Galley>| (!g.text().is_empty()).then(|| g.clone());
    (h.shown.as_ref().and_then(|s| some(&s.1)), h.body_shown.as_ref().and_then(|b| some(&b.1)))
}

/// Rechte Gruppe der Werkzeugleiste bei `room` px freier Breite: (Zoom-Regler zeigen, Zoom-Knöpfe zeigen).
/// Schalter und Mikrofon bleiben immer.
fn toolbar_room(room: f32) -> (bool, bool) {
    (room >= 300.0, room >= 210.0)
}

impl App {
    /// Fade/Lautstärke aus der Zeitleiste übernehmen – bei Mehrfachauswahl für alle Clips mit Ton.
    fn apply_audio_handle(&mut self, clip: ClipId, h: AudioHandle, v: f32) {
        let ids = if self.is_selected(clip) { self.selection() } else { vec![clip] };
        let ops: Vec<EditOp> = ids
            .iter()
            .filter_map(|id| self.project.find_clip(*id).map(|(_, c)| c.clone()))
            .filter(|c| match &c.content {
                ClipContent::Media { media, kind } => {
                    *kind == MediaKind::Audio || self.project.media(*media).is_some_and(|m| m.has_audio)
                }
                _ => false,
            })
            .map(|mut c| {
                set_audio_handle(&mut c, h, v);
                EditOp::ReplaceClip { clip: c }
            })
            .collect();
        self.apply_batch(ops);
    }

    fn row_height(&self, t: &Track) -> f32 {
        if self.timeline.collapsed.contains(&t.id) {
            return COLLAPSED_H;
        }
        let base = if t.main {
            MAIN_ROW_H
        } else if t.kind == TrackKind::Audio {
            AUDIO_ROW_H
        } else {
            ROW_H
        };
        base * self.timeline.heights.get(&t.id).copied().unwrap_or(1.0)
    }

    pub fn timeline_panel(&mut self, ui: &mut egui::Ui) {
        self.timeline_toolbar(ui);
        ui.add_space(2.0);
        self.timeline_canvas(ui);
    }

    fn timeline_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.set_min_height(26.0);
            ui.spacing_mut().item_spacing.x = 1.0;
            let (cu, cr) = {
                let d = self.doc.lock().unwrap_or_else(|e| e.into_inner());
                (d.can_undo(), d.can_redo())
            };
            if theme::icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, with_key(&self.t("undo"), &self.key_of("undo")), cu, false).clicked() {
                self.undo();
            }
            if theme::icon_button(ui, icon::ARROW_CLOCKWISE, with_key(&self.t("redo"), &self.key_of("redo")), cr, false).clicked() {
                self.redo();
            }
            ui.add(egui::Separator::default().vertical().spacing(10.0));
            // Werkzeug wählen (Auswählen, Teilen, alles links/rechts)
            let cur = self.timeline.tool;
            ui.menu_button(
                RichText::new(format!("{} {}", cur.icon(), icon::CARET_DOWN)).size(14.0) /* Icon-Glyphen im Werkzeugknopf */,
                |ui| {
                    for (t, g, name, key) in Tool::ALL {
                        let mark = if cur == t { icon::CHECK } else { " " };
                        let r = ui.add(
                            theme::menu_row(format!("{}   {mark}", crate::tr(format!("{g}   {name}"))))
                                .keys(key)
                                .min_width(250.0),
                        );
                        if r.clicked() {
                            self.timeline.tool = t;
                            ui.close_menu();
                        }
                    }
                },
            )
            .response
            .on_hover_text(crate::tr("Werkzeug"));
            // nur das Symbol; der Name steht im Tooltip
            let tools = ui.menu_button(egui::RichText::new(icon::LIGHTNING).size(14.0), |ui| self.tools_menu(ui)).response;
            self.tour_rects.insert("tools", tools.rect);
            tools.on_hover_text(format!("{}: {}", crate::tr("Schnellaktionen"), crate::tr("Pausen entfernen, Füllwörter, Ducking, Multicam, Hochkant …")));
            ui.add(egui::Separator::default().vertical().spacing(10.0));
            let has = !self.project.timeline.tracks.is_empty();
            let sel = self.selected.is_some() || !self.sel_extra.is_empty() || self.selected_transition.is_some();
            if theme::icon_button(ui, icon::SCISSORS, with_key("Teilen", &self.key_of("split")), has, false).clicked() {
                self.split_at_playhead();
            }
            if theme::icon_button(ui, icon::ARROW_LINE_LEFT, with_key("Links vom Playhead löschen", &self.key_of("del_left")), has, false).clicked() {
                self.delete_side(true);
            }
            if theme::icon_button(ui, icon::ARROW_LINE_RIGHT, with_key("Rechts vom Playhead löschen", &self.key_of("del_right")), has, false).clicked() {
                self.delete_side(false);
            }
            if theme::icon_button(ui, icon::TRASH, with_key("Löschen", &self.key_of("delete")), sel, false).clicked() {
                self.delete_selected();
            }
            ui.add(egui::Separator::default().vertical().spacing(10.0));
            let video_sel = self.selected.and_then(|s| self.project.find_clip(s)).is_some_and(|(_, c)| matches!(c.content, ClipContent::Media { kind: MediaKind::Video, .. }));
            let visual_sel = self.selected.and_then(|s| self.project.find_clip(s)).is_some_and(|(_, c)| c.kind() == TrackKind::Video);
            if theme::icon_button(ui, icon::SNOWFLAKE, "Standbild (3 s am Playhead)", has, false).clicked() {
                self.freeze_frame();
            }
            let rev = self.selected.and_then(|s| self.project.find_clip(s)).is_some_and(|(_, c)| c.fx.reverse);
            if theme::icon_button(ui, icon::ARROWS_LEFT_RIGHT, "Rückwärts", video_sel, rev).clicked() {
                self.modify_selection(|c| c.fx.reverse = !c.fx.reverse);
            }
            if theme::icon_button(ui, icon::FLIP_HORIZONTAL, "Spiegeln", visual_sel, false).clicked() {
                self.modify_selection(|c| c.fx.flip_h = !c.fx.flip_h);
            }
            if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, "90° drehen", visual_sel, false).clicked() {
                self.modify_selection(|c| c.transform.rotation_deg = ((c.transform.rotation_deg + 90.0 + 180.0).rem_euclid(360.0)) - 180.0);
            }
            if theme::icon_button(ui, icon::WAVEFORM, "Ton extrahieren", video_sel, false).clicked() {
                if let Some(s) = self.selected {
                    self.extract_audio(s);
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                ui.add_space(4.0);
                // schmale Zeitleiste: sonst liegen diese Knöpfe über denen der linken Gruppe
                let (slider, zoom) = toolbar_room(ui.available_width());
                let now = ui.input(|i| i.time);
                let dur = self.project.duration().as_secs_f64();
                let lo = min_pps(dur, self.timeline.view_w);
                let target = self.timeline.target_pps();
                if zoom && theme::icon_button(ui, icon::ARROWS_OUT_LINE_HORIZONTAL, "Ganze Zeitleiste zeigen", true, false).clicked() {
                    self.timeline.show_all(dur, now);
                }
                if zoom && theme::icon_button(ui, icon::PLUS, "Vergrößern (Strg+Mausrad)", true, false).clicked() {
                    self.timeline.zoom_to((target * 1.25).min(PPS_MAX), 0.0, now);
                }
                if slider {
                ui.allocate_ui(vec2(90.0, 20.0), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let (rect, resp) = ui.allocate_exact_size(vec2(90.0, 18.0), Sense::click_and_drag());
                    let track = Rect::from_center_size(rect.center(), vec2(80.0, 2.0));
                    let n = zoom_norm(target, lo);
                    if let Some(p) = resp.interact_pointer_pos() {
                        let z = norm_zoom(((p.x - track.left()) / track.width()).clamp(0.0, 1.0), lo);
                        if (z - target).abs() > target * 1e-4 {
                            self.timeline.zoom_to(z, 0.0, now);
                        }
                    }
                    ui.painter().rect_filled(track, 1.0, theme::BG_HOVER());
                    ui.painter().rect_filled(Rect::from_min_max(track.min, pos2(track.left() + n * track.width(), track.max.y)), 1.0, theme::TEXT_DIM());
                    ui.painter().circle_filled(pos2(track.left() + n * track.width(), track.center().y), if resp.hovered() { 5.5 } else { 4.5 }, theme::ACCENT());
                });
                }
                if zoom {
                    if theme::icon_button(ui, icon::MINUS, "Verkleinern", true, false).clicked() {
                        self.timeline.zoom_to((target / 1.25).max(lo), 0.0, now);
                    }
                    ui.add(egui::Separator::default().vertical().spacing(10.0));
                }
                let s = self.project.settings.clone();
                if theme::icon_button(ui, icon::ARROWS_IN_LINE_HORIZONTAL, with_key(&self.t("snapping"), &self.key_of("snap")), true, s.snapping).clicked() {
                    let mut n = s.clone();
                    n.snapping = !n.snapping;
                    self.apply(EditOp::SetSettings { settings: n });
                }
                if theme::icon_button(ui, icon::MAGNET, with_key(&self.t("magnet"), &self.key_of("magnet")), true, s.main_track_magnet).clicked() {
                    let mut n = s.clone();
                    n.main_track_magnet = !n.main_track_magnet;
                    self.apply(EditOp::SetSettings { settings: n });
                }
                if theme::icon_button(ui, icon::STACK, "Alles mitziehen: Ripple-Schnitte verschieben alle Spuren und Marker", true, s.ripple_all_tracks).clicked() {
                    let mut n = s.clone();
                    n.ripple_all_tracks = !n.ripple_all_tracks;
                    self.apply(EditOp::SetSettings { settings: n });
                }
                ui.add(egui::Separator::default().vertical().spacing(10.0));
                let rec = self.voiceover.recorder.is_some();
                // Eigener Knopf statt icon_button: bei laufender Aufnahme rot (icon_button kennt nur aktiv = Akzent)
                let mic = ui.add(
                    egui::Button::new(RichText::new(icon::MICROPHONE).size(14.0).color(if rec { theme::DANGER } else if self.voiceover.open { theme::ACCENT() } else { theme::TEXT_DIM() }))
                        .fill(if self.voiceover.open || rec { theme::overlay(22) } else { Color32::TRANSPARENT })
                        .corner_radius(theme::R_CTRL)
                        .min_size(vec2(24.0, 22.0)),
                );
                if mic.on_hover_text(crate::tr(with_key(&self.t("voiceover"), &self.key_of("voiceover")))).clicked() {
                    self.voiceover.open = !self.voiceover.open;
                }
            });
        });
    }

    fn timeline_canvas(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let x0 = rect.left() + HEADER_W;
        self.timeline.view_w = (rect.right() - x0).max(1.0);
        if self.timeline.step_view(ui.input(|i| i.time)) {
            ui.ctx().request_repaint();
        }
        let pps = self.timeline.pps;
        let fps = self.project.settings.fps;
        let snapping = self.project.settings.snapping;
        let key = CacheKey { epoch: self.project_epoch, lang: cl_i18n::ui_lang(), ppp: ui.ctx().pixels_per_point() };
        let old_tracks = self.timeline.cache.tracks.clone();
        let changed = self.timeline.cache.sync(key, || self.project.timeline.tracks.clone());
        let tracks = self.timeline.cache.tracks.clone();
        let now_s = ui.input(|i| i.time);
        self.timeline.now = now_s;
        self.timeline.cache.missing = missing_media(&self.project.media, &self.presence);
        let total_t = self.project.duration().as_secs_f64();
        let recording = self.voiceover.recorder.as_ref().map(|r| {
            (
                r.start_at.as_secs_f64(),
                r.elapsed().as_secs_f64(),
                r.peaks(),
            )
        });

        // ---------- Mausrad ----------
        let ruler = Rect::from_min_max(
            pos2(x0, rect.top()),
            pos2(rect.right(), rect.top() + RULER_H),
        );
        let lanes = Rect::from_min_max(pos2(rect.left(), ruler.bottom()), rect.max);
        let mut order = display_order(&tracks);
        if recording.is_some() {
            order.push(REC_ROW);
        }
        // Spurhöhen gleiten (ein-/ausklappen, Spurhöhe, neue Spur wächst aus 0); nicht beim ersten
        // Bild und nicht, wenn ein anderes Projekt geladen wurde
        let secs = theme::anim(theme::MOTION);
        let known: HashSet<TrackId> = self.timeline.motion.order.iter().copied().collect();
        let animate_tracks = !known.is_empty() && tracks.iter().any(|t| known.contains(&t.id));
        let mut old_h = std::mem::take(&mut self.timeline.motion.heights);
        let mut new_h = HashMap::new();
        let heights: Vec<f32> = order
            .iter()
            .map(|i| {
                if *i == REC_ROW {
                    AUDIO_ROW_H
                } else {
                    let t = &tracks[*i];
                    let appear = animate_tracks && !known.contains(&t.id);
                    let (g, h) = height_at(old_h.remove(&t.id), self.row_height(t), appear, now_s, secs);
                    new_h.insert(t.id, g);
                    h
                }
            })
            .collect();
        self.timeline.motion.heights = new_h;
        // weggefallene Spuren: ihr Platz schrumpft (über der nächsten noch vorhandenen Spur)
        if animate_tracks && secs > 0.0 {
            let m = &mut self.timeline.motion;
            for (i, id) in m.order.iter().enumerate() {
                if !tracks.iter().any(|t| t.id == *id) {
                    let anchor = m.order[i + 1..].iter().find(|a| tracks.iter().any(|t| t.id == **a)).copied();
                    let h = m.rows.get(id).map_or(0.0, |r| r.1) + ROW_GAP;
                    m.spacers.push((anchor, h, now_s));
                }
            }
        }
        self.timeline.motion.spacers.retain(|s| ((now_s - s.2) as f32) < secs);
        let spacers = self.timeline.motion.spacers.clone();
        let spacer = |anchor: Option<TrackId>| -> f32 {
            spacers.iter().filter(|s| s.0 == anchor).map(|s| theme::glide_at(s.1, 0.0, (now_s - s.2) as f32, secs)).sum()
        };
        let content_h: f32 = heights.iter().map(|h| h + ROW_GAP).sum::<f32>() + 24.0 + self.timeline.motion.spacers.iter().map(|s| s.1).sum::<f32>();
        let view_h = lanes.height();
        let overflow = (content_h - view_h).max(0.0);
        if resp.hovered() || resp.dnd_hover_payload::<DragAsset>().is_some() {
            let (dy, dx, ctrl, shift, pointer) = ui.input(|i| {
                (
                    i.raw_scroll_delta.y,
                    i.raw_scroll_delta.x,
                    i.modifiers.command,
                    i.modifiers.shift,
                    i.pointer.hover_pos(),
                )
            });
            // Bildlauf links höchstens bis 0:00 – oder bis zum Rand, den „Ganze Zeitleiste zeigen“ ließ
            let floor = self.timeline.scroll.min(0.0);
            if ctrl && dy != 0.0 {
                // Zoom gleitet um die Zeigerposition; weitere Rad-Schritte setzen das Ziel fort
                let lo = min_pps(total_t, self.timeline.view_w);
                let to = (self.timeline.target_pps() * (1.0 + dy * 0.0015)).clamp(lo, PPS_MAX);
                let ax = pointer.map_or(0.0, |p| (p.x - x0).max(0.0));
                self.timeline.zoom_to(to, ax, now_s);
            } else if shift || overflow <= 0.0 || pointer.is_some_and(|p| p.y < ruler.bottom()) {
                let d = if dx != 0.0 { dx } else { dy };
                if d != 0.0 {
                    self.timeline.view_glide = None;
                    self.timeline.scroll = (self.timeline.scroll - (d / pps) as f64).max(floor);
                }
            } else {
                if dy != 0.0 {
                    self.timeline.vscroll = (self.timeline.vscroll - dy).clamp(0.0, overflow);
                }
                if dx != 0.0 {
                    self.timeline.view_glide = None;
                    self.timeline.scroll = (self.timeline.scroll - (dx / pps) as f64).max(floor);
                }
            }
        }
        // Beim Ziehen (Playhead, Clips, Kanten) an den Rand: Zeitleiste rollt in diese Richtung,
        // je näher am Rand, desto schneller – auch wenn die Maus dort stillsteht.
        if resp.dragged() && !matches!(self.timeline.drag, Drag::None | Drag::Slip { .. }) {
            if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                let edge = 48.0;
                let (l, r) = (x0, lanes.right());
                let v = if p.x < l + edge {
                    -((l + edge - p.x) / edge).min(3.0)
                } else if p.x > r - edge {
                    ((p.x - (r - edge)) / edge).min(3.0)
                } else {
                    0.0
                };
                if v != 0.0 {
                    let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05) as f64;
                    let visible = ((r - x0) / pps) as f64;
                    self.timeline.view_glide = None;
                    self.timeline.scroll =
                        (self.timeline.scroll + v as f64 * visible * 0.8 * dt).max(self.timeline.scroll.min(0.0));
                    ui.ctx().request_repaint();
                }
            }
        }
        // Springt der Playhead (Pos1/Ende, Marker, Schnitte …) aus dem sichtbaren Bereich,
        // rollt die Ansicht hinterher.
        if !resp.dragged() && !self.playing && self.timeline.last_playhead != Some(self.playhead) {
            let t = self.playhead.as_secs_f64();
            let visible = ((lanes.right() - x0) / pps) as f64;
            if t < self.timeline.scroll || t > self.timeline.scroll + visible {
                self.timeline.scroll_to((t - visible * 0.3).max(0.0), now_s);
            }
        }
        self.timeline.last_playhead = Some(self.playhead);
        self.timeline.vscroll = self.timeline.vscroll.clamp(0.0, overflow);
        let pps = self.timeline.pps;
        let scroll = self.timeline.scroll;
        let tx = |t: f64| x0 + ((t - scroll) as f32) * pps;
        let xt = |x: f32| time_at(x, x0, scroll, pps);

        // ---------- Zeilen ----------
        let mut y = if overflow > 0.0 {
            lanes.top() + 8.0 - self.timeline.vscroll
        } else {
            lanes.top() + 8.0 + ((view_h - content_h) * 0.4).max(0.0)
        };
        let mut rows = Vec::new();
        for (k, i) in order.iter().enumerate() {
            if *i != REC_ROW {
                y += spacer(Some(tracks[*i].id));
            }
            rows.push(Row {
                track: *i,
                rect: Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), heights[k])),
            });
            y += heights[k] + ROW_GAP;
        }
        // Projektstand neu: bewegte Clips gleiten von ihrer angezeigten Lage, neue blenden ein,
        // gelöschte blenden mit ihrem letzten Aussehen aus
        if changed {
            let row_top: HashMap<TrackId, f32> =
                rows.iter().filter(|r| r.track != REC_ROW).map(|r| (tracks[r.track].id, r.rect.top())).collect();
            self.timeline.motion.update(&old_tracks, &tracks, &row_top, now_s);
        }
        // neue Spur sichtbar machen: weich rollen, solange sie (aus 0 wachsend) noch nicht ganz zu sehen ist
        if let Some((id, t0, from)) = self.timeline.reveal {
            let age = (now_s - t0) as f32;
            match rows.iter().find(|r| r.track != REC_ROW && tracks[r.track].id == id) {
                Some(r) if age < 2.0 * secs + 0.05 => {
                    let target = (self.timeline.vscroll + reveal_delta(r.rect, lanes)).clamp(0.0, overflow);
                    self.timeline.vscroll = theme::glide_at(from, target, age, secs).clamp(0.0, overflow);
                    ui.ctx().request_repaint();
                }
                Some(_) if age < 1.0 => {}
                _ => self.timeline.reveal = None,
            }
        }
        // Zeile (Anzeige-Index in `rows`) unter dem Zeiger; der Spalt zwischen zwei Zeilen zählt halb dazu
        let row_idx_at = |p: Pos2| {
            rows.iter().position(|r| {
                p.y >= r.rect.top() - ROW_GAP / 2.0 && p.y < r.rect.bottom() + ROW_GAP / 2.0 && r.track != REC_ROW
            })
        };
        let row_at = |p: Pos2| row_idx_at(p).map(|k| rows[k].track);
        let n_rows = rows.iter().filter(|r| r.track != REC_ROW).count();
        // senkrechte Lage der Lücke `g` (über Zeile g) – dort entsteht eine neue Spur
        let gap_y = |g: usize| match g {
            0 => rows.first().map_or(lanes.top() + 4.0, |r| r.rect.top() - ROW_GAP / 2.0),
            g => rows[g.min(n_rows) - 1].rect.bottom() + ROW_GAP / 2.0,
        };
        let mut insert_lines: Vec<f32> = Vec::new();

        // ---------- Spur am Kopf ziehen ----------
        let (primary_down, now, latest, esc) = ui.input(|i| {
            (i.pointer.primary_down(), i.time, i.pointer.latest_pos(), i.key_pressed(egui::Key::Escape))
        });
        if resp.hovered() && ui.input(|i| i.pointer.primary_pressed()) && matches!(self.timeline.drag, Drag::None) {
            let head = latest.filter(|p| p.x < x0 - 4.0 && p.y >= lanes.top());
            if let Some((p, k)) = head.and_then(|p| Some((p, row_idx_at(p)?))) {
                let tr = &tracks[rows[k].track];
                // nicht auf den Schaltern (Sperren, Auge …): die bleiben Knöpfe
                let n = toggle_count(tr, self.timeline.collapsed.contains(&tr.id));
                let on_toggle = n > 0 && p.x >= rect.left() + 23.0 && p.x < rect.left() + 32.0 + n as f32 * 19.0;
                if !on_toggle && !tr.main {
                    self.timeline.track_drag = Some(TrackDrag { track: tr.id, origin: p, t0: now, lifted: false });
                }
            }
        }
        let mut lifted_row: Option<usize> = None;
        // gedrückt, noch nicht angehoben: sofort sichtbar (Druck-Zustand im selben Bild)
        let mut pressed_row: Option<usize> = None;
        if let Some(mut d) = self.timeline.track_drag.take() {
            let from = rows.iter().position(|r| r.track != REC_ROW && tracks[r.track].id == d.track);
            if let (Some(from), Some(p), false) = (from, latest, esc) {
                d.lifted |= track_lifted(p.distance(d.origin), now - d.t0);
                let gap = rows.iter().filter(|r| r.track != REC_ROW && r.rect.center().y < p.y).count();
                let target = if d.lifted { track_drop_gap(&tracks, from, gap) } else { None };
                if !primary_down {
                    if let Some(op) = target.and_then(|g| move_track_to(&tracks, from, g)) {
                        self.apply(op);
                    }
                } else {
                    if !d.lifted {
                        pressed_row = Some(from);
                    }
                    if d.lifted {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                        lifted_row = Some(from);
                        insert_lines.extend(target.map(gap_y));
                        // am oberen/unteren Rand rollen die Spuren mit
                        let edge = 28.0;
                        let v = if p.y < lanes.top() + edge {
                            -((lanes.top() + edge - p.y) / edge).min(3.0)
                        } else if p.y > lanes.bottom() - edge {
                            ((p.y - (lanes.bottom() - edge)) / edge).min(3.0)
                        } else {
                            0.0
                        };
                        let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05);
                        self.timeline.vscroll = (self.timeline.vscroll + v * 500.0 * dt).clamp(0.0, overflow);
                    }
                    self.timeline.track_drag = Some(d);
                    ui.ctx().request_repaint();
                }
            }
        }

        let lane_paint = painter.with_clip_rect(lanes);
        for row in &rows {
            let lane = Rect::from_min_max(pos2(x0, row.rect.top()), row.rect.max);
            let fill = if row.track != REC_ROW && tracks[row.track].main {
                theme::BG_RAISED()
            } else {
                theme::BG_INPUT()
            };
            lane_paint.rect_filled(lane, theme::R_CTRL, fill);
        }

        // ---------- Treffer ----------
        let hit = |p: Pos2| -> Option<(ClipId, Option<Edge>, usize)> {
            if p.x < x0 || !lanes.contains(p) {
                return None;
            }
            for row in rows.iter().filter(|r| r.track != REC_ROW) {
                if !row.rect.contains(p) {
                    continue;
                }
                for c in &tracks[row.track].clips {
                    let (a, b) = (tx(c.start.as_secs_f64()), tx(c.end().as_secs_f64()));
                    if p.x >= a - 2.0 && p.x <= b + 2.0 {
                        let w = b - a;
                        let edge = if w > 3.0 * EDGE_PX && (p.x - a).abs() <= EDGE_PX {
                            Some(Edge::Start)
                        } else if (p.x - b).abs() <= EDGE_PX {
                            Some(Edge::End)
                        } else {
                            None
                        };
                        return Some((c.id, edge, row.track));
                    }
                }
            }
            None
        };
        // Übergangsblöcke
        let mut tblocks: Vec<(ClipId, Rect)> = Vec::new();
        for row in rows.iter().filter(|r| r.track != REC_ROW) {
            let tr = &tracks[row.track];
            for (i, c) in tr.clips.iter().enumerate() {
                let Some(t) = &c.transition_out else { continue };
                if !tr
                    .clips
                    .get(i + 1)
                    .is_some_and(|n| (n.start.0 - c.end().0).abs() < 2000)
                {
                    continue;
                }
                let w = (t.duration.as_secs_f64() as f32 * pps).clamp(16.0, 56.0);
                let r = Rect::from_center_size(
                    pos2(
                        tx(c.end().as_secs_f64()),
                        row.rect.center().y + BAND_H / 2.0,
                    ),
                    vec2(w, 18.0_f32.min(row.rect.height() - BAND_H - 4.0)),
                );
                tblocks.push((c.id, r));
            }
        }
        let hit_block = |p: Pos2| {
            tblocks
                .iter()
                .find(|(_, r)| r.expand(2.0).contains(p))
                .map(|(c, _)| *c)
        };

        let ph = self.playhead.as_secs_f64();
        let snap_cands = |ignore: &[ClipId]| -> Vec<f64> {
            let mut v = vec![0.0, ph];
            for tr in tracks.iter() {
                for c in tr.clips.iter().filter(|c| !ignore.contains(&c.id)) {
                    v.push(c.start.as_secs_f64());
                    v.push(c.end().as_secs_f64());
                    v.extend(c.beat_times().into_iter().map(|b| b.as_secs_f64()));
                }
            }
            v
        };
        let snap_to = |t: f64, cands: &[f64]| -> Option<f64> {
            if !snapping {
                return None;
            }
            cands
                .iter()
                .copied()
                .filter(|c| ((c - t).abs() as f32) * pps < SNAP_PX)
                .min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs()))
        };
        let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        let hovered_clip = resp.hover_pos().and_then(|p| hit(p).map(|h| h.0));
        let head_rect = Rect::from_center_size(pos2(tx(ph), rect.top() + 9.0), vec2(14.0, 18.0));
        // Clips mit Ton: Fade-Griffe oben an den Ecken, Lautstärkelinie quer über den Clip
        let audio_media: HashSet<MediaId> = self
            .project
            .media
            .iter()
            .filter(|m| m.has_audio || m.kind == MediaKind::Audio)
            .map(|m| m.id)
            .collect();
        let has_audio = |c: &Clip| matches!(&c.content, ClipContent::Media { media, .. } if audio_media.contains(media));
        let clip_body = |id: ClipId| -> Option<(Rect, Clip)> {
            for row in rows.iter().filter(|r| r.track != REC_ROW) {
                if let Some(c) = tracks[row.track].clips.iter().find(|c| c.id == id) {
                    let cr = Rect::from_min_max(
                        pos2(tx(c.start.as_secs_f64()), row.rect.top()),
                        pos2(tx(c.end().as_secs_f64()), row.rect.bottom()),
                    );
                    let body = Rect::from_min_max(pos2(cr.left(), cr.top() + BAND_H), cr.max);
                    return Some((body, c.clone()));
                }
            }
            None
        };
        let audio_hit = |p: Pos2| -> Option<(ClipId, AudioHandle, f32)> {
            // Fade-Griffe sitzen in den oberen Ecken und gehen dort dem Trimmen an der Kante vor
            let (id, edge, _) = hit(p)?;
            let (body, c) = clip_body(id)?;
            if !has_audio(&c) || body.width() < 24.0 || body.height() < 14.0 {
                return None;
            }
            let fi = pos2((body.left() + c.fade_in.as_secs_f64() as f32 * pps).max(body.left() + 5.0), body.top() + 5.0);
            let fo = pos2((body.right() - c.fade_out.as_secs_f64() as f32 * pps).min(body.right() - 5.0), body.top() + 5.0);
            if p.distance(fi) < 7.0 {
                return Some((id, AudioHandle::FadeIn, c.fade_in.as_secs_f64() as f32));
            }
            if p.distance(fo) < 7.0 {
                return Some((id, AudioHandle::FadeOut, c.fade_out.as_secs_f64() as f32));
            }
            if edge.is_none() && vol_line_hit(body, c.volume_db, p.y) {
                return Some((id, AudioHandle::Volume, c.volume_db));
            }
            None
        };

        if let Some(p) = resp.hover_pos() {
            if let Some((_, h, _)) = audio_hit(p) {
                ui.ctx().set_cursor_icon(if h == AudioHandle::Volume {
                    egui::CursorIcon::ResizeVertical
                } else {
                    egui::CursorIcon::ResizeHorizontal
                });
            } else if matches!(hit(p), Some((_, Some(_), _))) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            } else if head_rect.contains(p) || (p.x - tx(ph)).abs() < 4.0 && p.x >= x0 {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
            }
        }

        // Klick in die Zeitleiste beendet Texteingaben (sonst landet Entf im Textfeld)
        if resp.clicked() || resp.secondary_clicked() || resp.drag_started() {
            crate::app::release_text_focus(ui.ctx());
        }
        // ---------- Klicks ----------
        let mods = ui.input(|i| i.modifiers);
        let multi = mods.command || mods.shift;
        // Marker im Lineal: Klick springt hin, Doppelklick öffnet den Editor
        let markers = self.project.timeline.markers.clone();
        let marker_at = |p: Pos2| -> Option<(u64, Time)> {
            if p.y >= ruler.bottom() || p.x < x0 {
                return None;
            }
            markers
                .iter()
                .filter(|m| (tx(m.at.as_secs_f64()) - p.x).abs() <= 7.0)
                .min_by_key(|m| ((tx(m.at.as_secs_f64()) - p.x).abs() * 100.0) as i64)
                .map(|m| (m.id, m.at))
        };
        if resp.double_clicked() {
            if let Some((id, at)) = resp.interact_pointer_pos().and_then(marker_at) {
                self.stop_playback();
                self.playhead = at;
                self.marker_edit = Some(id);
            }
        }
        let tool = self.timeline.tool;
        let tool_click = tool != Tool::Select
            && resp
                .interact_pointer_pos()
                .or(resp.hover_pos())
                .is_some_and(|p| p.y >= ruler.bottom() && p.x >= x0);
        if tool_click {
            if let Some(p) = resp.hover_pos() {
                ui.ctx().set_cursor_icon(if tool == Tool::Blade {
                    egui::CursorIcon::Text
                } else {
                    egui::CursorIcon::PointingHand
                });
                // Vorschau: Schnittlinie bzw. Grenze der Auswahl
                let line_col = if tool == Tool::Blade { theme::ACCENT() } else { theme::overlay(160) };
                let (top, bottom) = match hit(p).and_then(|h| clip_body(h.0)) {
                    Some((b, _)) if tool == Tool::Blade => (b.top() - BAND_H, b.bottom()),
                    _ => (lanes.top(), lanes.bottom()),
                };
                painter.line_segment([pos2(p.x, top), pos2(p.x, bottom)], Stroke::new(1.5_f32, line_col));
            }
        }
        if resp.clicked() && tool_click {
            if let Some(p) = resp.interact_pointer_pos() {
                let t = Time::from_secs_f64(xt(p.x)).snap_to_frame(fps);
                match tool {
                    Tool::Blade => {
                        if let Some((c, _, _)) = hit(p) {
                            if let Some(OpOutput::Clip(r)) = self.apply_batch(vec![EditOp::SplitClip { clip: c, at: t }]) {
                                self.select_only(Some(r));
                            }
                        }
                    }
                    _ => {
                        let left = tool == Tool::SelectLeft;
                        let ids: Vec<ClipId> = tracks
                            .iter()
                            .filter(|tr| !tr.locked)
                            .flat_map(|tr| tr.clips.iter())
                            .filter(|c| if left { c.start < t } else { c.end() > t })
                            .map(|c| c.id)
                            .collect();
                        self.select_only(None);
                        self.selected = ids.last().copied();
                        self.sel_extra = ids.into_iter().collect();
                    }
                }
            }
        } else if resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some((_, at)) = marker_at(p) {
                    self.stop_playback();
                    self.playhead = at;
                } else if p.y < ruler.bottom() && p.x >= x0 {
                    self.set_playhead_ui(Time::from_secs_f64(xt(p.x)), false);
                } else if let Some(b) = hit_block(p) {
                    self.select_only(None);
                    self.selected_transition = Some(b);
                } else if let Some((c, _, _)) = hit(p) {
                    if multi {
                        self.toggle_select(c);
                    } else {
                        self.select_only(Some(c));
                    }
                    self.expand_groups();
                } else if p.x >= x0 {
                    // Kurzer Klick ins Leere: Playhead dorthin (höchstens Projektende)
                    if !multi {
                        self.select_only(None);
                    }
                    let t = xt(p.x).min(total_t);
                    self.set_playhead_ui(Time::from_secs_f64(t), true);
                }
            }
        }
        // Doppelklick nur, wenn beide Klicks dieselbe Stelle treffen (egui prüft nur die Zeit)
        let same_spot = resp
            .interact_pointer_pos()
            .zip(self.timeline.last_click)
            .is_some_and(|(a, b)| a.distance(b) < 6.0);
        if resp.double_clicked() && same_spot {
            if let Some((c, _, _)) = resp.interact_pointer_pos().and_then(hit) {
                if self
                    .project
                    .find_clip(c)
                    .is_some_and(|(_, c)| matches!(c.content, ClipContent::Text(_)))
                {
                    self.clip_tab = crate::app::ClipTab::Text;
                    self.focus_text = true;
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                self.timeline.ctx = Some(if p.y < ruler.bottom() && p.x >= x0 {
                    // Lineal: Playhead an die Stelle (bzw. auf den Marker), das Menü wirkt dort
                    let m = marker_at(p);
                    let at = m.map(|m| m.1).unwrap_or(Time::from_secs_f64(xt(p.x)));
                    self.set_playhead_ui(at, false);
                    CtxTarget::Ruler(m.map(|m| m.0))
                } else if let Some(b) = hit_block(p) {
                    CtxTarget::Transition(b)
                } else if let Some((c, _, _)) = hit(p) {
                    if !self.is_selected(c) {
                        self.select_only(Some(c));
                    }
                    CtxTarget::Clip(c)
                } else if p.x < x0 {
                    match row_at(p) {
                        Some(t) => CtxTarget::Track(tracks[t].id),
                        None => CtxTarget::Empty(None),
                    }
                } else {
                    CtxTarget::Empty(row_at(p).map(|t| tracks[t].id))
                });
            }
        }

        if resp.clicked() {
            self.timeline.last_click = resp.interact_pointer_pos();
        }
        // Klick in eine Spur (Fläche, Clip, Spurkopf; auch Rechtsklick): dorthin fügt Strg+V ein
        if resp.clicked() || resp.secondary_clicked() {
            if let Some(t) = resp.interact_pointer_pos().filter(|p| p.y >= ruler.bottom()).and_then(row_at) {
                self.paste_track = Some(tracks[t].id);
            }
        }
        // ---------- Ziehen beginnen ----------
        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(p) = ui
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
            {
                self.commit_live();
                self.timeline.drag = if (p.y < ruler.bottom() && p.x >= x0)
                    || head_rect.contains(p)
                    || ((p.x - tx(ph)).abs() < 4.0 && p.x >= x0 && hit(p).is_none())
                {
                    Drag::Scrub
                } else if self.timeline.tool != Tool::Select {
                    Drag::None
                } else if let Some((clip, handle, value)) = audio_hit(p) {
                    Drag::Audio { clip, handle, value }
                } else {
                    let alt = ui.input(|i| i.modifiers.alt);
                    // Nachbar, der an dieser Kante direkt anschließt
                    let neighbor = |c: ClipId, edge: Edge| -> Option<(ClipId, ClipId)> {
                        let (t, x) = self.project.find_clip(c)?;
                        match edge {
                            Edge::End => t
                                .clips
                                .iter()
                                .find(|o| o.id != c && (o.start.0 - x.end().0).abs() < 2000)
                                .map(|o| (c, o.id)),
                            Edge::Start => t
                                .clips
                                .iter()
                                .find(|o| o.id != c && (o.end().0 - x.start.0).abs() < 2000)
                                .map(|o| (o.id, c)),
                        }
                    };
                    match hit(p) {
                        Some((c, Some(edge), _)) if alt && neighbor(c, edge).is_some() => {
                            let (left, right) = neighbor(c, edge).unwrap();
                            self.select_only(Some(c));
                            Drag::Roll { left, right }
                        }
                        Some((c, Some(edge), _)) => {
                            self.select_only(Some(c));
                            Drag::Trim { clip: c, edge }
                        }
                        Some((c, None, _))
                            if alt
                                && self.project.find_clip(c).is_some_and(|(_, x)| {
                                    crate::edit_tools::source_limit(self, x).is_some()
                                }) =>
                        {
                            self.select_only(Some(c));
                            let src = self
                                .project
                                .find_clip(c)
                                .map(|(_, x)| x.source_in.as_secs_f64())
                                .unwrap_or(0.0);
                            Drag::Slip {
                                clip: c,
                                src,
                                x: p.x,
                            }
                        }
                        Some((c, None, _)) => {
                            if !self.is_selected(c) {
                                if multi {
                                    self.toggle_select(c);
                                } else {
                                    self.select_only(Some(c));
                                }
                            } else {
                                make_primary(&mut self.selected, &mut self.sel_extra, c);
                            }
                            self.expand_groups();
                            let start = self
                                .project
                                .find_clip(c)
                                .map(|(_, c)| c.start.as_secs_f64())
                                .unwrap_or(0.0);
                            let ids = self
                                .selection()
                                .into_iter()
                                .filter_map(|id| {
                                    self.project
                                        .find_clip(id)
                                        .map(|(_, x)| (id, x.start.as_secs_f64()))
                                })
                                .collect();
                            Drag::Move {
                                clip: c,
                                grab: xt(p.x) - start,
                                ids,
                            }
                        }
                        None if p.x >= x0 && hit_block(p).is_none() => Drag::Marquee {
                            origin: p,
                            additive: multi,
                        },
                        None => Drag::None,
                    }
                };
            }
        }

        // ---------- Ziehen ----------
        // Geist eines gezogenen Clips: (Clip, Beginn, Ende, Zeilenfläche, falls nicht die eigene)
        let mut ghost: Vec<(ClipId, f64, f64, Option<Rect>)> = Vec::new();
        let mut snap_line: Option<f64> = None;
        let dragging = resp.dragged() || resp.drag_stopped();
        // Esc bricht das Verschieben von Clips ab (nichts wird geändert)
        if esc && matches!(self.timeline.drag, Drag::Move { .. }) {
            self.timeline.drag = Drag::None;
        }
        if let (Some(p), true) = (pointer, dragging) {
            match self.timeline.drag.clone() {
                Drag::Scrub => {
                    if self.playing {
                        self.stop_playback();
                    }
                    let t = xt(p.x);
                    let s = snap_to(t, &snap_cands(&[])[2..].to_vec()).filter(|_| true);
                    if let Some(s) = s {
                        snap_line = Some(s);
                    }
                    self.playhead = Time::from_secs_f64(s.unwrap_or(t)).snap_to_frame(fps);
                }
                Drag::Move { clip, grab, ids } => {
                    let Some((_, c)) = self.project.find_clip(clip) else {
                        return;
                    };
                    let dur = c.duration.as_secs_f64();
                    let kind = c.kind();
                    let moving: Vec<ClipId> = ids.iter().map(|x| x.0).collect();
                    let cands = snap_cands(&moving);
                    let raw = (xt(p.x) - grab).max(0.0);
                    let ss = snap_to(raw, &cands);
                    let se = snap_to(raw + dur, &cands).map(|e| e - dur);
                    let s = match (ss, se) {
                        (Some(a), Some(b)) => {
                            if (a - raw).abs() <= (b - raw).abs() {
                                snap_line = Some(a);
                                a
                            } else {
                                snap_line = Some(b + dur);
                                b
                            }
                        }
                        (Some(a), None) => {
                            snap_line = Some(a);
                            a
                        }
                        (None, Some(b)) => {
                            snap_line = Some(b + dur);
                            b
                        }
                        _ => raw,
                    }
                    .max(0.0);
                    let orig = ids.iter().find(|x| x.0 == clip).map(|x| x.1).unwrap_or(0.0);
                    let delta = s - orig;
                    // Ziel: andere Spur gleicher Art; ins Leere darüber/darunter oder über einer
                    // unpassenden/gesperrten Spur = neue Spur(en) daneben (je Clip-Art eine)
                    let mut target = Target::Same;
                    if lanes.contains(p) && n_rows > 0 {
                        if p.y < rows[0].rect.top() - ROW_GAP && kind != TrackKind::Audio {
                            target = Target::New(0, true);
                        } else if p.y > rows[n_rows - 1].rect.bottom() + ROW_GAP && kind == TrackKind::Audio {
                            target = Target::New(n_rows - 1, false);
                        } else if let Some(k) = row_idx_at(p) {
                            let t = rows[k].track;
                            if !fits(&tracks[t], kind) {
                                target = Target::New(k, p.y < rows[k].rect.center().y);
                            } else if ids.len() <= 1 {
                                target = Target::Track(t);
                            }
                        }
                    }
                    for (id, st) in &ids {
                        let Some((_, x)) = self.project.find_clip(*id) else {
                            continue;
                        };
                        let ns = (st + delta).max(0.0);
                        let tr = match target {
                            Target::Track(t) if *id == clip => rows.iter().find(|r| r.track == t).map(|r| r.rect),
                            Target::New(k, upper) => {
                                let y = gap_y(new_track_gap(&tracks, k, upper, x.kind()));
                                if !insert_lines.contains(&y) {
                                    insert_lines.push(y);
                                }
                                Some(Rect::from_center_size(pos2(rect.center().x, y), vec2(rect.width(), ROW_H * 0.7)))
                            }
                            _ => None,
                        };
                        ghost.push((*id, ns, ns + x.duration.as_secs_f64(), tr));
                    }
                    if resp.drag_stopped() {
                        self.timeline.drag = Drag::None;
                        self.timeline.motion.skip.extend(ids.iter().map(|x| x.0));
                        let fr = |t: f64| Time::from_secs_f64(t).snap_to_frame(fps);
                        let ops: Vec<EditOp> = match target {
                            Target::New(k, upper) => {
                                // neue Spur(en) + Verschieben = ein Verlaufsschritt
                                let moves: Vec<(ClipId, Time)> =
                                    ids.iter().map(|(id, st)| (*id, fr((st + delta).max(0.0)))).collect();
                                let r = move_to_new_tracks(&mut self.doc.lock().unwrap_or_else(|e| e.into_inner()), k, upper, &moves);
                                match r {
                                    Ok(()) => {
                                        self.sync_project();
                                        // neue Spur mit dem Clip weich in den sichtbaren Bereich rollen
                                        let t = self.project.find_clip(clip).map(|(t, _)| t.id);
                                        self.timeline.reveal = t.map(|t| (t, now_s, self.timeline.vscroll));
                                    }
                                    Err(e) => self.toast_err(e.to_string()),
                                }
                                Vec::new()
                            }
                            Target::Track(t) => vec![EditOp::MoveClip {
                                clip,
                                track: Some(tracks[t].id),
                                start: fr(s),
                            }],
                            Target::Same => ids
                                .iter()
                                .map(|(id, st)| EditOp::MoveClip {
                                    clip: *id,
                                    track: None,
                                    start: fr((st + delta).max(0.0)),
                                })
                                .collect(),
                        };
                        let moved = ids.iter().any(|(_, st)| (st + delta - st).abs() > 1e-4)
                            || !matches!(target, Target::Same);
                        if moved {
                            self.apply_batch(ops);
                        }
                        ghost.clear();
                    }
                }
                Drag::Trim { clip, edge } => {
                    let Some((_, c)) = self.project.find_clip(clip) else {
                        return;
                    };
                    let cands = snap_cands(&[clip]);
                    let raw = xt(p.x);
                    let t = snap_to(raw, &cands)
                        .inspect(|s| snap_line = Some(*s))
                        .unwrap_or(raw);
                    let (cs, ce) = (c.start.as_secs_f64(), c.end().as_secs_f64());
                    // Videos/Bilder: Grenzen der Quelle (Audio darf länger werden)
                    let (min_s, max_e) = match &c.content {
                        ClipContent::Media {
                            media,
                            kind: MediaKind::Video,
                        } => {
                            let len = self
                                .project
                                .media(*media)
                                .map(|m| m.duration.as_secs_f64())
                                .unwrap_or(f64::MAX);
                            let sp = c.speed.max(0.01) as f64;
                            (
                                cs - c.source_in.as_secs_f64() / sp,
                                cs + (len - c.source_in.as_secs_f64()) / sp,
                            )
                        }
                        _ => (0.0, f64::MAX),
                    };
                    let (s, e) = match edge {
                        Edge::Start => (t.clamp(min_s.max(0.0), ce - 0.05), ce),
                        Edge::End => (cs, t.clamp(cs + 0.05, max_e)),
                    };
                    ghost.push((clip, s, e, None));
                    if resp.drag_stopped() {
                        self.timeline.drag = Drag::None;
                        self.timeline.motion.skip.insert(clip);
                        let to = if edge == Edge::Start { s } else { e };
                        if (to - if edge == Edge::Start { cs } else { ce }).abs() > 1e-4 {
                            self.apply(EditOp::TrimClip {
                                clip,
                                edge,
                                to: Time::from_secs_f64(to).snap_to_frame(fps),
                            });
                        }
                        ghost.clear();
                    }
                }
                Drag::Roll { left, right } => {
                    let raw = xt(p.x);
                    let t = snap_to(raw, &snap_cands(&[left, right]))
                        .inspect(|s| snap_line = Some(*s))
                        .unwrap_or(raw);
                    if let (Some((_, l)), Some((_, r))) =
                        (self.project.find_clip(left), self.project.find_clip(right))
                    {
                        let t = t.clamp(l.start.as_secs_f64() + 0.04, r.end().as_secs_f64() - 0.04);
                        ghost.push((left, l.start.as_secs_f64(), t, None));
                        ghost.push((right, t, r.end().as_secs_f64(), None));
                        painter.text(
                            pos2(tx(t), ruler.bottom() + 2.0),
                            egui::Align2::CENTER_TOP,
                            crate::tr("Rollen"),
                            egui::FontId::proportional(theme::FS_HINT),
                            theme::ACCENT(),
                        );
                    }
                    if resp.drag_stopped() {
                        self.timeline.drag = Drag::None;
                        self.timeline.motion.skip.extend([left, right]);
                        self.roll_edit(left, right, t);
                        ghost.clear();
                    }
                }
                Drag::Slip { clip, src, x } => {
                    let Some((_, c)) = self.project.find_clip(clip) else {
                        return;
                    };
                    // nach rechts ziehen = früherer Ausschnitt (wie in anderen Schnittprogrammen)
                    let lim = crate::edit_tools::source_limit(self, c).unwrap_or(0.0);
                    let v = (src - ((p.x - x) / pps) as f64 * c.speed as f64).clamp(0.0, lim);
                    let cx = tx(c.start.as_secs_f64());
                    painter.text(
                        pos2(cx + 4.0, ruler.bottom() + 2.0),
                        egui::Align2::LEFT_TOP,
                        crate::tr(format!("Slippen: Quelle ab {v:.2} s")),
                        egui::FontId::proportional(theme::FS_HINT),
                        theme::ACCENT(),
                    );
                    if resp.drag_stopped() {
                        self.timeline.drag = Drag::None;
                        self.slip_edit(clip, v);
                    }
                }
                Drag::Marquee { origin, additive } => {
                    let r = Rect::from_two_pos(origin, p);
                    painter.rect_filled(r, 2.0, theme::overlay(14));
                    painter.rect_stroke(
                        r,
                        2.0,
                        Stroke::new(1.0_f32, theme::overlay(120)),
                        egui::StrokeKind::Inside,
                    );
                    let mut inside = Vec::new();
                    for row in rows.iter().filter(|r| r.track != REC_ROW) {
                        for c in &tracks[row.track].clips {
                            let cr = Rect::from_min_max(
                                pos2(tx(c.start.as_secs_f64()), row.rect.top()),
                                pos2(tx(c.end().as_secs_f64()), row.rect.bottom()),
                            );
                            if cr.intersects(r) {
                                inside.push(c.id);
                            }
                        }
                    }
                    if !additive {
                        self.sel_extra.clear();
                        self.selected = None;
                    }
                    for c in &inside {
                        self.sel_extra.insert(*c);
                    }
                    if let Some(l) = inside.last() {
                        self.selected = Some(*l);
                        self.sel_extra.remove(l);
                    }
                    self.selected_transition = None;
                    if resp.drag_stopped() {
                        self.timeline.drag = Drag::None;
                    }
                }
                Drag::Audio { clip, handle, .. } => {
                    if let Some((body, c)) = clip_body(clip) {
                        let dur = c.duration.as_secs_f64() as f32;
                        let value = match handle {
                            AudioHandle::FadeIn => ((p.x - body.left()) / pps).clamp(0.0, dur),
                            AudioHandle::FadeOut => ((body.right() - p.x) / pps).clamp(0.0, dur),
                            AudioHandle::Volume => y_vol(body, p.y),
                        };
                        self.timeline.drag = Drag::Audio { clip, handle, value };
                        if resp.drag_stopped() {
                            self.apply_audio_handle(clip, handle, value);
                        }
                    }
                }
                Drag::None => {}
            }
            if resp.drag_stopped() {
                self.timeline.drag = Drag::None;
            }
        }

        // ---------- Asset-Ablage (Hervorhebung) ----------
        let drop_hover = resp.dnd_hover_payload::<DragAsset>();
        let mut drop_clip: Option<ClipId> = None;
        let mut drop_cut: Option<ClipId> = None;
        // Spur beim Ablegen: dieselbe Regel wie für Dateien von außen ([`crate::file_drop::lane_at`]).
        // Effekt/Filter auf einem Clip mit Bild wirkt dagegen auf diesen Clip.
        let drows: Vec<(usize, Rect)> = rows.iter().filter(|r| r.track != REC_ROW).map(|r| (r.track, r.rect)).collect();
        let asset_lane = |a: &DragAsset, k: Option<TrackKind>, p: Pos2| -> crate::file_drop::Lane {
            let on_visual = hit(p).and_then(|h| tracks[h.2].clips.iter().find(|c| c.id == h.0)).is_some_and(|c| {
                matches!(c.content, ClipContent::Media { kind: MediaKind::Video | MediaKind::Image, .. })
            });
            match k {
                Some(k) if !(on_visual && matches!(a, DragAsset::Effect(_) | DragAsset::Filter(_))) => {
                    crate::file_drop::lane_at(&tracks, &drows, lanes, p, k, ROW_GAP)
                }
                _ => crate::file_drop::Lane::Auto,
            }
        };
        if let (Some(payload), Some(p)) = (&drop_hover, pointer) {
            if let crate::file_drop::Lane::New { gap, .. } = asset_lane(payload, asset_kind(&self.project, payload), p) {
                insert_lines.push(gap_y(gap));
            }
        }
        if let (Some(payload), Some(p)) = (&drop_hover, pointer) {
            match &**payload {
                DragAsset::Effect(_) | DragAsset::Filter(_) => drop_clip = hit(p).map(|h| h.0),
                DragAsset::Transition(_) => {
                    if let Some(t) = row_at(p) {
                        let tr = &tracks[t];
                        drop_cut = tr
                            .clips
                            .iter()
                            .enumerate()
                            .filter(|(i, c)| {
                                tr.clips
                                    .get(i + 1)
                                    .is_some_and(|n| (n.start.0 - c.end().0).abs() < 2000)
                            })
                            .map(|(_, c)| c)
                            .filter(|c| (tx(c.end().as_secs_f64()) - p.x).abs() < 50.0)
                            .min_by(|a, b| {
                                (tx(a.end().as_secs_f64()) - p.x)
                                    .abs()
                                    .total_cmp(&(tx(b.end().as_secs_f64()) - p.x).abs())
                            })
                            .map(|c| c.id);
                    }
                }
                _ => {}
            }
        }

        // ---------- Dateien von außen (Explorer): Ziel und Platzhalter ----------
        self.drops.timeline_rect = Some(rect);
        let image_len = Time::from_secs_f64(self.settings.image_duration as f64);
        // (Balken, Name, Platzhalter nach dem Ablegen?)
        let mut file_ghosts: Vec<(Rect, String, bool)> = Vec::new();
        let mut place_ghosts = |items: &[crate::file_drop::Item], t: &crate::file_drop::Target, pending: bool| {
            let lane_rect = |l: &crate::file_drop::Lane, audio: bool| match l {
                crate::file_drop::Lane::Track(id) => drows.iter().find(|r| tracks[r.0].id == *id).map(|r| r.1),
                crate::file_drop::Lane::Auto => drows
                    .iter()
                    .find(|r| if audio { tracks[r.0].kind == TrackKind::Audio } else { tracks[r.0].main })
                    .map(|r| r.1),
                crate::file_drop::Lane::New { gap, .. } => {
                    Some(Rect::from_center_size(pos2(rect.center().x, gap_y(*gap)), vec2(rect.width(), ROW_H - 10.0)))
                }
            };
            let lanes_r = [lane_rect(&t.video, false), lane_rect(&t.audio, true)];
            for (it, s) in items.iter().zip(crate::file_drop::layout(items, t.at, image_len)) {
                let Some(r) = lanes_r[usize::from(it.audio)] else { continue };
                let e = s + it.dur.unwrap_or(image_len);
                let x = (tx(s.as_secs_f64()), tx(e.as_secs_f64()));
                file_ghosts.push((Rect::from_min_max(pos2(x.0, r.top() + 3.0), pos2(x.1.max(x.0 + 4.0), r.bottom() - 3.0)), it.name.clone(), pending));
            }
        };
        if let (Some(items), Some(p)) = (self.drops.hover.clone(), self.drops.pos.filter(|p| rect.contains(*p))) {
            let t = xt(p.x);
            let at = Time::from_secs_f64(snap_to(t, &snap_cands(&[])).unwrap_or(t)).snap_to_frame(fps);
            let target = crate::file_drop::Target {
                at,
                video: crate::file_drop::lane_at(&tracks, &drows, lanes, p, TrackKind::Video, ROW_GAP),
                audio: crate::file_drop::lane_at(&tracks, &drows, lanes, p, TrackKind::Audio, ROW_GAP),
            };
            for (l, audio) in [(target.video, false), (target.audio, true)] {
                if let (crate::file_drop::Lane::New { gap, .. }, true) = (l, items.iter().any(|i| i.audio == audio)) {
                    insert_lines.push(gap_y(gap));
                }
            }
            place_ghosts(&items, &target, false);
            painter.line_segment([pos2(tx(at.as_secs_f64()), ruler.bottom()), pos2(tx(at.as_secs_f64()), rect.bottom())], Stroke::new(1.5_f32, theme::ACCENT()));
            self.drops.target = Some(target);
        }
        for d in &self.drops.list {
            if let Some(t) = &d.target {
                place_ghosts(&d.items(image_len), t, true);
            }
        }
        if file_ghosts.iter().any(|g| g.2) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }

        // ---------- Ruler ----------
        let step = ruler_step(pps);
        let minor = step / 5.0;
        // Zeit aus dem Strichindex statt aufsummiert: keine Rundungsfehler bei vielen Strichen
        let mut k = (scroll / minor).floor() as i64;
        let mut t = k as f64 * minor;
        let rp = painter.with_clip_rect(Rect::from_min_max(
            pos2(x0, rect.top()),
            pos2(rect.right(), ruler.bottom()),
        ));
        while tx(t) < rect.right() {
            let x = tx(t);
            // vor 0:00 (Rand von „Ganze Zeitleiste zeigen“) keine Striche
            if x >= x0 - 1.0 && t > -1e-9 {
                let major = k.rem_euclid(5) == 0;
                let hh = if major { 7.0 } else { 3.0 };
                rp.line_segment(
                    [pos2(x, ruler.bottom() - hh), pos2(x, ruler.bottom())],
                    Stroke::new(
                        1.0_f32,
                        if major {
                            theme::TEXT_FAINT()
                        } else {
                            theme::BORDER()
                        },
                    ),
                );
                if major {
                    let label = ruler_label(t, step);
                    rp.text(
                        pos2(x + 3.0, ruler.top() + 3.0),
                        egui::Align2::LEFT_TOP,
                        crate::tr(label),
                        egui::FontId::proportional(theme::FS_HINT),
                        theme::TEXT_DIM(),
                    );
                }
            }
            k += 1;
            t = k as f64 * minor;
        }
        painter.line_segment(
            [
                pos2(rect.left(), ruler.bottom()),
                pos2(rect.right(), ruler.bottom()),
            ],
            Stroke::new(1.0_f32, theme::BORDER()),
        );
        // In/Out-Bereich
        if self.range_in.is_some() || self.range_out.is_some() {
            let a = self.range_in.map(|t| tx(t.as_secs_f64())).unwrap_or(x0);
            let b = self
                .range_out
                .map(|t| tx(t.as_secs_f64()))
                .unwrap_or(rect.right());
            let r = Rect::from_min_max(
                pos2(a.max(x0), ruler.top()),
                pos2(b.min(rect.right()), ruler.bottom()),
            );
            if r.width() > 0.0 {
                // dezentes Band unten im Lineal (Schleife: Akzent)
                let band = Rect::from_min_max(pos2(r.left(), ruler.bottom() - 5.0), r.max);
                rp.rect_filled(
                    band,
                    0.0,
                    if self.loop_range {
                        theme::ACCENT().gamma_multiply(0.7)
                    } else {
                        theme::TEXT_FAINT()
                    },
                );
                let lp = painter.with_clip_rect(lanes);
                for x in [r.left(), r.right()] {
                    if x > x0 && x < rect.right() - 1.0 {
                        lp.line_segment(
                            [pos2(x, ruler.bottom()), pos2(x, rect.bottom())],
                            Stroke::new(1.0_f32, theme::TEXT_FAINT()),
                        );
                    }
                }
            }
            for (t, glyph) in [(self.range_in, "["), (self.range_out, "]")] {
                if let Some(t) = t {
                    let x = tx(t.as_secs_f64());
                    if x >= x0 {
                        rp.line_segment(
                            [pos2(x, ruler.top()), pos2(x, ruler.bottom())],
                            Stroke::new(1.5_f32, theme::ACCENT()),
                        );
                        rp.text(
                            pos2(x, ruler.bottom() - 2.0),
                            egui::Align2::CENTER_BOTTOM,
                            glyph,
                            egui::FontId::monospace(11.0), // Glyphe der In-/Out-Marke am Lineal
                            theme::ACCENT(),
                        );
                    }
                }
            }
        }
        // Marker
        for m in self.project.timeline.markers.clone() {
            let x = tx(m.at.as_secs_f64());
            if x < x0 - 6.0 || x > rect.right() + 6.0 {
                continue;
            }
            let col = crate::navigate::marker_color(m.color);
            if m.duration > Time::ZERO {
                let x2 = tx((m.at + m.duration).as_secs_f64());
                rp.rect_filled(
                    Rect::from_min_max(
                        pos2(x.max(x0), ruler.bottom() - 4.0),
                        pos2(x2.min(rect.right()), ruler.bottom()),
                    ),
                    0.0,
                    col.gamma_multiply(0.6),
                );
            }
            let top = ruler.top() + 2.0;
            let pts = vec![
                pos2(x - 5.0, top),
                pos2(x + 5.0, top),
                pos2(x + 5.0, top + 7.0),
                pos2(x, top + 12.0),
                pos2(x - 5.0, top + 7.0),
            ];
            rp.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
            let hit = Rect::from_center_size(pos2(x, top + 6.0), vec2(12.0, 14.0));
            let r = ui.interact(hit, ui.id().with(("marker", m.id)), Sense::hover());
            let tip = if m.name.is_empty() {
                crate::navigate::chapter_time(m.at)
            } else {
                format!("{}  {}", crate::navigate::chapter_time(m.at), m.name)
            };
            let r = if m.note.is_empty() {
                r.on_hover_text(tip)
            } else {
                r.on_hover_text(format!("{tip}\n{}", m.note))
            };
            let _ = r;
        }

        if tracks.is_empty() && recording.is_none() {
            let r = Rect::from_center_size(
                pos2((x0 + rect.right()) / 2.0, lanes.center().y),
                vec2((rect.right() - x0 - 40.0).max(200.0), 58.0),
            );
            painter.add(egui::Shape::dashed_line(
                &[
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                    r.left_top(),
                ],
                Stroke::new(1.0_f32, theme::TEXT_FAINT()),
                6.0,
                4.0,
            ));
            painter.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                crate::tr(format!(
                    "{}   {}",
                    icon::FILM_STRIP,
                    self.t("timeline-empty")
                )),
                egui::FontId::proportional(theme::FS_TEXT),
                theme::TEXT_DIM(),
            );
        }

        // ---------- Clips ----------
        let lanes_clip = Rect::from_min_max(pos2(x0, lanes.top()), rect.max);
        let p = painter.with_clip_rect(lanes_clip);
        let mut waves = WaveMeshes { map: std::mem::take(&mut self.timeline.cache.waves), tess: None };
        let fast = theme::anim(theme::MOTION_FAST);
        let glides = std::mem::take(&mut self.timeline.motion.glides);
        let born = std::mem::take(&mut self.timeline.motion.born);
        // gelöschte Clips zuerst (unter den übrigen), mit ihrem letzten Aussehen ausblendend
        let gone = std::mem::take(&mut self.timeline.motion.gone);
        for g in &gone {
            let top = rows
                .iter()
                .find(|r| r.track != REC_ROW && tracks[r.track].id == g.track.id)
                .map_or(g.top, |r| r.rect.top());
            let cr = Rect::from_min_max(pos2(tx(g.s), top), pos2(tx(g.e), top + g.h));
            if !clip_visible(cr, lanes_clip) {
                continue;
            }
            let mut q = p.clone();
            q.multiply_opacity(fade_alpha((now_s - g.t0) as f32, fast, false));
            let rooms = clip_rooms(cr, lanes_clip, false);
            let mut h = make_head(ui.ctx(), &mut self.timeline.cache.kern, &g.clip, &self.project, None);
            let labels = head_galleys(&mut h, ui.ctx(), rooms.1, rooms.2);
            self.draw_clip(&q, &g.clip, &g.track, cr, lanes_clip, rect.right(), x0, false, false, rooms.0, labels, None);
        }
        for row in rows.iter().filter(|r| r.track != REC_ROW) {
            let tr = &tracks[row.track];
            // weit herausgezoomt: sehr schmale Clips als schlichte Balken, Nachbarn verschmolzen
            let mut bar: Option<(Rect, Color32)> = None;
            for c0 in &tr.clips {
                let (mut s, mut e) = (c0.start.as_secs_f64(), c0.end().as_secs_f64());
                let mut r = row.rect;
                let mut is_ghost = false;
                // gleitet noch: angezeigte Lage (Treffer-Tests bleiben an der Lage im Modell)
                if let Some(g) = glides.get(&c0.id) {
                    let (gs, ge, dy) = g.at(Place { s, e, track: tr.id }, (now_s - g.t0) as f32, secs);
                    (s, e, r) = (gs, ge, r.translate(vec2(0.0, dy)));
                }
                if let Some((_, gs, ge, gt)) = ghost.iter().find(|g| g.0 == c0.id) {
                    s = *gs;
                    e = *ge;
                    r = gt.unwrap_or(row.rect);
                    is_ghost = true;
                }
                let cr = Rect::from_min_max(pos2(tx(s), r.top()), pos2(tx(e), r.bottom()));
                let tiny = cr.width() < TINY_PX
                    && !is_ghost
                    && !self.is_selected(c0.id)
                    && hovered_clip != Some(c0.id)
                    && drop_clip != Some(c0.id);
                let cr = Rect::from_min_max(cr.min, pos2(cr.right().max(cr.left() + BAR_MIN), cr.bottom()));
                if !clip_visible(cr, lanes_clip) {
                    continue;
                }
                if tiny {
                    if let Some((b, col)) = merge_bar(&mut bar, cr, bar_color(tr, c0)) {
                        p.rect_filled(b, 1.0, col);
                    }
                    continue;
                }
                if let Some((b, col)) = bar.take() {
                    p.rect_filled(b, 1.0, col);
                }
                let live = self.live.as_ref().is_some_and(|l| l.id == c0.id);
                let missing = matches!(&c0.content, ClipContent::Media { media, .. } if self.timeline.cache.missing.contains(media));
                let rooms = clip_rooms(cr, lanes_clip, missing);
                let ctx = ui.ctx();
                // Texte: laufende Inspektor-Bearbeitung frisch, sonst aus dem Zwischenspeicher
                let labels = {
                    // wie `clip_ref`, nur ohne `self` ganz zu leihen
                    let shown = match &self.live {
                        Some(l) if live => l,
                        _ => c0,
                    };
                    let fam = match &shown.content {
                        ClipContent::Text(st) => Some(self.font_family(st.font_family.as_deref())),
                        _ => None,
                    };
                    if live {
                        let h = &mut make_head(ctx, &mut self.timeline.cache.kern, shown, &self.project, fam);
                        head_galleys(h, ctx, rooms.1, rooms.2)
                    } else {
                        let project = &self.project;
                        let h = self.timeline.cache.head(c0.id, |k| make_head(ctx, k, c0, project, fam));
                        head_galleys(h, ctx, rooms.1, rooms.2)
                    }
                };
                let mut c = std::borrow::Cow::Borrowed(self.clip_ref(c0));
                let audio_drag = match &self.timeline.drag {
                    Drag::Audio { clip, handle, value }
                        if has_audio(&c)
                            && (*clip == c.id || (self.is_selected(*clip) && self.is_selected(c.id))) =>
                    {
                        set_audio_handle(c.to_mut(), *handle, *value);
                        Some((*handle, *value))
                    }
                    _ => None,
                };
                let w = (!live && matches!(c, std::borrow::Cow::Borrowed(_))).then_some(&mut waves);
                // neu: blendet ein
                let faded;
                let pc = match born.get(&c0.id) {
                    Some(t0) => {
                        let mut q = p.clone();
                        q.multiply_opacity(fade_alpha((now_s - t0) as f32, fast, true));
                        faded = q;
                        &faded
                    }
                    None => &p,
                };
                self.draw_clip(pc, &c, tr, cr, lanes_clip, rect.right(), x0, is_ghost, missing, rooms.0, labels, w);
                self.proxy_line(ctx, pc, &c, cr);
                if missing && hovered_clip == Some(c.id) {
                    egui::show_tooltip_at_pointer(ui.ctx(), ui.layer_id(), egui::Id::new(("missing", c.id)), |ui| {
                        ui.label(crate::tr("Datei fehlt"));
                    });
                }
                if has_audio(&c)
                    && !is_ghost
                    && cr.width() >= 24.0
                    && (hovered_clip == Some(c.id) || audio_drag.is_some())
                {
                    let body = Rect::from_min_max(pos2(cr.left(), cr.top() + BAND_H), cr.max);
                    let ap = p.with_clip_rect(body.intersect(lanes_clip));
                    let vy = vol_y(body, c.volume_db);
                    let under = pointer.and_then(audio_hit);
                    let hot = |h: AudioHandle| {
                        audio_drag.is_some_and(|d| d.0 == h) || under.is_some_and(|x| x.0 == c.id && x.1 == h)
                    };
                    let strong = hot(AudioHandle::Volume);
                    ap.line_segment(
                        [pos2(body.left(), vy), pos2(body.right(), vy)],
                        Stroke::new(
                            if strong { 2.0_f32 } else { 1.2 },
                            if strong { Color32::WHITE } else { Color32::from_white_alpha(170) },
                        ),
                    );
                    for (h, x) in [
                        (
                            AudioHandle::FadeIn,
                            (body.left() + c.fade_in.as_secs_f64() as f32 * pps).max(body.left() + 5.0),
                        ),
                        (
                            AudioHandle::FadeOut,
                            (body.right() - c.fade_out.as_secs_f64() as f32 * pps).min(body.right() - 5.0),
                        ),
                    ] {
                        ap.circle(
                            pos2(x, body.top() + 5.0),
                            if hot(h) { 5.0 } else { 3.8 },
                            Color32::WHITE,
                            Stroke::new(1.0_f32, Color32::from_black_alpha(140)),
                        );
                    }
                    let own = matches!(&self.timeline.drag, Drag::Audio { clip, .. } if *clip == c.id);
                    if let Some((h, v)) = audio_drag.filter(|_| own) {
                        let txt = match h {
                            AudioHandle::Volume => format!("{v:+.1} dB"),
                            _ => format!("{v:.1} s"),
                        };
                        let at = pointer.unwrap_or(body.center()) + vec2(10.0, -18.0);
                        let g = p.layout_no_wrap(txt, egui::FontId::proportional(theme::FS_HINT), Color32::WHITE);
                        let bg = Rect::from_min_size(at, g.size()).expand2(vec2(5.0, 2.0));
                        p.rect_filled(bg, theme::R_CTRL, Color32::from_black_alpha(210));
                        p.galley(at, g, Color32::WHITE);
                    }
                }
                let sel = self.is_selected(c.id);
                if hovered_clip == Some(c.id) && !sel {
                    p.rect_stroke(
                        cr,
                        theme::R_CTRL,
                        Stroke::new(1.0_f32, Color32::from_white_alpha(90)),
                        egui::StrokeKind::Inside,
                    );
                }
                if drop_clip == Some(c.id) {
                    p.rect_stroke(
                        cr,
                        theme::R_CTRL,
                        Stroke::new(2.0_f32, theme::ACCENT()),
                        egui::StrokeKind::Inside,
                    );
                }
                if sel {
                    p.rect_stroke(
                        cr,
                        theme::R_CTRL,
                        Stroke::new(
                            if self.selected == Some(c.id) {
                                2.0_f32
                            } else {
                                1.5
                            },
                            theme::ACCENT(),
                        ),
                        egui::StrokeKind::Inside,
                    );
                    if self.selected == Some(c.id) && self.sel_extra.is_empty() {
                        for gx in [cr.left() + 3.5, cr.right() - 3.5] {
                            let g = Rect::from_center_size(
                                pos2(gx, cr.center().y),
                                vec2(4.0, (cr.height() * 0.45).min(20.0)),
                            );
                            p.rect_filled(g, 2.0, theme::ACCENT());
                        }
                        // Keyframes
                        for kt in c.keyframes.times() {
                            let kx = tx(s + kt.as_secs_f64());
                            if kx >= cr.left() && kx <= cr.right() {
                                let here = (self.playhead - c.start - kt).0.abs() < 20_000;
                                theme::diamond(
                                    &p,
                                    pos2(kx, cr.bottom() - 6.0),
                                    4.0,
                                    Some(if here {
                                        theme::ACCENT()
                                    } else {
                                        Color32::from_white_alpha(170)
                                    }),
                                    Stroke::new(1.0_f32, Color32::from_black_alpha(160)),
                                );
                            }
                        }
                    }
                }
            }
            if let Some((b, col)) = bar {
                p.rect_filled(b, 1.0, col);
            }
        }
        self.timeline.cache.waves = waves.map;
        // abgelaufene Bewegungen entfernen; solange etwas läuft, weiter zeichnen
        let m = &mut self.timeline.motion;
        m.glides = glides;
        m.glides.retain(|_, g| ((now_s - g.t0) as f32) < secs);
        m.born = born;
        m.born.retain(|_, t| ((now_s - *t) as f32) < fast);
        m.gone = gone;
        m.gone.retain(|g| ((now_s - g.t0) as f32) < fast);
        let moving = !m.glides.is_empty() || !m.born.is_empty() || !m.gone.is_empty() || !m.spacers.is_empty()
            || m.heights.values().any(|g| g.0 != g.1 && ((now_s - g.2) as f32) < secs);
        if moving {
            ui.ctx().request_repaint();
        }
        m.rows = rows.iter().filter(|r| r.track != REC_ROW).map(|r| (tracks[r.track].id, (r.rect.top(), r.rect.height()))).collect();
        m.order = rows.iter().filter(|r| r.track != REC_ROW).map(|r| tracks[r.track].id).collect();
        // Übergänge
        for (cid, r) in &tblocks {
            let sel = self.selected_transition == Some(*cid);
            let hov = pointer.is_some_and(|pp| r.contains(pp));
            p.rect_filled(
                *r,
                theme::R_CTRL,
                if sel {
                    theme::ACCENT()
                } else if hov {
                    theme::TEXT_FAINT()
                } else {
                    theme::BG_HOVER()
                },
            );
            p.rect_stroke(
                *r,
                theme::R_CTRL,
                Stroke::new(1.0_f32, Color32::from_black_alpha(140)),
                egui::StrokeKind::Outside,
            );
            p.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                icon::INTERSECT,
                egui::FontId::proportional(11.0), // Icon-Glyphe im Übergangsblock
                if sel { theme::ACCENT_INK() } else { theme::TEXT() },
            );
        }
        if let Some(cid) = drop_cut {
            if let Some((_, c)) = self.project.find_clip(cid) {
                let x = tx(c.end().as_secs_f64());
                if let Some(row) = rows.iter().find(|r| {
                    r.track != REC_ROW && tracks[r.track].clips.iter().any(|x| x.id == cid)
                }) {
                    p.rect_filled(
                        Rect::from_center_size(
                            pos2(x, row.rect.center().y),
                            vec2(22.0, row.rect.height() - 6.0),
                        ),
                        theme::R_CTRL,
                        Color32::from_white_alpha(70),
                    );
                }
            }
        }
        // Aufnahme läuft: wachsender Clip mit Live-Waveform
        if let (Some((start, elapsed, peaks)), Some(row)) =
            (&recording, rows.iter().find(|r| r.track == REC_ROW))
        {
            let cr = Rect::from_min_max(
                pos2(tx(*start), row.rect.top()),
                pos2(tx(start + elapsed), row.rect.bottom()),
            );
            p.rect_filled(cr, theme::R_CTRL, theme::DANGER_BG());
            let base = cr.bottom() - 3.0;
            let wh = cr.height() - BAND_H - 6.0;
            let mut x = cr.left().max(x0);
            while x < cr.right().min(rect.right()) {
                let ts = ((x - cr.left()) / pps) as f64;
                let i = (ts * WAVE_PER_SEC as f64) as usize;
                let j = (((ts + 2.0 / pps as f64) * WAVE_PER_SEC as f64) as usize).max(i + 1);
                let v = peaks
                    .get(i..j.min(peaks.len()))
                    .map(|s| s.iter().copied().fold(0.0f32, f32::max))
                    .unwrap_or(0.0);
                let hh = (v.sqrt() * wh).max(1.0);
                p.line_segment(
                    [pos2(x, base), pos2(x, base - hh)],
                    Stroke::new(1.5_f32, theme::DANGER_TEXT()),
                );
                x += 2.0;
            }
            p.text(
                cr.left_top() + vec2(6.0, 2.0),
                egui::Align2::LEFT_TOP,
                crate::tr(format!("{}  Aufnahme {:.1} s", icon::RECORD, elapsed)),
                egui::FontId::proportional(theme::FS_HINT),
                Color32::WHITE,
            );
            if self.playing && cr.right() > rect.right() - 60.0 {
                self.timeline.view_glide = None;
                self.timeline.scroll = (start + elapsed) - ((rect.right() - x0) / pps * 0.5) as f64;
            }
        }

        // ---------- Spurköpfe (über allem, damit verschobene Clips darunter laufen) ----------
        let head_bg = Rect::from_min_max(
            pos2(rect.left(), lanes.top()),
            pos2(x0 - 4.0, rect.bottom()),
        );
        painter.rect_filled(head_bg, 0.0, theme::BG_CARD());
        let hp = painter.with_clip_rect(head_bg);
        for row in &rows {
            let head = Rect::from_min_size(row.rect.min, vec2(HEADER_W - 8.0, row.rect.height()));
            if !head.intersects(head_bg) {
                continue;
            }
            if row.track == REC_ROW {
                hp.text(
                    head.left_center() + vec2(8.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    icon::MICROPHONE,
                    egui::FontId::proportional(13.0), // Icon-Glyphe im Spurkopf
                    theme::DANGER,
                );
                continue;
            }
            let tr = &tracks[row.track];
            if tr.main {
                hp.rect_filled(
                    head.shrink2(vec2(2.0, 1.0)),
                    theme::R_CTRL,
                    theme::BG_RAISED(),
                );
            }
            hp.text(
                head.left_center() + vec2(9.0, 0.0),
                egui::Align2::LEFT_CENTER,
                theme::track_icon(tr.kind),
                egui::FontId::proportional(13.0), // Icon-Glyphe im Spurkopf
                if tr.main {
                    theme::TEXT()
                } else {
                    theme::TEXT_DIM()
                },
            );
            let mut toggles = vec![(
                0usize,
                if tr.locked {
                    icon::LOCK_SIMPLE
                } else {
                    icon::LOCK_SIMPLE_OPEN
                },
                tr.locked,
                if tr.locked { "Entsperren" } else { "Sperren" },
            )];
            if tr.kind != TrackKind::Audio {
                toggles.push((
                    1,
                    if tr.hidden {
                        icon::EYE_SLASH
                    } else {
                        icon::EYE
                    },
                    tr.hidden,
                    if tr.hidden { "Spur einblenden" } else { "Spur verbergen" },

                ));
            }
            if matches!(tr.kind, TrackKind::Video | TrackKind::Audio) {
                toggles.push((
                    2,
                    if tr.muted {
                        icon::SPEAKER_SLASH
                    } else {
                        icon::SPEAKER_HIGH
                    },
                    tr.muted,
                    if tr.muted { "Stumm aufheben" } else { "Stumm" },
                ));
                let solo = self.solo.contains(&tr.id);
                toggles.push((
                    3,
                    icon::HEADPHONES,
                    solo,
                    if solo { "Solo aufheben" } else { "Solo (nur diese Spur hören)" },
                ));
            }
            // eingeklappt: nur Symbol und Name, die Schalter hätten keinen Platz
            if self.timeline.collapsed.contains(&tr.id) {
                toggles.clear();
            }
            // Spurname (eigener Name oder Art + Nummer)
            let own = tr.name.as_deref().map(crate::captions::shown_track_name);
            let name = own.unwrap_or_else(|| {
                let n = tracks
                    .iter()
                    .filter(|t| t.kind == tr.kind)
                    .position(|t| t.id == tr.id)
                    .unwrap_or(0)
                    + 1;
                let k = match tr.kind {
                    TrackKind::Audio => "Audio",
                    TrackKind::Text => "Text",
                    TrackKind::Effect => "Effekte",
                    TrackKind::Filter => "Filter",
                    _ => "Video",
                };
                format!("{} {n}", crate::tr(k))
            });
            let name_x = head.left() + name_column();
            if name_x < head.right() - 16.0 {
                let font = egui::FontId::proportional(theme::FS_HINT);
                let name = theme::elide(&name, head.right() - 6.0 - name_x, |s| theme::text_width(hp.ctx(), s, font.clone()));
                let g = hp.layout_no_wrap(name, font, theme::TEXT_DIM());
                let clip = Rect::from_min_max(
                    pos2(name_x, head.top()),
                    pos2(head.right() - 6.0, head.bottom()),
                );
                hp.with_clip_rect(clip.intersect(head_bg)).galley(
                    pos2(name_x, head.center().y - g.size().y / 2.0),
                    g,
                    theme::TEXT_DIM(),
                );
            }
            for (n, (k, glyph, on, tip)) in toggles.iter().enumerate() {
                let c = pos2(head.left() + 32.0 + n as f32 * 19.0, head.center().y);
                let r = Rect::from_center_size(c, vec2(18.0, 18.0));
                if !head_bg.contains(c) {
                    continue;
                }
                let br = ui
                    .interact(r, ui.id().with(("trk", tr.id, *k)), Sense::click())
                    .on_hover_text(crate::tr(*tip));
                if br.hovered() {
                    hp.rect_filled(r, theme::R_CTRL, theme::BG_RAISED());
                }
                hp.text(
                    c,
                    egui::Align2::CENTER_CENTER,
                    crate::tr(*glyph),
                    egui::FontId::proportional(12.0), // Icon-Glyphe der Spurkopf-Schalter
                    if *on {
                        theme::ACCENT()
                    } else {
                        theme::TEXT_DIM()
                    },
                );
                if br.clicked() && *k == 3 {
                    if !self.solo.remove(&tr.id) {
                        self.solo.insert(tr.id);
                    }
                    if self.playing {
                        self.restart_audio();
                    }
                } else if br.clicked() {
                    let (mut l, mut h, mut m) = (tr.locked, tr.hidden, tr.muted);
                    match k {
                        0 => l = !l,
                        1 => h = !h,
                        _ => m = !m,
                    }
                    self.apply(EditOp::SetTrackFlags {
                        track: tr.id,
                        locked: l,
                        hidden: h,
                        muted: m,
                    });
                }
            }
        }
        // Angehobene Spur und Einfüge-Linien (über die ganze Breite, auch über die Spurköpfe)
        let lp = painter.with_clip_rect(lanes);
        if let Some(r) = pressed_row.map(|k| rows[k].rect) {
            lp.rect_filled(r, theme::R_CTRL, theme::overlay(18));
        }
        // angehobene Spur: Fläche und Rahmen blenden schnell ein, die Zeile hebt sich um 2 px
        let lift = theme::fade(ui.ctx(), ui.id().with("track-lift"), lifted_row.is_some(), theme::MOTION_FAST);
        if let Some(r) = lifted_row.map(|k| rows[k].rect.translate(vec2(0.0, -2.0 * lift))) {
            lp.rect_filled(r.translate(vec2(0.0, 3.0)), theme::R_CTRL, Color32::from_black_alpha((60.0 * lift) as u8));
            lp.rect_filled(r, theme::R_CTRL, theme::ACCENT().gamma_multiply(0.18 * lift));
            lp.rect_stroke(r, theme::R_CTRL, Stroke::new(1.5_f32, theme::ACCENT().gamma_multiply(lift)), egui::StrokeKind::Inside);
        }
        for y in &insert_lines {
            lp.line_segment([pos2(rect.left() + 2.0, *y), pos2(rect.right() - 2.0, *y)], Stroke::new(2.0_f32, theme::ACCENT()));
            lp.circle_filled(pos2(rect.left() + 4.0, *y), 3.5, theme::ACCENT());
        }
        // Dateien von außen: Geisterbalken beim Ziehen, Platzhalter mit Lauflinie bis zur Prüfung
        let gp = painter.with_clip_rect(Rect::from_min_max(pos2(x0, lanes.top()), rect.max));
        for (r, name, pending) in &file_ghosts {
            gp.rect_filled(*r, theme::R_CTRL, theme::ACCENT().gamma_multiply(if *pending { 0.35 } else { 0.22 }));
            gp.rect_stroke(*r, theme::R_CTRL, Stroke::new(1.0_f32, theme::ACCENT()), egui::StrokeKind::Inside);
            let tp = gp.with_clip_rect(r.shrink(2.0).intersect(gp.clip_rect()));
            tp.text(pos2(r.left().max(x0) + 6.0, r.center().y), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(theme::FS_HINT), theme::TEXT());
            if *pending {
                let w = r.width();
                let f = (now_s * 0.8).rem_euclid(1.3) as f32 - 0.3;
                let a = r.left() + w * f.max(0.0);
                let b = r.left() + w * (f + 0.3).min(1.0);
                tp.line_segment([pos2(a, r.bottom() - 2.0), pos2(b, r.bottom() - 2.0)], Stroke::new(2.0_f32, theme::ACCENT()));
            }
        }
        // „+“ über den Spurköpfen (Ecke neben dem Lineal): neue Spur ganz oben (Bild) bzw. ganz unten (Ton)
        let corner = Rect::from_min_max(pos2(rect.left() + 4.0, rect.top()), pos2(x0 - 4.0, ruler.bottom()));
        let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(corner).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let add = theme::icon_button(&mut cui, icon::PLUS, "Spur hinzufügen", true, false);
        let pop = ui.make_persistent_id("add-track-menu");
        if add.clicked() {
            ui.memory_mut(|m| m.toggle_popup(pop));
        }
        let mut add_kind = None;
        egui::popup_below_widget(ui, pop, &add, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            for (k, name) in TRACK_KINDS {
                if menu_item(ui, theme::track_icon(k), name, "") {
                    add_kind = Some(k);
                }
            }
        });
        if let Some(kind) = add_kind {
            let index = self.project.timeline.tracks.len();
            self.apply(EditOp::InsertTrack { index, kind });
        }
        // Senkrechte Bildlaufleiste
        if overflow > 0.0 {
            let bar = Rect::from_min_max(
                pos2(rect.right() - 4.0, lanes.top() + 2.0),
                pos2(rect.right() - 1.0, rect.bottom() - 2.0),
            );
            let frac = view_h / content_h;
            let h = (bar.height() * frac).max(18.0);
            let y = bar.top() + (bar.height() - h) * (self.timeline.vscroll / overflow);
            painter.rect_filled(
                Rect::from_min_size(pos2(bar.left(), y), vec2(bar.width(), h)),
                2.0,
                theme::overlay(60),
            );
        }

        // ---------- Ablegen ----------
        if let Some(payload) = resp.dnd_release_payload::<DragAsset>() {
            let at = pointer.map(|p| {
                let t = xt(p.x);
                Time::from_secs_f64(snap_to(t, &snap_cands(&[])).unwrap_or(t)).snap_to_frame(fps)
            });
            let at = at.unwrap_or(self.playhead);
            let kind = asset_kind(&self.project, &payload);
            // neue Spur und Ablegen sind ein Verlaufsschritt (sonst bliebe nach Rückgängig eine leere Spur)
            let rev = self.doc.lock().unwrap_or_else(|e| e.into_inner()).revision();
            let mut track = None;
            match (pointer.map(|p| asset_lane(&payload, kind, p)), kind) {
                (Some(crate::file_drop::Lane::Track(t)), _) => track = Some(t),
                (Some(crate::file_drop::Lane::New { index, .. }), Some(k)) => {
                    if let Some(OpOutput::Track(t)) = self.apply(EditOp::InsertTrack { index, kind: k }) {
                        track = Some(t);
                        self.timeline.reveal = Some((t, now_s, self.timeline.vscroll));
                    }
                }
                _ => {}
            }
            let on = match &*payload {
                DragAsset::Transition(_) => drop_cut,
                _ => pointer.and_then(|p| hit(p).map(|h| h.0)),
            };
            if matches!(&*payload, DragAsset::Transition(_)) && on.is_none() {
                self.toast_err("Lege den Übergang auf einen Schnitt zwischen zwei Clips");
            } else {
                self.drop_asset(&payload, at, track, on);
            }
            self.doc.lock().unwrap_or_else(|e| e.into_inner()).squash_since(rev);
        } else if let (Some(payload), Some(p)) = (&drop_hover, pointer) {
            if !matches!(
                &**payload,
                DragAsset::Effect(_) | DragAsset::Filter(_) | DragAsset::Transition(_)
            ) {
                painter.line_segment(
                    [pos2(p.x, ruler.bottom()), pos2(p.x, rect.bottom())],
                    Stroke::new(1.5_f32, theme::ACCENT()),
                );
            }
        }

        // Einrast-Linie
        if let Some(s) = snap_line {
            let x = tx(s);
            painter.line_segment(
                [pos2(x, ruler.bottom()), pos2(x, rect.bottom())],
                Stroke::new(1.0_f32, theme::SNAP),
            );
        }

        // ---------- Playhead ----------
        let px = tx(self.playhead.as_secs_f64());
        if px >= x0 - 1.0 && px <= rect.right() {
            painter.line_segment(
                [pos2(px, rect.top() + 12.0), pos2(px, rect.bottom())],
                Stroke::new(1.2_f32, theme::ACCENT()),
            );
            let hr = Rect::from_center_size(pos2(px, rect.top() + 7.0), vec2(10.0, 12.0));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    hr.left_top(),
                    hr.right_top(),
                    pos2(hr.right(), hr.bottom() - 4.0),
                    pos2(px, hr.bottom() + 1.0),
                    pos2(hr.left(), hr.bottom() - 4.0),
                ],
                theme::ACCENT(),
                Stroke::NONE,
            ));
        }
        // Playhead folgen: kurz hinterhergleiten (blättern) statt hart umspringen
        if self.playing && recording.is_none() && (px > rect.right() - 40.0 || px < x0) && self.timeline.view_glide.is_none() {
            self.timeline.scroll_to((self.playhead.as_secs_f64() - 1.0).max(0.0), now_s);
        }

        // ---------- Kontextmenüs ----------
        resp.context_menu(|ui| self.timeline_menu(ui));
    }

    /// Playhead setzen (Klick); `clamp` begrenzt aufs Projektende.
    fn set_playhead_ui(&mut self, t: Time, clamp: bool) {
        let fps = self.project.settings.fps;
        if self.playing {
            self.stop_playback();
        }
        let t = if clamp {
            t.min(self.project.duration())
        } else {
            t
        };
        self.playhead = t.snap_to_frame(fps);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_clip(
        &self,
        p: &egui::Painter,
        c: &Clip,
        tr: &Track,
        cr: Rect,
        lanes_clip: Rect,
        right: f32,
        x0: f32,
        ghost: bool,
        missing: bool,
        head_x: f32,
        (head, body_text): (Option<std::sync::Arc<egui::Galley>>, Option<std::sync::Arc<egui::Galley>>),
        waves: Option<&mut WaveMeshes>,
    ) {
        let pps = self.timeline.pps;
        let kind = c.kind();
        let mut col = theme::clip_color(kind);
        if tr.hidden || (tr.muted && kind == TrackKind::Audio) || c.disabled {
            col = col.gamma_multiply(0.4);
        }
        if ghost {
            col = col.gamma_multiply(0.85);
        }
        p.rect_filled(cr, theme::R_CTRL, col);
        let band = Rect::from_min_max(
            cr.min,
            pos2(cr.right(), (cr.top() + BAND_H).min(cr.bottom())),
        );
        if c.group.is_some() {
            p.rect_stroke(
                cr.shrink(0.5),
                theme::R_CTRL,
                Stroke::new(1.0_f32, Color32::from_white_alpha(140)),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(l) = c.label {
            // Farbmarkierung als Kopfband
            p.rect_filled(
                band,
                egui::CornerRadius {
                    nw: theme::R_CTRL as u8,
                    ne: theme::R_CTRL as u8,
                    sw: 0,
                    se: 0,
                },
                crate::navigate::marker_color(l).gamma_multiply(0.85),
            );
        }
        let body = Rect::from_min_max(pos2(cr.left(), band.bottom()), cr.max);
        let inner = p.with_clip_rect(body.intersect(lanes_clip));
        if let ClipContent::Media { media, kind: mk } = &c.content {
            if matches!(mk, MediaKind::Video | MediaKind::Image) {
                if let Some(tex) = self.thumbs.get(media) {
                    let ts = tex.size_vec2();
                    let th = body.height();
                    let tw = (ts.x / ts.y * th).max(8.0);
                    let mut x = body.left().max(x0 - tw);
                    // auf Kachelraster ausrichten
                    x = body.left() + ((x - body.left()) / tw).floor() * tw;
                    while x < body.right().min(right) {
                        let uv = if c.fx.flip_h {
                            Rect::from_min_max(pos2(1.0, 0.0), pos2(0.0, 1.0))
                        } else {
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
                        };
                        inner.image(
                            tex.id(),
                            Rect::from_min_size(pos2(x, body.top()), vec2(tw, th)),
                            uv,
                            Color32::from_white_alpha(if c.disabled { 90 } else { 215 }),
                        );
                        x += tw;
                    }
                }
            }
            let src_len = self
                .project
                .media(*media)
                .map(|m| m.duration.as_secs_f64())
                .unwrap_or(f64::MAX);
            let stream = c.audio_stream.filter(|&n| self.project.media(*media).is_some_and(|m| n < m.audio_streams)).unwrap_or(0);
            if let Some(peaks) = self.waves.get(&(*media, stream)).filter(|w| !w.is_empty()) {
                let ps = peaks.as_ptr() as usize;
                let audio = *mk == MediaKind::Audio;
                let wave_h = if audio {
                    body.height() - 4.0
                } else {
                    (body.height() * 0.32).min(13.0)
                };
                let base = body.bottom() - 2.0;
                let wcol = if audio {
                    Color32::from_rgb(0x8F, 0xE3, 0xB8)
                } else {
                    Color32::from_rgba_unmultiplied(0xC8, 0xF0, 0xF2, 160)
                };
                // Ton stumm (Spur stumm oder −60 dB), obwohl die Datei Ton hat: Wellenform blass
                let wcol = if tr.muted || c.volume_db <= -59.9 { wcol.gamma_multiply(0.35) } else { wcol };
                if !audio {
                    inner.rect_filled(
                        Rect::from_min_max(
                            pos2(body.left(), base - wave_h - 1.0),
                            pos2(body.right(), base + 2.0),
                        ),
                        0.0,
                        Color32::from_black_alpha(100),
                    );
                }
                let x_end = body.right().min(right);
                let bars = |out: &mut Vec<egui::Shape>| {
                    let mut x = body.left().max(x0);
                    while x < x_end {
                        let rel = Time::from_secs_f64(((x - body.left()) / pps) as f64);
                        let t_src = c.source_time(c.start + rel).as_secs_f64();
                        if t_src < src_len {
                            let i = (t_src * WAVE_PER_SEC as f64) as usize;
                            let j = (((t_src + 2.0 * c.speed as f64 / pps as f64) * WAVE_PER_SEC as f64) as usize).max(i + 1);
                            let v = peaks
                                .get(i..j.min(peaks.len()))
                                .map(|s| s.iter().copied().fold(0.0f32, f32::max))
                                .unwrap_or(0.0);
                            let h = (v.sqrt() * wave_h * 10f32.powf(c.volume_db.min(6.0) / 40.0)).clamp(1.0, wave_h.max(1.0));
                            out.push(egui::Shape::line_segment([pos2(x, base), pos2(x, base - h)], Stroke::new(1.5_f32, wcol)));
                        } else {
                            // verlängert: Stille
                            out.push(egui::Shape::line_segment(
                                [pos2(x, base), pos2(x, base - 1.0)],
                                Stroke::new(1.0_f32, Color32::from_white_alpha(50)),
                            ));
                        }
                        x += 2.0;
                    }
                };
                // Balken als fertiges Netz zwischenspeichern (eine Form statt einer je 2 px);
                // laufende Bearbeitung im Inspektor wird frisch gezeichnet
                let cl = inner.clip_rect();
                let key = WaveKey {
                    geo: [body.min.x, body.min.y, body.max.x, body.max.y, x0, x_end, pps, c.volume_db, cl.min.x, cl.min.y, cl.max.x, cl.max.y]
                        .map(f32::to_bits),
                    peaks: (ps, peaks.len()),
                    muted: tr.muted,
                };
                match waves {
                    Some(w) => {
                        let mesh = match w.map.get(&c.id) {
                            Some((k, m)) if *k == key => m.clone(),
                            _ => {
                                let mut shapes = Vec::new();
                                bars(&mut shapes);
                                let m = w.tessellate(p.ctx(), cl, shapes);
                                if w.map.len() >= HEAD_CAP {
                                    w.map.clear();
                                }
                                w.map.insert(c.id, (key, m.clone()));
                                m
                            }
                        };
                        inner.add(egui::Shape::Mesh(mesh));
                    }
                    None => {
                        let mut shapes = Vec::new();
                        bars(&mut shapes);
                        inner.extend(shapes);
                    }
                }
            }
            // Stille-Bereich bei verlängertem Ton schraffieren
            if *mk == MediaKind::Audio {
                let real_end = c.start.as_secs_f64()
                    + (src_len - c.source_in.as_secs_f64()) / c.speed.max(0.01) as f64;
                let xe = x0 + ((real_end - self.timeline.scroll) as f32) * pps;
                if xe < cr.right() {
                    let sil = Rect::from_min_max(pos2(xe.max(cr.left()), body.top()), body.max);
                    inner.rect_filled(sil, 0.0, Color32::from_black_alpha(70));
                    let mut hx = sil.left() - sil.height();
                    while hx < sil.right() {
                        inner.line_segment(
                            [pos2(hx, sil.bottom()), pos2(hx + sil.height(), sil.top())],
                            Stroke::new(1.0_f32, Color32::from_white_alpha(18)),
                        );
                        hx += 7.0;
                    }
                }
            }
        }
        if let Some(g) = body_text {
            let at = egui::Align2::LEFT_CENTER.anchor_size(body.left_center() + vec2(8.0, 0.0), g.size()).min;
            inner.galley(at, g, Color32::from_white_alpha(230));
        }
        // Beat-Markierungen
        for b in c.beat_times() {
            let bx = x0 + (((b.as_secs_f64() - self.timeline.scroll) as f32) * pps)
                - (cr.left()
                    - (x0 + ((c.start.as_secs_f64() - self.timeline.scroll) as f32) * pps));
            if bx >= cr.left() && bx <= cr.right() {
                inner.circle_filled(
                    pos2(bx, cr.bottom() - 4.0),
                    2.2,
                    theme::SNAP,
                );
            }
        }
        // Titelband
        p.rect_filled(
            band,
            egui::CornerRadius {
                nw: theme::R_CTRL as u8,
                ne: theme::R_CTRL as u8,
                sw: 0,
                se: 0,
            },
            Color32::from_black_alpha(80),
        );
        if missing {
            p.with_clip_rect(band.intersect(lanes_clip)).text(
                pos2(band.left().max(x0) + 6.0, band.center().y),
                egui::Align2::LEFT_CENTER,
                icon::WARNING,
                egui::FontId::proportional(theme::FS_HINT),
                theme::WARN_TEXT(),
            );
        }
        if let Some(head) = head {
            let at = egui::Align2::LEFT_CENTER.anchor_size(pos2(head_x, band.center().y), head.size()).min;
            p.with_clip_rect(band.shrink2(vec2(4.0, 0.0)).intersect(lanes_clip))
                .galley(at, head, Color32::from_white_alpha(230));
        }
        // Ton-Fades als Rampen
        if c.fade_in > Time::ZERO || c.fade_out > Time::ZERO {
            let fi = c.fade_in.as_secs_f64() as f32 * pps;
            let fo = c.fade_out.as_secs_f64() as f32 * pps;
            let st = Stroke::new(1.0_f32, Color32::from_white_alpha(150));
            if fi > 1.0 {
                inner.line_segment(
                    [
                        pos2(body.left(), body.bottom()),
                        pos2(body.left() + fi, body.top()),
                    ],
                    st,
                );
            }
            if fo > 1.0 {
                inner.line_segment(
                    [
                        pos2(body.right() - fo, body.top()),
                        pos2(body.right(), body.bottom()),
                    ],
                    st,
                );
            }
        }
        // gesperrte und verborgene Spuren abgedunkelt
        if tr.locked || tr.hidden {
            p.rect_filled(cr, theme::R_CTRL, Color32::from_black_alpha(110));
        }
    }

    fn timeline_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(230.0);
        let Some(target) = self.timeline.ctx else {
            ui.close_menu();
            return;
        };
        match target {
            CtxTarget::Clip(id) => {
                let Some((tr, c)) = self
                    .project
                    .find_clip(id)
                    .map(|(t, c)| (t.clone(), c.clone()))
                else {
                    ui.close_menu();
                    return;
                };
                let video = matches!(
                    c.content,
                    ClipContent::Media {
                        kind: MediaKind::Video,
                        ..
                    }
                );
                let visual = c.kind() == TrackKind::Video;
                if menu_item(ui, icon::COPY, "Kopieren", "Strg+C") {
                    self.copy_selection();
                }
                if menu_item(ui, icon::SCISSORS, "Ausschneiden", "Strg+X") {
                    self.cut_selection();
                }
                if !self.clipboard.is_empty()
                    && menu_item(ui, icon::CLIPBOARD_TEXT, "Einfügen", "Strg+V")
                {
                    self.paste();
                }
                // ausgegraut, solange nichts kopiert ist
                let attrs_key = self.key_of("paste_attrs");
                if ui
                    .add_enabled_ui(!self.clipboard.is_empty(), |ui| {
                        menu_item(ui, icon::CLIPBOARD, "Attribute einfügen", &attrs_key)
                    })
                    .inner
                {
                    self.paste_attrs_open = true;
                }
                if menu_item(ui, icon::COPY_SIMPLE, "Duplizieren", &self.key_of("duplicate")) {
                    self.duplicate_selection();
                }
                if self.selection().len() >= 2
                    && menu_item(ui, icon::BOUNDING_BOX, "Gruppieren", &self.key_of("group"))
                {
                    self.group_selection(true);
                }
                if c.group.is_some()
                    && menu_item(ui, icon::SELECTION_SLASH, "Gruppe auflösen", &self.key_of("ungroup"))
                {
                    self.group_selection(false);
                }
                if menu_item(ui, icon::TRASH, "Löschen", "Entf") {
                    self.delete_selected();
                }
                if menu_item(ui, icon::TRASH_SIMPLE, "Löschen und Lücke schließen", &self.key_of("ripple_delete")) {
                    self.ripple_delete();
                }
                ui.separator();
                if menu_item(ui, icon::SCISSORS, "Teilen", &self.key_of("split")) {
                    self.split_at_playhead();
                }
                if menu_item(ui, icon::ARROW_LINE_LEFT, "Links vom Playhead löschen", &self.key_of("del_left")) {
                    self.delete_side(true);
                }
                if menu_item(ui, icon::ARROW_LINE_RIGHT, "Rechts vom Playhead löschen", &self.key_of("del_right")) {
                    self.delete_side(false);
                }
                ui.separator();
                if visual {
                    theme::submenu(
                        ui,
                        icon::PENCIL_SIMPLE,
                        "Bearbeiten",
                        |ui| {
                            ui.set_min_width(200.0);
                            if video && menu_item(ui, icon::SNOWFLAKE, "Standbild", "") {
                                self.freeze_frame();
                            }
                            if video
                                && menu_item(
                                    ui,
                                    icon::ARROWS_LEFT_RIGHT,
                                    if c.fx.reverse {
                                        "Vorwärts abspielen"
                                    } else {
                                        "Rückwärts"
                                    },
                                    "",
                                )
                            {
                                self.modify_selection(|c| c.fx.reverse = !c.fx.reverse);
                            }
                            if menu_item(ui, icon::FLIP_HORIZONTAL, "Horizontal spiegeln", "") {
                                self.modify_selection(|c| c.fx.flip_h = !c.fx.flip_h);
                            }
                            if menu_item(ui, icon::FLIP_VERTICAL, "Vertikal spiegeln", "") {
                                self.modify_selection(|c| c.fx.flip_v = !c.fx.flip_v);
                            }
                            if menu_item(ui, icon::ARROWS_CLOCKWISE, "90° drehen", "") {
                                self.modify_selection(|c| {
                                    c.transform.rotation_deg =
                                        ((c.transform.rotation_deg + 90.0 + 180.0)
                                            .rem_euclid(360.0))
                                            - 180.0
                                });
                            }
                            if menu_item(
                                ui,
                                icon::ARROW_COUNTER_CLOCKWISE,
                                "Transformation zurücksetzen",
                                "",
                            ) {
                                self.modify_selection(|c| {
                                    c.transform = Transform::default();
                                    for p in [Prop::X, Prop::Y, Prop::Scale, Prop::Rotation] {
                                        c.keyframes.0.remove(&p);
                                    }
                                });
                            }
                        },
                    );
                }
                if video && menu_item(ui, icon::WAVEFORM, "Ton extrahieren", "") {
                    self.extract_audio(id);
                }
                if matches!(
                    c.content,
                    ClipContent::Media {
                        kind: MediaKind::Video | MediaKind::Audio,
                        ..
                    }
                ) {
                    theme::submenu(
                        ui,
                        icon::GAUGE,
                        "Tempo",
                        |ui| {
                            for s in [0.25f32, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0] {
                                if ui
                                    .selectable_label(
                                        (c.speed - s).abs() < 1e-3,
                                        crate::tr(format!("{s}×")),
                                    )
                                    .clicked()
                                {
                                    self.set_speed_selection(s);
                                    ui.close_menu();
                                }
                            }
                        },
                    );
                }
                // gespeicherte Looks (wie die Preset-Leiste im Inspektor); ohne Presets kein Menü
                let is_text = matches!(c.content, ClipContent::Text(_));
                let looks: Vec<(usize, String)> = self
                    .presets
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.is_text() == is_text)
                    .map(|(i, p)| (i, p.name.clone()))
                    .collect();
                if !looks.is_empty() {
                    theme::submenu(ui, icon::SWATCHES, "Look anwenden", |ui| {
                        for (i, n) in &looks {
                            if ui.add(theme::menu_row(n.as_str()).min_width(160.0)).clicked() {
                                self.apply_preset(*i);
                                ui.close_menu();
                            }
                        }
                    });
                }
                // dieselbe Liste wie „Schnellaktionen“ in der Leiste (nicht passende sind ausgegraut)
                theme::submenu(ui, icon::LIGHTNING, "Schnellaktionen", |ui| {
                    self.tools_menu(ui)
                });
                if self.next_adjacent(id).is_some() {
                    if c.transition_out.is_some() {
                        if menu_item(ui, icon::INTERSECT, "Übergang entfernen", "") {
                            self.set_transition(id, None);
                        }
                    } else if menu_item(ui, icon::INTERSECT, "Übergang einfügen (Überblenden)", "")
                    {
                        self.set_transition(id, Some("fade"));
                    }
                }
                if menu_item(
                    ui,
                    if c.disabled {
                        icon::EYE
                    } else {
                        icon::EYE_SLASH
                    },
                    if c.disabled {
                        "Clip aktivieren"
                    } else {
                        "Clip deaktivieren"
                    },
                    "V",
                ) {
                    self.toggle_disabled();
                }
                theme::submenu(ui, icon::PALETTE, "Farbe", |ui| {
                    ui.set_min_width(150.0);
                    if menu_item(ui, icon::X, "Keine", "") {
                        self.modify_selection(|x| x.label = None);
                    }
                    for (i, (n, _)) in cl_core::MARKER_COLORS.iter().enumerate() {
                        // Zeile wie die übrigen Menüs; der Farbpunkt sitzt an der Stelle des Symbols
                        let r = ui.add(
                            theme::menu_row(format!("      {}", crate::tr(*n))).min_width(150.0),
                        );
                        ui.painter().circle_filled(
                            pos2(r.rect.left() + 14.0, r.rect.center().y),
                            4.5,
                            crate::navigate::marker_color(i as u8),
                        );
                        if r.clicked() {
                            self.modify_selection(|x| x.label = Some(i as u8));
                            ui.close_menu();
                        }
                    }
                });
                ui.separator();
                if menu_item(ui, icon::INFO, "Eigenschaften", "") {
                    self.props = Some(crate::props::Props::Clip(id));
                }
                let _ = tr;
            }
            CtxTarget::Transition(id) => {
                if menu_item(ui, icon::TRASH, "Übergang entfernen", "Entf") {
                    self.set_transition(id, None);
                }
            }
            CtxTarget::Ruler(marker) => {
                // dieselben Aktionen wie die Tasten (M, I, O, Alt+X, Strg+L)
                let ctx = ui.ctx().clone();
                if menu_item(ui, icon::BOOKMARK_SIMPLE, "Marker hier setzen", &self.key_of("marker")) {
                    self.run_action(&ctx, "marker");
                }
                if let Some(id) = marker {
                    if menu_item(ui, icon::PENCIL_SIMPLE, "Marker bearbeiten …", "") {
                        self.marker_draft = None;
                        self.marker_edit = Some(id);
                    }
                    if menu_item(ui, icon::TRASH, "Marker löschen", "") {
                        let mut ms = self.project.timeline.markers.clone();
                        ms.retain(|m| m.id != id);
                        self.apply(EditOp::SetMarkers { markers: ms });
                    }
                }
                ui.separator();
                for (g, label, action) in [
                    (icon::BRACKETS_SQUARE, "In-Punkt setzen", "mark_in"),
                    (icon::BRACKETS_SQUARE, "Out-Punkt setzen", "mark_out"),
                    (icon::X, "In/Out löschen", "clear_range"),
                ] {
                    if menu_item(ui, g, label, &self.key_of(action)) {
                        self.run_action(&ctx, action);
                    }
                }
                let mark = if self.loop_range { icon::CHECK } else { icon::REPEAT };
                if menu_item(ui, mark, "Bereich wiederholen", &self.key_of("loop")) {
                    self.run_action(&ctx, "loop");
                }
                ui.separator();
                let has = !self.project.timeline.markers.is_empty();
                if ui
                    .add_enabled_ui(has, |ui| menu_item(ui, icon::COPY, "Kapitel kopieren", ""))
                    .inner
                {
                    self.copy_chapters(&ctx);
                }
            }
            CtxTarget::Track(tid) => {
                let Some((idx, tr)) = self
                    .project
                    .timeline
                    .tracks
                    .iter()
                    .enumerate()
                    .find(|(_, t)| t.id == tid)
                    .map(|(i, t)| (i, t.clone()))
                else {
                    ui.close_menu();
                    return;
                };
                let audio = tr.kind == TrackKind::Audio;
                let (above, below) = if audio {
                    (idx, idx + 1)
                } else {
                    (idx + 1, idx)
                };
                if menu_item(ui, icon::PENCIL_SIMPLE, "Spur umbenennen …", "") {
                    self.track_rename = Some((tr.id, tr.name.clone().unwrap_or_default()));
                }
                for (label, glyph, index, show) in [
                    ("Spur darüber einfügen", icon::ROWS_PLUS_TOP, above, true),
                    ("Spur darunter einfügen", icon::ROWS_PLUS_BOTTOM, below, !tr.main),
                ] {
                    if !show {
                        continue;
                    }
                    theme::submenu(ui, glyph, label, |ui| {
                        for (k, name) in TRACK_KINDS {
                            // Ton liegt immer unter den Bildspuren: nur passende Arten anbieten
                            if (k == TrackKind::Audio) != audio {
                                continue;
                            }
                            if menu_item(ui, theme::track_icon(k), name, "") {
                                self.apply(EditOp::InsertTrack { index, kind: k });
                            }
                        }
                    });
                }
                if !tr.main {
                    if menu_item(ui, icon::ARROW_UP, "Spur nach oben", "") {
                        self.apply(EditOp::MoveTrack { track: tid, up: true });
                    }
                    if menu_item(ui, icon::ARROW_DOWN, "Spur nach unten", "") {
                        self.apply(EditOp::MoveTrack { track: tid, up: false });
                    }
                }
                ui.separator();
                if menu_item(
                    ui,
                    if tr.locked {
                        icon::LOCK_SIMPLE_OPEN
                    } else {
                        icon::LOCK_SIMPLE
                    },
                    if tr.locked { "Entsperren" } else { "Sperren" },
                    "",
                ) {
                    self.apply(EditOp::SetTrackFlags {
                        track: tid,
                        locked: !tr.locked,
                        hidden: tr.hidden,
                        muted: tr.muted,
                    });
                }
                if !audio
                    && menu_item(
                        ui,
                        if tr.hidden {
                            icon::EYE
                        } else {
                            icon::EYE_SLASH
                        },
                        if tr.hidden {
                            "Spur einblenden"
                        } else {
                            "Spur verbergen"
                        },
                        "",
                    )
                {
                    self.apply(EditOp::SetTrackFlags {
                        track: tid,
                        locked: tr.locked,
                        hidden: !tr.hidden,
                        muted: tr.muted,
                    });
                }
                if matches!(tr.kind, TrackKind::Video | TrackKind::Audio)
                    && menu_item(
                        ui,
                        if tr.muted {
                            icon::SPEAKER_HIGH
                        } else {
                            icon::SPEAKER_SLASH
                        },
                        if tr.muted { "Stumm aufheben" } else { "Stumm" },
                        "",
                    )
                {
                    self.apply(EditOp::SetTrackFlags {
                        track: tid,
                        locked: tr.locked,
                        hidden: tr.hidden,
                        muted: !tr.muted,
                    });
                }
                // Solo wie im Spurkopf (nur Vorschau, nicht im Export)
                let solo = self.solo.contains(&tid);
                if matches!(tr.kind, TrackKind::Video | TrackKind::Audio)
                    && menu_item(ui, icon::HEADPHONES, if solo { "Solo aufheben" } else { "Solo" }, "")
                {
                    if !self.solo.remove(&tid) {
                        self.solo.insert(tid);
                    }
                    if self.playing {
                        self.restart_audio();
                    }
                }
                ui.separator();
                if menu_item(ui, icon::SELECTION_ALL, "Alle Clips der Spur auswählen", "") {
                    self.select_only(None);
                    self.sel_extra = tr.clips.iter().map(|c| c.id).collect();
                    self.selected = tr.clips.last().map(|c| c.id);
                }
                if menu_item(ui, icon::ARROWS_MERGE, "Lücken schließen", "") {
                    self.close_gaps(Some(tid));
                }
                let folded = self.timeline.collapsed.contains(&tid);
                let (g, label) = if folded {
                    (icon::CARET_DOWN, "Spur ausklappen")
                } else {
                    (icon::CARET_UP, "Spur einklappen")
                };
                if menu_item(ui, g, label, "") && !self.timeline.collapsed.remove(&tid) {
                    self.timeline.collapsed.insert(tid);
                }
                theme::submenu(
                    ui,
                    icon::ARROWS_VERTICAL,
                    "Spurhöhe",
                    |ui| {
                        let cur = self.timeline.heights.get(&tid).copied().unwrap_or(1.0);
                        for (n, f) in [
                            ("Klein", 0.7f32),
                            ("Normal", 1.0),
                            ("Groß", 1.5),
                            ("Sehr groß", 2.2),
                        ] {
                            if ui
                                .selectable_label((cur - f).abs() < 0.01, crate::tr(n))
                                .clicked()
                            {
                                self.timeline.heights.insert(tid, f);
                                ui.close_menu();
                            }
                        }
                    },
                );
                if menu_item(ui, icon::INFO, "Eigenschaften", "") {
                    self.props = Some(crate::props::Props::Track(tid));
                }
                ui.separator();
                let label = if tr.main {
                    "Hauptspur leeren"
                } else {
                    "Spur löschen"
                };
                if menu_item(ui, icon::TRASH, label, "") {
                    self.apply(EditOp::RemoveTrack { track: tid });
                }
            }
            CtxTarget::Empty(track) => {
                if menu_item(ui, icon::CLIPBOARD_TEXT, "Einfügen", "Strg+V") {
                    self.paste();
                }
                if menu_item(ui, icon::SELECTION_ALL, "Alle auswählen", "Strg+A") {
                    self.select_all();
                }
                if let Some(t) = track {
                    if menu_item(ui, icon::ARROWS_MERGE, "Lücken schließen", "") {
                        self.close_gaps(Some(t));
                    }
                }
                if menu_item(ui, icon::ARROWS_MERGE, "Alle Lücken schließen", "") {
                    self.close_gaps(None);
                }
                ui.separator();
                let n = self.project.timeline.tracks.len();
                theme::submenu(ui, icon::PLUS, "Spur hinzufügen", |ui| {
                    for (k, name) in TRACK_KINDS {
                        if menu_item(ui, theme::track_icon(k), name, "") {
                            self.apply(EditOp::InsertTrack { index: n, kind: k });
                        }
                    }
                });
                if menu_item(ui, icon::TEXT_AA, "Text am Playhead", "") {
                    self.add_text_at(self.playhead, None, None);
                }
            }
        }
        let _ = Slider::new;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_x_to_time_with_zoom_and_scroll() {
        assert_eq!(time_at(300.0, 100.0, 0.0, 50.0), 4.0);
        assert_eq!(time_at(300.0, 100.0, 10.0, 200.0), 11.0);
        // links der Spurkante: nie vor 0:00
        assert_eq!(time_at(50.0, 100.0, 0.5, 50.0), 0.0);
    }

    #[test]
    fn narrow_toolbar_drops_zoom_before_toggles() {
        assert_eq!(toolbar_room(400.0), (true, true));
        assert_eq!(toolbar_room(250.0), (false, true));
        assert_eq!(toolbar_room(150.0), (false, false));
    }

    #[test]
    fn volume_line_round_trip() {
        let body = Rect::from_min_size(pos2(0.0, 100.0), vec2(200.0, 60.0));
        let mut db = VOL_MIN;
        while db <= VOL_MAX {
            let back = y_vol(body, vol_y(body, db));
            // 0,5-dB-Raster; um 0 dB rastet die Linie ein
            let tol = if db.abs() < 0.8 { 0.8 } else { 0.26 };
            assert!((back - db).abs() <= tol, "{db} dB -> {back} dB");
            db += 0.25;
        }
        // Knick: −12 dB auf einem Drittel, 0 dB bei zwei Dritteln, Grenzen an den Rändern
        assert!((vol_norm(-12.0) - 1.0 / 3.0).abs() < 1e-6);
        assert!((vol_norm(0.0) - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(vol_norm(VOL_MIN), 0.0);
        assert!((vol_norm(VOL_MAX) - 1.0).abs() < 1e-6);
        // alte Werte über +12 dB bleiben gespeichert, die Linie steht nur oben an
        assert!((vol_norm(20.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn track_head_click_versus_drag() {
        assert!(!track_lifted(0.0, 0.0), "Klick");
        assert!(!track_lifted(3.0, 0.1), "kleines Zittern beim Klicken");
        assert!(track_lifted(6.0, 0.0), "Bewegung");
        assert!(track_lifted(0.0, 0.25), "gedrückt gehalten");
    }

    #[test]
    fn head_toggles_per_track_kind() {
        let n = |k| toggle_count(&Track::new(k), false);
        assert_eq!((n(TrackKind::Video), n(TrackKind::Audio), n(TrackKind::Text)), (4, 3, 2));
        assert_eq!(toggle_count(&Track::new(TrackKind::Video), true), 0, "eingeklappt");
    }

    fn place(s: f64, e: f64, track: TrackId) -> Place {
        Place { s, e, track }
    }

    #[test]
    fn gliding_clip_moves_from_old_to_new_place() {
        let tr = TrackId::new();
        let g = ClipGlide { s0: 10.0, e0: 14.0, dy0: -40.0, t0: 5.0 };
        let to = place(20.0, 24.0, tr);
        let secs = theme::MOTION;
        assert_eq!(g.at(to, 0.0, secs), (10.0, 14.0, -40.0), "Anfang: alte Stelle");
        let (s, e, dy) = g.at(to, secs / 2.0, secs);
        assert!(s > 15.0 && s < 20.0 && (e - s - 4.0).abs() < 1e-9 && dy > -20.0 && dy < 0.0, "Mitte, auslaufend: {s} {dy}");
        assert_eq!(g.at(to, secs, secs), (20.0, 24.0, 0.0), "Ende: Ziel");
        assert_eq!(g.at(to, 0.0, 0.0), (20.0, 24.0, 0.0), "Animationen aus: sofort am Ziel");
    }

    #[test]
    fn target_change_mid_glide_starts_from_shown_place() {
        let tr = TrackId::new();
        let mut a = Track::new(TrackKind::Video);
        a.id = tr;
        let mut c = Clip::new(ClipContent::Text(TextStyle::default()), Time::ZERO, Time::from_secs_f64(2.0));
        a.clips.push(c.clone());
        let rows = HashMap::from([(tr, 100.0f32)]);
        let mut m = Motion::default();
        m.update(&[], std::slice::from_ref(&a), &rows, 0.0);
        assert!(m.glides.is_empty(), "erster Stand: nichts bewegt sich");
        // nach rechts verschoben (Rückgängig, Lücke schließen …)
        let mut b = a.clone();
        c.start = Time::from_secs_f64(10.0);
        b.clips[0] = c.clone();
        m.update(std::slice::from_ref(&a), std::slice::from_ref(&b), &rows, 1.0);
        let secs = theme::anim(theme::MOTION);
        if secs == 0.0 {
            assert!(m.glides.is_empty(), "Animationen aus: springt");
            return;
        }
        let g = m.glides[&c.id];
        assert_eq!((g.s0, g.t0), (0.0, 1.0));
        // halb unterwegs neues Ziel: die neue Bewegung beginnt an der gezeigten Stelle
        let half = g.at(place(10.0, 12.0, tr), secs / 2.0, secs).0;
        let mut d = b.clone();
        d.clips[0].start = Time::from_secs_f64(30.0);
        m.update(std::slice::from_ref(&b), std::slice::from_ref(&d), &rows, 1.0 + secs as f64 / 2.0);
        let g2 = m.glides[&c.id];
        assert!((g2.s0 - half).abs() < 1e-6, "{} != {half}", g2.s0);
        assert_eq!(g2.at(place(30.0, 32.0, tr), secs, secs).0, 30.0);
    }

    #[test]
    fn track_change_glides_vertically_and_own_drag_jumps() {
        let (t1, t2) = (TrackId::new(), TrackId::new());
        let id = ClipId::new();
        let mut m = Motion { places: HashMap::from([(id, place(0.0, 2.0, t1))]), ..Default::default() };
        m.rows = HashMap::from([(t1, (100.0, 34.0)), (t2, (200.0, 34.0))]);
        let mut nt = Track::new(TrackKind::Video);
        nt.id = t2;
        let mut c = Clip::new(ClipContent::Text(TextStyle::default()), Time::ZERO, Time::from_secs_f64(2.0));
        c.id = id;
        nt.clips.push(c);
        let rows = HashMap::from([(t1, 100.0f32), (t2, 200.0)]);
        // vom Nutzer gezogen: folgt dem Zeiger, kein Nachgleiten
        m.skip.insert(id);
        m.update(&[], std::slice::from_ref(&nt), &rows, 0.0);
        assert!(m.glides.is_empty() && m.skip.is_empty());
        // durch etwas anderes (Rückgängig) auf die andere Spur: gleitet von 100 nach 200 senkrecht
        m.places.insert(id, place(0.0, 2.0, t1));
        m.update(&[], std::slice::from_ref(&nt), &rows, 0.0);
        if theme::anim(theme::MOTION) > 0.0 {
            assert_eq!(m.glides[&id].dy0, -100.0);
        }
    }

    #[test]
    fn changes_find_moved_new_and_deleted_clips() {
        let (a, b) = (TrackId::new(), TrackId::new());
        let ids: Vec<ClipId> = (0..5).map(|_| ClipId::new()).collect();
        let prev = HashMap::from([
            (ids[0], place(0.0, 2.0, a)),
            (ids[1], place(2.0, 4.0, a)),
            (ids[2], place(4.0, 6.0, a)),
            (ids[3], place(0.0, 9.0, b)),
        ]);
        let next = HashMap::from([
            (ids[0], place(0.0, 1.0, a)), // nur kürzer (Teilen): springt
            (ids[1], place(1.0, 3.0, a)), // Lücke geschlossen: gleitet
            (ids[3], place(0.0, 9.0, a)), // andere Spur: gleitet
            (ids[4], place(1.0, 2.0, a)), // Rest nach dem Teilen, lag schon dort: blendet nicht ein
        ]);
        let mut ch = changes(&prev, &next, &HashSet::new());
        let mut moved = vec![ids[1], ids[3]];
        moved.sort();
        assert_eq!(ch.moved, moved);
        assert!(ch.born.is_empty());
        assert_eq!(ch.gone, vec![ids[2]]);
        // neuer Clip an freier Stelle blendet ein
        let mut next2 = next.clone();
        let fresh = ClipId::new();
        next2.insert(fresh, place(50.0, 52.0, b));
        ch = changes(&prev, &next2, &HashSet::new());
        assert_eq!(ch.born, vec![fresh]);
        // selbst gezogener Clip gleitet nicht
        assert!(!changes(&prev, &next, &HashSet::from([ids[1]])).moved.contains(&ids[1]));
        // anderes Projekt (kein gemeinsamer Clip) bzw. erster Stand: nichts
        let other = HashMap::from([(ClipId::new(), place(0.0, 1.0, a))]);
        assert_eq!(changes(&prev, &other, &HashSet::new()), Changes::default());
        assert_eq!(changes(&HashMap::new(), &next, &HashSet::new()), Changes::default());
    }

    #[test]
    fn too_many_moving_clips_jump() {
        let a = TrackId::new();
        for (n, glide) in [(MAX_GLIDES, true), (MAX_GLIDES + 1, false)] {
            let ids: Vec<ClipId> = (0..n + 1).map(|_| ClipId::new()).collect();
            let prev: HashMap<ClipId, Place> = ids.iter().enumerate().map(|(i, id)| (*id, place(i as f64, i as f64 + 1.0, a))).collect();
            // alle bis auf den ersten rücken eine Sekunde weiter
            let next: HashMap<ClipId, Place> =
                ids.iter().enumerate().map(|(i, id)| (*id, place(i as f64 + (i > 0) as u8 as f64, i as f64 + 2.0, a))).collect();
            assert_eq!(changes(&prev, &next, &HashSet::new()).moved.len(), if glide { n } else { 0 }, "{n}");
        }
    }

    #[test]
    fn track_height_glides_and_new_track_grows() {
        let secs = theme::MOTION;
        // erstes Bild: sofort am Ziel
        let (g, h) = height_at(None, 34.0, false, 0.0, secs);
        assert_eq!(h, 34.0);
        // einklappen: gleitet von 34 auf 16
        let (g, h) = height_at(Some(g), 16.0, false, 1.0, secs);
        assert_eq!(h, 34.0);
        let (_, mid) = height_at(Some(g), 16.0, false, 1.0 + secs as f64 / 2.0, secs);
        assert!(mid > 16.0 && mid < 25.0, "{mid}");
        assert_eq!(height_at(Some(g), 16.0, false, 2.0, secs).1, 16.0);
        // neue Spur wächst aus 0; Animationen aus: sofort
        assert_eq!(height_at(None, 34.0, true, 0.0, secs).1, 0.0);
        assert_eq!(height_at(None, 34.0, true, 0.0, 0.0).1, 34.0);
        // ein-/ausblenden
        assert_eq!((fade_alpha(0.0, 0.09, true), fade_alpha(0.09, 0.09, true)), (0.0, 1.0));
        assert_eq!((fade_alpha(0.0, 0.09, false), fade_alpha(0.0, 0.0, false)), (1.0, 0.0));
    }

    #[test]
    fn dragging_any_selected_clip_keeps_the_whole_selection() {
        // Strg-Auswahl: erst Video, dann Ton (Ton ist Hauptauswahl), gezogen wird am Video
        let (v, a) = (ClipId::new(), ClipId::new());
        let (mut sel, mut extra) = (Some(a), HashSet::from([v]));
        make_primary(&mut sel, &mut extra, v);
        assert_eq!(sel, Some(v));
        assert_eq!(extra, HashSet::from([a]), "Ton bleibt ausgewählt und wird mitgezogen");
        // am Ton gezogen: unverändert
        make_primary(&mut sel, &mut extra, v);
        assert_eq!((sel, extra.len()), (Some(v), 1));
    }

    #[test]
    fn new_track_is_rolled_into_view() {
        let lanes = Rect::from_min_max(pos2(0.0, 100.0), pos2(500.0, 300.0));
        let row = |top: f32, h: f32| Rect::from_min_size(pos2(0.0, top), vec2(500.0, h));
        assert_eq!(reveal_delta(row(150.0, 40.0), lanes), 0.0, "schon sichtbar");
        assert_eq!(reveal_delta(row(280.0, 42.0), lanes), 26.0, "unten halb abgeschnitten: so weit hoch");
        assert_eq!(reveal_delta(row(60.0, 34.0), lanes), -44.0, "oben abgeschnitten: zurück");
        // höher als die Ansicht: Oberkante bleibt sichtbar
        assert_eq!(reveal_delta(row(150.0, 400.0), lanes), 46.0);
    }

    /// Befund aus dem Klicktest: `PL_einfach` (ein Clip, 23:16 min) zeigte nach „Ganze Zeitleiste
    /// zeigen“ nur 0:00–10:20. Nach der Bewegung muss das ganze Projekt mit Rand zu sehen sein.
    #[test]
    fn show_all_fits_a_23_minute_project() {
        let dur = 23.0 * 60.0 + 16.0;
        for w in [600.0f32, 1100.0, 1428.0, 2400.0] {
            let mut st = TimelineState { view_w: w, pps: 60.0, scroll: 0.0, ..Default::default() };
            st.show_all(dur, 0.0);
            st.step_view(1.0);
            let (from, to) = (st.scroll, st.scroll + (w / st.pps) as f64);
            assert!(from < 0.0 && to > dur, "{w}: sichtbar {from:.1}…{to:.1} s");
            assert!(to - from < dur * 1.2, "{w}: nicht viel weiter als nötig");
        }
    }

    #[test]
    fn track_head_lifts_early() {
        assert!(track_lifted(0.0, 0.15), "nach 150 ms gehalten: hebt sich (Klicktest sah nach 100 ms noch nichts)");
        assert!(track_lifted(4.0, 0.0));
        assert!(!track_lifted(3.0, 0.1), "kurzer Klick bleibt ein Klick");
    }

    #[test]
    fn track_names_start_in_one_column() {
        // eine Spalte für alle Spurarten, eingeklappt oder nicht: immer hinter den eigenen Schaltern
        let col = name_column();
        for (k, _) in TRACK_KINDS {
            for folded in [false, true] {
                assert!(col >= 32.0 + toggle_count(&Track::new(k), folded) as f32 * 19.0 + 4.0, "{k:?} {folded}");
            }
        }
        assert_eq!(col, 32.0 + 4.0 * 19.0 + 4.0);
    }

    #[test]
    fn ruler_labels_rise_strictly() {
        assert_eq!(ruler_label(0.75, 0.25), "00:00.75");
        assert_eq!(ruler_label(1.5, 0.5), "00:01.5");
        assert_eq!(ruler_label(61.0, 1.0), "01:01");
        assert_eq!(ruler_label(3600.0, 600.0), "1:00:00");
        assert_eq!(ruler_label(5.0 * 3600.0 + 61.0, 1.0), "5:01:01");
        // Text → Sekunden (mm:ss[.f] bzw. h:mm:ss)
        let secs = |l: &str| l.split(':').fold(0.0, |a, p| a * 60.0 + p.parse::<f64>().unwrap());
        // jede Schrittweite: aufeinanderfolgende Hauptstriche streng aufsteigend, keine doppelten Texte
        for step in RULER_STEPS {
            let minor = step / 5.0;
            let labels: Vec<String> = (0..4000i64).filter(|k| k % 5 == 0).map(|k| k as f64 * minor).take_while(|t| *t < 200_000.0).map(|t| ruler_label(t, step)).collect();
            for w in labels.windows(2) {
                assert!(secs(&w[0]) < secs(&w[1]), "{step}: {} vor {}", w[0], w[1]);
            }
        }
        // ganz herausgezoomt (10 s … 6 h, schmal bis breit): Beschriftungen ≥ 70 px auseinander,
        // und es gibt höchstens rund zwei Dutzend
        for dur in [10.0, 600.0, 3600.0, 6.0 * 3600.0] {
            for w in [400.0f32, 1400.0, 3800.0] {
                let pps = min_pps(dur, w);
                let step = ruler_step(pps);
                assert!(step as f32 * pps >= 70.0, "{dur} {w}");
                assert!(w / (step as f32 * pps) < 60.0, "{dur} {w}: zu viele Striche");
            }
        }
    }

    #[test]
    fn zoom_out_shows_whole_project_with_room() {
        for dur in [0.0, 10.0, 644.0, 3600.0, 6.0 * 3600.0] {
            for w in [300.0f32, 1400.0, 3800.0] {
                let lo = min_pps(dur, w);
                // ganz herausgezoomt: Projekt (mind. 5 min) höchstens halb so breit wie die Ansicht
                assert!(dur.max(300.0) as f32 * lo <= 0.5 * w + 1e-3, "{dur} {w}");
                assert!(lo > 0.0 && lo <= 2.0);
                // „alles zeigen“: ganzes Projekt mit 5 % Rand links und rechts
                let (pps, scroll) = fit_all(dur, w, lo);
                if dur >= 1.0 && pps < PPS_MAX {
                    let x_end = ((dur - scroll) as f32) * pps;
                    assert!((((0.0 - scroll) as f32 * pps) / w - 0.05 / 1.1).abs() < 1e-3, "Rand links {dur} {w}");
                    assert!((x_end / w - 1.05 / 1.1).abs() < 1e-3, "Rand rechts {dur} {w}");
                }
                assert!(pps >= lo);
                // Regler: ganzer Bereich, hin und zurück
                assert_eq!(zoom_norm(lo, lo), 0.0);
                assert_eq!(zoom_norm(PPS_MAX, lo), 1.0);
                for n in [0.0f32, 0.25, 0.5, 0.9, 1.0] {
                    assert!((zoom_norm(norm_zoom(n, lo), lo) - n).abs() < 1e-4, "{n}");
                }
            }
        }
    }

    #[test]
    fn view_glide_keeps_time_under_pointer_and_ends_at_target() {
        let g = ViewGlide { pps: (10.0, 80.0), anchor: Anchor::At { t: 100.0, x: 300.0, floor: 0.0 }, t0: 0.0 };
        for age in [0.0f32, 0.04, 0.08, 0.12, 0.16, 0.5] {
            let (pps, scroll, done) = g.at(age, theme::MOTION);
            // Zeit unter dem Zeiger (300 px rechts der Kante) bleibt gleich
            assert!((scroll + (300.0 / pps) as f64 - 100.0).abs() < 1e-3, "{age}");
            assert_eq!(done, age >= theme::MOTION);
        }
        assert_eq!(g.at(0.0, theme::MOTION).0, 10.0, "Anfang");
        let mid = g.at(theme::MOTION / 2.0, theme::MOTION).0;
        assert!(mid > (10.0f32 * 80.0).sqrt() && mid < 80.0, "Mitte (auslaufend, logarithmisch): {mid}");
        assert_eq!(g.at(1.0, theme::MOTION).0, 80.0, "Ende genau am Ziel");
        // Animationen aus: sofort am Ziel
        assert_eq!(g.at(0.0, 0.0), (80.0, 100.0 - 300.0 / 80.0, true));
        // weit herausgezoomt am Anfang: Bildlauf nicht vor den erlaubten Rand
        let z = ViewGlide { pps: (100.0, 2.0), anchor: Anchor::At { t: 1.0, x: 600.0, floor: -3.0 }, t0: 0.0 };
        assert_eq!(z.at(1.0, theme::MOTION).1, -3.0);
        // Bildlauf gleitet (Maßstab bleibt), Ziel ändert sich mittendrin: weiter vom aktuellen Stand
        let mut st = TimelineState { pps: 50.0, scroll: 0.0, ..Default::default() };
        st.scroll_to(100.0, 0.0);
        st.step_view(theme::MOTION as f64 / 2.0);
        let half = st.scroll;
        assert!(half > 50.0 && half < 100.0 && st.pps == 50.0, "{half}");
        st.scroll_to(0.0, theme::MOTION as f64 / 2.0);
        assert_eq!(st.scroll, half, "neues Ziel beginnt am aktuellen Stand");
        assert!(!st.step_view(10.0));
        assert_eq!((st.scroll, st.view_glide), (0.0, None));
    }

    #[test]
    fn tiny_clips_merge_into_bars() {
        let r = |a: f32, b: f32| Rect::from_min_max(pos2(a, 0.0), pos2(b, 10.0));
        let (red, blue) = (Color32::RED, Color32::BLUE);
        let mut bar = None;
        assert_eq!(merge_bar(&mut bar, r(10.0, 11.5), red), None);
        assert_eq!(merge_bar(&mut bar, r(12.0, 13.5), red), None, "berührt: verschmolzen");
        assert_eq!(bar, Some((r(10.0, 13.5), red)));
        assert_eq!(merge_bar(&mut bar, r(20.0, 21.5), red), Some((r(10.0, 13.5), red)), "Lücke: alter Balken fertig");
        assert_eq!(merge_bar(&mut bar, r(21.0, 22.5), blue), Some((r(20.0, 21.5), red)), "andere Farbe");
        assert_eq!(bar, Some((r(21.0, 22.5), blue)));
    }

    #[test]
    fn volume_line_grabs_only_close_to_line() {
        // flacher Ton-Clip (Tonspur): Mitte gehört dem Verschieben, nicht der Linie bei 0 dB
        let body = Rect::from_min_size(pos2(0.0, 100.0), vec2(200.0, 26.0));
        assert!(!vol_line_hit(body, 0.0, body.center().y));
        assert!(vol_line_hit(body, 0.0, vol_y(body, 0.0) + 1.0));
        // hoher Clip: bis 4 px, nicht weiter
        let tall = Rect::from_min_size(pos2(0.0, 100.0), vec2(200.0, 60.0));
        assert!(vol_line_hit(tall, 0.0, vol_y(tall, 0.0) + 4.0));
        assert!(!vol_line_hit(tall, 0.0, vol_y(tall, 0.0) + 5.0));
    }

    #[test]
    fn clip_head_keeps_duration_and_elides_name() {
        let w = |s: &str| s.chars().count() as f32 * 6.0;
        assert_eq!(clip_head("Sunset.webm", " ✧ 2", "2.0s", 500.0, w), "Sunset.webm ✧ 2  2.0s");
        // eng: Kennzeichen weg, Name gekürzt, Dauer bleibt
        let h = clip_head("Sunset Libreville.webm", " ✧ 2", "2.0s", 90.0, w);
        assert!(h.ends_with("…  2.0s") && w(&h) <= 90.0, "{h}");
        // Effekt-Clip ohne Dauer: nur gekürzt mit „…“
        let h = clip_head("✧  Filmkorn", "", "", 40.0, w);
        assert!(h.ends_with('…') && w(&h) <= 40.0, "{h}");
    }

    /// Breitenmodell wie egui (Vorschub je Zeichen, Unterschneidung, Rundung auf Pixel), als
    /// Vorbild für `Measured`; ungerade Werte, damit Rundung und Unterschneidung zählen.
    fn model_adv(c: char) -> f32 {
        match c {
            ' ' => 2.6,
            '…' => 7.3,
            'i' | 'l' | '.' => 2.4,
            'm' | 'w' => 8.7,
            c if c.is_uppercase() => 6.9,
            _ => 5.45,
        }
    }
    fn model_kern(a: char, b: char) -> f32 {
        if (a, b) == ('a', 'v') || (a, b) == ('V', 'a') {
            -0.8
        } else {
            0.0
        }
    }
    fn model_width(s: &str, ppp: f32) -> f32 {
        let (mut x, mut w, mut prev) = (0.0, 0.0, None);
        for c in s.chars() {
            if let Some(p) = prev {
                x += model_kern(p, c);
            }
            w = x + model_adv(c);
            x = round_px(w, ppp);
            prev = Some(c);
        }
        w
    }

    #[test]
    fn measured_text_cuts_like_elide_and_clip_head() {
        for ppp in [1.0f32, 1.25, 2.0] {
            for (name, tags, dur) in [("Java vavoom.mp4", " ✧ 1.50x", "4,3 s"), ("a b  c", "", "1 s"), ("Text", "", ""), ("", " x", "2 s")] {
                let sep = if dur.is_empty() { "" } else { "  " };
                let mk = |t: &str| Measured::new(t, ppp, model_adv, model_kern);
                let (full, tail, m) = (mk(&format!("{name}{tags}{sep}{dur}")), mk(&format!("{sep}{dur}")), mk(name));
                let width = |s: &str| model_width(s, ppp);
                for w in (0..300).map(|w| w as f32 * 0.25) {
                    assert_eq!(clip_head_measured(&full, name.chars().count(), &tail, w), clip_head(name, tags, dur, w, width), "{name} {w} {ppp}");
                    assert_eq!(m.cut(m.chars.len(), w), theme::elide(name, w, width), "{name} {w} {ppp}");
                }
            }
        }
    }

    /// „Fehlt“ kommt aus der Hintergrundprüfung, nicht aus einem Dateizugriff im Oberflächen-Thread
    /// (Netzlaufwerke würden die Oberfläche anhalten).
    #[test]
    fn missing_media_comes_from_the_background_check() {
        let m = MediaRef {
            id: MediaId::new(),
            path: std::path::PathBuf::from("gibt/es/nicht.mp4"),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(1.0),
            width: 64,
            height: 36,
            fps: None,
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
        };
        let mut p = crate::loading::Presence::default();
        assert!(missing_media(std::slice::from_ref(&m), &p).is_empty(), "noch nicht geprüft: gilt als vorhanden");
        p.set_all(HashMap::from([(m.path.clone(), false)]), 1);
        assert_eq!(missing_media(std::slice::from_ref(&m), &p), HashSet::from([m.id]));
    }

    #[test]
    fn view_cache_follows_project_state_and_stays_bounded() {
        let key = |epoch| CacheKey { epoch, lang: cl_i18n::Lang::De, ppp: 1.0 };
        let mut c = ViewCache::default();
        let head = |_: &mut Kerning| Head {
            full: Measured::new("a", 1.0, |_| 5.0, |_, _| 0.0),
            name_n: 1,
            tail: Measured::new("", 1.0, |_| 5.0, |_, _| 0.0),
            body: None,
            shown: None,
            body_shown: None,
        };
        let one = Track::new(TrackKind::Video);
        // erster Stand: Spuren übernommen
        assert!(c.sync(key(1), || vec![one.clone()]));
        c.head(ClipId::new(), head);
        // gleicher Stand: nichts neu (der Spuren-Klon wird gar nicht erst gebaut)
        assert!(!c.sync(key(1), || unreachable!()));
        assert_eq!(c.heads.len(), 1);
        // neuer Stand, andere Sprache oder Skalierung: alles verworfen
        for k in [key(2), CacheKey { lang: cl_i18n::Lang::En, ..key(2) }, CacheKey { ppp: 2.0, ..key(2) }] {
            c.head(ClipId::new(), head);
            assert!(c.sync(k, Vec::new));
            assert!(c.heads.is_empty() && c.tracks.is_empty());
        }
        // Obergrenze: beim Überlauf neu beginnen
        for _ in 0..HEAD_CAP + 5 {
            c.head(ClipId::new(), head);
        }
        assert!(c.heads.len() <= HEAD_CAP && c.heads.len() >= 5);
    }

    #[test]
    fn only_visible_clips_are_built() {
        let view = Rect::from_min_max(pos2(172.0, 50.0), pos2(1000.0, 400.0));
        let clip = |x0: f32, x1: f32, y: f32| Rect::from_min_max(pos2(x0, y), pos2(x1, y + 30.0));
        assert!(clip_visible(clip(100.0, 200.0, 100.0), view), "links angeschnitten");
        assert!(clip_visible(clip(990.0, 1200.0, 100.0), view), "rechts angeschnitten");
        assert!(!clip_visible(clip(10.0, 171.0, 100.0), view), "links außerhalb");
        assert!(!clip_visible(clip(1001.0, 1100.0, 100.0), view), "rechts außerhalb");
        assert!(!clip_visible(clip(300.0, 400.0, 0.0), view), "Spur über dem Bildlauf");
        assert!(!clip_visible(clip(300.0, 400.0, 401.0), view), "Spur darunter");
        assert!(clip_visible(clip(300.0, 400.0, 380.0), view), "halb sichtbare Spur");
        assert!(!clip_visible(clip(300.0, 300.5, 100.0), view), "schmaler als ein Pixel");
        // Texte: Kopfzeile ab der sichtbaren Kante (+ Platz für das Warnzeichen)
        let (x, room, body) = clip_rooms(clip(100.0, 400.0, 100.0), view, true);
        assert_eq!((x, room, body), (172.0 + 6.0 + 14.0, 396.0 - 192.0, 396.0 - 108.0));
    }

    #[test]
    fn fades_never_overlap() {
        assert_eq!(fade_max(4.0, 1.5), 2.5);
        assert_eq!(fade_max(1.0, 3.0), 0.0);
        let mut c = Clip::new(
            ClipContent::Text(TextStyle::default()),
            Time::ZERO,
            Time::from_secs_f64(4.0),
        );
        set_audio_handle(&mut c, AudioHandle::FadeOut, 3.0);
        set_audio_handle(&mut c, AudioHandle::FadeIn, 3.0);
        assert!((c.fade_in.as_secs_f64() - 1.0).abs() < 1e-6);
        assert!(c.fade_in + c.fade_out <= c.duration);
    }

    /// Messung der Zeitleiste ohne Fenster (von Hand):
    /// `cargo test --release -p cl-app -- --ignored timeline_frame_time --nocapture --test-threads=1`
    mod bench {
        use super::*;
        use std::hash::{Hash, Hasher};


        /// Projekt wie `qa/PL_schnitt.cutline` (gleiche Spuren und Anteile), auf `n` Clips skaliert.
        pub(crate) fn synth_project(n: usize, file: &std::path::Path) -> Project {
            let mut p = Project::new("Messung");
            let media = |kind, audio, secs: f64| MediaRef {
                id: MediaId::new(),
                path: file.to_path_buf(),
                kind,
                duration: Time::from_secs_f64(secs),
                width: 1920,
                height: 1080,
                fps: None,
                has_audio: audio,
                audio_streams: audio as u32,
                video_codec: None,
                audio_codec: None,
            };
            let v = media(MediaKind::Video, false, 1400.0);
            let va = media(MediaKind::Video, true, 1400.0);
            let a = media(MediaKind::Audio, true, 1400.0);
            let img = media(MediaKind::Image, false, 0.0);
            let sfx = media(MediaKind::Audio, true, 2.0);
            let total = 644.0;
            // (Spurart, Clips in PL_schnitt, Inhalt, Clipdauer-Anteil)
            let plan: [(TrackKind, usize, u8, f64); 9] = [
                (TrackKind::Video, 150, 0, 1.0),
                (TrackKind::Audio, 150, 1, 1.0),
                (TrackKind::Video, 23, 2, 0.5),
                (TrackKind::Sticker, 39, 3, 0.3),
                (TrackKind::Text, 15, 4, 0.2),
                (TrackKind::Text, 249, 5, 0.9),
                (TrackKind::Effect, 6, 6, 0.3),
                (TrackKind::Audio, 1, 7, 1.0),
                (TrackKind::Audio, 24, 8, 0.1),
            ];
            for (k, (kind, count, what, frac)) in plan.into_iter().enumerate() {
                let count = ((count * n) as f64 / 657.0).round().max(1.0) as usize;
                let mut t = Track::new(kind);
                t.main = k == 0;
                let slot = total / count as f64;
                for i in 0..count {
                    let start = Time::from_secs_f64(i as f64 * slot);
                    let dur = Time::from_secs_f64(slot * frac);
                    let content = match what {
                        0 => ClipContent::Media { media: v.id, kind: MediaKind::Video },
                        1 => ClipContent::Media { media: va.id, kind: MediaKind::Audio },
                        2 => ClipContent::Media { media: va.id, kind: MediaKind::Video },
                        3 => ClipContent::Media { media: img.id, kind: MediaKind::Image },
                        4 => ClipContent::Text(TextStyle { text: format!("Teil {}", i + 1), ..TextStyle::default() }),
                        5 => ClipContent::Text(TextStyle { text: "dabei vom schiefgehen Heute Ende was".into(), ..TextStyle::default() }),
                        6 => ClipContent::Effect(cl_core::FxRef::new("cutline.effekte/super8")),
                        7 => ClipContent::Media { media: a.id, kind: MediaKind::Audio },
                        _ => ClipContent::Media { media: sfx.id, kind: MediaKind::Audio },
                    };
                    let mut c = Clip::new(content, start, dur);
                    c.source_in = Time::from_secs_f64((i as f64 * 7.3) % 600.0);
                    t.clips.push(c);
                }
                p.timeline.tracks.push(t);
            }
            p.media = vec![v, va, a, img, sfx];
            p
        }

        pub(crate) fn bench_app(ctx: &egui::Context, home: &std::path::Path) -> App {
            std::env::set_var("CUTLINE_HOME", home);
            std::env::set_var("CUTLINE_NO_UPDATE", "1");
            std::env::set_var("CUTLINE_NO_ONBOARDING", "1");
            App::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None)
        }

        /// Projekt setzen; Vorschaubilder und Wellenformen liegen schon vor (kein Decoder-Auftrag).
        pub(crate) fn load(app: &mut App, ctx: &egui::Context, p: Project) {
            for m in &p.media {
                if m.kind != MediaKind::Audio {
                    let img = egui::ColorImage::new([64, 36], Color32::from_gray(120));
                    app.thumbs.insert(m.id, ctx.load_texture("bench", img, Default::default()));
                }
                if m.has_audio {
                    let n = (m.duration.as_secs_f64() * WAVE_PER_SEC as f64) as usize;
                    app.waves.insert((m.id, 0), (0..n.max(1)).map(|i| ((i as f32 * 0.37).sin() * 0.5 + 0.5).powi(2)).collect());
                }
            }
            app.set_doc(p);
        }

        thread_local!(pub(crate) static TESS: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) });
        pub(crate) const SCREEN: egui::Vec2 = egui::vec2(1600.0, 1000.0);
        pub(crate) const PANEL_H: f32 = 470.0;

        /// Ein Bild: Zeitleiste in der unteren Leiste, Zeiger über den Clips. Gibt (ms, Formen, Prüfsumme).
        pub(crate) fn frame(app: &mut App, ctx: &egui::Context, t: f64, pointer: Pos2) -> (f64, usize, u64) {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
                time: Some(t),
                events: vec![egui::Event::PointerMoved(pointer)],
                ..Default::default()
            };
            let t0 = std::time::Instant::now();
            let out = ctx.run(input, |ctx| {
                egui::TopBottomPanel::bottom("tl").exact_height(PANEL_H).show(ctx, |ui| app.timeline_panel(ui));
            });
            let n = out.shapes.len();
            let run_ms = t0.elapsed().as_secs_f64() * 1000.0;
            TESS.with(|t| t.set(run_ms));
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let nf = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(d) = std::env::var_os("CUTLINE_BENCH_DUMP").filter(|_| nf == 31) {
                let txt: Vec<String> = out.shapes.iter().map(|c| {
                    let extra = match &c.shape { egui::Shape::Text(t) => format!("{:?} @{:?}", t.galley.text(), t.pos), s => format!("{:?}", s.visual_bounding_rect()) };
                    format!("{:?} {}", c.clip_rect, extra)
                }).collect();
                let _ = std::fs::write(d, txt.join("
"));
            }
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            (ms, n, fingerprint(&prims))
        }

        /// Prüfsumme des fertigen Bildes: alle Dreiecke in Zeichenreihenfolge mit Ausschnitt, Ecken,
        /// Farben und Textur – unabhängig davon, wie der Tessellierer sie zu Netzen bündelt.
        pub(crate) fn fingerprint(prims: &[egui::ClippedPrimitive]) -> u64 {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            let q = |v: f32| (v * 4.0).round() as i32;
            for p in prims {
                let r = p.clip_rect;
                if let egui::epaint::Primitive::Mesh(m) = &p.primitive {
                    // Schrift-Atlas: Lage der Zeichen darin hängt von der Reihenfolge des ersten Setzens ab
                    let font = m.texture_id == egui::TextureId::default();
                    let tex = format!("{:?}", m.texture_id);
                    for i in &m.indices {
                        let v = m.vertices[*i as usize];
                        [q(r.min.x), q(r.min.y), q(r.max.x), q(r.max.y)].hash(&mut h);
                        tex.hash(&mut h);
                        [q(v.pos.x), q(v.pos.y)].hash(&mut h);
                        if !font {
                            [q(v.uv.x * 64.0), q(v.uv.y * 64.0)].hash(&mut h);
                        }
                        v.color.to_array().hash(&mut h);
                    }
                }
            }
            h.finish()
        }

        /// Gemessene Kopfzeilen und Clip-Texte gleichen dem Setzen je Versuch (`clip_head`,
        /// `theme::elide`) mit den echten Schriften der App.
        #[test]
        #[ignore]
        fn measured_labels_match_layout() {
            let home = tempfile::tempdir().unwrap();
            let ctx = egui::Context::default();
            let _app = bench_app(&ctx, home.path());
            let _ = ctx.run(Default::default(), |_| {});
            let font = egui::FontId::proportional(theme::FS_HINT);
            let tags = format!(" {} 1.50x {}", icon::GAUGE, icon::SPARKLE);
            let mut bad = Vec::new();
            for (name, tags, dur) in [
                ("videoplayback.mp4", tags.as_str(), "4,3 s"),
                ("Sunset Libreville.webm", "", "12,0 s"),
                ("dabei vom schiefgehen Heute Ende was", "", ""),
                ("a b  c", " x", "1 s"),
            ] {
                let sep = if dur.is_empty() { "" } else { "  " };
                let mut kern = Kerning::default();
                let (full, tail, m) = ctx.fonts(|f| {
                    (kern.measured(f, &font, &format!("{name}{tags}{sep}{dur}")), kern.measured(f, &font, &format!("{sep}{dur}")), kern.measured(f, &font, name))
                });
                for w in (0..500).map(|w| w as f32 * 0.5) {
                    let a = clip_head(name, tags, dur, w, |s| theme::text_width(&ctx, s, font.clone()));
                    let b = clip_head_measured(&full, name.chars().count(), &tail, w);
                    if a != b {
                        bad.push(format!("{w}: {a:?} != {b:?}"));
                    }
                    let c = theme::elide(name, w, |s| theme::text_width(&ctx, s, font.clone()));
                    if c != m.cut(m.chars.len(), w) {
                        bad.push(format!("elide {w}: {c:?} != {:?}", m.cut(m.chars.len(), w)));
                    }
                }
            }
            assert!(bad.is_empty(), "{} Abweichungen:\n{}", bad.len(), bad[..bad.len().min(20)].join("\n"));
        }

        /// (Median, Max, 10-%-Wert) – der 10-%-Wert ist auf belastetem Rechner am verlässlichsten
        pub(crate) fn stats(mut v: Vec<f64>) -> (f64, f64, f64) {
            v.sort_by(f64::total_cmp);
            (v[v.len() / 2], v[v.len() - 1], v[v.len() / 10])
        }

        /// Bildzeit (Rechnen + Tessellieren) bei voll sichtbarer Zeitleiste: Stand und Abspielen.
        #[test]
        #[ignore]
        fn timeline_frame_time() {
            let home = tempfile::tempdir().unwrap();
            let file = std::env::current_exe().unwrap();
            let ctx = egui::Context::default();
            let mut app = bench_app(&ctx, home.path());
            for n in [657usize, 50] {
                load(&mut app, &ctx, synth_project(n, &file));
                let clips: usize = app.project.timeline.tracks.iter().map(|t| t.clips.len()).sum();
                app.timeline.pps = (SCREEN.x - HEADER_W - 16.0) / 644.0;
                app.timeline.scroll = 0.0;
                app.timeline.vscroll = 0.0;
                let pointer = pos2(700.0, SCREEN.y - PANEL_H + 80.0);
                let mut t = 0.0;
                for playing in [false, true] {
                    app.playing = playing;
                    app.playhead = Time::from_secs_f64(100.0);
                    let mut times = Vec::new();
                    let mut runs = Vec::new();
                    let (mut shapes, mut sum) = (0, 0);
                    for f in 0..150 {
                        t += 1.0 / 60.0;
                        if playing {
                            app.playhead = app.playhead + Time::from_secs_f64(1.0 / 30.0);
                        }
                        let (ms, s, fp) = frame(&mut app, &ctx, t, pointer);
                        if f >= 30 {
                            times.push(ms);
                            runs.push(TESS.with(|t| t.get()));
                        }
                        if f == 30 {
                            (shapes, sum) = (s, fp);
                        }
                    }
                    let (med, max, p10) = stats(times);
                    let (run, ..) = stats(runs);
                    println!(
                        "Zeitleiste {clips} Clips, {}: Median {med:.2} ms (davon Rechnen {run:.2} ms), 10 % {p10:.2} ms, Max {max:.2} ms, {shapes} Formen, Prüfsumme {sum:016x}",
                        if playing { "Abspielen" } else { "Stand" }
                    );
                }
                // Bewegung (nur 657): 300 Clips gleiten hin und her, weicher Zoom, Abspielen gezoomt mit Blättern
                if n != 657 {
                    continue;
                }
                app.playing = false;
                let base = app.project.clone();
                let mut times = Vec::new();
                for round in 0..12 {
                    let mut p2 = base.clone();
                    let shift = if round % 2 == 0 { 0.5 } else { 0.0 };
                    let mut k = 0;
                    for tr in &mut p2.timeline.tracks {
                        for c in &mut tr.clips {
                            if k < MAX_GLIDES {
                                c.start = c.start + Time::from_secs_f64(shift);
                                k += 1;
                            }
                        }
                    }
                    app.set_doc(p2);
                    for f in 0..12 {
                        t += 1.0 / 60.0;
                        let (ms, ..) = frame(&mut app, &ctx, t, pointer);
                        if f == 0 && round > 0 {
                            assert_eq!(app.timeline.motion.glides.len(), MAX_GLIDES);
                        }
                        if round > 0 {
                            times.push(ms);
                        }
                    }
                }
                let (med, max, p10) = stats(times);
                println!("Zeitleiste {clips} Clips, 300 Clips gleiten: Median {med:.2} ms, 10 % {p10:.2} ms, Max {max:.2} ms");
                let full = app.timeline.pps;
                let mut times = Vec::new();
                for f in 0..120 {
                    t += 1.0 / 60.0;
                    if f % 12 == 0 {
                        let to = if f % 24 == 0 { full * 3.0 } else { full };
                        app.timeline.zoom_to(to, 600.0, t);
                    }
                    let (ms, ..) = frame(&mut app, &ctx, t, pointer);
                    times.push(ms);
                }
                let (med, max, p10) = stats(times);
                println!("Zeitleiste {clips} Clips, weicher Zoom (ganze Breite ↔ 3×): Median {med:.2} ms, 10 % {p10:.2} ms, Max {max:.2} ms");
                app.timeline.pps = 60.0;
                app.timeline.view_glide = None;
                app.playing = true;
                app.playhead = Time::from_secs_f64(100.0);
                let mut times = Vec::new();
                for _ in 0..600 {
                    t += 1.0 / 60.0;
                    app.playhead = app.playhead + Time::from_secs_f64(1.0 / 30.0);
                    let (ms, ..) = frame(&mut app, &ctx, t, pointer);
                    times.push(ms);
                }
                let (med, max, p10) = stats(times);
                println!("Zeitleiste {clips} Clips, Abspielen gezoomt (60 px/s, Playhead folgt): Median {med:.2} ms, 10 % {p10:.2} ms, Max {max:.2} ms");
                app.playing = false;
            }
            app.playing = false;
            // echtes Projekt (z. B. `qa/PL_schnitt.cutline`), falls angegeben: CUTLINE_BENCH_PROJECT=Datei
            let Some(path) = std::env::var_os("CUTLINE_BENCH_PROJECT").filter(|p| std::path::Path::new(p).is_file()) else {
                return;
            };
            let proj = Project::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
            let dur = proj.duration().as_secs_f64();
            load(&mut app, &ctx, proj);
            let clips: usize = app.project.timeline.tracks.iter().map(|t| t.clips.len()).sum();
            app.timeline.pps = (SCREEN.x - HEADER_W - 16.0) / dur as f32;
            app.timeline.scroll = 0.0;
            let pointer = pos2(700.0, SCREEN.y - PANEL_H + 80.0);
            for playing in [false, true] {
                app.playing = playing;
                app.playhead = Time::from_secs_f64(100.0);
                let mut times = Vec::new();
                for f in 0..150 {
                    if playing {
                        app.playhead = app.playhead + Time::from_secs_f64(1.0 / 30.0);
                    }
                    let (ms, ..) = frame(&mut app, &ctx, 100.0 + f as f64 / 60.0, pointer);
                    if f >= 30 {
                        times.push(ms);
                    }
                }
                let (med, max, p10) = stats(times);
                println!(
                    "Projektdatei {clips} Clips, ganze Breite, {}: Median {med:.2} ms, 10 % {p10:.2} ms, Max {max:.2} ms",
                    if playing { "Abspielen" } else { "Stand" }
                );
            }
            app.playing = false;
        }
    }
}
