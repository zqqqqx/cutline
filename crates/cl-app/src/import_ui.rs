//! Projekte anderer Schnittprogramme importieren: Dialog auf der Startseite (lokale
//! CapCut-Entwürfe und Datei-Auswahl), Einstellungsseite „Import“ zum Ein-/Ausschalten.
//!
//! Der Import läuft vollständig im Hintergrund, in Schritten (Entwurf lesen → Medien finden →
//! Medien prüfen → Schriften → Zeitleiste → Projekt anlegen) mit Fortschritt und Abbrechen. Der
//! Dialog führt durch die Schritte und zeigt danach eine Zusammenfassung; mehrere Entwürfe
//! laufen nacheinander. Abgebrochen oder gescheitert bleibt nichts zurück.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cl_core::{MediaKind, Project};
use cl_import::{Progress, Report, Step};
use eframe::egui::{self, vec2, RichText};

use crate::app::App;
use crate::theme::{self, icon};
use crate::widgets;

/// Ein fertig importiertes Projekt (schon als `.cutline` gespeichert)
pub struct Imported {
    pub name: String,
    pub path: PathBuf,
    pub project: Project,
    pub report: Report,
}

struct Job {
    label: String,
    progress: Arc<Progress>,
    rx: Receiver<cl_import::Result<Imported>>,
}

#[derive(Default)]
pub struct ImportState {
    pub open: bool,
    since: u64,
    drafts: Option<Vec<cl_import::capcut::Draft>>,
    job: Option<Job>,
    /// wartende Entwürfe; Zähler für „Projekt 2 von 5“
    queue: VecDeque<PathBuf>,
    batch_total: usize,
    /// fertige Importe dieser Runde (Zusammenfassung)
    results: Vec<Imported>,
    /// Fehler dieser Runde: (Entwurf, Meldung)
    errors: Vec<(String, String)>,
}

/// Schritte des Imports (Anzeige)
const STEPS: [(Step, &str); 6] = [
    (Step::Read, "Entwurf lesen"),
    (Step::Find, "Medien finden"),
    (Step::Probe, "Medien prüfen"),
    (Step::Fonts, "Schriften"),
    (Step::Timeline, "Zeitleiste aufbauen"),
    (Step::Save, "Projekt anlegen"),
];

/// Freier Dateiname im Projektordner (`Name.cutline`, `Name (2).cutline` …)
pub fn free_project_path(dir: &Path, name: &str) -> PathBuf {
    let name = crate::paths::safe_name(name);
    let mut p = dir.join(format!("{}.cutline", name.trim()));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{} ({n}).cutline", name.trim()));
        n += 1;
    }
    p
}

/// Ganzer Import im Hintergrund: umwandeln, Bilder vorlesen, Projekt speichern, Schriften
/// übernehmen. Abbruch → nichts bleibt liegen (auch keine schon geschriebene Projektdatei).
pub fn run_import(
    src: &Path,
    dir: &Path,
    fonts: &Mutex<cl_fonts::FontLibrary>,
    progress: &Progress,
) -> cl_import::Result<Imported> {
    let t = Instant::now();
    let (project, report) = cl_import::import_with(src, progress)?;
    progress.check()?;
    progress.set(Step::Save, 0, 2, &project.name);
    // Animationen der Bilder (Zeit des Vorschaubilds) hier lesen, nicht beim ersten Bild
    for m in project.media.iter().filter(|m| m.kind == MediaKind::Image) {
        let _ = cl_media::animation(&m.path);
    }
    progress.check()?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = free_project_path(dir, &project.name);
    crate::paths::write_atomic(&path, project.to_json()).map_err(|e| e.to_string())?;
    if progress.cancelled() {
        let _ = std::fs::remove_file(&path);
        return Err(cl_import::CANCELLED.into());
    }
    progress.tick("");
    // Schriften der Texte übernehmen (schon vorhandene werden übersprungen)
    for f in &report.fonts {
        let _ = fonts.lock().unwrap_or_else(|e| e.into_inner()).import_file(f);
    }
    progress.tick("");
    if crate::app::perf() {
        eprintln!(
            "perf: Import „{}“ {:.1} s, {} Medien, {} Clips, {} Texte",
            project.name,
            t.elapsed().as_secs_f64(),
            project.media.len(),
            report.clips,
            report.texts
        );
    }
    Ok(Imported { name: project.name.clone(), path, project, report })
}

impl App {
    pub fn import_enabled(&self) -> bool {
        self.settings.import_capcut || self.settings.import_fcpxml
    }

    /// Läuft gerade ein Import (oder wartet einer)?
    pub fn import_busy(&self) -> bool {
        self.import.job.is_some() || !self.import.queue.is_empty()
    }

