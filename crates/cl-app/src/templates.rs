//! Projektvorlagen: das offene Projekt als Vorlage speichern (Intro, Bauchbinden, Outro,
//! Endcard, Format), auf der Startseite neue Projekte daraus anlegen, Vorlagen umbenennen
//! und entfernen. Ablage: `<Daten>/Templates/<Name>.cutline`.

use std::path::PathBuf;

use cl_core::{Document, Project};
use eframe::egui::{self, RichText};

use crate::app::{App, Screen};
use crate::paths;
use crate::theme::{self, icon};

fn dir() -> PathBuf {
    paths::data_dir().join("Templates")
}

pub fn list() -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "cutline"))
        .map(|p| (p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), p))
        .collect();
    v.sort_by_key(|(n, _)| n.to_lowercase());
    v
}

fn safe(name: &str) -> String {
    name.trim().chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect()
}

pub enum TplDialog {
    Save { name: String },
    Rename { path: PathBuf, name: String },
}

impl App {
    /// Startseite: Liste der Vorlagen unter „Neues Projekt“.
    pub fn home_templates(&mut self, ui: &mut egui::Ui) {
        let list = list();
        if list.is_empty() {
            return;
        }
        ui.add_space(8.0);
        ui.label(RichText::new(crate::tr("Aus Vorlage")).small().color(theme::TEXT_FAINT));
        let mut open: Option<PathBuf> = None;
        let mut rename: Option<(PathBuf, String)> = None;
        let mut remove: Option<PathBuf> = None;
        for (name, p) in list {
            let r = ui.add_sized([ui.available_width(), 26.0], theme::ghost(format!("{}  {name}", icon::COPY_SIMPLE)));
            if r.clicked() {
                open = Some(p.clone());
            }
            r.context_menu(|ui| {
                if ui.button(crate::tr(format!("{}  Umbenennen", icon::PENCIL_SIMPLE))).clicked() {
                    rename = Some((p.clone(), name.clone()));
                    ui.close_menu();
                }
                if ui.button(crate::tr(format!("{}  Entfernen", icon::TRASH))).clicked() {
                    remove = Some(p.clone());
                    ui.close_menu();
                }
            });
        }
        if let Some(p) = open {
            self.new_from_template(&p);
        }
        if let Some((path, name)) = rename {
            self.tpl_dialog = Some(TplDialog::Rename { path, name });
        }
        if let Some(p) = remove {
            let trash = paths::data_dir().join("Papierkorb");
            let _ = std::fs::create_dir_all(&trash);
            if std::fs::rename(&p, trash.join(p.file_name().unwrap_or_default())).is_ok() {
                self.toast("Vorlage in den Papierkorb verschoben");
            }
        }
    }

    pub fn new_from_template(&mut self, p: &std::path::Path) {
        match std::fs::read_to_string(p).map_err(|e| e.to_string()).and_then(|s| Project::from_json(&s).map_err(|e| e.to_string())) {
            Ok(mut proj) => {
                self.new_project();
                proj.name = crate::home::next_project_name();
                *self.doc.lock().unwrap() = Document::new(proj);
                self.revision = u64::MAX;
                self.sync_project();
                self.screen = Screen::Editor;
                self.toast("Neues Projekt aus Vorlage");
            }
            Err(e) => self.toast_err(e),
        }
    }

    /// Dialog „Als Vorlage speichern“ / „Vorlage umbenennen“.
    pub fn template_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.tpl_dialog else { return };
        let (mut ok, mut close) = (false, false);
        let title = match d {
            TplDialog::Save { .. } => "Als Vorlage speichern",
            TplDialog::Rename { .. } => "Vorlage umbenennen",
        };
        egui::Window::new(crate::tr(title)).id(egui::Id::new("tpl-dialog")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -60.0)).show(ctx, |ui| {
            let name = match d {
                TplDialog::Save { name } | TplDialog::Rename { name, .. } => name,
            };
            let r = ui.add(egui::TextEdit::singleline(name).hint_text(crate::tr("z. B. Folge mit Intro und Endcard")).desired_width(280.0));
            r.request_focus();
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ok = true;
            }
            if matches!(d, TplDialog::Save { .. }) {
                ui.label(RichText::new(crate::tr("Gespeichert werden Format, Spuren, Clips, Texte, Effekte und Marker. Neue Projekte aus der Vorlage starten mit diesem Stand.")).small().color(theme::TEXT_DIM));
            }
            ui.horizontal(|ui| {
                if ui.add(theme::primary("OK")).clicked() {
                    ok = true;
                }
                if ui.add(theme::secondary("Abbrechen")).clicked() {
                    close = true;
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if ok {
            match self.tpl_dialog.take() {
                Some(TplDialog::Save { name }) if !safe(&name).is_empty() => {
                    let _ = std::fs::create_dir_all(dir());
                    let mut p = self.project.clone();
                    p.name = name.trim().to_string();
                    match std::fs::write(dir().join(format!("{}.cutline", safe(&name))), p.to_json()) {
                        Ok(_) => self.toast(format!("Vorlage „{}“ gespeichert", name.trim())),
                        Err(e) => self.toast_err(e.to_string()),
                    }
                }
                Some(TplDialog::Rename { path, name }) if !safe(&name).is_empty() => {
                    let to = dir().join(format!("{}.cutline", safe(&name)));
                    if let Err(e) = std::fs::rename(&path, &to) {
                        self.toast_err(e.to_string());
                    }
                }
                _ => self.toast_err("Bitte einen Namen eingeben"),
            }
        } else if close {
            self.tpl_dialog = None;
        }
    }
}
