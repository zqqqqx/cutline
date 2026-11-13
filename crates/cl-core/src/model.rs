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
id_type!(TrackId);
id_type!(MediaId);

pub const PROJECT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub schema_version: u32,
    pub name: String,
    pub settings: ProjectSettings,
    pub media: Vec<MediaRef>,
    pub timeline: Timeline,
    /// Schriften, die nur in diesem Projekt gelten (werden beim Archivieren mitkopiert).
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

    /// Alle Clips, die zum Zeitpunkt `t` sichtbar sind, von unten nach oben.
    pub fn clips_at(&self, t: Time) -> Vec<(&Track, &Clip)> {
        self.timeline
            .tracks
            .iter()
            .filter(|tr| !tr.hidden)
            .flat_map(|tr| {
                tr.clips
                    .iter()
                    .filter(move |c| c.contains(t))
                    .map(move |c| (tr, c))
            })
            .collect()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("Projekt ist immer serialisierbar")
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
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
    /// Übernimmt das Format des ersten Clips der Hauptspur.
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
    pub color_space: ColorSpace,
    pub float_pipeline: bool,
    /// Hauptspur-Magnet: Clips der Hauptspur schließen lückenlos an.
    pub main_track_magnet: bool,
    pub snapping: bool,
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
        }
    }
}

impl ProjectSettings {
    /// Setzt ein Seitenverhältnis und hält die kürzere Kante (1080 bleibt 1080).
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
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Video,
    Audio,
    Image,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    /// Index 0 ist die unterste Spur. Die erste Video-Spur mit `main = true` ist die Hauptspur.
    pub tracks: Vec<Track>,
    /// Marker (Notizen, Kapitel), nach Zeit sortiert
    #[serde(default)]
    pub markers: Vec<Marker>,
}

/// Marker auf der Zeitleiste: Zeitpunkt (optional mit Dauer), Farbe und Notiz.
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
    /// Index in `MARKER_COLORS`
    #[serde(default)]
    pub color: u8,
}

/// Farbpalette für Marker und Clip-Labels (Name, RGB)
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub main: bool,
    pub locked: bool,
    pub hidden: bool,
    pub muted: bool,
    pub clips: Vec<Clip>,
    /// Eigener Spurname (sonst Art + Nummer)
    #[serde(default)]
    pub name: Option<String>,
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
    /// Freier Bereich ohne Überlappung?
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
    /// Position auf der Zeitleiste.
    pub start: Time,
    pub duration: Time,
    /// Startpunkt innerhalb der Quelle (bei Medien-Clips).
    pub source_in: Time,
    pub content: ClipContent,
    pub transform: Transform,
    pub opacity: f32,
    pub volume_db: f32,
    /// Abspielgeschwindigkeit (1 = normal)
    #[serde(default = "one")]
    pub speed: f32,
    /// Deaktiviert (V): bleibt in der Zeitleiste, wird aber nicht gezeigt/gehört
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub fx: ClipFx,
    #[serde(default)]
    pub keyframes: Keyframes,
    #[serde(default)]
    pub anim: Animations,
    /// Übergang zum nächsten Clip derselben Spur
    #[serde(default)]
    pub transition_out: Option<TransitionRef>,
    /// Ton ein-/ausblenden
    #[serde(default)]
    pub fade_in: Time,
    #[serde(default)]
    pub fade_out: Time,
    #[serde(default)]
    pub audio: AudioFx,
    /// Erkannte Beats (Quellzeit), für Markierungen und Einrasten
    #[serde(default)]
    pub beats: Vec<Time>,
    /// Farbmarkierung (Index in `MARKER_COLORS`)
    #[serde(default)]
    pub label: Option<u8>,
    /// Gruppe: Clips mit gleicher Kennung werden gemeinsam ausgewählt und bewegt
    #[serde(default)]
    pub group: Option<u64>,
}

fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}

impl Clip {
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
            beats: Vec::new(),
            label: None,
            group: None,
        }
    }

    /// Beats in Zeitleisten-Zeit (nur die im sichtbaren Bereich des Clips).
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
    /// Zeitleisten-Zeit → Quellzeit (berücksichtigt Geschwindigkeit und Rückwärts).
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
    /// Länge des genutzten Quellbereichs.
    pub fn source_span(&self) -> Time {
        Time((self.duration.0 as f64 * self.speed.max(0.01) as f64).round() as i64)
    }
    pub fn is_animated(&self) -> bool {
        !self.keyframes.is_empty() || !self.anim.is_empty()
    }
    /// Darstellung zum Zeitpunkt `t_rel` (relativ zum Clipanfang): Keyframes,
    /// dann Ein-/Aus-/Kombi-Animation.
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
    pub fn kind(&self) -> TrackKind {
        match &self.content {
            ClipContent::Media {
                kind: MediaKind::Audio,
                ..
            } => TrackKind::Audio,
            ClipContent::Media { .. } => TrackKind::Video,
            ClipContent::Text(_) => TrackKind::Text,
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClipContent {
    Media { media: MediaId, kind: MediaKind },
    Text(TextStyle),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub text: String,
    /// Familienname; `None` = Systemstandard.
    pub font_family: Option<String>,
    pub size: f32,
    pub color: [u8; 4],
    pub bold: bool,
    pub italic: bool,
    pub uppercase: bool,
    /// Zeilenumbruch innerhalb der Bildbreite.
    pub auto_wrap: bool,
    /// Kontur (Breite in px, Farbe)
    #[serde(default)]
    pub outline: Option<(f32, [u8; 4])>,
    /// Hintergrundfläche hinter dem Text
    #[serde(default)]
    pub background: Option<[u8; 4]>,
    #[serde(default = "yes")]
    pub shadow: bool,
    #[serde(default = "one")]
    pub line_spacing: f32,
    /// Ausrichtung: -1 links, 0 Mitte, 1 rechts
    #[serde(default)]
    pub align: i8,
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    /// Position relativ zur Bildmitte in Pixeln des Projekts.
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