    /// Import starten (Entwurfsordner oder Datei); läuft schon einer, wird er eingereiht.
    pub fn start_import(&mut self, path: PathBuf, ctx: &egui::Context) {
        if !self.import_busy() {
            self.import.batch_total = 0;
            self.import.results.clear();
            self.import.errors.clear();
        }
        self.import.batch_total += 1;
        self.import.queue.push_back(path);
        self.next_import(ctx);
    }

    fn next_import(&mut self, ctx: &egui::Context) {
        if self.import.job.is_some() {
            return;
        }
        let Some(path) = self.import.queue.pop_front() else { return };
        let c = ctx.clone();
        let progress = Arc::new(Progress::new(move || c.request_repaint()));
        let (tx, rx) = channel();
        let label = self
            .import
            .drafts
            .iter()
            .flatten()
            .find(|d| d.dir == path)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        let (pr, fonts, dir, c) = (progress.clone(), self.fonts.clone(), crate::paths::projects_dir(), ctx.clone());
        std::thread::Builder::new()
            .name("cl-import".into())
            .spawn(move || {
                let _ = tx.send(run_import(&path, &dir, &fonts, &pr));
                c.request_repaint();
            })
            .expect("Import-Thread");
        self.import.job = Some(Job { label, progress, rx });
    }

    /// Alle Importe dieser Runde abbrechen (wirkt sofort; der Thread räumt hinter sich auf).
    fn cancel_import(&mut self) {
        if let Some(j) = self.import.job.take() {
            j.progress.cancel();
        }
        self.import.queue.clear();
    }

