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
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("Das Projekt ist leer")]
    Empty,
    #[error("Medium fehlt: {0}")]
    MissingMedia(String),
    #[error("FFmpeg: {0}")]
    Ffmpeg(String),
    #[error("Abgebrochen")]
    Cancelled,
    #[error("E/A-Fehler: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Media(#[from] cl_media::MediaError),
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
        }
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
}

pub(crate) fn f(x: f64) -> String {
    format!("{x:.6}")
}

/// Erzeugt den kompletten Export-Aufruf.
pub fn build(p: &Project, s: &ExportSettings, fonts: FontResolver, out: &Path) -> Result<Job> {
    let total = p.duration();
    if total <= Time::ZERO {
        return Err(ExportError::Empty);
    }
    for m in &p.media {
        if !m.path.is_file() {
            return Err(ExportError::MissingMedia(m.path.display().to_string()));
        }
    }
    let (start, end) = s.range.unwrap_or((Time::ZERO, total));
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
    };
    let mut vlabel = None;
    if !s.format.audio_only() {
        let cur = graph::video_part(&mut ctx, p, end, dur)?;
        if s.format == Format::Gif {
            ctx.graph.push(format!(
                "[{cur}]fps=min(15\\,{}),scale={}:-2:flags=lanczos,split[g0][g1]",
                ctx.fps,
                w.min(720)
            ));
            ctx.graph.push("[g0]palettegen=stats_mode=diff[pal]".into());
            ctx.graph
                .push("[g1][pal]paletteuse=dither=sierra2_4a[vout]".into());
        } else {
            let pf = match (s.alpha(), s.codec) {
                (true, Codec::ProRes) => "yuva444p10le",
                (true, _) => "yuva420p",
                _ => "yuv420p",
            };
            ctx.graph.push(format!("[{cur}]format={pf}[vout]"));
        }
        vlabel = Some("vout");
    }
    let wants_audio = s.format.audio_only() || (s.include_audio && s.format != Format::Gif);
    let alabel = wants_audio.then(|| {
        graph::audio_part(
            p,
            start,
            end,
            &mut ctx.inputs,
            &mut ctx.n_input,
            &mut ctx.graph,
        )
    });

    std::fs::write(workdir.path().join("graph.txt"), ctx.graph.join(";\n"))?;
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
    ];
    args.extend(std::mem::take(&mut ctx.inputs));
    // Marker als Kapitel in die Datei (Player zeigen sie an; gleiche Liste wie die YouTube-Kapitel)
    let chapters = matches!(s.format, Format::Mp4 | Format::Mov | Format::Mkv | Format::WebM | Format::M4a)
        .then(|| ffmetadata(&p.timeline.markers, start, end))
        .flatten();
    if let Some(meta) = chapters {
        std::fs::write(workdir.path().join("chapters.txt"), meta)?;
        args.extend(["-f".into(), "ffmetadata".into(), "-i".into(), "chapters.txt".into()]);
    }
    args.extend(["-/filter_complex".into(), "graph.txt".into()]);
    if let Some(v) = vlabel {
        args.extend(["-map".into(), format!("[{v}]")]);
    }
    if let Some(a) = &alabel {
        args.extend(["-map".into(), format!("[{a}]")]);
    }
    if args.iter().any(|a| a == "chapters.txt") {
        args.extend(["-map_chapters".into(), ctx.n_input.to_string()]);
    }
    args.extend(codec_args(s));
    args.extend(["-t".into(), f(dur.as_secs_f64())]);
    args.push(out.to_string_lossy().into_owned());
    Ok(Job {
        args,
        workdir,
        duration: dur,
    })
}

/// Kapitel (Start relativ zu `start`, Titel) aus den Markern im Bereich `start..end`.
/// YouTube verlangt ein Kapitel bei 0:00: Ein Marker in der ersten Sekunde wird dorthin
/// gezogen, sonst kommt „Intro“ davor.
pub fn chapter_list(markers: &[cl_core::Marker], start: Time, end: Time) -> Vec<(Time, String)> {
    let mut v: Vec<(Time, String)> = markers
        .iter()
        .filter(|m| m.at >= start && m.at < end)
        .map(|m| (m.at - start, if m.name.is_empty() { "Kapitel".into() } else { m.name.clone() }))
        .collect();
    v.sort_by_key(|m| m.0);
    match v.first_mut() {
        Some(f) if f.0 < Time::from_secs_f64(1.0) => f.0 = Time::ZERO,
        _ => v.insert(0, (Time::ZERO, "Intro".into())),
    }
    v
}

