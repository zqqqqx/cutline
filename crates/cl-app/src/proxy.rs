//! Proxy-Dateien: kleine, schnell decodierbare Kopien großer Videos (über 1080p, über 30 fps
//! oder HEVC/AV1/ProRes) für eine flüssige Vorschau. Der Export nutzt immer die Originale.
//! Ablage: `<Daten>/cache/proxies/<Kennung>-<Höhe>[-<Bildrate>fps].mp4` (Kennung aus Pfad, Größe
//! und Änderungszeit; ohne Bildrate = Bildrate der Quelle bis 60, so hießen auch ältere Proxys).
//!
//! Gebaut wird in Stücken (siehe [`parts`]): jedes fertige Stück ist sofort nutzbar, fertige
//! Stücke überleben Neustarts, am Ende wird daraus die gewohnte Ein-Datei-Proxy.
//!
//! Der Bau stört den Player nicht: Priorität unter normal, angehalten kurz nach jedem Einzelbild-
//! Auftrag (Klick, Scrubben) und beim Abspielen, sobald die Wiedergabe nicht mehr mitkommt;
//! Hardware-Encoder zuerst.

mod parts;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use cl_core::{Clip, ClipContent, MediaId, MediaKind, MediaRef, Rational, Time};
use cl_export::Encoder;
use eframe::egui::{self, RichText};

pub use parts::Pick;
use parts::{Focus, Parting, MAX_FAILS, PART_SECS};

use crate::app::App;
use crate::paths;
use crate::tasks::TaskDone;
use crate::theme::{self, icon};
use crate::widgets;

pub fn dir() -> PathBuf {
    paths::data_dir().join("cache").join("proxies")
}

/// Proxy nie größer als das Original
fn proxy_height(m: &MediaRef, height: u32) -> u32 {
    height.min(m.height.max(2)) & !1
}

/// Kennung der Quelle (ändert sich, wenn die Datei sich ändert).
fn media_hash(m: &MediaRef) -> u64 {
    let meta = std::fs::metadata(&m.path).ok();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    m.path.hash(&mut h);
    meta.as_ref().map(|m| m.len()).hash(&mut h);
    meta.and_then(|m| m.modified().ok()).hash(&mut h);
    h.finish()
}

/// Was ein Proxy sein soll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProxySpec {
    pub height: u32,
    /// Projekt-Bildrate, wenn die Quelle schneller ist; `None` = wie die Quelle (bis 60)
    pub fps: Option<Rational>,
    /// Schlüsselbild-Abstand in Bildern (höchstens 0,5 s: schnelles Springen)
    pub gop: u32,
}

/// Proxy für ein Medium in der gewählten Höhe und der Bildrate des Projekts: eine 60-fps-Quelle
/// in einem 30-fps-Projekt wird mit 30 fps gespeichert (halb so viel zu decodieren).
pub fn proxy_spec(m: &MediaRef, height: u32, project_fps: Rational) -> ProxySpec {
    let src = m.fps.filter(|f| f.num > 0 && f.den > 0).map_or(30.0, |f| f.as_f64());
    let fps = (project_fps.num > 0 && project_fps.den > 0 && src > project_fps.as_f64() + 0.01)
        .then_some(project_fps);
    let out = fps.map_or(src.min(60.0), |f| f.as_f64());
    ProxySpec {
        height: proxy_height(m, height),
        fps,
        gop: ((out / 2.0).round() as u32).max(1),
    }
}

fn file_name(hash: u64, height: u32, fps: Option<Rational>) -> String {
    match fps {
        Some(f) => format!("{hash:016x}-{height}-{}_{}fps.mp4", f.num, f.den),
        None => format!("{hash:016x}-{height}.mp4"),
    }
}

/// Dateinamen, die als Proxy taugen, in der Reihenfolge der Wahl: gewünschte Höhe, dann größere
/// (ein vorhandener 720p-Proxy reicht auch bei der Einstellung 540p – kein Neubau); je Höhe erst
/// in Projekt-Bildrate, dann in Bildrate der Quelle (auch ältere Proxys).
fn candidates(m: &MediaRef, height: u32, project_fps: Rational, hash: u64) -> Vec<(String, u32)> {
    let spec = proxy_spec(m, height, project_fps);
    let mut hs = vec![spec.height];
    for h in PROXY_HEIGHTS.iter().map(|p| p.0).filter(|h| *h > height) {
        let h = proxy_height(m, h);
        if !hs.contains(&h) {
            hs.push(h);
        }
    }
    hs.into_iter()
        .flat_map(|h| {
            spec.fps
                .map(|f| (file_name(hash, h, Some(f)), h))
                .into_iter()
                .chain([(file_name(hash, h, None), h)])
        })
        .collect()
}

/// Pfad, unter dem ein neuer Proxy entsteht.
pub fn proxy_path(m: &MediaRef, height: u32, project_fps: Rational) -> PathBuf {
    proxy_path_in(&dir(), m, height, project_fps)
}

fn proxy_path_in(d: &Path, m: &MediaRef, height: u32, project_fps: Rational) -> PathBuf {
    let s = proxy_spec(m, height, project_fps);
    d.join(file_name(media_hash(m), s.height, s.fps))
}

/// Vorhandener, brauchbarer Proxy mit seiner Höhe.
pub fn existing_proxy(m: &MediaRef, height: u32, project_fps: Rational) -> Option<(PathBuf, u32)> {
    existing_proxy_in(&dir(), m, height, project_fps)
}

fn existing_proxy_in(d: &Path, m: &MediaRef, height: u32, project_fps: Rational) -> Option<(PathBuf, u32)> {
    candidates(m, height, project_fps, media_hash(m))
        .into_iter()
        .map(|(n, h)| (d.join(n), h))
        .find(|(p, _)| p.is_file())
}

/// Lohnt sich ein Proxy?
pub fn wants_proxy(m: &MediaRef) -> bool {
    if m.kind != MediaKind::Video {
        return false;
    }
    let fps = m.fps.map(|f| f.as_f64()).unwrap_or(30.0);
    let codec = m.video_codec.as_deref().unwrap_or("").to_ascii_lowercase();
    m.height > 1080
        || m.width > 1920
        || fps > 31.0
        || ["hevc", "av1", "prores", "vp9"]
            .iter()
            .any(|c| codec.contains(c))
}

/// Wählbare Proxy-Höhen (Einstellungen → Leistung)
pub const PROXY_HEIGHTS: [(u32, &str); 3] = [(540, "Schnell · 540p"), (720, "Ausgewogen · 720p"), (1080, "Hoch · 1080p")];

// ------------------------------------------------------------------ Encoder

/// Reihenfolge der Encoder wie beim Export ([`cl_export::encoder_chain`], `pref` = Einstellung
/// „Hardware-Kodierung“) ohne die in dieser Sitzung gescheiterten, zuletzt immer Software.
pub fn encoder_chain(available: &[Encoder], broken: &[Encoder], pref: &str) -> Vec<Encoder> {
    cl_export::encoder_chain(available, pref)
        .into_iter()
        .filter(|e| *e == Encoder::Software || !broken.contains(e))
        .collect()
}

static ENCODER_PREF: Mutex<String> = Mutex::new(String::new());

/// Einstellung „Hardware-Kodierung“ (setzen die Einstellungen beim Laden und Ändern)
pub fn set_encoder_pref(pref: &str) {
    *ENCODER_PREF.lock().unwrap_or_else(|e| e.into_inner()) = pref.to_string();
}

pub fn encoder_pref() -> String {
    ENCODER_PREF.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Gefundene Encoder, ohne zu warten: `None`, solange die Erkennung im Hintergrund läuft.
pub fn found_encoders() -> Option<&'static [Encoder]> {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::spawn(available_encoders);
    });
    AVAILABLE.get().map(Vec::as_slice)
}

/// Encoder, die in dieser Sitzung beim Proxy-Bau gescheitert sind
fn broken_encoders() -> std::sync::MutexGuard<'static, Vec<Encoder>> {
    static B: Mutex<Vec<Encoder>> = Mutex::new(Vec::new());
    B.lock().unwrap_or_else(|e| e.into_inner())
}

/// Hardware-Encoder dieses Rechners (Erkennung des Exports, einmal je Sitzung).
fn available_encoders() -> &'static [Encoder] {
    AVAILABLE.get_or_init(cl_export::available_encoders)
}

static AVAILABLE: OnceLock<Vec<Encoder>> = OnceLock::new();

/// Hardware-Decodierung der Quelle beim Proxy-Bau: nur mit NVENC und nur bei Quellen, deren
/// Software-Decodierung bremst (über 1080p, HEVC/AV1/VP9). Gemessen: 1080p-H.264 ist in Software
/// schneller (17× statt 10× Echtzeit), 4K-HEVC-10-Bit mit CUDA 2,8× statt 2,2×.
fn hw_decode(enc: Encoder, m: &MediaRef) -> Option<&'static str> {
    let codec = m.video_codec.as_deref().unwrap_or("").to_ascii_lowercase();
    let heavy = m.height > 1080 || m.width > 1920 || ["hevc", "av1", "vp9"].iter().any(|c| codec.contains(c));
    (enc == Encoder::Nvenc && heavy).then_some("cuda")
}

/// Je Encoder: erst mit Hardware-Decodierung (falls sinnvoll), dann ohne.
fn decode_attempts(enc: Encoder, m: &MediaRef) -> Vec<Option<&'static str>> {
    match hw_decode(enc, m) {
        Some(h) => vec![Some(h), None],
        None => vec![None],
    }
}

/// Encoder-Argumente (Qualität: 540p ≈ 17 MB je Minute, Ziel ≤ 25)
fn codec_args(enc: Encoder) -> &'static [&'static str] {
    match enc {
        Encoder::Nvenc => &["-c:v", "h264_nvenc", "-preset", "p4", "-rc", "constqp", "-qp", "26"],
        Encoder::Qsv => &["-c:v", "h264_qsv", "-preset", "veryfast", "-global_quality", "26"],
        Encoder::Amf => &["-c:v", "h264_amf", "-quality", "speed", "-rc", "cqp", "-qp_i", "26", "-qp_p", "26"],
        // Bitrate je nach Größe: siehe `part_args`
        Encoder::Mf => &["-c:v", "h264_mf", "-pix_fmt", "nv12", "-hw_encoding", "1"],
        Encoder::Software => &["-c:v", "libx264", "-preset", "veryfast", "-crf", "23"],
    }
}

/// Bildrate des Proxys: Projekt-Bildrate, sonst die der Quelle (höchstens 60, unbekannt 30)
fn out_fps(m: &MediaRef, spec: &ProxySpec) -> Rational {
    spec.fps.unwrap_or_else(|| match m.fps.filter(|f| f.num > 0 && f.den > 0) {
        Some(f) if f.as_f64() <= 60.01 => f,
        Some(_) => Rational::new(60, 1),
        None => Rational::new(30, 1),
    })
}

/// FFmpeg-Argumente für Stück `k` (Eingang und Ausgang ohne Zieldatei). `st` = Startzeit des
/// Videostroms der Quelle: Mit `-copyts` sieht jedes Stück dieselben Zeitstempel wie ein Bau in
/// einem Zug und rechnet auf demselben Bildraster (kein doppeltes/fehlendes Bild an der Naht).
fn part_args(src: &Path, st: f64, pt: &Parting, k: u32, height: u32, gop: u32, enc: Encoder, hw: Option<&str>) -> Vec<String> {
    let s = pt.start(k);
    let mut a: Vec<String> = Vec::new();
    if let Some(h) = hw {
        a.extend(["-hwaccel".into(), h.into()]);
    }
    // ab dem Schlüsselbild ein Stück vor dem Anfang lesen: das Bild am Anfang kann davor liegen
    a.extend(["-noaccurate_seek".into(), "-ss".into(), format!("{:.6}", (s - 0.5).max(0.0)), "-copyts".into(), "-i".into()]);
    a.push(src.to_string_lossy().into_owned());
    let vf = format!(
        "fps={}/{}:start_time={:.6},setpts=PTS-STARTPTS,scale=-2:{height}:flags=bicubic,format=yuv420p",
        pt.fps.num,
        pt.fps.den,
        st + s
    );
    a.extend(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &vf].map(String::from));
    a.extend(codec_args(enc).iter().map(|s| s.to_string()));
    if enc == Encoder::Mf {
        // ohne verlässliche konstante Qualität: Bitrate, 540p30 ≈ 2,5 Mbit/s (≈ 17 MB je Minute)
        a.extend(["-b:v".into(), format!("{}k", (height as f64 * height as f64 * pt.fps.as_f64() / 3500.0).round() as u32)]);
    }
    // ohne B-Bilder: Stücke verschiedener Encoder (Rückfall) haben sonst verschiedene Vorlaufzeiten,
    // zusammengefügt entstand an der Naht ein doppeltes Bild; beim Springen ohnehin schneller
    a.extend(["-bf".into(), "0".into(), "-g".into(), gop.to_string(), "-frames:v".into(), pt.per.to_string()]);
    // FFmpeg 7 vergisst die Bildrate nach `setpts`: ohne `-r` hätte das letzte Bild jedes Stücks die
    // Dauer 0 und fiele beim Zusammenfügen weg
    a.extend(["-r".into(), format!("{}/{}", pt.fps.num, pt.fps.den)]);
    a
}