    /// Fertigen Import übernehmen (jedes Bild).
    pub fn poll_import(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.import.job else { return };
        let res = match job.rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            // Import-Thread abgestürzt: sonst dreht sich die Anzeige ewig
            Err(TryRecvError::Disconnected) => Err("Import fehlgeschlagen (interner Fehler)".into()),
        };
        let Some(job) = self.import.job.take() else { return };
        match res {
            Ok(imp) => {
                if !imp.report.fonts.is_empty() {
                    self.register_fonts(ctx);
                }
                self.home.refresh();
                if !self.import.open {
                    self.toast(format!("„{}“ importiert: {} Clips, {} Texte", imp.name, imp.report.clips, imp.report.texts));
                }
                self.import.results.push(imp);
            }
            Err(e) if e == cl_import::CANCELLED => {}
            Err(e) => {
                if !self.import.open {
                    self.toast_err(e.clone());
                }
                self.import.errors.push((job.label, e));
            }
        }
        self.next_import(ctx);
    }

    /// Importiertes Projekt öffnen (aus dem Speicher, ohne es neu zu lesen).
    pub fn open_imported(&mut self, i: usize) {
        if i >= self.import.results.len() {
            return;
        }
        let imp = self.import.results.remove(i);
        // fehlende Dateien (Platzhalter): „Medien neu verknüpfen“ geht gleich auf
        let present: HashMap<PathBuf, bool> = imp.project.media.iter().map(|m| (m.path.clone(), !imp.report.missing.contains(&m.path))).collect();
        let fonts = imp
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter_map(|c| match &c.content {
                cl_core::ClipContent::Text(s) => s.font_family.clone(),
                _ => None,
            })
            .collect();
        self.apply_loaded(&imp.path, crate::loading::Loaded { project: imp.project, present, fonts, gone: Vec::new() }, None);
        // Titelbild für die Startseite (das Projekt selbst liegt schon auf der Platte)
        self.send_project_thumb(&imp.path);
        self.import.open = false;
        self.import.results.clear();
        self.import.errors.clear();
    }

    /// Dialog „Projekt importieren“: Auswahl, geführter Fortschritt, Zusammenfassung.
    pub fn import_dialogs(&mut self, ctx: &egui::Context) {
        let _s = crate::app::section("Import");
        self.poll_import(ctx);
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
        let busy = self.import_busy();
        let summary = !busy && (!self.import.results.is_empty() || !self.import.errors.is_empty());
        let mut close = false;
        let mut cancel = false;
        let mut pick: Option<PathBuf> = None;
        let mut open: Option<usize> = None;
        let m = crate::dialog::modal(ctx, "import").show(ctx, |ui| {
            ui.set_width(520.0);
            if let Some(job) = &self.import.job {
                // geführter Fortschritt
                close |= crate::dialog::title(ui, &crate::tr(format!("„{}“ wird importiert", job.label)));
                let n = self.import.batch_total;
                if n > 1 {
                    let i = n - self.import.queue.len();
                    ui.label(RichText::new(crate::tr(format!("Projekt {i} von {n}"))).color(theme::TEXT_DIM()));
                    ui.add_space(4.0);
                }
                crate::loading::steps_view(ui, egui::Id::new("import-steps"), &STEPS, &job.progress.status());
                cancel |= crate::dialog::footer(ui, |ui| ui.add(theme::secondary("Abbrechen")).clicked());
                return;
            }
            if summary {
                close |= crate::dialog::title(ui, "Import abgeschlossen");
                open = self.import_summary(ui);
                close |= crate::dialog::footer(ui, |ui| {
                    if let Some(last) = self.import.results.len().checked_sub(1) {
                        if ui.add(theme::primary("Projekt öffnen")).clicked() {
                            open = Some(last);
                        }
                        false
                    } else {
                        ui.add(theme::primary("OK")).clicked()
                    }
                });
                return;
            }
            close |= crate::dialog::title(ui, "Projekt importieren");
            ui.add_space(8.0);
            if self.settings.import_capcut {
                theme::section(ui, "CapCut-Entwürfe auf diesem Rechner");
                let drafts = self.import.drafts.clone().unwrap_or_default();
                if drafts.is_empty() {
                    theme::empty_state(ui, icon::FOLDER_OPEN, "Keine Entwürfe gefunden", "");
                } else {
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for d in &drafts {
                            ui.horizontal(|ui| {
                                ui.set_min_height(28.0);
                                ui.label(RichText::new(icon::FILM_STRIP).color(theme::TEXT_DIM()));
                                ui.add(egui::Label::new(&d.name).truncate());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add(theme::secondary("Importieren")).clicked() {
                                        pick = Some(d.dir.clone());
                                    }
                                    if let Some(t) = d.modified {
                                        let s = crate::home::ago(t);
                                        ui.label(RichText::new(crate::tr(s)).small().color(theme::TEXT_DIM()));
                                    }
                                });
                            });
                        }
                    });
                }
            }
            crate::dialog::footer(ui, |ui| {
                let mut exts: Vec<&str> = Vec::new();
                if self.settings.import_capcut {
                    exts.push("json");
                }
                if self.settings.import_fcpxml {
                    exts.extend(["fcpxml", "xml"]);
                }
                if ui.add(theme::secondary(format!("{}  Datei wählen …", icon::FOLDER_OPEN))).on_hover_text(crate::tr("draft_content.json (CapCut) oder FCPXML (Final Cut Pro, DaVinci Resolve)")).clicked() {
                    if let Some(p) = crate::dialog().add_filter(crate::tr("Projekte"), &exts).pick_file() {
                        pick = Some(p);
                    }
                }
            });
        });
        // Schließen während des Imports bricht ab (sofort, nichts bleibt zurück)
        if cancel || (busy && (close || (m.should_close() && !fresh))) {
            self.cancel_import();
        } else if (m.should_close() && !fresh) || close {
            self.import.open = false;
            self.import.drafts = None;
            self.import.results.clear();
            self.import.errors.clear();
        }
        if let Some(p) = pick {
            self.start_import(p, ctx);
        }
        if let Some(i) = open {
            self.open_imported(i);
        }
    }

    /// Zusammenfassung: je Projekt übernommen, nicht übernommen (mit Gründen), fehlende Dateien.
    fn import_summary(&self, ui: &mut egui::Ui) -> Option<usize> {
        let mut open = None;
        let dim = |t: String| RichText::new(t).color(theme::TEXT_DIM());
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            let n = self.import.results.len();
            for (i, imp) in self.import.results.iter().enumerate() {
                let r = &imp.report;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::CHECK_CIRCLE).color(theme::ACCENT()));
                    ui.add(egui::Label::new(RichText::new(&imp.name).strong()).truncate());
                    if n > 1 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(theme::ghost("Öffnen")).clicked() {
                                open = Some(i);
                            }
                        });
                    }
                });
                let tracks = imp.project.timeline.tracks.len();
                let mut took = vec![
                    crate::tr(format!("{} Clips", r.clips)),
                    crate::tr(format!("{} Texte", r.texts)),
                    crate::tr(format!("{tracks} Spuren")),
                ];
                if r.nested > 0 {
                    took.push(crate::tr(format!("{} zusammengesetzte Clips aufgelöst", r.nested)));
                }
                ui.label(dim(took.join("  ·  ")));
                let skipped: usize = r.skipped.iter().map(|s| s.1).sum::<usize>() + r.failed.len();
                if skipped > 0 {
                    egui::CollapsingHeader::new(crate::tr(format!("Nicht übernommen: {skipped}")))
                        .id_salt(("import-skipped", i))
                        .show(ui, |ui| {
                            for (w, k) in &r.skipped {
                                ui.label(dim(format!("{}  ×{k}", crate::tr(w))));
                            }
                            if !r.failed.is_empty() {
                                ui.label(dim(format!("{}  ×{}", crate::tr("Datei nicht lesbar"), r.failed.len())));
                            }
                        });
                }
                if !r.missing.is_empty() {
                    egui::CollapsingHeader::new(crate::tr(crate::theme::count_text(
                        r.missing.len(),
                        "{} Mediendatei nicht gefunden",
                        "{} Mediendateien nicht gefunden",
                    )))
                    .id_salt(("import-missing", i))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().id_salt(("import-missing-list", i)).max_height(140.0).show(ui, |ui| {
                            for p in &r.missing {
                                ui.label(RichText::new(p.display().to_string()).small().color(theme::TEXT_DIM()));
                            }
                        });
                    });
                }
                ui.add_space(8.0);
            }
            for (name, e) in &self.import.errors {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::WARNING).color(theme::DANGER));
                    ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                });
                ui.label(dim(crate::tr(e)));
                ui.add_space(8.0);
            }
        });
        open
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
            ui.label(RichText::new(crate::tr(if roots.is_empty() { "Kein CapCut-Entwurfsordner gefunden; Entwürfe lassen sich trotzdem als Datei wählen." } else { "Entwurfsordner gefunden" })).small().color(theme::TEXT_DIM()));
        }
        // Importieren selbst: Hauptmenü → „Projekt aus anderem Programm importieren …“
        let _ = vec2(0.0, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft_dir(root: &Path) -> PathBuf {
        let d = root.join("entwurf");
        std::fs::create_dir_all(&d).unwrap();
        let content = serde_json::json!({"text": "Hallo", "styles": [{"size": 10.0}]}).to_string();
        let j = serde_json::json!({
            "materials": {"texts": [{"id": "T", "content": content}]},
            "tracks": [{"type": "text", "segments": [{"material_id": "T", "target_timerange": {"start": 0, "duration": 1000000}}]}]
        });
        std::fs::write(d.join("draft_content.json"), j.to_string()).unwrap();
        std::fs::write(d.join("draft_meta_info.json"), serde_json::json!({"draft_name": "Mein Film"}).to_string()).unwrap();
        d
    }

    /// Fehlende Datei beim Import: Clip bleibt, beim Öffnen gilt sie als fehlend und
    /// „Medien neu verknüpfen“ geht auf.
    #[test]
    fn opened_import_marks_missing_media_for_relinking() {
        let (mut app, home, _g) = crate::saving::tests::test_app();
        let src = draft_dir(home.path());
        let gone = home.path().join("fehlt.mp4");
        let j = serde_json::json!({
            "materials": {"videos": [{"id": "V", "type": "video", "path": gone.to_string_lossy()}]},
            "tracks": [{"type": "video", "segments": [{"material_id": "V", "target_timerange": {"start": 0, "duration": 1000000}}]}]
        });
        std::fs::write(src.join("draft_content.json"), j.to_string()).unwrap();
        let fonts = Mutex::new(cl_fonts::FontLibrary::default());
        let imp = run_import(&src, &home.path().join("Projekte"), &fonts, &Progress::default()).unwrap();
        assert_eq!(imp.report.missing, [gone.clone()]);
        app.import.results.push(imp);
        app.open_imported(0);
        assert_eq!(app.project.timeline.tracks[0].clips.len(), 1);
        assert!(app.missing_open && !app.presence.get(&gone));
    }

    #[test]
    fn import_saves_a_new_project_next_to_existing_ones() {
        let root = tempfile::tempdir().unwrap();
        let src = draft_dir(root.path());
        let out = root.path().join("Projekte");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("Mein Film.cutline"), "alt").unwrap();
        let fonts = Mutex::new(cl_fonts::FontLibrary::default());
        let imp = run_import(&src, &out, &fonts, &Progress::default()).unwrap();
        assert_eq!(imp.path, out.join("Mein Film (2).cutline"));
        assert_eq!(std::fs::read_to_string(out.join("Mein Film.cutline")).unwrap(), "alt");
        let saved = Project::from_json(&std::fs::read_to_string(&imp.path).unwrap()).unwrap();
        assert_eq!(saved.timeline.tracks.len(), imp.project.timeline.tracks.len());
    }

    #[test]
    fn cancelled_or_failed_import_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let src = draft_dir(root.path());
        let out = root.path().join("Projekte");
        let fonts = Mutex::new(cl_fonts::FontLibrary::default());
        let p = Progress::default();
        p.cancel();
        assert_eq!(run_import(&src, &out, &fonts, &p).err().as_deref(), Some(cl_import::CANCELLED));
        std::fs::write(src.join("draft_content.json"), "{kaputt").unwrap();
        assert!(run_import(&src, &out, &fonts, &Progress::default()).is_err());
        assert_eq!(std::fs::read_dir(&out).map_or(0, |d| d.count()), 0, "kein halbes Projekt");
    }
}
