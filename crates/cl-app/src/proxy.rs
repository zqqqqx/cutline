//! Proxy-Dateien: kleine, schnell decodierbare Kopien großer Videos (über 1080p, über 30 fps
//! oder HEVC/AV1/ProRes) für eine flüssige Vorschau. Der Export nutzt immer die Originale.
//! Ablage: `<Daten>/cache/proxies/<Kennung>-<Höhe>[-<Bildrate>fps].mp4` (Kennung aus Pfad, Größe
//! und Änderungszeit; ohne Bildrate = Bildrate der Quelle bis 60, so hießen auch ältere Proxys).
//!
//! Der Bau stört den Player nicht: Priorität unter normal, angehalten beim Abspielen und kurz
//! nach jedem Einzelbild-Auftrag (Klick, Scrubben), Hardware-Encoder zuerst.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cl_core::{ClipContent, MediaId, MediaKind, MediaRef, Rational, Time};
use cl_export::Encoder;
use eframe::egui::{self, RichText};

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
    let s = proxy_spec(m, height, project_fps);
    dir().join(file_name(media_hash(m), s.height, s.fps))
}

/// Vorhandener, brauchbarer Proxy mit seiner Höhe.
pub fn existing_proxy(m: &MediaRef, height: u32, project_fps: Rational) -> Option<(PathBuf, u32)> {
    let d = dir();
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

/// Reihenfolge der Encoder: Hardware (NVENC → QSV → AMF), soweit vorhanden und in dieser
/// Sitzung nicht gescheitert, zuletzt immer Software.
pub fn encoder_chain(available: &[Encoder], broken: &[Encoder]) -> Vec<Encoder> {
    [Encoder::Nvenc, Encoder::Qsv, Encoder::Amf, Encoder::Software]
        .into_iter()
        .filter(|e| *e == Encoder::Software || (available.contains(e) && !broken.contains(e)))
        .collect()
}

/// Encoder, die in dieser Sitzung beim Proxy-Bau gescheitert sind
fn broken_encoders() -> std::sync::MutexGuard<'static, Vec<Encoder>> {
    static B: Mutex<Vec<Encoder>> = Mutex::new(Vec::new());
    B.lock().unwrap_or_else(|e| e.into_inner())
}

/// Hardware-Encoder dieses Rechners (Erkennung des Exports, einmal je Sitzung).
fn available_encoders() -> &'static [Encoder] {
    static A: OnceLock<Vec<Encoder>> = OnceLock::new();
    A.get_or_init(cl_export::available_encoders)
}

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

