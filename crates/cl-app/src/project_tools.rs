//! Projekt-Werkzeuge: automatische Sicherungen mit Wiederherstellen, fehlende Medien neu
//! verknüpfen (Ordner durchsuchen) und Projekt archivieren (Projekt + Medien + Schriften).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use cl_core::{Document, EditOp, Project};
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::paths;
use crate::theme::{self, icon};

/// Abstand der automatischen Sicherungen
/// So viele Sicherungen je Projekt bleiben erhalten
#[cfg(test)]
const BACKUP_KEEP: usize = 30;

fn backup_dir(project: &Path) -> PathBuf {
    let stem = project
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Projekt".into());
    paths::data_dir().join("Backups").join(stem)
}

/// Sicherungen eines Projekts, neueste zuerst.
pub fn list_backups(project: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut v: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(backup_dir(project))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "cutline"))
        .filter_map(|p| p.metadata().and_then(|m| m.modified()).ok().map(|t| (p, t)))
        .collect();
    v.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    v
}

/// Sicherung schreiben und alte entfernen.
pub fn write_backup(project: &Path, json: &str, keep: usize) -> std::io::Result<PathBuf> {
    let dir = backup_dir(project);
    std::fs::create_dir_all(&dir)?;
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let p = dir.join(format!("{stamp}.cutline"));
    paths::write_atomic(&p, json)?;
    for (old, _) in list_backups(project).into_iter().skip(keep.max(1)) {
        let _ = std::fs::remove_file(old);
    }
    Ok(p)
}

/// Dateien rekursiv nach Namen suchen (für fehlende Medien).
pub fn find_by_name(root: &Path, names: &[String], depth: usize) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        return found;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                found.extend(find_by_name(&p, names, depth - 1));
            }
        } else if let Some(n) = p.file_name().map(|s| s.to_string_lossy().to_lowercase()) {
            if let Some(want) = names.iter().find(|w| w.to_lowercase() == n) {
                found.push((want.clone(), p));
            }
        }
    }
    found
}

/// Projekt archivieren: `<ziel>/<Name>/` mit `Medien/`, `Schriften/` und angepasstem Projekt.
pub fn archive(p: &Project, fonts: &[PathBuf], dest_root: &Path) -> Result<PathBuf, String> {
    let safe: String = p
        .name
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let dir = (1..)
        .map(|i| {
            if i == 1 {
                dest_root.join(&safe)
            } else {
                dest_root.join(format!("{safe} ({i})"))
            }
        })
        .find(|d| !d.exists())
        .unwrap();
    let media_dir = dir.join("Medien");
    std::fs::create_dir_all(&media_dir).map_err(|e| e.to_string())?;
    let mut out = p.clone();
    let mut used = std::collections::HashSet::new();
    for m in &mut out.media {
        if !m.path.is_file() {
            continue;
        }
        let name = m
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        // gleiche Dateinamen aus verschiedenen Ordnern nicht überschreiben
        let mut target = media_dir.join(&name);
        let mut i = 2;
        while used.contains(&target) {
            target = media_dir.join(format!("{i}_{name}"));
            i += 1;
        }
        used.insert(target.clone());
        std::fs::copy(&m.path, &target).map_err(|e| format!("{}: {e}", m.path.display()))?;
        m.path = target;
    }
    if !fonts.is_empty() {
        let fd = dir.join("Schriften");
        std::fs::create_dir_all(&fd).map_err(|e| e.to_string())?;
        out.project_fonts.clear();
        for f in fonts.iter().filter(|f| f.is_file()) {
            let t = fd.join(f.file_name().unwrap_or_default());
            std::fs::copy(f, &t).map_err(|e| e.to_string())?;
            out.project_fonts.push(t);
        }
    }
    paths::write_atomic(&dir.join(format!("{safe}.cutline")), out.to_json())
        .map_err(|e| e.to_string())?;
    Ok(dir)
}

impl App {
    /// Jeden Frame: Sicherung fällig? Dialoge zeichnen.
    pub fn project_tools(&mut self, ctx: &egui::Context) {
        // Automatische Sicherung (nur bei gespeicherten Projekten mit Änderungen seit der letzten)
        if let Some(path) = self.project_path.clone() {
            let every = Duration::from_secs(self.settings.backup_minutes.max(1) as u64 * 60);
            let due = self.last_backup.is_none_or(|t| t.elapsed() >= every);
            if due && self.revision != self.backup_revision {
                let _ = write_backup(&path, &self.project.to_json(), self.settings.backup_keep);
                self.backup_revision = self.revision;
                self.last_backup = Some(Instant::now());
            }
        }
        if let Some(rx) = &self.archive {
            if let Ok(r) = rx.try_recv() {
                self.archive = None;
                match r {
                    Ok(d) => {
                        self.toast(format!("Archiv erstellt: {}", d.display()));
                        crate::home::reveal(&d);
                    }
                    Err(e) => self.toast_err(format!("Archivieren fehlgeschlagen: {e}")),
                }
            }
        }
        self.backups_window(ctx);
        self.missing_window(ctx);
        self.props_window(ctx);
        self.exports_window(ctx);
    }

