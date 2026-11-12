//! Clip-Eigenschaften für Phase 2: Effekte, Filter, Anpassungen, Maske,
//! Keyframes, Animationen und Übergänge. Die Auswertung (`Clip::eval_at`) ist die
//! gemeinsame Wahrheit für Vorschau und Export.

use crate::time::Time;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn one() -> f32 {
    1.0
}

/// Verweis auf einen Katalog-Eintrag (Effekt oder Filter) mit Parametern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FxRef {
    pub id: String,
    #[serde(default = "one")]
    pub intensity: f32,
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
}

impl FxRef {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), intensity: 1.0, params: BTreeMap::new() }
    }
}

/// Grundlegende Farb- und Bildanpassungen (alle 0 = unverändert, Bereich etwa −1…1).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjust {
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub sharpen: f32,
    pub vignette: f32,
    pub grain: f32,
    pub fade: f32,
    /// Farbton-Drehung in Grad
    pub hue: f32,
    /// HSL je Farbbereich (Rot, Gelb, Grün, Cyan, Blau, Magenta): Farbton, Sättigung, Helligkeit (−1…1)
    pub hsl: [[f32; 3]; 6],
}

pub const HSL_COLORS: [(&str, &str, [u8; 3]); 6] = [
    ("r", "Rot", [230, 70, 70]),
    ("y", "Gelb", [230, 200, 60]),
    ("g", "Grün", [80, 200, 90]),
    ("c", "Cyan", [70, 200, 210]),
    ("b", "Blau", [70, 110, 230]),
    ("m", "Magenta", [210, 80, 200]),
];