// ------------------------------------------------------------------ Rücksicht auf den Player

static PLAYING: AtomicBool = AtomicBool::new(false);
/// Die Wiedergabe kam in dieser Abspiel-Sitzung nicht mit (gilt bis zum Stopp)
static BEHIND: AtomicBool = AtomicBool::new(false);
/// Vom Nutzer pausiert (Aufgaben-Anzeige): laufender Bau hält an, kein neuer beginnt
static USER_PAUSED: AtomicBool = AtomicBool::new(false);
static LAST_ACTIVITY: Mutex<Option<Instant>> = Mutex::new(None);

/// Der Player hat gerade ein Einzelbild angefordert oder bekommen (Klick, Scrubben).
pub fn note_activity() {
    *LAST_ACTIVITY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

/// Zustand der Wiedergabe (vom UI-Thread je Bild): läuft sie, und meldet der Kern gerade einen
/// Rückstand (automatische Qualität abgesenkt bzw. ausgeschaltet = unbekannt)?
pub fn set_play_state(playing: bool, behind_now: bool) {
    PLAYING.store(playing, Ordering::Relaxed);
    let prev = BEHIND.load(Ordering::Relaxed);
    BEHIND.store(latch_behind(prev, playing, behind_now), Ordering::Relaxed);
}

/// Rückstand merken, bis die Wiedergabe endet: sonst liefe der Bau nach dem Erholen der Qualität
/// wieder an, bremste erneut – ein Hin und Her im Takt der automatischen Qualität.
pub fn latch_behind(prev: bool, playing: bool, behind_now: bool) -> bool {
    playing && (prev || behind_now)
}

fn idle() -> Duration {
    LAST_ACTIVITY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map_or(Duration::MAX, |t| t.elapsed())
}

/// Pause sofort nach einem Einzelbild-Auftrag …
const PAUSE_WITHIN: Duration = Duration::from_millis(300);
/// … und Fortsetzen erst nach einer Ruhepause, damit schnelles Klicken nicht ständig schaltet.
const RESUME_AFTER: Duration = Duration::from_millis(800);

/// Soll der Proxy-Bau (jetzt) angehalten sein? Beim Abspielen läuft er weiter (ein Stück-Encoder,
/// Priorität unter normal), außer die Wiedergabe kommt nicht mit (`behind`); im Stand hält jeder
/// Klick ihn kurz an.
pub fn want_pause(paused: bool, playing: bool, behind: bool, idle: Duration) -> bool {
    if playing {
        return behind;
    }
    idle < if paused { RESUME_AFTER } else { PAUSE_WITHIN }
}

fn pause_now(paused: bool) -> bool {
    USER_PAUSED.load(Ordering::Relaxed) || want_pause(paused, PLAYING.load(Ordering::Relaxed), BEHIND.load(Ordering::Relaxed), idle())
}

// ------------------------------------------------------------------ Bau

/// Ergebnis eines Proxy-Baus (für den Messpunkt).
#[derive(Debug, Clone, Copy)]
pub struct BuildReport {
    /// Laufzeit ohne Pausen
    pub secs: f64,
    /// angehalten (Wiedergabe, Klicks)
    pub paused: f64,
}

/// Proxy erzeugen: Encoder der Reihe nach (siehe [`encoder_chain`]), je Encoder erst mit, dann
/// ohne Hardware-Decodierung. Ein Encoder, der scheitert, wird für die Sitzung gemieden.
pub fn build(
    m: &MediaRef,
    out: &Path,
    spec: &ProxySpec,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<BuildReport, String> {
    let chain = encoder_chain(available_encoders(), &broken_encoders(), &encoder_pref());
    let mut last = String::new();
    for enc in chain {
        for hw in decode_attempts(enc, m) {
            match build_with(m, out, spec, enc, hw, progress, cancel) {
                Ok(r) => {
                    // Bau in einem Zug (Dauer unbekannt): sonst zählte er nie als fertig
                    with_mine(&m.path, out, Entry::built);
                    return Ok(r);
                }
                Err(e) if e == "Abgebrochen" => return Err(e),
                Err(e) => last = e,
            }
        }
        if enc != Encoder::Software {
            broken_encoders().push(enc);
        }
    }
    Err(last)
}

#[allow(clippy::too_many_arguments)]
fn build_with(
    m: &MediaRef,
    out: &Path,
    spec: &ProxySpec,
    enc: Encoder,
    hw: Option<&str>,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<BuildReport, String> {
    std::fs::create_dir_all(out.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    let tmp = parts::tmp_of(out);
    let mut cmd = encode_command()?;
    // derselbe Weg wie ein Stück über alles: gleiches Bildraster, gleicher Startversatz wie das Original
    let whole = Parting { fps: out_fps(m, spec), per: u32::MAX, count: 1 };
    cmd.args(part_args(&m.path, probe_start(&m.path), &whole, 0, spec.height, spec.gop, enc, hw));
    let dur = m.duration.as_secs_f64().max(0.1);
    let (report, n) = run_encode(cmd, &tmp, out, dur, progress, cancel)?;
    if n == 0 {
        return Err("Proxy konnte nicht erstellt werden".into());
    }
    if crate::app::perf() {
        let mb = std::fs::metadata(out).map_or(0.0, |x| x.len() as f64 / 1e6);
        crate::applog::perf_line!(
            "perf: proxy {} enc={:?}{} {:.1}× echtzeit {:.1} MB/min pausiert {:.1} s",
            m.path.file_name().unwrap_or_default().to_string_lossy(),
            enc,
            hw.map(|h| format!("+{h}")).unwrap_or_default(),
            dur / report.secs,
            mb / (dur / 60.0),
            report.paused
        );
    }
    Ok(report)
}

/// FFmpeg für den Proxy-Bau: Priorität unter normal, Fortschritt auf stdout
fn encode_command() -> Result<Command, String> {
    let mut cmd = cl_media::ffmpeg_command().map_err(|e| e.to_string())?;
    cl_media::lower_priority(&mut cmd);
    cmd.args(["-v", "error", "-y", "-nostdin", "-progress", "pipe:1", "-nostats"]);
    Ok(cmd)
}

/// FFmpeg nach `tmp` schreiben lassen und bei Erfolg zu `out` umbenennen (eine fertige Datei
/// ist nie halb). Hält den Prozess an, solange der Player Vorrang hat ([`pause_now`]).
/// Ergebnis: Laufzeit und Zahl der geschriebenen Bilder.
fn run_encode(
    mut cmd: Command,
    tmp: &Path,
    out: &Path,
    dur: f64,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<(BuildReport, u64), String> {
    let mut child = cmd
        .arg(tmp)
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // ungelesen würde der volle Puffer FFmpeg anhalten
        .spawn()
        .map_err(|e| e.to_string())?;
    // Fortschritt in eigenem Thread lesen: angehalten schreibt FFmpeg nichts, die Steuerung
    // unten muss trotzdem weiterlaufen (Fortsetzen, Abbrechen).
    let done_us = Arc::new(AtomicU64::new(0));
    let frames = Arc::new(AtomicU64::new(0));
    let reader = child.stdout.take().map(|out| {
        let (done_us, frames) = (done_us.clone(), frames.clone());
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<u64>().ok()) {
                    done_us.store(us, Ordering::Relaxed);
                } else if let Some(n) = line.strip_prefix("frame=").and_then(|v| v.trim().parse::<u64>().ok()) {
                    frames.store(n, Ordering::Relaxed);
                }
            }
        })
    });
    let dur = dur.max(0.1);
    let start = Instant::now();
    let (mut paused, mut paused_total, mut paused_since) = (false, Duration::ZERO, None::<Instant>);
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            // warten, bis FFmpeg die Datei losgelassen hat (Windows), sonst bleibt `.part.mp4`
            let _ = child.wait();
            let _ = std::fs::remove_file(tmp);
            return Err("Abgebrochen".into());
        }
        let want = pause_now(paused);
        if want != paused && cl_media::suspend(&child, want) {
            paused = want;
            if want {
                paused_since = Some(Instant::now());
            } else if let Some(t) = paused_since.take() {
                paused_total += t.elapsed();
            }
        }
        progress((done_us.load(Ordering::Relaxed) as f64 / 1e6 / dur).clamp(0.0, 1.0) as f32);
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(tmp);
                return Err(e.to_string());
            }
        }
    };
    if let Some(r) = reader {
        let _ = r.join();
    }
    if !status.success() {
        let _ = std::fs::remove_file(tmp);
        return Err("Proxy konnte nicht erstellt werden".into());
    }
    let paused = (paused_total + paused_since.map_or(Duration::ZERO, |t| t.elapsed())).as_secs_f64();
    let report = BuildReport {
        secs: (start.elapsed().as_secs_f64() - paused).max(1e-3),
        paused,
    };
    let n = frames.load(Ordering::Relaxed);
    if n == 0 {
        // kein Bild geschrieben: nichts umbenennen, der Aufrufer entscheidet
        let _ = std::fs::remove_file(tmp);
        return Ok((report, 0));
    }
    std::fs::rename(tmp, out).map_err(|e| e.to_string())?;
    Ok((report, n))
}