/// FFmpeg-Argumente nach dem Eingang: Größe, Bildrate, Schlüsselbilder, Encoder.
/// Ziel ≤ 25 MB je Minute bei 1080p-Quellen (gemessen 14–19 MB/min).
pub fn encode_args(enc: Encoder, spec: &ProxySpec) -> Vec<String> {
    let mut vf = String::new();
    if let Some(f) = spec.fps {
        vf.push_str(&format!("fps={}/{},", f.num, f.den));
    }
    vf.push_str(&format!("scale=-2:{}:flags=bicubic,format=yuv420p", spec.height));
    let mut a: Vec<String> = ["-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &vf].map(String::from).into();
    if spec.fps.is_none() {
        a.extend(["-fpsmax", "60"].map(String::from));
    }
    let codec: &[&str] = match enc {
        Encoder::Nvenc => &["-c:v", "h264_nvenc", "-preset", "p4", "-rc", "constqp", "-qp", "26"],
        Encoder::Qsv => &["-c:v", "h264_qsv", "-preset", "veryfast", "-global_quality", "26"],
        Encoder::Amf => &["-c:v", "h264_amf", "-quality", "speed", "-rc", "cqp", "-qp_i", "26", "-qp_p", "26"],
        Encoder::Software => &["-c:v", "libx264", "-preset", "veryfast", "-crf", "23"],
    };
    a.extend(codec.iter().map(|s| s.to_string()));
    a.extend(["-g".to_string(), spec.gop.to_string()]);
    a
}

// ------------------------------------------------------------------ Rücksicht auf den Player

static PLAYING: AtomicBool = AtomicBool::new(false);
static LAST_ACTIVITY: Mutex<Option<Instant>> = Mutex::new(None);

/// Der Player hat gerade ein Einzelbild angefordert oder bekommen (Klick, Scrubben).
pub fn note_activity() {
    *LAST_ACTIVITY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
}

/// Wiedergabe läuft (vom UI-Thread je Bild gesetzt).
pub fn set_playing(on: bool) {
    PLAYING.store(on, Ordering::Relaxed);
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

/// Soll der Proxy-Bau (jetzt) angehalten sein?
pub fn want_pause(paused: bool, playing: bool, idle: Duration) -> bool {
    playing || idle < if paused { RESUME_AFTER } else { PAUSE_WITHIN }
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
    let chain = encoder_chain(available_encoders(), &broken_encoders());
    let mut last = String::new();
    for enc in chain {
        for hw in decode_attempts(enc, m) {
            match build_with(m, out, spec, enc, hw, progress, cancel) {
                Ok(r) => return Ok(r),
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
    let tmp = out.with_extension("part.mp4");
    let mut cmd = cl_media::ffmpeg_command().map_err(|e| e.to_string())?;
    cl_media::lower_priority(&mut cmd);
    cmd.args(["-v", "error", "-y", "-nostdin", "-progress", "pipe:1", "-nostats"]);
    if let Some(h) = hw {
        cmd.args(["-hwaccel", h]);
    }
    let mut child = cmd
        .arg("-i")
        .arg(&m.path)
        .args(encode_args(enc, spec))
        .arg(&tmp)
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // ungelesen würde der volle Puffer FFmpeg anhalten
        .spawn()
        .map_err(|e| e.to_string())?;
    // Fortschritt in eigenem Thread lesen: angehalten schreibt FFmpeg nichts, die Steuerung
    // unten muss trotzdem weiterlaufen (Fortsetzen, Abbrechen).
    let done_us = Arc::new(AtomicU64::new(0));
    let reader = child.stdout.take().map(|out| {
        let done_us = done_us.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<u64>().ok()) {
                    done_us.store(us, Ordering::Relaxed);
                }
            }
        })
    });
    let dur = m.duration.as_secs_f64().max(0.1);
    let start = Instant::now();
    let (mut paused, mut paused_total, mut paused_since) = (false, Duration::ZERO, None::<Instant>);
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            // warten, bis FFmpeg die Datei losgelassen hat (Windows), sonst bleibt `.part.mp4`
            let _ = child.wait();
            let _ = std::fs::remove_file(&tmp);
            return Err("Abgebrochen".into());
        }
        let want = want_pause(paused, PLAYING.load(Ordering::Relaxed), idle());
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
                let _ = std::fs::remove_file(&tmp);
                return Err(e.to_string());
            }
        }
    };
    if let Some(r) = reader {
        let _ = r.join();
    }
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err("Proxy konnte nicht erstellt werden".into());
    }
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    let paused = (paused_total + paused_since.map_or(Duration::ZERO, |t| t.elapsed())).as_secs_f64();
    let report = BuildReport {
        secs: (start.elapsed().as_secs_f64() - paused).max(1e-3),
        paused,
    };
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

/// Reihenfolge der Proxy-Aufträge: Medien am Playhead zuerst, dann im sichtbaren Bereich der
/// Zeitleiste, dann der Rest (in Medienreihenfolge).
pub fn next_proxy(candidates: &[MediaId], at_playhead: &HashSet<MediaId>, visible: &HashSet<MediaId>) -> Option<MediaId> {
    candidates
        .iter()
        .min_by_key(|id| if at_playhead.contains(id) { 0 } else if visible.contains(id) { 1 } else { 2 })
        .copied()
}

impl App {
    /// Datei für die Vorschau: beim Abspielen der Proxy (flüssig), im Stand das Original, wenn
    /// die Zeilen des Proxys für die benötigte Höhe `need_h` nicht reichen (scharf).
    pub fn preview_path(&self, m: &MediaRef, need_h: u32) -> PathBuf {
        if self.settings.use_proxies && m.kind == MediaKind::Video {
            if let Some((p, h)) = existing_proxy(m, self.settings.proxy_h, self.project.settings.fps) {
                if self.playing || need_h <= h * 6 / 5 {
                    return p;
                }
            }
        }
        m.path.clone()
    }