impl Adjust {
    pub fn is_identity(&self) -> bool {
        *self == Adjust::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskShape {
    Linear,
    Mirror,
    Circle,
    Rect,
}

/// Maske relativ zum Clip: Mittelpunkt und Größe als Anteil der Clipgröße.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    pub shape: MaskShape,
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    pub rotation_deg: f32,
    /// Weiche Kante als Anteil der Clipgröße
    pub feather: f32,
    pub invert: bool,
}

impl Mask {
    pub fn new(shape: MaskShape) -> Self {
        Self { shape, cx: 0.5, cy: 0.5, w: 0.6, h: 0.6, rotation_deg: 0.0, feather: 0.05, invert: false }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipFx {
    pub effects: Vec<FxRef>,
    pub filter: Option<FxRef>,
    pub adjust: Adjust,
    pub mask: Option<Mask>,
    /// LUT-Datei (.cube)
    pub lut: Option<String>,
    pub flip_h: bool,
    pub flip_v: bool,
    pub reverse: bool,
    /// Geschwindigkeitskurve (verteilt die Zeit um, die Cliplänge bleibt)
    pub speed_curve: Option<String>,
    /// Farbe freistellen (Greenscreen)
    pub chroma: Option<ChromaKey>,
    /// Verwackeln ausgleichen (0 = aus, 1 = stark)
    pub stabilize: f32,
    /// Hintergrund entfernen: Pfad des Maskenvideos (Graustufen, gleiche Zeitbasis wie die Quelle)
    pub cutout: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChromaKey {
    pub color: [u8; 3],
    /// Toleranz 0…1
    pub similarity: f32,
    /// weiche Kante 0…1
    pub blend: f32,
    /// Farbsaum entfernen 0…1
    pub spill: f32,
}

impl Default for ChromaKey {
    fn default() -> Self {
        Self { color: [0, 177, 64], similarity: 0.18, blend: 0.08, spill: 0.5 }
    }
}

/// Ton-Bearbeitung eines Clips.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioFx {
    /// Rauschen entfernen 0…1
    pub denoise: f32,
    /// Stimme hervorheben (Hintergrund absenken)
    pub voice_isolate: bool,
    /// Lautheit angleichen (−14 LUFS, Social-Media-Standard)
    pub normalize: bool,
    /// Stimmfilter (id aus `cl_fx::audio::VOICES`)
    pub voice: Option<String>,
    /// Bass / Höhen in dB (−12…12)
    pub bass: f32,
    pub treble: f32,
}

impl AudioFx {
    pub fn is_empty(&self) -> bool {
        *self == AudioFx::default()
    }
}

impl ClipFx {
    pub fn is_empty(&self) -> bool {
        *self == ClipFx::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Prop {
    X,
    Y,
    Scale,
    Rotation,
    Opacity,
    Volume,
}

impl Prop {
    pub const ALL: [Prop; 6] = [Prop::X, Prop::Y, Prop::Scale, Prop::Rotation, Prop::Opacity, Prop::Volume];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Ease {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Hold,
}

impl Ease {
    pub fn apply(self, p: f32) -> f32 {
        let p = p.clamp(0.0, 1.0);
        match self {
            Ease::Linear => p,
            Ease::EaseIn => p * p,
            Ease::EaseOut => 1.0 - (1.0 - p) * (1.0 - p),
            Ease::EaseInOut => p * p * (3.0 - 2.0 * p),
            Ease::Hold => 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Zeit relativ zum Clipanfang (Zeitleisten-Zeit)
    pub t: Time,
    pub v: f32,
    #[serde(default)]
    pub ease: Ease,
}

/// Keyframes je Eigenschaft (sortiert nach Zeit).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Keyframes(pub BTreeMap<Prop, Vec<Keyframe>>);

impl Keyframes {
    pub fn is_empty(&self) -> bool {
        self.0.values().all(|v| v.is_empty())
    }
    pub fn get(&self, p: Prop) -> &[Keyframe] {
        self.0.get(&p).map(|v| v.as_slice()).unwrap_or(&[])
    }
    /// Keyframe bei `t` setzen (ersetzt einen vorhandenen innerhalb von 1 ms).
    pub fn set(&mut self, p: Prop, t: Time, v: f32) {
        let list = self.0.entry(p).or_default();
        if let Some(k) = list.iter_mut().find(|k| (k.t.0 - t.0).abs() < 1000) {
            k.v = v;
        } else {
            list.push(Keyframe { t, v, ease: Ease::EaseInOut });
            list.sort_by_key(|k| k.t);
        }
    }
    pub fn remove(&mut self, p: Prop, t: Time) -> bool {
        let Some(list) = self.0.get_mut(&p) else { return false };
        let before = list.len();
        list.retain(|k| (k.t.0 - t.0).abs() >= 1000);
        let removed = list.len() != before;
        if list.is_empty() {
            self.0.remove(&p);
        }
        removed
    }
    pub fn has_at(&self, p: Prop, t: Time) -> bool {
        self.get(p).iter().any(|k| (k.t.0 - t.0).abs() < 1000)
    }
    /// Interpolierter Wert oder `None`, wenn die Eigenschaft keine Keyframes hat.
    pub fn eval(&self, p: Prop, t: Time) -> Option<f32> {
        let list = self.get(p);
        let first = list.first()?;
        if t <= first.t {
            return Some(first.v);
        }
        for w in list.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t <= b.t {
                let span = (b.t.0 - a.t.0).max(1) as f32;
                let q = a.ease.apply((t.0 - a.t.0) as f32 / span);
                return Some(a.v + (b.v - a.v) * q);
            }
        }
        list.last().map(|k| k.v)
    }
    /// Alle Keyframes um `delta` nach vorn verschieben (beim Trimmen am Anfang).
    pub fn shift(&mut self, delta: Time) {
        for list in self.0.values_mut() {
            for k in list.iter_mut() {
                k.t = Time(k.t.0 - delta.0);
            }
        }
    }
    /// Alle Keyframe-Zeiten (für die Anzeige in der Zeitleiste).
    pub fn times(&self) -> Vec<Time> {
        let mut v: Vec<Time> = self.0.values().flat_map(|l| l.iter().map(|k| k.t)).collect();
        v.sort();
        v.dedup_by(|a, b| (a.0 - b.0).abs() < 1000);
        v
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimRef {
    pub id: String,
    pub duration: Time,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Animations {
    pub intro: Option<AnimRef>,
    pub outro: Option<AnimRef>,
    /// Schleife über die ganze Cliplänge (Dauer = Periode)
    pub combo: Option<AnimRef>,
}

impl Animations {
    pub fn is_empty(&self) -> bool {
        self.intro.is_none() && self.outro.is_none() && self.combo.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionRef {
    pub id: String,
    pub duration: Time,
}

/// Ausgewertete Darstellung eines Clips zu einem Zeitpunkt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Evaluated {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub rotation_deg: f32,
    pub opacity: f32,
    pub volume_db: f32,
}

/// Wirkung einer Animation bei Fortschritt p (0 = Beginn, 1 = Ruhezustand).
/// Verschiebungen als Anteil der Projektbreite/-höhe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimDelta {
    pub dx: f32,
    pub dy: f32,
    pub scale_mul: f32,
    pub rot_add: f32,
    pub opacity_mul: f32,
}

impl AnimDelta {
    pub const NONE: AnimDelta = AnimDelta { dx: 0.0, dy: 0.0, scale_mul: 1.0, rot_add: 0.0, opacity_mul: 1.0 };
}

/// Eingebaute Ein-/Ausblend-Animationen (id, Name).
pub const ANIMATIONS: &[(&str, &str)] = &[
    ("fade", "Einblenden"),
    ("slide-left", "Von links"),
    ("slide-right", "Von rechts"),
    ("slide-up", "Von unten"),
    ("slide-down", "Von oben"),
    ("zoom-in", "Heranzoomen"),
    ("zoom-out", "Herauszoomen"),
    ("pop", "Aufploppen"),
    ("spin", "Drehen"),
    ("rise", "Aufsteigen"),
    ("drop", "Fallen"),
    ("swing", "Schwingen"),
];

/// Kombi-Animationen über die ganze Dauer (id, Name).
pub const COMBO_ANIMATIONS: &[(&str, &str)] = &[("pulse", "Pulsieren"), ("wobble", "Wackeln"), ("float", "Schweben"), ("spin-loop", "Rotieren")];

pub fn anim_delta(id: &str, p: f32) -> AnimDelta {
    let p = p.clamp(0.0, 1.0);
    let e = Ease::EaseOut.apply(p);
    let q = 1.0 - e;
    let mut d = AnimDelta::NONE;
    match id {
        "fade" => d.opacity_mul = p,
        "slide-left" => d.dx = -q,
        "slide-right" => d.dx = q,
        "slide-up" => d.dy = q,
        "slide-down" => d.dy = -q,
        "zoom-in" => {
            d.scale_mul = 0.3 + 0.7 * e;
            d.opacity_mul = p;
        }
        "zoom-out" => {
            d.scale_mul = 1.0 + 0.8 * q;
            d.opacity_mul = p;
        }
        "pop" => {
            let s = if p < 0.7 { p / 0.7 * 1.15 } else { 1.15 - (p - 0.7) / 0.3 * 0.15 };
            d.scale_mul = s.max(0.01);
        }
        "spin" => {
            d.rot_add = -180.0 * q;
            d.scale_mul = 0.5 + 0.5 * e;
            d.opacity_mul = p;
        }
        "rise" => {
            d.dy = 0.15 * q;
            d.opacity_mul = p;
        }
        "drop" => {
            d.dy = -0.15 * q;
            d.opacity_mul = p;
        }
        "swing" => {
            d.rot_add = 25.0 * q * (p * 12.0).cos();
            d.opacity_mul = p;
        }
        _ => {}
    }
    d
}

/// Kombi-Animation (t in Sekunden ab Clipbeginn, period = Dauer einer Schleife).
pub fn combo_delta(id: &str, t: f32, period: f32) -> AnimDelta {
    let w = std::f32::consts::TAU * t / period.max(0.05);
    let mut d = AnimDelta::NONE;
    match id {
        "pulse" => d.scale_mul = 1.0 + 0.06 * w.sin(),
        "wobble" => d.rot_add = 6.0 * w.sin(),
        "float" => d.dy = 0.02 * w.sin(),
        "spin-loop" => d.rot_add = 360.0 * (t / period.max(0.05)).fract(),
        _ => {}
    }
    d
}

/// Geschwindigkeitskurven: Stützpunkte (Position 0…1 in der Clipzeit, relative Geschwindigkeit).
pub const SPEED_CURVES: &[(&str, &str, &[(f32, f32)])] = &[
    ("montage", "Montage", &[(0.0, 1.0), (0.3, 4.0), (0.5, 0.4), (0.7, 4.0), (1.0, 1.0)]),
    ("hero", "Held", &[(0.0, 2.5), (0.35, 2.5), (0.5, 0.25), (0.65, 2.5), (1.0, 2.5)]),
    ("bullet", "Kugel", &[(0.0, 3.0), (0.4, 0.2), (0.6, 0.2), (1.0, 3.0)]),
    ("jump-cut", "Sprung", &[(0.0, 1.0), (0.45, 1.0), (0.5, 6.0), (0.55, 1.0), (1.0, 1.0)]),
    ("flash-in", "Blitz rein", &[(0.0, 5.0), (0.4, 1.0), (1.0, 1.0)]),
    ("flash-out", "Blitz raus", &[(0.0, 1.0), (0.6, 1.0), (1.0, 5.0)]),
    ("ease-in", "Langsam rein", &[(0.0, 0.3), (1.0, 2.0)]),
    ("ease-out", "Langsam raus", &[(0.0, 2.0), (1.0, 0.3)]),
];

fn curve_points(id: &str) -> Option<&'static [(f32, f32)]> {
    SPEED_CURVES.iter().find(|c| c.0 == id).map(|c| c.2)
}

fn curve_speed(pts: &[(f32, f32)], u: f32) -> f32 {
    for w in pts.windows(2) {
        if u <= w[1].0 {
            let q = (u - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
            let q = Ease::EaseInOut.apply(q);
            return w[0].1 + (w[1].1 - w[0].1) * q;
        }
    }
    pts.last().map(|p| p.1).unwrap_or(1.0)
}

/// Anteil des Quellbereichs, der bei Clip-Fortschritt `u` (0…1) erreicht ist.
pub fn curve_progress(id: &str, u: f32) -> f32 {
    let Some(pts) = curve_points(id) else { return u.clamp(0.0, 1.0) };
    const N: usize = 64;
    let u = u.clamp(0.0, 1.0);
    let mut total = 0.0;
    let mut upto = 0.0;
    for i in 0..N {
        let a = i as f32 / N as f32;
        let v = curve_speed(pts, a + 0.5 / N as f32) / N as f32;
        total += v;
        if a + 1.0 / N as f32 <= u {
            upto += v;
        } else if a < u {
            upto += v * (u - a) * N as f32;
        }
    }
    (upto / total.max(1e-6)).clamp(0.0, 1.0)
}

/// Relative Geschwindigkeit der Kurve bei `u` (für die Anzeige).
pub fn curve_speed_at(id: &str, u: f32) -> f32 {
    curve_points(id).map(|p| curve_speed(p, u)).unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframes_interpolate_and_edit() {
        let mut k = Keyframes::default();
        assert_eq!(k.eval(Prop::Scale, Time(0)), None);
        k.set(Prop::Scale, Time::from_secs_f64(0.0), 1.0);
        k.set(Prop::Scale, Time::from_secs_f64(2.0), 2.0);
        assert_eq!(k.eval(Prop::Scale, Time::from_secs_f64(-1.0)), Some(1.0));
        assert_eq!(k.eval(Prop::Scale, Time::from_secs_f64(5.0)), Some(2.0));
        let mid = k.eval(Prop::Scale, Time::from_secs_f64(1.0)).unwrap();
        assert!((mid - 1.5).abs() < 1e-4);
        k.set(Prop::Scale, Time::from_secs_f64(2.0), 3.0);
        assert_eq!(k.get(Prop::Scale).len(), 2);
        assert!(k.has_at(Prop::Scale, Time::from_secs_f64(2.0)));
        assert!(k.remove(Prop::Scale, Time::from_secs_f64(2.0)));
        assert_eq!(k.times().len(), 1);
    }

    #[test]
    fn animations_reach_rest_state() {
        for (id, _) in ANIMATIONS {
            assert_eq!(anim_delta(id, 1.0).opacity_mul, 1.0, "{id}");
            let d = anim_delta(id, 1.0);
            assert!(d.dx.abs() < 1e-4 && d.dy.abs() < 1e-4, "{id}");
            assert!((d.scale_mul - 1.0).abs() < 1e-3, "{id}");
        }
        assert_eq!(anim_delta("fade", 0.0).opacity_mul, 0.0);
    }

    #[test]
    fn speed_curves_are_monotone_and_complete() {
        for (id, ..) in SPEED_CURVES {
            assert_eq!(curve_progress(id, 0.0), 0.0);
            assert!((curve_progress(id, 1.0) - 1.0).abs() < 1e-4, "{id}");
            let mut last = 0.0;
            for i in 1..=100 {
                let v = curve_progress(id, i as f32 / 100.0);
                assert!(v >= last - 1e-6, "{id}");
                last = v;
            }
        }
    }
}
