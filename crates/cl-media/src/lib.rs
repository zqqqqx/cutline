//! Medienzugriff für Cutline.
//!
//! Phase 0 nutzt die FFmpeg-/FFprobe-Programme als Unterprozess. Das ist robust,
//! unterstützt sofort alle FFmpeg-Formate und braucht keine C-Header zum Bauen.
//! Phase 1 ersetzt das durch libav-Bindings mit Hardware-Decode und Zero-Copy nach wgpu.
//! Die öffentliche API (probe / decode_frame / FrameStream) bleibt dabei gleich.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};
use std::thread::JoinHandle;

use cl_core::{MediaId, MediaKind, MediaRef, Rational, Time};
use serde::Deserialize;

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

/// FFmpeg-Befehl (ohne Konsolenfenster) für andere Crates, z. B. den Export.
pub fn ffmpeg_command() -> Result<Command> {
    command("ffmpeg")
}

/// FFprobe-Befehl (ohne Konsolenfenster).
pub fn ffprobe_command() -> Result<Command> {
    command("ffprobe")
}

pub fn ffmpeg_available() -> bool {
    tool("ffmpeg").is_ok() && tool("ffprobe").is_ok()
}

fn command(name: &str) -> Result<Command> {
    #[allow(unused_mut)]
    let mut cmd = Command::new(tool(name)?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: kein Konsolenfenster aufblitzen lassen
    }
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
    }) && !fmt_name.contains("gif");

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

    Ok(MediaRef {
        id: MediaId::new(),
        path: path.to_path_buf(),
        kind,
        duration: if kind == MediaKind::Image {
            Time::ZERO
        } else {
            duration
        },
        width: video.and_then(|v| v.width).unwrap_or(0),
        height: video.and_then(|v| v.height).unwrap_or(0),
        fps: if kind == MediaKind::Video { fps } else { None },
        has_audio: audio.is_some(),
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
    let mut cmd = command("ffmpeg")?;
    cmd.args(["-v", "error", "-nostdin"])
        .args(["-ss", &format!("{:.6}", at.as_secs_f64().max(0.0))])
        .arg("-i")
        .arg(path);
    match graph {
        Some(g) => cmd.args(["-filter_complex", g, "-map", "[out]"]),
        None => cmd.args(["-vf", &format!("scale={w}:{h}:flags=bilinear,format=rgba")]),
    };
    let out = cmd
        .args([
            "-frames:v",
            "1",
            "-an",
            "-sn",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-",
        ])
        .stdin(Stdio::null())
        .output()?;
    let need = (w * h * 4) as usize;
    if out.stdout.len() < need {
        return Err(MediaError::Unreadable(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let mut data = out.stdout;
    data.truncate(need);
    Ok(RgbaFrame {
        width: w,
        height: h,
        data,
    })
}

/// Spitzenpegel (0..1) für die Waveform-Anzeige, `per_sec` Werte pro Sekunde.
pub fn waveform(path: &Path, per_sec: u32) -> Result<Vec<f32>> {
    const RATE: u32 = 8000;
    let out = command("ffmpeg")?
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
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
pub struct FrameStream {
    child: Child,
    rx: Option<Receiver<RgbaFrame>>,
    reader: Option<JoinHandle<()>>,
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub start: Time,
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
        let mut cmd = command("ffmpeg")?;
        cmd.args(["-v", "error", "-nostdin"])
            .args(["-ss", &format!("{:.6}", start.as_secs_f64().max(0.0))])
            .arg("-i")
            .arg(path)
            .args(["-an", "-sn"]);
        match graph {
            Some(g) => cmd.args(["-filter_complex", g, "-map", "[out]"]),
            None => cmd.args([
                "-vf",
                &format!(
                    "fps={}/{},scale={w}:{h}:flags=bilinear,format=rgba",
                    fps.num, fps.den
                ),
            ]),
        };
        let mut child = cmd
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdout = child.stdout.take().expect("piped");
        let (tx, rx) = sync_channel::<RgbaFrame>(6);
        let frame_len = (w * h * 4) as usize;
        let reader = std::thread::Builder::new()
            .name("cl-frame-stream".into())
            .spawn(move || loop {
                let mut buf = vec![0u8; frame_len];
                if stdout.read_exact(&mut buf).is_err() {
                    break;
                }
                if tx
                    .send(RgbaFrame {
                        width: w,
                        height: h,
                        data: buf,
                    })
                    .is_err()
                {
                    break;
                }
            })?;
        Ok(Self {
            child,
            rx: Some(rx),
            reader: Some(reader),
            width: w,
            height: h,
            fps,
            start,
        })
    }

    /// Nächstes Bild, falls schon dekodiert. `Ok(None)` = noch nicht bereit, `Err` = Ende.
    pub fn try_next(&self) -> std::result::Result<Option<RgbaFrame>, ()> {
        let Some(rx) = &self.rx else { return Err(()) };
        match rx.try_recv() {
            Ok(f) => Ok(Some(f)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(()),
        }
    }

    /// Blockierend (für Tests und Export).
    pub fn next_blocking(&self) -> Option<RgbaFrame> {
        self.rx.as_ref()?.recv().ok()
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        // Zuerst den Empfänger verwerfen: Ein im vollen Kanal blockierter Leser-Thread
        // bekommt so einen Sendefehler und endet, sonst würde join() ewig warten.
        drop(self.rx.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(
            t.elapsed() < std::time::Duration::from_secs(3),
            "Drop hat blockiert"
        );
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
}