/// Startzeit der Datei (frühester Strom; Quellen mit Versatz, z. B. aus Kameras oder geschnittenen
/// Dateien). Nicht die der Videospur: `-ss` am Original zählt ab dem Dateianfang, beginnt das Bild
/// später als der Ton, füllt der `fps`-Filter vorne mit dem ersten Bild auf – wie das Original.
fn probe_start(src: &Path) -> f64 {
    cl_media::ffprobe_command()
        .ok()
        .and_then(|mut c| {
            c.args(["-v", "error", "-show_entries", "format=start_time", "-of", "csv=p=0"])
                .arg(src)
                .stdin(Stdio::null())
                .output()
                .ok()
        })
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

/// Stück `k` bauen (Encoder-Kette wie [`build`]). Ein Encoder gilt erst als kaputt, wenn ein
/// anderer dasselbe Stück schafft – sonst liegt es an der Quelle. Ergebnis: Zahl der Bilder
/// (0: das Video endet vor diesem Stück, die Datei bleibt leer).
#[allow(clippy::too_many_arguments)]
fn build_part(
    m: &MediaRef,
    dir: &Path,
    pt: &Parting,
    st: f64,
    k: u32,
    spec: &ProxySpec,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<u64, String> {
    let mut failed = Vec::new();
    let mut last = String::new();
    // erst lösen: in der Schleife wird die Liste wieder gesperrt
    let chain = encoder_chain(available_encoders(), &broken_encoders(), &encoder_pref());
    for enc in chain {
        for hw in decode_attempts(enc, m) {
            match build_part_with(m, dir, pt, st, k, spec, enc, hw, progress, cancel) {
                Ok(n) => {
                    broken_encoders().extend(failed.iter().filter(|e| **e != Encoder::Software));
                    return Ok(n);
                }
                Err(e) if e == "Abgebrochen" => return Err(e),
                Err(e) => last = e,
            }
        }
        failed.push(enc);
    }
    Err(last)
}

#[allow(clippy::too_many_arguments)]
fn build_part_with(
    m: &MediaRef,
    dir: &Path,
    pt: &Parting,
    st: f64,
    k: u32,
    spec: &ProxySpec,
    enc: Encoder,
    hw: Option<&str>,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<u64, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let out = dir.join(parts::part_name(k));
    let tmp = parts::tmp_of(&out);
    let mut cmd = encode_command()?;
    cmd.args(part_args(&m.path, st, pt, k, spec.height, spec.gop, enc, hw));
    let secs = pt.start(k + 1) - pt.start(k);
    let (_, n) = run_encode(cmd, &tmp, &out, secs, progress, cancel)?;
    if n == 0 {
        // hinter dem Ende des Videos (Ton länger als Bild): leeres Stück
        std::fs::write(&out, b"").map_err(|e| e.to_string())?;
    }
    Ok(n)
}

/// Alle Stücke verlustfrei zur Ein-Datei-Proxy `out` zusammenfügen (erst fertig, dann umbenennen).
fn merge(dir: &Path, pt: &Parting, out: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let list = parts::concat_list(dir, 0, pt.count).map_err(|e| e.to_string())?;
    let tmp = parts::tmp_of(out);
    let mut cmd = encode_command()?;
    cmd.args(["-f", "concat", "-i"]).arg(&list).args(["-map", "0:v:0", "-c", "copy", "-movflags", "+faststart"]);
    let dur = pt.start(pt.count);
    match run_encode(cmd, &tmp, out, dur, &|_| {}, cancel)? {
        (_, 0) => Err("Proxy-Stücke ohne Bild".into()),
        _ => Ok(()),
    }
}

/// Reihenfolge der Proxy-Aufträge: Medien am Playhead zuerst, dann im sichtbaren Bereich der
/// Zeitleiste, dann der Rest (in Medienreihenfolge).
pub fn next_proxy(candidates: &[MediaId], at_playhead: &HashSet<MediaId>, visible: &HashSet<MediaId>) -> Option<MediaId> {
    candidates
        .iter()
        .min_by_key(|id| if at_playhead.contains(id) { 0 } else if visible.contains(id) { 1 } else { 2 })
        .copied()
}

/// Ergebnis einer Suche nach Proxys (Hintergrund): je Video der vorhandene Proxy, dazu die
/// Medien, für die einer gebaut werden soll (oder Stücke fehlen)
type Scan = (Vec<(MediaId, Option<(PathBuf, u32)>)>, Vec<MediaId>);

/// Proxy-Verwaltung ohne Dateisystem im UI-Thread: Suche nach vorhandenen/fehlenden Proxys im
/// Hintergrund, Ergebnis im Zwischenspeicher; Zähler für die Gesamtanzeige.
#[derive(Default)]
pub struct ProxyState {
    /// vorhandener Proxy je Medium (gilt für `key`)
    known: HashMap<MediaId, Option<(PathBuf, u32)>>,
    /// Einstellung, für die `known` gilt: (Höhe, Projekt-Bildrate)
    key: Option<(u32, Rational)>,
    scan: Option<std::sync::mpsc::Receiver<Scan>>,
    /// letzte Suche: Zeit und Anzahl Medien
    scanned: Option<(Instant, usize)>,
    /// fertig gebaut / insgesamt in dieser Runde (Anzeige „Proxys 3 / 40“)
    pub done: usize,
    pub total: usize,
    /// vom Nutzer abgebrochen: in diesem Projekt keine weiteren
    pub stopped: bool,
}

impl ProxyState {
    /// Anderes Projekt: Zähler und Suche neu; vorhandene Proxys gelten weiter (Kennung aus der Datei)
    pub fn reset(&mut self) {
        *self = Self { key: self.key, known: std::mem::take(&mut self.known), ..Default::default() };
    }

    /// Vorhandener Proxy aus dem Zwischenspeicher (`None` = unbekannt, `Some(None)` = keiner)
    pub fn cached(&self, m: MediaId, key: (u32, Rational)) -> Option<Option<(PathBuf, u32)>> {
        (self.key == Some(key)).then(|| self.known.get(&m).cloned()).flatten()
    }

    pub fn paused() -> bool {
        USER_PAUSED.load(Ordering::Relaxed)
    }

    pub fn set_paused(on: bool) {
        USER_PAUSED.store(on, Ordering::Relaxed);
    }
}

/// Neue Suche fällig? Nach 2 s oder wenn sich die Zahl der Medien ändert.
pub fn scan_due(scanned: Option<(Instant, usize)>, media: usize) -> bool {
    scanned.is_none_or(|(t, n)| n != media || t.elapsed() >= Duration::from_secs(2))
}

/// Neue Gesamtzahl der Runde: fertige plus fehlende; nichts fehlt → Runde vorbei.
pub fn round(done: usize, missing: usize) -> (usize, usize) {
    if missing == 0 {
        (0, 0)
    } else {
        (done, done + missing)
    }
}

// ------------------------------------------------------------------ Stück-Bestand

/// Stand des Proxys einer Quelle: vom Bau (Hintergrund) geschrieben, vom Player und der Zeitleiste
/// gelesen (ohne Dateisystem je Bild).
struct Entry {
    /// gewählte Proxy-Höhe und Projekt-Bildrate, für die der Eintrag gilt
    want: (u32, Rational),
    /// Ziel: die fertige Ein-Datei-Proxy (daneben der Stück-Ordner)
    out: PathBuf,
    spec: ProxySpec,
    /// `None`: Dauer unbekannt, Bau in einem Zug
    pt: Option<Parting>,
    dur: f64,
    ready: Vec<bool>,
    fails: Vec<u8>,
    /// fertiger Proxy vorhanden (auch ein älterer Ein-Datei-Proxy) …
    complete: bool,
    /// … und seine Datei (Strom ohne Dateizugriff im UI-Thread)
    file: Option<PathBuf>,
    /// Bau gescheitert oder abgebrochen: in dieser Sitzung nicht neu
    dead: bool,
    /// fertige Quellbereiche (Zeitleiste), nur bei Änderungen neu berechnet
    ranges: Arc<Vec<(f64, f64)>>,
}

impl Entry {
    fn changed(&mut self) {
        self.ranges = Arc::new(match self.pt {
            _ if self.complete => vec![(0.0, self.dur.max(0.0))],
            Some(pt) => parts::ready_ranges(&pt, &self.ready, self.dur),
            None => Vec::new(),
        });
    }

    /// Der eigene Bau ist fertig: die Datei liegt unter `out`
    fn built(&mut self) {
        self.complete = true;
        self.file = Some(self.out.clone());
        self.changed();
    }

    /// Fehlt das Stück an Quellzeit `t` (ohne Einteilung: der ganze Proxy) und ist es baubar?
    fn needs(&self, t: f64) -> bool {
        !self.complete
            && !self.dead
            && self.pt.is_none_or(|pt| {
                let k = pt.part_of(t) as usize;
                !self.ready.get(k).copied().unwrap_or(true) && self.fails.get(k).is_some_and(|f| *f < MAX_FAILS)
            })
    }
}

fn registry() -> MutexGuard<'static, HashMap<PathBuf, Entry>> {
    static R: OnceLock<Mutex<HashMap<PathBuf, Entry>>> = OnceLock::new();
    R.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Wo die Arbeit hin soll, je Quelle (vom UI-Thread gesetzt, siehe [`App::update_focus`])
fn focus() -> MutexGuard<'static, HashMap<PathBuf, Focus>> {
    static F: OnceLock<Mutex<HashMap<PathBuf, Focus>>> = OnceLock::new();
    F.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Eintrag des laufenden Baus ändern (nur, wenn er noch zu diesem Ziel gehört)
fn with_mine<R>(src: &Path, out: &Path, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
    registry().get_mut(src).filter(|e| e.out == out).map(f)
}

/// Eintrag für `m` auf Stand bringen (alle 2 s): fertigen Proxy erkennen, nach Neustart oder
/// geänderter Quelle die Stücke vom Datenträger lesen. `true`: es fehlt noch etwas.
/// Läuft in der Hintergrund-Suche; Dateisystem nur außerhalb der Sperre (Player und Zeitleiste
/// lesen den Bestand je Bild). Ergebnis: vorhandener Proxy, und ob noch etwas zu bauen ist.
fn refresh_entry(root: &Path, m: &MediaRef, h: u32, fps: Rational, playing: bool) -> (Option<(PathBuf, u32)>, bool) {
    let found = existing_proxy_in(root, m, h, fps);
    let full = found.is_some();
    let out = proxy_path_in(root, m, h, fps);
    if registry().get(&m.path).is_none_or(|e| e.want != (h, fps) || e.out != out) {
        let spec = proxy_spec(m, h, fps);
        let dur = m.duration.as_secs_f64();
        let pt = Parting::new(dur, out_fps(m, &spec), PART_SECS);
        let pdir = parts::parts_dir(&out);
        let ready = pt.map_or_else(Vec::new, |pt| parts::scan_ready(&pdir, pt.count));
        parts::remove_stale(root, &m.path, &pdir);
        let fails = vec![0; ready.len()];
        let file = found.as_ref().map(|f| f.0.clone());
        let mut e = Entry { want: (h, fps), out, spec, pt, dur, ready, fails, complete: full, file, dead: false, ranges: Arc::default() };
        e.changed();
        registry().insert(m.path.clone(), e);
    }
    let (needs, pdir) = {
        let mut reg = registry();
        let Some(e) = reg.get_mut(&m.path) else { return (found, false) };
        // nur aufwärts: der Bau kann gerade nach der Suche fertig geworden sein
        if let Some(f) = found.as_ref().filter(|_| !e.complete) {
            e.complete = true;
            e.file = Some(f.0.clone());
            e.changed();
        }
        (!e.complete && !e.dead, parts::parts_dir(&e.out))
    };
    // zusammengefügt: Stücke weg (nicht beim Abspielen – ein laufender Strom liest sie noch)
    if full && !playing && pdir.is_dir() {
        let _ = std::fs::remove_dir_all(&pdir);
    }
    (found, needs)
}

/// Quellen am Playhead, denen dort ein Stück fehlt. Dieselbe Regel für die Wahl des nächsten Baus
/// und fürs Abgeben eines laufenden – sonst wählt die Suche das abgebende Medium gleich wieder.
fn playhead_wants() -> HashSet<PathBuf> {
    // Sperren nacheinander, nie beide zugleich
    let at: Vec<(PathBuf, f64)> = focus().iter().filter_map(|(p, f)| f.at.map(|a| (p.clone(), a.0))).collect();
    let reg = registry();
    at.into_iter().filter(|(p, t)| reg.get(p).is_some_and(|e| e.needs(*t))).map(|(p, _)| p).collect()
}

/// Hat ein anderes Medium am Playhead Vorrang (dort fehlt das Stück, hier nicht)?
fn other_media_first(src: &Path) -> bool {
    let w = playhead_wants();
    !w.contains(src) && !w.is_empty()
}

/// So viele Stück-Versuche dürfen nacheinander scheitern, dann endet der Bau (Platte voll, Quelle
/// weg) – sonst probierte jedes weitere Stück die ganze Encoder-Kette, bei 2 h tausende Starts.
const MAX_FAILS_IN_ROW: u32 = 3;

/// Gescheiterte Versuche in Folge nach einem Ergebnis; `None`: aufgeben
fn fails_in_row(prev: u32, ok: bool) -> Option<u32> {
    if ok {
        Some(0)
    } else {
        Some(prev + 1).filter(|n| *n < MAX_FAILS_IN_ROW)
    }
}

/// Stücke einer Quelle bauen, bis alle da sind (dann zusammenfügen) oder ein anderes Medium am
/// Playhead Vorrang hat. Nach jedem Stück wird neu geplant (Playhead gesprungen: dort weiter; das
/// laufende Stück wird fertig). Ein Stück, das wiederholt scheitert, wird übersprungen – dort
/// bleibt das Original. `Ok(true)`: Proxy fertig; `Ok(false)`: unterbrochen (Vorrang, veraltet).
fn build_parts(m: &MediaRef, out: &Path, spec: &ProxySpec, pt: &Parting, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<bool, String> {
    let pdir = parts::parts_dir(out);
    std::fs::create_dir_all(&pdir).map_err(|e| e.to_string())?;
    let _ = std::fs::write(pdir.join("quelle.txt"), m.path.to_string_lossy().as_bytes());
    let st = probe_start(&m.path);
    let name = m.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let (mut paused, mut in_row) = (false, 0);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Abgebrochen".into());
        }
        if other_media_first(&m.path) {
            return Ok(false);
        }
        // Rücksicht auf den Player schon vor dem Start eines Stücks
        paused = pause_now(paused);
        if paused {
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }
        let f = focus().get(&m.path).cloned().unwrap_or_default();
        let Some((next, done)) = with_mine(&m.path, out, |e| {
            let k = parts::next_part(pt, |i| e.ready[i as usize] || e.fails[i as usize] >= MAX_FAILS, &f);
            (k, e.ready.iter().filter(|r| **r).count())
        }) else {
            return Ok(false);
        };
        let Some(k) = next else { break };
        let n = pt.count.max(1) as f32;
        let base = done as f32 / n;
        let r = build_part(m, &pdir, pt, st, k, spec, &|v| progress(base + v / n), cancel);
        in_row = fails_in_row(in_row, r.is_ok()).ok_or_else(|| format!("{MAX_FAILS_IN_ROW} Stücke nacheinander gescheitert"))?;
        match r {
            Ok(_) => {
                with_mine(&m.path, out, |e| {
                    e.ready[k as usize] = true;
                    e.changed();
                });
            }
            Err(e) if e == "Abgebrochen" => return Err(e),
            Err(e) => {
                log::warn!(target: "proxy", "Stück {k} von {name} gescheitert: {e}");
                with_mine(&m.path, out, |en| en.fails[k as usize] += 1);
            }
        }
    }
    if with_mine(&m.path, out, |e| e.ready.iter().all(|r| *r)) != Some(true) {
        return Err("Proxy-Stücke fehlgeschlagen".into());
    }
    merge(&pdir, pt, out, cancel)?;
    with_mine(&m.path, out, Entry::built);
    if !PLAYING.load(Ordering::Relaxed) {
        let _ = std::fs::remove_dir_all(&pdir);
    }
    Ok(true)
}

/// Proxys im Ordner (fertige Dateien und Stück-Ordner) und ihr Platz in Bytes
fn folder_usage(dir: &Path) -> (u64, u64) {
    let (mut n, mut bytes) = (0, 0);
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            n += 1;
            bytes += std::fs::read_dir(e.path()).into_iter().flatten().flatten().filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum::<u64>();
        } else {
            n += u64::from(!e.file_name().to_string_lossy().ends_with(".part.mp4"));
            bytes += md.len();
        }
    }
    (n, bytes)
}

