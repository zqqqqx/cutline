//! Projekte anderer Schnittprogramme importieren: Dialog auf der Startseite (lokale
//! CapCut-Entwürfe und Datei-Auswahl), Einstellungsseite „Import“ zum Ein-/Ausschalten.
//! Der Import läuft im Hintergrund, danach wird das Projekt als neue `.cutline`-Datei geöffnet.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use cl_core::{Document, Project};
use eframe::egui::{self, vec2, RichText};

use crate::app::{App, Screen};
use crate::theme::{self, icon};
use crate::widgets;

type ImportResult = cl_import::Result<(Project, cl_import::Report)>;

#[derive(Default)]
pub struct ImportState {
    pub open: bool,
    since: u64,
    drafts: Option<Vec<cl_import::capcut::Draft>>,
    running: Option<(String, Receiver<ImportResult>)>,
    /// Bericht des letzten Imports (übersprungene Teile, fehlende Dateien)
    pub report: Option<(String, cl_import::Report)>,
    /// Schriften aus dem Import, die noch in die Schriftbibliothek müssen
    fonts: Vec<PathBuf>,
}

impl App {
    pub fn import_enabled(&self) -> bool {
        self.settings.import_capcut || self.settings.import_fcpxml
    }

    /// Import starten (Entwurfsordner oder Datei).
    pub fn start_import(&mut self, path: PathBuf, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let c = ctx.clone();
        let label = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        std::thread::spawn(move || {
            let _ = tx.send(cl_import::import(&path));
            c.request_repaint();
        });
        self.import.running = Some((label, rx));
    }

