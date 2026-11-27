//! Import projects from other editors: dialog on the start page (local
//! CapCut drafts and file picker), settings page „Import“ for switching on/off.
//!
//! The import runs fully in the background, in steps (read draft → find media →
//! check media → fonts → timeline → create project) with progress and cancel. The
//! dialog guides through the steps and shows a summary afterward; several drafts
//! run one after another. Cancelled or failed, nothing is left behind.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cl_core::{MediaKind, Project};
use cl_import::{Progress, Report, Step};
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::theme::{self, icon};
use crate::widgets;

/// A finished imported project (already saved as `.cutline`)
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
    /// The draft list is read in the background (folders and files, not in the UI thread)
    drafts_rx: Option<Receiver<Vec<cl_import::capcut::Draft>>>,
    job: Option<Job>,
    /// waiting drafts; counter for "Project 2 of 5"
    queue: VecDeque<PathBuf>,
    batch_total: usize,
    /// finished imports of this round (summary)
    results: Vec<Imported>,
    /// Errors of this round: (draft, message)
    errors: Vec<(String, String)>,
}

/// Steps of the import (display)
const STEPS: [(Step, &str); 6] = [
    (Step::Read, "Read draft"),
    (Step::Find, "Find media"),
    (Step::Probe, "Check media"),
    (Step::Fonts, "Fonts"),
    (Step::Timeline, "Build timeline"),
    (Step::Save, "Create project"),
];

/// Free file name in the project folder (`Name.cutline`, `Name (2).cutline` …)
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

/// The whole import in the background: convert, pre-read images, save the project, take over
/// fonts. Cancel → nothing is left behind (not even an already written project file).
pub fn run_import(
    src: &Path,
    dir: &Path,
    fonts: &Mutex<cl_fonts::FontLibrary>,
    progress: &Progress,
) -> cl_import::Result<Imported> {
    let t = Instant::now();
    let (project, mut report) = cl_import::import_with(src, progress)?;
    report.missing.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    progress.check()?;
    progress.set(Step::Save, 0, 2, &project.name);
    // Read image animations (thumbnail time) here, not on the first frame
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
    // Take over the fonts of the texts (existing ones are skipped)
    for f in &report.fonts {
        let _ = fonts.lock().unwrap_or_else(|e| e.into_inner()).import_file(f);
    }
    progress.tick("");
    // cancelled during the fonts: no project is left behind then either
    if progress.cancelled() {
        let _ = std::fs::remove_file(&path);
        return Err(cl_import::CANCELLED.into());
    }
    if crate::app::perf() {
        eprintln!(
            "perf: import “{}” {:.1} s, {} media, {} clips, {} texts",
            project.name,
            t.elapsed().as_secs_f64(),
            project.media.len(),
            report.clips,
            report.texts
        );
    }
    Ok(Imported { name: project.name.clone(), path, project, report })
}

/// Natural order: numbers in the name by their value („fehlt_2“ before „fehlt_10“).
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let num = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.next_if(|c| c.is_ascii_digit()) {
                        s.push(c);
                    }
                    s.trim_start_matches('0').to_string()
                };
                let (n, m) = (num(&mut a), num(&mut b));
                // without leading zeros: first the length, then the digits (no overflows)
                let o = n.len().cmp(&m.len()).then_with(|| n.cmp(&m));
                if o.is_ne() {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o.is_ne() {
                    return o;
                }
                a.next();
                b.next();
            }
        }
    }
}

impl App {
    pub fn import_enabled(&self) -> bool {
        self.settings.import_capcut || self.settings.import_fcpxml
    }

    /// Is an import running (or one waiting)?
    pub fn import_busy(&self) -> bool {
        self.import.job.is_some() || !self.import.queue.is_empty()
    }

