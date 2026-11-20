//! Medienzugriff für Cutline.
//!
//! Phase 0 nutzt die FFmpeg-/FFprobe-Programme als Unterprozess. Das ist robust,
//! unterstützt sofort alle FFmpeg-Formate und braucht keine C-Header zum Bauen.
//! Phase 1 ersetzt das durch libav-Bindings mit Hardware-Decode und Zero-Copy nach wgpu.
//! Die öffentliche API (probe / decode_frame / FrameStream) bleibt dabei gleich.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cl_core::{MediaId, MediaKind, MediaRef, Rational, Time};
use serde::Deserialize;

mod anim;
mod proc;
pub use anim::{animation, decode_all as decode_animation, frame_index as animation_frame, Animation};
pub use proc::{cpu_time, limit_threads, lower_priority, raise_priority, suspend};

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("FFmpeg/FFprobe nicht gefunden. Bitte installieren oder CUTLINE_FFMPEG_DIR setzen.")]
    FfmpegMissing,
    #[error("Datei kann nicht gelesen werden: {0}")]
    Unreadable(String),
    #[error("Kein Video- oder Audiostrom gefunden")]
    NoStreams,
    #[error("E/A-Fehler: {0}")]
    Io(#[from] std::io::Error),
    #[error("Unerwartete FFprobe-Ausgabe: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("Abgebrochen")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, MediaError>;

/// Dateiendungen für den Import-Dialog. Erkannt wird später aber per Inhalt (FFprobe),
/// nicht per Endung. Falsch benannte Dateien funktionieren also auch.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "qt", "mkv", "webm", "avi", "mxf", "mts", "m2ts", "m2t", "ts", "mpg",
    "mpeg", "mpe", "m2v", "vob", "flv", "f4v", "wmv", "asf", "3gp", "3g2", "ogv", "rm", "rmvb",
    "dv", "y4m", "nut", "ivf", "mpeg4", "hevc", "h264", "264", "265", "av1", "obu", "braw", "r3d",
];
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "bwf", "aif", "aiff", "aifc", "mp3", "aac", "m4a", "m4b", "flac", "alac", "opus", "ogg",
    "oga", "spx", "wma", "ac3", "eac3", "dts", "caf", "w64", "amr", "ape", "wv", "mka", "mp2",
];
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jfif", "png", "apng", "webp", "avif", "heic", "heif", "jxl", "tif", "tiff",
    "bmp", "gif", "tga", "psd", "exr", "hdr", "dpx", "svg", "ico", "qoi", "dng", "cr2", "cr3",
    "nef", "arw", "raf", "orf", "rw2",
];

pub fn all_media_extensions() -> Vec<&'static str> {
    VIDEO_EXTENSIONS
        .iter()
        .chain(AUDIO_EXTENSIONS)
        .chain(IMAGE_EXTENSIONS)
        .copied()
        .collect()
}

fn tool(name: &str) -> Result<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    if let Some(dir) = std::env::var_os("CUTLINE_FFMPEG_DIR") {
        let p = PathBuf::from(dir).join(&exe);
        if p.is_file() {
            return Ok(p);
        }
    }
    // Mitgeliefertes FFmpeg im Release-Paket: <exe-Ordner>/ffmpeg/
    if let Some(p) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("ffmpeg").join(&exe)))
        .filter(|p| p.is_file())
    {
        return Ok(p);
    }
    let path = std::env::var_os("PATH").ok_or(MediaError::FfmpegMissing)?;
    std::env::split_paths(&path)
        .map(|d| d.join(&exe))
        .find(|p| p.is_file())
        .ok_or(MediaError::FfmpegMissing)
}

/// FFmpeg-Befehl (ohne Konsolenfenster) für andere Crates, z. B. den Export. Der gestartete
/// Prozess endet mit der App, auch wenn sie hart beendet wird (Windows: Job-Objekt; Linux: endet
/// mit dem startenden Thread, siehe `proc::prepare`).
pub fn ffmpeg_command() -> Result<Command> {
    command("ffmpeg")
}

/// FFmpeg-Befehl eines Wiedergabe-Prozesses ohne Hardware-Decodierung (Ton-Fenster): Threads
/// begrenzt. Nach dem Start unbedingt [`raise_priority`] aufrufen: erst das ordnet ihn dem Job zu.
pub fn playback_command() -> Result<Command> {
    let mut cmd = command_with("ffmpeg", false)?;
    limit_threads(&mut cmd, false);
    Ok(cmd)
}

/// FFprobe-Befehl (ohne Konsolenfenster).
pub fn ffprobe_command() -> Result<Command> {
    command("ffprobe")
}

pub fn ffmpeg_available() -> bool {
    tool("ffmpeg").is_ok() && tool("ffprobe").is_ok()
}

fn command(name: &str) -> Result<Command> {
    command_with(name, true)
}

/// `adopt`: der Wächter sucht das gestartete Kind und ordnet es dem Job zu (Windows); `false` nur,
/// wenn danach [`raise_priority`] es direkt zuordnet (Wiedergabe: keine Prozess-Suche je Start).
fn command_with(name: &str, adopt: bool) -> Result<Command> {
    let mut cmd = Command::new(tool(name)?);
    proc::prepare(&mut cmd, adopt);
    Ok(cmd)
}

#[derive(Deserialize)]
struct ProbeOut {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}
#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    duration: Option<String>,
    nb_frames: Option<String>,
    #[serde(default)]
    disposition: Disposition,
    #[serde(default)]
    side_data_list: Vec<SideData>,
    tags: Option<StreamTags>,
}
#[derive(Deserialize)]
struct SideData {
    rotation: Option<f64>,
}
#[derive(Deserialize)]
struct StreamTags {
    rotate: Option<String>,
}
#[derive(Deserialize, Default)]
struct Disposition {
    #[serde(default)]
    attached_pic: u8,
}
#[derive(Deserialize)]
struct ProbeFormat {
    format_name: Option<String>,
    duration: Option<String>,
}

const IMAGE_CODECS: &[&str] = &[
    "png", "mjpeg", "jpegls", "webp", "bmp", "tiff", "gif", "targa", "psd", "exr", "dpx",
    "jpeg2000", "qoi", "hdr", "jpegxl", "svg",
];

