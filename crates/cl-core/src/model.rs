use crate::fx::*;
use crate::time::{Rational, Time};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub uuid::Uuid);
        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4())
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl std::str::FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(s.parse()?))
            }
        }
    };
}
id_type!(ClipId);

impl ClipId {
    /// Derived, stable ID (e.g. for a second preview version of the same clip).
    pub fn derived(&self, salt: u128) -> Self {
        Self(uuid::Uuid::from_u128(self.0.as_u128() ^ salt))
    }
}
id_type!(TrackId);
id_type!(MediaId);

/// Raise this whenever a field is added whose loss would change the user's edit: older versions
/// then refuse the file instead of loading it and saving it back without that field.
pub const PROJECT_SCHEMA_VERSION: u32 = 1;

/// Load error for a project saved by a newer Cutline (the UI translates it as a whole).
pub const NEWER_PROJECT: &str = "Dieses Projekt wurde mit einer neueren Cutline-Version gespeichert. Bitte Cutline aktualisieren.";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub schema_version: u32,
    pub name: String,
    pub settings: ProjectSettings,
    pub media: Vec<MediaRef>,
    pub timeline: Timeline,
    /// Fonts that apply only to this project (copied along when archiving).
    pub project_fonts: Vec<PathBuf>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: name.into(),
            settings: ProjectSettings::default(),
            media: Vec::new(),
            timeline: Timeline::default(),
            project_fonts: Vec::new(),
        }
    }

    pub fn media(&self, id: MediaId) -> Option<&MediaRef> {
        self.media.iter().find(|m| m.id == id)
    }

    pub fn find_clip(&self, id: ClipId) -> Option<(&Track, &Clip)> {
        self.timeline
            .tracks
            .iter()
            .find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t, c)))
    }

    pub fn duration(&self) -> Time {
        self.timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(|c| c.end())
            .max()
            .unwrap_or(Time::ZERO)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("Projekt ist immer serialisierbar")
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        #[derive(Deserialize)]
        struct Version {
            schema_version: u32,
        }
        let newer = || <serde_json::Error as serde::de::Error>::custom(NEWER_PROJECT);
        // a newer file may also fail to parse here: then the version is the better message
        let mut p: Self = serde_json::from_str(s).map_err(|e| match serde_json::from_str::<Version>(s) {
            Ok(v) if v.schema_version > PROJECT_SCHEMA_VERSION => newer(),
            _ => e,
        })?;
        if p.schema_version > PROJECT_SCHEMA_VERSION {
            return Err(newer());
        }
        for t in &mut p.timeline.tracks {
            t.clips.retain_mut(Clip::sanitize_times);
        }
        // bring broken values (hand edits, foreign tools) to valid ones; valid ones stay
        if !p.settings.valid() {
            let s = &mut p.settings;
            s.width = s.width.clamp(16, 16384);
            s.height = s.height.clamp(16, 16384);
            if !(s.fps.as_f64() > 0.0 && s.fps.as_f64() <= 240.0) {
                s.fps = Rational::new(30, 1);
            }
        }
        Ok(p)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorSpace {
    Rec709Sdr,
    Rec2020Sdr,
    Rec2100Hlg,
    Rec2100Pq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AspectPreset {
    /// Takes the format of the first clip on the main track.
    Original,
    Ratio(u32, u32),
    Custom,
}

impl AspectPreset {
    pub const PRESETS: [(&'static str, AspectPreset); 10] = [
        ("Original", AspectPreset::Original),
        ("16:9", AspectPreset::Ratio(16, 9)),
        ("9:16", AspectPreset::Ratio(9, 16)),
        ("1:1", AspectPreset::Ratio(1, 1)),
        ("4:3", AspectPreset::Ratio(4, 3)),
        ("3:4", AspectPreset::Ratio(3, 4)),
        ("4:5", AspectPreset::Ratio(4, 5)),
        ("2.35:1", AspectPreset::Ratio(235, 100)),
        ("2:1", AspectPreset::Ratio(2, 1)),
        ("1.85:1", AspectPreset::Ratio(185, 100)),
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSettings {
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub aspect: AspectPreset,
    /// Not evaluated (choice removed from the UI); kept with `float_pipeline`
    /// so project files load and save unchanged.
    pub color_space: ColorSpace,
    pub float_pipeline: bool,
    /// Main track magnet: main track clips join without gaps.
    pub main_track_magnet: bool,
    pub snapping: bool,
    /// „Alles mitziehen“: ripple edits act on all unlocked tracks.
    #[serde(default = "yes")]
    pub ripple_all_tracks: bool,
    /// Chosen target platform (format and export preset), e.g. "shorts"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// Default for the audio channels of all clips (can be overridden per clip)
    #[serde(default)]
    pub channels: crate::fx::Channels,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: Rational::new(30, 1),
            aspect: AspectPreset::Original,
            color_space: ColorSpace::Rec709Sdr,
            float_pipeline: false,
            main_track_magnet: true,
            snapping: true,
            ripple_all_tracks: true,
            platform: None,
            channels: Default::default(),
        }
    }
}

impl ProjectSettings {
    /// Values preview and export can work with: size 16…16384, frame rate above 0 up to 240.
    pub fn valid(&self) -> bool {
        let f = self.fps.as_f64();
        (16..=16384).contains(&self.width) && (16..=16384).contains(&self.height) && f > 0.0 && f <= 240.0
    }

    /// Sets an aspect ratio and keeps the shorter edge (1080 stays 1080).
    pub fn apply_aspect(&mut self, aspect: AspectPreset) {
        self.aspect = aspect;
        if let AspectPreset::Ratio(w, h) = aspect {
            let short = self.width.min(self.height).max(2);
            let (nw, nh) = if w >= h {
                ((short as u64 * w as u64 / h as u64) as u32, short)
            } else {
                (short, (short as u64 * h as u64 / w as u64) as u32)
            };
            self.width = nw & !1;
            self.height = nh & !1;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaRef {
    pub id: MediaId,
    pub path: PathBuf,
    pub kind: MediaKind,
    pub duration: Time,
    pub width: u32,
    pub height: u32,
    pub fps: Option<Rational>,
    pub has_audio: bool,
    /// Number of audio streams in the file (0 = unknown, older projects; then treated as one)
    #[serde(default)]
    pub audio_streams: u32,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

impl MediaRef {
    /// Is the file local? Only then may it go to FFmpeg (preview and export) – a path
    /// like `http://…` from the project file would otherwise make FFmpeg go online. Otherwise it counts
    /// as missing (relink).
    pub fn is_local(&self) -> bool {
        self.path.is_file()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Video,
    Audio,
    Image,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    /// Index 0 is the bottom track. The first video track with `main = true` is the main track.
    pub tracks: Vec<Track>,
    /// Markers (notes, chapters), sorted by time
    #[serde(default)]
    pub markers: Vec<Marker>,
}

/// Marker on the timeline: point in time (optionally with duration), color and note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: u64,
    pub at: Time,
    #[serde(default)]
    pub duration: Time,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub note: String,
    /// Index into `MARKER_COLORS`
    #[serde(default)]
    pub color: u8,
}

/// Color palette for markers and clip labels (name, RGB)
pub const MARKER_COLORS: [(&str, [u8; 3]); 8] = [
    ("Blau", [70, 140, 255]),
    ("Grün", [70, 200, 120]),
    ("Gelb", [240, 200, 60]),
    ("Orange", [250, 140, 60]),
    ("Rot", [235, 80, 80]),
    ("Pink", [240, 110, 190]),
    ("Lila", [160, 110, 240]),
    ("Grau", [150, 150, 160]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackKind {
    Video,
    Audio,
    Text,
    Sticker,
    Effect,
    Filter,
}

impl TrackKind {
    /// May a clip of kind `clip` (see [`Clip::kind`]) go on a track of this kind? Image clips
    /// (media with picture, stickers) count as `Video`; sticker tracks (e.g. from plugins or
    /// generated projects) take them too.
    pub fn accepts(self, clip: TrackKind) -> bool {
        self == clip || (self == TrackKind::Sticker && clip == TrackKind::Video)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub main: bool,
    pub locked: bool,
    pub hidden: bool,
    pub muted: bool,
    pub clips: Vec<Clip>,
    /// Custom track name (otherwise kind + number)
    #[serde(default)]
    pub name: Option<String>,
    /// Collapsed in the timeline (narrow, stays visible)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsed: bool,
    /// Height factor in the timeline (1 = normal)
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub height: f32,
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

impl Track {
    pub fn new(kind: TrackKind) -> Self {
        Self {
            id: TrackId::new(),
            kind,
            main: false,
            locked: false,
            hidden: false,
            muted: false,
            clips: Vec::new(),
            name: None,
            collapsed: false,
            height: 1.0,
        }
    }
    pub fn end(&self) -> Time {
        self.clips
            .iter()
            .map(|c| c.end())
            .max()
            .unwrap_or(Time::ZERO)
    }
    pub(crate) fn sort(&mut self) {
        self.clips.sort_by_key(|c| c.start);
    }
    /// Free range without overlap?
    pub fn is_free(&self, start: Time, dur: Time, ignore: Option<ClipId>) -> bool {
        let end = start + dur;
        self.clips
            .iter()
            .filter(|c| Some(c.id) != ignore)
            .all(|c| end <= c.start || start >= c.end())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    /// Position on the timeline.
    pub start: Time,
    pub duration: Time,
    /// Start point within the source (for media clips).
    pub source_in: Time,
    pub content: ClipContent,
    pub transform: Transform,
    pub opacity: f32,
    pub volume_db: f32,
    /// Playback speed (1 = normal)
    #[serde(default = "one")]
    pub speed: f32,
    /// Disabled (V): stays in the timeline but is not shown/heard
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub fx: ClipFx,
    #[serde(default)]
    pub keyframes: Keyframes,
    #[serde(default)]
    pub anim: Animations,
    /// Transition to the next clip on the same track
    #[serde(default)]
    pub transition_out: Option<TransitionRef>,
    /// Audio fade in/out
    #[serde(default)]
    pub fade_in: Time,
    #[serde(default)]
    pub fade_out: Time,
    #[serde(default)]
    pub audio: AudioFx,
    /// Audio stream of the file (0 = first); `None` = first
    #[serde(default)]
    pub audio_stream: Option<u32>,
    /// Detected beats (source time), for marks and snapping
    #[serde(default)]
    pub beats: Vec<Time>,
    /// Color label (index into `MARKER_COLORS`)
    #[serde(default)]
    pub label: Option<u8>,
    /// Group: clips with the same id are selected and moved together
    #[serde(default)]
    pub group: Option<u64>,
    /// Crop of the source image (without keyframes); empty = field missing in the file
    #[serde(default, skip_serializing_if = "Crop::is_none")]
    pub crop: Crop,
}

/// Crop: edges as a fraction of source width/height (0…1). Applies before scaling, rotation
/// and position; the rest stays in its place in the image.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Crop {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

impl Crop {
    /// At least this much remains in each direction.
    pub const MIN_REST: f32 = 0.02;

    pub fn is_none(&self) -> bool {
        *self == Crop::default()
    }

    /// Clamp edges to 0…1; if the rest is too small, both sides are shortened proportionally.
    pub fn clamped(self) -> Self {
        fn pair(a: f32, b: f32) -> (f32, f32) {
            let (a, b) = (a.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
            let max = 1.0 - Crop::MIN_REST;
            if a + b <= max {
                (a, b)
            } else {
                (a * max / (a + b), b * max / (a + b))
            }
        }
        let (left, right) = pair(self.left, self.right);
        let (top, bottom) = pair(self.top, self.bottom);
        Self { left, right, top, bottom }
    }
}

fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}

impl Clip {
    /// Times from outside (files, MCP, plugins): nothing before 0. `false` if the clip cannot
    /// exist (duration not above 0). `Time::LIMIT` is only checked for new clips (`AddClip`):
    /// edits can push later clips past it, and loading must not change such a timeline.
    pub fn sanitize_times(&mut self) -> bool {
        self.start = Time(self.start.0.max(0));
        self.source_in = Time(self.source_in.0.max(0));
        self.duration > Time::ZERO
    }

    pub fn new(content: ClipContent, start: Time, duration: Time) -> Self {
        Self {
            id: ClipId::new(),
            start,
            duration,
            source_in: Time::ZERO,
            content,
            transform: Transform::default(),
            opacity: 1.0,
            volume_db: 0.0,
            speed: 1.0,
            disabled: false,
            fx: ClipFx::default(),
            keyframes: Keyframes::default(),
            anim: Animations::default(),
            transition_out: None,
            fade_in: Time::ZERO,
            fade_out: Time::ZERO,
            audio: AudioFx::default(),
            audio_stream: None,
            beats: Vec::new(),
            label: None,
            group: None,
            crop: Crop::default(),
        }
    }

    /// Beats in timeline time (only those in the visible range of the clip).
    pub fn beat_times(&self) -> Vec<Time> {
        let span = self.source_span();
        self.beats
            .iter()
            .filter(|b| **b >= self.source_in && **b <= self.source_in + span)
            .map(|b| {
                self.start
                    + Time(
                        ((b.0 - self.source_in.0) as f64 / self.speed.max(0.01) as f64).round()
                            as i64,
                    )
            })
            .collect()
    }
    pub fn end(&self) -> Time {
        self.start + self.duration
    }
    pub fn contains(&self, t: Time) -> bool {
        t >= self.start && t < self.end()
    }
    /// Change karaoke word times like keyframes (`f` per time). Call before changing the duration:
    /// evenly spread words are pinned with the previous duration.
    pub fn retime_words(&mut self, f: impl Fn(Time) -> Time) {
        let dur = self.duration;
        if let ClipContent::Text(TextStyle { text, karaoke: Some(k), .. }) = &mut self.content {
            k.words = k.starts(text, dur).into_iter().map(f).collect();
        }
    }
    /// Timeline time → source time (accounts for speed and reverse).
    pub fn source_time(&self, t: Time) -> Time {
        let mut rel = (t - self.start).0 as f64 * self.speed.max(0.01) as f64;
        if let Some(c) = &self.fx.speed_curve {
            let u = ((t - self.start).0 as f64 / self.duration.0.max(1) as f64) as f32;
            rel = curve_progress(c, u) as f64 * self.source_span().0 as f64;
        }
        if self.fx.reverse {
            Time(self.source_in.0 + self.source_span().0 - rel.round() as i64)
        } else {
            Time(self.source_in.0 + rel.round() as i64)
        }
    }
    /// Censor areas with the path in clip time (from clip start) instead of source time, as the
    /// filter chain needs them. `at`: only the position at this clip time (single frame).
    pub fn censor_in_clip_time(&self, at: Option<Time>) -> Vec<Censor> {
        let dur = self.duration;
        let mut ts = vec![Time::ZERO, dur];
        if let Some(a) = at {
            ts = vec![a];
        } else if self.fx.speed_curve.is_some() {
            // curve: mapping not linear, sample in frame steps
            let step = Time::from_secs_f64(1.0 / 30.0);
            ts.extend((1..dur.0 / step.0.max(1)).map(|i| Time(i * step.0)));
        }
        let sp = self.speed.max(0.01) as f64;
        self.fx
            .censor
            .iter()
            .map(|cs| {
                let mut ts = ts.clone();
                if at.is_none() && self.fx.speed_curve.is_none() {
                    ts.extend(
                        cs.path
                            .iter()
                            .map(|k| {
                                let rel = (k.t - self.source_in).0 as f64;
                                let rel = if self.fx.reverse { self.source_span().0 as f64 - rel } else { rel };
                                Time((rel / sp).round() as i64)
                            })
                            .filter(|t| *t > Time::ZERO && *t < dur),
                    );
                }
                ts.sort();
                ts.dedup();
                let path = ts
                    .into_iter()
                    .map(|t| CensorKey { t, ..cs.at(self.source_time(self.start + t)) })
                    .collect();
                Censor { path, ..cs.clone() }
            })
            .collect()
    }
    /// Length of the used source range.
    pub fn source_span(&self) -> Time {
        Time((self.duration.0 as f64 * self.speed.max(0.01) as f64).round() as i64)
    }
    /// Appearance at time `t_rel` (relative to the clip start): keyframes,
    /// then in/out/combo animation.
    pub fn eval_at(&self, t_rel: Time, canvas_w: f32, canvas_h: f32) -> Evaluated {
        let k = &self.keyframes;
        let mut e = Evaluated {
            x: k.eval(Prop::X, t_rel).unwrap_or(self.transform.x),
            y: k.eval(Prop::Y, t_rel).unwrap_or(self.transform.y),
            scale: k.eval(Prop::Scale, t_rel).unwrap_or(self.transform.scale),
            rotation_deg: k
                .eval(Prop::Rotation, t_rel)
                .unwrap_or(self.transform.rotation_deg),
            opacity: k.eval(Prop::Opacity, t_rel).unwrap_or(self.opacity),
            volume_db: k.eval(Prop::Volume, t_rel).unwrap_or(self.volume_db),
            crop: self.crop,
        };
        let mut apply = |d: AnimDelta| {
            e.x += d.dx * canvas_w;
            e.y += d.dy * canvas_h;
            e.scale *= d.scale_mul;
            e.rotation_deg += d.rot_add;
            e.opacity *= d.opacity_mul;
        };
        if let Some(a) = &self.anim.intro {
            if t_rel < a.duration && a.duration.0 > 0 {
                apply(anim_delta(&a.id, t_rel.0 as f32 / a.duration.0 as f32));
            }
        }
        if let Some(a) = &self.anim.outro {
            let left = self.duration - t_rel;
            if left < a.duration && a.duration.0 > 0 {
                apply(anim_delta(
                    &a.id,
                    left.0.max(0) as f32 / a.duration.0 as f32,
                ));
            }
        }
        if let Some(a) = &self.anim.combo {
            apply(combo_delta(
                &a.id,
                t_rel.as_secs_f64() as f32,
                a.duration.as_secs_f64() as f32,
            ));
        }
        e.opacity = e.opacity.clamp(0.0, 1.0);
        e
    }
    /// Strength of an effect/filter element at time `t_rel` (keyframes or fixed value).
    pub fn strength_at(&self, t_rel: Time) -> f32 {
        let base = match &self.content {
            ClipContent::Effect(r) | ClipContent::Filter(r) => r.intensity,
            _ => 1.0,
        };
        self.keyframes
            .eval(Prop::Strength, t_rel)
            .unwrap_or(base)
            .clamp(0.0, 1.0)
    }
    pub fn kind(&self) -> TrackKind {
        match &self.content {
            ClipContent::Media {
                kind: MediaKind::Audio,
                ..
            } => TrackKind::Audio,
            ClipContent::Media { .. } => TrackKind::Video,
            ClipContent::Text(_) => TrackKind::Text,
            ClipContent::Effect(_) => TrackKind::Effect,
            ClipContent::Filter(_) => TrackKind::Filter,
        }
    }
    pub fn label(&self, project: &Project) -> String {
        match &self.content {
            ClipContent::Media { media, .. } => project
                .media(*media)
                .and_then(|m| m.path.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Medium fehlt".into()),
            ClipContent::Text(t) => t.text.clone(),
            ClipContent::Effect(f) | ClipContent::Filter(f) => f.id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClipContent {
    Media {
        media: MediaId,
        kind: MediaKind,
    },
    Text(TextStyle),
    /// Effect as its own element on an effect track: acts on everything below
    Effect(FxRef),
    /// Filter as its own element on a filter track: acts on everything below
    Filter(FxRef),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub text: String,
    /// Family name; `None` = system default.
    pub font_family: Option<String>,
    pub size: f32,
    pub color: [u8; 4],
    pub bold: bool,
    pub italic: bool,
    pub uppercase: bool,
    /// Line wrap within the image width.
    pub auto_wrap: bool,
    /// Outline (width in px, color)
    #[serde(default)]
    pub outline: Option<(f32, [u8; 4])>,
    /// Background box behind the text
    #[serde(default)]
    pub background: Option<[u8; 4]>,
    #[serde(default = "yes")]
    pub shadow: bool,
    #[serde(default = "one")]
    pub line_spacing: f32,
    /// Alignment: -1 left, 0 center, 1 right
    #[serde(default)]
    pub align: i8,
    /// Karaoke: the word currently spoken in its own color
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub karaoke: Option<Karaoke>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Karaoke {
    /// Highlight color
    pub color: [u8; 4],
    /// „Füllen“: words already spoken keep the highlight
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fill: bool,
    /// Start of each word relative to the clip start (speech recognition); if the word count does not
    /// match the text, the words are spread evenly by character count
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<Time>,
}

/// Byte ranges of the words (split at whitespace).
pub fn word_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut v = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                v.push(s..i);
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    v
}

impl Karaoke {
    /// Start of the words of `text` at clip duration `dur`: the stored ones if the word count
    /// matches, otherwise evenly by character count (like `resegment` in speech recognition).
    pub fn starts(&self, text: &str, dur: Time) -> Vec<Time> {
        let words = text.split_whitespace().map(|w| w.chars().count() + 1).collect::<Vec<_>>();
        if self.words.len() == words.len() {
            return self.words.clone();
        }
        let total = words.iter().sum::<usize>().max(1) as f64;
        let mut acc = 0;
        words
            .iter()
            .map(|n| {
                let t = Time((dur.0 as f64 * acc as f64 / total).round() as i64);
                acc += n;
                t
            })
            .collect()
    }
}

/// Word highlighted at time `t` (relative to the clip start): the last one started, also in
/// speech pauses until the next; none before the first word.
pub fn karaoke_word(starts: &[Time], t: Time) -> Option<usize> {
    starts.partition_point(|s| *s <= t).checked_sub(1)
}

impl TextStyle {
    /// Highlighted words (indices) at time `t` of a clip of duration `dur`.
    pub fn karaoke_lit(&self, dur: Time, t: Time) -> Option<std::ops::Range<usize>> {
        let k = self.karaoke.as_ref()?;
        let i = karaoke_word(&k.starts(&self.text, dur), t)?;
        Some(if k.fill { 0..i + 1 } else { i..i + 1 })
    }
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            text: "Standardtext".into(),
            font_family: None,
            size: 64.0,
            color: [255, 255, 255, 255],
            bold: false,
            italic: false,
            uppercase: false,
            auto_wrap: true,
            outline: None,
            background: None,
            shadow: true,
            line_spacing: 1.0,
            align: 0,
            karaoke: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    /// Position relative to the image center in project pixels.
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub rotation_deg: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            rotation_deg: 0.0,
        }
    }
}