    pub fn start_archive(&mut self, ctx: &egui::Context) {
        let Some(dest) = crate::dialog()
            .set_title(crate::tr("Zielordner für das Archiv"))
            .pick_folder()
        else {
            return;
        };
        self.commit_live();
        let p = self.project.clone();
        let fonts: Vec<PathBuf> = p.project_fonts.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(archive(&p, &fonts, &dest));
            c.request_repaint();
        });
        self.archive = Some(rx);
        self.toast("Projekt wird archiviert …");
    }

    fn backups_window(&mut self, ctx: &egui::Context) {
        if !self.backups_open {
            return;
        }
        let mut open = true;
        let mut restore: Option<PathBuf> = None;
        egui::Window::new(crate::tr("Sicherungen")).id(egui::Id::new("backups")).open(&mut open).collapsible(false).default_width(380.0).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
            ui.label(RichText::new(crate::tr("Alle 5 Minuten bei Änderungen, die letzten 30 Stände")).small().color(theme::TEXT_DIM()));
            ui.add_space(6.0);
            let Some(path) = self.project_path.clone() else {
                ui.label(crate::tr("Das Projekt ist noch nicht gespeichert."));
                return;
            };
            if ui.add(theme::secondary(format!("{}  Jetzt sichern", icon::FLOPPY_DISK))).clicked() {
                match write_backup(&path, &self.project.to_json(), self.settings.backup_keep) {
                    Ok(_) => {
                        self.backup_revision = self.revision;
                        self.last_backup = Some(Instant::now());
                    }
                    Err(e) => self.toast_err(e.to_string()),
                }
            }
            ui.add_space(4.0);
            let list = list_backups(&path);
            if list.is_empty() {
                theme::empty_state(ui, icon::CLOCK_COUNTER_CLOCKWISE, "Noch keine Sicherungen", "");
            }
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (p, t) in list {
                    ui.horizontal(|ui| {
                        let local = chrono_like(t);
                        ui.label(RichText::new(local).monospace());
                        ui.label(RichText::new(crate::tr(crate::home::ago(t))).small().color(theme::TEXT_FAINT()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(theme::ghost("Wiederherstellen")).clicked() {
                                restore = Some(p.clone());
                            }
                        });
                    });
                }
            });
        });
        if !open {
            self.backups_open = false;
        }
        if let Some(p) = restore {
            match std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|s| Project::from_json(&s).map_err(|e| e.to_string()))
            {
                Ok(proj) => {
                    // aktuellen Stand vorher sichern, dann ersetzen (Pfad bleibt)
                    if let Some(path) = &self.project_path {
                        let _ = write_backup(path, &self.project.to_json(), self.settings.backup_keep);
                    }
                    *self.doc.lock().unwrap() = Document::new(proj);
                    self.revision = u64::MAX;
                    self.sync_project();
                    self.backups_open = false;
                    self.toast("Sicherung wiederhergestellt");
                }
                Err(e) => self.toast_err(e),
            }
        }
    }

    fn missing_window(&mut self, ctx: &egui::Context) {
        if !self.missing_open {
            return;
        }
        let missing: Vec<(cl_core::MediaId, PathBuf)> = self
            .project
            .media
            .iter()
            .filter(|m| !m.path.is_file() && !m.path.to_string_lossy().contains("://"))
            .map(|m| (m.id, m.path.clone()))
            .collect();
        let mut open = true;
        let mut search = false;
        let mut pick: Option<cl_core::MediaId> = None;
        egui::Window::new(crate::tr("Medien neu verknüpfen")).id(egui::Id::new("relink")).open(&mut open).collapsible(false).default_width(460.0).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
            if missing.is_empty() {
                ui.label(RichText::new(crate::tr(format!("{}  Alle Medien sind vorhanden", icon::CHECK_CIRCLE))).color(theme::TEXT_DIM()));
                return;
            }
            ui.label(crate::tr(format!("{} Mediendateien wurden nicht gefunden (verschoben oder umbenannt).", missing.len())));
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for (id, p) in &missing {
                    // Knopf zuerst (rechts), der Pfad bekommt den Rest und wird gekürzt
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(theme::ghost("Datei wählen …")).clicked() {
                            pick = Some(*id);
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.label(RichText::new(icon::WARNING).color(theme::DANGER));
                            ui.add(egui::Label::new(RichText::new(p.display().to_string()).small()).truncate()).on_hover_text(p.display().to_string());
                        });
                    });
                }
            });
            ui.add_space(6.0);
            if ui.add(theme::primary(format!("{}  Ordner durchsuchen …", icon::MAGNIFYING_GLASS))).on_hover_text(crate::tr("Sucht alle fehlenden Dateien nach Namen in einem Ordner und seinen Unterordnern")).clicked() {
                search = true;
            }
        });
        if !open {
            self.missing_open = false;
        }
        if let Some(id) = pick {
            if let Some(f) = crate::dialog().pick_file() {
                self.apply(EditOp::RelinkMedia { media: id, path: f });
                self.invalidate_preview();
            }
        }
        if search {
            if let Some(dir) = crate::dialog().pick_folder() {
                let names: Vec<String> = missing
                    .iter()
                    .filter_map(|(_, p)| p.file_name().map(|s| s.to_string_lossy().into_owned()))
                    .collect();
                let found = find_by_name(&dir, &names, 6);
                let ops: Vec<EditOp> = missing
                    .iter()
                    .filter_map(|(id, p)| {
                        let n = p.file_name()?.to_string_lossy().into_owned();
                        found
                            .iter()
                            .find(|(w, _)| *w == n)
                            .map(|(_, f)| EditOp::RelinkMedia {
                                media: *id,
                                path: f.clone(),
                            })
                    })
                    .collect();
                let n = ops.len();
                if n > 0 {
                    self.apply_batch(ops);
                    self.invalidate_preview();
                }
                self.toast(format!("{n} von {} Dateien gefunden", missing.len()));
            }
        }
    }
}