/// Liest Metadaten einer Mediendatei.
pub fn probe(path: &Path) -> Result<MediaRef> {
    let out = command("ffprobe")?
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(MediaError::Unreadable(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let p: ProbeOut = serde_json::from_slice(&out.stdout)?;
    let video = p
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video") && s.disposition.attached_pic == 0);
    let audio = p
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("audio"));
    if video.is_none() && audio.is_none() {
        return Err(MediaError::NoStreams);
    }
    let fmt_name = p
        .format
        .as_ref()
        .and_then(|f| f.format_name.clone())
        .unwrap_or_default();
    let duration = p
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .or_else(|| video.and_then(|v| v.duration.as_deref()))
        .and_then(|d| d.parse::<f64>().ok())
        .map(Time::from_secs_f64)
        .unwrap_or(Time::ZERO);

    let is_image = video.is_some_and(|v| {
        let codec = v.codec_name.as_deref().unwrap_or("");
        let single = v
            .nb_frames
            .as_deref()
            .map_or(true, |n| n == "1" || n == "0");
        (fmt_name.contains("image2") || fmt_name.ends_with("_pipe") || fmt_name == "svg")
            || (IMAGE_CODECS.contains(&codec)
                && single
                && duration <= Time::from_millis(100)
                && audio.is_none())
    }) || fmt_name.contains("gif")
        // bewegte Bilder laufen als Bild-Clip (siehe [`animation`]), nicht als Video
        || fmt_name == "apng";

    let kind = match (video, is_image) {
        (Some(_), true) => MediaKind::Image,
        (Some(_), false) => MediaKind::Video,
        (None, _) => MediaKind::Audio,
    };
    let fps = video.and_then(|v| {
        [v.avg_frame_rate.as_deref(), v.r_frame_rate.as_deref()]
            .into_iter()
            .flatten()
            .filter_map(Rational::parse)
            .find(|r| r.num > 0)
    });

    // Hochkant-Handyvideos: gespeichert quer mit ±90°-Drehung, FFmpeg dreht beim Dekodieren
    let rotation = video
        .and_then(|v| {
            v.side_data_list
                .iter()
                .find_map(|d| d.rotation)
                .or_else(|| v.tags.as_ref()?.rotate.as_deref()?.trim().parse().ok())
        })
        .unwrap_or(0.0);
    let (mut width, mut height) = (
        video.and_then(|v| v.width).unwrap_or(0),
        video.and_then(|v| v.height).unwrap_or(0),
    );
    if (rotation.round() as i64).rem_euclid(180) == 90 {
        std::mem::swap(&mut width, &mut height);
    }

    Ok(MediaRef {
        id: MediaId::new(),
        path: path.to_path_buf(),
        kind,
        duration: if kind == MediaKind::Image {
            Time::ZERO
        } else {
            duration
        },
        width,
        height,
        fps: if kind == MediaKind::Video { fps } else { None },
        has_audio: audio.is_some(),
        audio_streams: p.streams.iter().filter(|s| s.codec_type.as_deref() == Some("audio")).count() as u32,
        video_codec: video.and_then(|v| v.codec_name.clone()),
        audio_codec: audio.and_then(|a| a.codec_name.clone()),
    })
}

/// RGBA8-Bild (unpremultiplied).
#[derive(Clone)]
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for RgbaFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RgbaFrame({}x{})", self.width, self.height)
    }
}

/// Zielgröße, die in `max_w`×`max_h` passt und das Seitenverhältnis hält (gerade Werte).
pub fn fit_size(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 {
        return (max_w.max(2) & !1, max_h.max(2) & !1);
    }
    let s = (max_w as f64 / src_w as f64).min(max_h as f64 / src_h as f64);
    let w = ((src_w as f64 * s).round() as u32).max(2) & !1;
    let h = ((src_h as f64 * s).round() as u32).max(2) & !1;
    (w, h)
}

/// Dekodiert ein einzelnes Bild an Quellzeit `at`, skaliert auf `w`×`h`.
pub fn decode_frame(path: &Path, at: Time, w: u32, h: u32) -> Result<RgbaFrame> {
    decode_frame_fx(path, at, w, h, None)
}

/// Wie [`decode_frame`], optional mit Filtergraph (`[0:v]` → `[out]`, liefert `w`×`h` RGBA).
pub fn decode_frame_fx(
    path: &Path,
    at: Time,
    w: u32,
    h: u32,
    graph: Option<&str>,
) -> Result<RgbaFrame> {
    decode_frame_opts(path, at, w, h, graph, false, None).map(|(f, _)| f)
}

/// Wie ein Einzelbild entstand (für Messpunkte).
#[derive(Debug, Clone, Copy, Default)]
pub struct DecodeInfo {
    /// mit Hardware-Decodierung
    pub hw: bool,
    /// CPU-Zeit des FFmpeg-Prozesses (nicht auf jedem System ermittelbar)
    pub cpu: Option<Duration>,
}

/// Abbruch eines laufenden Einzelbild-Auftrags von einem anderen Thread aus: beendet den
/// FFmpeg sofort, der Auftrag endet mit [`MediaError::Cancelled`].
#[derive(Default)]
pub struct KillSwitch(Mutex<(bool, Option<Child>)>);

impl KillSwitch {
    pub fn kill(&self) {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        g.0 = true;
        if let Some(c) = g.1.as_mut() {
            let _ = c.kill();
        }
    }
    pub fn is_killed(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).0
    }
    fn hold(&self, mut c: Child) -> Result<()> {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 {
            let _ = c.kill();
            let _ = c.wait();
            return Err(MediaError::Cancelled);
        }
        g.1 = Some(c);
        Ok(())
    }
    fn release(&self) -> Option<Child> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).1.take()
    }
}

/// Einzelbild mit Optionen. `hw`: Hardware-Decodierung versuchen; scheitert sie, kommt das Bild
/// aus der Software, und die Datei wird für den Rest der Sitzung nur noch in Software gelesen.
/// `kill`: Auftrag von außen abbrechen.
pub fn decode_frame_opts(
    path: &Path,
    at: Time,
    w: u32,
    h: u32,
    graph: Option<&str>,
    hw: bool,
    kill: Option<&KillSwitch>,
) -> Result<(RgbaFrame, DecodeInfo)> {
    if let Some(slot) = if hw { HwSlot::acquire(path) } else { None } {
        match decode_once(path, at, w, h, graph, Some(slot.accel), kill) {
            Ok((f, cpu)) => return Ok((f, DecodeInfo { hw: true, cpu })),
            Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
            Err(_) => {
                drop(slot);
                let (f, cpu) = decode_once(path, at, w, h, graph, None, kill)?;
                // nur merken, wenn Software an derselben Stelle ein Bild hat
                hw_failed(path);
                return Ok((f, DecodeInfo { hw: false, cpu }));
            }
        }
    }
    decode_once(path, at, w, h, graph, None, kill).map(|(f, cpu)| (f, DecodeInfo { hw: false, cpu }))
}

/// `n` aufeinanderfolgende Bilder ab Quellzeit `from` in einem FFmpeg (Software), skaliert auf
/// `w`×`h`: Fenster für das Rückwärts-Abspielen. Bild `k` ist dasselbe wie [`decode_frame`] beim
/// `k`-ten Bild nach `from`; nur die mit `keep(k)` bleiben im Speicher (Nummer, Bild). Am
/// Dateiende weniger, kein einziges = Fehler.
pub fn decode_frames(path: &Path, from: Time, n: u32, w: u32, h: u32, keep: &dyn Fn(u32) -> bool, kill: Option<&KillSwitch>) -> Result<Vec<(u32, RgbaFrame)>> {
    let (frames, _) = decode_raw(path, from, n.max(1), w, h, None, None, keep, kill)?;
    Ok(frames.into_iter().map(|(k, data)| (k, RgbaFrame { width: w, height: h, data })).collect())
}

fn decode_once(
    path: &Path,
    at: Time,
    w: u32,
    h: u32,
    graph: Option<&str>,
    accel: Option<&str>,
    kill: Option<&KillSwitch>,
) -> Result<(RgbaFrame, Option<Duration>)> {
    let (mut f, cpu) = decode_raw(path, at, 1, w, h, graph, accel, &|_| true, kill)?;
    Ok((RgbaFrame { width: w, height: h, data: f.remove(0).1 }, cpu))
}