    /// Fertigen Import übernehmen (jeden Frame).
    pub fn poll_import(&mut self) {
        let Some((_, rx)) = &self.import.running else {
            return;
        };
        let res = match rx.try_recv() {
            Ok(res) => res,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            // Import-Thread abgestürzt: sonst dreht sich die Anzeige ewig
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.import.running = None;
                self.toast_err("Import fehlgeschlagen (interner Fehler)");
                return;
            }
        };
        self.import.running = None;
        match res {
            Ok((proj, rep)) => {
                self.import.open = false;
                self.open_imported(proj, rep);
            }
            Err(e) => self.toast_err(e),
        }
    }

    fn open_imported(&mut self, proj: Project, rep: cl_import::Report) {
        let name = proj.name.clone();
        if !self.new_project() {
            return;
        }
        *self.doc.lock().unwrap() = Document::new(proj);
        self.revision = u64::MAX;
        self.sync_project();
        let p = self.default_project_path();
        if self.write_project(&p).is_ok() {
            self.project_path = Some(p);
        }
        self.screen = Screen::Editor;
        self.import.fonts = rep.fonts.clone();
        self.toast(format!(
            "„{name}“ importiert: {} Clips, {} Texte",
            rep.clips, rep.texts
        ));
        if !rep.skipped.is_empty() || !rep.missing.is_empty() {
            self.import.report = Some((name, rep));
        }
    }

    /// Dialog „Projekt importieren“ und Import-Bericht.
    pub fn import_dialogs(&mut self, ctx: &egui::Context) {
        self.poll_import();
        // Schriften der importierten Texte übernehmen (still, schon vorhandene werden übersprungen)
        let fonts = std::mem::take(&mut self.import.fonts);
        if !fonts.is_empty() {
            for f in &fonts {
                let _ = self.fonts.lock().unwrap().import_file(f);
            }
            self.register_fonts(ctx);
        }
        if let Some((name, rep)) = &self.import.report {
            let mut close = false;
            let m = egui::Modal::new(egui::Id::new("import-report")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(
                    RichText::new(crate::tr(format!("„{name}“ importiert")))
                        .strong()
                        .size(14.0),
                );
                ui.add_space(6.0);
                if !rep.skipped.is_empty() {
                    ui.label(crate::tr(
                        "Nicht übertragen (gibt es nur im Ursprungsprogramm):",
                    ));
                    for (w, n) in &rep.skipped {
                        ui.label(
                            RichText::new(format!("·  {}  ×{n}", crate::tr(w)))
                                .color(theme::TEXT_DIM()),
                        );
                    }
                }
                if !rep.missing.is_empty() {
                    ui.add_space(6.0);
                    ui.label(crate::tr(format!(
                        "{} Mediendateien wurden nicht gefunden:",
                        rep.missing.len()
                    )));
                    for p in rep.missing.iter().take(6) {
                        ui.label(
                            RichText::new(p.display().to_string())
                                .small()
                                .color(theme::TEXT_FAINT()),
                        );
                    }
                }
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(theme::primary("OK")).clicked() {
                        close = true;
                    }
                });
            });
            if close || m.should_close() {
                self.import.report = None;
            }
        }
        if !self.import.open {
            self.import.since = 0;
            return;
        }
        let pass = ctx.cumulative_pass_nr();
        if self.import.since == 0 {
            self.import.since = pass;
        }
        let fresh = pass <= self.import.since + 1;
        if self.settings.import_capcut && self.import.drafts.is_none() {
            self.import.drafts = Some(cl_import::capcut::list_drafts());
        }
        let mut close = false;
        let mut pick: Option<PathBuf> = None;
        let busy = self.import.running.is_some();
        let m = egui::Modal::new(egui::Id::new("import")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::tr("Projekt importieren")).strong().size(14.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, icon::X, "Schließen (Esc)", true, false).clicked() {
                        close = true;
                    }
                });
            });
            ui.label(RichText::new(crate::tr("Programmeigene Effekte und Vorlagen werden nicht übernommen.")).small().color(theme::TEXT_DIM()));
            ui.add_space(8.0);
            if let Some((label, _)) = &self.import.running {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(crate::tr(format!("„{label}“ wird importiert …")));
                });
                ui.add_space(6.0);
            }
            if self.settings.import_capcut {
                theme::section(ui, "CapCut-Entwürfe auf diesem Rechner");
                let drafts = self.import.drafts.clone().unwrap_or_default();
                if drafts.is_empty() {
                    ui.label(RichText::new(crate::tr("Keine Entwürfe gefunden")).color(theme::TEXT_DIM()));
                } else {
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for d in &drafts {
                            ui.horizontal(|ui| {
                                ui.set_min_height(28.0);
                                ui.label(RichText::new(icon::FILM_STRIP).color(theme::TEXT_DIM()));
                                ui.add(egui::Label::new(&d.name).truncate());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add_enabled(!busy, theme::secondary("Importieren")).clicked() {
                                        pick = Some(d.dir.clone());
                                    }
                                    if let Some(t) = d.modified {
                                        let s = crate::home::ago(t);
                                        ui.label(RichText::new(crate::tr(s)).small().color(theme::TEXT_FAINT()));
                                    }
                                });
                            });
                        }
                    });
                }
                ui.add_space(4.0);
            }
            ui.horizontal(|ui| {
                let mut exts: Vec<&str> = Vec::new();
                if self.settings.import_capcut {
                    exts.push("json");
                }
                if self.settings.import_fcpxml {
                    exts.extend(["fcpxml", "xml"]);
                }
                if ui.add_enabled(!busy, theme::secondary(format!("{}  Datei wählen …", icon::FOLDER_OPEN))).on_hover_text(crate::tr("draft_content.json (CapCut) oder FCPXML (Final Cut Pro, DaVinci Resolve)")).clicked() {
                    if let Some(p) = crate::dialog().add_filter(crate::tr("Projekte"), &exts).pick_file() {
                        pick = Some(p);
                    }
                }
            });
        });
        if (m.should_close() && !fresh && !busy) || close {
            self.import.open = false;
            self.import.drafts = None;
        }
        if let Some(p) = pick {
            self.start_import(p, ctx);
        }
    }

    /// Einstellungsseite „Import“.
    pub fn import_settings(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let mut changed =
            widgets::toggle_row(ui, "CapCut-Entwürfe", &mut self.settings.import_capcut);
        changed |= widgets::toggle_row(
            ui,
            "FCPXML (Final Cut Pro, DaVinci Resolve)",
            &mut self.settings.import_fcpxml,
        );
        if changed {
            self.settings.save();
            self.import.drafts = None;
        }
        if self.settings.import_capcut {
            let roots = cl_import::capcut::draft_roots();
            ui.add_space(4.0);
            ui.label(RichText::new(crate::tr(if roots.is_empty() { "Kein CapCut-Entwurfsordner gefunden; Entwürfe lassen sich trotzdem als Datei wählen." } else { "Entwurfsordner gefunden" })).small().color(theme::TEXT_FAINT()));
        }
        ui.add_space(8.0);
        ui.add_enabled_ui(self.import_enabled(), |ui| {
            if ui
                .add(theme::secondary(format!(
                    "{}  Projekt importieren …",
                    icon::DOWNLOAD_SIMPLE
                )))
                .clicked()
            {
                self.settings_open = false;
                self.import.open = true;
            }
        });
        let _ = vec2(0.0, 0.0);
    }
}
