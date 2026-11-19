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
    /// Abgeleitete, stabile ID (z. B. für eine zweite Vorschau-Fassung desselben Clips).
    pub fn derived(&self, salt: u128) -> Self {
        Self(uuid::Uuid::from_u128(self.0.as_u128() ^ salt))
    }
}
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

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("Projekt ist immer serialisierbar")
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        let mut p: Self = serde_json::from_str(s)?;
        // kaputte Werte (Hand-Bearbeitung, fremde Werkzeuge) auf gültige bringen; gültige bleiben
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
    /// Nicht ausgewertet (Auswahl aus der Oberfläche entfernt); bleiben mit `float_pipeline`,
    /// damit Projektdateien unverändert laden und speichern.
    pub color_space: ColorSpace,
    pub float_pipeline: bool,
    /// Hauptspur-Magnet: Clips der Hauptspur schließen lückenlos an.
    pub main_track_magnet: bool,
    pub snapping: bool,
    /// „Alles mitziehen“: Ripple-Schnitte wirken auf alle nicht gesperrten Spuren.
    #[serde(default = "yes")]
    pub ripple_all_tracks: bool,
    /// Gewählte Zielplattform (Format und Export-Vorlage), z. B. "shorts"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// Standard für die Tonkanäle aller Clips (einzeln überschreibbar)
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
    /// Werte, mit denen Vorschau und Export arbeiten können: Größe 16…16384, Bildrate über 0 bis 240.
    pub fn valid(&self) -> bool {
        let f = self.fps.as_f64();
        (16..=16384).contains(&self.width) && (16..=16384).contains(&self.height) && f > 0.0 && f <= 240.0
    }

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
    /// Zahl der Tonspuren der Datei (0 = unbekannt, ältere Projekte; dann wie eine)
    #[serde(default)]
    pub audio_streams: u32,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

impl MediaRef {
    /// Liegt die Datei lokal vor? Nur dann darf sie an FFmpeg (Vorschau wie Export) – ein Pfad
    /// wie `http://…` aus der Projektdatei ließe FFmpeg sonst ins Netz gehen. Sonst gilt sie
    /// als fehlend (neu verknüpfen).
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

impl TrackKind {
    /// Darf ein Clip der Art `clip` (siehe [`Clip::kind`]) auf eine Spur dieser Art? Bild-Clips
    /// (Medien mit Bild, Sticker) gelten als `Video`; Sticker-Spuren (z. B. aus Plugins oder
    /// erzeugten Projekten) nehmen sie ebenfalls auf.
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
    /// Eigener Spurname (sonst Art + Nummer)
    #[serde(default)]
    pub name: Option<String>,
    /// In der Zeitleiste eingeklappt (schmal, bleibt sichtbar)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsed: bool,
    /// Höhenfaktor in der Zeitleiste (1 = normal)
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
    /// Tonspur der Datei (0 = erste); `None` = erste
    #[serde(default)]
    pub audio_stream: Option<u32>,
    /// Erkannte Beats (Quellzeit), für Markierungen und Einrasten
    #[serde(default)]
    pub beats: Vec<Time>,
    /// Farbmarkierung (Index in `MARKER_COLORS`)
    #[serde(default)]
    pub label: Option<u8>,
    /// Gruppe: Clips mit gleicher Kennung werden gemeinsam ausgewählt und bewegt
    #[serde(default)]
    pub group: Option<u64>,
    /// Zuschnitt des Quellbilds (ohne Keyframes); leer = Feld fehlt in der Datei
    #[serde(default, skip_serializing_if = "Crop::is_none")]
    pub crop: Crop,
}

/// Zuschnitt: Ränder als Anteil der Quellbreite/-höhe (0…1). Wirkt vor Skalierung, Drehung
/// und Position; der Rest bleibt an seiner Stelle im Bild.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Crop {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

impl Crop {
    /// Mindestens so viel bleibt je Richtung übrig.
    pub const MIN_REST: f32 = 0.02;

    pub fn is_none(&self) -> bool {
        *self == Crop::default()
    }

    /// Ränder auf 0…1 begrenzen; reicht der Rest nicht, werden beide Seiten anteilig gekürzt.
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
    /// Karaoke-Wortzeiten wie Keyframes ändern (`f` je Zeit). Vor dem Ändern der Dauer aufrufen:
    /// gleichmäßig verteilte Wörter werden dabei mit der bisherigen Dauer festgehalten.
    pub fn retime_words(&mut self, f: impl Fn(Time) -> Time) {
        let dur = self.duration;
        if let ClipContent::Text(TextStyle { text, karaoke: Some(k), .. }) = &mut self.content {
            k.words = k.starts(text, dur).into_iter().map(f).collect();
        }
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
    /// Stärke eines Effekt-/Filter-Elements zum Zeitpunkt `t_rel` (Keyframes oder fester Wert).
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
    /// Effekt als eigenes Element auf einer Effektspur: wirkt auf alles darunter
    Effect(FxRef),
    /// Filter als eigenes Element auf einer Filterspur: wirkt auf alles darunter
    Filter(FxRef),
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
    /// Karaoke: das gerade gesprochene Wort in eigener Farbe
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub karaoke: Option<Karaoke>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Karaoke {
    /// Hervorhebungsfarbe
    pub color: [u8; 4],
    /// „Füllen“: bereits gesprochene Wörter behalten die Hervorhebung
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fill: bool,
    /// Beginn je Wort relativ zum Clipanfang (Spracherkennung); passt die Wortzahl nicht zum
    /// Text, werden die Wörter gleichmäßig nach Zeichenzahl verteilt
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<Time>,
}

/// Byte-Bereiche der Wörter (getrennt an Leerraum).
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
    /// Beginn der Wörter von `text` bei Clipdauer `dur`: die gespeicherten, wenn die Wortzahl
    /// passt, sonst gleichmäßig nach Zeichenzahl (wie `resegment` der Spracherkennung).
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

/// Zur Zeit `t` (relativ zum Clipanfang) hervorgehobenes Wort: das zuletzt begonnene, auch in
/// Sprechpausen bis zum nächsten; vor dem ersten Wort keins.
pub fn karaoke_word(starts: &[Time], t: Time) -> Option<usize> {
    starts.partition_point(|s| *s <= t).checked_sub(1)
}

impl TextStyle {
    /// Hervorgehobene Wörter (Indizes) zur Zeit `t` eines Clips der Dauer `dur`.
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