/// Kapitel im FFmetadata-Format (siehe [`chapter_list`]); `None` ohne Marker.
fn ffmetadata(markers: &[cl_core::Marker], start: Time, end: Time) -> Option<String> {
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
    let v = chapter_list(markers, start, end);
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

fn codec_args(s: &ExportSettings) -> Vec<String> {
    let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut a = Vec::new();
    match s.format {
        Format::Wav => return v(&["-c:a", "pcm_s16le"]),
        Format::Mp3 => return v(&["-c:a", "libmp3lame", "-b:a", "320k"]),
        Format::M4a => return v(&["-c:a", "aac", "-b:a", "256k"]),
        Format::Flac => return v(&["-c:a", "flac"]),
        Format::Gif => return v(&["-loop", "0"]),
        _ => {}
    }
    let crf = |lo: u32, rec: u32, hi: u32| match s.quality {
        Quality::Lower => lo,
        Quality::Recommended => rec,
        Quality::Higher => hi,
        Quality::Bitrate(_) => rec,
    };
    let codec = if Codec::for_format(s.format).contains(&s.codec) {
        s.codec
    } else {
        Codec::for_format(s.format)[0]
    };
    match (codec, s.encoder) {
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
            v(&["-c:v", "libsvtav1", "-preset", "8", "-crf"])
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
        a.extend(["-b:v".into(), format!("{k}k")]);
    }
    a.extend(["-r".into(), format!("{}/{}", s.fps.num, s.fps.den)]);
    if s.include_audio {
        match (s.format, codec) {
            (Format::WebM, _) => a.extend(v(&["-c:a", "libopus", "-b:a", "192k"])),
            (_, Codec::ProRes) => a.extend(v(&["-c:a", "pcm_s16le"])),
            _ => a.extend(v(&["-c:a", "aac", "-b:a", "256k"])),
        }
    }
    if matches!(s.format, Format::Mp4 | Format::Mov) {
        a.extend(v(&["-movflags", "+faststart"]));
    }
    a
}

/// Läuft einen Job und meldet den Fortschritt (0..1). `cancel` bricht ab.
pub fn run(job: &Job, cancel: Arc<AtomicBool>, mut progress: impl FnMut(f32)) -> Result<()> {
    let mut child: Child = cl_media::ffmpeg_command()?
        .args(&job.args)
        .current_dir(job.workdir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = child.stderr.take().expect("piped");
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut s);
        s
    });
    let total = job.duration.as_secs_f64().max(0.001);
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
        return Err(ExportError::Ffmpeg(
            err.lines().rev().take(4).collect::<Vec<_>>().join(" | "),
        ));
    }
    progress(1.0);
    Ok(())
}

/// Prüft, welche Hardware-Encoder auf diesem Rechner wirklich funktionieren (kurzer Testlauf).
pub fn available_encoders() -> Vec<Encoder> {
    let mut v = vec![Encoder::Software];
    for (enc, name) in [
        (Encoder::Nvenc, "h264_nvenc"),
        (Encoder::Qsv, "h264_qsv"),
        (Encoder::Amf, "h264_amf"),
    ] {
        let ok = cl_media::ffmpeg_command()
            .ok()
            .and_then(|mut c| {
                c.args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=black:s=256x256:d=0.1",
                    "-c:v",
                    name,
                    "-f",
                    "null",
                    "-",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .ok()
            })
            .is_some_and(|s| s.success());
        if ok {
            v.push(enc);
        }
    }
    v
}

/// Ton der Zeitleiste ab `from` als f32le-Stereo mit `rate` Hz auf stdout (für die Vorschau-Wiedergabe).
pub fn spawn_audio_preview(
    p: &Project,
    from: Time,
    rate: u32,
) -> Result<(Child, tempfile::TempDir)> {
    let end = p.duration();
    if end <= from {
        return Err(ExportError::Empty);
    }
    let workdir = tempfile::Builder::new()
        .prefix("cutline-audio-")
        .tempdir()?;
    let mut inputs = Vec::new();
    let mut graph = Vec::new();
    let mut n = 0;
    let label = graph::audio_part(p, from, end, &mut inputs, &mut n, &mut graph);
    std::fs::write(workdir.path().join("graph.txt"), graph.join(";\n"))?;
    let child = cl_media::ffmpeg_command()?
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

/// Textclips als SRT (z. B. für Untertitel-Export).
pub fn to_srt(p: &Project) -> String {
    let ts = |t: Time| {
        let ms = t.as_millis().max(0);
        format!(
            "{:02}:{:02}:{:02},{:03}",
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
            ClipContent::Text(s) => Some((c.start, c.end(), s.text.clone())),
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
        run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        assert!(cl_media::probe(&out).is_ok());
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
    }
}
