//! Merkt sich, wohin Projekte exportiert wurden (`exports.json` im Datenordner), damit sich
//! fertige Dateien später je Projekt wiederfinden und mit dem Projektordner umziehen lassen.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Export {
    /// Projektname zum Zeitpunkt des Exports
    pub project: String,
    pub path: PathBuf,
    /// Sekunden seit 1970
    pub at: u64,
}

fn file() -> PathBuf {
    paths::data_dir().join("exports.json")
}

fn load() -> Vec<Export> {
    std::fs::read_to_string(file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(v: &[Export]) {
    let _ = paths::write_atomic(&file(), serde_json::to_string_pretty(v).unwrap_or_default());
}

/// Fertigen Export eintragen (derselbe Pfad ersetzt den alten Eintrag)
pub fn record(project: &str, path: &Path) {
    let mut v = load();
    v.retain(|e| e.path != path);
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    v.push(Export { project: project.to_string(), path: path.to_path_buf(), at });
    save(&v);
}

/// Noch vorhandene Exporte eines Projekts, neueste zuerst
pub fn for_project(project: &str) -> Vec<Export> {
    let mut v: Vec<Export> = load().into_iter().filter(|e| e.project == project && e.path.is_file()).collect();
    v.sort_by(|a, b| b.at.cmp(&a.at));
    v
}

/// Projekt umbenannt: Einträge folgen dem neuen Namen
pub fn rename_project(old: &str, new: &str) {
    let mut v = load();
    if v.iter().any(|e| e.project == old) {
        v.iter_mut().filter(|e| e.project == old).for_each(|e| e.project = new.to_string());
        save(&v);
    }
}

/// Exporte, die direkt im alten Projektordner liegen, in den neuen verschieben
pub fn move_dir(old: &Path, new: &Path) -> usize {
    let mut v = load();
    let mut n = 0;
    for e in v.iter_mut().filter(|e| e.path.parent() == Some(old) && e.path.is_file()) {
        let to = new.join(e.path.file_name().unwrap_or_default());
        if !to.exists() && paths::move_path(&e.path, &to).is_ok() {
            // Untertiteldatei daneben mitnehmen
            let srt = e.path.with_extension("srt");
            if srt.is_file() {
                let _ = paths::move_path(&srt, &to.with_extension("srt"));
            }
            e.path = to;
            n += 1;
        }
    }
    if n > 0 {
        save(&v);
    }
    n
}

impl crate::app::App {
    /// Liste der fertigen Exporte eines Projekts, jeweils mit „Zur Datei“
    pub fn exports_window(&mut self, ctx: &eframe::egui::Context) {
        use crate::theme::{self, icon};
        use eframe::egui::{self, RichText};
        let Some(name) = self.exports_for.clone() else { return };
        let list = for_project(&name);
        let mut open = true;
        egui::Window::new(crate::tr(format!("{}  Exporte", icon::EXPORT)))
            .id(egui::Id::new("exports"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(RichText::new(&name).strong().size(13.0));
                ui.add_space(6.0);
                if list.is_empty() {
                    ui.label(RichText::new(crate::tr("Noch nichts exportiert")).color(theme::TEXT_DIM()));
                }
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for e in &list {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon::FILE_VIDEO).color(theme::ACCENT()));
                            ui.vertical(|ui| {
                                ui.label(e.path.file_name().unwrap_or_default().to_string_lossy());
                                let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(e.at);
                                ui.label(
                                    RichText::new(format!("{} · {}", crate::home::ago(when), crate::props::file_size(&e.path)))
                                        .small()
                                        .color(theme::TEXT_DIM()),
                                )
                                .on_hover_text(e.path.display().to_string());
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add(theme::secondary(format!("{}  Zur Datei", icon::FOLDER_OPEN))).clicked() {
                                    crate::home::reveal(&e.path);
                                }
                            });
                        });
                        ui.separator();
                    }
                });
            });
        if !open {
            self.exports_for = None;
        }
    }
}

#[test]
fn exports_are_listed_per_project_and_move_with_the_folder() {
    let _g = paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", home.path());
    let (a, b) = (home.path().join("alt"), home.path().join("neu"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("Film.mp4"), b"x").unwrap();
    std::fs::write(a.join("Film.srt"), b"x").unwrap();
    std::fs::write(a.join("fremd.mp4"), b"x").unwrap();
    record("Film", &a.join("Film.mp4"));
    record("Film", &a.join("Film.mp4"));
    record("Film", &a.join("weg.mp4"));
    assert_eq!(for_project("Film").len(), 1);
    assert!(for_project("Anderes").is_empty());
    assert_eq!(move_dir(&a, &b), 1);
    assert!(b.join("Film.mp4").is_file() && b.join("Film.srt").is_file());
    assert!(a.join("fremd.mp4").is_file(), "nur bekannte Exporte ziehen um");
    assert_eq!(for_project("Film")[0].path, b.join("Film.mp4"));
    rename_project("Film", "Kino");
    assert_eq!(for_project("Kino").len(), 1);
    std::env::remove_var("CUTLINE_HOME");
}