    /// Start an import (draft folder or file); if one is already running, it is queued.
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
            .expect("import thread");
        self.import.job = Some(Job { label, progress, rx });
    }

    /// Cancel all imports of this round (takes effect at once; the thread cleans up after itself).
    pub(crate) fn cancel_import(&mut self) {
        if let Some(j) = self.import.job.take() {
            j.progress.cancel();
        }
        self.import.queue.clear();
    }

    /// Take over a finished import (every frame).
    pub fn poll_import(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.import.job else { return };
        let res = match job.rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            // import thread crashed: otherwise the display spins forever
            Err(TryRecvError::Disconnected) => Err("Import failed (internal error)".into()),
        };
        let Some(job) = self.import.job.take() else { return };
        match res {
            Ok(imp) => {
                if !imp.report.fonts.is_empty() {
                    self.register_fonts(ctx);
                }
                self.home.refresh();
                if !self.import.open {
                    self.toast(format!("“{}” imported: {} clips, {} texts", imp.name, imp.report.clips, imp.report.texts));
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

    /// Open the imported project (from memory, without reading it again).
    pub fn open_imported(&mut self, i: usize) {
        if i >= self.import.results.len() {
            return;
        }
        let imp = self.import.results.remove(i);
        // missing files (placeholders): „Relink media“ opens right away
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
        // Cover image for the start page (the project itself is already on disk)
        self.send_project_thumb(&imp.path);
        self.import.open = false;
        self.import.results.clear();
        self.import.errors.clear();
    }

    /// „Import project“ dialog: selection, guided progress, summary.
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
            match &self.import.drafts_rx {
                Some(rx) => {
                    if let Ok(d) = rx.try_recv() {
                        self.import.drafts = Some(d);
                        self.import.drafts_rx = None;
                    }
                }
                None => {
                    let (tx, rx) = channel();
                    let c = ctx.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(cl_import::capcut::list_drafts());
                        c.request_repaint();
                    });
                    self.import.drafts_rx = Some(rx);
                }
            }
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
                // guided progress
                close |= crate::dialog::title(ui, &crate::tr(format!("Importing “{}”", job.label)));
                let n = self.import.batch_total;
                if n > 1 {
                    let i = n - self.import.queue.len();
                    ui.label(RichText::new(crate::tr(format!("Project {i} of {n}"))).color(theme::TEXT_DIM()));
                    ui.add_space(4.0);
                }
                crate::loading::steps_view(ui, egui::Id::new("import-steps"), &STEPS, &job.progress.status());
                cancel |= crate::dialog::footer(ui, |ui| ui.add(theme::secondary("Cancel")).clicked());
                return;
            }
            if summary {
                close |= crate::dialog::title(ui, "Import finished");
                open = self.import_summary(ui);
                close |= crate::dialog::footer(ui, |ui| {
                    if let Some(last) = self.import.results.len().checked_sub(1) {
                        if ui.add(theme::primary("Open project")).clicked() {
                            open = Some(last);
                        }
                        false
                    } else {
                        ui.add(theme::primary("OK")).clicked()
                    }
                });
                return;
            }
            close |= crate::dialog::title(ui, "Import project");
            ui.add_space(8.0);
            if self.settings.import_capcut {
                theme::section(ui, "CapCut drafts on this computer");
                let drafts = self.import.drafts.clone().unwrap_or_default();
                if self.import.drafts.is_none() {
                    ui.spinner();
                } else if drafts.is_empty() {
                    theme::empty_state(ui, icon::FOLDER_OPEN, "No drafts found", "");
                } else {
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for d in &drafts {
                            ui.horizontal(|ui| {
                                ui.set_min_height(28.0);
                                ui.label(RichText::new(icon::FILM_STRIP).color(theme::TEXT_DIM()));
                                ui.add(egui::Label::new(&d.name).truncate());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add(theme::secondary("Import~verb")).clicked() {
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
                if ui.add(theme::secondary(format!("{}  Choose file …", icon::FOLDER_OPEN))).on_hover_text(crate::tr("draft_content.json (CapCut) or FCPXML (Final Cut Pro, DaVinci Resolve)")).clicked() {
                    if let Some(p) = crate::dialog().add_filter(crate::tr("Projects"), &exts).pick_file() {
                        pick = Some(p);
                    }
                }
            });
        });
        // Closing during the import cancels it (at once, nothing is left behind)
        if cancel || (busy && (close || (m.should_close() && !fresh))) {
            self.cancel_import();
        } else if (m.should_close() && !fresh) || close {
            self.import.open = false;
            self.import.drafts = None;
            self.import.drafts_rx = None;
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

    /// Summary: per project taken over, not taken over (with reasons), missing files.
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
                            if ui.add(theme::ghost("Open")).clicked() {
                                open = Some(i);
                            }
                        });
                    }
                });
                let tracks = imp.project.timeline.tracks.len();
                let mut took = vec![
                    crate::tr(format!("{} clips", r.clips)),
                    crate::tr(format!("{} texts", r.texts)),
                    crate::tr(format!("{tracks} tracks")),
                ];
                if r.nested > 0 {
                    took.push(crate::tr(format!("{} compound clips resolved", r.nested)));
                }
                ui.label(dim(took.join("  ·  ")));
                let skipped: usize = r.skipped.iter().map(|s| s.1).sum::<usize>() + r.failed.len();
                if skipped > 0 {
                    egui::CollapsingHeader::new(crate::tr(format!("Not imported: {skipped}")))
                        .id_salt(("import-skipped", i))
                        .show(ui, |ui| {
                            for (w, k) in &r.skipped {
                                ui.label(dim(format!("{}  ×{k}", crate::tr(w))));
                            }
                            if !r.failed.is_empty() {
                                ui.label(dim(format!("{}  ×{}", crate::tr("File can't be read"), r.failed.len())));
                            }
                        });
                }
                if !r.missing.is_empty() {
                    egui::CollapsingHeader::new(crate::tr(crate::theme::count_text(
                        r.missing.len(),
                        "{} media file not found",
                        "{} media files not found",
                    )))
                    .id_salt(("import-missing", i))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().id_salt(("import-missing-list", i)).max_height(140.0).show(ui, |ui| {
                            for p in &r.missing {
                                ui.label(RichText::new(crate::paths::shown(p)).small().color(theme::TEXT_DIM()));
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

    /// Settings page „Import“.
    pub fn import_settings(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let mut changed =
            widgets::toggle_row(ui, "CapCut drafts", &mut self.settings.import_capcut);
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
            ui.label(RichText::new(crate::tr(if roots.is_empty() { "No CapCut draft folder found; you can still pick a draft as a file." } else { "Draft folder found" })).small().color(theme::TEXT_DIM()));
        }
        // Importing itself: main menu → „Import project from another app …“
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft_dir(root: &Path) -> PathBuf {
        let d = root.join("draft");
        std::fs::create_dir_all(&d).unwrap();
        let content = serde_json::json!({"text": "Hello", "styles": [{"size": 10.0}]}).to_string();
        let j = serde_json::json!({
            "materials": {"texts": [{"id": "T", "content": content}]},
            "tracks": [{"type": "text", "segments": [{"material_id": "T", "target_timerange": {"start": 0, "duration": 1000000}}]}]
        });
        std::fs::write(d.join("draft_content.json"), j.to_string()).unwrap();
        std::fs::write(d.join("draft_meta_info.json"), serde_json::json!({"draft_name": "My Film"}).to_string()).unwrap();
        d
    }

    /// Import dialog: the CapCut drafts are read in the background, not in the first frame
    /// (folders and files in the UI thread caused a long frame on import).
    #[test]
    fn draft_list_is_read_off_the_ui_thread() {
        let mut app = crate::saving::tests::test_app();
        let ctx = egui::Context::default();
        app.settings.import_capcut = true;
        app.import.open = true;
        let _ = ctx.run(Default::default(), |ctx| app.import_dialogs(ctx));
        assert!(app.import.drafts.is_none(), "draft list read in the UI thread");
        let t = Instant::now();
        while app.import.drafts.is_none() && t.elapsed().as_secs() < 30 {
            std::thread::yield_now();
            let _ = ctx.run(Default::default(), |ctx| app.import_dialogs(ctx));
        }
        assert!(app.import.drafts.is_some());
    }

    /// Missing file on import: the clip stays, on opening it counts as missing and
    /// „Relink media“ opens.
    #[test]
    fn opened_import_marks_missing_media_for_relinking() {
        let mut app = crate::saving::tests::test_app();
        let src = draft_dir(app.dir());
        let gone = app.dir().join("missing.mp4");
        let j = serde_json::json!({
            "materials": {"videos": [{"id": "V", "type": "video", "path": gone.to_string_lossy()}]},
            "tracks": [{"type": "video", "segments": [{"material_id": "V", "target_timerange": {"start": 0, "duration": 1000000}}]}]
        });
        std::fs::write(src.join("draft_content.json"), j.to_string()).unwrap();
        let fonts = Mutex::new(cl_fonts::FontLibrary::default());
        let imp = run_import(&src, &app.dir().join("Projects"), &fonts, &Progress::default()).unwrap();
        assert_eq!(imp.report.missing, [gone.clone()]);
        app.import.results.push(imp);
        app.open_imported(0);
        assert_eq!(app.project.timeline.tracks[0].clips.len(), 1);
        assert!(app.missing_open && !app.presence.get(&gone));
    }

    #[test]
    fn missing_files_are_listed_in_natural_order() {
        let root = tempfile::tempdir().unwrap();
        let src = draft_dir(root.path());
        let names = ["missing_1", "missing_10", "missing_0", "Missing_3", "missing_2"];
        let p = |n: &str| root.path().join(format!("{n}.mp4"));
        let videos: Vec<_> = names.iter().map(|n| serde_json::json!({"id": n, "type": "video", "path": p(n).to_string_lossy()})).collect();
        let segs: Vec<_> = names
            .iter()
            .enumerate()
            .map(|(i, n)| serde_json::json!({"material_id": n, "target_timerange": {"start": i * 1000000, "duration": 1000000}}))
            .collect();
        let j = serde_json::json!({"materials": {"videos": videos}, "tracks": [{"type": "video", "segments": segs}]});
        std::fs::write(src.join("draft_content.json"), j.to_string()).unwrap();
        let fonts = Mutex::new(cl_fonts::FontLibrary::default());
        let imp = run_import(&src, &root.path().join("Projects"), &fonts, &Progress::default()).unwrap();
        let want: Vec<_> = ["missing_0", "missing_1", "missing_2", "Missing_3", "missing_10"].iter().map(|n| p(n)).collect();
        assert_eq!(imp.report.missing, want);
        assert!(natural_cmp("a007", "a7").is_eq() && natural_cmp("a99999999999999999999999", "a100000000000000000000000").is_lt());
    }

    /// Cancel during "Take over fonts" (after writing): no project is left behind.
    #[test]
    fn cancel_during_fonts_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let src = draft_dir(root.path());
        let out = root.path().join("Projects");
        let fonts = Mutex::new(cl_fonts::FontLibrary::default());
        let slot: Arc<std::sync::OnceLock<std::sync::Weak<Progress>>> = Arc::default();
        let sl = slot.clone();
        let p = Arc::new(Progress::new(move || {
            let Some(p) = sl.get().and_then(|w| w.upgrade()) else { return };
            let st = p.status();
            if st.step == Step::Save && st.done >= st.total {
                p.cancel();
            }
        }));
        slot.set(Arc::downgrade(&p)).unwrap();
        assert_eq!(run_import(&src, &out, &fonts, &p).err().as_deref(), Some(cl_import::CANCELLED));
        assert_eq!(std::fs::read_dir(&out).map_or(0, |d| d.count()), 0, "no half project");
    }

    /// Quitting cancels a running import (otherwise its checks keep running).
    #[test]
    fn exit_cancels_running_import() {
        let mut app = crate::saving::tests::test_app();
        let progress = Arc::new(Progress::default());
        let (_tx, rx) = channel();
        app.import.job = Some(Job { label: "x".into(), progress: progress.clone(), rx });
        eframe::App::on_exit(&mut *app);
        assert!(progress.cancelled());
    }

    #[test]
    fn import_saves_a_new_project_next_to_existing_ones() {
        let root = tempfile::tempdir().unwrap();
        let src = draft_dir(root.path());
        let out = root.path().join("Projects");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("My Film.cutline"), "old").unwrap();
        let fonts = Mutex::new(cl_fonts::FontLibrary::default());
        let imp = run_import(&src, &out, &fonts, &Progress::default()).unwrap();
        assert_eq!(imp.path, out.join("My Film (2).cutline"));
        assert_eq!(std::fs::read_to_string(out.join("My Film.cutline")).unwrap(), "old");
        let saved = Project::from_json(&std::fs::read_to_string(&imp.path).unwrap()).unwrap();
        assert_eq!(saved.timeline.tracks.len(), imp.project.timeline.tracks.len());
    }

    #[test]
    fn cancelled_or_failed_import_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let src = draft_dir(root.path());
        let out = root.path().join("Projects");
        let fonts = Mutex::new(cl_fonts::FontLibrary::default());
        let p = Progress::default();
        p.cancel();
        assert_eq!(run_import(&src, &out, &fonts, &p).err().as_deref(), Some(cl_import::CANCELLED));
        std::fs::write(src.join("draft_content.json"), "{broken").unwrap();
        assert!(run_import(&src, &out, &fonts, &Progress::default()).is_err());
        assert_eq!(std::fs::read_dir(&out).map_or(0, |d| d.count()), 0, "no half project");
    }
}
