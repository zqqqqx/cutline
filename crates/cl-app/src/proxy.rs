//! Proxy-Dateien: kleine, schnell decodierbare Kopien großer Videos (über 1080p, über 30 fps
//! oder HEVC/AV1/ProRes) für eine flüssige Vorschau. Der Export nutzt immer die Originale.
//! Ablage: `<Daten>/cache/proxies/<Kennung>.mp4` (Kennung aus Pfad, Größe und Änderungszeit).

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

use cl_core::{MediaKind, MediaRef};
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::paths;
use crate::tasks::TaskDone;
use crate::theme::{self, icon};
use crate::widgets;

pub fn dir() -> PathBuf {
    paths::data_dir().join("cache").join("proxies")
}

/// Proxy-Pfad eines Mediums (ändert sich, wenn die Datei sich ändert).
pub fn proxy_path(m: &MediaRef) -> PathBuf {
    let meta = std::fs::metadata(&m.path).ok();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    m.path.hash(&mut h);
    meta.as_ref().map(|m| m.len()).hash(&mut h);
    meta.and_then(|m| m.modified().ok()).hash(&mut h);
    dir().join(format!("{:016x}.mp4", h.finish()))
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

/// Höhe der Proxys in Zeilen
pub const PROXY_H: u32 = 540;

/// Proxy erzeugen: 540p, H.264 sehr schnell, Schlüsselbild alle 10 Bilder (schnelles Springen).
pub fn build(
    src: &Path,
    out: &Path,
    dur: f64,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<(), String> {
    std::fs::create_dir_all(out.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    let tmp = out.with_extension("part.mp4");
    let mut child = cl_media::ffmpeg_command()
        .map_err(|e| e.to_string())?
        .args([
            "-v",
            "error",
            "-y",
            "-nostdin",
            "-progress",
            "pipe:1",
            "-nostats",
            "-i",
        ])
        .arg(src)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-vf",
            "scale=-2:540:flags=bilinear,fps=30",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "26",
            "-g",
            "10",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&tmp)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    use std::io::BufRead;
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = std::fs::remove_file(&tmp);
                return Err("Abgebrochen".into());
            }
            if let Some(v) = line.strip_prefix("out_time_us=") {
                if let Ok(us) = v.trim().parse::<f64>() {
                    progress((us / 1e6 / dur.max(0.1)).clamp(0.0, 1.0) as f32);
                }
            }
        }
    }
    let st = child.wait().map_err(|e| e.to_string())?;
    if !st.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err("Proxy konnte nicht erstellt werden".into());
    }
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())
}

impl App {
    /// Datei für die Vorschau: beim Abspielen der Proxy (flüssig), im Stand das Original, wenn
    /// die 540 Zeilen des Proxys für die benötigte Höhe `need_h` nicht reichen (scharf).
    pub fn preview_path(&self, m: &MediaRef, need_h: u32) -> PathBuf {
        if self.settings.use_proxies
            && m.kind == MediaKind::Video
            && (self.playing || need_h <= PROXY_H * 6 / 5)
        {
            let p = proxy_path(m);
            if p.is_file() {
                return p;
            }
        }
        m.path.clone()
    }

    /// Fehlende Proxys im Hintergrund erzeugen (nacheinander, je Medium eine Aufgabe).
    pub fn ensure_proxies(&mut self, ctx: &egui::Context) {
        if !self.settings.use_proxies || self.tasks.running("proxy").is_some() {
            return;
        }
        let next = self
            .project
            .media
            .iter()
            .find(|m| {
                wants_proxy(m)
                    && m.path.is_file()
                    && !proxy_path(m).is_file()
                    && !self.proxy_failed.contains(&m.id)
            })
            .cloned();
        if let Some(m) = next {
            let out = proxy_path(&m);
            let dur = m.duration.as_secs_f64();
            let name = m
                .path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let id = m.id;
            self.tasks.spawn(
                ctx,
                "proxy",
                format!("Proxy wird erstellt: {name}"),
                move |p, cancel| match build(&m.path, &out, dur, p, cancel) {
                    Ok(()) => TaskDone::Proxy {
                        media: id,
                        ok: true,
                    },
                    Err(_) => TaskDone::Proxy {
                        media: id,
                        ok: false,
                    },
                },
            );
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
        ui.label(RichText::new(crate::tr("Für Videos über 1080p, über 30 fps oder in HEVC/AV1/ProRes. Der Export nutzt immer die Originale.")).small().color(theme::TEXT_DIM));
        let (n, bytes) = std::fs::read_dir(dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .fold((0u64, 0u64), |(n, b), m| (n + 1, b + m.len()));
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(crate::tr(format!(
                    "{n} Proxys · {:.0} MB",
                    bytes as f64 / 1e6
                )))
                .small()
                .color(theme::TEXT_FAINT),
            );
            if n > 0
                && ui
                    .add(theme::ghost(format!("{}  Proxys löschen", icon::TRASH)))
                    .clicked()
            {
                let _ = std::fs::remove_dir_all(dir());
                self.invalidate_preview();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_small_proxy() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("big.mp4");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=2560x1440:r=60:d=1",
                "-pix_fmt",
                "yuv420p"
            ])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        let m = cl_media::probe(&src).unwrap();
        assert!(wants_proxy(&m));
        let out = d.path().join("p.mp4");
        build(&src, &out, 1.0, &|_| {}, &AtomicBool::new(false)).unwrap();
        let p = cl_media::probe(&out).unwrap();
        assert_eq!(p.height, 540);
        assert!((p.duration.as_secs_f64() - 1.0).abs() < 0.2);
    }
}
