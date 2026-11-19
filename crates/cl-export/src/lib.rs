//! Export (Phase 1): Die Zeitleiste wird in einen FFmpeg-Filtergraphen übersetzt.
//!
//! - Video: schwarze Basis in Projektauflösung, darauf alle sichtbaren Video-/Bildclips
//!   (Spurreihenfolge, Transform, Deckkraft) und Textclips (drawtext mit importierter Schrift,
//!   Zeilenumbruch wie in der Vorschau, zentriert, Schatten).
//! - Audio: alle hörbaren Clips (Lautstärke, Stummschaltung der Spur) werden gemischt.
//! - Alle Hilfsdateien liegen in einem Arbeitsordner, damit im Graphen keine Windows-Pfade
//!   escaped werden müssen.
//!
//! Dieselbe Graph-Erzeugung liefert auch den Ton für die Vorschau-Wiedergabe.
//! Phase 2: Effekte, Filter, Masken, Keyframes, Animationen, Übergänge und Geschwindigkeit
//! kommen aus `graph.rs` (Sprite pro Clip, xfade-Ketten).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cl_core::*;

mod graph;
mod sections;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("Das Projekt ist leer")]
    Empty,
    /// Dateinamen der fehlenden Medien im Exportbereich
    #[error("{}", missing_text(.0))]
    MissingMedia(Vec<String>),
    #[error("FFmpeg: {0}")]
    Ffmpeg(String),
    #[error("Abgebrochen")]
    Cancelled,
    #[error("Lautheit konnte nicht gemessen werden")]
    Loudness,
    #[error("E/A-Fehler: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Media(#[from] cl_media::MediaError),
    /// Fehler in einem Abschnitt eines großen Exports (Zeiten der Zeitleiste; `args`: der
    /// gescheiterte FFmpeg-Aufruf, leer bei anderen Fehlern)
    #[error("Abschnitt {from}–{to}: {err}")]
    Section { from: String, to: String, err: Box<ExportError>, args: Vec<String> },
    /// So viele Clips gleichzeitig, dass schon ein einzelnes Bild FFmpeg überfordert
    #[error("Zu viele gleichzeitige Clips bei {0} – FFmpeg kann sie nicht auf einmal öffnen")]
    TooManyClips(String),
    #[error("Zu wenig Speicherplatz für die Zwischendateien: {need} nötig, {free} frei")]
    NoSpace { need: String, free: String },
}

