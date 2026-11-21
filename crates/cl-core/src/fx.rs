//! Clip properties for phase 2: effects, filters, adjustments, mask,
//! keyframes, animations and transitions. The evaluation (`Clip::eval_at`) is the
//! shared truth for preview and export.

use crate::time::Time;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn one() -> f32 {
    1.0
}

/// Reference to a catalog entry (effect or filter) with parameters.
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
        Self {
            id: id.into(),
            intensity: 1.0,
            params: BTreeMap::new(),
        }
    }
}

/// Basic color and image adjustments (all 0 = unchanged, range about −1…1).
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
    /// Hue rotation in degrees
    pub hue: f32,
    /// HSL per color range (red, yellow, green, cyan, blue, magenta): hue, saturation, lightness (−1…1)
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

/// Mask relative to the clip: center and size as a fraction of the clip size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    pub shape: MaskShape,
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    pub rotation_deg: f32,
    /// Soft edge as a fraction of the clip size
    pub feather: f32,
    pub invert: bool,
}

impl Mask {
    pub fn new(shape: MaskShape) -> Self {
        Self {
            shape,
            cx: 0.5,
            cy: 0.5,
            w: 0.6,
            h: 0.6,
            rotation_deg: 0.0,
            feather: 0.05,
            invert: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipFx {
    pub effects: Vec<FxRef>,
    pub filter: Option<FxRef>,
    pub adjust: Adjust,
    pub mask: Option<Mask>,
    /// LUT file (.cube)
    pub lut: Option<String>,
    pub flip_h: bool,
    pub flip_v: bool,
    pub reverse: bool,
    /// Speed curve (redistributes time, the clip length stays)
    pub speed_curve: Option<String>,
    /// Chroma key (green screen)
    pub chroma: Option<ChromaKey>,
    /// Stabilization (0 = off, 1 = strong)
    pub stabilize: f32,
    /// Remove background: path of the mask video (grayscale, same time base as the source)
    pub cutout: Option<String>,
    /// What happens to the background when a person mask exists
    pub person: Option<PersonFx>,
    /// Denoise 0…1
    pub denoise: f32,
    /// Deflicker brightness (artificial light, time-lapse)
    pub deflicker: bool,
    /// Slow motion with computed in-between frames (optical flow) instead of doubled frames
    pub smooth_slowmo: bool,
    /// Censor areas (pixelate/blur); empty = field missing in the file
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub censor: Vec<Censor>,
}

/// Censor area: pixelates or blurs a rectangle or an ellipse. Position as a fraction
/// of the source image (before flip, crop and transform), path in source time – split, trim,
/// speed and reverse thus carry it along without conversion (like `Clip::beats`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Censor {
    /// Blur instead of pixelate
    pub blur: bool,
    pub ellipse: bool,
    /// 0 = minimum strength (already unrecognizable, see `CENSOR_MIN`) … 1 = very coarse
    pub strength: f32,
    /// Track the subject (otherwise the area stands still)
    pub track: bool,
    /// Keypoints sorted by time; one = the area stands still
    pub path: Vec<CensorKey>,
    /// runtime only, see `CensorId`
    #[serde(skip)]
    pub id: CensorId,
}

/// Id of a censor area that only lives at runtime (not saved, every new area
/// gets its own): the result of a running track finds its area through it,
/// even if the list has shifted meanwhile. Ignored in comparisons.
#[derive(Debug, Clone, Copy)]
pub struct CensorId(pub u64);

impl Default for CensorId {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

impl PartialEq for CensorId {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// Position of the censor area at a point in time (center, width, height as a fraction of the image).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct CensorKey {
    pub t: Time,
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
}

/// Block size or blur (sigma) of the censor as a fraction of the shorter image side at strength 0
/// and 1. At least 1/24 (45 px in 1080p): a face (~300 px) then has at most ~7 blocks
/// across or is blurred over 45 px – no longer recognizable.
pub const CENSOR_MIN: f32 = 1.0 / 24.0;
pub const CENSOR_MAX: f32 = 1.0 / 8.0;
/// The area is this much larger than the tracked box; after simplifying the path
/// at least `CENSOR_KEEP` margin remains.
pub const CENSOR_PAD: f32 = 1.15;
pub const CENSOR_KEEP: f32 = 1.05;

impl CensorKey {
    fn lerp(a: CensorKey, b: CensorKey, t: Time) -> CensorKey {
        let q = ((t.0 - a.t.0) as f64 / (b.t.0 - a.t.0).max(1) as f64).clamp(0.0, 1.0) as f32;
        let m = |x: f32, y: f32| x + (y - x) * q;
        CensorKey { t, cx: m(a.cx, b.cx), cy: m(a.cy, b.cy), w: m(a.w, b.w), h: m(a.h, b.h) }
    }
    /// Enlarged by `k` (center stays)
    pub fn grown(self, k: f32) -> CensorKey {
        CensorKey { w: self.w * k, h: self.h * k, ..self }
    }
    /// Does `inner` lie entirely in this area (as a rectangle)?
    pub fn covers(&self, inner: &CensorKey) -> bool {
        const EPS: f32 = 1e-5;
        self.cx - self.w / 2.0 <= inner.cx - inner.w / 2.0 + EPS
            && self.cx + self.w / 2.0 >= inner.cx + inner.w / 2.0 - EPS
            && self.cy - self.h / 2.0 <= inner.cy - inner.h / 2.0 + EPS
            && self.cy + self.h / 2.0 >= inner.cy + inner.h / 2.0 - EPS
    }
}

impl Censor {
    /// Position at time `t` (linear between keypoints, held before/after).
    pub fn at(&self, t: Time) -> CensorKey {
        let p = &self.path;
        let Some(first) = p.first() else {
            return CensorKey { t, cx: 0.5, cy: 0.5, w: 0.3, h: 0.3 };
        };
        if t <= first.t {
            return CensorKey { t, ..*first };
        }
        match p.windows(2).find(|w| t <= w[1].t) {
            Some(w) => CensorKey::lerp(w[0], w[1], t),
            None => CensorKey { t, ..p[p.len() - 1] },
        }
    }
}

/// Tracked boxes (sampled evenly, sorted by time) → path of the censor area: enlarged
/// by `CENSOR_PAD` and simplified. A keypoint is dropped as long as the line between
/// its neighbours still covers every box in between with `CENSOR_KEEP` margin; at most 2 s
/// between two keypoints. Since path and boxes are linear between sample times,
/// the coverage then holds for every frame in between too.
pub fn censor_simplify(boxes: &[CensorKey]) -> Vec<CensorKey> {
    let pad = |k: CensorKey| k.grown(CENSOR_PAD);
    let Some(first) = boxes.first() else { return Vec::new() };
    let fits = |i: usize, j: usize| {
        let (a, b) = (pad(boxes[i]), pad(boxes[j]));
        b.t.0 - a.t.0 <= 2_000_000
            && boxes[i + 1..j]
                .iter()
                .all(|m| CensorKey::lerp(a, b, m.t).covers(&m.grown(CENSOR_KEEP)))
    };
    let mut out = vec![pad(*first)];
    let mut i = 0;
    while i + 1 < boxes.len() {
        let mut j = i + 1;
        while j + 1 < boxes.len() && fits(i, j + 1) {
            j += 1;
        }
        out.push(pad(boxes[j]));
        i = j;
    }
    out
}

/// Person effects based on the mask from „Hintergrund entfernen“.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersonFx {
    /// Blurred background (portrait look), strength 0…1
    BlurBackground { amount: f32 },
    /// Replace the background with a color
    ColorBackground { color: [u8; 3] },
    /// Glowing outline around the person, background stays
    Outline {
        color: [u8; 3],
        width: f32,
        /// Transparent background: only outline and person (layer over another image)
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        transparent: bool,
    },
}

impl PersonFx {
    pub const KINDS: [&'static str; 3] = ["Hintergrund weich", "Hintergrundfarbe", "Umriss"];
    pub fn index(&self) -> usize {
        match self {
            PersonFx::BlurBackground { .. } => 0,
            PersonFx::ColorBackground { .. } => 1,
            PersonFx::Outline { .. } => 2,
        }
    }
    pub fn from_index(i: usize) -> Self {
        match i {
            1 => PersonFx::ColorBackground {
                color: [24, 120, 255],
            },
            2 => PersonFx::Outline {
                color: [255, 255, 255],
                width: 0.4,
                transparent: false,
            },
            _ => PersonFx::BlurBackground { amount: 0.6 },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChromaKey {
    pub color: [u8; 3],
    /// Tolerance 0…1
    pub similarity: f32,
    /// soft edge 0…1
    pub blend: f32,
    /// remove color spill 0…1
    pub spill: f32,
}

impl Default for ChromaKey {
    fn default() -> Self {
        Self {
            color: [0, 177, 64],
            similarity: 0.18,
            blend: 0.08,
            spill: 0.5,
        }
    }
}

/// Audio editing of a clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioFx {
    /// Denoise 0…1
    pub denoise: f32,
    /// Enhance voice (lower the background)
    pub voice_isolate: bool,
    /// Normalize loudness (−14 LUFS, social media standard)
    pub normalize: bool,
    /// Voice filter (id from `cl_fx::audio::VOICES`)
    pub voice: Option<String>,
    /// Bass / treble in dB (−12…12)
    pub bass: f32,
    pub treble: f32,
    /// Remove mains hum (50/60 Hz and harmonics)
    pub hum: bool,
    /// Remove rumble and wind (lows below ~90 Hz)
    pub rumble: bool,
    /// Remove clicks and crackles
    pub declick: bool,
    /// Compressor 0…1 (more even volume)
    pub compress: f32,
    /// Audio filter (id from `cl_fx::audio::FILTERS`)
    pub filter: Option<String>,
    /// Reverb amount 0…1 (0 = off)
    #[serde(skip_serializing_if = "is_zero")]
    pub reverb: f32,
    /// Reverb room size 0…1
    #[serde(skip_serializing_if = "is_half")]
    pub reverb_size: f32,
    /// Channels; `None` = project default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<Channels>,
}

/// Handling of a clip's audio channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Channels {
    /// If only one channel is used (e.g. microphone only left), spread it to both
    #[default]
    Auto,
    /// Unchanged
    Stereo,
    /// Mix both channels, same on both
    Mono,
    /// Use only the left channel (on both)
    Left,
    /// Use only the right channel (on both)
    Right,
}

impl Channels {
    pub const ALL: [(Channels, &'static str); 5] = [
        (Channels::Auto, "Automatisch"),
        (Channels::Stereo, "Stereo"),
        (Channels::Mono, "Mono"),
        (Channels::Left, "Nur links"),
        (Channels::Right, "Nur rechts"),
    ];
    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|c| c.0 == self).map(|c| c.1).unwrap_or("")
    }
    /// FFmpeg filter (after `aformat=…stereo`). `silent`: index of the silent channel (Auto only).
    pub fn filter(self, silent: Option<u8>) -> Option<&'static str> {
        let left = "pan=stereo|c0=c0|c1=c0";
        let right = "pan=stereo|c0=c1|c1=c1";
        match self {
            Channels::Stereo => None,
            Channels::Mono => Some("pan=stereo|c0=0.5*c0+0.5*c1|c1=0.5*c0+0.5*c1"),
            Channels::Left => Some(left),
            Channels::Right => Some(right),
            Channels::Auto => match silent {
                Some(1) => Some(left),
                Some(0) => Some(right),
                _ => None,
            },
        }
    }
}

impl Default for AudioFx {
    fn default() -> Self {
        Self {
            denoise: 0.0,
            voice_isolate: false,
            normalize: false,
            voice: None,
            bass: 0.0,
            treble: 0.0,
            hum: false,
            rumble: false,
            declick: false,
            compress: 0.0,
            filter: None,
            reverb: 0.0,
            reverb_size: 0.5,
            channels: None,
        }
    }
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_half(v: &f32) -> bool {
    *v == 0.5
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
    /// Strength of an effect/filter element (0…1)
    Strength,
}

impl Prop {
    pub const ALL: [Prop; 6] = [
        Prop::X,
        Prop::Y,
        Prop::Scale,
        Prop::Rotation,
        Prop::Opacity,
        Prop::Volume,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum Ease {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Hold,
    /// Custom curve like CSS `cubic-bezier(x1, y1, x2, y2)`; y may go beyond 0…1
    /// (anticipation, overshoot)
    Bezier(f32, f32, f32, f32),
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
            Ease::Bezier(x1, y1, x2, y2) => {
                let (x1, x2) = (x1.clamp(0.0, 1.0), x2.clamp(0.0, 1.0));
                let bez = |s: f32, a: f32, b: f32| {
                    let u = 1.0 - s;
                    3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
                };
                // x(s) grows monotonically: find the parameter for p by bisection
                let (mut lo, mut hi) = (0.0f32, 1.0f32);
                for _ in 0..24 {
                    let mid = (lo + hi) / 2.0;
                    if bez(mid, x1, x2) < p {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                bez((lo + hi) / 2.0, y1, y2)
            }
        }
    }
    /// Control points of the curve (for the curve editor); `None` for „Halten“
    pub fn bezier(self) -> Option<(f32, f32, f32, f32)> {
        match self {
            Ease::Linear => Some((0.0, 0.0, 1.0, 1.0)),
            Ease::EaseIn => Some((0.42, 0.0, 1.0, 1.0)),
            Ease::EaseOut => Some((0.0, 0.0, 0.58, 1.0)),
            Ease::EaseInOut => Some((0.42, 0.0, 0.58, 1.0)),
            Ease::Hold => None,
            Ease::Bezier(a, b, c, d) => Some((a, b, c, d)),
        }
    }
}

#[test]
fn bezier_ease_hits_ends_and_can_overshoot() {
    let lin = Ease::Bezier(0.0, 0.0, 1.0, 1.0);
    for p in [0.0, 0.25, 0.5, 1.0] {
        assert!((lin.apply(p) - p).abs() < 1e-3);
    }
    let back = Ease::Bezier(0.34, 1.56, 0.64, 1.0);
    assert!(back.apply(0.0).abs() < 1e-3 && (back.apply(1.0) - 1.0).abs() < 1e-3);
    assert!((0..=20).any(|i| back.apply(i as f32 / 20.0) > 1.02), "schwingt über");
    // read old projects without curve and new ones with curve
    assert_eq!(serde_json::from_str::<Ease>("\"EaseIn\"").unwrap(), Ease::EaseIn);
    let s = serde_json::to_string(&back).unwrap();
    assert_eq!(serde_json::from_str::<Ease>(&s).unwrap(), back);
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Time relative to the clip start (timeline time)
    pub t: Time,
    pub v: f32,
    #[serde(default)]
    pub ease: Ease,
}

/// Keyframes per property (sorted by time).
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
    /// Set a keyframe at `t` (replaces an existing one within 1 ms).
    pub fn set(&mut self, p: Prop, t: Time, v: f32) {
        let list = self.0.entry(p).or_default();
        if let Some(k) = list.iter_mut().find(|k| (k.t.0 - t.0).abs() < 1000) {
            k.v = v;
        } else {
            list.push(Keyframe {
                t,
                v,
                ease: Ease::EaseInOut,
            });
            list.sort_by_key(|k| k.t);
        }
    }
    pub fn remove(&mut self, p: Prop, t: Time) -> bool {
        let Some(list) = self.0.get_mut(&p) else {
            return false;
        };
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
    /// Interpolated value, or `None` if the property has no keyframes.
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
    /// Shift all keyframes forward by `delta` (when trimming the start).
    pub fn shift(&mut self, delta: Time) {
        for list in self.0.values_mut() {
            for k in list.iter_mut() {
                k.t = Time(k.t.0 - delta.0);
            }
        }
    }
    /// All keyframe times (for display in the timeline).
    pub fn times(&self) -> Vec<Time> {
        let mut v: Vec<Time> = self
            .0
            .values()
            .flat_map(|l| l.iter().map(|k| k.t))
            .collect();
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
    /// Loop over the whole clip length (duration = period)
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

/// Evaluated appearance of a clip at a point in time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Evaluated {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub rotation_deg: f32,
    pub opacity: f32,
    pub volume_db: f32,
    /// Crop of the clip (not animated)
    pub crop: crate::model::Crop,
}

/// Effect of an animation at progress p (0 = start, 1 = rest state).
/// Offsets as a fraction of the project width/height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimDelta {
    pub dx: f32,
    pub dy: f32,
    pub scale_mul: f32,
    pub rot_add: f32,
    pub opacity_mul: f32,
}

impl AnimDelta {
    pub const NONE: AnimDelta = AnimDelta {
        dx: 0.0,
        dy: 0.0,
        scale_mul: 1.0,
        rot_add: 0.0,
        opacity_mul: 1.0,
    };
}

/// Built-in in/out animations (id, name).
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
    ("bounce-in", "Hüpfen"),
    ("elastic", "Elastisch"),
    ("stamp", "Stempel"),
    ("punch", "Schlag"),
    ("snap-left", "Schnell von links"),
    ("snap-right", "Schnell von rechts"),
    ("snap-down", "Schnell von oben"),
    ("snap-up", "Schnell von unten"),
    ("unfold", "Aufklappen"),
    ("roll-in", "Hereinrollen"),
    ("whirl", "Wirbel"),
    ("tilt-in", "Einkippen"),
    ("wipe-fade", "Wischen"),
    ("flicker", "Neon-Start"),
    ("glitch-in", "Glitch"),
    ("shake-in", "Rütteln"),
];

/// Combo animations over the whole duration (id, name).
pub const COMBO_ANIMATIONS: &[(&str, &str)] = &[
    ("pulse", "Pulsieren"),
    ("wobble", "Wackeln"),
    ("float", "Schweben"),
    ("spin-loop", "Rotieren"),
    ("shake", "Zittern"),
    ("pendulum", "Pendeln"),
    ("heartbeat", "Herzschlag"),
    ("breathe", "Atmen"),
    ("bob", "Wippen"),
    ("blink", "Blinken"),
    ("rock", "Schaukeln"),
    ("ken-burns", "Langsamer Zoom"),
    ("glitch", "Glitch-Schleife"),
];

/// Anticipation and overshoot (ends exactly on 1 at p = 1), `s` = strength
fn back_out(p: f32, s: f32) -> f32 {
    let x = p - 1.0;
    1.0 + (s + 1.0) * x * x * x + s * x * x
}

/// Fixed pseudo-random number −1…1 per step (same in preview and export)
fn jitter(i: u32) -> f32 {
    ((i.wrapping_add(7).wrapping_mul(2_654_435_761) >> 16) & 0xffff) as f32 / 32767.5 - 1.0
}

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
            let s = if p < 0.7 {
                p / 0.7 * 1.15
            } else {
                1.15 - (p - 0.7) / 0.3 * 0.15
            };
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
        "bounce-in" => {
            // falls from above and bounces twice
            let r = 1.0 - p;
            d.dy = -0.3 * r * r * (p * std::f32::consts::PI * 3.5).cos().abs();
            d.opacity_mul = (p * 4.0).min(1.0);
        }
        "elastic" => {
            let r = 1.0 - p;
            d.scale_mul = (1.0 - r.powi(4) * (p * std::f32::consts::PI * 4.6).cos()).max(0.01);
            d.opacity_mul = (p * 5.0).min(1.0);
        }
        "stamp" => {
            // large → briefly too small (impact) → normal
            d.scale_mul = if p < 0.5 {
                2.2 - 1.28 * (p / 0.5).powi(2)
            } else {
                let u = (p - 0.5) / 0.5;
                1.0 - 0.08 * (1.0 - u) * (u * std::f32::consts::PI * 1.5).cos()
            };
            d.rot_add = -6.0 * q;
            d.opacity_mul = (p * 3.0).min(1.0);
        }
        "punch" => {
            // past the target, back, brief spring
            let r = 1.0 - p;
            d.scale_mul = (1.0 - 0.4 * r * r * (p * std::f32::consts::PI * 4.0).cos()).max(0.01);
            d.opacity_mul = (p * 6.0).min(1.0);
        }
        "snap-left" | "snap-right" | "snap-up" | "snap-down" => {
            let off = 1.2 * (1.0 - back_out(p, 1.3));
            match id {
                "snap-left" => d.dx = -off,
                "snap-right" => d.dx = off,
                "snap-up" => d.dy = off,
                _ => d.dy = -off,
            }
        }
        "unfold" => {
            // soft S-curve out of nothing, without overshoot
            let s = p * p * p * (p * (p * 6.0 - 15.0) + 10.0);
            d.scale_mul = 0.02 + 0.98 * s;
            d.opacity_mul = (p * 2.0).min(1.0);
        }
        "roll-in" => {
            let c = 1.0 - (1.0 - p).powi(3);
            d.dx = -0.5 * (1.0 - c);
            d.rot_add = -360.0 * (1.0 - c);
            d.opacity_mul = (p * 3.0).min(1.0);
        }
        "whirl" => {
            d.rot_add = 540.0 * q;
            d.scale_mul = 0.05 + 0.95 * e;
            d.opacity_mul = p;
        }
        "tilt-in" => {
            let r = 1.0 - back_out(p, 2.0);
            d.rot_add = 30.0 * r;
            d.dy = 0.05 * r;
            d.opacity_mul = (p * 3.0).min(1.0);
        }
        "wipe-fade" => {
            d.dx = -0.08 * q;
            d.opacity_mul = e;
        }
        "flicker" => {
            // neon tube flickers on: fixed brightness steps
            const STEPS: [f32; 10] = [0.0, 0.8, 0.1, 0.0, 1.0, 0.3, 1.0, 0.6, 1.0, 1.0];
            d.opacity_mul = STEPS[((p * 10.0) as usize).min(9)];
        }
        "glitch-in" => {
            let i = (p * 24.0) as u32;
            d.dx = 0.06 * q * jitter(i);
            d.dy = 0.02 * q * jitter(i + 101);
            d.opacity_mul = if jitter(i + 57) > 0.5 && p < 0.9 {
                0.3
            } else {
                (p * 3.0).min(1.0)
            };
        }
        "shake-in" => {
            d.dx = 0.05 * (1.0 - p) * (p * 60.0).sin();
            d.opacity_mul = (p * 3.0).min(1.0);
        }
        _ => {}
    }
    d
}

/// Combo animation (t in seconds from clip start, period = duration of one loop).
pub fn combo_delta(id: &str, t: f32, period: f32) -> AnimDelta {
    let w = std::f32::consts::TAU * t / period.max(0.05);
    let mut d = AnimDelta::NONE;
    match id {
        "pulse" => d.scale_mul = 1.0 + 0.06 * w.sin(),
        "wobble" => d.rot_add = 6.0 * w.sin(),
        "float" => d.dy = 0.02 * w.sin(),
        "spin-loop" => d.rot_add = 360.0 * (t / period.max(0.05)).fract(),
        "shake" => {
            d.dx = 0.006 * ((7.0 * w).sin() + 0.5 * (13.0 * w).sin());
            d.dy = 0.006 * (11.0 * w).sin();
        }
        "pendulum" => {
            d.rot_add = 15.0 * w.sin();
            d.dy = -0.01 * (1.0 - (2.0 * w).cos()) / 2.0;
        }
        "heartbeat" => {
            // two beats in quick succession, then a pause
            let u = (t / period.max(0.05)).fract();
            let bump = |a: f32, b: f32| {
                if (a..b).contains(&u) {
                    (std::f32::consts::PI * (u - a) / (b - a)).sin().powi(2)
                } else {
                    0.0
                }
            };
            d.scale_mul = 1.0 + 0.12 * bump(0.0, 0.15) + 0.08 * bump(0.2, 0.35);
        }
        "breathe" => {
            let k = (1.0 - w.cos()) / 2.0;
            d.scale_mul = 1.0 + 0.04 * k;
            d.opacity_mul = 1.0 - 0.15 * k;
        }
        "bob" => d.dy = -0.03 * (w / 2.0).sin().abs(),
        "blink" => {
            let u = (t / period.max(0.05)).fract();
            if (0.3..0.65).contains(&u) {
                d.opacity_mul = 0.1;
            }
        }
        "rock" => {
            d.rot_add = 5.0 * w.sin();
            d.dx = 0.012 * w.sin();
            d.dy = 0.008 * (2.0 * w).sin();
        }
        "ken-burns" => d.scale_mul = 1.0 + 0.08 * (1.0 - w.cos()) / 2.0,
        "glitch" => {
            // short outlier in the beat, otherwise calm
            let u = (t / period.max(0.05)).fract();
            if (0.2..0.4).contains(&u) {
                let i = (u * 40.0) as u32;
                d.dx = 0.02 * jitter(i);
                d.dy = 0.008 * jitter(i + 31);
            }
        }
        _ => {}
    }
    d
}

/// Speed curves: keypoints (position 0…1 in clip time, relative speed).
pub const SPEED_CURVES: &[(&str, &str, &[(f32, f32)])] = &[
    (
        "montage",
        "Montage",
        &[(0.0, 1.0), (0.3, 4.0), (0.5, 0.4), (0.7, 4.0), (1.0, 1.0)],
    ),
    (
        "hero",
        "Held",
        &[
            (0.0, 2.5),
            (0.35, 2.5),
            (0.5, 0.25),
            (0.65, 2.5),
            (1.0, 2.5),
        ],
    ),
    (
        "bullet",
        "Kugel",
        &[(0.0, 3.0), (0.4, 0.2), (0.6, 0.2), (1.0, 3.0)],
    ),
    (
        "jump-cut",
        "Sprung",
        &[(0.0, 1.0), (0.45, 1.0), (0.5, 6.0), (0.55, 1.0), (1.0, 1.0)],
    ),
    (
        "flash-in",
        "Blitz rein",
        &[(0.0, 5.0), (0.4, 1.0), (1.0, 1.0)],
    ),
    (
        "flash-out",
        "Blitz raus",
        &[(0.0, 1.0), (0.6, 1.0), (1.0, 5.0)],
    ),
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

/// Fraction of the source range reached at clip progress `u` (0…1).
pub fn curve_progress(id: &str, u: f32) -> f32 {
    let Some(pts) = curve_points(id) else {
        return u.clamp(0.0, 1.0);
    };
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

/// Relative speed of the curve at `u` (for display).
pub fn curve_speed_at(id: &str, u: f32) -> f32 {
    curve_points(id).map(|p| curve_speed(p, u)).unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reverb off writes no new field: the audio block is saved as before
    #[test]
    fn audio_without_reverb_is_saved_as_before() {
        let a = AudioFx::default();
        assert_eq!((a.reverb, a.reverb_size), (0.0, 0.5));
        let old = r#"{"denoise":0.3,"voice_isolate":true,"normalize":false,"voice":"robot","bass":2.0,"treble":0.0,"hum":true,"rumble":false,"declick":false,"compress":0.5,"filter":null}"#;
        let a: AudioFx = serde_json::from_str(old).unwrap();
        assert_eq!((a.reverb, a.reverb_size), (0.0, 0.5), "old file loads without reverb");
        assert_eq!(serde_json::to_string(&a).unwrap(), old, "round trip unchanged");
        let r = AudioFx { reverb: 0.4, reverb_size: 0.8, ..a };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"reverb\":0.4") && s.contains("\"reverb_size\":0.8"), "{s}");
        assert_eq!(serde_json::from_str::<AudioFx>(&s).unwrap(), r);
    }

    /// Outline without „durchsichtig“ does not write the field (project file as before 0.13)
    #[test]
    fn outline_without_transparent_is_saved_as_before() {
        use crate::model::*;
        let mut p = Project::new("Umriss");
        let mut t = Track::new(TrackKind::Video);
        let mut c = Clip::new(ClipContent::Text(Default::default()), Time::ZERO, Time::from_secs_f64(1.0));
        c.fx.person = Some(PersonFx::Outline { color: [255, 255, 255], width: 8.0, transparent: false });
        t.clips.push(c);
        p.timeline.tracks.push(t);
        let s = p.to_json();
        assert!(s.contains("\"outline\"") && !s.contains("transparent"), "{s}");
        assert_eq!(Project::from_json(&s).unwrap().to_json(), s);
        if let Some(PersonFx::Outline { transparent, .. }) = &mut p.timeline.tracks[0].clips[0].fx.person {
            *transparent = true;
        }
        assert!(p.to_json().contains("\"transparent\": true"));
    }

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

    fn rest_dist(d: AnimDelta) -> f32 {
        // rotation by full turns is the rest state (rotate)
        let rot = (d.rot_add + 180.0).rem_euclid(360.0) - 180.0;
        d.dx.abs() + d.dy.abs() + (d.scale_mul - 1.0).abs() + rot.abs() / 90.0 + (d.opacity_mul - 1.0).abs()
    }

    fn sane(d: AnimDelta) -> bool {
        [d.dx, d.dy, d.scale_mul, d.rot_add, d.opacity_mul].iter().all(|v| v.is_finite())
            && d.dx.abs() <= 2.0
            && d.dy.abs() <= 2.0
            && d.scale_mul > 0.0
            && d.scale_mul <= 4.0
            && (0.0..=1.0).contains(&d.opacity_mul)
    }

    #[test]
    fn intro_animations_are_distinct_bounded_and_end_without_jump() {
        let ids: std::collections::HashSet<_> = ANIMATIONS.iter().map(|a| a.0).collect();
        assert_eq!(ids.len(), ANIMATIONS.len(), "doppelte id");
        for (id, _) in ANIMATIONS {
            assert!(rest_dist(anim_delta(id, 0.3)) > 1e-3, "{id} tut nichts");
            assert!(rest_dist(anim_delta(id, 1.0)) < 1e-4, "{id} does not end at rest");
            assert!(rest_dist(anim_delta(id, 0.995)) < 0.05, "{id} springt am Ende");
            for i in 0..=200 {
                let d = anim_delta(id, i as f32 / 200.0);
                assert!(sane(d), "{id} at {i}: {d:?}");
            }
        }
        // existing animations keep their behaviour (saved projects)
        assert!((anim_delta("pop", 0.35).scale_mul - 0.575).abs() < 1e-4);
        assert!((anim_delta("slide-left", 0.5).dx + 0.25).abs() < 1e-6);
        assert!((anim_delta("spin", 0.0).rot_add + 180.0).abs() < 1e-4);
        assert_eq!(anim_delta("gibt-es-nicht", 0.3), AnimDelta::NONE);
    }

    #[test]
    fn combo_animations_loop_seamlessly() {
        let ids: std::collections::HashSet<_> = COMBO_ANIMATIONS.iter().map(|a| a.0).collect();
        assert_eq!(ids.len(), COMBO_ANIMATIONS.len(), "doppelte id");
        let period = 1.2;
        for (id, _) in COMBO_ANIMATIONS {
            assert!(rest_dist(combo_delta(id, 0.3 * period, period)) > 1e-3, "{id} tut nichts");
            for t in [0.0, period, 3.0 * period] {
                assert!(rest_dist(combo_delta(id, t, period)) < 1e-3, "{id} at {t}");
            }
            assert!(rest_dist(combo_delta(id, period * 0.999, period)) < 0.05, "{id} Schleife springt");
            for i in 0..=240 {
                let d = combo_delta(id, i as f32 / 100.0, period);
                assert!(sane(d), "{id} at {i}: {d:?}");
            }
        }
        assert!((combo_delta("pulse", 0.3, 1.2).scale_mul - 1.06).abs() < 1e-4);
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