/// Datum/Uhrzeit in Ortszeit ohne zusätzliche Abhängigkeit (Windows: über die Zeitzone des Systems
/// nicht exakt; wir zeigen UTC-Offset-frei die Systemzeit über `ago` daneben).
fn chrono_like(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let off = local_offset_secs();
    let s = secs + off;
    let days = s.div_euclid(86400);
    let rem = s.rem_euclid(86400);
    // Tage → Datum (proleptischer Gregorianischer Kalender)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{d:02}.{m:02}.{y} {:02}:{:02}", rem / 3600, rem / 60 % 60)
}

/// Abstand der Ortszeit zu UTC in Sekunden (aus der Umgebung; Standard Mitteleuropa).
fn local_offset_secs() -> i64 {
    static OFF: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        // PowerShell/`date` wären teuer; wir lesen die Abweichung einmal über die Windows-API-freie
        // Variante: Vergleich der Datei-Zeitstempel ist nicht möglich, daher TZ-Variable oder +1 h.
        if let Ok(tz) = std::env::var("CUTLINE_TZ_OFFSET") {
            return tz.parse::<i64>().unwrap_or(0) * 60;
        }
        if cfg!(windows) {
            local_offset_windows()
        } else {
            0
        }
    })
}

#[cfg(windows)]
fn local_offset_windows() -> i64 {
    #[repr(C)]
    struct SystemTimeW([u16; 8]);
    #[repr(C)]
    struct TzInfo {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: SystemTimeW,
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: SystemTimeW,
        daylight_bias: i32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTimeZoneInformation(tz: *mut TzInfo) -> u32;
    }
    let mut tz: TzInfo = unsafe { std::mem::zeroed() };
    let r = unsafe { GetTimeZoneInformation(&mut tz) };
    // 2 = Sommerzeit aktiv
    let bias = tz.bias
        + if r == 2 {
            tz.daylight_bias
        } else {
            tz.standard_bias
        };
    -(bias as i64) * 60
}

#[cfg(not(windows))]
fn local_offset_windows() -> i64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backups_rotate_and_archive_copies() {
        let home = tempfile::tempdir().unwrap();
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CUTLINE_HOME", home.path());
        let proj = home.path().join("Film.cutline");
        for i in 0..(BACKUP_KEEP + 3) {
            let p = write_backup(&proj, &format!("{{\"n\":{i}}}"), BACKUP_KEEP).unwrap();
            // eindeutige Zeitstempel erzwingen
            let _ = std::fs::rename(&p, p.with_file_name(format!("{}.cutline", 1000 + i)));
        }
        let _ = write_backup(&proj, "{}", BACKUP_KEEP);
        assert!(list_backups(&proj).len() <= BACKUP_KEEP + 1);
        // Archiv
        let media = home.path().join("src");
        std::fs::create_dir_all(media.join("a")).unwrap();
        std::fs::create_dir_all(media.join("b")).unwrap();
        std::fs::write(media.join("a/clip.mp4"), b"1").unwrap();
        std::fs::write(media.join("b/clip.mp4"), b"2").unwrap();
        let mut p = Project::new("Film");
        for (i, sub) in ["a", "b"].iter().enumerate() {
            let mut m: cl_core::MediaRef = serde_json::from_value(serde_json::json!({
                "id": cl_core::MediaId::new(), "path": media.join(sub).join("clip.mp4"), "kind": "Video", "duration": 1000000,
                "width": 2, "height": 2, "fps": null, "has_audio": false, "video_codec": null, "audio_codec": null
            })).unwrap();
            m.width = 2 + i as u32;
            p.media.push(m);
        }
        let out = archive(&p, &[], &home.path().join("out")).unwrap();
        let back = Project::from_json(&std::fs::read_to_string(out.join("Film.cutline")).unwrap())
            .unwrap();
        assert!(back
            .media
            .iter()
            .all(|m| m.path.starts_with(out.join("Medien")) && m.path.is_file()));
        assert_ne!(
            back.media[0].path, back.media[1].path,
            "gleiche Namen bleiben getrennt"
        );
        // Suche nach Namen
        let f = find_by_name(&media, &["CLIP.mp4".into()], 3);
        assert_eq!(f.len(), 2);
    }
}