impl ExportError {
    /// Argumente des gescheiterten FFmpeg-Laufs, wenn er nicht der Endlauf (`Job::args`) war.
    pub fn failed_args(&self) -> Option<&[String]> {
        match self {
            ExportError::Section { args, .. } => Some(args),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, ExportError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    Mp4,
    Mov,
    Mkv,
    WebM,
    Gif,
    /// Nur Ton
    Wav,
    Mp3,
    M4a,
    Flac,
}

impl Format {
    pub const ALL: [Format; 9] = [
        Format::Mp4,
        Format::Mov,
        Format::Mkv,
        Format::WebM,
        Format::Gif,
        Format::Wav,
        Format::Mp3,
        Format::M4a,
        Format::Flac,
    ];
    pub fn extension(self) -> &'static str {
        match self {
            Format::Mp4 => "mp4",
            Format::Mov => "mov",
            Format::Mkv => "mkv",
            Format::WebM => "webm",
            Format::Gif => "gif",
            Format::Wav => "wav",
            Format::Mp3 => "mp3",
            Format::M4a => "m4a",
            Format::Flac => "flac",
        }
    }
    pub fn audio_only(self) -> bool {
        matches!(self, Format::Wav | Format::Mp3 | Format::M4a | Format::Flac)
    }
    pub fn label(self) -> &'static str {
        match self {
            Format::Mp4 => "MP4",
            Format::Mov => "MOV",
            Format::Mkv => "MKV",
            Format::WebM => "WebM",
            Format::Gif => "GIF",
            Format::Wav => "WAV (nur Ton)",
            Format::Mp3 => "MP3 (nur Ton)",
            Format::M4a => "AAC/M4A (nur Ton)",
            Format::Flac => "FLAC (nur Ton)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
    ProRes,
    Vp9,
}

impl Codec {
    pub fn label(self) -> &'static str {
        match self {
            Codec::H264 => "H.264",
            Codec::Hevc => "HEVC (H.265)",
            Codec::Av1 => "AV1",
            Codec::ProRes => "ProRes 422 HQ",
            Codec::Vp9 => "VP9",
        }
    }
    /// Welche Codecs in welchem Container sinnvoll sind.
    pub fn for_format(f: Format) -> &'static [Codec] {
        match f {
            Format::Mp4 => &[Codec::H264, Codec::Hevc, Codec::Av1],
            Format::Mov => &[Codec::H264, Codec::Hevc, Codec::ProRes],
            Format::Mkv => &[Codec::H264, Codec::Hevc, Codec::Av1, Codec::Vp9],
            Format::WebM => &[Codec::Vp9, Codec::Av1],
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    Lower,
    Recommended,
    Higher,
    /// kbit/s
    Bitrate(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Encoder {
    /// Software (x264/x265/SVT-AV1/libvpx)
    Software,
    Nvenc,
    Qsv,
    Amf,
    /// Windows (Media Foundation): `h264_mf`/`hevc_mf`, nur Hardware
    Mf,
}

impl Encoder {
    /// Reihenfolge der automatischen Wahl
    pub const AUTO: [Encoder; 5] = [Encoder::Nvenc, Encoder::Qsv, Encoder::Amf, Encoder::Mf, Encoder::Software];

    /// Kennung in den Einstellungen („Hardware-Kodierung“)
    pub fn id(self) -> &'static str {
        match self {
            Encoder::Software => "software",
            Encoder::Nvenc => "nvenc",
            Encoder::Qsv => "qsv",
            Encoder::Amf => "amf",
            Encoder::Mf => "mf",
        }
    }
}

/// Encoder in der Reihenfolge der Versuche: der gewählte (`pref` = [`Encoder::id`]) und dann
/// Software; sonst oder wenn der gewählte hier fehlt (anderer Rechner, Treiber weg) automatisch
/// NVENC → QSV → AMF → Media Foundation → Software, soweit `found`. Software steht immer am Ende.
pub fn encoder_chain(found: &[Encoder], pref: &str) -> Vec<Encoder> {
    let have = |e: &Encoder| *e == Encoder::Software || found.contains(e);
    match Encoder::AUTO.into_iter().find(|e| have(e) && e.id() == pref) {
        Some(Encoder::Software) => vec![Encoder::Software],
        Some(e) => vec![e, Encoder::Software],
        None => Encoder::AUTO.into_iter().filter(have).collect(),
    }
}

/// Vorauswahl im Export-Dialog: Software – bei gleicher Qualität die kleinere Datei –, außer in den
/// Einstellungen ist ausdrücklich ein Weg gewählt, den es hier gibt. („Automatisch" gilt für Proxys.)
pub fn export_default(found: &[Encoder], pref: &str) -> Encoder {
    Encoder::AUTO.into_iter().find(|e| e.id() == pref && found.contains(e)).unwrap_or(Encoder::Software)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportSettings {
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub format: Format,
    pub codec: Codec,
    pub quality: Quality,
    pub encoder: Encoder,
    pub include_audio: bool,
    /// Nur einen Bereich exportieren
    pub range: Option<(Time, Time)>,
    /// Transparenter Hintergrund (nur ProRes 4444 in MOV und VP9 in WebM)
    #[serde(default)]
    pub transparent: bool,
    /// Lautheit angleichen auf dieses Ziel (LUFS), zweistufig mit `loudnorm`, Spitze −1 dBTP
    #[serde(default)]
    pub loudness: Option<f32>,
    /// AAC-Tonbitrate in kbit/s (`None` = 256)
    #[serde(default)]
    pub audio_kbps: Option<u32>,
    /// Marker als Kapitel in die Datei schreiben
    #[serde(default)]
    pub chapters: bool,
    /// Kapitelname für Marker ohne Namen (Sprache der Oberfläche)
    #[serde(default = "default_chapter_name")]
    pub chapter_name: String,
}

fn default_chapter_name() -> String {
    "Kapitel".into()
}

impl ExportSettings {
    pub fn from_project(p: &Project) -> Self {
        Self {
            width: p.settings.width,
            height: p.settings.height,
            fps: p.settings.fps,
            format: Format::Mp4,
            codec: Codec::H264,
            quality: Quality::Recommended,
            encoder: Encoder::Software,
            include_audio: true,
            range: None,
            transparent: false,
            loudness: None,
            audio_kbps: None,
            chapters: true,
            chapter_name: default_chapter_name(),
        }
    }

    /// Tatsächliche Tonbitrate im Video (kbit/s, 0 = ohne Ton), wie in `codec_args`.
    pub fn audio_bitrate(&self) -> u32 {
        match (self.format, self.codec) {
            _ if !self.include_audio || self.format == Format::Gif => 0,
            (Format::WebM, _) => 192,
            (_, Codec::ProRes) => 1536,
            _ => self.audio_kbps.unwrap_or(256),
        }
    }

    /// Größe der geschriebenen Datei (gerade Maße); GIF höchstens [`GIF_MAX_W`] breit.
    pub fn output_size(&self) -> (u32, u32) {
        let (w, h) = (self.width.max(2) & !1, self.height.max(2) & !1);
        if self.format == Format::Gif && w > GIF_MAX_W {
            let gh = (h as f64 * GIF_MAX_W as f64 / w as f64).round() as u32;
            return (GIF_MAX_W, gh.max(2) & !1);
        }
        (w, h)
    }

    /// Unterstützen Container und Codec einen Alphakanal?
    pub fn alpha_possible(&self) -> bool {
        matches!(
            (self.format, self.codec),
            (Format::Mov, Codec::ProRes) | (Format::WebM, Codec::Vp9)
        )
    }

    /// Wird tatsächlich mit Alphakanal exportiert?
    pub fn alpha(&self) -> bool {
        self.transparent && self.alpha_possible()
    }
}

/// GIF: höchstens so breit und so viele Bilder je Sekunde (Dateigröße).
pub const GIF_MAX_W: u32 = 720;
pub const GIF_FPS: u32 = 15;

/// Schriftauflösung für Textclips: Familie → (Datei, Face-Index).
pub type FontResolver<'a> = &'a dyn Fn(Option<&str>) -> Option<(PathBuf, u32)>;

/// Standardschrift des Systems (wenn ein Textclip keine Schrift gesetzt hat).
pub fn default_font() -> Option<PathBuf> {
    [
        "C:/Windows/Fonts/segoeui.ttf",
        "C:/Windows/Fonts/arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}

/// Ein vorbereiteter FFmpeg-Aufruf samt Arbeitsordner (wird beim Drop gelöscht).
pub struct Job {
    pub args: Vec<String>,
    pub workdir: tempfile::TempDir,
    pub duration: Time,
    /// Ziel; FFmpeg schreibt nach `part`, erst bei Erfolg wird umbenannt
    pub out: PathBuf,
    pub part: PathBuf,
    /// Lautheit: erst den Ton messen, dann `[aout]` linear angleichen (siehe [`run`])
    measure: Option<Measure>,
    /// Live-Vorschau (JPEG, wird während des Exports laufend überschrieben); nur mit Bild
    preview: Option<PathBuf>,
    /// Vorläufe der Shader-Übergänge (vor dem eigentlichen Export, siehe [`run`])
    passes: Vec<graph::ShaderPass>,
    /// Bilder des Exports (Gewichtung des Fortschritts gegenüber den Vorläufen)
    frames: f64,
    /// Großes Projekt: Abschnitte, die vor `args` laufen; `args` hängt sie dann nur noch
    /// aneinander (Bild ohne Neukodierung, Ton einmal begrenzt/angeglichen/kodiert)
    sections: Vec<Section>,
}

/// Ein Abschnitt eines großen Exports (siehe `sections.rs`).
struct Section {
    from: Time,
    to: Time,
    /// Ton-Samples (48 kHz) ab Exportbeginn: erstes Sample und Anzahl
    samples: (u64, u64),
    /// FFmpeg-Aufrufe: Bild (mit Vorschau), dann Ton
    runs: Vec<Vec<String>>,
    /// Graph des Bildes (Shader-Rückfälle werden dort eingesetzt)
    graph: String,
    passes: Vec<graph::ShaderPass>,
    /// Bild (Codec des Ziels bzw. FFV1 für GIF) und Ton (f32le, ohne Begrenzer)
    video: Option<String>,
    audio: Option<String>,
}

/// Anteil der Abschnitte am Fortschritt (Rest: Zusammenfügen, ggf. Lautheit)
const SECTIONS_SHARE: f32 = 0.96;
/// Ton aller Abschnitte hintereinander (f32le, Stereo, 48 kHz)
const MIX_FILE: &str = "mix.f32";
/// Zeichen der Befehlszeile ohne Eingänge (Programmpfad, feste Argumente, Vorschau)
const BASE_CHARS: usize = 900;

/// Breite des Live-Vorschaubilds in Pixeln.
pub const PREVIEW_W: u32 = 480;

struct Measure {
    args: Vec<String>,
    target: f32,
    /// Graph des Exports ohne die Angleichung
    graph: String,
}

impl Job {
    fn frames_per_sec(&self) -> f64 {
        self.frames / self.duration.as_secs_f64().max(1e-6)
    }

    /// Läuft vor dem Export ein Messlauf für „Lautheit angleichen“? (In Abschnitten läuft er
    /// erst am Ende über den fertigen Ton, kurz und ohne eigene Anzeige.)
    pub fn measures_loudness(&self) -> bool {
        self.measure.is_some() && self.sections.is_empty()
    }

    /// Befehlszeilen der Abschnitte (Messungen).
    #[doc(hidden)]
    pub fn section_args(&self) -> Vec<Vec<String>> {
        self.sections.iter().flat_map(|s| s.runs.clone()).collect()
    }

    /// Zahl der Abschnitte (0 = ein Durchgang).
    pub fn sections(&self) -> usize {
        self.sections.len()
    }

    /// Vorschaubild des laufenden Exports (existiert erst nach den ersten Bildern).
    pub fn preview_path(&self) -> Option<&Path> {
        self.preview.as_deref()
    }
}

/// Anteil des Messlaufs am gemeldeten Fortschritt (bei „Lautheit angleichen“).
pub const MEASURE_SHARE: f32 = 0.1;

fn loudnorm(target: f32) -> String {
    format!("loudnorm=I={target:.1}:TP=-1:LRA=50")
}

/// Zweiter Durchgang aus dem JSON des Messlaufs; Stille (−inf) bleibt unverändert.
/// `None`: Messergebnis fehlt, obwohl Ton da ist.
fn second_pass(log: &str, target: f32) -> Option<String> {
    let raw = |k: &str| {
        let key = format!("\"{k}\" : \"");
        let i = log.rfind(&key)? + key.len();
        log[i..].split('"').next()
    };
    let get = |k: &str| raw(k)?.parse::<f64>().ok().filter(|v| v.is_finite());
    raw("input_i")?;
    match get("input_i") {
        Some(i) if i > -70.0 => {}
        _ => return Some("anull".into()),
    }
    let (i, tp, lra, th, off) = (get("input_i")?, get("input_tp")?, get("input_lra")?, get("input_thresh")?, get("target_offset")?);
    // loudnorm nimmt measured_I/measured_thresh nur bis 0: vorher linear absenken, Messwerte mitschieben
    let n = (i.max(th) + 2.0).ceil().max(0.0);
    let pre = if n > 0.0 { format!("volume=-{n}dB,") } else { String::new() };
    let (i, tp, th) = (i - n, tp - n, th - n);
    Some(format!(
        "{pre}{}:measured_I={i}:measured_TP={tp}:measured_LRA={lra}:measured_thresh={th}:offset={off}:linear=true:print_format=none,aresample=48000",
        loudnorm(target)
    ))
}

/// `<stem>.part.<ext>` neben dem Ziel (Endung bleibt, FFmpeg wählt danach den Muxer).
fn part_path(out: &Path) -> PathBuf {
    let stem = out.file_stem().unwrap_or_default().to_string_lossy();
    match out.extension() {
        Some(e) => out.with_file_name(format!("{stem}.part.{}", e.to_string_lossy())),
        None => out.with_file_name(format!("{stem}.part")),
    }
}

pub(crate) fn f(x: f64) -> String {
    format!("{x:.6}")
}

/// Erzeugt den kompletten Export-Aufruf. Große Projekte (zu lange Befehlszeile, zu viele
/// Eingänge) werden in Abschnitten gerechnet, siehe `sections.rs`.
pub fn build(p: &Project, s: &ExportSettings, fonts: FontResolver, out: &Path) -> Result<Job> {
    build_with(p, s, fonts, out, sections::SINGLE_INPUTS, sections::MAX_INPUTS)
}

/// Wie [`build`] mit eigener Höchstzahl an Eingängen je FFmpeg (Tests und Messungen rechnen
/// damit auch kleine Projekte in Abschnitten).
#[doc(hidden)]
pub fn build_limited(p: &Project, s: &ExportSettings, fonts: FontResolver, out: &Path, max_inputs: usize) -> Result<Job> {
    build_with(p, s, fonts, out, max_inputs, max_inputs)
}

/// `single`: höchstens so viele Eingänge im einen Durchgang; `section`: je FFmpeg eines Abschnitts.
fn build_with(p: &Project, s: &ExportSettings, fonts: FontResolver, out: &Path, single: usize, section: usize) -> Result<Job> {
    let p = &in_edit_ranges(p);
    let total = p.duration();
    if total <= Time::ZERO {
        return Err(ExportError::Empty);
    }
    let (start, end) = s.range.unwrap_or((Time::ZERO, total));
    let missing = missing_media(p, start, end);
    if !missing.is_empty() {
        return Err(ExportError::MissingMedia(missing));
    }
    // leerer oder verkehrter Bereich (In = Out): FFmpeg bekäme `-t 0` bzw. eine negative Länge
    if end <= start {
        return Err(ExportError::Empty);
    }
    let units = sections::units(p, start, end, !s.format.audio_only(), wants_audio(s));
    let base = BASE_CHARS + out.as_os_str().len();
    let one = sections::Limits { inputs: single.max(1), ..sections::Limits::DEFAULT };
    let lim = sections::Limits { inputs: section.max(1), ..sections::Limits::DEFAULT };
    if sections::single_pass(&units, start, end, base, one) {
        let job = build_single(p, s, fonts, out, start, end)?;
        let inputs = job.args.iter().filter(|a| *a == "-i").count();
        if sections::cmd_len(&job.args) + 300 <= one.chars && inputs <= one.inputs + 1 {
            return Ok(job);
        }
    }
    match build_sections(p, s, fonts, out, (start, end), &units, base, lim) {
        // Übergangskette mit mehr Eingängen, als ein Abschnitt fassen darf (Diashow): ein
        // Durchgang wie bisher, solange die Befehlszeile passt
        Err(ExportError::TooManyClips(t)) => {
            let job = build_single(p, s, fonts, out, start, end)?;
            if sections::cmd_len(&job.args) + 300 <= sections::HARD_CMD {
                Ok(job)
            } else {
                Err(ExportError::TooManyClips(t))
            }
        }
        r => r,
    }
}

/// Mit Ton? (Nur-Ton-Formate immer, GIF nie)
fn wants_audio(s: &ExportSettings) -> bool {
    s.format.audio_only() || (s.include_audio && s.format != Format::Gif)
}

/// Ein Durchgang (kleine und mittlere Projekte).
fn build_single(p: &Project, s: &ExportSettings, fonts: FontResolver, out: &Path, start: Time, end: Time) -> Result<Job> {
    let dur = end - start;
    let workdir = tempfile::Builder::new()
        .prefix("cutline-export-")
        .tempdir()?;
    let (w, h) = (s.width & !1, s.height & !1);
    let mut ctx = graph::Ctx {
        w,
        h,
        fps: format!("{}/{}", s.fps.num, s.fps.den),
        start,
        workdir: workdir.path(),
        fonts,
        inputs: Vec::new(),
        n_input: 0,
        alpha: s.alpha(),
        graph: Vec::new(),
        uniq: 0,
        shader: true,
        passes: Vec::new(),
        pass_base: 0,
        lead: false,
    };
    let mut vlabel = None;
    if !s.format.audio_only() {
        let full = graph::video_part(&mut ctx, p, end, dur)?;
        // Live-Vorschau für den Export-Dialog: zweiter Zweig, 2 Bilder/s, klein
        ctx.graph.push(format!("[{full}]split[vmain][pv0]"));
        ctx.graph.push(format!("[pv0]fps=2:eof_action=pass,scale={PREVIEW_W}:-2[pv]"));
        let cur = "vmain";
        if s.format == Format::Gif {
            let (gw, gh) = s.output_size();
            ctx.graph.push(format!(
                "[{cur}]fps=min({GIF_FPS}\\,{}),scale={gw}:{gh}:flags=lanczos,split[g0][g1]",
                ctx.fps
            ));
            ctx.graph.push("[g0]palettegen=stats_mode=diff[pal]".into());
            ctx.graph
                .push("[g1][pal]paletteuse=dither=sierra2_4a[vout]".into());
        } else {
            ctx.graph.push(format!("[{cur}]format={}[vout]", pix_fmt(s)));
        }
        vlabel = Some("vout");
    }
    let mut alabel = wants_audio(s).then(|| {
        graph::audio_part(
            p,
            start,
            end,
            &mut ctx.inputs,
            &mut ctx.n_input,
            &mut ctx.graph,
            EXPORT_AUDIO,
        )
    });

    std::fs::write(workdir.path().join("graph.txt"), ctx.graph.join(";\n"))?;
    // Messlauf nur über den Ton (gleicher Graph wie die Vorschau), nicht über das ganze Video
    let measure = match (s.loudness, &alabel) {
        (Some(target), Some(_)) => {
            let (mut inputs, mut graph, mut n) = (Vec::new(), Vec::new(), 0);
            let l = graph::audio_part(p, start, end, &mut inputs, &mut n, &mut graph, EXPORT_AUDIO);
            graph.push(format!("[{l}]{}:print_format=json[m]", loudnorm(target)));
            std::fs::write(workdir.path().join("measure.txt"), graph.join(";\n"))?;
            let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-progress", "pipe:1", "-nostats"]
                .map(String::from)
                .into();
            args.extend(inputs);
            args.extend(
                ["-/filter_complex", "measure.txt", "-map", "[m]", "-t", &f(dur.as_secs_f64()), "-f", "null", "-"]
                    .map(String::from),
            );
            alabel = Some("anorm".into());
            Some(Measure { args, target, graph: ctx.graph.join(";\n") })
        }
        _ => None,
    };
    let mut args = head();
    args.extend(std::mem::take(&mut ctx.inputs));
    let chapters = chapters_input(p, s, (start, end), workdir.path())?;
    args.extend(chapters.iter().cloned());
    args.extend(["-/filter_complex".into(), "graph.txt".into()]);
    if let Some(v) = vlabel {
        args.extend(["-map".into(), format!("[{v}]")]);
    }
    if let Some(a) = &alabel {
        args.extend(["-map".into(), format!("[{a}]")]);
    }
    if !chapters.is_empty() {
        args.extend(["-map_chapters".into(), ctx.n_input.to_string()]);
    }
    args.extend(codec_args(s));
    args.extend(["-t".into(), f(dur.as_secs_f64())]);
    let part = part_path(out);
    args.push(part.to_string_lossy().into_owned());
    // zweite Ausgabe: ein immer wieder überschriebenes JPEG im Arbeitsordner
    let preview = vlabel.map(|_| {
        args.extend(
            ["-map", "[pv]", "-an", "-f", "image2", "-update", "1", "-q:v", "6", "-t", &f(dur.as_secs_f64()), PREVIEW_FILE]
                .map(String::from),
        );
        workdir.path().join(PREVIEW_FILE)
    });
    let passes = std::mem::take(&mut ctx.passes);
    Ok(Job {
        args,
        workdir,
        duration: dur,
        out: out.to_path_buf(),
        part,
        measure,
        preview,
        passes,
        frames: dur.as_secs_f64() * s.fps.num as f64 / s.fps.den.max(1) as f64,
        sections: Vec::new(),
    })
}

/// Ton-Mix des Exports (ein Durchgang): mit Begrenzer, wartet auf die Kanalprüfung.
const EXPORT_AUDIO: graph::AudioOpts = graph::AudioOpts { wait_probe: true, limiter: true, lead: false, shared: false, skip_bad: false };

/// Pixelformat des Bildes vor dem Encoder.
fn pix_fmt(s: &ExportSettings) -> &'static str {
    match (s.alpha(), s.codec) {
        (true, Codec::ProRes) => "yuva444p10le",
        (true, _) => "yuva420p",
        _ => "yuv420p",
    }
}

/// Ein FFmpeg-Aufruf eines Abschnitts: Graph in die Datei `gfile`, Eingänge, Ausgänge.
fn section_run(dir: &Path, gfile: &str, inputs: Vec<String>, graph: &[String], outs: Vec<String>) -> Result<Vec<String>> {
    std::fs::write(dir.join(gfile), graph.join(";\n"))?;
    let mut args = head();
    args.extend(inputs);
    args.extend(["-/filter_complex".into(), gfile.into()]);
    args.extend(outs);
    Ok(args)
}

/// Feste Kopf-Argumente jedes Export-Laufs (Fortschritt über stdout).
fn head() -> Vec<String> {
    ["-y", "-v", "error", "-nostdin", "-progress", "pipe:1", "-nostats"].map(String::from).into()
}

/// Marker als Kapitel in die Datei (Player zeigen sie an; gleiche Liste wie die YouTube-Kapitel):
/// schreibt `chapters.txt` in `dir` und gibt den Eingang dafür zurück (leer ohne Kapitel).
fn chapters_input(p: &Project, s: &ExportSettings, (start, end): (Time, Time), dir: &Path) -> Result<Vec<String>> {
    let fits = matches!(s.format, Format::Mp4 | Format::Mov | Format::Mkv | Format::WebM | Format::M4a);
    let Some(meta) = (s.chapters && fits).then(|| ffmetadata(&p.timeline.markers, start, end, &s.chapter_name)).flatten() else {
        return Ok(Vec::new());
    };
    std::fs::write(dir.join("chapters.txt"), meta)?;
    Ok(["-f", "ffmetadata", "-i", "chapters.txt"].map(String::from).into())
}

/// Erstes Sample (48 kHz) zur Zeit `t` ab `start` (auf dem Abschnittsraster ganzzahlig).
fn sample_at(t: Time, start: Time) -> u64 {
    ((t.0 - start.0).max(0) as u128 * 48_000 / 1_000_000) as u64
}

/// Geschätzter Platz der Zwischendateien (Bytes): Bild großzügig je Codec, Ton als f32.
fn scratch_bytes(s: &ExportSettings, dur: f64) -> u64 {
    let (w, h) = s.output_size();
    let px = w as f64 * h as f64 * s.fps.as_f64();
    let video_bits = if s.format.audio_only() {
        0.0
    } else if s.format == Format::Gif || copy_unproven(s) {
        px * 8.0 // FFV1, verlustfrei
    } else if let Quality::Bitrate(k) = s.quality {
        k as f64 * 1100.0
    } else if s.codec == Codec::ProRes && Codec::for_format(s.format).contains(&Codec::ProRes) {
        px * if s.alpha() { 5.5 } else { 3.6 }
    } else {
        px * 0.15
    };
    let audio = if wants_audio(s) { 48_000.0 * 8.0 } else { 0.0 };
    ((video_bits / 8.0 + audio) * dur) as u64 + (64 << 20)
}

/// Hardware-Encoder, deren Abschnitte beim Zusammenfügen mit `-c:v copy` nicht geprüft sind
/// (QSV, AMF, AV1 über NVENC: auf dem Prüfrechner nicht startbar): Abschnitte dann verlustfrei
/// (FFV1) und einmal über das Ganze mit dem gewählten Encoder kodieren, wie bei GIF.
/// Media Foundation ist geprüft (Test `sections_hardware_encoders_concat`, NVIDIA-MFT).
fn copy_unproven(s: &ExportSettings) -> bool {
    matches!(
        (video_codec(s), s.encoder),
        (Codec::H264 | Codec::Hevc, Encoder::Qsv | Encoder::Amf) | (Codec::Av1, Encoder::Nvenc)
    )
}

/// Nötiger Platz je Laufwerk (Ordner, Bytes): Die Abschnitte (`scratch`) bleiben liegen, bis die
/// fertige Datei (etwa gleich groß) geschrieben ist; liegt sie auf demselben Laufwerk, zählt
/// alles doppelt.
fn space_needs(scratch: u64, work: &Path, out: &Path) -> Vec<(PathBuf, u64)> {
    let dir = out.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let root = |p: &Path| std::path::absolute(p).ok().and_then(|a| a.components().next().map(|c| c.as_os_str().to_string_lossy().to_uppercase()));
    if root(work) == root(dir) {
        vec![(work.to_path_buf(), 2 * scratch)]
    } else {
        vec![(work.to_path_buf(), scratch), (dir.to_path_buf(), scratch)]
    }
}

/// Großes Projekt: Abschnitte planen und je Abschnitt den Bereichs-Export bauen, dazu den
/// Endlauf, der alles zusammenfügt.
#[allow(clippy::too_many_arguments)]
fn build_sections(
    p: &Project,
    s: &ExportSettings,
    fonts: FontResolver,
    out: &Path,
    (start, end): (Time, Time),
    units: &[sections::Unit],
    base: usize,
    lim: sections::Limits,
) -> Result<Job> {
    let plan = sections::plan(units, start, end, sections::grid_step(s.fps), base, lim)
        .map_err(|t| ExportError::TooManyClips(sections::clock(t)))?;
    let workdir = tempfile::Builder::new().prefix("cutline-export-").tempdir()?;
    let dur = end - start;
    for (dir, need) in space_needs(scratch_bytes(s, dur.as_secs_f64()), workdir.path(), out) {
        if let Some(free) = sections::free_space(&dir) {
            if free < need {
                return Err(ExportError::NoSpace { need: sections::size_text(need), free: sections::size_text(free) });
            }
        }
    }
    let (video, audio, gif) = (!s.format.audio_only(), wants_audio(s), s.format == Format::Gif);
    let (w, h) = (s.width & !1, s.height & !1);
    let fps = format!("{}/{}", s.fps.num, s.fps.den);
    let lossless = video && copy_unproven(s);
    let ext = if gif || lossless { "nut" } else { s.format.extension() };
    let (mut uniq, mut pass_base) = (0, 0);
    let mut secs = Vec::new();
    for (k, &(a, b)) in plan.iter().enumerate() {
        let mut ctx = graph::Ctx {
            w,
            h,
            fps: fps.clone(),
            start: a,
            workdir: workdir.path(),
            fonts,
            inputs: Vec::new(),
            n_input: 0,
            alpha: s.alpha(),
            graph: Vec::new(),
            uniq,
            shader: true,
            passes: Vec::new(),
            pass_base,
            lead: true,
        };
        let len = f((b - a).as_secs_f64());
        let (vname, aname) = (format!("sec_{k}.{ext}"), format!("sec_{k}.f32"));
        let gfile = format!("graph_{k}.txt");
        // Bild und Ton je in eigenem FFmpeg: in einem gemeinsamen Graphen mit vielen Eingängen
        // warten die Zweige aufeinander (gemessen: ein Vielfaches der Summe beider Läufe)
        let mut runs = Vec::new();
        if video {
            let full = graph::video_part(&mut ctx, p, b, b - a)?;
            ctx.graph.push(format!("[{full}]split[vmain][pv0]"));
            ctx.graph.push(format!("[pv0]fps=2:eof_action=pass,scale={PREVIEW_W}:-2[pv]"));
            let mut outs: Vec<String> = Vec::new();
            if gif {
                // GIF: nur die Größe hier (je Bild); Bildrate und Palette einmal über das Ganze
                let (gw, gh) = s.output_size();
                ctx.graph.push(format!("[vmain]scale={gw}:{gh}:flags=lanczos[vout]"));
                outs.extend(["-map", "[vout]", "-c:v", "ffv1", "-level", "3"].map(String::from));
            } else {
                ctx.graph.push(format!("[vmain]format={}[vout]", pix_fmt(s)));
                outs.extend(["-map".into(), "[vout]".into()]);
                outs.extend(if lossless { ["-c:v", "ffv1", "-level", "3"].map(String::from).into() } else { video_args(s) });
            }
            outs.extend(["-t".into(), len.clone(), vname.clone()]);
            outs.extend(["-map", "[pv]", "-an", "-f", "image2", "-update", "1", "-q:v", "6", "-t", &len, PREVIEW_FILE].map(String::from));
            runs.push(section_run(workdir.path(), &gfile, std::mem::take(&mut ctx.inputs), &ctx.graph, outs)?);
        }
        if audio {
            let o = graph::AudioOpts { wait_probe: true, limiter: false, lead: true, shared: false, skip_bad: false };
            let (mut inputs, mut graph, mut n) = (Vec::new(), Vec::new(), 0);
            let l = graph::audio_part(p, a, b, &mut inputs, &mut n, &mut graph, o);
            let mut outs = vec!["-map".into(), format!("[{l}]"), "-c:a".into(), "pcm_f32le".into(), "-t".into(), len.clone()];
            outs.extend(["-f".into(), "f32le".into(), aname.clone()]);
            runs.push(section_run(workdir.path(), &format!("graph_{k}_a.txt"), inputs, &graph, outs)?);
        }
        if runs.iter().any(|r| sections::cmd_len(r) > sections::HARD_CMD) {
            return Err(ExportError::TooManyClips(sections::clock(a)));
        }
        uniq = ctx.uniq;
        pass_base += ctx.passes.len();
        let samples = (sample_at(a, start), sample_at(b, start) - sample_at(a, start));
        secs.push(Section {
            from: a,
            to: b,
            samples,
            runs,
            graph: gfile,
            passes: std::mem::take(&mut ctx.passes),
            video: video.then_some(vname),
            audio: audio.then_some(aname),
        });
    }
    // Endlauf: Bild aneinanderhängen (Liste der Abschnitte, ohne Neukodierung; GIF: Palette über
    // das Ganze), Ton einmal begrenzen, ggf. angleichen und kodieren
    let d = f(dur.as_secs_f64());
    let mut args = head();
    let (mut n, mut graph) = (0usize, Vec::<String>::new());
    let mut vmap = None;
    if video {
        args.extend(["-f", "concat", "-safe", "0", "-i", "video.txt"].map(String::from));
        n += 1;
        if gif {
            graph.push(format!("[0:v]fps=min({GIF_FPS}\\,{fps}),split[g0][g1]"));
            graph.push("[g0]palettegen=stats_mode=diff[pal]".into());
            graph.push("[g1][pal]paletteuse=dither=sierra2_4a[vout]".into());
            vmap = Some("[vout]".to_string());
        } else {
            vmap = Some("0:v".to_string());
        }
    }
    let raw_in = ["-f", "f32le", "-ar", "48000", "-ch_layout", "stereo", "-i", MIX_FILE].map(String::from);
    let tail = format!("{},apad=whole_dur={d},atrim=0:{d},aformat=sample_fmts=fltp:channel_layouts=stereo[aout]", graph::limiter());
    let mut alabel = None;
    if audio {
        args.extend(raw_in.clone());
        graph.push(format!("[{n}:a]{tail}"));
        n += 1;
        alabel = Some("aout");
    }
    let measure = match (s.loudness, audio) {
        (Some(target), true) => {
            let mg = format!("[0:a]{tail};\n[aout]{}:print_format=json[m]", loudnorm(target));
            std::fs::write(workdir.path().join("measure.txt"), mg)?;
            let mut margs: Vec<String> = ["-hide_banner", "-nostdin", "-progress", "pipe:1", "-nostats"].map(String::from).into();
            margs.extend(raw_in);
            margs.extend(["-/filter_complex", "measure.txt", "-map", "[m]", "-t", &d, "-f", "null", "-"].map(String::from));
            alabel = Some("anorm");
            Some(Measure { args: margs, target, graph: graph.join(";\n") })
        }
        _ => None,
    };
    let chapters = chapters_input(p, s, (start, end), workdir.path())?;
    args.extend(chapters.iter().cloned());
    if !graph.is_empty() {
        std::fs::write(workdir.path().join("graph.txt"), graph.join(";\n"))?;
        args.extend(["-/filter_complex".into(), "graph.txt".into()]);
    }
    if let Some(v) = &vmap {
        args.extend(["-map".into(), v.clone()]);
    }
    if let Some(a) = alabel {
        args.extend(["-map".into(), format!("[{a}]")]);
    }
    if !chapters.is_empty() {
        args.extend(["-map_chapters".into(), n.to_string()]);
    }
    if gif || lossless {
        args.extend(codec_args(s));
    } else {
        if video {
            args.extend(["-c:v".into(), "copy".into()]);
            if video_args(s).iter().any(|a| a == "hvc1") {
                args.extend(["-tag:v".into(), "hvc1".into()]);
            }
        }
        args.extend(if video { audio_args(s) } else { codec_args(s) });
        if matches!(s.format, Format::Mp4 | Format::Mov) {
            args.extend(["-movflags".into(), "+faststart".into()]);
        }
    }
    args.extend(["-t".into(), d.clone()]);
    let part = part_path(out);
    args.push(part.to_string_lossy().into_owned());
    let preview = video.then(|| workdir.path().join(PREVIEW_FILE));
    Ok(Job {
        args,
        workdir,
        duration: dur,
        out: out.to_path_buf(),
        part,
        measure,
        preview,
        passes: Vec::new(),
        frames: dur.as_secs_f64() * s.fps.as_f64(),
        sections: secs,
    })
}

/// Dateinamen fehlender Medien, die ein aktiver Clip im Bereich `start..end` nutzt (samt den
/// über Übergänge verbundenen Nachbarn, die der Export mitliest); ungenutzte dürfen fehlen.
fn missing_media(p: &Project, start: Time, end: Time) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for g in p.timeline.tracks.iter().flat_map(graph::transition_groups) {
        if g.last().is_none_or(|c| c.end() <= start) || g[0].start >= end {
            continue;
        }
        for c in g {
            let ClipContent::Media { media, .. } = &c.content else { continue };
            let name = match p.media(*media) {
                Some(m) if m.is_local() => continue,
                Some(m) => m.path.file_name().map_or_else(|| m.path.display().to_string(), |n| n.to_string_lossy().into_owned()),
                None => media.to_string(),
            };
            if !v.contains(&name) {
                v.push(name);
            }
        }
    }
    v
}

/// „Datei fehlt: a.mp4“ bzw. „4 Dateien fehlen: a.mp4, b.wav, c.png …“
pub fn missing_text(names: &[String]) -> String {
    match names {
        [one] => format!("Datei fehlt: {one}"),
        _ => format!(
            "{} Dateien fehlen: {}{}",
            names.len(),
            names.iter().take(3).cloned().collect::<Vec<_>>().join(", "),
            if names.len() > 3 { " …" } else { "" }
        ),
    }
}

/// Lautstärke und Tempo in die Bereiche der Bearbeitung (wie `EditOp::SetVolume`/`SetSpeed`):
/// Projektdateien und MCP können mehr enthalten, +1000 dB bzw. Tempo 0,001 brachen den
/// AAC-Encoder ab.
/// Liegt schon alles in den Bereichen, ohne Kopie (Ton-Start der Vorschau bei jeder Bearbeitung).
fn in_edit_ranges(p: &Project) -> std::borrow::Cow<'_, Project> {
    let db = |v: f32| (-60.0..=20.0).contains(&v);
    let ok = p.timeline.tracks.iter().flat_map(|t| t.clips.iter()).all(|c| {
        db(c.volume_db) && (0.1..=100.0).contains(&c.speed) && c.keyframes.0.get(&Prop::Volume).is_none_or(|v| v.iter().all(|k| db(k.v)))
    });
    if ok {
        return std::borrow::Cow::Borrowed(p);
    }
    let mut p = p.clone();
    for c in p.timeline.tracks.iter_mut().flat_map(|t| t.clips.iter_mut()) {
        c.volume_db = c.volume_db.clamp(-60.0, 20.0);
        c.speed = c.speed.clamp(0.1, 100.0);
        if let Some(v) = c.keyframes.0.get_mut(&Prop::Volume) {
            for k in v {
                k.v = k.v.clamp(-60.0, 20.0);
            }
        }
    }
    std::borrow::Cow::Owned(p)
}

/// Kapitel (Start relativ zu `start`, Titel) aus den Markern im Bereich `start..end`.
/// YouTube verlangt ein Kapitel bei 0:00: Ein Marker in der ersten Sekunde wird dorthin
/// gezogen, sonst kommt davor das Kapitel, das am Bereichsanfang läuft (letzter Marker davor),
/// ohne solchen Marker „Intro“. Marker ohne Namen heißen `unnamed`.
pub fn chapter_list(markers: &[cl_core::Marker], start: Time, end: Time, unnamed: &str) -> Vec<(Time, String)> {
    let name = |m: &cl_core::Marker| if m.name.is_empty() { unnamed.to_string() } else { m.name.clone() };
    let mut v: Vec<(Time, String)> = markers
        .iter()
        .filter(|m| m.at >= start && m.at < end)
        .map(|m| (m.at - start, name(m)))
        .collect();
    v.sort_by_key(|m| m.0);
    match v.first_mut() {
        Some(f) if f.0 < Time::from_secs_f64(1.0) => f.0 = Time::ZERO,
        _ => {
            let running = markers.iter().filter(|m| m.at < start).max_by_key(|m| m.at);
            v.insert(0, (Time::ZERO, running.map_or_else(|| "Intro".into(), name)));
        }
    }
    // zwei Marker in derselben Sekunde ergäben zweimal dieselbe Zeit: der erste gewinnt
    v.dedup_by(|b, a| (b.0 - a.0) < Time::from_secs_f64(1.0));
    v
}

/// Kapitel im FFmetadata-Format (siehe [`chapter_list`]); `None` ohne Marker.
fn ffmetadata(markers: &[cl_core::Marker], start: Time, end: Time, unnamed: &str) -> Option<String> {
    let esc = |t: &str| {
        t.chars()
            .flat_map(|c| match c {
                '=' | ';' | '#' | '\\' => vec!['\\', c],
                '\n' => vec!['\\', '\n'],
                c => vec![c],
            })
            .collect::<String>()
    };
    if !markers.iter().any(|m| m.at >= start && m.at < end) {
        return None;
    }
    let v = chapter_list(markers, start, end, unnamed);
    let ms = |t: Time| (t.as_secs_f64() * 1000.0).round() as i64;
    let len = end - start;
    let mut out = String::from(";FFMETADATA1\n");
    for (i, (t, name)) in v.iter().enumerate() {
        let e = v.get(i + 1).map(|n| n.0).unwrap_or(len);
        out += &format!(
            "[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={}\n",
            ms(*t),
            ms(e),
            esc(name)
        );
    }
    Some(out)
}

/// AV1 in Software: SVT-AV1, wenn das FFmpeg ihn mitbringt, sonst libaom (das mitgelieferte
/// Windows-FFmpeg hat kein SVT-AV1). Endet auf `-crf`, der Wert folgt.
fn av1_software(svt: bool) -> &'static [&'static str] {
    if svt {
        &["-c:v", "libsvtav1", "-preset", "8", "-crf"]
    } else {
        &["-c:v", "libaom-av1", "-cpu-used", "6", "-row-mt", "1", "-b:v", "0", "-crf"]
    }
}

/// Kennt das benutzte FFmpeg den Encoder libsvtav1? (einmal je Programmlauf geprüft)
fn has_svtav1() -> bool {
    static HAS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAS.get_or_init(|| {
        cl_media::ffmpeg_command()
            .ok()
            .and_then(|mut c| {
                c.args(["-hide_banner", "-encoders"])
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output()
                    .ok()
            })
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains("libsvtav1"))
    })
}

fn codec_args(s: &ExportSettings) -> Vec<String> {
    let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match s.format {
        Format::Wav => return v(&["-c:a", "pcm_s16le"]),
        Format::Mp3 => return v(&["-c:a", "libmp3lame", "-b:a", "320k"]),
        Format::M4a => return v(&["-c:a", "aac", "-b:a", "256k"]),
        Format::Flac => return v(&["-c:a", "flac"]),
        Format::Gif => return v(&["-loop", "0"]),
        _ => {}
    }
    let mut a = video_args(s);
    a.extend(audio_args(s));
    if matches!(s.format, Format::Mp4 | Format::Mov) {
        a.extend(v(&["-movflags", "+faststart"]));
    }
    a
}

/// Codec des Bildes (passend zum Container).
fn video_codec(s: &ExportSettings) -> Codec {
    if Codec::for_format(s.format).contains(&s.codec) {
        s.codec
    } else {
        Codec::for_format(s.format).first().copied().unwrap_or(Codec::H264)
    }
}

/// Ton-Encoder in Videoformaten (leer ohne Ton).
fn audio_args(s: &ExportSettings) -> Vec<String> {
    let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    if !s.include_audio {
        return Vec::new();
    }
    match (s.format, video_codec(s)) {
        (Format::WebM, _) => v(&["-c:a", "libopus", "-b:a", "192k"]),
        (_, Codec::ProRes) => v(&["-c:a", "pcm_s16le"]),
        _ => vec!["-c:a".into(), "aac".into(), "-b:a".into(), format!("{}k", s.audio_bitrate())],
    }
}

/// Bitrate (kbit/s) für Media Foundation: Ein Modus mit konstanter Qualität ist nicht bei jedem
/// Hardware-MFT sicher (ein unbekannter wird still übergangen, dann kodiert er mit < 1 Mbit/s),
/// darum Bits je Bildpunkt: 1080p60 H.264 ≈ 6 / 10 / 15 Mbit/s, HEVC 70 %.
fn mf_kbps(s: &ExportSettings, c: Codec) -> u32 {
    let (w, h) = s.output_size();
    let bpp = match s.quality {
        Quality::Lower => 0.05,
        Quality::Higher => 0.12,
        _ => 0.08,
    } * if c == Codec::Hevc { 0.7 } else { 1.0 };
    (w as f64 * h as f64 * s.fps.as_f64() * bpp / 1000.0).round().max(500.0) as u32
}

/// Bild-Encoder samt Bildrate (Videoformate).
fn video_args(s: &ExportSettings) -> Vec<String> {
    let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut a = Vec::new();
    let crf = |lo: u32, rec: u32, hi: u32| match s.quality {
        Quality::Lower => lo,
        Quality::Recommended => rec,
        Quality::Higher => hi,
        Quality::Bitrate(_) => rec,
    };
    match (video_codec(s), s.encoder) {
        (Codec::H264, Encoder::Nvenc) => a.extend(
            v(&["-c:v", "h264_nvenc", "-preset", "p5", "-cq"])
                .into_iter()
                .chain([crf(28, 21, 17).to_string()]),
        ),
        (Codec::Hevc, Encoder::Nvenc) => a.extend(
            v(&["-c:v", "hevc_nvenc", "-preset", "p5", "-cq"])
                .into_iter()
                .chain([crf(30, 24, 19).to_string()]),
        ),
        (Codec::Av1, Encoder::Nvenc) => a.extend(
            v(&["-c:v", "av1_nvenc", "-preset", "p5", "-cq"])
                .into_iter()
                .chain([crf(36, 30, 24).to_string()]),
        ),
        (Codec::H264, Encoder::Qsv) => a.extend(
            v(&["-c:v", "h264_qsv", "-global_quality"])
                .into_iter()
                .chain([crf(28, 21, 17).to_string()]),
        ),
        (Codec::Hevc, Encoder::Qsv) => a.extend(
            v(&["-c:v", "hevc_qsv", "-global_quality"])
                .into_iter()
                .chain([crf(30, 24, 19).to_string()]),
        ),
        (Codec::H264, Encoder::Amf) => a.extend(v(&["-c:v", "h264_amf", "-quality", "quality"])),
        (Codec::Hevc, Encoder::Amf) => a.extend(v(&["-c:v", "hevc_amf", "-quality", "quality"])),
        (c @ (Codec::H264 | Codec::Hevc), Encoder::Mf) => {
            let name = if c == Codec::H264 { "h264_mf" } else { "hevc_mf" };
            // `-hw_encoding 1`: ohne Hardware-Weg scheitert der Start (kein Software-MFT);
            // `-g 250` wie x264/NVENC (ohne: alle 12 Bilder ein Schlüsselbild)
            a.extend(v(&["-c:v", name, "-pix_fmt", "nv12", "-hw_encoding", "1", "-g", "250"]));
            if !matches!(s.quality, Quality::Bitrate(_)) {
                a.extend(["-b:v".into(), format!("{}k", mf_kbps(s, c))]);
            }
        }
        (Codec::H264, _) => a.extend(
            v(&["-c:v", "libx264", "-preset", "medium", "-crf"])
                .into_iter()
                .chain([crf(26, 20, 16).to_string()]),
        ),
        (Codec::Hevc, _) => a.extend(
            v(&[
                "-c:v", "libx265", "-preset", "medium", "-tag:v", "hvc1", "-crf",
            ])
            .into_iter()
            .chain([crf(30, 24, 19).to_string()]),
        ),
        (Codec::Av1, _) => a.extend(
            v(av1_software(has_svtav1()))
                .into_iter()
                .chain([crf(38, 32, 26).to_string()]),
        ),
        (Codec::Vp9, _) => {
            a.extend(
                v(&["-c:v", "libvpx-vp9", "-b:v", "0", "-row-mt", "1", "-crf"])
                    .into_iter()
                    .chain([crf(38, 32, 26).to_string()]),
            );
            if s.alpha() {
                a.extend(v(&["-pix_fmt", "yuva420p", "-auto-alt-ref", "0"]));
            }
        }
        (Codec::ProRes, _) if s.alpha() => a.extend(v(&[
            "-c:v",
            "prores_ks",
            "-profile:v",
            "4",
            "-pix_fmt",
            "yuva444p10le",
            "-alpha_bits",
            "16",
            "-vendor",
            "apl0",
        ])),
        (Codec::ProRes, _) => a.extend(v(&[
            "-c:v",
            "prores_ks",
            "-profile:v",
            "3",
            "-pix_fmt",
            "yuv422p10le",
        ])),
    }
    if let Quality::Bitrate(k) = s.quality {
        // Konstante Qualität (-crf/-cq/-global_quality) würde die Bitrate übersteuern
        if let Some(i) = a.iter().position(|x| matches!(x.as_str(), "-crf" | "-cq" | "-global_quality")) {
            a.drain(i..i + 2);
        }
        a.extend(["-b:v".into(), format!("{k}k")]);
        // Deckel knapp über dem Ziel, sonst überziehen vor allem Hardware-Encoder deutlich;
        // libsvtav1 kennt -maxrate nur im CRF-Modus und bricht sonst ab
        if !a.iter().any(|x| x == "libsvtav1") {
            a.extend(["-maxrate".into(), format!("{}k", k + k / 10), "-bufsize".into(), format!("{}k", k * 2)]);
        }
    }
    a.extend(["-r".into(), format!("{}/{}", s.fps.num, s.fps.den)]);
    a
}

/// Läuft einen Job und meldet den Fortschritt (0..1). `cancel` bricht ab.
/// Eine vorhandene Zieldatei bleibt bei Abbruch oder Fehler unverändert.
/// Mit „Lautheit angleichen“ läuft vorher der Messlauf (erste [`MEASURE_SHARE`] des Fortschritts).
pub fn run(job: &Job, cancel: Arc<AtomicBool>, mut progress: impl FnMut(f32)) -> Result<()> {
    let r = run_job(job, &cancel, &mut progress);
    if r.is_err() {
        let _ = std::fs::remove_file(&job.part);
    }
    r.and_then(|_| finish(&job.part, &job.out))
}

fn run_job(job: &Job, cancel: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<()> {
    // große Projekte: erst die Abschnitte, dann fügt `args` sie zusammen
    let (lo, rest) = if job.sections.is_empty() {
        (0.0, 1.0)
    } else {
        run_sections(job, cancel, &mut |x| progress(x * SECTIONS_SHARE))?;
        (SECTIONS_SHARE, 1.0 - SECTIONS_SHARE)
    };
    let progress = &mut |x: f32| progress(lo + x * rest);
    let secs = job.duration.as_secs_f64();
    // Vorläufe der Shader-Übergänge: ihr Anteil am Fortschritt nach Bildern
    let pre: usize = job.passes.iter().map(|p| p.progress.len()).sum();
    let share = if pre == 0 { 0.0 } else { (pre as f64 / (pre as f64 + job.frames.max(1.0))).min(0.5) as f32 };
    let failed = run_passes(job, &job.passes, cancel, &mut |x| progress(x * share))?;
    let fallbacks = |g: &str| {
        failed
            .iter()
            .fold(g.to_string(), |g, p| g.replace(&p.block, &p.fallback))
    };
    if !failed.is_empty() {
        let file = job.workdir.path().join("graph.txt");
        std::fs::write(&file, fallbacks(&std::fs::read_to_string(&file)?))?;
    }
    let mut progress = |x: f32| progress(share + x * (1.0 - share));
    match &job.measure {
        Some(m) => run_ffmpeg(job, &m.args, secs, cancel, &mut |x| progress(x * MEASURE_SHARE)).and_then(|log| {
            let pass = second_pass(&log, m.target).ok_or(ExportError::Loudness)?;
            let g = format!("{};\n[aout]{pass}[anorm]", fallbacks(&m.graph));
            std::fs::write(job.workdir.path().join("graph.txt"), g)?;
            run_ffmpeg(job, &job.args, secs, cancel, &mut |x| progress(MEASURE_SHARE + x * (1.0 - MEASURE_SHARE)))
        }),
        None => run_ffmpeg(job, &job.args, secs, cancel, &mut progress),
    }
    .map(|_| ())
}

/// Alle Abschnitte nacheinander (Fortschritt 0..1 nach Länge); hängt den Ton sample-genau an
/// `mix.f32` und schreibt die Bildliste `video.txt` für den Endlauf. Ein Fehler nennt den
/// Zeitbereich des Abschnitts; Zwischendateien liegen im Arbeitsordner (fällt mit dem Job weg).
fn run_sections(job: &Job, cancel: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<()> {
    let dir = job.workdir.path();
    let total: f64 = job.sections.iter().map(|s| (s.to - s.from).as_secs_f64()).sum::<f64>().max(1e-6);
    let mut done = 0.0;
    let mut mix = match job.sections.first().and_then(|s| s.audio.as_ref()) {
        Some(_) => Some(std::io::BufWriter::new(std::fs::File::create(dir.join(MIX_FILE))?)),
        None => None,
    };
    let mut list = String::new();
    for sec in &job.sections {
        let len = (sec.to - sec.from).as_secs_f64();
        let w = len / total;
        let r = run_section(job, sec, cancel, &mut |x| progress((done + x as f64 * w).min(1.0) as f32)).and_then(|_| {
            if let (Some(a), Some(mix)) = (&sec.audio, mix.as_mut()) {
                append_exact(&dir.join(a), mix, sec.samples.1 * 8)?;
                let _ = std::fs::remove_file(dir.join(a));
            }
            Ok(())
        });
        match r {
            Err(e @ (ExportError::Cancelled | ExportError::Section { .. })) => return Err(e),
            Err(e) => return Err(section_error(sec, e, Vec::new())),
            Ok(()) => {}
        }
        if let Some(v) = &sec.video {
            list += &format!("file '{v}'\nduration {}\n", f(len));
        }
        done += w;
    }
    if let Some(mut m) = mix {
        std::io::Write::flush(&mut m)?;
    }
    if !list.is_empty() {
        std::fs::write(dir.join("video.txt"), list)?;
    }
    progress(1.0);
    Ok(())
}

/// Genau `bytes` Bytes aus `src` anhängen (fehlende als Stille, überzählige weg), damit jeder
/// Abschnitt sample-genau an seiner Stelle liegt.
fn append_exact(src: &Path, dst: &mut impl std::io::Write, bytes: u64) -> Result<()> {
    let mut f = std::io::Read::take(std::fs::File::open(src)?, bytes);
    let n = std::io::copy(&mut f, dst)?;
    if n < bytes {
        std::io::copy(&mut std::io::Read::take(std::io::repeat(0), bytes - n), dst)?;
    }
    Ok(())
}

/// Ein Abschnitt: seine Shader-Vorläufe, dann FFmpeg (Bild nach `sec_k.*`, Ton nach `sec_k.f32`).
fn run_section(job: &Job, sec: &Section, cancel: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<()> {
    let secs = (sec.to - sec.from).as_secs_f64();
    let pre: usize = sec.passes.iter().map(|p| p.progress.len()).sum();
    let frames = secs * job.frames_per_sec();
    let share = if pre == 0 { 0.0 } else { (pre as f64 / (pre as f64 + frames.max(1.0))).min(0.5) as f32 };
    let failed = run_passes(job, &sec.passes, cancel, &mut |x| progress(x * share))?;
    if !failed.is_empty() {
        let file = job.workdir.path().join(&sec.graph);
        let g = failed.iter().fold(std::fs::read_to_string(&file)?, |g, p| g.replace(&p.block, &p.fallback));
        std::fs::write(&file, g)?;
    }
    // Ton ist viel schneller als Bild: kleiner Anteil am Fortschritt
    let weights: Vec<f32> = match sec.runs.len() {
        2 => vec![0.9, 0.1],
        _ => vec![1.0],
    };
    let mut at = share;
    for (args, w) in sec.runs.iter().zip(weights) {
        let w = w * (1.0 - share);
        run_ffmpeg(job, args, secs, cancel, &mut |x| progress(at + x * w)).map_err(|e| match e {
            ExportError::Cancelled => e,
            e => section_error(sec, e, args.clone()),
        })?;
        at += w;
    }
    Ok(())
}

/// Fehler eines Abschnitts mit Zeitbereich und dem gescheiterten Aufruf (für das Protokoll).
fn section_error(sec: &Section, e: ExportError, args: Vec<String>) -> ExportError {
    ExportError::Section { from: sections::clock(sec.from), to: sections::clock(sec.to), err: Box::new(e), args }
}

/// `.part` ans Ziel. Scheitert das (Ziel z. B. im Player geöffnet), bleibt die fertige Datei liegen.
fn finish(part: &Path, out: &Path) -> Result<()> {
    std::fs::rename(part, out).map_err(|e| {
        std::io::Error::new(e.kind(), format!("{e} – die fertige Datei liegt unter {}", part.display())).into()
    })
}

/// Beendet Kindprozesse, wenn der Vorlauf abbricht oder scheitert.
struct Children(Vec<Child>);

impl Drop for Children {
    fn drop(&mut self) {
        for c in &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Alle Vorläufe nacheinander; gibt die gescheiterten zurück (dort bleibt der `xfade`-Rückfall).
/// Fehler nur bei Abbruch.
fn run_passes<'a>(job: &Job, passes: &'a [graph::ShaderPass], cancel: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<Vec<&'a graph::ShaderPass>> {
    let total = passes.iter().map(|p| p.progress.len()).sum::<usize>().max(1);
    let mut done = 0;
    let mut failed = Vec::new();
    for p in passes {
        match run_pass(job, p, cancel, &mut |k| progress((done + k) as f32 / total as f32)) {
            Ok(true) => {}
            Ok(false) => failed.push(p),
            Err(ExportError::Cancelled) => return Err(ExportError::Cancelled),
            // kein Shader-Fehler darf den Export kosten
            Err(e) => {
                eprintln!("Shader-Übergang: {e}");
                failed.push(p);
            }
        }
        done += p.progress.len();
    }
    Ok(failed)
}

/// Ein Vorlauf: Bildpaare aus FFmpeg lesen, mit cl-gpu mischen, verlustfrei schreiben.
/// `Ok(false)`: nicht möglich (keine GPU, FFmpeg- oder Shader-Fehler).
fn run_pass(job: &Job, p: &graph::ShaderPass, cancel: &AtomicBool, progress: &mut dyn FnMut(usize)) -> Result<bool> {
    let Some(gpu) = cl_gpu::gpu() else {
        return Ok(false);
    };
    let dir = job.workdir.path();
    let log = |n: &str| std::fs::File::create(dir.join(n)).map(Stdio::from).unwrap_or_else(|_| Stdio::null());
    let n = p.progress.len();
    let mut kids = Children(Vec::new());
    let mut reader = cl_media::ffmpeg_command()?;
    reader
        .current_dir(dir)
        .args(["-v", "error", "-nostdin"])
        .args(&p.inputs)
        .args(["-/filter_complex", &p.graph_file, "-map", "[pout]", "-frames:v", &n.to_string()])
        // Bildrate nennen: FFmpeg 7 kennt sie nach `setpts` nicht mehr und nähme 25
        .args(["-r", &p.fps, "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(log(&format!("{}.read.log", p.graph_file)));
    kids.0.push(reader.spawn()?);
    let mut writer = cl_media::ffmpeg_command()?;
    writer
        .current_dir(dir)
        .args(["-v", "error", "-nostdin", "-y", "-f", "rawvideo", "-pix_fmt", "rgba"])
        .args(["-s", &format!("{}x{}", p.w, p.h), "-framerate", &p.fps, "-i", "-"])
        .args(["-c:v", "ffv1", "-level", "3", "-slices", "4", "-pix_fmt", "bgra", &p.out_file])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(log(&format!("{}.write.log", p.graph_file)));
    kids.0.push(writer.spawn()?);
    let mut src = kids.0[0].stdout.take().expect("piped");
    let mut dst = kids.0[1].stdin.take().expect("piped");
    let prm = {
        let mut a = [0.0; cl_gpu::PARAMS];
        for (x, v) in a.iter_mut().zip(&p.params) {
            *x = *v;
        }
        a
    };
    let row = p.w as usize * 4;
    let mut pair = vec![0u8; row * 2 * p.h as usize];
    let (mut a, mut b) = (Vec::with_capacity(row * p.h as usize), Vec::with_capacity(row * p.h as usize));
    for (k, t) in p.progress.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExportError::Cancelled);
        }
        if std::io::Read::read_exact(&mut src, &mut pair).is_err() {
            return Ok(false);
        }
        a.clear();
        b.clear();
        for line in pair.chunks_exact(row * 2) {
            a.extend_from_slice(&line[..row]);
            b.extend_from_slice(&line[row..]);
        }
        let fa = cl_gpu::Frame { rgba: &a, width: p.w, height: p.h };
        let fb = cl_gpu::Frame { rgba: &b, width: p.w, height: p.h };
        let img = match gpu.render(&p.shader, &prm, fa, fb, (p.w, p.h), *t) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("Shader-Übergang: {e}");
                return Ok(false);
            }
        };
        if std::io::Write::write_all(&mut dst, &img).is_err() {
            return Ok(false);
        }
        progress(k + 1);
    }
    drop(dst);
    Ok(kids.0[1].wait()?.success())
}

/// Dateiname des Live-Vorschaubilds im Arbeitsordner
const PREVIEW_FILE: &str = "preview.jpg";

/// Argumente ohne die Vorschau-Ausgabe; nimmt auch den Vorschau-Zweig aus dem Graphen.
/// `None`, wenn `args` keine Vorschau enthält (dann gibt es nichts zu wiederholen).
fn without_preview(job: &Job, args: &[String]) -> Option<Vec<String>> {
    // die Vorschau ist die letzte Ausgabe: alles ab ihrem `-map [pv]` fällt weg
    let i = args.iter().position(|a| a == "[pv]")?.checked_sub(1)?;
    let graph = args.iter().position(|a| a == "-/filter_complex").and_then(|k| args.get(k + 1))?;
    let file = job.workdir.path().join(graph);
    let g = std::fs::read_to_string(&file).ok()?;
    let g = g
        .replace("split[vmain][pv0]", "null[vmain]")
        .replace(&format!("[pv0]fps=2:eof_action=pass,scale={PREVIEW_W}:-2[pv]"), "")
        .replace(";\n;\n", ";\n");
    std::fs::write(&file, g).ok()?;
    Some(args[..i].to_vec())
}

/// Startet FFmpeg im Arbeitsordner des Jobs; gibt bei Erfolg die Fehlerausgabe (Log) zurück.
/// `secs` = erwartete Länge (Fortschritt).
fn run_ffmpeg(job: &Job, args: &[String], secs: f64, cancel: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<String> {
    let mut child: Child = cl_media::ffmpeg_command()?
        .args(args)
        .current_dir(job.workdir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = child.stderr.take().expect("piped");
    let err_thread = std::thread::spawn(move || {
        // Metadaten im Log können anders kodiert sein: Bytes lesen, nicht an UTF-8 scheitern
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut BufReader::new(stderr), &mut b);
        String::from_utf8_lossy(&b).into_owned()
    });
    let total = secs.max(0.001);
    for line in BufReader::new(child.stdout.take().expect("piped"))
        .lines()
        .map_while(|l| l.ok())
    {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ExportError::Cancelled);
        }
        if let Some(us) = line
            .strip_prefix("out_time_us=")
            .and_then(|v| v.trim().parse::<f64>().ok())
        {
            progress((us / 1e6 / total).clamp(0.0, 1.0) as f32);
        }
    }
    let status = child.wait()?;
    let err = err_thread.join().unwrap_or_default();
    if !status.success() {
        // Das Vorschaubild darf den Export nie kosten: lässt es sich nicht schreiben (z. B. von einem
        // Virenscanner gesperrt), läuft derselbe Export noch einmal ohne Vorschau.
        if err.contains(PREVIEW_FILE) {
            if let Some(plain) = without_preview(job, args) {
                return run_ffmpeg(job, &plain, secs, cancel, progress);
            }
        }
        return Err(ExportError::Ffmpeg(
            err.lines().rev().take(4).collect::<Vec<_>>().join(" | "),
        ));
    }
    progress(1.0);
    Ok(err)
}

/// Der Testlauf eines Hardware-Encoders scheiterte NUR am Treiber: Die Grafikkarte kann es, der
/// Treiber ist zu alt. Das meldet bisher allein NVENC eindeutig („Driver does not support the
/// required nvenc API version … minimum required Nvidia driver …"); fehlt die Karte ganz, heißt es
/// „No capable devices found" bzw. die Bibliothek lässt sich nicht laden – das zählt nicht.
pub fn driver_too_old(stderr: &str) -> bool {
    let l = stderr.to_ascii_lowercase();
    l.contains("driver does not support the required nvenc api version") || l.contains("minimum required nvidia driver")
}

static OLD_DRIVER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Hinweis „Grafiktreiber veraltet“ nach der letzten Erkennung (siehe [`driver_hint`])?
pub fn old_driver_found() -> bool {
    OLD_DRIVER.load(Ordering::Relaxed)
}

/// Hinweis „Grafiktreiber veraltet“: nur wenn NVENC allein am Treiber scheitert UND kein anderer
/// Hardware-Weg (QSV, AMF, Media Foundation) gefunden wurde – sonst kodiert die App ja in Hardware.
pub fn driver_hint(nvenc_driver_old: bool, found: &[Encoder]) -> bool {
    nvenc_driver_old && found.iter().all(|e| *e == Encoder::Software)
}

/// Testläufe der Hardware-Encoder: Encoder und Argumente (Media Foundation nur unter Windows,
/// mit `nv12` und erzwungener Hardware wie im Export).
fn encoder_probes() -> Vec<(Encoder, &'static [&'static str])> {
    let mut v: Vec<(Encoder, &'static [&'static str])> = vec![
        (Encoder::Nvenc, &["h264_nvenc"]),
        (Encoder::Qsv, &["h264_qsv"]),
        (Encoder::Amf, &["h264_amf"]),
    ];
    if cfg!(windows) {
        v.push((Encoder::Mf, &["h264_mf", "-pix_fmt", "nv12", "-hw_encoding", "1"]));
    }
    v
}