impl App {
    fn proxy_want(&self) -> (u32, Rational) {
        (self.settings.proxy_h, self.project.settings.fps)
    }

    /// Datei für die Vorschau: beim Abspielen der Proxy (flüssig), im Stand das Original, wenn
    /// die Zeilen des Proxys für die benötigte Höhe `need_h` nicht reichen (scharf).
    /// Vorhandene Proxys kommen aus dem Zwischenspeicher der Hintergrund-Suche.
    pub fn preview_path(&self, m: &MediaRef, need_h: u32) -> PathBuf {
        if self.settings.use_proxies && m.kind == MediaKind::Video {
            let key = self.proxy_want();
            let found = match self.proxies.cached(m.id, key) {
                Some(c) => c,
                None => existing_proxy(m, key.0, key.1),
            };
            if let Some((p, h)) = found {
                if self.playing || need_h <= h * 6 / 5 {
                    return p;
                }
            }
        }
        m.path.clone()
    }

    /// Einzelbild an Quellzeit `at`: (Datei, Zeit darin) – aus einem fertigen Stück, sonst wie
    /// [`Self::preview_path`].
    pub fn still_source(&self, m: &MediaRef, need_h: u32, at: Time) -> (PathBuf, Time) {
        if self.settings.use_proxies && m.kind == MediaKind::Video {
            let want = self.proxy_want();
            let part = registry().get(&m.path).filter(|e| e.want == want && !e.complete).and_then(|e| {
                let (k, rel) = parts::still_part(&e.pt?, at.as_secs_f64())?;
                let sharp = self.playing || need_h <= e.spec.height * 6 / 5;
                (sharp && e.ready.get(k as usize) == Some(&true)).then(|| (parts::parts_dir(&e.out).join(parts::part_name(k)), rel))
            });
            // leeres Stück (hinter dem Videoende): Original
            if let Some((p, rel)) = part.filter(|(p, _)| std::fs::metadata(p).is_ok_and(|x| x.len() > 0)) {
                return (p, Time::from_secs_f64(rel));
            }
        }
        (self.preview_path(m, need_h), at)
    }

    /// Quelle des Stroms der Ebene `id` ab Zeitleistenstelle `from` (Regeln: [`parts::choose_stream`]).
    /// Geht in den Stromschlüssel ein: ein Wechsel läuft über den Ersatzstrom des Player-Kerns.
    pub fn stream_pick(&mut self, id: u128, c: &Clip, m: &MediaRef, from: Time) -> Pick {
        if !self.settings.use_proxies || m.kind != MediaKind::Video {
            return Pick::Whole;
        }
        let want = self.proxy_want();
        let p = c.source_time(Time(from.0.clamp(c.start.0, c.end().0))).as_secs_f64();
        let mut end = (c.source_in + c.source_span()).as_secs_f64();
        if m.duration > Time::ZERO {
            end = end.min(m.duration.as_secs_f64());
        }
        let margin = parts::SWITCH_MARGIN * c.speed.max(0.01) as f64;
        let prev = self.proxy_picks.get(&id).copied();
        let pick = registry().get(&m.path).filter(|e| e.want == want).map_or(Pick::Whole, |e| match e.pt {
            _ if e.complete => Pick::Full,
            Some(pt) => parts::choose_stream(prev, &e.ready, &pt, p, end, margin),
            None => Pick::Whole,
        });
        self.proxy_picks.insert(id, pick);
        pick
    }

    /// Datei eines Stroms und Quellzeit ihres Anfangs (Stücke beginnen mitten in der Quelle).
    pub fn stream_source(&self, m: &MediaRef, need_h: u32, pick: Pick) -> (PathBuf, Time) {
        match pick {
            Pick::Original { .. } => (m.path.clone(), Time::ZERO),
            Pick::Parts { first, end } => {
                let at = registry().get(&m.path).and_then(|e| Some((parts::parts_dir(&e.out), e.pt?.start(first))));
                at.and_then(|(d, s)| parts::concat_list(&d, first, end).ok().map(|l| (l, Time::from_secs_f64(s))))
                    .unwrap_or_else(|| (m.path.clone(), Time::ZERO))
            }
            // Datei aus dem Bestand (gerade fertig geworden: die Suche kennt sie evtl. noch nicht),
            // ohne Dateizugriff im UI-Thread
            Pick::Full => (registry().get(&m.path).and_then(|e| e.file.clone()).unwrap_or_else(|| self.preview_path(m, need_h)), Time::ZERO),
            Pick::Whole => (self.preview_path(m, need_h), Time::ZERO),
        }
    }

    /// Zeitleiste: dünne Linie am unteren Rand des Clips – welche Teile seiner Quelle schon als
    /// Proxy-Stück vorliegen; blendet aus, sobald alles da ist.
    pub fn proxy_line(&self, ctx: &egui::Context, p: &egui::Painter, c: &Clip, r: egui::Rect) {
        let ClipContent::Media { media, .. } = c.content else { return };
        if !self.settings.use_proxies {
            return;
        }
        let Some(m) = self.project.media(media) else { return };
        let want = self.proxy_want();
        let Some((ranges, complete)) = registry().get(&m.path).filter(|e| e.want == want).map(|e| (e.ranges.clone(), e.complete)) else {
            return;
        };
        let a = theme::fade(ctx, egui::Id::new(("proxy-line", c.id)), !complete, theme::MOTION);
        if a <= 0.0 {
            return;
        }
        let line = egui::Rect::from_min_max(egui::pos2(r.left(), r.bottom() - 2.0), r.max);
        p.rect_filled(line, 0.0, theme::ACCENT().gamma_multiply(0.25 * a));
        for (x0, x1) in parts::clip_coverage(&ranges, c.source_in.as_secs_f64(), c.source_span().as_secs_f64(), c.fx.reverse) {
            let seg = egui::Rect::from_x_y_ranges(line.left() + line.width() * x0..=line.left() + line.width() * x1, line.y_range());
            p.rect_filled(seg, 0.0, theme::ACCENT().gamma_multiply(a));
        }
    }

    /// Wo der Bau zuerst hin soll (Playhead, sichtbare und benutzte Bereiche je Quelle). Nur neu,
    /// wenn sich Playhead, Ansicht oder Projekt ändern; beim Abspielen höchstens alle 250 ms.
    fn update_focus(&self, ctx: &egui::Context) {
        static LAST: Mutex<(u64, Option<Instant>)> = Mutex::new((0, None));
        let w = ctx.screen_rect().width();
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        (self.playhead.0, self.timeline.scroll.to_bits(), self.timeline.pps.to_bits(), w.to_bits(), self.project_epoch, self.project.media.len(), self.revision).hash(&mut hs);
        // nur Quellen, die gebaut werden können: ein fehlendes (NAS schläft) oder gescheitertes
        // Medium am Playhead ließe den laufenden Bau sonst immer wieder abgeben
        let usable = |m: &MediaRef| m.kind == MediaKind::Video && self.presence.get(&m.path) && !self.proxy_failed.contains(&m.id);
        for m in &self.project.media {
            (m.id, usable(m)).hash(&mut hs);
        }
        let sig = hs.finish();
        {
            let mut l = LAST.lock().unwrap_or_else(|e| e.into_inner());
            if l.0 == sig || (self.playing && l.1.is_some_and(|t| t.elapsed() < Duration::from_millis(250))) {
                return;
            }
            *l = (sig, Some(Instant::now()));
        }
        let paths: HashMap<MediaId, &Path> = self.project.media.iter().filter(|m| usable(m)).map(|m| (m.id, m.path.as_path())).collect();
        let v0 = self.timeline.scroll;
        let v1 = v0 + (w / self.timeline.pps.max(1e-3)) as f64;
        let mut map: HashMap<PathBuf, Focus> = HashMap::new();
        for c in self.project.timeline.tracks.iter().flat_map(|t| &t.clips) {
            let ClipContent::Media { media, .. } = c.content else { continue };
            let Some(p) = paths.get(&media) else { continue };
            let f = map.entry(p.to_path_buf()).or_default();
            let a = c.source_in.as_secs_f64();
            let b = a + c.source_span().as_secs_f64();
            f.used.push((a, b));
            let (s, e) = (c.start.as_secs_f64(), c.end().as_secs_f64());
            if s < v1 && v0 < e {
                let x = c.source_time(Time::from_secs_f64(s.max(v0))).as_secs_f64();
                let y = c.source_time(Time::from_secs_f64(e.min(v1))).as_secs_f64();
                f.visible.push((x.min(y), x.max(y)));
            }
            if c.contains(self.playhead) {
                let t = c.source_time(self.playhead).as_secs_f64();
                f.at = Some((t, if c.fx.reverse { t } else { b }));
            }
        }
        *focus() = map;
    }