    /// Fehlende Proxys im Hintergrund erzeugen (nacheinander, je Medium eine Aufgabe).
    pub fn ensure_proxies(&mut self, ctx: &egui::Context) {
        set_playing(self.playing);
        if !self.settings.use_proxies || self.tasks.running("proxy").is_some() {
            // nach Ende einer Aufgabe sofort weitersuchen
            self.proxy_scan = None;
            return;
        }
        // Dateisystem höchstens alle 2 s prüfen (oder wenn sich die Medienliste ändert)
        let n = self.project.media.len();
        if self
            .proxy_scan
            .is_some_and(|(t, len)| len == n && t.elapsed() < std::time::Duration::from_secs(2))
        {
            return;
        }
        self.proxy_scan = Some((std::time::Instant::now(), n));
        let fps = self.project.settings.fps;
        let missing: Vec<MediaId> = self
            .project
            .media
            .iter()
            .filter(|m| {
                wants_proxy(m)
                    && !self.proxy_failed.contains(&m.id)
                    && m.path.is_file()
                    && existing_proxy(m, self.settings.proxy_h, fps).is_none()
            })
            .map(|m| m.id)
            .collect();
        if missing.is_empty() {
            return;
        }
        let (mut at, mut vis) = (HashSet::new(), HashSet::new());
        let v0 = Time::from_secs_f64(self.timeline.scroll);
        let v1 = Time::from_secs_f64(self.timeline.scroll + (ctx.screen_rect().width() / self.timeline.pps.max(1e-3)) as f64);
        for c in self.project.timeline.tracks.iter().flat_map(|t| &t.clips) {
            if let ClipContent::Media { media, .. } = c.content {
                if c.start <= self.playhead && self.playhead < c.end() {
                    at.insert(media);
                }
                if c.start < v1 && v0 < c.end() {
                    vis.insert(media);
                }
            }
        }
        let Some(m) = next_proxy(&missing, &at, &vis)
            .and_then(|id| self.project.media.iter().find(|m| m.id == id))
            .cloned()
        else {
            return;
        };
        let spec = proxy_spec(&m, self.settings.proxy_h, fps);
        let out = proxy_path(&m, self.settings.proxy_h, fps);
        let name = m
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let id = m.id;
        self.tasks.spawn(
            ctx,
            "proxy",
            crate::tr(format!("Proxy wird erstellt: {name}")),
            move |p, cancel| TaskDone::Proxy {
                media: id,
                ok: crate::applog::outcome("proxy", &name, std::time::Instant::now(), build(&m, &out, &spec, p, cancel)).is_ok(),
            },
        );
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
                self.settings.save();
                self.proxy_failed.clear();
                self.invalidate_preview();
            }
        });
        // Ordner höchstens alle 2 s zählen
        let (n, bytes) = match self.proxy_stats {
            Some((t, n, b)) if t.elapsed() < std::time::Duration::from_secs(2) => (n, b),
            _ => {
                let (n, b) = std::fs::read_dir(dir())
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| e.metadata().ok())
                    .fold((0u64, 0u64), |(n, b), m| (n + 1, b + m.len()));
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
                let _ = std::fs::remove_dir_all(dir());
                self.proxy_stats = None;
                self.invalidate_preview();
            }
        });
    }
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
        assert_eq!(encoder_chain(&[Software, Amf, Nvenc, Qsv], &[]), [Nvenc, Qsv, Amf, Software]);
        assert_eq!(encoder_chain(&[Software, Nvenc, Qsv], &[Nvenc]), [Qsv, Software]);
        assert_eq!(encoder_chain(&[], &[]), [Software]);
        assert_eq!(encoder_chain(&[Software], &[Software]), [Software], "Software bleibt immer");
        let m = media(3840, 2160, Rational::new(60, 1), "hevc");
        assert_eq!(hw_decode(Nvenc, &m), Some("cuda"));
        assert_eq!(hw_decode(Software, &m), None);
        assert_eq!(hw_decode(Nvenc, &media(1920, 1080, Rational::new(60, 1), "h264")), None);
    }

    #[test]
    fn encode_args_set_size_rate_and_gop() {
        let spec = ProxySpec { height: 540, fps: Some(Rational::new(30, 1)), gop: 15 };
        for enc in [Encoder::Nvenc, Encoder::Qsv, Encoder::Amf, Encoder::Software] {
            let a = encode_args(enc, &spec).join(" ");
            assert!(a.contains("fps=30/1,scale=-2:540:"), "{a}");
            assert!(a.ends_with("-g 15") && !a.contains("fpsmax"), "{a}");
        }
        let a = encode_args(Encoder::Software, &ProxySpec { height: 720, fps: None, gop: 30 }).join(" ");
        assert!(a.contains("-vf scale=-2:720:") && a.contains("-fpsmax 60") && a.contains("libx264"), "{a}");
    }

    #[test]
    fn pause_while_playing_or_clicking_with_hysteresis() {
        let ms = Duration::from_millis;
        // läuft: Wiedergabe oder frischer Klick → anhalten
        assert!(want_pause(false, true, Duration::MAX));
        assert!(want_pause(false, false, ms(100)));
        assert!(!want_pause(false, false, ms(400)));
        // angehalten: erst nach 0,8 s Ruhe fortsetzen
        assert!(want_pause(true, false, ms(400)));
        assert!(!want_pause(true, false, ms(900)));
        assert!(want_pause(true, true, ms(5000)));
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
        // Gesamtweg mit Encoder-Kette (Hardware, falls vorhanden, sonst Software)
        let out2 = d.path().join("q.mp4");
        build(&m, &out2, &spec, &|_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(cl_media::probe(&out2).unwrap().height, 540);
        assert!(!out2.with_extension("part.mp4").exists());
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
        assert!(!out.exists() && !out.with_extension("part.mp4").exists());
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
        for enc in encoder_chain(available_encoders(), &[]) {
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