/// Prüft, welche Hardware-Encoder auf diesem Rechner wirklich funktionieren (kurzer Testlauf).
pub fn available_encoders() -> Vec<Encoder> {
    let mut v = vec![Encoder::Software];
    let mut old = false;
    for (enc, codec) in encoder_probes() {
        let ok = cl_media::ffmpeg_command()
            .ok()
            .and_then(|mut c| {
                c.args(["-v", "error", "-f", "lavfi", "-i", "color=black:s=256x256:d=0.1", "-c:v"])
                    .args(codec)
                    .args(["-f", "null", "-"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .output()
                    .ok()
            })
            .is_some_and(|o| {
                old |= enc == Encoder::Nvenc && !o.status.success() && driver_too_old(&String::from_utf8_lossy(&o.stderr));
                o.status.success()
            });
        if ok {
            v.push(enc);
        }
    }
    OLD_DRIVER.store(driver_hint(old, &v), Ordering::Relaxed);
    v
}

/// Ton der Zeitleiste ab `from` bis zum Projektende als f32le-Stereo mit `rate` Hz auf stdout.
pub fn spawn_audio_preview(
    p: &Project,
    from: Time,
    rate: u32,
) -> Result<(Child, tempfile::TempDir)> {
    spawn_audio_window(p, from, p.duration(), rate)
}

/// Ids der hörbaren Clips, die den Bereich `start..end` berühren (= Eingänge des Ton-Mixes).
pub fn audio_clips_in(p: &Project, start: Time, end: Time) -> Vec<ClipId> {
    graph::audible_clips(p, start, end).map(|(c, _)| c.id).collect()
}

/// Ton der Zeitleiste im Bereich `from..to` (höchstens bis zum Projektende) als f32le-Stereo mit
/// `rate` Hz auf stdout (Vorschau-Wiedergabe in Fenstern). Gemischt werden nur die Clips, die den
/// Bereich berühren; jeder Clip wird ab seinem Anfang gerechnet, darum klingt ein Abschnitt
/// sample-gleich wie derselbe Abschnitt eines längeren Mixes.
pub fn spawn_audio_window(
    p: &Project,
    from: Time,
    to: Time,
    rate: u32,
) -> Result<(Child, tempfile::TempDir)> {
    let p = &in_edit_ranges(p);
    let end = p.duration().min(to);
    if end <= from {
        return Err(ExportError::Empty);
    }
    let workdir = tempfile::Builder::new()
        .prefix("cutline-audio-")
        .tempdir()?;
    let (inputs, graph, label) = audio_window_graph(p, from, end)?;
    std::fs::write(workdir.path().join("graph.txt"), graph.join(";\n"))?;
    let child = cl_media::playback_command()?
        .args(["-v", "error", "-nostdin"])
        .args(&inputs)
        .args([
            "-/filter_complex",
            "graph.txt",
            "-map",
            &format!("[{label}]"),
        ])
        .args([
            "-t",
            &f((end - from).as_secs_f64()),
            "-f",
            "f32le",
            "-ar",
            &rate.to_string(),
            "-ac",
            "2",
            "-",
        ])
        .current_dir(workdir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    Ok((child, workdir))
}

/// Eingänge, Graph und Ausgang des Ton-Mixes für ein Vorschau-Fenster. Unlesbare Dateien fallen
/// einzeln heraus ([`audio_readable`]). Bei sehr vielen Clips (dichter Schnitt) teilen sich Clips
/// derselben Datei einen Eingang, damit Befehlszeile und Anlauf klein bleiben.
fn audio_window_graph(p: &Project, from: Time, end: Time) -> Result<(Vec<String>, Vec<String>, String)> {
    let mut o = graph::AudioOpts { wait_probe: false, limiter: true, lead: false, shared: false, skip_bad: true };
    loop {
        let (mut inputs, mut graph, mut n) = (Vec::new(), Vec::new(), 0);
        let label = graph::audio_part(p, from, end, &mut inputs, &mut n, &mut graph, o);
        let len = sections::cmd_len(&inputs) + BASE_CHARS;
        if (n <= sections::MAX_INPUTS && len <= sections::MAX_CMD) || (o.shared && len <= sections::HARD_CMD) {
            return Ok((inputs, graph, label));
        }
        if o.shared {
            return Err(ExportError::TooManyClips(sections::clock(from)));
        }
        o.shared = true;
    }
}

/// Lesbarkeit des Tons je Datei (Größe und Änderungszeit gehören zum Schlüssel: eine ersetzte
/// Datei wird neu geprüft). `None` = Prüfung läuft.
type Readable = std::collections::HashMap<(PathBuf, u64, Option<std::time::SystemTime>), Option<bool>>;
static READABLE: std::sync::Mutex<Option<Readable>> = std::sync::Mutex::new(None);
static AUDIO_WARNINGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Kann FFmpeg den Ton dieser Datei lesen? Geprüft wird einmal je Datei mit ffprobe; ohne `wait`
/// im Hintergrund (bis dahin gilt die Datei als lesbar, die Wiedergabe wartet nie). Eine
/// unlesbare Datei landet einmal in [`take_audio_warnings`].
pub(crate) fn audio_readable(path: &Path, wait: bool) -> bool {
    let meta = std::fs::metadata(path).ok();
    let key = (path.to_path_buf(), meta.as_ref().map_or(0, |m| m.len()), meta.and_then(|m| m.modified().ok()));
    {
        let mut g = READABLE.lock().unwrap_or_else(|e| e.into_inner());
        match g.get_or_insert_with(Default::default).get(&key) {
            Some(Some(ok)) => return *ok,
            Some(None) if !wait => return true,
            Some(None) => {}
            None => {
                g.get_or_insert_with(Default::default).insert(key.clone(), None);
            }
        }
    }
    let check = move || {
        let ok = cl_media::probe(&key.0).is_ok_and(|m| m.has_audio);
        if !ok {
            let name = key.0.file_name().map_or_else(|| key.0.display().to_string(), |n| n.to_string_lossy().into_owned());
            AUDIO_WARNINGS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(format!("Ton von {name} kann nicht gelesen werden – Wiedergabe ohne diese Datei"));
        }
        READABLE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(key, Some(ok));
        ok
    };
    if wait {
        check()
    } else {
        std::thread::spawn(check);
        true
    }
}

/// Warnungen der Vorschau-Wiedergabe seit dem letzten Aufruf (je unlesbarer Datei einmal), für
/// einen Hinweis im Player.
pub fn take_audio_warnings() -> Vec<String> {
    std::mem::take(&mut *AUDIO_WARNINGS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Textclips als SRT (z. B. für Untertitel-Export).
pub fn to_srt(p: &Project) -> String {
    to_srt_in(p, Time::ZERO, Time(i64::MAX))
}

/// Textclips als WebVTT.
pub fn to_vtt(p: &Project) -> String {
    to_vtt_in(p, Time::ZERO, Time(i64::MAX))
}

/// SRT nur für den Bereich `start..end` (Zeiten ab `start`, am Rand gekürzt).
pub fn to_srt_in(p: &Project, start: Time, end: Time) -> String {
    subtitles(p, ',', start, end)
}

/// WebVTT nur für den Bereich `start..end`.
pub fn to_vtt_in(p: &Project, start: Time, end: Time) -> String {
    format!("WEBVTT\n\n{}", subtitles(p, '.', start, end))
}

/// SRT oder VTT je nach Dateiendung.
pub fn subtitles_for(p: &Project, path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some(e) if e.eq_ignore_ascii_case("vtt") => to_vtt(p),
        _ => to_srt(p),
    }
}

fn subtitles(p: &Project, ms_sep: char, start: Time, end: Time) -> String {
    let vtt = ms_sep == '.';
    let ts = |t: Time| {
        let ms = t.as_millis().max(0);
        format!(
            "{:02}:{:02}:{:02}{ms_sep}{:03}",
            ms / 3_600_000,
            (ms / 60_000) % 60,
            (ms / 1000) % 60,
            ms % 1000
        )
    };
    let mut items: Vec<(Time, Time, String)> = p
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Text && !t.hidden)
        .flat_map(|t| t.clips.iter())
        .filter_map(|c| match &c.content {
            ClipContent::Text(s) if c.end() > start && c.start < end => {
                // Leerzeilen beenden die Cue; in VTT sind <, >, & Markup und „-->“ trennt Zeiten
                let mut t = s.text.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n");
                if vtt {
                    t = t.replace("-->", "->").replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
                }
                let t0 = c.start.max(start) - start;
                (!t.is_empty()).then(|| (t0, c.end().min(end) - start, t))
            }
            _ => None,
        })
        .collect();
    items.sort_by_key(|i| i.0);
    items
        .iter()
        .enumerate()
        .map(|(i, (a, b, t))| format!("{}\n{} --> {}\n{}\n", i + 1, ts(*a), ts(*b), t))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_chapters_start_with_the_running_marker() {
        let mk = |at: f64, name: &str| Marker { id: 0, at: Time::from_secs_f64(at), duration: Time::ZERO, name: name.into(), note: String::new(), color: 0 };
        let ms = [mk(0.0, "Intro"), mk(10.0, "Thema"), mk(24.93, "Highlights"), mk(35.13, "Interview"), mk(40.0, "Ende")];
        let s = |x: f64| Time::from_secs_f64(x);
        let v = chapter_list(&ms, s(34.13), s(36.13), "Kapitel");
        assert_eq!(v, vec![(Time::ZERO, "Highlights".to_string()), (s(35.13) - s(34.13), "Interview".to_string())]);
        // ganze Datei: wie bisher
        assert_eq!(chapter_list(&ms[1..], Time::ZERO, s(50.0), "Kapitel")[0].1, "Intro");
        // Marker ohne Namen in der Sprache der Oberfläche
        assert_eq!(chapter_list(&[mk(0.0, ""), mk(20.0, "B")], Time::ZERO, s(50.0), "Chapter")[0].1, "Chapter");
        // Kapiteldatei mit denselben Namen und Zeiten ab Bereichsanfang
        let meta = ffmetadata(&ms, s(34.13), s(36.13), "Kapitel").unwrap();
        assert!(ffmetadata(&[mk(0.0, ""), mk(20.0, "B")], Time::ZERO, s(50.0), "Chapter").unwrap().contains("title=Chapter\n"));
        assert!(meta.contains("START=0\nEND=1000\ntitle=Highlights\n") && meta.contains("START=1000\nEND=2000\ntitle=Interview\n"), "{meta}");
    }

    #[test]
    fn audio_window_takes_only_audible_clips_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let media = |name: &str, audio: bool, kind: MediaKind| {
            let path = dir.path().join(name);
            std::fs::write(&path, b"x").unwrap();
            MediaRef { id: MediaId::new(), path, kind, duration: Time::from_secs_f64(600.0), width: 0, height: 0, fps: None, has_audio: audio, audio_streams: audio as u32, video_codec: None, audio_codec: None }
        };
        let s = Time::from_secs_f64;
        let (snd, mute_vid, img) = (media("a.wav", true, MediaKind::Audio), media("v.mp4", false, MediaKind::Video), media("i.png", false, MediaKind::Image));
        let mut gone = media("weg.wav", true, MediaKind::Audio);
        gone.path = dir.path().join("fehlt.wav");
        let mut p = Project::new("T");
        p.media.extend([snd.clone(), mute_vid.clone(), img.clone(), gone.clone()]);
        let clip = |m: &MediaRef, a: f64, b: f64| Clip::new(ClipContent::Media { media: m.id, kind: m.kind }, s(a), s(b - a));
        let mut t = Track::new(TrackKind::Audio);
        let before = clip(&snd, 0.0, 10.0); // endet genau am Fensteranfang: nicht dabei
        let across_start = clip(&snd, 5.0, 12.0);
        let inside = clip(&snd, 12.0, 14.0);
        let mut disabled = clip(&snd, 12.0, 14.0);
        disabled.disabled = true;
        let mut silent = clip(&snd, 14.0, 15.0);
        silent.volume_db = -60.0;
        let mut fading_in = clip(&snd, 15.0, 16.0);
        fading_in.volume_db = -60.0;
        fading_in.keyframes.set(Prop::Volume, Time::ZERO, -60.0);
        let across_end = clip(&snd, 25.0, 40.0);
        let after = clip(&snd, 30.0, 31.0); // beginnt genau am Fensterende: nicht dabei
        let missing = clip(&gone, 12.0, 13.0);
        for c in [&before, &across_start, &inside, &disabled, &silent, &fading_in, &across_end, &after, &missing] {
            t.clips.push(c.clone());
        }
        let mut v = Track::new(TrackKind::Video);
        let no_audio = clip(&mute_vid, 10.0, 20.0);
        let image = clip(&img, 10.0, 20.0);
        v.clips.extend([no_audio, image]);
        let mut muted = Track::new(TrackKind::Audio);
        muted.muted = true;
        muted.clips.push(clip(&snd, 10.0, 20.0));
        p.timeline.tracks.extend([t, v, muted]);
        assert_eq!(audio_clips_in(&p, s(10.0), s(30.0)), vec![across_start.id, inside.id, fading_in.id, across_end.id]);
        assert_eq!(audio_clips_in(&p, s(0.0), s(5.0)), vec![before.id]);
        assert!(audio_clips_in(&p, s(41.0), s(60.0)).is_empty());
    }

    #[test]
    fn missing_media_only_blocks_clips_in_the_range() {
        let mut p = Project::new("T");
        let gone = |name: &str| MediaRef {
            id: MediaId::new(),
            path: PathBuf::from(format!("C:/weg/{name}")),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(60.0),
            width: 320,
            height: 180,
            fps: None,
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
        };
        let mut t = Track::new(TrackKind::Video);
        for (i, n) in ["a.mp4", "b.mp4", "c.mp4"].iter().enumerate() {
            let m = gone(n);
            t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::from_secs_f64(i as f64 * 10.0), Time::from_secs_f64(10.0)));
            p.media.push(m);
        }
        p.timeline.tracks.push(t);
        let dir = tempfile::tempdir().unwrap();
        let mut s = ExportSettings::from_project(&p);
        let none = |_: Option<&str>| None;
        // alle drei fehlen: alle genannt
        match build(&p, &s, &none, &dir.path().join("o.mp4")) {
            Err(ExportError::MissingMedia(v)) => assert_eq!(v, ["a.mp4", "b.mp4", "c.mp4"]),
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("Export trotz fehlender Medien"),
        }
        // Bereich nur im zweiten Clip: nur dessen Datei zählt
        s.range = Some((Time::from_secs_f64(12.0), Time::from_secs_f64(15.0)));
        assert!(matches!(build(&p, &s, &none, &dir.path().join("o.mp4")), Err(ExportError::MissingMedia(v)) if v == ["b.mp4"]));
    }

    #[test]
    fn text_clips_share_one_font_copy() {
        let dir = tempfile::tempdir().unwrap();
        let font = dir.path().join("schrift.ttf");
        std::fs::write(&font, b"keine echte Schrift").unwrap();
        let mut p = Project::new("T");
        let mut t = Track::new(TrackKind::Text);
        for i in 0..50 {
            t.clips.push(Clip::new(
                ClipContent::Text(TextStyle { text: format!("Zeile {i}"), ..Default::default() }),
                Time::from_secs_f64(i as f64),
                Time::from_secs_f64(1.0),
            ));
        }
        p.timeline.tracks.push(t);
        let f = font.clone();
        let resolver = move |_: Option<&str>| Some((f.clone(), 0));
        let job = build(&p, &ExportSettings::from_project(&p), &resolver, &dir.path().join("o.mp4")).unwrap();
        let copies = std::fs::read_dir(job.workdir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("font_"))
            .count();
        assert_eq!(copies, 1);
    }

    #[test]
    fn finished_part_survives_failed_rename() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("a.mp4.part");
        std::fs::write(&part, b"fertig").unwrap();
        // Ziel ist ein Ordner: Umbenennen scheitert wie bei einer gesperrten Datei
        let out = dir.path().join("a.mp4");
        std::fs::create_dir(&out).unwrap();
        let e = finish(&part, &out).unwrap_err().to_string();
        assert!(part.is_file());
        assert!(e.contains("a.mp4.part"), "{e}");
    }

    fn make_media(dir: &Path) -> (MediaRef, MediaRef) {
        let v = dir.join("v.mp4");
        let img = dir.join("i.png");
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=30:duration=3",
                "-f",
                "lavfi",
                "-i",
                "sine=f=440:d=3",
            ])
            .args([
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&v)
            .status()
            .unwrap();
        assert!(st.success());
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=orange:size=200x200",
                "-frames:v",
                "1",
            ])
            .arg(&img)
            .status()
            .unwrap();
        assert!(st.success());
        (cl_media::probe(&v).unwrap(), cl_media::probe(&img).unwrap())
    }

    fn project(dir: &Path) -> Project {
        let (v, i) = make_media(dir);
        let mut d = Document::new(Project::new("Export-Test"));
        let mut s = d.project().settings.clone();
        s.width = 320;
        s.height = 568; // 9:16
        d.apply(EditOp::SetSettings { settings: s }, OpSource::Ui)
            .unwrap();
        d.apply(EditOp::ImportMedia { media: v.clone() }, OpSource::Ui)
            .unwrap();
        d.apply(EditOp::ImportMedia { media: i.clone() }, OpSource::Ui)
            .unwrap();
        let vc = Clip::new(
            ClipContent::Media {
                media: v.id,
                kind: MediaKind::Video,
            },
            Time::ZERO,
            v.duration,
        );
        let OpOutput::Clip(vid) = d
            .apply(
                EditOp::AddClip {
                    track: None,
                    clip: vc,
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        d.apply(
            EditOp::SplitClip {
                clip: vid,
                at: Time::from_secs_f64(1.0),
            },
            OpSource::Ui,
        )
        .unwrap();
        let ic = Clip::new(
            ClipContent::Media {
                media: i.id,
                kind: MediaKind::Image,
            },
            Time::from_secs_f64(3.0),
            Time::from_secs_f64(1.0),
        );
        d.apply(
            EditOp::AddClip {
                track: None,
                clip: ic,
            },
            OpSource::Ui,
        )
        .unwrap();
        let mut style = TextStyle {
            text: "Export mit Umlauten: ÄÖÜ und langem Text der umbrechen muss".into(),
            size: 40.0,
            ..Default::default()
        };
        style.uppercase = true;
        let tc = Clip::new(
            ClipContent::Text(style.clone()),
            Time::from_secs_f64(0.5),
            Time::from_secs_f64(2.0),
        );
        d.apply(
            EditOp::AddClip {
                track: None,
                clip: tc,
            },
            OpSource::Ui,
        )
        .unwrap();
        // gedrehter Text + gedrehtes Video (Player-Bearbeitung)
        let mut rc = Clip::new(
            ClipContent::Text(TextStyle {
                text: "Gedreht".into(),
                ..style
            }),
            Time::from_secs_f64(1.0),
            Time::from_secs_f64(1.0),
        );
        rc.transform = Transform {
            x: 40.0,
            y: -60.0,
            scale: 1.2,
            rotation_deg: -30.0,
        };
        d.apply(
            EditOp::AddClip {
                track: None,
                clip: rc,
            },
            OpSource::Ui,
        )
        .unwrap();
        let first = d
            .project()
            .timeline
            .tracks
            .iter()
            .find(|t| t.main)
            .unwrap()
            .clips[0]
            .id;
        d.apply(
            EditOp::SetTransform {
                clip: first,
                transform: Transform {
                    x: 0.0,
                    y: 0.0,
                    scale: 0.8,
                    rotation_deg: 15.0,
                },
            },
            OpSource::Ui,
        )
        .unwrap();
        d.project().clone()
    }

    fn no_fonts(_: Option<&str>) -> Option<(PathBuf, u32)> {
        None
    }

    #[test]
    fn effect_and_filter_elements_apply_to_layers_below() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = project(dir.path());
        let mut fx = Track::new(TrackKind::Effect);
        fx.clips.push(Clip::new(
            ClipContent::Effect(FxRef::new("shake")),
            Time::from_secs_f64(0.5),
            Time::from_secs_f64(1.0),
        ));
        p.timeline.tracks.push(fx);
        let mut fl = Track::new(TrackKind::Filter);
        fl.clips.push(Clip::new(
            ClipContent::Filter(FxRef::new("noir")),
            Time::from_secs_f64(1.0),
            Time::from_secs_f64(2.0),
        ));
        p.timeline.tracks.push(fl);
        let s = ExportSettings::from_project(&p);
        let out = dir.path().join("fx.mp4");
        let job = build(&p, &s, &no_fonts, &out).unwrap();
        run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let m = cl_media::probe(&out).unwrap();
        assert!((m.duration.as_secs_f64() - 4.0).abs() < 0.15);
        // Filter-Element (Schwarzweiß) wirkt nur in seinem Zeitraum
        let gray = cl_media::decode_frame(&out, Time::from_secs_f64(2.0), 32, 18).unwrap();
        let color = cl_media::decode_frame(&out, Time::from_secs_f64(0.2), 32, 18).unwrap();
        let sat = |f: &cl_media::RgbaFrame| {
            f.data
                .chunks(4)
                .map(|px| (px[0] as i32 - px[1] as i32).abs() + (px[1] as i32 - px[2] as i32).abs())
                .sum::<i32>()
        };
        assert!(sat(&gray) < sat(&color), "Filter wirkt ab 1 s");
    }

    #[test]
    fn effect_element_mask_and_strength_keyframes() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = project(dir.path());
        // Schwarzweiß nur in der linken Bildhälfte
        let mut fl = Track::new(TrackKind::Filter);
        let mut c = Clip::new(
            ClipContent::Filter(FxRef::new("noir")),
            Time::ZERO,
            Time::from_secs_f64(2.0),
        );
        c.fx.mask = Some(Mask {
            cx: 0.25,
            cy: 0.5,
            w: 0.5,
            h: 1.2,
            feather: 0.0,
            ..Mask::new(MaskShape::Rect)
        });
        fl.clips.push(c);
        p.timeline.tracks.push(fl);
        // Zweites Element ab 2 s: Stärke 0 → 1 über eine Sekunde
        let mut fl2 = Track::new(TrackKind::Filter);
        let mut c2 = Clip::new(
            ClipContent::Filter(FxRef::new("noir")),
            Time::from_secs_f64(2.0),
            Time::from_secs_f64(2.0),
        );
        c2.keyframes.set(Prop::Strength, Time::ZERO, 0.0);
        c2.keyframes
            .set(Prop::Strength, Time::from_secs_f64(1.0), 1.0);
        fl2.clips.push(c2);
        p.timeline.tracks.push(fl2);
        let s = ExportSettings::from_project(&p);
        let out = dir.path().join("mask.mp4");
        run(
            &build(&p, &s, &no_fonts, &out).unwrap(),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        let sat = |f: &cl_media::RgbaFrame, x0: u32, x1: u32| {
            let mut sum = 0i64;
            for y in 0..f.height {
                for x in x0..x1 {
                    let i = ((y * f.width + x) * 4) as usize;
                    let px = &f.data[i..i + 4];
                    sum += ((px[0] as i32 - px[1] as i32).abs()
                        + (px[1] as i32 - px[2] as i32).abs()) as i64;
                }
            }
            sum
        };
        let f = cl_media::decode_frame(&out, Time::from_secs_f64(1.0), 32, 56).unwrap();
        assert!(
            sat(&f, 0, 12) * 3 < sat(&f, 20, 32),
            "links grau, rechts farbig"
        );
        let early = cl_media::decode_frame(&out, Time::from_secs_f64(2.05), 32, 56).unwrap();
        let late = cl_media::decode_frame(&out, Time::from_secs_f64(3.5), 32, 56).unwrap();
        assert!(
            sat(&late, 0, 32) * 3 < sat(&early, 0, 32),
            "Stärke steigt an"
        );
    }

    /// Export einer echten Projektdatei: `CUTLINE_EXPORT_PROJECT=<pfad> cargo test -p cl-export export_project_file -- --ignored`
    #[test]
    #[ignore]
    fn export_project_file() {
        let Some(path) = std::env::var_os("CUTLINE_EXPORT_PROJECT") else {
            return;
        };
        let p = Project::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let s = ExportSettings::from_project(&p);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.mp4");
        let job = build(&p, &s, &no_fonts, &out).unwrap();
        
        let t = std::time::Instant::now();
        run(&job, Arc::new(AtomicBool::new(false)), |x| eprintln!("{:.1}s {x:.3}", t.elapsed().as_secs_f32())).unwrap();
        assert!(cl_media::probe(&out).is_ok());
    }

    /// Mikrofon nur links aufgenommen: im Export auf beiden Kanälen (Standard „Automatisch“),
    /// mit „Stereo“ bleibt der rechte Kanal still.
    #[test]
    fn one_sided_audio_is_spread_to_both_channels() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("links.wav");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=440:d=2", "-af", "pan=stereo|c0=c0|c1=0*c0"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        let m = cl_media::probe(&src).unwrap();
        let peaks = |p: &Project| -> Vec<f64> {
            let out = dir.path().join("out.wav");
            let mut s = ExportSettings::from_project(p);
            s.format = Format::Wav;
            run(&build(p, &s, &no_fonts, &out).unwrap(), Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
            let o = cl_media::ffmpeg_command()
                .unwrap()
                .args(["-v", "info", "-nostdin", "-i"])
                .arg(&out)
                .args(["-af", "astats=measure_overall=none:measure_perchannel=Peak_level", "-f", "null", "-"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stderr)
                .lines()
                .filter_map(|l| l.split("Peak level dB:").nth(1))
                .map(|v| v.trim().parse::<f64>().unwrap_or(f64::NEG_INFINITY))
                .collect()
        };
        let mut p = Project::new("Kanäle");
        p.media.push(m.clone());
        let mut t = Track::new(TrackKind::Audio);
        t.clips.push(Clip::new(
            ClipContent::Media { media: m.id, kind: MediaKind::Audio },
            Time::ZERO,
            Time::from_secs_f64(2.0),
        ));
        p.timeline.tracks.push(t);
        let auto = peaks(&p);
        assert_eq!(auto.len(), 2, "{auto:?}");
        assert!(auto[0] > -20.0 && auto[1] > -20.0, "beide Kanäle hörbar: {auto:?}");
        p.settings.channels = Channels::Stereo;
        let stereo = peaks(&p);
        assert!(stereo[1] < -60.0, "rechts still bei Stereo: {stereo:?}");
    }

    #[test]
    fn exports_mp4_gif_and_audio() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = project(dir.path());
        assert_eq!(p.duration(), Time::from_secs_f64(4.0)); // 3 s Video + 1 s Bild (Magnet)
        let mk = |id, at: f64, name: &str| cl_core::Marker {
            id,
            at: Time::from_secs_f64(at),
            duration: Time::ZERO,
            name: name.into(),
            note: String::new(),
            color: 0,
        };
        p.timeline.markers = vec![mk(1, 1.0, "Teil = 1"), mk(2, 2.5, "Ende")];

        let mut s = ExportSettings::from_project(&p);
        let out = dir.path().join("out.mp4");
        let job = build(&p, &s, &no_fonts, &out).unwrap();
        let mut last = 0.0;
        run(&job, Arc::new(AtomicBool::new(false)), |x| last = x).unwrap();
        assert_eq!(last, 1.0);
        let m = cl_media::probe(&out).unwrap();
        assert_eq!((m.width, m.height), (320, 568));
        assert!(
            (m.duration.as_secs_f64() - 4.0).abs() < 0.15,
            "{}",
            m.duration
        );
        assert!(m.has_audio);
        // Marker landen als Kapitel in der Datei (mit „Intro“ ab 0:00)
        let ch = cl_media::ffprobe_command()
            .unwrap()
            .args(["-v", "error", "-show_chapters", "-of", "csv=p=0"])
            .arg(&out)
            .output()
            .unwrap();
        let ch = String::from_utf8_lossy(&ch.stdout);
        let titles: Vec<&str> = ch.lines().filter_map(|l| l.rsplit(',').next()).collect();
        assert_eq!(titles, ["Intro", "Teil = 1", "Ende"], "{ch}");
        // ausgeschaltet: keine Kapitel in der Datei
        let off = build(&p, &ExportSettings { chapters: false, ..s.clone() }, &no_fonts, &out).unwrap();
        assert!(!off.args.iter().any(|a| a == "chapters.txt"));

        s.format = Format::Gif;
        let gif = dir.path().join("out.gif");
        run(
            &build(&p, &s, &no_fonts, &gif).unwrap(),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        assert!(std::fs::metadata(&gif).unwrap().len() > 1000);

        s.format = Format::Wav;
        let wav = dir.path().join("out.wav");
        run(
            &build(&p, &s, &no_fonts, &wav).unwrap(),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        let w = cl_media::probe(&wav).unwrap();
        assert_eq!(w.kind, MediaKind::Audio);
        assert!((w.duration.as_secs_f64() - 4.0).abs() < 0.1);

        // Bereichsexport
        s.format = Format::Mp4;
        s.range = Some((Time::from_secs_f64(1.0), Time::from_secs_f64(2.5)));
        let part = dir.path().join("part.mp4");
        run(
            &build(&p, &s, &no_fonts, &part).unwrap(),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        assert!((cl_media::probe(&part).unwrap().duration.as_secs_f64() - 1.5).abs() < 0.15);
    }

    /// Live-Vorschau: zweite Ausgabe schreibt ein kleines JPEG, die Hauptdatei bleibt wie bisher.
    #[test]
    fn export_writes_live_preview_without_changing_output() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = project(dir.path());
        let mut s = ExportSettings::from_project(&p);
        s.range = Some((Time::ZERO, Time::from_secs_f64(2.0)));
        let out = dir.path().join("live.mp4");
        let job = build(&p, &s, &no_fonts, &out).unwrap();
        let pv = job.preview_path().unwrap().to_path_buf();
        let mut seen = Vec::new();
        run(&job, Arc::new(AtomicBool::new(false)), |x| seen.push(x)).unwrap();
        assert!(seen.windows(2).all(|w| w[0] <= w[1]) && seen.last() == Some(&1.0), "{seen:?}");
        let m = cl_media::probe(&out).unwrap();
        assert!((m.duration.as_secs_f64() - 2.0).abs() < 0.15, "{}", m.duration);
        assert_eq!((m.width, m.height), (320, 568));
        let streams = cl_media::ffprobe_command()
            .unwrap()
            .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
            .arg(&out)
            .output()
            .unwrap();
        let mut kinds: Vec<String> = String::from_utf8_lossy(&streams.stdout).lines().map(|l| l.trim().to_string()).collect();
        kinds.sort();
        assert_eq!(kinds, ["audio", "video"]);
        let img = cl_media::probe(&pv).unwrap();
        assert_eq!(img.width, PREVIEW_W);
        // mit Lautheits-Messlauf: zweiter Durchgang schreibt den Graph samt Vorschau-Zweig neu
        s.loudness = Some(-14.0);
        let job = build(&p, &s, &no_fonts, &out).unwrap();
        let mut seen = Vec::new();
        run(&job, Arc::new(AtomicBool::new(false)), |x| seen.push(x)).unwrap();
        assert!(seen.windows(2).all(|w| w[0] <= w[1]) && seen.last() == Some(&1.0), "{seen:?}");
        assert!(job.preview_path().unwrap().is_file());
        assert!((cl_media::probe(&out).unwrap().duration.as_secs_f64() - 2.0).abs() < 0.15);
        // Ton-Export: keine Vorschau
        s.format = Format::Wav;
        let wav = build(&p, &s, &no_fonts, &dir.path().join("live.wav")).unwrap();
        assert!(wav.preview_path().is_none() && !wav.args.iter().any(|a| a == "preview.jpg"));
        run(&wav, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(!wav.workdir.path().join("preview.jpg").exists());
    }

    /// Lässt sich das Vorschaubild nicht schreiben, läuft der Export ohne Vorschau durch.
    #[test]
    fn blocked_preview_does_not_fail_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = project(dir.path());
        let mut s = ExportSettings::from_project(&p);
        s.range = Some((Time::ZERO, Time::from_secs_f64(2.0)));
        for loudness in [None, Some(-14.0)] {
            s.loudness = loudness;
            let out = dir.path().join("blocked.mp4");
            let job = build(&p, &s, &no_fonts, &out).unwrap();
            // ein Ordner unter dem Namen der Vorschau: FFmpeg kann die Datei nicht anlegen
            std::fs::create_dir(job.preview_path().unwrap()).unwrap();
            let mut seen = Vec::new();
            run(&job, Arc::new(AtomicBool::new(false)), |x| seen.push(x)).unwrap();
            assert_eq!(seen.last(), Some(&1.0));
            assert!((cl_media::probe(&out).unwrap().duration.as_secs_f64() - 2.0).abs() < 0.15);
            assert!(job.preview_path().unwrap().is_dir(), "Vorschau wurde nicht geschrieben");
            std::fs::remove_file(&out).unwrap();
        }
        // ohne Vorschau in den Argumenten gibt es nichts zu wiederholen
        let job = build(&p, &s, &no_fonts, &dir.path().join("x.mp4")).unwrap();
        let plain = without_preview(&job, &job.args).unwrap();
        assert!(!plain.iter().any(|a| a == PREVIEW_FILE) && without_preview(&job, &plain).is_none());
    }

    /// Exportdauer mit/ohne Vorschau: `cargo test --release -p cl-export preview_overhead -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn preview_overhead() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.mp4");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30:duration=10", "-f", "lavfi", "-i", "sine=d=10"])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        let m = cl_media::probe(&src).unwrap();
        let mut p = Project::new("Messung");
        p.settings.width = 1920;
        p.settings.height = 1080;
        p.media.push(m.clone());
        let mut t = Track::new(TrackKind::Video);
        t.main = true;
        t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, m.duration));
        p.timeline.tracks.push(t);
        let s = ExportSettings::from_project(&p);
        for round in 0..2 {
            for with in [true, false] {
                let job = build(&p, &s, &no_fonts, &dir.path().join("o.mp4")).unwrap();
                let mut job = job;
                if !with {
                    job.args = without_preview(&job, &job.args).unwrap();
                }
                let t0 = std::time::Instant::now();
                run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
                eprintln!("Runde {round}, Vorschau {with}: {:.2} s", t0.elapsed().as_secs_f64());
            }
        }
    }

    #[test]
    fn exports_phase2_features() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (v, img) = make_media(dir.path());
        // Audio-Datei (1 s), die auf 2,5 s verlängert wird
        let wav = dir.path().join("a.wav");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=300:d=1"])
            .arg(&wav)
            .status()
            .unwrap()
            .success());
        let a = cl_media::probe(&wav).unwrap();
        let mut d = Document::new(Project::new("Phase2"));
        let mut s = d.project().settings.clone();
        s.width = 320;
        s.height = 180;
        d.apply(EditOp::SetSettings { settings: s }, OpSource::Ui)
            .unwrap();
        for m in [&v, &img, &a] {
            d.apply(EditOp::ImportMedia { media: m.clone() }, OpSource::Ui)
                .unwrap();
        }
        let OpOutput::Clip(c1) = d
            .apply(
                EditOp::AddClip {
                    track: None,
                    clip: Clip::new(
                        ClipContent::Media {
                            media: v.id,
                            kind: MediaKind::Video,
                        },
                        Time::ZERO,
                        v.duration,
                    ),
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        let OpOutput::Clip(c2) = d
            .apply(
                EditOp::SplitClip {
                    clip: c1,
                    at: Time::from_secs_f64(1.5),
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        // Übergang, Effekte, Filter, Maske, Speed, Rückwärts
        d.apply(
            EditOp::SetTransition {
                clip: c1,
                transition: Some(TransitionRef {
                    id: "circleopen".into(),
                    duration: Time::from_secs_f64(0.5),
                }),
            },
            OpSource::Ui,
        )
        .unwrap();
        let mut fx = ClipFx::default();
        fx.effects.push(FxRef::new("vhs"));
        fx.filter = Some(FxRef {
            id: "teal-orange".into(),
            intensity: 0.7,
            params: Default::default(),
        });
        fx.adjust.brightness = 0.2;
        d.apply(EditOp::SetFx { clip: c1, fx }, OpSource::Ui)
            .unwrap();
        let mut fx2 = ClipFx::default();
        fx2.mask = Some(Mask::new(MaskShape::Circle));
        fx2.reverse = true;
        d.apply(EditOp::SetFx { clip: c2, fx: fx2 }, OpSource::Ui)
            .unwrap();
        d.apply(
            EditOp::SetSpeed {
                clip: c2,
                speed: 1.5,
            },
            OpSource::Ui,
        )
        .unwrap();
        // Bild mit Keyframes und Animation
        let OpOutput::Clip(ic) = d
            .apply(
                EditOp::AddClip {
                    track: None,
                    clip: Clip::new(
                        ClipContent::Media {
                            media: img.id,
                            kind: MediaKind::Image,
                        },
                        Time::from_secs_f64(0.2),
                        Time::from_secs_f64(1.5),
                    ),
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        let tid = d.project().find_clip(ic).map(|(t, _)| t.id).unwrap();
        let mut k = Keyframes::default();
        k.set(Prop::X, Time::ZERO, -80.0);
        k.set(Prop::X, Time::from_secs_f64(1.0), 80.0);
        k.set(Prop::Opacity, Time::ZERO, 0.2);
        k.set(Prop::Opacity, Time::from_secs_f64(1.0), 1.0);
        k.set(Prop::Scale, Time::ZERO, 0.3);
        k.set(Prop::Scale, Time::from_secs_f64(1.2), 0.6);
        k.set(Prop::Rotation, Time::ZERO, 0.0);
        k.set(Prop::Rotation, Time::from_secs_f64(1.2), 90.0);
        d.apply(
            EditOp::SetKeyframes {
                clip: ic,
                keyframes: k,
            },
            OpSource::Ui,
        )
        .unwrap();
        let _ = tid;
        // Text mit Kontur, Fläche, Einblend-Animation
        let st = TextStyle {
            text: "Hallo".into(),
            size: 40.0,
            outline: Some((3.0, [0, 0, 0, 255])),
            background: Some([200, 0, 0, 180]),
            align: -1,
            ..Default::default()
        };
        let OpOutput::Clip(tc) = d
            .apply(
                EditOp::AddClip {
                    track: None,
                    clip: Clip::new(
                        ClipContent::Text(st),
                        Time::from_secs_f64(0.5),
                        Time::from_secs_f64(1.5),
                    ),
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        d.apply(
            EditOp::SetAnimations {
                clip: tc,
                anim: Animations {
                    intro: Some(AnimRef {
                        id: "slide-left".into(),
                        duration: Time::from_secs_f64(0.5),
                    }),
                    outro: Some(AnimRef {
                        id: "fade".into(),
                        duration: Time::from_secs_f64(0.3),
                    }),
                    combo: None,
                },
            },
            OpSource::Ui,
        )
        .unwrap();
        // Audio verlängert + Lautstärke-Keyframes + Fades
        let OpOutput::Clip(ac) = d
            .apply(
                EditOp::AddClip {
                    track: None,
                    clip: Clip::new(
                        ClipContent::Media {
                            media: a.id,
                            kind: MediaKind::Audio,
                        },
                        Time::from_secs_f64(0.3),
                        a.duration,
                    ),
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        d.apply(
            EditOp::TrimClip {
                clip: ac,
                edge: Edge::End,
                to: Time::from_secs_f64(2.8),
            },
            OpSource::Ui,
        )
        .unwrap();
        d.apply(
            EditOp::SetFades {
                clip: ac,
                fade_in: Time::from_secs_f64(0.2),
                fade_out: Time::from_secs_f64(0.2),
            },
            OpSource::Ui,
        )
        .unwrap();
        let mut kv = Keyframes::default();
        kv.set(Prop::Volume, Time::ZERO, -20.0);
        kv.set(Prop::Volume, Time::from_secs_f64(1.0), 0.0);
        d.apply(
            EditOp::SetKeyframes {
                clip: ac,
                keyframes: kv,
            },
            OpSource::Ui,
        )
        .unwrap();
        let p = d.project().clone();
        let total = p.duration().as_secs_f64();

        let s = ExportSettings::from_project(&p);
        let out = dir.path().join("p2.mp4");
        let job = build(&p, &s, &no_fonts, &out).unwrap();
        run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_or_else(|e| {
            panic!(
                "{e}\n{}",
                std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap()
            )
        });
        let m = cl_media::probe(&out).unwrap();
        assert!(
            (m.duration.as_secs_f64() - total).abs() < 0.15,
            "{} vs {total}",
            m.duration
        );
        // Bild in der Mitte des Übergangs unterscheidet sich von beiden Nachbarn nicht schwarz
        let f = cl_media::decode_frame(&out, Time::from_secs_f64(1.5), 64, 36).unwrap();
        assert!(f
            .data
            .chunks(4)
            .any(|px| px[0] > 30 || px[1] > 30 || px[2] > 30));

        // Teilbereich mit Animationen
        let mut s2 = s.clone();
        s2.range = Some((Time::from_secs_f64(0.7), Time::from_secs_f64(2.0)));
        let part = dir.path().join("p2part.mp4");
        run(
            &build(&p, &s2, &no_fonts, &part).unwrap(),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        assert!((cl_media::probe(&part).unwrap().duration.as_secs_f64() - 1.3).abs() < 0.15);
    }

    #[test]
    fn exports_phase3_features() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (v, _) = make_media(dir.path());
        // Greenscreen-Clip und ein Maskenvideo (linke Hälfte sichtbar)
        let green = dir.path().join("green.mp4");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=0x00b140:s=320x180:r=30:d=3",
                "-vf",
                "drawbox=x=100:y=40:w=120:h=100:color=red:t=fill",
                "-pix_fmt",
                "yuv420p"
            ])
            .arg(&green)
            .status()
            .unwrap()
            .success());
        let matte = dir.path().join("matte.mkv");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=black:s=320x180:r=30:d=3",
                "-vf",
                "drawbox=x=0:y=0:w=160:h=180:color=white:t=fill,format=gray",
                "-c:v",
                "ffv1"
            ])
            .arg(&matte)
            .status()
            .unwrap()
            .success());
        let g = cl_media::probe(&green).unwrap();
        let mut d = Document::new(Project::new("Phase3"));
        let mut s = d.project().settings.clone();
        s.width = 320;
        s.height = 180;
        d.apply(EditOp::SetSettings { settings: s }, OpSource::Ui)
            .unwrap();
        d.apply(EditOp::ImportMedia { media: v.clone() }, OpSource::Ui)
            .unwrap();
        d.apply(EditOp::ImportMedia { media: g.clone() }, OpSource::Ui)
            .unwrap();
        let OpOutput::Clip(base) = d
            .apply(
                EditOp::AddClip {
                    track: None,
                    clip: Clip::new(
                        ClipContent::Media {
                            media: v.id,
                            kind: MediaKind::Video,
                        },
                        Time::ZERO,
                        v.duration,
                    ),
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        let mut over = Clip::new(
            ClipContent::Media {
                media: g.id,
                kind: MediaKind::Video,
            },
            Time::ZERO,
            g.duration,
        );
        over.fx.chroma = Some(ChromaKey::default());
        over.fx.stabilize = 0.5;
        let oid = over.id;
        // Overlay landet auf eigener Spur über der Hauptspur
        let main = d.project().timeline.tracks[0].id;
        d.apply(
            EditOp::InsertTrack {
                index: 1,
                kind: TrackKind::Video,
            },
            OpSource::Ui,
        )
        .unwrap();
        let t2 = d
            .project()
            .timeline
            .tracks
            .iter()
            .find(|t| t.id != main)
            .unwrap()
            .id;
        d.apply(
            EditOp::AddClip {
                track: Some(t2),
                clip: over,
            },
            OpSource::Ui,
        )
        .unwrap();
        // Hauptclip: Maske, Ton-Werkzeuge, Stimmfilter
        let mut c = d.project().find_clip(base).unwrap().1.clone();
        c.fx.cutout = Some(matte.to_string_lossy().into_owned());
        c.audio = AudioFx {
            denoise: 0.5,
            voice_isolate: true,
            normalize: true,
            voice: Some("robot".into()),
            bass: 3.0,
            treble: 2.0,
            hum: true,
            compress: 0.4,
            filter: Some("warm".into()),
            ..Default::default()
        };
        d.apply(EditOp::ReplaceClip { clip: c }, OpSource::Ui)
            .unwrap();
        let p = d.project().clone();
        let out = dir.path().join("p3.mp4");
        let job = build(&p, &ExportSettings::from_project(&p), &no_fonts, &out).unwrap();
        run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_or_else(|e| {
            panic!(
                "{e}\n{}",
                std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap()
            )
        });
        let m = cl_media::probe(&out).unwrap();
        assert!(m.has_audio);
        let f = cl_media::decode_frame(&out, Time::from_secs_f64(1.0), 320, 180).unwrap();
        let px = |x: usize, y: usize| &f.data[(y * 320 + x) * 4..(y * 320 + x) * 4 + 3];
        // rechte Hälfte: Maske schwarz → Hauptclip weg → schwarzer Grund; Grün ist freigestellt
        let r = px(300, 10);
        assert!(
            r[0] < 40 && r[1] < 40 && r[2] < 40,
            "rechts oben sollte schwarz sein: {r:?}"
        );
        // rotes Rechteck des Greenscreen-Clips bleibt sichtbar
        let c = px(160, 90);
        assert!(c[0] > 150 && c[1] < 90, "Mitte sollte rot sein: {c:?}");
        // links: Hauptclip (Testbild) sichtbar, kein Grün
        let l = px(20, 160);
        assert!(
            !(l[1] > 150 && l[0] < 60 && l[2] < 100),
            "links unten kein Greenscreen-Grün: {l:?}"
        );
        let _ = oid;
    }

    #[test]
    fn exports_phase4_features() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (v, _) = make_media(dir.path());
        let matte = dir.path().join("matte.mkv");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=black:s=320x180:r=30:d=3",
                "-vf",
                "drawbox=x=0:y=0:w=160:h=180:color=white:t=fill,format=gray",
                "-c:v",
                "ffv1"
            ])
            .arg(&matte)
            .status()
            .unwrap()
            .success());
        let mk = |person: Option<PersonFx>, speed: f64| {
            let mut d = Document::new(Project::new("Phase4"));
            let mut s = d.project().settings.clone();
            s.width = 320;
            s.height = 180;
            d.apply(EditOp::SetSettings { settings: s }, OpSource::Ui)
                .unwrap();
            d.apply(EditOp::ImportMedia { media: v.clone() }, OpSource::Ui)
                .unwrap();
            let mut c = Clip::new(
                ClipContent::Media {
                    media: v.id,
                    kind: MediaKind::Video,
                },
                Time::ZERO,
                v.duration,
            );
            c.fx.cutout = Some(matte.to_string_lossy().into_owned());
            c.fx.person = person;
            c.fx.denoise = 0.5;
            c.fx.deflicker = true;
            c.fx.smooth_slowmo = true;
            c.speed = speed as _;
            d.apply(
                EditOp::AddClip {
                    track: None,
                    clip: c,
                },
                OpSource::Ui,
            )
            .unwrap();
            d.project().clone()
        };
        let render = |p: &Project, s: &ExportSettings, name: &str| {
            let out = dir.path().join(name);
            let job = build(p, s, &no_fonts, &out).unwrap();
            run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_or_else(|e| {
                panic!(
                    "{e}
{}",
                    std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap()
                )
            });
            out
        };
        // Hintergrundfarbe: rechte Hälfte (Maske schwarz) wird blau, links bleibt das Bild
        let p = mk(Some(PersonFx::ColorBackground { color: [0, 0, 255] }), 0.5);
        let out = render(&p, &ExportSettings::from_project(&p), "color.mp4");
        let f = cl_media::decode_frame(&out, Time::from_secs_f64(0.5), 320, 180).unwrap();
        let px = |x: usize, y: usize| f.data[(y * 320 + x) * 4..(y * 320 + x) * 4 + 3].to_vec();
        let r = px(300, 90);
        assert!(r[2] > 180 && r[0] < 60, "rechts sollte blau sein: {r:?}");
        // Weichzeichnen und Umriss laufen ohne Fehler durch
        for pf in [
            PersonFx::BlurBackground { amount: 0.7 },
            PersonFx::Outline {
                color: [255, 0, 0],
                width: 0.5,
            },
        ] {
            let p = mk(Some(pf), 1.0);
            render(&p, &ExportSettings::from_project(&p), "pf.mp4");
        }
        // Transparenter Export: freigestellt ohne Personen-Effekt → Alpha in MOV und WebM
        let p = mk(None, 1.0);
        for (fmt, codec, name) in [
            (Format::Mov, Codec::ProRes, "a.mov"),
            (Format::WebM, Codec::Vp9, "a.webm"),
        ] {
            let mut s = ExportSettings::from_project(&p);
            s.format = fmt;
            s.codec = codec;
            s.transparent = true;
            s.include_audio = false;
            let out = render(&p, &s, name);
            let pf = cl_media::ffprobe_command()
                .unwrap()
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=pix_fmt:stream_tags=alpha_mode",
                    "-of",
                    "default=nw=1",
                ])
                .arg(&out)
                .output()
                .unwrap();
            let txt = String::from_utf8_lossy(&pf.stdout).to_string();
            assert!(
                txt.contains("yuva") || txt.contains("alpha_mode=1"),
                "{name}: kein Alphakanal: {txt}"
            );
        }
    }

    #[test]
    fn audio_preview_streams_pcm() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = project(dir.path());
        let (mut child, _wd) = spawn_audio_preview(&p, Time::from_secs_f64(1.0), 48000).unwrap();
        let mut buf = Vec::new();
        std::io::Read::read_to_end(child.stdout.as_mut().unwrap(), &mut buf).unwrap();
        child.wait().unwrap();
        let secs = buf.len() as f64 / (48000.0 * 2.0 * 4.0);
        assert!((secs - 3.0).abs() < 0.1, "{secs}");
    }

    /// Peak-Pegel je Kanal (dB) einer Tondatei, nur der Abschnitt `ss`..`ss + t`.
    fn peaks_db(path: &Path, ss: f64, t: f64) -> Vec<f64> {
        let o = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "info", "-nostdin", "-ss", &f(ss), "-t", &f(t), "-i"])
            .arg(path)
            .args(["-af", "astats=measure_overall=none:measure_perchannel=Peak_level", "-f", "null", "-"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stderr)
            .lines()
            .filter_map(|l| l.split("Peak level dB:").nth(1))
            .map(|v| v.trim().parse::<f64>().unwrap_or(f64::NEG_INFINITY))
            .collect()
    }

    /// Projekt mit einer Tonspur: Sinus (2 s) ab `start`.
    fn sine_project(dir: &Path, start: f64) -> Project {
        let src = dir.join("sine.wav");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=440:d=2", "-ac", "2"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        let m = cl_media::probe(&src).unwrap();
        let mut p = Project::new("Ton");
        p.media.push(m.clone());
        let mut t = Track::new(TrackKind::Audio);
        t.clips.push(Clip::new(
            ClipContent::Media { media: m.id, kind: MediaKind::Audio },
            Time::from_secs_f64(start),
            Time::from_secs_f64(2.0),
        ));
        p.timeline.tracks.push(t);
        p
    }

    fn wav_settings(p: &Project) -> ExportSettings {
        let mut s = ExportSettings::from_project(p);
        s.format = Format::Wav;
        s
    }

    #[test]
    fn overlapping_loud_clips_are_limited_quiet_stays() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = sine_project(dir.path(), 0.0);
        let quiet = dir.path().join("quiet.wav");
        run(&build(&p, &wav_settings(&p), &no_fonts, &quiet).unwrap(), Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let src = peaks_db(&dir.path().join("sine.wav"), 0.0, 2.0);
        let q = peaks_db(&quiet, 0.0, 2.0);
        assert!(q.len() == src.len() && q.iter().zip(&src).all(|(a, b)| (a - b).abs() < 0.01), "leise unverändert: {q:?} / {src:?}");
        // drei gleichphasige Sinus mit je +15 dB: ohne Begrenzer ca. +6,5 dBFS
        let clip = p.timeline.tracks[0].clips[0].clone();
        for _ in 0..3 {
            let mut t = Track::new(TrackKind::Audio);
            let mut c = clip.clone();
            c.id = Default::default();
            c.volume_db = 15.0;
            t.clips.push(c);
            p.timeline.tracks.push(t);
        }
        let loud = dir.path().join("loud.wav");
        run(&build(&p, &wav_settings(&p), &no_fonts, &loud).unwrap(), Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let l = peaks_db(&loud, 0.0, 2.0);
        assert!(!l.is_empty() && l.iter().all(|v| *v <= graph::LIMIT_DB + 0.1 && *v > -3.0), "begrenzt: {l:?}");
    }

    #[test]
    fn limited_aac_peak_stays_below_minus_one_db() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        // wie im Testbericht: laute Sounds gleichzeitig, je +6 dB
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::new("Laut");
        for id in ["whoosh", "impact", "boom", "airhorn"] {
            let path = dir.path().join(format!("{id}.wav"));
            cl_fx::sfx::render(cl_fx::sfx::sfx(id).unwrap(), &path).unwrap();
            let m = cl_media::probe(&path).unwrap();
            let mut t = Track::new(TrackKind::Audio);
            let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Audio }, Time::ZERO, m.duration);
            c.volume_db = 6.0;
            t.clips.push(c);
            p.timeline.tracks.push(t);
            p.media.push(m);
        }
        let mut s = ExportSettings::from_project(&p);
        s.format = Format::M4a;
        let out = dir.path().join("loud.m4a");
        run(&build(&p, &s, &no_fonts, &out).unwrap(), Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let l = peaks_db(&out, 0.0, 3.0);
        assert!(!l.is_empty() && l.iter().all(|v| *v <= -1.0), "AAC-Spitze: {l:?}");
    }

    #[test]
    fn audio_clip_offset_leaves_leading_silence() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = sine_project(dir.path(), 1.0);
        let out = dir.path().join("off.wav");
        run(&build(&p, &wav_settings(&p), &no_fonts, &out).unwrap(), Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let gap = peaks_db(&out, 0.0, 0.9);
        let tone = peaks_db(&out, 1.2, 1.5);
        assert!(!gap.is_empty() && gap.iter().all(|v| *v < -60.0), "Lücke still: {gap:?}");
        assert!(!tone.is_empty() && tone.iter().all(|v| *v > -30.0), "Ton ab 1 s: {tone:?}");
    }

    #[test]
    fn fade_longer_than_clip_still_exports() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = sine_project(dir.path(), 0.0);
        // z. B. nach Tempo-Änderung: Blenden länger als der Clip
        let c = &mut p.timeline.tracks[0].clips[0];
        c.fade_in = Time::from_secs_f64(5.0);
        c.fade_out = Time::from_secs_f64(5.0);
        let out = dir.path().join("fade.wav");
        run(&build(&p, &wav_settings(&p), &no_fonts, &out).unwrap(), Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(out.is_file());
    }

    /// Integrierte Lautheit (LUFS) einer Datei laut `ebur128`.
    fn lufs(path: &Path) -> f64 {
        let o = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "info", "-nostdin", "-i"])
            .arg(path)
            .args(["-af", "ebur128", "-f", "null", "-"])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&o.stderr);
        let i = log.rfind("I:").expect("ebur128-Zusammenfassung");
        log[i + 2..].split_whitespace().next().unwrap().parse().unwrap()
    }

    #[test]
    fn loudness_is_normalized_in_two_passes() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = sine_project(dir.path(), 0.0);
        let mut s = wav_settings(&p);
        s.loudness = Some(-14.0);
        let out = dir.path().join("laut.wav");
        let mut seen = Vec::new();
        run(&build(&p, &s, &no_fonts, &out).unwrap(), Arc::new(AtomicBool::new(false)), |x| seen.push(x)).unwrap();
        let l = lufs(&out);
        assert!((l + 14.0).abs() <= 1.0, "{l} LUFS");
        // Messlauf meldet sich im ersten Teil des Fortschritts
        assert!(seen.iter().any(|x| *x <= MEASURE_SHARE) && seen.last() == Some(&1.0), "{seen:?}");
        // Projekt ohne hörbaren Ton: Stille bleibt, kein Fehler
        p.timeline.tracks[0].muted = true;
        run(&build(&p, &s, &no_fonts, &out).unwrap(), Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(peaks_db(&out, 0.0, 2.0).iter().all(|v| *v < -60.0));
    }

    #[test]
    fn speed_curve_keeps_audio() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = sine_project(dir.path(), 0.0);
        p.timeline.tracks[0].clips[0].fx.speed_curve = Some("montage".into());
        let out = dir.path().join("kurve.wav");
        run(&build(&p, &wav_settings(&p), &no_fonts, &out).unwrap(), Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let tone = peaks_db(&out, 0.05, 1.9);
        assert!(!tone.is_empty() && tone.iter().all(|v| *v > -30.0), "{tone:?}");
        let secs = cl_media::probe(&out).unwrap().duration.as_secs_f64();
        assert!((secs - 2.0).abs() < 0.05, "{secs}");
    }

    #[test]
    fn audio_bitrate_follows_settings() {
        let mut s = ExportSettings::from_project(&Project::new("a"));
        assert_eq!(s.audio_bitrate(), 256);
        s.audio_kbps = Some(384);
        assert!(codec_args(&s).contains(&"384k".to_string()));
        s.include_audio = false;
        assert_eq!(s.audio_bitrate(), 0);
    }

    #[test]
    fn cancelled_export_keeps_existing_file() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = sine_project(dir.path(), 0.0);
        let out = dir.path().join("ziel.wav");
        std::fs::write(&out, b"alte Datei").unwrap();
        let job = build(&p, &wav_settings(&p), &no_fonts, &out).unwrap();
        assert!(matches!(run(&job, Arc::new(AtomicBool::new(true)), |_| {}), Err(ExportError::Cancelled)));
        assert_eq!(std::fs::read(&out).unwrap(), b"alte Datei");
        assert!(!dir.path().join("ziel.part.wav").exists());
        // Erfolg: Ziel ersetzt, keine Teildatei übrig
        run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(cl_media::probe(&out).is_ok());
        assert!(!job.part.exists());
    }

    #[test]
    fn unused_or_remote_media_is_ignored() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = sine_project(dir.path(), 0.0);
        // Ungenutztes fehlendes Medium blockiert den Export nicht
        let mut gone = p.media[0].clone();
        gone.id = MediaId::new();
        gone.path = dir.path().join("weg.wav");
        p.media.push(gone);
        assert!(build(&p, &wav_settings(&p), &no_fonts, &dir.path().join("o.wav")).is_ok());
        // Vorschau lädt keine URL aus der Projektdatei
        p.media[0].path = PathBuf::from("http://127.0.0.1:9/x.wav");
        let (mut child, wd) = spawn_audio_preview(&p, Time::ZERO, 48000).unwrap();
        let _ = child.kill();
        let _ = child.wait();
        let g = std::fs::read_to_string(wd.path().join("graph.txt")).unwrap();
        assert!(!g.contains(":a]"), "{g}");
    }

    /// Harter Wisch von links: zur Mitte deutlich anders als der Rückfall „fade“.
    const WIPE: &str = "fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f { return select(from_color(uv), to_color(uv), uv.x < progress); }";

    /// Testpaket mit Shader-Übergängen (der Katalog ist global: in allen Tests gleich, einmal
    /// angelegt): `t.shader/wipe` und `t.shader/broken` (ungültiger Shader), Rückfall `fade`.
    fn shader_pack() {
        static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            use cl_fx::catalog::{Entry, Kind, Pack, PackInfo, Payload};
            // fester Name: je Testlauf ein neuer Ordner sammelte sich in %TEMP% (Inhalt ist immer gleich)
            let dir = std::env::temp_dir().join("cutline-shader-test");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("wipe.wgsl"), WIPE).unwrap();
            std::fs::write(dir.join("broken.wgsl"), "fn transition( kaputt").unwrap();
            let e = |id: &str| Entry {
                id: format!("t.shader/{id}"),
                kind: Kind::Transition,
                name: id.into(),
                categories: vec!["Effekt".into()],
                pack: "t.shader".into(),
                license: "CC0-1.0".into(),
                author: "Test".into(),
                source: None,
                tags: Vec::new(),
                payload: Payload::Shader {
                    file: dir.join(format!("{id}.wgsl")),
                    fallback: "fade".into(),
                    params: Vec::new(),
                },
            };
            cl_fx::catalog::load_packs(vec![Pack {
                info: PackInfo {
                    id: "t.shader".into(),
                    name: "Test".into(),
                    version: "1.0.0".into(),
                    description: String::new(),
                    license: "CC0-1.0".into(),
                    author: "Test".into(),
                    source: None,
                },
                enabled: true,
                entries: vec![e("wipe"), e("broken")],
            }]);
        });
    }

    /// Einfarbiges Video (30 Bilder/s)
    fn color_video(dir: &Path, name: &str, color: &str, secs: f64, size: &str) -> MediaRef {
        let path = dir.join(name);
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("color={color}:size={size}:rate=30:duration={secs}"))
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(st.success());
        cl_media::probe(&path).unwrap()
    }

    /// 3 s, 320×180: rot (0–1,5 s) → blau (1,5–3 s), Übergang `id` über 0,6 s (Fenster 1,2–1,8 s),
    /// optional ein Text mit grüner Fläche darüber.
    fn shader_project(dir: &Path, id: &str, text: bool) -> Project {
        let a = color_video(dir, "a.mp4", "red", 2.0, "320x180");
        let b = color_video(dir, "b.mp4", "blue", 2.0, "320x180");
        let mut p = Project::new("Shader");
        p.settings.width = 320;
        p.settings.height = 180;
        p.settings.fps = Rational { num: 30, den: 1 };
        let mut t = Track::new(TrackKind::Video);
        for (i, m) in [&a, &b].into_iter().enumerate() {
            let mut c = Clip::new(
                ClipContent::Media { media: m.id, kind: MediaKind::Video },
                Time::from_secs_f64(1.5 * i as f64),
                Time::from_secs_f64(1.5),
            );
            if i == 0 {
                c.transition_out = Some(TransitionRef { id: id.into(), duration: Time::from_secs_f64(0.6) });
            }
            t.clips.push(c);
        }
        p.media = vec![a, b];
        p.timeline.tracks.push(t);
        if text {
            let mut tt = Track::new(TrackKind::Text);
            tt.clips.push(Clip::new(
                ClipContent::Text(TextStyle {
                    text: "MM".into(),
                    size: 60.0,
                    color: [0, 255, 0, 255],
                    background: Some([0, 255, 0, 255]),
                    ..Default::default()
                }),
                Time::ZERO,
                Time::from_secs_f64(3.0),
            ));
            p.timeline.tracks.push(tt);
        }
        p
    }

    fn export(p: &Project, dir: &Path, name: &str) -> (PathBuf, Job) {
        let out = dir.join(name);
        let job = build(p, &ExportSettings::from_project(p), &no_fonts, &out).unwrap();
        run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        (out, job)
    }

    fn mean_diff(a: &[u8], b: &[u8]) -> f64 {
        let n = a.len().min(b.len()) / 4;
        let s: u64 = a
            .chunks_exact(4)
            .zip(b.chunks_exact(4))
            .map(|(x, y)| (0..3).map(|i| x[i].abs_diff(y[i]) as u64).sum::<u64>())
            .sum();
        s as f64 / (n * 3) as f64
    }

    /// Bild Nummer `n` (bildgenau, `decode_frame` sucht nur ungefähr)
    fn frame_n(f: &Path, n: usize) -> cl_media::RgbaFrame {
        let o = cl_media::ffmpeg_command()
            .unwrap()
            .arg("-i")
            .arg(f)
            .args(["-v", "error", "-vf", &format!("select=eq(n\\,{n})"), "-frames:v", "1"])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .output()
            .unwrap();
        let p = cl_media::probe(f).unwrap();
        assert_eq!(o.stdout.len(), (p.width * p.height * 4) as usize, "Bild {n}");
        cl_media::RgbaFrame { width: p.width, height: p.height, data: o.stdout }
    }

    fn rgb_at(f: &cl_media::RgbaFrame, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * f.width + x) * 4) as usize;
        [f.data[i], f.data[i + 1], f.data[i + 2]]
    }

    /// Karaoke über zwei Zeilen mit Kontur und Fett: zu jeder Zeit ist genau das gesprochene Wort
    /// ganz gelb (keine weißen Reste darin, nichts Gelbes in den anderen Wörtern).
    #[test]
    fn karaoke_highlights_exactly_the_spoken_word() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::new("Karaoke");
        (p.settings.width, p.settings.height) = (640, 360);
        p.settings.fps = Rational { num: 30, den: 1 };
        let mut tt = Track::new(TrackKind::Text);
        tt.clips.push(Clip::new(
            ClipContent::Text(TextStyle {
                text: "AAA BBB\nCCC DDD".into(),
                size: 60.0,
                bold: true,
                outline: Some((4.0, [0, 0, 0, 255])),
                karaoke: Some(Karaoke { color: [255, 221, 0, 255], fill: false, words: [0.0, 1.0, 2.0, 3.0].map(Time::from_secs_f64).to_vec() }),
                ..Default::default()
            }),
            Time::ZERO,
            Time::from_secs_f64(4.0),
        ));
        p.timeline.tracks.push(tt);
        let (out, _job) = export(&p, dir.path(), "k.mp4");
        for word in 0..4u32 {
            let f = frame_n(&out, (word * 30 + 15) as usize);
            // Viertel des Bilds um die Textmitte: Zeile oben/unten, Wort links/rechts
            let (mut yellow, mut white_in, mut yellow_out) = (0, 0, 0);
            for y in 0..360 {
                for x in 0..640 {
                    let [r, g, b] = rgb_at(&f, x, y);
                    let inside = (y >= 180) == (word >= 2) && (x >= 320) == (word % 2 == 1);
                    let is_yellow = r > 170 && g > 140 && b < 100;
                    let is_white = r > 200 && g > 200 && b > 200;
                    match (inside, is_yellow, is_white) {
                        (true, true, _) => yellow += 1,
                        (true, _, true) => white_in += 1,
                        (false, true, _) => yellow_out += 1,
                        _ => {}
                    }
                }
            }
            assert!(yellow > 500 && white_in < 20 && yellow_out < 20, "Wort {word}: gelb {yellow}, weiß darin {white_in}, gelb daneben {yellow_out}");
        }
    }

    #[test]
    fn window_frames_match_xfade() {
        // 30 Bilder/s, Versatz 1,2 s, 0,6 s: Bilder 36 bis 54
        assert_eq!(graph::window_frames(1.2, 0.6, 30.0), (36, 19));
        // Versatz zwischen zwei Bildern: erstes Bild danach
        assert_eq!(graph::window_frames(1.21, 0.6, 30.0).0, 37);
        assert_eq!(graph::window_frames(0.0, 0.04, 25.0), (0, 2));
    }

    /// Zuschnitt: der abgeschnittene Bereich ist leer (schwarz), der Rest liegt Pixel für Pixel
    /// an derselben Stelle wie ohne Zuschnitt – auch verkleinert, verschoben und gedreht.
    #[test]
    fn crop_blanks_the_edges_and_keeps_the_rest_in_place() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (v, _) = make_media(dir.path());
        let crop = Crop { left: 0.25, right: 0.1, top: 0.2, bottom: 0.0 };
        for (n, tf) in [
            Transform { x: 20.0, y: 0.0, scale: 0.8, rotation_deg: 0.0 },
            Transform { x: 0.0, y: 10.0, scale: 1.0, rotation_deg: 180.0 },
        ]
        .into_iter()
        .enumerate()
        {
            let mut p = Project::new("Zuschnitt");
            (p.settings.width, p.settings.height) = (320, 180);
            p.settings.fps = Rational { num: 30, den: 1 };
            p.media = vec![v.clone()];
            let mut c = Clip::new(ClipContent::Media { media: v.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(1.0));
            c.transform = tf;
            let mut t = Track::new(TrackKind::Video);
            t.clips.push(c);
            p.timeline.tracks.push(t);
            let (plain, _) = export(&p, dir.path(), &format!("ohne{n}.mp4"));
            p.timeline.tracks[0].clips[0].crop = crop;
            let (cut, job) = export(&p, dir.path(), &format!("mit{n}.mp4"));
            let g = std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap();
            assert!(g.contains("crop=208:144:80:36,pad=320:180:80:36"), "{g}");
            let (a, b) = (frame_n(&plain, 15), frame_n(&cut, 15));
            // Ausgabepixel → Quellpixel (Quelle 320×180 = Clipgröße, Drehung um die Bildmitte)
            let (rad, s) = (tf.rotation_deg.to_radians(), tf.scale);
            let (mut gone, mut kept, mut diff) = (0, 0, 0u32);
            for py in (2..178).step_by(4) {
                for px in (2..318).step_by(4) {
                    let dx = px as f32 + 0.5 - (160.0 + tf.x);
                    let dy = py as f32 + 0.5 - (90.0 + tf.y);
                    let sx = (dx * rad.cos() + dy * rad.sin()) / s + 160.0;
                    let sy = (-dx * rad.sin() + dy * rad.cos()) / s + 90.0;
                    let inside = |m: f32| sx > m && sx < 320.0 - m && sy > m && sy < 180.0 - m;
                    if !inside(6.0) {
                        continue;
                    }
                    let (l, r, top) = (80.0, 320.0 - 32.0, 36.0);
                    if sx < l - 6.0 || sx > r + 6.0 || sy < top - 6.0 {
                        let v = rgb_at(&b, px, py);
                        assert!(v.iter().all(|&c| c < 30), "#{n} ({px},{py}) sollte leer sein: {v:?}");
                        gone += 1;
                    } else if sx > l + 6.0 && sx < r - 6.0 && sy > top + 6.0 {
                        let (x, y) = (rgb_at(&a, px, py), rgb_at(&b, px, py));
                        // einzelne Pixel weichen durch die Kodierung ab (4:2:0 an Farbkanten)
                        diff += (0..3).map(|i| x[i].abs_diff(y[i]) as u32).sum::<u32>();
                        kept += 1;
                    }
                }
            }
            assert!(gone > 100 && kept > 400, "#{n}: {gone} leer, {kept} gleich");
            let mean = diff as f64 / (kept * 3) as f64;
            assert!(mean < 6.0, "#{n}: Rest weicht im Mittel um {mean:.1} ab (verschoben?)");
        }
    }

    /// Shader-Übergang im Export: gleiche Länge und Ströme wie mit xfade, Mitte = cl-gpu-Bild,
    /// davor/danach reine Clips, Ebenen darüber bleiben, kaputter Shader → Rückfall.
    #[test]
    fn shader_transition_exports_like_gpu_render() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        shader_pack();
        let dir = tempfile::tempdir().unwrap();
        let p = shader_project(dir.path(), "t.shader/wipe", false);
        // erzwungener Fehler im Vorlauf (ungültiger Shader): Export gelingt mit dem Rückfall
        let mut pb = p.clone();
        pb.timeline.tracks[0].clips[0].transition_out.as_mut().unwrap().id = "t.shader/broken".into();
        let (fall, job) = export(&pb, dir.path(), "fallback.mp4");
        assert_eq!(job.passes.len(), 1);
        let g = std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap();
        assert!(!g.contains("movie="), "{g}");
        let Some(gpu) = cl_gpu::gpu() else {
            eprintln!("keine GPU: nur der Rückfall geprüft");
            return;
        };
        let (out, job) = export(&p, dir.path(), "shader.mp4");
        let g = std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap();
        assert!(g.contains("movie=shader_0.nut") && g.contains("blend=all_expr=B"), "{g}");
        let (a, b) = (cl_media::probe(&out).unwrap(), cl_media::probe(&fall).unwrap());
        assert!((a.duration.as_secs_f64() - b.duration.as_secs_f64()).abs() < 0.05, "{a:?} {b:?}");
        assert_eq!((a.width, a.height, a.has_audio), (b.width, b.height, b.has_audio));
        // Mitte: Bild 45 (1,5 s) = Fortschritt 0,5
        let mid = frame_n(&out, 45);
        let red = vec![255u8, 0, 0, 255].repeat(320 * 180);
        let blue = vec![0u8, 0, 255, 255].repeat(320 * 180);
        let fa = cl_gpu::Frame { rgba: &red, width: 320, height: 180 };
        let fb = cl_gpu::Frame { rgba: &blue, width: 320, height: 180 };
        let want = gpu.render(WIPE, &[0.0; cl_gpu::PARAMS], fa, fb, (320, 180), 0.5).unwrap();
        let d = mean_diff(&mid.data, &want);
        assert!(d < 8.0, "Mitte weicht vom cl-gpu-Bild ab: {d}");
        let d_fall = mean_diff(&frame_n(&fall, 45).data, &want);
        assert!(d_fall > 40.0, "Rückfall sollte anders aussehen: {d_fall}");
        // links schon blau, rechts noch rot
        assert!(rgb_at(&mid, 40, 90)[2] > 200 && rgb_at(&mid, 280, 90)[0] > 200, "{:?} {:?}", rgb_at(&mid, 40, 90), rgb_at(&mid, 280, 90));
        // kurz vor und nach dem Fenster (Bilder 36–54): reine Clips
        let before = frame_n(&out, 35);
        let after = frame_n(&out, 55);
        assert!(mean_diff(&before.data, &red) < 8.0, "vorher nicht rot");
        assert!(mean_diff(&after.data, &blue) < 8.0, "nachher nicht blau");
    }

    /// FFmpeg 7 vergisst die Bildrate nach `setpts`; ein `tpad` direkt dahinter hält dann kein Bild
    /// (Clip-Ende, Vorlauf des Shader-Übergangs). In keinem Graphen darf `tpad` auf `setpts` folgen.
    #[test]
    fn no_tpad_right_after_setpts() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        shader_pack();
        let dir = tempfile::tempdir().unwrap();
        let p = shader_project(dir.path(), "t.shader/wipe", false);
        let job = build(&p, &ExportSettings::from_project(&p), &no_fonts, &dir.path().join("x.mp4")).unwrap();
        let mut graphs = 0;
        for e in std::fs::read_dir(job.workdir.path()).unwrap() {
            let g = std::fs::read_to_string(e.unwrap().path()).unwrap_or_default();
            graphs += usize::from(g.contains("tpad="));
            // Filter ohne vorangestellte Labels: „[a][b]xfade=…“ → „xfade=…“
            let parts: Vec<&str> = g.split([',', ';', '\n']).map(|s| s.split(']').rfind(|x| !x.is_empty()).unwrap_or("")).collect();
            assert!(!parts.windows(2).any(|w| w[0].starts_with("setpts=") && w[1].starts_with("tpad=")), "{g}");
        }
        assert!(graphs >= 2, "Hauptgraph und Vorlauf geprüft");
    }

    #[test]
    fn layer_above_shader_transition_stays_visible() {
        if !cl_media::ffmpeg_available() || default_font().is_none() || cl_gpu::gpu().is_none() {
            return;
        }
        shader_pack();
        let dir = tempfile::tempdir().unwrap();
        let p = shader_project(dir.path(), "t.shader/wipe", true);
        let (out, job) = export(&p, dir.path(), "text.mp4");
        assert_eq!(job.passes.len(), 1);
        let mid = frame_n(&out, 45);
        // grüne Textfläche in der Mitte über dem Übergang
        let c = rgb_at(&mid, 160, 90);
        assert!(c[1] > 180 && c[0] < 80 && c[2] < 80, "{c:?}");
    }

    #[test]
    fn cancel_during_shader_pass_stops() {
        if !cl_media::ffmpeg_available() || cl_gpu::gpu().is_none() {
            return;
        }
        shader_pack();
        let dir = tempfile::tempdir().unwrap();
        let p = shader_project(dir.path(), "t.shader/wipe", false);
        let out = dir.path().join("c.mp4");
        let job = build(&p, &ExportSettings::from_project(&p), &no_fonts, &out).unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(matches!(run(&job, cancel, |_| {}), Err(ExportError::Cancelled)));
        assert!(!out.exists());
    }

    /// Messung: 10-s-Projekt (1080p, 30 Bilder/s) mit 3 Übergängen, mit/ohne Shader.
    /// `cargo test --release -p cl-export shader_export_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn shader_export_timing() {
        shader_pack();
        let dir = tempfile::tempdir().unwrap();
        let cols = ["red", "blue", "green", "yellow"];
        let mut p = Project::new("Zeit");
        p.settings.width = 1920;
        p.settings.height = 1080;
        p.settings.fps = Rational { num: 30, den: 1 };
        let mut t = Track::new(TrackKind::Video);
        for (i, c) in cols.iter().enumerate() {
            let m = color_video(dir.path(), &format!("{c}.mp4"), &format!("{c}"), 3.0, "1920x1080");
            let mut cl = Clip::new(
                ClipContent::Media { media: m.id, kind: MediaKind::Video },
                Time::from_secs_f64(2.5 * i as f64),
                Time::from_secs_f64(2.5),
            );
            if i < 3 {
                cl.transition_out = Some(TransitionRef { id: "t.shader/wipe".into(), duration: Time::from_secs_f64(0.6) });
            }
            p.media.push(m);
            t.clips.push(cl);
        }
        p.timeline.tracks.push(t);
        let time = |p: &Project, n: &str| {
            let s = std::time::Instant::now();
            let (_, job) = export(p, dir.path(), n);
            let g = std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap();
            println!("{n}: {} Shader-Übergänge im Graph", g.matches("blend=all_expr=B").count());
            s.elapsed().as_secs_f64()
        };
        let with = time(&p, "mit.mp4");
        for c in &mut p.timeline.tracks[0].clips {
            if let Some(t) = &mut c.transition_out {
                t.id = "fade".into();
            }
        }
        let without = time(&p, "ohne.mp4");
        println!("Export 10 s 1080p, 3 Übergänge: mit Shader {with:.2} s, ohne {without:.2} s");
    }

    #[test]
    fn transition_id_cannot_inject_filters() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = project(dir.path());
        let main = p.timeline.tracks.iter_mut().find(|t| t.main).unwrap();
        main.clips[0].transition_out = Some(TransitionRef {
            id: "fade[x];movie=geheim.mp4[y];[y]nullsink".into(),
            duration: Time::from_secs_f64(0.5),
        });
        let job = build(&p, &ExportSettings::from_project(&p), &no_fonts, &dir.path().join("x.mp4")).unwrap();
        let g = std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap();
        assert!(!g.contains("movie="), "{g}");
        assert!(g.contains("xfade=transition=fade:"), "{g}");
    }

    #[test]
    fn text_with_percent_and_backslash_is_literal() {
        if !cl_media::ffmpeg_available() || default_font().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::new("Text");
        let mut t = Track::new(TrackKind::Text);
        t.clips.push(Clip::new(
            ClipContent::Text(TextStyle { text: "50% Rabatt \\o/".into(), ..Default::default() }),
            Time::ZERO,
            Time::from_secs_f64(0.5),
        ));
        p.timeline.tracks.push(t);
        let out = dir.path().join("t.mp4");
        let job = build(&p, &ExportSettings::from_project(&p), &no_fonts, &out).unwrap();
        let g = std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap();
        assert!(g.contains("expansion=none"), "{g}");
        run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(cl_media::probe(&out).is_ok());
    }

    #[test]
    fn bitrate_quality_drops_constant_quality_args() {
        let mut s = ExportSettings::from_project(&Project::new("q"));
        s.quality = Quality::Bitrate(5000);
        for (codec, enc) in [
            (Codec::H264, Encoder::Software),
            (Codec::Hevc, Encoder::Software),
            (Codec::H264, Encoder::Nvenc),
            (Codec::H264, Encoder::Qsv),
        ] {
            s.codec = codec;
            s.encoder = enc;
            let a = codec_args(&s);
            assert!(!a.iter().any(|x| matches!(x.as_str(), "-crf" | "-cq" | "-global_quality")), "{a:?}");
            assert_eq!(a.iter().filter(|x| *x == "-b:v").count(), 1, "{a:?}");
            assert!(a.contains(&"5000k".to_string()), "{a:?}");
        }
        s.quality = Quality::Recommended;
        assert!(codec_args(&s).contains(&"-global_quality".to_string()));
    }

    /// Jede angebotene Codec-Container-Kombination mit Software-Encoder exportiert mit eigener Bitrate.
    #[test]
    fn bitrate_export_works_for_every_software_codec() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = sine_project(dir.path(), 0.0);
        let mut s = ExportSettings::from_project(&p);
        (s.width, s.height) = (128, 72);
        s.quality = Quality::Bitrate(800);
        s.include_audio = false;
        s.range = Some((Time::ZERO, Time::from_secs_f64(1.0)));
        for format in [Format::Mp4, Format::Mov, Format::Mkv, Format::WebM] {
            for codec in Codec::for_format(format) {
                (s.format, s.codec) = (format, *codec);
                let out = dir.path().join(format!("{codec:?}.{}", format.extension()));
                let job = build(&p, &s, &no_fonts, &out).unwrap();
                run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_or_else(|e| panic!("{codec:?} in {format:?}: {e}"));
            }
        }
    }

    /// Das mitgelieferte Windows-FFmpeg hat kein SVT-AV1: der Ersatz (libaom) muss dort laufen,
    /// mit Qualität und mit fester Bitrate samt Deckel.
    #[test]
    fn av1_fallback_encoder_works() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        for extra in [&["32"][..], &["32", "-b:v", "800k", "-maxrate", "880k", "-bufsize", "1600k"][..]] {
            let out = dir.path().join("a.mkv");
            let st = cl_media::ffmpeg_command()
                .unwrap()
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=128x72:rate=30:duration=0.5"])
                .args(av1_software(false))
                .args(extra)
                .arg(&out)
                .stdin(Stdio::null())
                .status()
                .unwrap();
            assert!(st.success(), "libaom-av1 mit {extra:?}");
            assert!(out.metadata().unwrap().len() > 0);
        }
    }

    #[test]
    fn second_pass_handles_silence_missing_and_loud_input() {
        let log = |i: &str, th: &str| {
            format!("{{\n\t\"input_i\" : \"{i}\",\n\t\"input_tp\" : \"{i}\",\n\t\"input_lra\" : \"3.0\",\n\t\"input_thresh\" : \"{th}\",\n\t\"target_offset\" : \"0.5\"\n}}")
        };
        let normal = second_pass(&log("-20.00", "-30.00"), -14.0).unwrap();
        assert!(normal.starts_with("loudnorm=") && normal.contains("measured_I=-20:"), "{normal}");
        assert_eq!(second_pass(&log("-inf", "-inf"), -14.0).as_deref(), Some("anull"));
        assert_eq!(second_pass("kein JSON", -14.0), None);
        assert_eq!(second_pass(&log("-20.00", "-30.00").replace("input_lra", "x"), -14.0), None);
        // +4 LUFS: loudnorm nimmt measured_I nur bis 0 → vorher um 6 dB absenken
        let loud = second_pass(&log("4.00", "-6.00"), -14.0).unwrap();
        assert!(loud.starts_with("volume=-6dB,loudnorm=") && loud.contains("measured_I=-2:") && loud.contains("measured_thresh=-12:"), "{loud}");
    }

    #[test]
    fn very_loud_audio_is_normalized() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = sine_project(dir.path(), 0.0);
        p.timeline.tracks[0].clips[0].volume_db = 30.0;
        let mut s = wav_settings(&p);
        s.loudness = Some(-14.0);
        let out = dir.path().join("sehr-laut.wav");
        run(&build(&p, &s, &no_fonts, &out).unwrap(), Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let l = lufs(&out);
        assert!((l + 14.0).abs() <= 1.0, "{l} LUFS");
    }

    #[test]
    fn subtitles_escape_and_follow_range() {
        let mut p = Project::new("x");
        let mut t = Track::new(TrackKind::Text);
        let txt = |s: &str| ClipContent::Text(TextStyle { text: s.into(), ..Default::default() });
        t.clips.push(Clip::new(txt("<3 A --> B & C\n\nzwei"), Time::from_secs_f64(1.0), Time::from_secs_f64(2.0)));
        t.clips.push(Clip::new(txt("später"), Time::from_secs_f64(5.0), Time::from_secs_f64(1.0)));
        p.timeline.tracks.push(t);
        assert_eq!(
            to_vtt(&p),
            "WEBVTT\n\n1\n00:00:01.000 --> 00:00:03.000\n&lt;3 A -&gt; B &amp; C\nzwei\n\n2\n00:00:05.000 --> 00:00:06.000\nspäter\n"
        );
        assert!(to_srt(&p).contains("\n<3 A --> B & C\nzwei\n"));
        // Bereich 2–4 s: erster Text gekürzt und verschoben, zweiter fehlt
        assert_eq!(
            to_srt_in(&p, Time::from_secs_f64(2.0), Time::from_secs_f64(4.0)),
            "1\n00:00:00,000 --> 00:00:01,000\n<3 A --> B & C\nzwei\n"
        );
    }

    #[test]
    fn srt_and_errors() {
        let mut p = Project::new("x");
        assert!(matches!(
            build(
                &p,
                &ExportSettings::from_project(&p),
                &no_fonts,
                Path::new("x.mp4")
            ),
            Err(ExportError::Empty)
        ));
        let mut t = Track::new(TrackKind::Text);
        t.clips.push(Clip::new(
            ClipContent::Text(TextStyle {
                text: "Hallo".into(),
                ..Default::default()
            }),
            Time::from_millis(1500),
            Time::from_millis(2000),
        ));
        p.timeline.tracks.push(t);
        assert_eq!(to_srt(&p), "1\n00:00:01,500 --> 00:00:03,500\nHallo\n");
        assert_eq!(to_vtt(&p), "WEBVTT\n\n1\n00:00:01.500 --> 00:00:03.500\nHallo\n");
        assert!(subtitles_for(&p, Path::new("a.VTT")).starts_with("WEBVTT"));
    }
    /// Bewegtes Bild (1 s, Streifen läuft nach rechts) als 4-s-Bild-Clip über blauem Video:
    /// einmalig → steht bei 3,5 s auf dem letzten Bild; Schleife → läuft weiter. Transparenz
    /// bleibt (daneben Hintergrund). Früher brach der Export mit „Option loop not found“ ab.
    #[test]
    fn animated_image_clips_hold_or_loop() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let ff = |args: &[&str]| {
            let st = cl_media::ffmpeg_command()
                .unwrap()
                .args(["-v", "error", "-y"])
                .args(args)
                .status()
                .unwrap();
            assert!(st.success(), "{args:?}");
        };
        let p = |n: &str| dir.path().join(n).to_string_lossy().into_owned();
        ff(&["-f", "lavfi", "-i", "color=blue:size=320x320:rate=30:duration=4", "-pix_fmt", "yuv420p", &p("bg.mp4")]);
        let band = "color=red:size=64x64:rate=10:duration=1,format=rgba,geq=r=255:g=0:b=0:a='if(between(X-T*48,0,15),255,0)'";
        ff(&["-f", "lavfi", "-i", band, "-plays", "0", "-f", "apng", &p("loop.png")]);
        ff(&["-f", "lavfi", "-i", band, "-plays", "1", "-f", "apng", &p("once.png")]);
        let pal = "split[a][b];[a]palettegen=reserve_transparent=1[p];[b][p]paletteuse=alpha_threshold=128";
        ff(&["-f", "lavfi", "-i", band, "-vf", pal, "-loop", "-1", &p("once.gif")]);
        // Spalte (0..64) mit rotem Element bzw. blauem Hintergrund?
        let col = |f: &cl_media::RgbaFrame, x: u32| {
            let i = ((32 * f.width + x) * 4) as usize;
            let px = &f.data[i..i + 3];
            if px[0] > 180 && px[2] < 90 {
                'r'
            } else if px[2] > 180 && px[0] < 90 {
                'b'
            } else {
                '?'
            }
        };
        for (file, looping) in [("once.png", false), ("once.gif", false), ("loop.png", true)] {
            let bg = cl_media::probe(Path::new(&p("bg.mp4"))).unwrap();
            let el = cl_media::probe(Path::new(&p(file))).unwrap();
            assert_eq!(el.kind, MediaKind::Image, "{file}");
            let mut pr = Project::new("Bewegtes Bild");
            pr.settings.width = 320;
            pr.settings.height = 320;
            let mut v = Track::new(TrackKind::Video);
            v.clips.push(Clip::new(
                ClipContent::Media { media: bg.id, kind: MediaKind::Video },
                Time::ZERO,
                Time::from_secs_f64(4.0),
            ));
            let mut o = Track::new(TrackKind::Video);
            o.clips.push(Clip::new(
                ClipContent::Media { media: el.id, kind: MediaKind::Image },
                Time::ZERO,
                Time::from_secs_f64(4.0),
            ));
            pr.media = vec![bg, el];
            pr.timeline.tracks = vec![v, o];
            let out = dir.path().join(format!("{file}.mp4"));
            let s = ExportSettings::from_project(&pr);
            run(&build(&pr, &s, &no_fonts, &out).unwrap(), Arc::new(AtomicBool::new(false)), |_| {})
                .unwrap_or_else(|e| panic!("{file}: {e}"));
            assert!((cl_media::probe(&out).unwrap().duration.as_secs_f64() - 4.0).abs() < 0.15);
            let at = |t: f64| cl_media::decode_frame(&out, Time::from_secs_f64(t), 64, 64).unwrap();
            let (f30, f35) = (at(3.0), at(3.5));
            if looping {
                // 3,0 s = Anfang der dritten Runde (Streifen links), 3,5 s = Mitte
                assert_eq!((col(&f30, 8), col(&f35, 8), col(&f35, 32), col(&f35, 55)), ('r', 'b', 'r', 'b'), "{file}");
            } else {
                // letztes Bild (Streifen bei 43..58) steht, links Hintergrund
                for f in [&f30, &f35] {
                    assert_eq!((col(f, 8), col(f, 32), col(f, 50)), ('b', 'b', 'r'), "{file}");
                }
            }
        }
    }

    /// Aufteilung von `codec_args` in Bild/Ton/Container: Argumente wie zuvor (Reihenfolge inklusive).
    #[test]
    fn codec_args_unchanged_by_split() {
        let p = Project::new("C");
        let s = |f: Format, c: Codec, q: Quality, audio: bool, transparent: bool| {
            let mut s = ExportSettings::from_project(&p);
            (s.format, s.codec, s.quality, s.include_audio, s.transparent) = (f, c, q, audio, transparent);
            codec_args(&s).join(" ")
        };
        assert_eq!(s(Format::Mp4, Codec::H264, Quality::Recommended, true, false), "-c:v libx264 -preset medium -crf 20 -r 30/1 -c:a aac -b:a 256k -movflags +faststart");
        assert_eq!(s(Format::Mov, Codec::Hevc, Quality::Higher, true, false), "-c:v libx265 -preset medium -tag:v hvc1 -crf 19 -r 30/1 -c:a aac -b:a 256k -movflags +faststart");
        assert_eq!(s(Format::Mkv, Codec::H264, Quality::Bitrate(4000), false, false), "-c:v libx264 -preset medium -b:v 4000k -maxrate 4400k -bufsize 8000k -r 30/1");
        assert_eq!(s(Format::WebM, Codec::Vp9, Quality::Lower, true, true), "-c:v libvpx-vp9 -b:v 0 -row-mt 1 -crf 38 -pix_fmt yuva420p -auto-alt-ref 0 -r 30/1 -c:a libopus -b:a 192k");
        assert_eq!(s(Format::Mov, Codec::ProRes, Quality::Recommended, true, false), "-c:v prores_ks -profile:v 3 -pix_fmt yuv422p10le -r 30/1 -c:a pcm_s16le -movflags +faststart");
        assert_eq!(s(Format::Gif, Codec::H264, Quality::Recommended, true, false), "-loop 0");
        assert_eq!(s(Format::M4a, Codec::H264, Quality::Recommended, true, false), "-c:a aac -b:a 256k");
    }

    /// Kleines Projekt: ein Durchgang mit genau dem Aufruf von `build_single` (keine Abschnitte).
    #[test]
    fn small_project_builds_one_pass_as_before() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        std::fs::write(&path, b"x").unwrap();
        let m = MediaRef { id: MediaId::new(), path, kind: MediaKind::Audio, duration: Time::from_secs_f64(60.0), width: 0, height: 0, fps: None, has_audio: true, audio_streams: 1, video_codec: None, audio_codec: None };
        let mut p = Project::new("K");
        p.media.push(m.clone());
        let mut t = Track::new(TrackKind::Audio);
        for i in 0..20 {
            t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Audio }, Time::from_secs_f64(i as f64), Time::from_secs_f64(1.0)));
        }
        p.timeline.tracks.push(t);
        let mut s = ExportSettings::from_project(&p);
        s.format = Format::Wav;
        let out = dir.path().join("o.wav");
        let job = build(&p, &s, &|_| None, &out).unwrap();
        assert_eq!(job.sections(), 0);
        let single = build_single(&in_edit_ranges(&p), &s, &|_| None, &out, Time::ZERO, p.duration()).unwrap();
        assert_eq!(job.args, single.args);
        let g = |j: &Job| std::fs::read_to_string(j.workdir.path().join("graph.txt")).unwrap();
        assert_eq!(g(&job), g(&single));
        assert!(g(&job).contains("alimiter"), "Begrenzer im Durchgang");
    }

    /// Dichter Schnitt im Vorschau-Fenster (300 Clips à 0,1 s in 20 s, lange Pfade): Clips einer
    /// Datei teilen sich Eingänge, die Befehlszeile bleibt klein; mit 300 verschiedenen Dateien
    /// gibt es eine klare Meldung statt eines stillen Fehlstarts.
    #[test]
    fn dense_preview_window_stays_below_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("x".repeat(120)).join("y".repeat(60));
        std::fs::create_dir_all(&deep).unwrap();
        let media = |n: usize| {
            let path = deep.join(format!("datei_{n}.wav"));
            std::fs::write(&path, b"x").unwrap();
            // als lesbar vormerken: Die Prüfung im Hintergrund würde die Attrappe sonst mitten im
            // Aufbau als unlesbar melden und ihre Clips fielen ab dann heraus (Test schwankte)
            let meta = std::fs::metadata(&path).unwrap();
            READABLE.lock().unwrap().get_or_insert_with(Default::default).insert((path.clone(), meta.len(), meta.modified().ok()), Some(true));
            MediaRef { id: MediaId::new(), path, kind: MediaKind::Audio, duration: Time::from_secs_f64(600.0), width: 0, height: 0, fps: None, has_audio: true, audio_streams: 2, video_codec: None, audio_codec: None }
        };
        let dense = |files: usize| {
            let ms: Vec<MediaRef> = (0..files).map(media).collect();
            let mut p = Project::new("Dicht");
            p.media.extend(ms.iter().cloned());
            let mut t = Track::new(TrackKind::Audio);
            for i in 0..300 {
                let m = &ms[i % files]; // 300 Clips à 0,1 s, davon 200 im 20-s-Fenster
                let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Audio }, Time::from_secs_f64(i as f64 * 0.1), Time::from_secs_f64(0.1));
                c.source_in = Time::from_secs_f64(30.0 + i as f64 * 0.37);
                c.audio_stream = Some((i % 2) as u32);
                t.clips.push(c);
            }
            p.timeline.tracks.push(t);
            p
        };
        let p = dense(1);
        let (inputs, graph, _) = audio_window_graph(&p, Time::ZERO, Time::from_secs_f64(20.0)).unwrap();
        let n = inputs.iter().filter(|a| *a == "-i").count();
        assert!(n <= 4, "{n} Eingänge");
        assert!(sections::cmd_len(&inputs) < sections::MAX_CMD);
        let g = graph.join(";\n");
        assert!(g.contains(":a:1]asplit") && g.contains(":a]asplit"), "Tonspur je Clip auch bei gemeinsamen Eingängen");
        assert_eq!(g.matches("atrim=start=").count(), 200, "200 Clips im Fenster");
        // viele verschiedene Dateien: verständlicher Fehler mit Zeit
        let e = audio_window_graph(&dense(300), Time::ZERO, Time::from_secs_f64(20.0)).unwrap_err();
        assert!(matches!(e, ExportError::TooManyClips(_)), "{e}");
    }

    #[test]
    fn shared_inputs_group_nearby_ranges_of_one_file() {
        // Datei 0: 10–11, 12–13 (nah), 100–101 (weit); Datei 1: 10–11
        let r = [(0, 10.0, 11.0), (1, 10.0, 11.0), (0, 100.0, 101.0), (0, 12.0, 13.0)];
        let (of, groups) = graph::share_inputs(&r, 10.0);
        assert_eq!(groups.len(), 3);
        assert_eq!(of[0], of[3]);
        assert_ne!(of[0], of[2]);
        assert_eq!(groups[of[0]], (0, 10.0, 13.0));
    }

    #[test]
    fn lead_skip_only_for_plain_long_clips() {
        let s = Time::from_secs_f64;
        let mut c = Clip::new(ClipContent::Text(TextStyle::default()), s(10.0), s(100.0));
        assert_eq!(graph::lead_skip(&c, s(11.5)), 0.0, "zu nah am Anfang");
        assert_eq!(graph::lead_skip(&c, s(40.7)), 29.0, "ganze Sekunden, ≥ 1 s Vorlauf");
        c.speed = 2.0;
        assert_eq!(graph::lead_skip(&c, s(40.7)), 0.0, "Tempo");
        c.speed = 1.0;
        c.fx.reverse = true;
        assert_eq!(graph::lead_skip(&c, s(40.7)), 0.0, "rückwärts");
    }

    /// Bild-Sprung trifft Quell- und Projektbilder und ganze µs.
    #[test]
    fn video_skip_lands_on_source_and_project_frames() {
        let s = Time::from_secs_f64;
        let c = Clip::new(ClipContent::Text(TextStyle::default()), s(10.0), s(100.0));
        let r = Rational::new;
        let at = |src, prj, t| graph::video_skip(&c, src, prj, s(t));
        assert_eq!(at(Some(r(30, 1)), r(30, 1), 40.7), 29.0);
        assert_eq!(at(Some(r(60000, 2000)), r(30000, 1001), 40.7), 0.0, "Raster 50,05 s");
        assert_eq!(at(Some(r(60, 1)), r(30000, 1001), 71.0), 50.05);
        assert_eq!(at(Some(r(24000, 1001)), r(30000, 1001), 40.7), 28.5285, "57 × 0,5005 s");
        assert_eq!(at(None, r(30, 1), 40.7), 0.0, "Quellrate unbekannt");
    }

    /// Platzprüfung: Die fertige Datei braucht noch einmal so viel wie die Abschnitte – auf
    /// demselben Laufwerk doppelt (1 h ProRes 1080p30 ≈ 100 GB: 110 GB frei reichen nicht).
    #[test]
    fn space_check_counts_the_finished_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ExportSettings::from_project(&Project::new("p"));
        (s.width, s.height, s.fps, s.format, s.codec) = (1920, 1080, Rational::new(30, 1), Format::Mov, Codec::ProRes);
        let need = scratch_bytes(&s, 3600.0);
        assert!((95..115).contains(&(need >> 30)), "{} GB", need >> 30);
        let work = dir.path().join("work");
        assert_eq!(space_needs(need, &work, &dir.path().join("ziel.mov")), vec![(work.clone(), 2 * need)]);
        #[cfg(windows)]
        {
            let other = if dir.path().to_string_lossy().starts_with(['Z', 'z']) { "Y:/x/ziel.mov" } else { "Z:/x/ziel.mov" };
            assert_eq!(space_needs(need, &work, Path::new(other)), vec![(work, need), (PathBuf::from(&other[..4]), need)]);
        }
    }

    /// Effekte, die eigene Zeitstempel vergeben oder Bilder zählen, rechnen ab Clipbeginn;
    /// Ausdrücke mit `t` (Zeitstempel bleiben Clipzeit) dürfen springen.
    #[test]
    fn effects_with_own_time_start_at_the_clip() {
        let has = |id: &str| graph::fx_has_memory(&ClipFx { effects: vec![FxRef::new(id)], ..Default::default() });
        for id in ["motion-blur", "echo", "grain", "vhs", "glitch"] {
            assert!(has(id), "{id}");
        }
        for id in ["zoom-pulse", "shake", "rainbow", "strobe", "flash", "old-film", "sepia"] {
            assert_eq!(has(id), id == "old-film", "{id}");
        }
        assert!(graph::counts_frames("[a]hue=h='n*3'[b]") && graph::counts_frames("geq=lum='N+p(X,Y)'"));
        assert!(!graph::counts_frames("[u1n]eq=brightness='sin(t*3)':eval=frame,noise_x[o]"));
    }

    /// Kaputte Datei: die Vorschau lässt sie aus und meldet sie einmal.
    #[test]
    fn unreadable_audio_is_left_out_and_reported_once() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("kaputt_c2.wav");
        std::fs::write(&bad, b"kein Ton").unwrap();
        assert!(!audio_readable(&bad, true));
        assert!(!audio_readable(&bad, false), "Ergebnis gemerkt");
        let w: Vec<String> = take_audio_warnings().into_iter().filter(|w| w.contains("kaputt_c2.wav")).collect();
        assert_eq!(w.len(), 1, "{w:?}");
    }

    /// Shader-Übergang, durch den eine Abschnittsgrenze läuft (Grenze bei 1,5 s, Fenster
    /// 1,2–1,8 s): beide Abschnitte rechnen ihren Vorlauf, das Ergebnis ist bildgleich mit dem
    /// Durchgang (ohne GPU: der Rückfall, ebenso gleich).
    #[test]
    fn shader_transition_across_section_boundary() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        shader_pack();
        let dir = tempfile::tempdir().unwrap();
        let mut p = shader_project(dir.path(), "t.shader/wipe", false);
        // zweite Spur mit kurzen Clips: zwingt Abschnittsgrenzen alle 0,5 s
        let c = color_video(dir.path(), "c.mp4", "green", 1.0, "320x180");
        let mut t = Track::new(TrackKind::Video);
        for i in 0..6 {
            let mut k = Clip::new(ClipContent::Media { media: c.id, kind: MediaKind::Video }, Time::from_secs_f64(0.5 * i as f64), Time::from_secs_f64(0.5));
            k.transform.scale = 0.2;
            k.transform.x = -100.0;
            t.clips.push(k);
        }
        p.media.push(c);
        p.timeline.tracks.push(t);
        let mut s = ExportSettings::from_project(&p);
        (s.format, s.codec) = (Format::Mov, Codec::ProRes);
        let (one, cut) = (dir.path().join("eins.mov"), dir.path().join("teile.mov"));
        let job = build(&p, &s, &no_fonts, &one).unwrap();
        run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let job = build_limited(&p, &s, &no_fonts, &cut, 3).unwrap();
        let bounds: Vec<f64> = job.sections.iter().map(|x| x.from.as_secs_f64()).collect();
        assert!(bounds.iter().any(|b| *b > 1.2 && *b < 1.8), "Grenze im Übergang: {bounds:?}");
        assert!(job.sections.iter().filter(|x| !x.passes.is_empty()).count() >= 2, "Vorlauf in beiden Abschnitten");
        run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        for n in 0..90 {
            let d = mean_diff(&frame_n(&one, n).data, &frame_n(&cut, n).data);
            assert!(d < 0.05, "Bild {n}: Abweichung {d}");
        }
        if cl_gpu::gpu().is_some() {
            let used = job.sections.iter().filter(|x| std::fs::read_to_string(job.workdir.path().join(&x.graph)).unwrap().contains("movie=shader_")).count();
            assert!(used >= 2, "Shader statt Rückfall in beiden Abschnitten: {used}");
        }
    }

    /// Ton-Start der Vorschau: liegt alles in den Bereichen, wird das Projekt nicht kopiert.
    #[test]
    fn edit_ranges_copy_only_when_needed() {
        let mut p = Project::new("x");
        let mut t = Track::new(TrackKind::Audio);
        let mut c = Clip::new(ClipContent::Text(Default::default()), Time::ZERO, Time::from_secs_f64(1.0));
        c.volume_db = 6.0;
        t.clips.push(c);
        p.timeline.tracks.push(t);
        assert!(matches!(in_edit_ranges(&p), std::borrow::Cow::Borrowed(_)));
        p.timeline.tracks[0].clips[0].volume_db = 1000.0;
        let q = in_edit_ranges(&p);
        assert!(matches!(q, std::borrow::Cow::Owned(_)));
        assert_eq!(q.timeline.tracks[0].clips[0].volume_db, 20.0);
    }

    /// Nur „Treiber zu alt" zählt – keine Karte, fehlende Bibliothek oder andere Fehler nicht.
    #[test]
    fn only_an_outdated_driver_counts() {
        assert!(driver_too_old("[h264_nvenc @ 0x1] Driver does not support the required nvenc API version. Required: 13.0 Found: 12.1"));
        assert!(driver_too_old("[h264_nvenc @ 0x1] The minimum required Nvidia driver for nvenc is 570.0 or newer"));
        assert!(!driver_too_old("[h264_nvenc @ 0x1] No capable devices found"));
        assert!(!driver_too_old("[h264_nvenc @ 0x1] Cannot load nvEncodeAPI64.dll"));
        assert!(!driver_too_old("[h264_amf @ 0x1] DLL amfrt64.dll failed to open"));
        assert!(!driver_too_old("[h264_qsv @ 0x1] Error creating a MFX session: -9"));
        assert!(!driver_too_old(""));
    }

    /// Hinweis nur, wenn NVENC am Treiber scheitert und kein anderer Hardware-Weg da ist.
    #[test]
    fn driver_hint_only_without_other_hardware() {
        use Encoder::*;
        for old in [false, true] {
            for found in [vec![Software], vec![Software, Qsv], vec![Software, Amf], vec![Software, Mf], vec![Software, Nvenc], vec![], vec![Software, Qsv, Amf, Mf]] {
                let want = old && found.iter().all(|e| *e == Software);
                assert_eq!(driver_hint(old, &found), want, "{old} {found:?}");
            }
        }
        assert!(driver_hint(true, &[Software]));
        assert!(!driver_hint(true, &[Software, Mf]));
    }

    /// Automatisch NVENC → QSV → AMF → Media Foundation → Software; ein gewählter Weg vorn, fehlt
    /// er hier, gilt automatisch.
    #[test]
    fn encoder_chain_follows_setting_and_falls_back_to_auto() {
        use Encoder::*;
        let all = [Software, Mf, Amf, Qsv, Nvenc];
        assert_eq!(encoder_chain(&all, "auto"), [Nvenc, Qsv, Amf, Mf, Software]);
        assert_eq!(encoder_chain(&[Software, Mf], "auto"), [Mf, Software]);
        assert_eq!(encoder_chain(&[Software, Mf, Nvenc], ""), [Nvenc, Mf, Software]);
        assert_eq!(encoder_chain(&[], "auto"), [Software]);
        assert_eq!(encoder_chain(&all, "mf"), [Mf, Software]);
        assert_eq!(encoder_chain(&all, "software"), [Software]);
        assert_eq!(encoder_chain(&[], "software"), [Software]);
        // gewählter Weg fehlt (anderer Rechner, Treiber weg) oder unbekannt: automatisch
        assert_eq!(encoder_chain(&[Software, Mf], "nvenc"), [Mf, Software]);
        assert_eq!(encoder_chain(&[Software, Qsv], "quatsch"), [Qsv, Software]);
        for e in Encoder::AUTO {
            assert_eq!(encoder_chain(&all, e.id())[0], e);
        }
    }

    /// Media Foundation: nv12, erzwungene Hardware, GOP 250, Bitrate nach Qualität; eigene
    /// Bitrate ersetzt sie; andere Codecs nicht über diesen Weg.
    #[test]
    fn media_foundation_args() {
        let mut s = ExportSettings::from_project(&Project::new("mf"));
        (s.width, s.height, s.fps, s.format, s.encoder, s.include_audio) = (1920, 1080, Rational::new(60, 1), Format::Mp4, Encoder::Mf, false);
        let args = |s: &ExportSettings| codec_args(s).join(" ");
        s.codec = Codec::H264;
        s.quality = Quality::Recommended;
        assert_eq!(args(&s), "-c:v h264_mf -pix_fmt nv12 -hw_encoding 1 -g 250 -b:v 9953k -r 60/1 -movflags +faststart");
        s.quality = Quality::Lower;
        assert!(args(&s).contains("-b:v 6221k"), "{}", args(&s));
        s.quality = Quality::Higher;
        assert!(args(&s).contains("-b:v 14930k"), "{}", args(&s));
        s.codec = Codec::Hevc;
        s.quality = Quality::Recommended;
        assert!(args(&s).starts_with("-c:v hevc_mf -pix_fmt nv12 -hw_encoding 1 -g 250 -b:v 6967k "), "{}", args(&s));
        s.quality = Quality::Bitrate(8000);
        let a = codec_args(&s);
        assert_eq!(a.iter().filter(|x| *x == "-b:v").count(), 1, "{a:?}");
        assert!(args(&s).contains("-b:v 8000k -maxrate 8800k"), "{}", args(&s));
        // kleine Ausgabe: nie unter 500 kbit/s
        (s.width, s.height, s.fps, s.quality) = (128, 72, Rational::new(30, 1), Quality::Lower);
        assert!(args(&s).contains("-b:v 500k"), "{}", args(&s));
        s.quality = Quality::Recommended;
        for (format, codec) in [(Format::Mp4, Codec::Av1), (Format::WebM, Codec::Vp9), (Format::Mov, Codec::ProRes)] {
            (s.format, s.codec) = (format, codec);
            assert!(!args(&s).contains("_mf"), "{codec:?}: {}", args(&s));
        }
        (s.format, s.codec) = (Format::Gif, Codec::H264);
        assert!(!args(&s).contains("_mf"));
    }

    #[test]
    fn export_preselects_software_unless_a_way_is_chosen() {
        let found = [Encoder::Software, Encoder::Nvenc, Encoder::Mf];
        assert_eq!(export_default(&found, "auto"), Encoder::Software);
        assert_eq!(export_default(&found, ""), Encoder::Software);
        assert_eq!(export_default(&found, Encoder::Nvenc.id()), Encoder::Nvenc);
        assert_eq!(export_default(&found, Encoder::Mf.id()), Encoder::Mf);
        assert_eq!(export_default(&found, Encoder::Qsv.id()), Encoder::Software, "gewählter Weg fehlt hier");
        assert_eq!(export_default(&found, Encoder::Software.id()), Encoder::Software);
    }
}