    /// Fehlende Proxys im Hintergrund erzeugen (nacheinander, je Medium eine Aufgabe; in Stücken).
    /// Welche fehlen (und welche Stücke schon da sind), sucht ein Hintergrund-Thread; hier nur
    /// Ergebnis abholen und den nächsten Bau starten.
    pub fn ensure_proxies(&mut self, ctx: &egui::Context) {
        let _s = crate::app::section("Proxys");
        set_play_state(self.playing, !self.settings.auto_quality || self.core.quality.level > 0);
        if !self.core.streaming() || self.proxy_picks.len() > 512 {
            self.proxy_picks.clear();
        }
        let key = self.proxy_want();
        if self.proxies.key != Some(key) {
            self.proxies.known.clear();
            self.proxies.key = Some(key);
            self.proxies.scanned = None;
        }
        if !self.settings.use_proxies || self.proxies.stopped {
            return;
        }
        self.update_focus(ctx);
        // Suche auch während eines Baus: ein neues Medium am Playhead bekommt seinen Eintrag, und
        // der laufende Bau gibt den Vorrang ab (siehe `other_media_first`)
        let res = match &self.proxies.scan {
            Some(rx) => match rx.try_recv() {
                Ok(r) => r,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.proxies.scan = None;
                    return;
                }
            },
            None => {
                let n = self.project.media.len();
                if scan_due(self.proxies.scanned, n) {
                    self.proxies.scanned = Some((Instant::now(), n));
                    // alle Videos (auch der Player fragt nach Proxys), gebaut nur, wo es sich lohnt
                    let media: Vec<(MediaRef, bool)> = self
                        .project
                        .media
                        .iter()
                        .filter(|m| m.kind == MediaKind::Video && self.presence.get(&m.path))
                        .map(|m| (m.clone(), wants_proxy(m) && !self.proxy_failed.contains(&m.id)))
                        .collect();
                    let (tx, rx) = std::sync::mpsc::channel();
                    let (c, playing) = (ctx.clone(), self.playing);
                    std::thread::spawn(move || {
                        // Reste vom letzten Lauf (auch nach hartem Beenden), einmal je Sitzung
                        static CLEANED: std::sync::Once = std::sync::Once::new();
                        let root = dir();
                        CLEANED.call_once(|| parts::cleanup(&root));
                        let mut found = Vec::new();
                        let mut missing = Vec::new();
                        for (m, want) in &media {
                            if *want {
                                let (p, needs) = refresh_entry(&root, m, key.0, key.1, playing);
                                found.push((m.id, p));
                                if needs {
                                    missing.push(m.id);
                                }
                            } else {
                                found.push((m.id, existing_proxy_in(&root, m, key.0, key.1)));
                            }
                        }
                        let _ = tx.send((found, missing));
                        c.request_repaint();
                    });
                    self.proxies.scan = Some(rx);
                }
                return;
            }
        };
        self.proxies.scan = None;
        let (found, missing) = res;
        self.proxies.known.extend(found);
        (self.proxies.done, self.proxies.total) = round(self.proxies.done, missing.len());
        if missing.is_empty() || self.tasks.running("proxy").is_some() {
            return;
        }
        let Some(m) = self.next_build(&missing, ctx).and_then(|id| self.project.media.iter().find(|m| m.id == id)).cloned() else {
            return;
        };
        let spec = proxy_spec(&m, key.0, key.1);
        // Ziel und Einteilung stehen im Bestand (von der Suche im Hintergrund ermittelt)
        let target = registry().get(&m.path).filter(|e| e.want == key).map(|e| (e.out.clone(), e.pt));
        let id = m.id;
        let label = format!("Proxys {} / {}", self.proxies.done + 1, self.proxies.total);
        self.tasks.spawn(ctx, "proxy", label, move |p, cancel| {
            let t0 = std::time::Instant::now();
            let name = m.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let r = match target {
                Some((out, Some(pt))) => build_parts(&m, &out, &spec, &pt, p, cancel),
                Some((out, None)) => build(&m, &out, &spec, p, cancel).map(|_| true),
                None => build(&m, &proxy_path(&m, key.0, key.1), &spec, p, cancel).map(|_| true),
            };
            if r.is_err() {
                if let Some(e) = registry().get_mut(&m.path) {
                    e.dead = true;
                }
            }
            TaskDone::Proxy { media: id, ok: crate::applog::outcome("proxy", &name, t0, r).is_ok() }
        });
    }

    /// Welches fehlende Medium als nächstes: am Playhead nur, wo dort ein Stück fehlt (wie beim
    /// Abgeben, siehe [`other_media_first`]), dann sichtbare, dann der Rest.
    fn next_build(&self, missing: &[MediaId], ctx: &egui::Context) -> Option<MediaId> {
        let wants = playhead_wants();
        let at: HashSet<MediaId> = self.project.media.iter().filter(|m| wants.contains(&m.path)).map(|m| m.id).collect();
        let v0 = Time::from_secs_f64(self.timeline.scroll);
        let v1 = Time::from_secs_f64(self.timeline.scroll + (ctx.screen_rect().width() / self.timeline.pps.max(1e-3)) as f64);
        let vis: HashSet<MediaId> = self
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.start < v1 && v0 < c.end())
            .filter_map(|c| match c.content {
                ClipContent::Media { media, .. } => Some(media),
                _ => None,
            })
            .collect();
        next_proxy(missing, &at, &vis)
    }

    /// Ein Proxy-Auftrag ist zu Ende: gleich weitersuchen. Gezählt wird nur, was fertig oder
    /// gescheitert ist – ein Bau, der einem Medium am Playhead Vorrang gab, kommt wieder.
    pub fn proxy_finished(&mut self, media: MediaId, ok: bool) {
        let complete = self
            .project
            .media(media)
            .is_none_or(|m| registry().get(&m.path).is_none_or(|e| e.complete));
        if complete || !ok {
            self.proxies.done += 1;
        }
        // Zwischenspeicher bleibt bis zur gleich folgenden Suche (leer fragte der Player die Platte)
        self.proxies.scanned = None;
    }

    /// Proxy-Bau für dieses Projekt beenden (Abbrechen in der Aufgaben-Anzeige)
    pub fn stop_proxies(&mut self) {
        self.proxies.stopped = true;
        if let Some(t) = self.tasks.running("proxy") {
            t.cancel();
        }
    }

    /// Einstellungen → Leistung: Proxys
    pub fn proxy_settings(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "Proxys");
        if widgets::toggle_row(
            ui,
            "Proxys für große Videos verwenden",
            &mut self.settings.use_proxies,
        ) {
            self.settings.save();
            self.invalidate_preview();
        }
        theme::row(ui, "Qualität", |ui| {
            let names: Vec<&str> = PROXY_HEIGHTS.iter().map(|p| p.1).collect();
            let mut i = PROXY_HEIGHTS.iter().position(|p| p.0 == self.settings.proxy_h).unwrap_or(0);
            if widgets::segmented(ui, &names, &mut i) {
                self.settings.proxy_h = PROXY_HEIGHTS[i].0;
                self.proxies.stopped = false;
                self.settings.save();
                self.proxy_failed.clear();
                self.invalidate_preview();
            }
        });
        // Ordner höchstens alle 2 s zählen (fertige Proxys und Stück-Ordner)
        let (n, bytes) = match self.proxy_stats {
            Some((t, n, b)) if t.elapsed() < std::time::Duration::from_secs(2) => (n, b),
            _ => {
                let (n, b) = folder_usage(&dir());
                self.proxy_stats = Some((std::time::Instant::now(), n, b));
                (n, b)
            }
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(crate::tr(format!(
                    "{n} Proxys · {:.0} MB",
                    bytes as f64 / 1e6
                )))
                .small()
                .color(theme::TEXT_DIM()),
            );
            if n > 0
                && ui
                    .add(theme::ghost(format!("{}  Proxys löschen", icon::TRASH)))
                    .clicked()
            {
                // ein laufender Bau merkt das nach seinem Stück (Eintrag fehlt) und endet
                let _ = std::fs::remove_dir_all(dir());
                registry().clear();
                self.proxy_picks.clear();
                self.proxy_stats = None;
                self.proxies.known.clear();
                self.proxies.scanned = None;
                self.invalidate_preview();
            }
        });
    }
}