/// Rohe RGBA-Bytes von bis zu `n` Bildern ab `at` (mindestens eins gelesen, sonst Fehler), nur die
/// mit `keep(k)` behalten
#[allow(clippy::too_many_arguments)]
fn decode_raw(
    path: &Path,
    at: Time,
    n: u32,
    w: u32,
    h: u32,
    graph: Option<&str>,
    accel: Option<&str>,
    keep: &dyn Fn(u32) -> bool,
    kill: Option<&KillSwitch>,
) -> Result<(Vec<(u32, Vec<u8>)>, Option<Duration>)> {
    let mut cmd = command_with("ffmpeg", kill.is_none())?;
    cmd.args(["-v", "error", "-nostdin"]);
    if kill.is_some() {
        limit_threads(&mut cmd, accel.is_some());
    }
    if let Some(a) = accel {
        cmd.args(["-hwaccel", a]);
    }
    cmd.args(["-ss", &format!("{:.6}", at.as_secs_f64().max(0.0))])
        .arg("-i")
        .arg(path);
    match graph {
        Some(g) => graph_arg(&mut cmd, g)?.args(["-map", "[out]"]),
        None => cmd.args(["-vf", &format!("scale={w}:{h}:flags=bilinear,format=rgba")]),
    };
    let mut child = cmd
        .args(["-frames:v", &n.to_string(), "-an", "-sn", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Einzelbild des Players (mit Abbruch-Schalter): vor anderer Last wie die Ströme
    if kill.is_some() {
        raise_priority(&child);
    }
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    // stderr nebenher lesen: ein voller Puffer würde FFmpeg anhalten
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let local = KillSwitch::default();
    let ks = kill.unwrap_or(&local);
    ks.hold(child)?;
    let need = (w * h * 4) as usize;
    let (mut frames, mut read, mut buf) = (Vec::new(), 0u32, vec![0u8; need]);
    while read < n && out.read_exact(&mut buf).is_ok() {
        if keep(read) {
            frames.push((read, std::mem::replace(&mut buf, vec![0u8; need])));
        }
        read += 1;
    }
    let mut child = ks.release();
    if let Some(c) = child.as_mut() {
        let _ = c.wait();
    }
    let cpu = child.as_ref().and_then(cpu_time);
    let err = err.join().unwrap_or_default();
    if ks.is_killed() {
        return Err(MediaError::Cancelled);
    }
    if read == 0 {
        let err = err.trim().to_string();
        // ohne Fehlertext: an dieser Stelle gibt es kein Bild (z. B. hinter dem letzten)
        return Err(MediaError::Unreadable(if err.is_empty() {
            format!("kein Bild bei {:.3} s in {}", at.as_secs_f64(), path.display())
        } else {
            err
        }));
    }
    Ok((frames, cpu))
}

/// Höchstzahl gleichzeitiger Hardware-Decoder (NVDEC-Sitzungen u. Ä. sind begrenzt); jeder
/// weitere Auftrag läuft in Software.
pub const MAX_HW_DECODERS: usize = 3;
static HW_ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// `-hwaccel` dieses Systems: Windows D3D11VA, macOS VideoToolbox, sonst keiner (ungeprüft).
/// `CUTLINE_NO_HWDEC` schaltet die Hardware-Decodierung der Vorschau ab.
pub fn hwaccel() -> Option<&'static str> {
    if std::env::var_os("CUTLINE_NO_HWDEC").is_some() {
        return None;
    }
    if cfg!(windows) {
        Some("d3d11va")
    } else if cfg!(target_os = "macos") {
        Some("videotoolbox")
    } else {
        None
    }
}

fn hw_broken() -> std::sync::MutexGuard<'static, HashSet<PathBuf>> {
    static B: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    B.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

fn hw_failed(path: &Path) {
    hw_broken().insert(path.to_path_buf());
}

/// Ist der Hardware-Weg für diese Datei in dieser Sitzung gescheitert?
pub fn hw_decode_failed(path: &Path) -> bool {
    hw_broken().contains(path)
}

/// Einen Platz unter `max` belegen (zählt hoch, wenn frei).
fn take_slot(n: &AtomicUsize, max: usize) -> bool {
    let mut v = n.load(Ordering::Acquire);
    while v < max {
        match n.compare_exchange_weak(v, v + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(x) => v = x,
        }
    }
    false
}

/// Belegter Hardware-Decoder; gibt den Platz beim Verwerfen frei.
struct HwSlot {
    accel: &'static str,
}

impl HwSlot {
    fn acquire(path: &Path) -> Option<Self> {
        let accel = hwaccel()?;
        if hw_decode_failed(path) || !take_slot(&HW_ACTIVE, MAX_HW_DECODERS) {
            return None;
        }
        Some(Self { accel })
    }
}

impl Drop for HwSlot {
    fn drop(&mut self) {
        HW_ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Spitzenpegel (0..1) für die Waveform-Anzeige, `per_sec` Werte pro Sekunde.
pub fn waveform(path: &Path, per_sec: u32) -> Result<Vec<f32>> {
    waveform_head(path, per_sec, None)
}

/// Wie [`waveform`], aber nur die ersten `max_secs` Sekunden (stundenlange Dateien: Zeit und Speicher).
pub fn waveform_head(path: &Path, per_sec: u32, max_secs: Option<f64>) -> Result<Vec<f32>> {
    waveform_of(path, per_sec, max_secs, 0)
}

/// Wie [`waveform`] für die Tonspur `stream` der Datei (0 = erste).
pub fn waveform_stream(path: &Path, per_sec: u32, stream: u32) -> Result<Vec<f32>> {
    waveform_of(path, per_sec, None, stream)
}

fn waveform_of(path: &Path, per_sec: u32, max_secs: Option<f64>, stream: u32) -> Result<Vec<f32>> {
    const RATE: u32 = 8000;
    let mut cmd = command("ffmpeg")?;
    cmd.args(["-v", "error", "-nostdin", "-i"]).arg(path);
    if stream > 0 {
        cmd.args(["-map", &format!("0:a:{stream}")]);
    }
    if let Some(t) = max_secs {
        cmd.args(["-t", &format!("{t:.3}")]);
    }
    let out = cmd
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            &RATE.to_string(),
            "-f",
            "s16le",
            "-",
        ])
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(MediaError::Unreadable(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let bucket = (RATE / per_sec.max(1)).max(1) as usize;
    Ok(out
        .stdout
        .chunks_exact(2)
        .map(|b| (i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).abs())
        .collect::<Vec<_>>()
        .chunks(bucket)
        .map(|c| c.iter().copied().fold(0.0, f32::max))
        .collect())
}

/// Ist bei einer Stereo-Datei nur ein Kanal belegt? Gibt den Index des stummen Kanals zurück
/// (0 = links, 1 = rechts). Prüft die ersten 90 s, Ergebnis wird je Datei zwischengespeichert.
pub fn silent_channel(path: &Path) -> Option<u8> {
    silent_channel_opt(path, true)
}

/// Wie [`silent_channel`]; mit `wait = false` wird nie auf FFmpeg gewartet (für die
/// Wiedergabe): Ist das Ergebnis noch nicht bekannt, wird es im Hintergrund ermittelt und
/// dieses Mal `None` geliefert.
pub fn silent_channel_opt(path: &Path, wait: bool) -> Option<u8> {
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashMap<PathBuf, Option<u8>>>> = Mutex::new(None);
    static PENDING: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
    if let Some(r) = CACHE.lock().ok()?.get_or_insert_with(HashMap::new).get(path) {
        return *r;
    }
    if !wait {
        if PENDING.lock().ok()?.get_or_insert_with(HashSet::new).insert(path.to_path_buf()) {
            let p = path.to_path_buf();
            std::thread::spawn(move || {
                silent_channel_opt(&p, true);
            });
        }
        return None;
    }
    let r = (|| {
        let out = command("ffmpeg")
            .ok()?
            .args(["-v", "info", "-nostdin", "-t", "90", "-i"])
            .arg(path)
            .args([
                "-map",
                "0:a:0",
                "-af",
                "astats=measure_overall=none:measure_perchannel=Peak_level",
                "-f",
                "null",
                "-",
            ])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        let err = String::from_utf8_lossy(&out.stderr);
        let peaks: Vec<f64> = err
            .lines()
            .filter_map(|l| l.split("Peak level dB:").nth(1))
            .map(|v| v.trim().parse::<f64>().unwrap_or(f64::NEG_INFINITY))
            .collect();
        match peaks[..] {
            [l, r] if l < -70.0 && r > -50.0 => Some(0),
            [l, r] if r < -70.0 && l > -50.0 => Some(1),
            _ => None,
        }
    })();
    if let Ok(mut c) = CACHE.lock() {
        c.get_or_insert_with(HashMap::new).insert(path.to_path_buf(), r);
    }
    r
}

/// Fortlaufender Frame-Strom für die Wiedergabe (eigener Thread, begrenzter Puffer).
/// Verwerfen blockiert nicht: Der Prozess wird nur beendet, abgeholt wird er im Lese-Thread.
pub struct FrameStream {
    shared: Arc<StreamShared>,
    rx: Receiver<RgbaFrame>,
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub start: Time,
}

struct StreamShared {
    /// laufende Prozesse: Software und ggf. Hardware (siehe [`FrameStream::open_opts`])
    children: Mutex<Vec<Child>>,
    stop: AtomicBool,
    /// Bild, ab dem die Hardware lieferte (`u64::MAX`: nie)
    hw_from: AtomicU64,
}

/// Filtergraph als Argument. Lange Graphen (Bahnen von Zensur-Flächen) gehen über eine Datei:
/// die Windows-Befehlszeile fasst nur 32 767 Zeichen. Die Datei heißt nach dem Inhalt; dabei
/// werden solche Dateien, die älter als eine Stunde sind, aufgeräumt (FFmpeg liest sie beim Start).
fn graph_arg<'a>(cmd: &'a mut Command, g: &str) -> Result<&'a mut Command> {
    if g.len() < 16_000 {
        return Ok(cmd.args(["-filter_complex", g]));
    }
    cleanup_graphs();
    let file = graph_file(&std::env::temp_dir(), g)?;
    Ok(cmd.arg("-/filter_complex").arg(file))
}

/// Datei für den Graphen `g` in `dir`. Gleicher Name = gleicher Inhalt: Eine vorhandene Datei
/// wird nicht neu geschrieben (ein anderer Dekoder liest sie vielleicht gerade), nur ihre Zeit
/// erneuert, damit das Aufräumen sie nicht erwischt. Neu entsteht sie über eine temporäre Datei
/// mit Umbenennen – nie halb geschrieben sichtbar.
fn graph_file(dir: &Path, g: &str) -> Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    static N: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    g.hash(&mut h);
    let file = dir.join(format!("cutline-graph-{:016x}.txt", h.finish()));
    if let Ok(f) = std::fs::File::options().append(true).open(&file) {
        let _ = f.set_modified(std::time::SystemTime::now());
        return Ok(file);
    }
    let tmp = dir.join(format!("cutline-graph-{}-{}.tmp", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&tmp, g)?;
    // scheitert unter Windows, wenn ein gleichzeitiger Aufruf sie schon angelegt hat und FFmpeg liest
    if let Err(e) = std::fs::rename(&tmp, &file) {
        let _ = std::fs::remove_file(&tmp);
        if !file.exists() {
            return Err(e.into());
        }
    }
    Ok(file)
}

/// Graph-Dateien (und Reste davon) im Temp-Ordner entfernen, die älter als eine Stunde sind.
pub fn cleanup_graphs() {
    for e in std::fs::read_dir(std::env::temp_dir()).into_iter().flatten().flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok());
        if e.file_name().to_string_lossy().starts_with("cutline-graph-") && old.is_some_and(|d| d.as_secs() > 3600) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn spawn_stream(
    path: &Path,
    start: Time,
    (w, h): (u32, u32),
    fps: Rational,
    graph: Option<&str>,
    accel: Option<&str>,
) -> Result<Child> {
    let mut cmd = command_with("ffmpeg", false)?;
    cmd.args(["-v", "error", "-nostdin"]);
    limit_threads(&mut cmd, accel.is_some());
    if let Some(a) = accel {
        cmd.args(["-hwaccel", a]);
    }
    cmd.args(["-ss", &format!("{:.6}", start.as_secs_f64().max(0.0))])
        .arg("-i")
        .arg(path)
        .args(["-an", "-sn"]);
    match graph {
        Some(g) => graph_arg(&mut cmd, g)?.args(["-map", "[out]"]),
        None => cmd.args([
            "-vf",
            &format!("fps={}/{},scale={w}:{h}:flags=bilinear,format=rgba", fps.num, fps.den),
        ]),
    };
    // Bilder unverändert weitergeben (das `fps` im Graphen liefert schon das Raster). Nicht `-r`:
    // FFmpeg 7 vergaß zwar sonst die Bildrate nach `setpts` (25 Bilder/s), aber mit `-r` füllt
    // FFmpeg 8 bei einem Graphen, dessen Zeit mitten im Clip beginnt (`setpts=…+t`, Effekte und
    // Tempo), die Zeit davor mit Kopien des ersten Bildes – das Bild stand so viele Sekunden.
    Ok(cmd
        .args(["-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?)
}

/// Decodierung eines Wiedergabe-Stroms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hw {
    Off,
    /// Hardware von Anfang an (ohne ein Bild: Software)
    On,
    /// Software zuerst (erstes Bild schneller), die Hardware übernimmt nahtlos; nur für Quellen,
    /// die die Software kurz mitdecodieren kann (sonst bremst sie die Hardware aus)
    AfterSoftware,
}

/// Hardware-Strom, der so lange nicht aufholt: aufgeben, nur noch Software
const HW_TAKEOVER_MAX: Duration = Duration::from_secs(4);

/// Liest Bilder (`len` Bytes) aus `out` in einen Kanal (`ahead` voraus); endet mit dem Prozess.
fn read_frames(mut out: std::process::ChildStdout, len: usize, ahead: usize) -> Receiver<Vec<u8>> {
    let (tx, rx) = sync_channel(ahead);
    let _ = std::thread::Builder::new().name("cl-frame-read".into()).spawn(move || loop {
        let mut buf = vec![0u8; len];
        if out.read_exact(&mut buf).is_err() || tx.send(buf).is_err() {
            return;
        }
    });
    rx
}

enum HwCatch {
    /// Bild Nummer `n` (das nächste fällige): ab hier liefert die Hardware
    Frame(Vec<u8>),
    Wait,
    Gone,
}

/// Bilder der Hardware vor Nummer `n` verwerfen (Software hat sie schon geliefert).
fn hw_catch_up(rx: &Receiver<Vec<u8>>, read: &mut u64, n: u64) -> HwCatch {
    loop {
        match rx.try_recv() {
            Ok(b) => {
                *read += 1;
                if *read > n {
                    return HwCatch::Frame(b);
                }
            }
            Err(TryRecvError::Empty) => return HwCatch::Wait,
            Err(TryRecvError::Disconnected) => return HwCatch::Gone,
        }
    }
}

/// Nächstes Bild der Hardware ab Nummer `n` (blockierend), `None` am Ende.
fn hw_next(rx: &Receiver<Vec<u8>>, read: &mut u64, n: u64) -> Option<Vec<u8>> {
    loop {
        let b = rx.recv().ok()?;
        *read += 1;
        if *read > n {
            return Some(b);
        }
    }
}

impl FrameStream {
    pub fn open(path: &Path, start: Time, w: u32, h: u32, fps: Rational) -> Result<Self> {
        Self::open_fx(path, start, w, h, fps, None)
    }

    /// Strom mit eigenem Filtergraph (`[0:v]` → `[out]`, muss `fps`, Größe und RGBA selbst setzen).
    pub fn open_fx(
        path: &Path,
        start: Time,
        w: u32,
        h: u32,
        fps: Rational,
        graph: Option<&str>,
    ) -> Result<Self> {
        Self::open_opts(path, start, w, h, fps, graph, Hw::Off)
    }

    /// Wie [`FrameStream::open_fx`]; `hw`: Hardware-Decodierung (höchstens [`MAX_HW_DECODERS`]
    /// gleichzeitig), siehe [`Hw`]. Ein Hardware-Decoder braucht bis zum ersten Bild 0,3–0,8 s
    /// länger (gemessen FFmpeg 8.1/D3D11VA, Suche ab dem Keyframe); mit [`Hw::AfterSoftware`]
    /// laufen beide ab derselben Stelle, und sobald die Hardware das nächste fällige Bild hat,
    /// übernimmt sie und die Software endet. Liefert die Hardware gar nichts, bleibt es bei
    /// Software (für den Rest der Sitzung, siehe `hw_failed`).
    pub fn open_opts(
        path: &Path,
        start: Time,
        w: u32,
        h: u32,
        fps: Rational,
        graph: Option<&str>,
        hw: Hw,
    ) -> Result<Self> {
        let slot = if hw == Hw::Off { None } else { HwSlot::acquire(path) };
        let spawn = {
            let (path, graph) = (path.to_path_buf(), graph.map(str::to_string));
            move |accel: Option<&str>| -> Result<Child> {
                let c = spawn_stream(&path, start, (w, h), fps, graph.as_deref(), accel)?;
                // Wiedergabe: vor anderer Last (siehe `raise_priority`)
                raise_priority(&c);
                Ok(c)
            }
        };
        let frame_len = (w * h * 4) as usize;
        let mut children = Vec::new();
        let mut start_one = |accel: Option<&str>, ahead: usize| -> Result<(u32, Receiver<Vec<u8>>)> {
            let mut c = spawn(accel)?;
            let r = (c.id(), read_frames(c.stdout.take().expect("piped"), frame_len, ahead));
            children.push(c);
            Ok(r)
        };
        let sw = if slot.is_none() || hw == Hw::AfterSoftware { Some(start_one(None, 1)?) } else { None };
        let hw_proc = match &slot {
            // ohne Software-Strom muss die Hardware starten, sonst nur versuchen
            Some(s) if sw.is_none() => Some(start_one(Some(s.accel), 2)?),
            Some(s) => start_one(Some(s.accel), 2).ok(),
            None => None,
        };
        let mut slot = slot.filter(|_| hw_proc.is_some());
        let hw = hw_proc;
        let shared = Arc::new(StreamShared {
            children: Mutex::new(children),
            stop: AtomicBool::new(false),
            hw_from: AtomicU64::new(u64::MAX),
        });
        let (tx, rx) = sync_channel::<RgbaFrame>(6);
        let sh = shared.clone();
        let path = path.to_path_buf();
        std::thread::Builder::new()
            .name("cl-frame-stream".into())
            .spawn(move || {
                let send = |data: Vec<u8>| tx.send(RgbaFrame { width: w, height: h, data }).is_ok();
                let end = |pid: u32| {
                    let mut g = sh.children.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(i) = g.iter().position(|c| c.id() == pid) {
                        let mut c = g.remove(i);
                        let _ = c.kill();
                        let _ = c.wait();
                    }
                };
                let (mut sw, mut hw) = (sw, hw);
                // gesendete Bilder, von der Hardware gelesene Bilder
                let (mut n, mut hw_n) = (0u64, 0u64);
                let opened = std::time::Instant::now();
                let mut fallback = false;
                while !sh.stop.load(Ordering::Acquire) {
                    let catch = match (&sw, &hw) {
                        (Some(_), Some((_, h))) => Some(hw_catch_up(h, &mut hw_n, n)),
                        _ => None,
                    };
                    match catch {
                        Some(HwCatch::Frame(buf)) => {
                            if let Some((pid, _)) = sw.take() {
                                end(pid);
                            }
                            sh.hw_from.store(n, Ordering::Release);
                            if !send(buf) {
                                break;
                            }
                            n += 1;
                            continue;
                        }
                        Some(HwCatch::Wait) if n == 0 || opened.elapsed() < HW_TAKEOVER_MAX => {}
                        // ohne ein einziges Bild zu Ende bzw. holt nicht auf: nur noch Software
                        Some(c) => {
                            if matches!(c, HwCatch::Gone) && hw_n == 0 && n > 0 {
                                hw_failed(&path);
                            }
                            if let Some((pid, _)) = hw.take() {
                                end(pid);
                            }
                            slot = None;
                        }
                        None => {}
                    }
                    let next = match (&sw, &hw) {
                        (Some((_, s)), Some(_)) => match s.recv_timeout(Duration::from_millis(5)) {
                            Ok(b) => Some(b),
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                            // Software zu Ende: weiter mit der Hardware (ab dem nächsten Bild)
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                sw = None;
                                continue;
                            }
                        },
                        (Some((_, s)), None) => s.recv().ok(),
                        (None, Some((_, h))) => hw_next(h, &mut hw_n, n),
                        (None, None) => None,
                    };
                    // nur Hardware und kein einziges Bild: in Software noch einmal
                    if next.is_none() && n == 0 && hw.is_some() && sw.is_none() && !fallback && !sh.stop.load(Ordering::Acquire) {
                        fallback = true;
                        if let Some((pid, _)) = hw.take() {
                            end(pid);
                        }
                        slot = None;
                        match spawn(None) {
                            Ok(mut c) => {
                                sw = Some((c.id(), read_frames(c.stdout.take().expect("piped"), frame_len, 1)));
                                sh.children.lock().unwrap_or_else(|e| e.into_inner()).push(c);
                                continue;
                            }
                            Err(_) => break,
                        }
                    }
                    if fallback && n == 0 && next.is_some() {
                        hw_failed(&path);
                    }
                    let Some(buf) = next else { break };
                    if !send(buf) {
                        break;
                    }
                    n += 1;
                }
                drop(slot);
                // Aufräumen hier statt im UI-Thread
                let cs = std::mem::take(&mut *sh.children.lock().unwrap_or_else(|e| e.into_inner()));
                for mut c in cs {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            })?;
        Ok(Self {
            shared,
            rx,
            width: w,
            height: h,
            fps,
            start,
        })
    }

    /// Bild, ab dem die Hardware lieferte (`None`: bisher nur Software)
    pub fn hw_from(&self) -> Option<u64> {
        Some(self.shared.hw_from.load(Ordering::Acquire)).filter(|n| *n != u64::MAX)
    }

    /// Nächstes Bild, falls schon dekodiert. `Ok(None)` = noch nicht bereit, `Err` = Ende.
    pub fn try_next(&self) -> std::result::Result<Option<RgbaFrame>, ()> {
        match self.rx.try_recv() {
            Ok(f) => Ok(Some(f)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(()),
        }
    }

    /// Blockierend (für Tests und Export).
    pub fn next_blocking(&self) -> Option<RgbaFrame> {
        self.rx.recv().ok()
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        // Nur beenden, nicht warten: Der Lese-Thread bekommt einen Lese- bzw. Sendefehler
        // (der Empfänger wird gleich danach verworfen) und holt den Prozess selbst ab.
        self.shared.stop.store(true, Ordering::Release);
        for c in self.shared.children.lock().unwrap_or_else(|e| e.into_inner()).iter_mut() {
            let _ = c.kill();
        }
    }
}

/// Pfad als Wert einer Filteroption im Filtergraphen escapen (`C:/x.cube` → `C\\:/x.cube`).
/// Zwei Ebenen: erst für die Option (`:` und `'`), dann für den Graphen (`\ ' [ ] , ;`) –
/// so übersteht auch ein Apostroph im Pfad beide.
pub fn escape_path(p: &str) -> String {
    let opt = p.replace('\\', "/").replace(':', "\\:").replace('\'', "\\'");
    let mut out = String::with_capacity(opt.len() + 8);
    for c in opt.chars() {
        if matches!(c, '\\' | '\'' | '[' | ']' | ',' | ';') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lange Graphen: Eine vorhandene Datei wird nicht neu geschrieben (ein anderer Dekoder liest
    /// sie vielleicht gerade); gleichzeitige Aufrufe sehen nie eine halbe Datei, nichts bleibt liegen.
    #[test]
    fn graph_file_is_written_once_and_never_partially() {
        let dir = tempfile::tempdir().unwrap();
        let g = "null,".repeat(20_000);
        let f = graph_file(dir.path(), &g).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), g);
        // Stellvertreter für „wird gerade gelesen“: der Inhalt darf nicht angefasst werden
        std::fs::write(&f, "gelesen").unwrap();
        assert_eq!(graph_file(dir.path(), &g).unwrap(), f);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "gelesen");
        let g2 = "copy,".repeat(20_000);
        let read: Vec<String> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..8)
                .map(|_| s.spawn(|| std::fs::read_to_string(graph_file(dir.path(), &g2).unwrap()).unwrap()))
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(read.iter().all(|r| *r == g2));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn escape_path_keeps_every_character() {
        // je Ebene (Graph, Option) fällt ein Backslash weg; nichts wird gelöscht
        assert_eq!(escape_path(r"C:\a'b[1],c;d"), r"C\\:/a\\\'b\[1\]\,c\;d");
    }

    #[test]
    fn detects_one_sided_stereo() {
        let dir = tempfile::tempdir().unwrap();
        let mk = |name: &str, pan: &str| {
            let out = dir.path().join(name);
            let st = command("ffmpeg")
                .unwrap()
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=f=440:d=2", "-af", pan, "-ac", "2"])
                .arg(&out)
                .status()
                .unwrap();
            assert!(st.success());
            out
        };
        assert_eq!(silent_channel(&mk("l.wav", "pan=stereo|c0=c0|c1=0*c0")), Some(1));
        assert_eq!(silent_channel(&mk("r.wav", "pan=stereo|c0=0*c0|c1=c0")), Some(0));
        assert_eq!(silent_channel(&mk("b.wav", "pan=stereo|c0=c0|c1=c0")), None);
    }

    fn make_clip(dir: &Path) -> PathBuf {
        let out = dir.join("t.mp4");
        let st = command("ffmpeg")
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=25:duration=2",
            ])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
            .args([
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&out)
            .status()
            .unwrap();
        assert!(st.success());
        out
    }

    /// Ein Strom mit eigenem Graphen liefert jedes Bild des Graphen auf dem Raster der Strom-Bildrate,
    /// auch wenn der Graph eine andere Bildrate meldet (FFmpeg 7: nach `setpts` gar keine, dann 25).
    #[test]
    fn graph_stream_keeps_every_frame_at_the_stream_rate() {
        if !ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = make_clip(dir.path());
        // 20 Bilder (2 s bei 10/s), Zeitstempel im 30er-Raster; der Graph meldet weiter 10/s
        let g = "[0:v]fps=10,setpts=N/(30*TB),scale=32:18,format=rgba[out]";
        let s = FrameStream::open_fx(&src, Time::ZERO, 32, 18, Rational::new(30, 1), Some(g)).unwrap();
        // FFmpeg 8: + 2 für die Dauer des letzten Bildes (1/10 s), FFmpeg 7 kennt sie nicht (−1);
        // ohne Bildrate am Ausgang 9 (FFmpeg 8) bzw. 16 (FFmpeg 7)
        let n = std::iter::from_fn(|| s.next_blocking()).count();
        assert!((19..=22).contains(&n), "{n}");
    }

    /// Nachtest K7: Strom mit Graph, dessen Zeit mitten im Clip beginnt (Tempo 2 ab Clipzeit 20 s:
    /// `setpts=…+20 s`). Mit `-r` lieferte FFmpeg 8 erst 20 s lang Kopien des ersten Bildes – die
    /// Ebene blieb ein Standbild. Jetzt bewegt sich das Bild sofort.
    #[test]
    fn graph_stream_with_a_late_clip_time_moves_at_once() {
        if !ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = gen(dir.path(), "lauf.mp4", &["-f", "lavfi", "-i", "testsrc2=s=64x36:r=30:d=4", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-g", "30"]);
        let g = "[0:v]setpts=PTS-STARTPTS,setpts=(PTS-STARTPTS)/2,fps=30/1,scale=32:18[pre];[pre]setpts=PTS-STARTPTS+round(20/TB)[pc];[pc]format=rgba[out]";
        let s = FrameStream::open_fx(&src, Time::ZERO, 32, 18, Rational::new(30, 1), Some(g)).unwrap();
        let frames: Vec<Vec<u8>> = std::iter::from_fn(|| s.next_blocking().map(|f| f.data)).take(20).collect();
        assert_eq!(frames.len(), 20);
        let distinct = frames.iter().collect::<HashSet<_>>().len();
        assert!(distinct >= 15, "{distinct} verschiedene von 20 Bildern");
    }

    /// Rückwärts-Fenster: jedes Bild des Fensters ist bytegleich mit dem Einzelbild an seiner
    /// Rasterzeit (so wie der Player sie rundet); am Dateiende kommen weniger Bilder.
    #[test]
    fn window_frames_equal_single_frames() {
        if !ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = gen(dir.path(), "fenster.mp4", &["-f", "lavfi", "-i", "testsrc2=s=64x36:r=60:d=3", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-g", "60"]);
        let at = |i: i64| Time::from_secs_f64(i as f64 / 60.0);
        let win = decode_frames(&src, at(97), 16, 32, 18, &|_| true, None).unwrap();
        assert_eq!(win.len(), 16);
        for (k, f) in &win {
            let one = decode_frame(&src, at(97 + *k as i64), 32, 18).unwrap();
            assert!(one.data == f.data, "Bild {} weicht ab", 97 + k);
        }
        assert_ne!(win[0].1.data, win[1].1.data, "aufeinanderfolgende Bilder");
        // nur ausgewählte behalten: dieselben Bilder an denselben Nummern
        let some = decode_frames(&src, at(97), 16, 32, 18, &|k| k < 3 || k == 15, None).unwrap();
        assert_eq!(some.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0, 1, 2, 15]);
        assert!(some[3].1.data == win[15].1.data);
        let tail = decode_frames(&src, at(172), 16, 32, 18, &|_| true, None).unwrap();
        assert!((1..=8).contains(&tail.len()), "{} Bilder am Ende", tail.len());
    }

    /// Graphen über 32 767 Zeichen (lange Zensur-Bahnen) gehen über eine Datei statt die
    /// Befehlszeile zu sprengen – Einzelbild und Strom.
    #[test]
    fn long_graph_goes_through_a_file() {
        if !ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = make_clip(dir.path());
        let g = format!("[0:v]{}scale=32:18,format=rgba[out]", "null,".repeat(8000));
        assert!(g.len() > 40_000);
        assert!(decode_frame_fx(&src, Time::ZERO, 32, 18, Some(&g)).is_ok());
        let s = FrameStream::open_fx(&src, Time::ZERO, 32, 18, Rational::new(30, 1), Some(&g)).unwrap();
        assert!(s.next_blocking().is_some());
    }

    #[test]
    fn probe_swaps_size_for_rotated_video() {
        if !ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = make_clip(dir.path());
        let rot = dir.path().join("rot.mp4");
        // Hochkant-Handyvideo nachbilden: quer gespeichert, Drehung als Seitendaten
        let st = command("ffmpeg")
            .unwrap()
            .args(["-v", "error", "-y", "-display_rotation", "90", "-i"])
            .arg(&src)
            .args(["-c", "copy"])
            .arg(&rot)
            .status()
            .unwrap();
        assert!(st.success());
        let m = probe(&rot).unwrap();
        assert_eq!((m.width, m.height), (180, 320));
        assert_eq!((probe(&src).unwrap().width, probe(&src).unwrap().height), (320, 180));
    }

    #[test]
    fn missing_frame_error_names_time_and_file() {
        if !ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = make_clip(dir.path());
        // hinter dem letzten Bild: FFmpeg liefert nichts und meldet auch nichts
        let e = decode_frame(&clip, Time::from_millis(2500), 32, 18).unwrap_err().to_string();
        assert!(e.contains("kein Bild bei 2.500 s") && e.contains("t.mp4"), "{e}");
        assert!(decode_frame(&clip, Time::from_millis(1900), 32, 18).is_ok());
    }

    #[test]
    fn probe_decode_and_stream() {
        if !ffmpeg_available() {
            eprintln!("FFmpeg fehlt, Test übersprungen");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = make_clip(dir.path());
        let m = probe(&clip).unwrap();
        assert_eq!(m.kind, MediaKind::Video);
        assert_eq!((m.width, m.height), (320, 180));
        assert_eq!(m.fps, Some(Rational::new(25, 1)));
        assert!(m.has_audio);
        assert!((m.duration.as_secs_f64() - 2.0).abs() < 0.1);

        let (w, h) = fit_size(m.width, m.height, 160, 160);
        assert_eq!((w, h), (160, 90));
        let f = decode_frame(&clip, Time::from_secs_f64(1.0), w, h).unwrap();
        assert_eq!(f.data.len(), (160 * 90 * 4) as usize);

        let s =
            FrameStream::open(&clip, Time::from_secs_f64(1.0), w, h, Rational::new(25, 1)).unwrap();
        let mut n = 0;
        while s.next_blocking().is_some() {
            n += 1;
        }
        assert!((23..=27).contains(&n), "{n} Frames ab 1 s erwartet ~25");
    }

    #[test]
    fn waveform_has_expected_resolution() {
        if !ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = make_clip(dir.path());
        let w = waveform(&clip, 50).unwrap();
        assert!((95..=105).contains(&w.len()), "{}", w.len());
        assert!(
            w.iter().any(|v| *v > 0.1),
            "Sinus (Amplitude 1/8) sollte ausschlagen"
        );
    }

    #[test]
    fn dropping_full_stream_does_not_deadlock() {
        if !ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = make_clip(dir.path());
        let s = FrameStream::open(&clip, Time::ZERO, 64, 36, Rational::new(25, 1)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500)); // Kanal läuft voll
        let t = std::time::Instant::now();
        drop(s);
        // nur beenden, nicht warten (der UI-Thread darf hier nicht hängen)
        assert!(
            t.elapsed() < std::time::Duration::from_millis(250),
            "Drop hat blockiert: {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn hw_slots_are_limited() {
        let n = AtomicUsize::new(0);
        assert!((0..3).all(|_| take_slot(&n, 3)));
        assert!(!take_slot(&n, 3), "vierter Hardware-Decoder");
        n.fetch_sub(1, Ordering::AcqRel);
        assert!(take_slot(&n, 3));
        assert!(!take_slot(&AtomicUsize::new(0), 0));
    }

    #[test]
    fn killed_job_returns_cancelled() {
        if !ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = make_clip(dir.path());
        let k = KillSwitch::default();
        k.kill();
        let r = decode_frame_opts(&clip, Time::from_millis(500), 32, 18, None, false, Some(&k));
        assert!(matches!(r, Err(MediaError::Cancelled)));
        let (f, info) =
            decode_frame_opts(&clip, Time::from_millis(500), 32, 18, None, false, Some(&KillSwitch::default())).unwrap();
        assert_eq!((f.data.len(), info.hw), (32 * 18 * 4, false));
    }

    fn mean_diff(a: &[u8], b: &[u8]) -> f64 {
        a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as f64).sum::<f64>() / a.len().max(1) as f64
    }

    /// Hardware- und Software-Decodierung liefern an derselben Stelle dasselbe Bild (Toleranz
    /// für Rundung der Farbumrechnung). Ohne Hardware-Decoder übersprungen.
    #[test]
    fn hw_and_sw_frames_match() {
        if !ffmpeg_available() || hwaccel().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = gen(
            dir.path(),
            "hd.mp4",
            &["-f", "lavfi", "-i", "testsrc2=s=1920x1080:r=30:d=2", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-g", "15"],
        );
        let at = Time::from_millis(1234);
        let (sw, _) = decode_frame_opts(&clip, at, 480, 270, None, false, None).unwrap();
        let (hw, info) = decode_frame_opts(&clip, at, 480, 270, None, true, None).unwrap();
        if !info.hw {
            eprintln!("kein Hardware-Decoder: Vergleich übersprungen");
            return;
        }
        let d = mean_diff(&sw.data, &hw.data);
        assert!(d < 2.0, "mittlere Abweichung HW/SW {d:.2}");
        // Strom: gleiche Bildzahl und gleiches erstes Bild
        let count = |hw: bool| {
            let s = FrameStream::open_opts(&clip, Time::from_secs_f64(1.0), 480, 270, Rational::new(30, 1), None, if hw { Hw::On } else { Hw::Off }).unwrap();
            let first = s.next_blocking().unwrap();
            (1 + std::iter::from_fn(|| s.next_blocking()).count(), first.data)
        };
        let (n_sw, f_sw) = count(false);
        let (n_hw, f_hw) = count(true);
        assert_eq!(n_sw, n_hw);
        assert!(mean_diff(&f_sw, &f_hw) < 2.0);
    }

    /// Hardware-Strom beginnt in Software (erstes Bild schneller) und die Hardware übernimmt
    /// ohne Sprung: jedes Bild gleicht dem des reinen Software-Stroms, keins fehlt oder doppelt.
    #[test]
    fn hw_stream_starts_in_software_and_takes_over_seamlessly() {
        if !ffmpeg_available() || hwaccel().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = gen(
            dir.path(),
            "lang.mp4",
            &["-f", "lavfi", "-i", "testsrc2=s=1920x1080:r=30:d=8", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-g", "120"],
        );
        let works = decode_frame_opts(&clip, Time::from_secs_f64(3.0), 320, 180, None, true, None).is_ok_and(|(_, i)| i.hw);
        // im Takt der Wiedergabe abholen (sonst ist die Software fertig, bevor die Hardware anläuft)
        let frames = |hw: bool| {
            let s = FrameStream::open_opts(&clip, Time::from_secs_f64(3.0), 320, 180, Rational::new(30, 1), None, if hw { Hw::AfterSoftware } else { Hw::Off }).unwrap();
            let v: Vec<Vec<u8>> = std::iter::from_fn(|| {
                std::thread::sleep(Duration::from_millis(20));
                s.next_blocking().map(|f| f.data)
            })
            .collect();
            (v, s.hw_from())
        };
        let (sw, none) = frames(false);
        assert_eq!(none, None);
        let (hw, from) = frames(true);
        eprintln!("Hardware ab Bild {from:?} von {}", hw.len());
        assert_ne!(from, Some(0), "erstes Bild aus der Software");
        if works {
            assert!(from.is_some(), "Hardware übernimmt");
        }
        assert_eq!(sw.len(), hw.len(), "kein Bild fehlt oder doppelt");
        for (i, (a, b)) in sw.iter().zip(&hw).enumerate() {
            assert!(mean_diff(a, b) < 2.0, "Bild {i} weicht ab");
        }
    }

    /// Scheitert der Hardware-Weg (hier: Datei ohne Bild am Ende), kommt kein falsches
    /// „kaputt“-Merken zustande, solange auch Software kein Bild hat.
    #[test]
    fn hw_failure_is_only_remembered_when_software_works() {
        if !ffmpeg_available() || hwaccel().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = make_clip(dir.path());
        assert!(decode_frame_opts(&clip, Time::from_secs_f64(9.0), 32, 18, None, true, None).is_err());
        assert!(!hw_decode_failed(&clip));
    }

    /// Mess-Test (von Hand): Einzelbild aus dem Original an 10 Stellen, Software gegen
    /// Hardware, Wanduhr und CPU-Zeit des FFmpeg.
    /// `cargo test --release -p cl-media -- --ignored --nocapture measure_still_frames`
    #[test]
    #[ignore]
    fn measure_still_frames() {
        let files = [
            "F:/cutline-testmedien/1080p60_gop10s.mp4",
            "F:/cutline-testmedien/4k60_h264_80mbit.mp4",
            "F:/cutline-testmedien/4k60_hevc10_50mbit.mp4",
        ];
        for f in files.iter().map(Path::new).filter(|p| p.is_file()) {
            let m = probe(f).unwrap();
            let (w, h) = fit_size(m.width, m.height, 696, 392);
            // abwechselnd SW und HW an derselben Stelle: gleiche Last für beide
            let mut wall = [Vec::new(), Vec::new()];
            let mut cpu = [Duration::ZERO; 2];
            let mut used_hw = 0;
            for i in 0..10 {
                let at = Time::from_secs_f64(m.duration.as_secs_f64() * (i as f64 + 0.37) / 10.5);
                for hw in [false, true] {
                    let t = std::time::Instant::now();
                    let (_, info) = decode_frame_opts(f, at, w, h, None, hw, None).unwrap();
                    wall[hw as usize].push(t.elapsed().as_millis() as u64);
                    cpu[hw as usize] += info.cpu.unwrap_or_default();
                    used_hw += info.hw as u32;
                }
            }
            for (i, label) in ["SW", "HW"].iter().enumerate() {
                wall[i].sort();
                println!(
                    "{} {label}: Median {} ms, max {} ms, CPU je Bild {} ms",
                    f.file_name().unwrap().to_string_lossy(),
                    wall[i][5],
                    wall[i][9],
                    cpu[i].as_millis() / 10
                );
            }
            assert_eq!(used_hw, 10, "Hardware-Weg nicht benutzt");
        }
    }

    #[test]
    fn probe_image_and_garbage() {
        if !ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("bild.png");
        let st = command("ffmpeg")
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=red:size=64x32",
                "-frames:v",
                "1",
            ])
            .arg(&png)
            .status()
            .unwrap();
        assert!(st.success());
        let m = probe(&png).unwrap();
        assert_eq!(m.kind, MediaKind::Image);
        assert_eq!((m.width, m.height), (64, 32));

        let bad = dir.path().join("kaputt.mp4");
        std::fs::write(&bad, b"kein video").unwrap();
        assert!(matches!(probe(&bad), Err(MediaError::Unreadable(_))));
    }

    #[test]
    fn fit_size_edge_cases() {
        assert_eq!(fit_size(0, 0, 100, 50), (100, 50));
        assert_eq!(fit_size(1920, 1080, 0, 0), (2, 2));
        assert_eq!(fit_size(10000, 1, 640, 360), (640, 2));
        assert_eq!(fit_size(1, 10000, 640, 360), (2, 360));
        assert_eq!(fit_size(u32::MAX, u32::MAX, 101, 101), (100, 100));
        let (w, h) = fit_size(1080, 1920, 1920, 1080);
        assert!(w % 2 == 0 && h == 1080 && w <= 1920);
    }

    fn gen(dir: &Path, name: &str, args: &[&str]) -> PathBuf {
        let out = dir.join(name);
        let st = command("ffmpeg")
            .unwrap()
            .args(["-v", "error", "-y"])
            .args(args)
            .args(["-threads", "2"])
            .arg(&out)
            .status()
            .unwrap();
        assert!(st.success(), "{name}");
        out
    }

    #[test]
    fn probe_unusual_files() {
        if !ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("leer.mp4");
        std::fs::write(&empty, b"").unwrap();
        assert!(probe(&empty).is_err());
        assert!(probe(&dir.path().join("fehlt.mp4")).is_err());
        assert!(probe(dir.path()).is_err(), "Ordner");
        // nur Ton
        let a = gen(dir.path(), "ton ä.m4a", &["-f", "lavfi", "-i", "sine=d=1", "-c:a", "aac"]);
        let m = probe(&a).unwrap();
        assert_eq!((m.kind, m.fps, m.width, m.has_audio), (MediaKind::Audio, None, 0, true));
        // nur Bild, 23,976 fps, Umlaut/Emoji im Namen
        let v = gen(dir.path(), "Bild ö 🎬.mp4", &["-f", "lavfi", "-i", "testsrc2=s=64x36:r=24000/1001:d=1", "-pix_fmt", "yuv420p"]);
        let m = probe(&v).unwrap();
        assert_eq!((m.kind, m.has_audio), (MediaKind::Video, false));
        assert_eq!(m.fps, Some(cl_core::Rational::new(24000, 1001)));
        // ein einziges Bild als Video, Drehung 270°
        let one = gen(dir.path(), "eins.mp4", &["-f", "lavfi", "-i", "testsrc2=s=64x36:r=25:d=0.04", "-pix_fmt", "yuv420p"]);
        let m = probe(&one).unwrap();
        assert_eq!(m.kind, MediaKind::Video);
        let rot = dir.path().join("rot.mp4");
        assert!(command("ffmpeg").unwrap().args(["-v", "error", "-y", "-display_rotation", "270", "-i"]).arg(&one)
            .args(["-c", "copy"]).arg(&rot).status().unwrap().success());
        assert_eq!((probe(&rot).unwrap().width, probe(&rot).unwrap().height), (36, 64));
        // abgeschnittene Datei: Fehler oder Video mit Dauer, aber kein Absturz
        let data = std::fs::read(&v).unwrap();
        let cut = dir.path().join("halb.mp4");
        std::fs::write(&cut, &data[..data.len() / 2]).unwrap();
        let _ = probe(&cut);
        // Bild nach dem Ende, Bild aus Müll, Strom aus Müll: Fehler statt Absturz/Hänger
        assert!(decode_frame(&v, Time::from_secs_f64(50.0), 32, 18).is_err());
        assert!(decode_frame(&empty, Time::ZERO, 32, 18).is_err());
        assert!(decode_frame(&a, Time::ZERO, 32, 18).is_err(), "kein Bild im Ton");
        let s = FrameStream::open(&empty, Time::ZERO, 32, 18, cl_core::Rational::new(25, 1)).unwrap();
        assert!(s.next_blocking().is_none());
        assert!(waveform(&empty, 50).is_err());
        assert!(waveform(&v, 50).map(|w| w.is_empty()).unwrap_or(true), "Video ohne Ton");
        let w = waveform(&a, 0).unwrap();
        assert!(!w.is_empty());
        assert_eq!(silent_channel(&a), None, "Mono");
        assert_eq!(silent_channel(&v), None, "ohne Ton");
    }
}
