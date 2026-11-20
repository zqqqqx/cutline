//! Remembers where projects were exported to (`exports.json` in the data folder), so that
//! finished files can later be found per project and moved along with the project folder.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Export {
    /// Project name at the time of export
    pub project: String,
    pub path: PathBuf,
    /// Seconds since 1970
    pub at: u64,
    /// Project file; tells same-named projects apart (older entries have none)
    #[serde(default)]
    pub project_path: Option<PathBuf>,
}

/// Counts finished exports so that an open exports window reloads
static RECORDED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn file() -> PathBuf {
    paths::data_dir().join("exports.json")
}

fn load() -> Vec<Export> {
    paths::read_json_or_quarantine(&file())
}

fn save(v: &[Export]) {
    let _ = paths::write_atomic(&file(), serde_json::to_string_pretty(v).unwrap_or_default());
}

/// Record a finished export (the same path replaces the old entry)
pub fn record(project: &str, project_path: Option<&Path>, path: &Path) {
    let mut v = load();
    v.retain(|e| e.path != path);
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    v.push(Export { project: project.to_string(), path: path.to_path_buf(), at, project_path: project_path.map(Path::to_path_buf) });
    save(&v);
    RECORDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Still existing exports of a project, newest first
pub fn for_project(project: &str, project_path: Option<&Path>) -> Vec<Export> {
    let mut v: Vec<Export> = load()
        .into_iter()
        .filter(|e| match (&e.project_path, project_path) {
            (Some(a), Some(b)) => a == b,
            _ => e.project == project,
        })
        .filter(|e| e.path.is_file())
        .collect();
    v.sort_by(|a, b| b.at.cmp(&a.at));
    v
}

fn belongs(e: &Export, name: &str, path: &Path) -> bool {
    e.project_path.as_ref().map_or(e.project == name, |p| p == path)
}

/// Project renamed (and file possibly from `from` to `to`): entries follow name and file
pub fn rename_project(from: &Path, to: &Path, old: &str, new: &str) {
    let mut v = load();
    if v.iter().any(|e| belongs(e, old, from)) {
        for e in v.iter_mut().filter(|e| belongs(e, old, from)) {
            e.project = new.to_string();
            e.project_path = e.project_path.as_ref().map(|_| to.to_path_buf());
        }
        save(&v);
    }
}

/// Project files moved: entries point to the new files
pub fn move_projects(moved: &[(PathBuf, PathBuf)]) {
    let mut v = load();
    let mut n = 0;
    for e in &mut v {
        if let Some((_, to)) = moved.iter().find(|(a, _)| e.project_path.as_ref() == Some(a)) {
            e.project_path = Some(to.clone());
            n += 1;
        }
    }
    if n > 0 {
        save(&v);
    }
}

/// Open exports window: list loaded once, sizes already formatted
pub struct ExportsList {
    pub name: String,
    path: Option<PathBuf>,
    items: Vec<(Export, String)>,
    loaded: u64,
}

impl ExportsList {
    pub fn new(name: &str, path: Option<&Path>) -> Self {
        let items = for_project(name, path)
            .into_iter()
            .map(|e| {
                let size = crate::props::file_size(&e.path);
                (e, size)
            })
            .collect();
        Self { name: name.to_string(), path: path.map(Path::to_path_buf), items, loaded: RECORDED.load(std::sync::atomic::Ordering::Relaxed) }
    }
}

/// Move exports that sit directly in the old project folder into the new one
pub fn move_dir(old: &Path, new: &Path) -> usize {
    let mut v = load();
    let mut n = 0;
    for e in v.iter_mut().filter(|e| e.path.parent() == Some(old) && e.path.is_file()) {
        let to = new.join(e.path.file_name().unwrap_or_default());
        if !to.exists() && paths::move_path(&e.path, &to).is_ok() {
            // Take subtitle and chapter files next to it along
            for ext in ["srt", "vtt", "chapters.txt"] {
                let side = e.path.with_extension(ext);
                if side.is_file() {
                    let _ = paths::move_path(&side, &to.with_extension(ext));
                }
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
    /// List of a project's finished exports, each with „Zur Datei“
    pub fn exports_window(&mut self, ctx: &eframe::egui::Context) {
        use crate::theme::{self, icon};
        use eframe::egui::{self, RichText};
        let Some(l) = &mut self.exports_for else { return };
        if l.loaded != RECORDED.load(std::sync::atomic::Ordering::Relaxed) {
            *l = ExportsList::new(&l.name, l.path.as_deref());
        }
        let (name, list) = (&l.name, &l.items);
        // Queue (all projects): running export with progress, the waiting ones below
        let file = |f: &Path| f.file_name().unwrap_or_default().to_string_lossy().to_string();
        let current = self.export.current();
        let waiting: Vec<(String, String)> = self
            .export
            .waiting()
            .map(|(f, s, parts)| {
                let mut d = crate::tr(s.format.label());
                if !s.format.audio_only() {
                    let (w, h) = s.output_size();
                    d = format!("{d} · {w}×{h}");
                }
                if parts > 0 {
                    d = format!("{d} · {}", crate::tr(format!("{parts} Teile")));
                }
                (file(f), format!("{d} · {}", crate::tr("Wartet")))
            })
            .collect();
        let (mut cancel, mut remove) = (false, None);
        let mut open = true;
        let title = format!("{}  Exporte", icon::EXPORT);
        crate::dialog::window(ctx, crate::tr(title.as_str()), "exports")
            .default_width(420.0)
            .show(ctx, |ui| {
                if crate::dialog::title(ui, &title) {
                    open = false;
                }
                ui.label(RichText::new(name).strong().size(theme::FS_TEXT));
                ui.add_space(6.0);
                if let Some((f, prog)) = &current {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::FILE_VIDEO).color(theme::TEXT_DIM()));
                        ui.vertical(|ui| {
                            ui.label(f);
                            let (r, _) = ui.allocate_exact_size(egui::vec2(220.0, 3.0), egui::Sense::hover());
                            crate::widgets::progress_line(ui.painter(), egui::Id::new("exports-current"), r, *prog);
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(format!("{:.0} %", prog * 100.0)).color(theme::TEXT_DIM()));
                            cancel = theme::icon_button(ui, icon::X, "Abbrechen", true, false).clicked();
                        });
                    });
                    ui.separator();
                }
                for (i, (f, d)) in waiting.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::FILE_VIDEO).color(theme::TEXT_FAINT()));
                        ui.vertical(|ui| {
                            ui.label(f);
                            ui.label(RichText::new(d).small().color(theme::TEXT_DIM()));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(ui, icon::X, "Entfernen", true, false).clicked() {
                                remove = Some(Some(i));
                            }
                        });
                    });
                    ui.separator();
                }
                if waiting.len() > 1 && ui.add(theme::secondary("Alle wartenden entfernen")).clicked() {
                    remove = Some(None);
                }
                if list.is_empty() && current.is_none() && waiting.is_empty() {
                    theme::empty_state(ui, icon::EXPORT, "Noch nichts exportiert", "");
                }
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for (e, size) in list {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon::FILE_VIDEO).color(theme::TEXT_DIM()));
                            ui.vertical(|ui| {
                                ui.label(e.path.file_name().unwrap_or_default().to_string_lossy());
                                let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(e.at);
                                ui.label(
                                    RichText::new(format!("{} · {}", crate::home::ago(when), size))
                                        .small()
                                        .color(theme::TEXT_DIM()),
                                )
                                .on_hover_text(crate::paths::shown(&e.path));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add(theme::secondary(format!("{}  Im Explorer zeigen", icon::FOLDER_OPEN))).clicked() {
                                    crate::home::reveal(&e.path);
                                }
                            });
                        });
                        ui.separator();
                    }
                });
            });
        if cancel {
            self.export.cancel_current();
        }
        if let Some(i) = remove {
            self.export.remove_waiting(i);
        }
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
    let (pa, pb) = (a.join("Film.cutline"), b.join("Film.cutline"));
    record("Film", Some(&pa), &a.join("Film.mp4"));
    record("Film", Some(&pa), &a.join("Film.mp4"));
    record("Film", Some(&pa), &a.join("weg.mp4"));
    assert_eq!(for_project("Film", Some(&pa)).len(), 1);
    assert!(for_project("Anderes", None).is_empty());
    // a same-named project elsewhere does not see the exports, an old entry without a path counts by name
    assert!(for_project("Film", Some(&home.path().join("x/Film.cutline"))).is_empty());
    assert_eq!(for_project("Film", None).len(), 1);
    assert_eq!(move_dir(&a, &b), 1);
    assert!(b.join("Film.mp4").is_file() && b.join("Film.srt").is_file());
    assert!(a.join("fremd.mp4").is_file(), "nur bekannte Exporte ziehen um");
    move_projects(&[(pa.clone(), pb.clone())]);
    assert_eq!(for_project("Film", Some(&pb))[0].path, b.join("Film.mp4"));
    let pk = b.join("Kino.cutline");
    rename_project(&pb, &pk, "Film", "Kino");
    assert_eq!(for_project("Kino", Some(&pk)).len(), 1);
    std::env::remove_var("CUTLINE_HOME");
}