/// Test: Bestand für `src` setzen – Quelle 100 s, Stücke à 10 s, fertig laut `ready` (je fertigem
/// Stück eine Datei)
#[cfg(test)]
pub(crate) fn set_test_entry(src: &Path, out: PathBuf, want: (u32, Rational), ready: Vec<bool>, complete: bool) {
    let pt = Parting::new(100.0, want.1, PART_SECS).unwrap();
    let pdir = parts::parts_dir(&out);
    std::fs::create_dir_all(&pdir).unwrap();
    for k in (0..ready.len()).filter(|k| ready[*k]) {
        std::fs::write(pdir.join(parts::part_name(k as u32)), b"x").unwrap();
    }
    let spec = ProxySpec { height: want.0, fps: None, gop: 15 };
    let fails = vec![0; ready.len()];
    let file = complete.then(|| out.clone());
    let mut e = Entry { want, out, spec, pt: Some(pt), dur: 100.0, ready, fails, complete, file, dead: false, ranges: Arc::default() };
    e.changed();
    registry().insert(src.to_path_buf(), e);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(w: u32, h: u32, fps: Rational, codec: &str) -> MediaRef {
        MediaRef {
            id: MediaId::new(),
            path: PathBuf::from("quelle.mp4"),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(60.0),
            width: w,
            height: h,
            fps: Some(fps),
            has_audio: false,
            audio_streams: 0,
            video_codec: Some(codec.into()),
            audio_codec: None,
        }
    }

    #[test]
    fn spec_follows_project_rate_and_keeps_short_gop() {
        let r = |n| Rational::new(n, 1);
        // 60-fps-Quelle im 30-fps-Projekt → 30 fps, Schlüsselbild alle 15 Bilder (0,5 s)
        let s = proxy_spec(&media(1920, 1080, r(60), "h264"), 540, r(30));
        assert_eq!(s, ProxySpec { height: 540, fps: Some(r(30)), gop: 15 });
        // im 60-fps-Projekt bleibt es bei 60
        assert_eq!(proxy_spec(&media(1920, 1080, r(60), "h264"), 540, r(60)), ProxySpec { height: 540, fps: None, gop: 30 });
        // langsamere Quelle wird nicht hochgerechnet; 29,97 im 30er-Projekt bleibt
        let ntsc = Rational::new(30000, 1001);
        assert_eq!(proxy_spec(&media(3840, 2160, ntsc, "hevc"), 720, r(30)).fps, None);
        assert_eq!(proxy_spec(&media(3840, 2160, r(25), "hevc"), 720, r(30)).gop, 13);
        // 120 fps im 60er-Projekt; nie höher als das Original
        let s = proxy_spec(&media(1280, 720, r(120), "h264"), 1080, r(60));
        assert_eq!((s.height, s.fps, s.gop), (720, Some(r(60)), 30));
        // kaputte Angaben: keine Division durch null, GOP mindestens 1
        let mut m = media(1920, 1080, Rational::new(0, 0), "h264");
        assert_eq!(proxy_spec(&m, 540, Rational::new(0, 0)).fps, None);
        m.fps = Some(Rational::new(1, 1));
        assert_eq!(proxy_spec(&m, 540, r(30)).gop, 1);
    }

    #[test]
    fn existing_and_larger_proxies_stay_valid() {
        let m = media(1920, 1080, Rational::new(60, 1), "h264");
        let c = candidates(&m, 540, Rational::new(30, 1), 0xab);
        let names: Vec<&str> = c.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "00000000000000ab-540-30_1fps.mp4",
                "00000000000000ab-540.mp4",
                "00000000000000ab-720-30_1fps.mp4",
                "00000000000000ab-720.mp4",
                "00000000000000ab-1080-30_1fps.mp4",
                "00000000000000ab-1080.mp4",
            ]
        );
        // 1080 gewählt: nur 1080 (kleinere Proxys wären unschärfer als gewünscht)
        assert_eq!(candidates(&m, 1080, Rational::new(30, 1), 1).len(), 2);
        // Quelle nicht schneller als das Projekt: nur die Namen ohne Bildrate (wie früher)
        let m = media(3840, 2160, Rational::new(30, 1), "hevc");
        assert!(candidates(&m, 720, Rational::new(30, 1), 1).iter().all(|(n, _)| !n.contains("fps")));
    }

    #[test]
    fn encoder_chain_falls_back_to_software() {
        use Encoder::*;
        assert_eq!(encoder_chain(&[Software, Mf, Amf, Nvenc, Qsv], &[], "auto"), [Nvenc, Qsv, Amf, Mf, Software]);
        assert_eq!(encoder_chain(&[Software, Nvenc, Qsv], &[Nvenc], "auto"), [Qsv, Software]);
        assert_eq!(encoder_chain(&[], &[], "auto"), [Software]);
        assert_eq!(encoder_chain(&[Software], &[Software], "auto"), [Software], "Software bleibt immer");
        // Einstellung: gewählter Weg vorn; gescheitert oder nicht gefunden → wie automatisch bzw. Software
        assert_eq!(encoder_chain(&[Software, Nvenc, Mf], &[], "mf"), [Mf, Software]);
        assert_eq!(encoder_chain(&[Software, Nvenc, Mf], &[Mf], "mf"), [Software]);
        assert_eq!(encoder_chain(&[Software, Mf], &[], "nvenc"), [Mf, Software]);
        assert_eq!(encoder_chain(&[Software, Nvenc], &[], "software"), [Software]);
        let m = media(3840, 2160, Rational::new(60, 1), "hevc");
        assert_eq!(hw_decode(Nvenc, &m), Some("cuda"));
        assert_eq!(hw_decode(Software, &m), None);
        assert_eq!(hw_decode(Nvenc, &media(1920, 1080, Rational::new(60, 1), "h264")), None);
    }

    #[test]
    fn encode_args_set_size_rate_and_gop() {
        // Stücke (und der Bau in einem Zug als ein Stück über alles): gleiches Raster wie ein Bau in einem Zug, genau `per` Bilder, ohne B-Bilder
        // (Stücke verschiedener Encoder ergaben zusammengefügt ein doppeltes Bild an der Naht)
        let pt = Parting::new(25.0, Rational::new(30, 1), 10.0).unwrap();
        for enc in Encoder::AUTO {
            let a = part_args(Path::new("q.mp4"), 1.5, &pt, 2, 540, 15, enc, None).join(" ");
            assert!(a.contains("-ss 19.500000 -copyts -i q.mp4"), "{a}");
            assert!(a.contains("fps=30/1:start_time=21.500000,setpts=PTS-STARTPTS,scale=-2:540"), "{a}");
            assert!(a.ends_with("-bf 0 -g 15 -frames:v 300 -r 30/1"), "{a}");
        }
        // Media Foundation: nv12, nur Hardware, Bitrate nach Größe und Bildrate
        let a = part_args(Path::new("q.mp4"), 1.5, &pt, 2, 540, 15, Encoder::Mf, None).join(" ");
        assert!(a.contains("-c:v h264_mf -pix_fmt nv12 -hw_encoding 1 -b:v 2499k -bf 0"), "{a}");
        let a = part_args(Path::new("q.mp4"), 1.5, &pt, 2, 1080, 15, Encoder::Mf, None).join(" ");
        assert!(a.contains("-b:v 9998k"), "{a}");
    }

    #[test]
    fn pause_on_clicks_and_while_playback_falls_behind() {
        let ms = Duration::from_millis;
        // Stand: frischer Klick → anhalten; erst nach 0,8 s Ruhe fortsetzen
        assert!(want_pause(false, false, false, ms(100)));
        assert!(!want_pause(false, false, false, ms(400)));
        assert!(want_pause(true, false, false, ms(400)));
        assert!(!want_pause(true, false, false, ms(900)));
        // Wiedergabe ohne Rückstand: der Bau läuft weiter (auch bei Einzelbild-Aufträgen) …
        assert!(!want_pause(false, true, false, ms(0)));
        assert!(!want_pause(true, true, false, Duration::MAX));
        // … mit Rückstand angehalten
        assert!(want_pause(false, true, true, Duration::MAX));
        assert!(want_pause(true, true, true, ms(0)));
        // Rückstand gilt bis zum Stopp (kein Hin und Her mit der automatischen Qualität)
        assert!(latch_behind(false, true, true));
        assert!(latch_behind(true, true, false));
        assert!(!latch_behind(true, false, false));
        assert!(!latch_behind(false, true, false));
    }

    #[test]
    fn scan_runs_every_two_seconds_or_on_new_media_and_counts_the_round() {
        assert!(scan_due(None, 0));
        let now = Some((Instant::now(), 5));
        assert!(!scan_due(now, 5));
        assert!(scan_due(now, 6));
        assert!(scan_due(Instant::now().checked_sub(Duration::from_secs(3)).map(|t| (t, 5)), 5));
        // 3 fertig, 7 fehlen → „Proxys 4 / 10“; nichts fehlt → Runde vorbei
        assert_eq!(round(3, 7), (3, 10));
        assert_eq!(round(10, 0), (0, 0));
    }

    #[test]
    fn cached_proxy_only_for_the_same_setting() {
        let mut s = ProxyState::default();
        let (id, r) = (MediaId::new(), Rational::new(30, 1));
        s.key = Some((540, r));
        s.known.insert(id, Some((PathBuf::from("p.mp4"), 540)));
        assert_eq!(s.cached(id, (540, r)), Some(Some((PathBuf::from("p.mp4"), 540))));
        assert_eq!(s.cached(id, (720, r)), None, "andere Höhe: unbekannt");
        assert_eq!(s.cached(MediaId::new(), (540, r)), None);
        s.reset();
        assert_eq!(s.cached(id, (540, r)), Some(Some((PathBuf::from("p.mp4"), 540))), "bleibt über Projektwechsel");
    }

    #[test]
    fn folder_usage_counts_parts() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a-540.mp4"), [0u8; 10]).unwrap();
        std::fs::write(d.path().join("b-540.part.mp4"), [0u8; 5]).unwrap();
        let t = d.path().join("c-540.teile");
        std::fs::create_dir(&t).unwrap();
        std::fs::write(t.join("0000.mp4"), [0u8; 7]).unwrap();
        std::fs::write(t.join("0001.mp4"), [0u8; 3]).unwrap();
        // zwei Proxys (fertig, in Stücken), alle Bytes
        assert_eq!(folder_usage(d.path()), (2, 25));
        assert_eq!(folder_usage(&d.path().join("fehlt")), (0, 0));
    }

    /// Ältere Ein-Datei-Proxys gelten weiter: kein Stück-Bau, Strom aus der fertigen Datei.
    #[test]
    fn old_single_file_proxy_is_used_not_rebuilt() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("quelle.mp4");
        std::fs::write(&src, b"x").unwrap();
        let mut m = media(1920, 1080, Rational::new(30, 1), "h264");
        m.path = src;
        let root = d.path().join("proxies");
        std::fs::create_dir_all(&root).unwrap();
        // Name wie früher (ohne Bildrate)
        std::fs::write(root.join(format!("{:016x}-540.mp4", media_hash(&m))), b"p").unwrap();
        let (found, needs) = refresh_entry(&root, &m, 540, Rational::new(30, 1), false);
        assert!(!needs, "nichts zu bauen");
        assert_eq!(found.map(|f| f.1), Some(540));
        let reg = registry();
        let e = reg.get(&m.path).unwrap();
        assert!(e.complete && !e.needs(1.0));
        assert!(!parts::parts_dir(&e.out).exists());
    }

    /// Quelle mit Bildnummer im Grauwert (Bild n: Luma 19 + (n mod 32)·6, im Videobereich 16–235), 60 fps.
    fn coded_source(dir: &Path, name: &str, secs: f64, extra_vf: &str, extra: &[&str]) -> MediaRef {
        let src = dir.join(name);
        let vf = format!("geq=lum='19+mod(N\\,32)*6':cb=128:cr=128{extra_vf}");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("color=c=black:s=320x180:r=60:d={secs}"), "-vf", &vf])
            .args(extra)
            .args(["-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", "-g", "600", "-threads", "2"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        cl_media::probe(&src).unwrap()
    }

    fn code_of_luma(y: f32) -> u8 {
        (((y - 19.0) / 6.0).round().max(0.0) as u8) % 32
    }

    /// Bildnummern (mod 32) einer Datei, Bild für Bild (Luma direkt, ohne Umrechnung)
    fn codes(path: &Path) -> Vec<u8> {
        let o = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(path)
            .args(["-vf", "scale=2:2:flags=area", "-pix_fmt", "yuv420p", "-f", "rawvideo", "-"])
            .output()
            .unwrap();
        o.stdout.chunks_exact(6).map(|f| code_of_luma(f[0] as f32)).collect()
    }

    /// Stücke über mehrere Nähte = ein Bau in einem Zug: kein doppeltes, kein fehlendes Bild – auch
    /// bei Startversatz und schwankender Bildrate. Zusammengefügt dieselben Bilder; ein Strom über
    /// die Stück-Liste ab einer Stelle mitten im zweiten Stück liefert die passenden Bilder.
    /// Wo Media Foundation startet, wechseln sich die Stücke zwischen ihr und Software ab.
    #[test]
    fn parts_join_frame_exact_and_merge_keeps_every_frame() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let mix = if available_encoders().contains(&Encoder::Mf) { vec![Encoder::Mf, Encoder::Software] } else { vec![Encoder::Software] };
        let d = tempfile::tempdir().unwrap();
        let vfr = ",setpts='(N/60+0.006*sin(N*1.7))/TB'";
        let cases: [(&str, &str, &[&str]); 3] = [
            ("cfr.mp4", "", &[]),
            ("versatz.mp4", "", &["-output_ts_offset", "1.37"]),
            ("vfr.mp4", vfr, &["-fps_mode", "passthrough", "-video_track_timescale", "90000"]),
        ];
        for (name, vf, extra) in cases {
            let m = coded_source(d.path(), name, 4.5, vf, extra);
            let spec = proxy_spec(&m, 90, Rational::new(30, 1));
            let pt = Parting::new(m.duration.as_secs_f64(), out_fps(&m, &spec), 1.0).unwrap();
            assert!(pt.count >= 4, "{name}: {pt:?}");
            let st = probe_start(&m.path);
            let pdir = d.path().join(format!("{name}.teile"));
            let no = AtomicBool::new(false);
            let mut joined = Vec::new();
            for k in 0..pt.count {
                let n = build_part_with(&m, &pdir, &pt, st, k, &spec, mix[k as usize % mix.len()], None, &|_| {}, &no).unwrap();
                let c = codes(&pdir.join(parts::part_name(k)));
                assert_eq!(c.len() as u64, n);
                if k + 1 < pt.count {
                    assert_eq!(n, pt.per as u64, "{name}: Stück {k} hat nicht genau {} Bilder", pt.per);
                }
                joined.extend(c);
            }
            // Vergleich: derselbe Weg in einem Zug (ein „Stück“ über alles)
            let whole = Parting { per: 1_000_000, count: 1, ..pt };
            let wdir = d.path().join(format!("{name}.ganz"));
            build_part_with(&m, &wdir, &whole, st, 0, &spec, Encoder::Software, None, &|_| {}, &no).unwrap();
            let reference = codes(&wdir.join(parts::part_name(0)));
            assert_eq!(joined, reference, "{name}: Nähte nicht bildgenau");
            if name == "cfr.mp4" {
                // 60 → 30 fps: jedes zweite Quellbild
                assert!(reference.windows(2).all(|w| (w[1] + 32 - w[0]) % 32 == 2), "{reference:?}");
            }
            // zusammenfügen: dieselben Bilder
            let out = d.path().join(format!("{name}-ganz.mp4"));
            merge(&pdir, &pt, &out, &no).unwrap();
            assert_eq!(codes(&out), reference, "{name}: zusammengefügt anders");
            assert!(!parts::tmp_of(&out).exists());
            // Strom über die Stücke 1.. ab 1,5 s (0,5 s ins Stück 1): Bild 45 und folgende
            if name == "cfr.mp4" {
                let list = parts::concat_list(&pdir, 1, pt.count).unwrap();
                let s = cl_media::FrameStream::open(&list, Time::from_secs_f64(1.5 - pt.start(1)), 2, 2, Rational::new(30, 1)).unwrap();
                // RGBA (Vollbereich) zurück in Luma
                let got: Vec<u8> = std::iter::from_fn(|| s.next_blocking()).map(|f| code_of_luma(16.0 + f.data[0] as f32 * 219.0 / 255.0)).collect();
                assert!(got.len() >= 80, "{}", got.len());
                assert_eq!(&got[..80], &reference[45..125], "Strom über die Naht");
            }
        }
    }

    /// Bildnummer des Einzelbilds, das der Player an `t` aus `path` holt (wie für das Original)
    fn still_code(path: &Path, t: f64) -> u8 {
        let (f, _) = cl_media::decode_frame_opts(path, Time::from_secs_f64(t), 2, 2, None, false, None).unwrap();
        code_of_luma(16.0 + f.data[0] as f32 * 219.0 / 255.0)
    }

    /// Videospur beginnt 0,1 s nach dem Ton: Stück, zusammengefügter Proxy und Bau in einem Zug
    /// zeigen an derselben Quellzeit dasselbe Bild wie das Original (`-ss` zählt ab Dateianfang).
    #[test]
    fn parts_and_proxy_match_the_original_when_video_starts_late() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("spaet.mp4");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-itsoffset", "0.1", "-f", "lavfi", "-i", "color=c=black:s=320x180:r=60:d=4", "-f", "lavfi", "-i", "sine=d=4.1"])
            .args(["-vf", "geq=lum='19+mod(N\\,32)*6':cb=128:cr=128", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", "-g", "600"])
            .args(["-c:a", "aac", "-threads", "2"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        let m = cl_media::probe(&src).unwrap();
        let spec = proxy_spec(&m, 90, Rational::new(30, 1));
        let pt = Parting::new(m.duration.as_secs_f64(), out_fps(&m, &spec), 1.0).unwrap();
        let pdir = d.path().join("s.teile");
        let no = AtomicBool::new(false);
        let st = probe_start(&src);
        for k in 0..pt.count {
            build_part_with(&m, &pdir, &pt, st, k, &spec, Encoder::Software, None, &|_| {}, &no).unwrap();
        }
        let merged = d.path().join("s-ganz.mp4");
        merge(&pdir, &pt, &merged, &no).unwrap();
        let whole = d.path().join("s-zug.mp4");
        build_with(&m, &whole, &spec, Encoder::Software, None, &|_| {}, &no).unwrap();
        // Zeiten auf dem Raster beider Bildraten (30 fps im Proxy, 60 fps im Original)
        for t in [0.0, 1.0, 1.5, 2.0 + 1.0 / 30.0, 3.0] {
            let want = still_code(&src, t);
            let (k, rel) = parts::still_part(&pt, t).unwrap();
            assert_eq!(still_code(&pdir.join(parts::part_name(k)), rel), want, "Stück bei {t} s");
            assert_eq!(still_code(&merged, t), want, "zusammengefügt bei {t} s");
            assert_eq!(still_code(&whole, t), want, "in einem Zug bei {t} s");
        }
    }

    /// Video kürzer als die Datei (Ton länger): das letzte Stück hat kein Bild, der Bau wird trotzdem fertig.
    #[test]
    fn part_behind_video_end_is_empty_and_merge_still_works() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("kurz.mp4");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=2", "-f", "lavfi", "-i", "sine=d=3.5"])
            .args(["-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", "-threads", "2"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        let m = cl_media::probe(&src).unwrap();
        let spec = proxy_spec(&m, 90, Rational::new(30, 1));
        let pt = Parting::new(m.duration.as_secs_f64(), out_fps(&m, &spec), 1.0).unwrap();
        assert!(pt.count >= 3, "{pt:?}");
        let pdir = d.path().join("k.teile");
        let no = AtomicBool::new(false);
        // Zwischendateien eines zweiten Fensters (anderer Prozess) bleiben unberührt
        std::fs::create_dir_all(&pdir).unwrap();
        let foreign = [pdir.join("0000.part.mp4"), pdir.join("0000.1.part.mp4")];
        for f in &foreign {
            std::fs::write(f, b"fremd").unwrap();
        }
        let n: Vec<u64> = (0..pt.count).map(|k| build_part_with(&m, &pdir, &pt, 0.0, k, &spec, Encoder::Software, None, &|_| {}, &no).unwrap()).collect();
        assert!(foreign.iter().all(|f| std::fs::read(f).unwrap() == b"fremd"), "fremde Zwischendatei überschrieben");
        assert_eq!(*n.last().unwrap(), 0, "{n:?}");
        assert_eq!(std::fs::metadata(pdir.join(parts::part_name(pt.count - 1))).unwrap().len(), 0);
        let out = d.path().join("k.mp4");
        merge(&pdir, &pt, &out, &no).unwrap();
        assert_eq!(codes(&out).len() as u64, n.iter().sum::<u64>());
        assert_eq!(n.iter().sum::<u64>(), 60);
    }

    /// Neustart mitten im Bau: abgebrochenes Stück und hart liegen gebliebene `.part` werden
    /// aufgeräumt, nur Fehlendes wird gebaut (ein fertiges Stück nie neu), am Ende eine Datei.
    #[test]
    fn resume_after_abort_builds_only_missing_parts() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let m = coded_source(d.path(), "q.mp4", 4.0, "", &[]);
        let spec = proxy_spec(&m, 90, Rational::new(30, 1));
        let pt = Parting::new(m.duration.as_secs_f64(), out_fps(&m, &spec), 1.0).unwrap();
        assert_eq!(pt.count, 4);
        let root = d.path().join("proxies");
        let out = root.join("q-90.mp4");
        let pdir = parts::parts_dir(&out);
        let no = AtomicBool::new(false);
        // erster Lauf: Stücke 0 und 2 fertig, Stück 1 abgebrochen, Stück 3 „hart beendet“
        for k in [0, 2] {
            build_part_with(&m, &pdir, &pt, 0.0, k, &spec, Encoder::Software, None, &|_| {}, &no).unwrap();
        }
        let cancel = AtomicBool::new(false);
        let r = build_part_with(&m, &pdir, &pt, 0.0, 1, &spec, Encoder::Software, None, &|_| cancel.store(true, Ordering::Relaxed), &cancel);
        assert_eq!(r.unwrap_err(), "Abgebrochen");
        std::fs::write(pdir.join("0003.part.mp4"), b"halb").unwrap();
        // Merkmal: Stück 2 durch eine Kopie von Stück 0 ersetzen – würde es neu gebaut, wäre es wieder richtig
        std::fs::copy(pdir.join(parts::part_name(0)), pdir.join(parts::part_name(2))).unwrap();
        // „Neustart“: aufräumen, Bestand lesen, weiterbauen
        parts::cleanup(&root);
        assert!(!pdir.join("0003.part.mp4").exists() && !parts::tmp_of(&pdir.join(parts::part_name(1))).exists());
        let ready = parts::scan_ready(&pdir, pt.count);
        assert_eq!(ready, [true, false, true, false]);
        let mut e = Entry {
            want: (90, Rational::new(30, 1)),
            out: out.clone(),
            spec,
            pt: Some(pt),
            dur: m.duration.as_secs_f64(),
            fails: vec![0; 4],
            ready,
            complete: false,
            file: None,
            dead: false,
            ranges: Arc::default(),
        };
        e.changed();
        registry().insert(m.path.clone(), e);
        assert_eq!(build_parts(&m, &out, &spec, &pt, &|_| {}, &no), Ok(true));
        let c = codes(&out);
        assert_eq!(c.len(), 120);
        // Stück 2 blieb die Kopie von Stück 0 (nicht neu gebaut), 1 und 3 sind richtig
        assert_eq!(&c[60..90], &c[0..30], "fertiges Stück neu gebaut");
        assert!(c[30..60].iter().enumerate().all(|(i, v)| *v == ((30 + i) * 2 % 32) as u8), "{c:?}");
        assert!(c[90..].iter().enumerate().all(|(i, v)| *v == ((90 + i) * 2 % 32) as u8), "{c:?}");
        assert!(registry().get(&m.path).unwrap().complete);
        assert!(!pdir.exists(), "Stücke nach dem Zusammenfügen gelöscht");
    }

    /// Zwei Videos übereinander am Playhead: Ist beim ersten das Playhead-Stück fertig, kommt das
    /// zweite dran, und nur dann gibt der Bau des ersten ab (vorher wählte die Suche immer wieder das
    /// erste, das sofort wieder abgab). Fehlt das zweite (NAS schläft) oder ist es gescheitert, gibt
    /// niemand ab.
    #[test]
    fn second_media_at_playhead_gets_its_part() {
        let (mut app, home, _g) = crate::saving::tests::test_app();
        let ctx = egui::Context::default();
        let mk = |name: &str| {
            let mut m = media(3840, 2160, Rational::new(30, 1), "h264");
            m.path = home.path().join(name);
            m.duration = Time::from_secs_f64(100.0);
            m
        };
        let (x, y) = (mk("x.mp4"), mk("y.mp4"));
        for m in [&x, &y] {
            let mut t = cl_core::Track::new(cl_core::TrackKind::Video);
            t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(100.0)));
            app.project.timeline.tracks.push(t);
            app.project.media.push(m.clone());
        }
        app.playhead = Time::from_secs_f64(42.0);
        let ready = |k: Option<usize>| (0..10).map(|i| Some(i) == k).collect::<Vec<_>>();
        let set = |x_ready: Option<usize>, y_ready: Option<usize>| {
            let want = (540, Rational::new(30, 1));
            set_test_entry(&x.path, home.path().join("x-540.mp4"), want, ready(x_ready), false);
            set_test_entry(&y.path, home.path().join("y-540.mp4"), want, ready(y_ready), false);
        };
        let missing = [x.id, y.id];
        // keins fertig: Medienreihenfolge, keiner gibt ab
        set(None, None);
        app.update_focus(&ctx);
        assert_eq!(app.next_build(&missing, &ctx), Some(x.id));
        assert!(!other_media_first(&x.path) && !other_media_first(&y.path));
        // beim ersten ist das Playhead-Stück (4) fertig: das zweite ist dran, das erste gibt ab
        set(Some(4), None);
        assert_eq!(app.next_build(&missing, &ctx), Some(y.id));
        assert!(other_media_first(&x.path) && !other_media_first(&y.path));
        // beide fertig: normale Reihenfolge, keiner gibt ab
        set(Some(4), Some(4));
        assert_eq!(app.next_build(&missing, &ctx), Some(x.id));
        assert!(!other_media_first(&x.path));
        // zweites fehlt laut Prüfung bzw. ist gescheitert: kein Abgeben
        set(Some(4), None);
        app.presence.set_all([(y.path.clone(), false)].into(), 2);
        app.update_focus(&ctx);
        assert!(!other_media_first(&x.path), "fehlendes Medium");
        app.presence.set_all([(y.path.clone(), true)].into(), 2);
        app.proxy_failed.insert(y.id);
        app.update_focus(&ctx);
        assert!(!other_media_first(&x.path), "gescheitertes Medium");
        app.proxy_failed.clear();
        app.update_focus(&ctx);
        assert!(other_media_first(&x.path));
        // Bearbeitung bei stehendem Playhead: das zweite Video rückt weg – der Fokus folgt
        app.project.timeline.tracks[1].clips[0].start = Time::from_secs_f64(200.0);
        app.revision += 1;
        app.update_focus(&ctx);
        assert!(!other_media_first(&x.path), "Fokus kennt die Bearbeitung");
        assert_eq!(app.next_build(&missing, &ctx), Some(x.id));
        registry().remove(&x.path);
        registry().remove(&y.path);
    }

    /// Quelle weg oder Platte voll: nach 3 gescheiterten Versuchen in Folge endet der Bau, statt
    /// jedes Stück zweimal mit der ganzen Encoder-Kette zu probieren.
    #[test]
    fn build_gives_up_after_three_failures_in_a_row() {
        assert_eq!(fails_in_row(0, false), Some(1));
        assert_eq!(fails_in_row(1, false), Some(2));
        assert_eq!(fails_in_row(2, false), None);
        assert_eq!(fails_in_row(2, true), Some(0), "ein Erfolg setzt zurück");
        if !cl_media::ffmpeg_available() {
            return;
        }
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let mut m = media(1920, 1080, Rational::new(30, 1), "h264");
        m.path = d.path().join("weg.mp4");
        let out = d.path().join("weg-540.mp4");
        let want = (540, Rational::new(30, 1));
        set_test_entry(&m.path, out.clone(), want, vec![false; 10], false);
        let pt = Parting::new(100.0, want.1, PART_SECS).unwrap();
        let spec = proxy_spec(&m, 540, want.1);
        let r = build_parts(&m, &out, &spec, &pt, &|_| {}, &AtomicBool::new(false));
        assert!(r.is_err(), "{r:?}");
        let fails: u32 = registry().remove(&m.path).unwrap().fails.iter().map(|f| u32::from(*f)).sum();
        assert_eq!(fails, 2, "dritter Fehlschlag beendet den Bau");
    }

    /// Fertiger Proxy beim Strom: Datei aus dem Bestand, nicht von der Platte gesucht (dort fände
    /// sie die Suche hier gar nicht); nach dem Bau bleibt der Zwischenspeicher bis zur nächsten Suche.
    #[test]
    fn full_proxy_stream_comes_from_the_registry() {
        let (mut app, home, _g) = crate::saving::tests::test_app();
        let mut m = media(3840, 2160, Rational::new(30, 1), "h264");
        m.path = home.path().join("v.mp4");
        app.project.media.push(m.clone());
        let want = (app.settings.proxy_h, app.project.settings.fps);
        let out = home.path().join("woanders").join("v-540.mp4");
        set_test_entry(&m.path, out.clone(), want, vec![true; 10], true);
        assert_eq!(app.stream_source(&m, 540, Pick::Full), (out, Time::ZERO));
        app.proxies.key = Some(want);
        app.proxies.known.insert(m.id, None);
        app.proxy_finished(m.id, true);
        assert_eq!(app.proxies.cached(m.id, want), Some(None), "kein Nachsehen auf der Platte bis zur Suche");
        registry().remove(&m.path);
    }

    #[test]
    fn playhead_media_comes_first() {
        let ids: Vec<MediaId> = (0..4).map(|_| MediaId::new()).collect();
        let set = |v: &[usize]| v.iter().map(|i| ids[*i]).collect::<HashSet<_>>();
        assert_eq!(next_proxy(&ids, &set(&[]), &set(&[])), Some(ids[0]));
        assert_eq!(next_proxy(&ids, &set(&[]), &set(&[2, 3])), Some(ids[2]));
        assert_eq!(next_proxy(&ids, &set(&[3]), &set(&[1, 2])), Some(ids[3]));
        assert_eq!(next_proxy(&[], &set(&[1]), &set(&[])), None);
    }

    fn make_source(dir: &Path, s: &str) -> MediaRef {
        let src = dir.join("big.mp4");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", s, "-pix_fmt", "yuv420p", "-threads", "2"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        cl_media::probe(&src).unwrap()
    }

    /// Software-Weg (die CI hat keine GPU): richtige Größe, Bildrate, Schlüsselbild-Abstand,
    /// abspielbar.
    #[test]
    fn builds_small_proxy() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let m = make_source(d.path(), "testsrc2=s=2560x1440:r=60:d=2");
        assert!(wants_proxy(&m));
        let out = d.path().join("p.mp4");
        let spec = proxy_spec(&m, 540, Rational::new(30, 1));
        build_with(&m, &out, &spec, Encoder::Software, None, &|_| {}, &AtomicBool::new(false)).unwrap();
        let p = cl_media::probe(&out).unwrap();
        assert_eq!((p.width, p.height), (960, 540));
        assert!((p.fps.unwrap().as_f64() - 30.0).abs() < 0.01);
        assert!((p.duration.as_secs_f64() - 2.0).abs() < 0.2);
        // Schlüsselbilder: 0, 0,5, 1,0, 1,5 s
        let o = cl_media::ffprobe_command()
            .unwrap()
            .args(["-v", "error", "-select_streams", "v:0", "-skip_frame", "nokey", "-show_entries", "frame=pts_time", "-of", "csv=p=0"])
            .arg(&out)
            .output()
            .unwrap();
        let keys = String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(keys, 4, "{}", String::from_utf8_lossy(&o.stdout));
        // abspielbar: Strom liefert alle Bilder
        let s = cl_media::FrameStream::open(&out, Time::ZERO, 96, 54, Rational::new(30, 1)).unwrap();
        assert!(std::iter::from_fn(|| s.next_blocking()).count() >= 58);
        // Gesamtweg mit Encoder-Kette (Hardware, falls vorhanden, sonst Software); im Bestand ohne
        // Einteilung (Dauer unbekannt) zählt er danach als fertig
        let out2 = d.path().join("q.mp4");
        set_test_entry(&m.path, out2.clone(), (540, Rational::new(30, 1)), Vec::new(), false);
        registry().get_mut(&m.path).unwrap().pt = None;
        build(&m, &out2, &spec, &|_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(cl_media::probe(&out2).unwrap().height, 540);
        let e = registry().remove(&m.path).unwrap();
        assert!(e.complete && e.file.as_ref() == Some(&out2) && !e.needs(0.0));
        assert!(!parts::tmp_of(&out2).exists());
    }

    /// Abbrechen auch während der Pause: keine halbe Datei bleibt liegen.
    #[test]
    fn cancel_while_paused_leaves_nothing() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let m = make_source(d.path(), "testsrc2=s=1920x1080:r=60:d=4");
        let out = d.path().join("p.mp4");
        let spec = proxy_spec(&m, 540, Rational::new(30, 1));
        let cancel = AtomicBool::new(false);
        let calls = std::cell::Cell::new(0);
        let r = build_with(&m, &out, &spec, Encoder::Software, None, &|_| {
            // erster Fortschritt: Player aktiv (nächste Runde hält an), dann abbrechen
            calls.set(calls.get() + 1);
            match calls.get() {
                1 => note_activity(),
                3 => cancel.store(true, Ordering::Relaxed),
                _ => {}
            }
        }, &cancel);
        assert_eq!(r.unwrap_err(), "Abgebrochen");
        assert!(!out.exists() && !parts::tmp_of(&out).exists());
    }

    fn median(v: &mut [f64]) -> f64 {
        v.sort_by(|a, b| a.total_cmp(b));
        v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
    }

    /// Mess-Test (von Hand): Proxy in Stücken an einer echten Quelle (Vorgabe: 23-min-1080p60-Original).
    /// `cargo test --release -p cl-app --bin cutline -- --ignored --nocapture measure_parts`
    /// (`CUTLINE_PROXY_SRC=<Datei>`; `CUTLINE_PROXY_FULL=1`: Gesamtbau ohne und mit simulierter Wiedergabe)
    #[test]
    #[ignore]
    fn measure_parts() {
        let src = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Downloads/videoplayback.mp4");
        let src = std::env::var_os("CUTLINE_PROXY_SRC").map(PathBuf::from).unwrap_or(src);
        if !src.is_file() {
            return;
        }
        let m = cl_media::probe(&src).unwrap();
        let spec = proxy_spec(&m, 540, Rational::new(30, 1));
        let pt = Parting::new(m.duration.as_secs_f64(), out_fps(&m, &spec), PART_SECS).unwrap();
        let d = tempfile::tempdir().unwrap();
        let no = AtomicBool::new(false);
        let secs = |t: Instant| t.elapsed().as_secs_f64();
        println!("Quelle {} ({:.0} s, {} Stücke à {} Bilder)", src.display(), m.duration.as_secs_f64(), pt.count, pt.per);
        let picks = [0, pt.count / 3, pt.count / 2, pt.count * 3 / 4, pt.count - 2];
        let mut built: Option<PathBuf> = None;
        for enc in encoder_chain(available_encoders(), &[], "auto") {
            for hw in decode_attempts(enc, &m) {
                let dir = d.path().join(format!("{enc:?}-{hw:?}"));
                let t = Instant::now();
                // wie im Bau: Startzeit der Quelle zuerst
                let st = probe_start(&src);
                let first = build_part_with(&m, &dir, &pt, st, 0, &spec, enc, hw, &|_| {}, &no).map(|_| secs(t));
                let Ok(first) = first else {
                    println!("{enc:?} hw={hw:?}: Fehler {first:?}");
                    continue;
                };
                let mut more: Vec<f64> = picks[1..]
                    .iter()
                    .filter_map(|k| {
                        let t = Instant::now();
                        build_part_with(&m, &dir, &pt, st, *k, &spec, enc, hw, &|_| {}, &no).ok().map(|_| secs(t))
                    })
                    .collect();
                let best = more.iter().copied().fold(f64::MAX, f64::min);
                let bytes: u64 = picks.iter().filter_map(|k| std::fs::metadata(dir.join(parts::part_name(*k))).ok()).map(|x| x.len()).sum();
                println!(
                    "{enc:?} hw={hw:?}: erstes Stück {first:.2} s (mit ffprobe), weitere Median {:.2} s bestes {best:.2} s = {:.1} s je Minute Quelle, {:.1} MB/min",
                    median(&mut more),
                    median(&mut more.clone()) / PART_SECS * 60.0,
                    bytes as f64 / 1e6 / (picks.len() as f64 * PART_SECS / 60.0)
                );
                built.get_or_insert(dir);
            }
        }
        let Some(dir) = built else { return };
        // Einzelbild und Stromstart: Stück gegen Original an denselben Stellen
        let still = |path: &Path, at: f64| {
            let t = Instant::now();
            cl_media::decode_frame_opts(path, Time::from_secs_f64(at), 960, 540, None, false, None).unwrap();
            secs(t) * 1e3
        };
        let (mut sp, mut so, mut ap, mut ao) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (i, k) in picks.iter().enumerate() {
            for off in [1.3, 6.7] {
                let at = pt.start(*k) + off + i as f64 * 0.37;
                let (kk, rel) = parts::still_part(&pt, at).unwrap();
                sp.push(still(&dir.join(parts::part_name(kk)), rel));
                so.push(still(&src, at));
                let first = |p: &Path, at: f64, hw: bool| {
                    let t = Instant::now();
                    let s = cl_media::FrameStream::open_opts(p, Time::from_secs_f64(at), 960, 540, Rational::new(30, 1), None, if hw { cl_media::Hw::On } else { cl_media::Hw::Off }).unwrap();
                    s.next_blocking().unwrap();
                    secs(t) * 1e3
                };
                let list = parts::concat_list(&dir, *k, k + 1).unwrap();
                ap.push(first(&list, at - pt.start(*k), false));
                ao.push(first(&src, at, crate::preview::stream_hw(&m, &src)));
            }
        }
        let best = |v: &[f64]| v.iter().copied().fold(f64::MAX, f64::min);
        println!(
            "Einzelbild: Stück Median {:.0} ms bestes {:.0} ms, Original Median {:.0} ms bestes {:.0} ms",
            median(&mut sp.clone()),
            best(&sp),
            median(&mut so.clone()),
            best(&so)
        );
        println!(
            "Stromstart: Stücke Median {:.0} ms bestes {:.0} ms, Original Median {:.0} ms bestes {:.0} ms",
            median(&mut ap.clone()),
            best(&ap),
            median(&mut ao.clone()),
            best(&ao)
        );
        // Neustart mitten im Bau: Stück abbrechen (FFmpeg beendet), „.part“ eines harten Endes liegt herum
        let rdir = d.path().join("neustart.teile");
        for k in 0..3 {
            build_part_with(&m, &rdir, &pt, 0.0, k, &spec, Encoder::Software, None, &|_| {}, &no).unwrap();
        }
        let cancel = AtomicBool::new(false);
        let t = Instant::now();
        let r = build_part_with(&m, &rdir, &pt, 0.0, 3, &spec, Encoder::Software, None, &|_| {
            if t.elapsed() > Duration::from_millis(300) {
                cancel.store(true, Ordering::Relaxed)
            }
        }, &cancel);
        std::fs::write(rdir.join("0004.part.mp4"), b"halb").unwrap();
        let before: Vec<_> = (0..3).map(|k| std::fs::metadata(rdir.join(parts::part_name(k))).unwrap().modified().unwrap()).collect();
        parts::cleanup(d.path());
        let ready = parts::scan_ready(&rdir, pt.count);
        let next = parts::next_part(&pt, |i| ready[i as usize], &Focus::default());
        let after: Vec<_> = (0..3).map(|k| std::fs::metadata(rdir.join(parts::part_name(k))).unwrap().modified().unwrap()).collect();
        println!(
            "Neustart: Abbruch {:?}, fertig {:?}, .part übrig {}, weiter mit Stück {next:?}, fertige unverändert {}",
            r.err(),
            (0..pt.count).filter(|k| ready[*k as usize]).collect::<Vec<_>>(),
            std::fs::read_dir(&rdir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".part.mp4")),
            before == after
        );
        if std::env::var_os("CUTLINE_PROXY_FULL").is_none() {
            return;
        }
        // Gesamtbau: ohne Wiedergabe und mit durchgehend laufender (Strom aus dem Original in Echtzeit)
        for playing in [false, true] {
            let root = d.path().join(format!("voll-{playing}"));
            let out = root.join("p.mp4");
            let mut e = Entry {
                want: (540, Rational::new(30, 1)),
                out: out.clone(),
                spec,
                pt: Some(pt),
                dur: m.duration.as_secs_f64(),
                ready: vec![false; pt.count as usize],
                fails: vec![0; pt.count as usize],
                complete: false,
                file: None,
                dead: false,
                ranges: Arc::default(),
            };
            e.changed();
            registry().insert(m.path.clone(), e);
            let stop = Arc::new(AtomicBool::new(false));
            let player = playing.then(|| {
                let (src, stop, m) = (src.clone(), stop.clone(), m.clone());
                set_play_state(true, false);
                std::thread::spawn(move || {
                    let mut at = 0.0;
                    let (mut frames, mut late) = (0u64, 0u64);
                    while !stop.load(Ordering::Relaxed) {
                        let s = cl_media::FrameStream::open_opts(&src, Time::from_secs_f64(at), 960, 540, Rational::new(30, 1), None, crate::preview::stream_decode(&m, &src)).unwrap();
                        let t0 = Instant::now();
                        while let Some(_f) = s.next_blocking() {
                            frames += 1;
                            let due = frames as f64 / 30.0;
                            let now = t0.elapsed().as_secs_f64();
                            if now < due {
                                std::thread::sleep(Duration::from_secs_f64(due - now));
                            } else if now - due > 0.1 {
                                late += 1;
                            }
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                        at = 0.0;
                    }
                    (frames, late)
                })
            });
            let t = Instant::now();
            let r = build_parts(&m, &out, &spec, &pt, &|_| {}, &no);
            let total = secs(t);
            stop.store(true, Ordering::Relaxed);
            set_play_state(false, false);
            let pl = player.map(|h| h.join().unwrap());
            println!(
                "Gesamtbau {}: {total:.0} s ({:.1} s je Minute Quelle) {r:?}{}",
                if playing { "bei laufender Wiedergabe" } else { "ohne Wiedergabe" },
                total / (m.duration.as_secs_f64() / 60.0),
                pl.map(|(f, l)| format!(", Wiedergabe {f} Bilder, davon {l} über 100 ms zu spät")).unwrap_or_default()
            );
        }
    }

    /// Mess-Test (von Hand): Proxy für das 23-min-1080p60-Original mit jedem Encoder.
    /// `cargo test --release -p cl-app -- --ignored --nocapture measure_proxy_build`
    #[test]
    #[ignore]
    fn measure_proxy_build() {
        let src = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Downloads/videoplayback.mp4");
        let src = std::env::var_os("CUTLINE_PROXY_SRC").map(PathBuf::from).unwrap_or(src);
        if !src.is_file() {
            return;
        }
        let m = cl_media::probe(&src).unwrap();
        let d = tempfile::tempdir().unwrap();
        let spec = proxy_spec(&m, 540, Rational::new(30, 1));
        let dur = m.duration.as_secs_f64();
        for enc in encoder_chain(available_encoders(), &[], "auto") {
            for hw in decode_attempts(enc, &m) {
                let out = d.path().join(format!("{enc:?}-{hw:?}.mp4"));
                match build_with(&m, &out, &spec, enc, hw, &|_| {}, &AtomicBool::new(false)) {
                    Ok(r) => {
                        let mb = std::fs::metadata(&out).unwrap().len() as f64 / 1e6;
                        println!(
                            "{enc:?} hw={hw:?}: {:.1} s, {:.1}× Echtzeit, {:.1} MB/min ({:.0} MB)",
                            r.secs,
                            dur / r.secs,
                            mb / (dur / 60.0),
                            mb
                        );
                    }
                    Err(e) => println!("{enc:?} hw={hw:?}: Fehler {e}"),
                }
            }
        }
    }
}
