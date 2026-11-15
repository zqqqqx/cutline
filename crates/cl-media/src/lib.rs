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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

fn decode_once(
    path: &Path,
    at: Time,
    w: u32,
    h: u32,
    graph: Option<&str>,
    accel: Option<&str>,
    kill: Option<&KillSwitch>,
) -> Result<(RgbaFrame, Option<Duration>)> {
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
        Some(g) => cmd.args(["-filter_complex", g, "-map", "[out]"]),
        None => cmd.args(["-vf", &format!("scale={w}:{h}:flags=bilinear,format=rgba")]),
    };
    let mut child = cmd
        .args(["-frames:v", "1", "-an", "-sn", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
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
    let mut data = Vec::with_capacity(need);
    let _ = out.read_to_end(&mut data);
    let mut child = ks.release();
    if let Some(c) = child.as_mut() {
        let _ = c.wait();
    }
    let cpu = child.as_ref().and_then(cpu_time);
    let err = err.join().unwrap_or_default();
    if ks.is_killed() {
        return Err(MediaError::Cancelled);
    }
    if data.len() < need {
        let err = err.trim().to_string();
        // ohne Fehlertext: an dieser Stelle gibt es kein Bild (z. B. hinter dem letzten)
        return Err(MediaError::Unreadable(if err.is_empty() {
            format!("kein Bild bei {:.3} s in {}", at.as_secs_f64(), path.display())
        } else {
            err
        }));
    }
    data.truncate(need);
    Ok((RgbaFrame { width: w, height: h, data }, cpu))
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
    child: Mutex<Option<Child>>,
    stop: AtomicBool,
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
        Some(g) => cmd.args(["-filter_complex", g, "-map", "[out]"]),
        None => cmd.args([
            "-vf",
            &format!("fps={}/{},scale={w}:{h}:flags=bilinear,format=rgba", fps.num, fps.den),
        ]),
    };
    Ok(cmd
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?)
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
        Self::open_opts(path, start, w, h, fps, graph, false)
    }

    /// Wie [`FrameStream::open_fx`]; `hw`: Hardware-Decodierung versuchen (höchstens
    /// [`MAX_HW_DECODERS`] gleichzeitig). Liefert der Hardware-Weg kein einziges Bild, läuft
    /// derselbe Strom in Software weiter.
    pub fn open_opts(
        path: &Path,
        start: Time,
        w: u32,
        h: u32,
        fps: Rational,
        graph: Option<&str>,
        hw: bool,
    ) -> Result<Self> {
        let mut slot = if hw { HwSlot::acquire(path) } else { None };
        let spawn = {
            let (path, graph) = (path.to_path_buf(), graph.map(str::to_string));
            // Wiedergabe: vor anderer Last (siehe `raise_priority`)
            move |accel: Option<&str>| -> Result<Child> {
                let c = spawn_stream(&path, start, (w, h), fps, graph.as_deref(), accel)?;
                raise_priority(&c);
                Ok(c)
            }
        };
        let mut child = spawn(slot.as_ref().map(|s| s.accel))?;
        let mut stdout = child.stdout.take().expect("piped");
        let shared = Arc::new(StreamShared {
            child: Mutex::new(Some(child)),
            stop: AtomicBool::new(false),
        });
        let (tx, rx) = sync_channel::<RgbaFrame>(6);
        let frame_len = (w * h * 4) as usize;
        let sh = shared.clone();
        let path = path.to_path_buf();
        std::thread::Builder::new()
            .name("cl-frame-stream".into())
            .spawn(move || {
                let mut retried = false;
                loop {
                    let mut n = 0u64;
                    loop {
                        let mut buf = vec![0u8; frame_len];
                        if stdout.read_exact(&mut buf).is_err() {
                            break;
                        }
                        n += 1;
                        if tx.send(RgbaFrame { width: w, height: h, data: buf }).is_err() {
                            sh.stop.store(true, Ordering::Release);
                            break;
                        }
                    }
                    if retried && n > 0 {
                        hw_failed(&path);
                    }
                    // Hardware-Weg ohne ein einziges Bild: in Software noch einmal
                    if n > 0 || slot.is_none() || sh.stop.load(Ordering::Acquire) {
                        break;
                    }
                    slot = None;
                    retried = true;
                    let mut g = sh.child.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(mut old) = g.take() {
                        let _ = old.kill();
                        let _ = old.wait();
                    }
                    if sh.stop.load(Ordering::Acquire) {
                        break;
                    }
                    match spawn(None) {
                        Ok(mut c) => {
                            stdout = c.stdout.take().expect("piped");
                            *g = Some(c);
                        }
                        Err(_) => break,
                    }
                }
                drop(slot);
                // Aufräumen hier statt im UI-Thread
                let c = sh.child.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(mut c) = c {
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
        if let Some(c) = self.shared.child.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
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
            let s = FrameStream::open_opts(&clip, Time::from_secs_f64(1.0), 480, 270, Rational::new(30, 1), None, hw).unwrap();
            let first = s.next_blocking().unwrap();
            (1 + std::iter::from_fn(|| s.next_blocking()).count(), first.data)
        };
        let (n_sw, f_sw) = count(false);
        let (n_hw, f_hw) = count(true);
        assert_eq!(n_sw, n_hw);
        assert!(mean_diff(&f_sw, &f_hw) < 2.0);
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
