//! Update check in the background (GitHub Releases), install at the push of a button, and „Was ist neu“.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use cl_update::changelog::{self, Notes};
use cl_update::{Release, Version};
use eframe::egui::{self, vec2, RichText};

use crate::app::App;
use crate::theme::{self, icon};
use crate::widgets;

#[derive(Clone)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available(Box<Release>),
    /// Release, downloaded bytes, total size
    Downloading(Box<Release>, u64, u64),
    /// Installed, restart needed
    Installed(Version),
    /// Message; failed while downloading: the release for „Erneut versuchen“
    Error(String, Option<Box<Release>>),
}

/// Content of the „Update“ / „Was ist neu“ window
pub enum View {
    /// New version: changes from the release notes (empty = show the raw text)
    Update(Vec<Notes>),
    /// Embedded change list
    News(Vec<Notes>),
}

pub struct Updater {
    pub state: Arc<Mutex<UpdateState>>,
    cancel: Arc<AtomicBool>,
    /// End of the last check
    checked: Arc<Mutex<Option<SystemTime>>>,
    pub view: Option<View>,
    /// Offered once in the title bar after an update
    pub news: Vec<Notes>,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(UpdateState::Idle)),
            cancel: Arc::new(AtomicBool::new(false)),
            checked: Default::default(),
            view: None,
            news: Vec::new(),
        }
    }
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Click test without a real update, only in developer builds (debug or `CUTLINE_SHOTS`):
/// `CUTLINE_FAKE_UPDATE=0.9.0` reports this version as installed; the check then returns, without
/// network, the own version with the embedded change list (downloading fails at once).
fn fake_installed() -> Option<Version> {
    let dev = cfg!(debug_assertions) || std::env::var_os("CUTLINE_SHOTS").is_some();
    dev.then(|| std::env::var("CUTLINE_FAKE_UPDATE").ok())
        .flatten()
        .and_then(|v| Version::parse(&v))
}

fn own_version() -> Version {
    Version::parse(current_version()).unwrap_or(Version(0, 0, 0))
}

/// Version being updated from
pub fn installed_version() -> Version {
    fake_installed().unwrap_or_else(own_version)
}

fn fake_release() -> Release {
    let v = own_version();
    Release {
        version: v,
        tag: format!("v{v}"),
        name: format!("v{v}"),
        notes: changelog::CHANGELOG.into(),
        page: format!("https://github.com/{}/releases", cl_update::REPO),
        // Port 9 on the local machine: the connection is refused at once
        asset_url: "http://127.0.0.1:9/cutline.zip".into(),
        asset_size: 48_300_000,
        sha_url: Some("http://127.0.0.1:9/cutline.zip.sha256".into()),
        changes_url: None,
        changes: None,
    }
}

/// Development builds (cargo run) are not overwritten.
pub fn can_install() -> bool {
    !cfg!(debug_assertions)
}

fn exe_dir() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|p| p.to_path_buf())
}

/// Changes of the new version(s): from the `changes.json` attachment (all versions), for older
/// releases from the release notes (up to three older ones appended collapsed). Empty = not readable.
pub fn release_notes(rel: &Release, installed: Version) -> Vec<Notes> {
    let all = rel.changes.clone().unwrap_or_else(|| changelog::parse(&rel.notes));
    changelog::between(&all, installed, rel.version)
        .into_iter()
        .cloned()
        .collect()
}

/// All versions of this build, newest at the top
pub fn embedded_notes() -> Vec<Notes> {
    changelog::between(&changelog::parse(changelog::CHANGELOG), Version(0, 0, 0), own_version())
        .into_iter()
        .cloned()
        .collect()
}

/// „42,3 MB“ (German) or „42.3 MB“
pub(crate) fn mb_text(bytes: u64, comma: bool) -> String {
    let s = format!("{:.1} MB", bytes as f64 / 1_048_576.0);
    if comma { s.replace('.', ",") } else { s }
}

/// „42 % · 17,8 / 42,3 MB“; without a known size only the downloaded amount
pub(crate) fn progress_text(done: u64, total: u64, comma: bool) -> String {
    if total <= 1 {
        return mb_text(done, comma);
    }
    let pct = (done.min(total) as f64 * 100.0 / total as f64).floor();
    let total_mb = mb_text(total, comma);
    let done_mb = mb_text(done.min(total), comma);
    format!("{pct:.0} % · {} / {total_mb}", done_mb.trim_end_matches(" MB"))
}

/// Temp folder for the checked update package: in the own data folder, not in the shared temp folder
/// (on Linux another user could swap the ZIP there between checking and unpacking)
fn update_dir() -> std::path::PathBuf {
    crate::paths::data_dir().join("update")
}

/// Check result → state (short message) and log line (with raw text, e.g. TLS/proxy/DNS errors)
fn check_outcome(r: Result<Option<Release>, String>) -> (UpdateState, String) {
    match r {
        Ok(Some(rel)) => {
            let from = if rel.changes.is_some() { " (Änderungen aus changes.json)" } else { "" };
            let line = format!("Version {} verfügbar{from}", rel.version);
            (UpdateState::Available(Box::new(rel)), line)
        }
        Ok(None) => (UpdateState::UpToDate, "aktuell".into()),
        Err(e) => (UpdateState::Error(cl_update::friendly(&e), None), format!("Fehler {e}")),
    }
}

fn fraction(done: u64, total: u64) -> Option<f32> {
    (total > 1).then(|| (done.min(total) as f64 / total as f64) as f32)
}

/// `2026-11-15` → „15.11.2026“ (German), otherwise unchanged
fn date_text(iso: &str, de: bool) -> String {
    let p: Vec<&str> = iso.split('-').collect();
    match p.as_slice() {
        [y, m, d] if de => format!("{d}.{m}.{y}"),
        _ => iso.to_string(),
    }
}

fn de() -> bool {
    cl_i18n::ui_lang() == cl_i18n::Lang::De
}

impl Updater {
    pub fn get(&self) -> UpdateState {
        self.state
            .lock()
            .map(|s| s.clone())
            .unwrap_or(UpdateState::Idle)
    }

    pub fn checked(&self) -> Option<SystemTime> {
        self.checked.lock().ok().and_then(|c| *c)
    }

    pub fn check(&self, ctx: &egui::Context) {
        if matches!(
            self.get(),
            UpdateState::Checking | UpdateState::Downloading(..)
        ) {
            return;
        }
        *self.state.lock().unwrap() = UpdateState::Checking;
        let (state, checked, ctx) = (self.state.clone(), self.checked.clone(), ctx.clone());
        let fake = fake_installed();
        std::thread::spawn(move || {
            let r = match fake {
                Some(_) => Ok(Some(fake_release())),
                None => cl_update::check(current_version()),
            };
            let (r, line) = check_outcome(r);
            let level = if matches!(r, UpdateState::Error(..)) { log::Level::Warn } else { log::Level::Info };
            log::log!(target: "update", level, "Suche: {line}");
            *state.lock().unwrap() = r;
            *checked.lock().unwrap() = Some(SystemTime::now());
            ctx.request_repaint();
        });
    }

    pub fn install(&self, rel: Release, ctx: &egui::Context) {
        let Some(dir) = exe_dir() else { return };
        self.cancel.store(false, Ordering::Relaxed);
        *self.state.lock().unwrap() = UpdateState::Downloading(Box::new(rel.clone()), 0, rel.asset_size);
        let (state, cancel, ctx) = (self.state.clone(), self.cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let tmp = update_dir();
            let st = state.clone();
            let c2 = ctx.clone();
            let boxed = Box::new(rel.clone());
            let res = cl_update::download(
                &rel,
                &tmp,
                |done, total| {
                    *st.lock().unwrap() = UpdateState::Downloading(boxed.clone(), done, total);
                    c2.request_repaint_after(std::time::Duration::from_millis(100));
                },
                &cancel,
            )
            .and_then(|zip| {
                let n = cl_update::install(&zip, &dir);
                let _ = std::fs::remove_file(&zip);
                n
            });
            log::info!(target: "update", "Installation {}: {}", rel.version, res.as_ref().map_or_else(|e| format!("Fehler {e}"), |_| "fertig".into()));
            *state.lock().unwrap() = match res {
                Ok(_) => UpdateState::Installed(rel.version),
                // Cancel leads back to the offer
                Err(_) if cancel.load(Ordering::Relaxed) => UpdateState::Available(Box::new(rel)),
                Err(e) => UpdateState::Error(cl_update::friendly(&e), Some(Box::new(rel))),
            };
            ctx.request_repaint();
        });
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Open the window for the available update
    pub fn open_update(&mut self) {
        let notes = match self.get() {
            UpdateState::Available(rel) | UpdateState::Downloading(rel, ..) | UpdateState::Error(_, Some(rel)) => {
                release_notes(&rel, installed_version())
            }
            _ => Vec::new(),
        };
        self.view = Some(View::Update(notes));
    }
}

/// Restart the program (after installation).
pub fn restart(ctx: &egui::Context) {
    if let Ok(exe) = std::env::current_exe() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if std::process::Command::new(exe).args(args).spawn().is_ok() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// At startup: remove leftovers of old updates, plus work folders of aborted runs in %TEMP%.
pub fn startup_cleanup() {
    if let Some(d) = exe_dir() {
        cl_update::cleanup_old(&d);
    }
    std::thread::spawn(|| {
        cleanup_temp(&std::env::temp_dir(), std::time::Duration::from_secs(2 * 86_400));
        cl_media::cleanup_graphs();
        cl_update::packs::cleanup_parts(&crate::contents::download_dir(), crate::paths::pid_alive);
    });
}

/// Work folders of audio preview, export, Whisper and thumbnail are only left behind after a crash or a
/// forced quit (exports up to a few MB). Only own folders unchanged for longer than `age`
/// are removed – running instances do not use their folders that long.
fn cleanup_temp(dir: &std::path::Path, age: std::time::Duration) {
    const OWN: [&str; 4] = ["cutline-audio-", "cutline-export-", "cutline-whisper-", "cutline-thumb"];
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let own = OWN.iter().any(|p| e.file_name().to_string_lossy().starts_with(p));
        let old = e
            .metadata()
            .ok()
            .filter(|m| m.is_dir())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|a| a >= age);
        if own && old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod temp_tests {
    #[test]
    fn cleanup_temp_removes_only_own_old_dirs() {
        let t = tempfile::tempdir().unwrap();
        for d in ["cutline-audio-x1", "cutline-export-x2", "cutline-whisper-1-0", "cutline-thumbAbc12", "cutline-models", "fremd"] {
            std::fs::create_dir(t.path().join(d)).unwrap();
        }
        std::fs::write(t.path().join("cutline-export-datei"), "x").unwrap();
        // freshly created: stays (could belong to a running instance)
        super::cleanup_temp(t.path(), std::time::Duration::from_secs(3600));
        assert!(t.path().join("cutline-audio-x1").exists());
        super::cleanup_temp(t.path(), std::time::Duration::ZERO);
        let mut left: Vec<String> = std::fs::read_dir(t.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["cutline-export-datei", "cutline-models", "fremd"]);
    }
}

/// Progress or running line of the update window (4 px); `None` = duration unknown.
fn progress_line(ui: &mut egui::Ui, frac: Option<f32>) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
    match frac {
        Some(f) => crate::widgets::progress_line(ui.painter(), ui.id().with("update"), r, f),
        None => crate::widgets::busy_line(ui.painter(), r),
    }
}

/// One entry: light title, muted sentence, wrapped in the flow
fn entry(ui: &mut egui::Ui, e: &changelog::Entry) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new("•").size(theme::FS_TEXT).color(theme::TEXT_FAINT()));
        let mut job = egui::text::LayoutJob::default();
        let style = ui.style().clone();
        if !e.title.is_empty() {
            RichText::new(format!("{} ", e.title))
                .size(theme::FS_TEXT)
                .color(theme::TEXT())
                .append_to(&mut job, &style, egui::FontSelection::Default, egui::Align::Min);
        }
        RichText::new(&e.text)
            .size(theme::FS_TEXT)
            .color(theme::TEXT_DIM())
            .append_to(&mut job, &style, egui::FontSelection::Default, egui::Align::Min);
        ui.add(egui::Label::new(job).wrap());
    });
}

/// Changes of one or more versions; `headings`: version line above each section
fn notes_list(ui: &mut egui::Ui, notes: &[Notes], headings: bool) {
    let de = de();
    for (i, n) in notes.iter().enumerate() {
        if headings {
            if i > 0 {
                ui.add_space(14.0);
            }
            let v = n.version.map(|v| v.to_string()).unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(RichText::new(v).size(theme::FS_TITLE).strong().color(theme::TEXT()));
                if !n.date.is_empty() {
                    ui.label(RichText::new(date_text(&n.date, de)).size(theme::FS_UI).color(theme::TEXT_DIM()));
                }
            });
        }
        for g in n.groups(de).iter().filter(|g| !g.entries.is_empty()) {
            ui.add_space(8.0);
            if !g.title.is_empty() {
                ui.label(RichText::new(&g.title).size(theme::FS_UI).strong().color(theme::TEXT_DIM()));
                ui.add_space(2.0);
            }
            for e in &g.entries {
                entry(ui, e);
                ui.add_space(3.0);
            }
        }
    }
}

impl App {
    /// At startup: offer „Was ist neu“ once after an update (never on a fresh install),
    /// then remember the started version.
    pub fn note_started_version(&mut self) {
        let last = self.settings.last_version.as_deref().and_then(Version::parse);
        let all = changelog::parse(changelog::CHANGELOG);
        self.updater.news = changelog::news_after_update(&all, last, own_version(), self.settings.onboarding_done)
            .into_iter()
            .cloned()
            .collect();
        if self.settings.last_version.as_deref() != Some(current_version()) {
            self.settings.last_version = Some(current_version().into());
            self.settings.save();
        }
    }

    /// Small hint in the title bar (only when there is something to do).
    pub fn update_badge(&mut self, ui: &mut egui::Ui) {
        if let Some(v) = self.updater.news.first().and_then(|n| n.version) {
            let b = theme::secondary(format!("{}  Was ist neu in {v}", icon::SPARKLE)).min_size(vec2(0.0, 22.0));
            if ui.add(b).clicked() {
                self.updater.view = Some(View::News(std::mem::take(&mut self.updater.news)));
            }
        }
        match self.updater.get() {
            UpdateState::Available(rel)
                if self.settings.skipped_version.as_deref() != Some(&rel.version.to_string()) =>
            {
                let b = theme::secondary(format!("{}  Update {}", icon::ARROW_CIRCLE_UP, rel.version))
                    .min_size(vec2(0.0, 22.0));
                if ui
                    .add(b)
                    .on_hover_text(crate::tr("Neue Version verfügbar"))
                    .clicked()
                {
                    self.updater.open_update();
                }
            }
            UpdateState::Downloading(_, done, total) => {
                let r = ui.add(
                    egui::Label::new(
                        RichText::new(crate::tr(format!(
                            "{}  Update {:.0} %",
                            icon::DOWNLOAD_SIMPLE,
                            fraction(done, total).unwrap_or(0.0) * 100.0
                        )))
                        .size(theme::FS_UI)
                        .color(theme::TEXT_DIM()),
                    )
                    .sense(egui::Sense::click()),
                );
                if r.clicked() {
                    self.updater.open_update();
                }
            }
            UpdateState::Installed(v) => {
                let b = theme::primary(format!("{}  Neu starten ({v})", icon::ARROWS_CLOCKWISE))
                    .min_size(vec2(0.0, 22.0));
                if ui
                    .add(b)
                    .on_hover_text(crate::tr("Update ist installiert. Cutline neu starten"))
                    .clicked()
                {
                    self.restart_after_update(ui.ctx());
                }
            }
            _ => {}
        }
    }

    fn restart_after_update(&mut self, ctx: &egui::Context) {
        self.settings.save();
        restart(ctx);
    }

    /// „Update verfügbar“ or „Was ist neu“ window (above everything, also above the settings)
    pub fn update_dialog(&mut self, ctx: &egui::Context) {
        let Some(view) = self.updater.view.take() else { return };
        let state = self.updater.get();
        let mut close = false;
        let modal = crate::dialog::modal(ctx, "update").show(ctx, |ui| {
            ui.set_width(520.0);
            match &view {
                View::News(notes) => {
                    close |= crate::dialog::title(ui, "Was ist neu");
                    egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        notes_list(ui, notes, true);
                    });
                    crate::dialog::footer(ui, |ui| {
                        close |= ui.add(theme::primary("Schließen")).clicked();
                    });
                }
                View::Update(notes) => close |= self.update_view(ui, &state, notes),
            }
        });
        if modal.should_close() {
            close = true;
        }
        // Buttons in the window may have set a new view
        if !close && self.updater.view.is_none() {
            self.updater.view = Some(view);
        }
    }

    /// Content for an available update. `true` = close.
    fn update_view(&mut self, ui: &mut egui::Ui, state: &UpdateState, notes: &[Notes]) -> bool {
        let rel = match state {
            UpdateState::Available(r) | UpdateState::Downloading(r, ..) | UpdateState::Error(_, Some(r)) => Some(r.as_ref()),
            _ => None,
        };
        let mut close = crate::dialog::title(
            ui,
            if matches!(state, UpdateState::Installed(_)) { "Update installiert" } else { "Update verfügbar" },
        );
        let de = de();
        // Header: „0.11.0 → 0.11.1“, below it date and size
        if let Some(rel) = rel {
            ui.horizontal(|ui| {
                ui.label(RichText::new(installed_version().to_string()).size(theme::FS_PAGE).color(theme::TEXT_DIM()));
                ui.label(RichText::new(icon::ARROW_RIGHT).size(theme::FS_TITLE).color(theme::TEXT_FAINT()));
                ui.label(RichText::new(rel.version.to_string()).size(theme::FS_PAGE).strong().color(theme::TEXT()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, icon::ARROW_SQUARE_OUT, "Auf GitHub ansehen", true, false).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(&rel.page));
                    }
                });
            });
            let date = notes.iter().find(|n| n.version == Some(rel.version)).map(|n| date_text(&n.date, de));
            let size = (rel.asset_size > 0).then(|| mb_text(rel.asset_size, de));
            let meta: Vec<String> = date.into_iter().chain(size).filter(|s| !s.is_empty()).collect();
            if !meta.is_empty() {
                ui.label(RichText::new(meta.join(" · ")).size(theme::FS_UI).color(theme::TEXT_DIM()));
            }
            ui.add_space(10.0);
            egui::Frame::default()
                .fill(theme::BG_INPUT())
                .corner_radius(theme::R_CARD)
                .inner_margin(theme::PAD_CARD_L)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if notes.is_empty() {
                            // release notes not in the expected structure: show them plainly
                            ui.label(RichText::new(rel.notes.trim()).size(theme::FS_TEXT).color(theme::TEXT_DIM()));
                        } else {
                            notes_list(ui, notes, notes.len() > 1);
                        }
                    });
                });
        }
        ui.add_space(10.0);
        match state {
            UpdateState::Downloading(_, done, total) => {
                progress_line(ui, fraction(*done, *total));
                ui.add_space(4.0);
                ui.label(RichText::new(progress_text(*done, *total, de)).size(theme::FS_UI).color(theme::TEXT_DIM()));
                crate::dialog::footer(ui, |ui| {
                    if ui.add(theme::secondary("Abbrechen")).clicked() {
                        self.updater.cancel();
                    }
                });
            }
            UpdateState::Installed(v) => {
                ui.label(
                    RichText::new(crate::tr(format!("{}  Version {v} ist installiert", icon::CHECK_CIRCLE)))
                        .size(theme::FS_TEXT)
                        .color(theme::TEXT()),
                );
                crate::dialog::footer(ui, |ui| {
                    if ui.add(theme::primary(format!("{}  Jetzt neu starten", icon::ARROWS_CLOCKWISE))).clicked() {
                        self.restart_after_update(ui.ctx());
                    }
                    close |= ui.add(theme::secondary("Später")).clicked();
                });
            }
            UpdateState::Error(e, r) => {
                ui.label(
                    RichText::new(format!("{}  {}", icon::WARNING_CIRCLE, crate::tr(e)))
                        .size(theme::FS_TEXT)
                        .color(theme::WARN_TEXT()),
                );
                crate::dialog::footer(ui, |ui| {
                    if ui.add(theme::primary(format!("{}  Erneut versuchen", icon::ARROWS_CLOCKWISE))).clicked() {
                        match r {
                            Some(r) => self.updater.install((**r).clone(), ui.ctx()),
                            None => self.updater.check(ui.ctx()),
                        }
                    }
                    close |= ui.add(theme::secondary("Schließen")).clicked();
                });
            }
            UpdateState::Available(rel) => {
                crate::dialog::footer(ui, |ui| {
                    let main = ui
                        .add_enabled(can_install(), theme::primary(format!("{}  Jetzt aktualisieren", icon::DOWNLOAD_SIMPLE)))
                        .on_disabled_hover_text(crate::tr("Entwicklungs-Build: bitte über Git aktualisieren"));
                    if main.clicked() {
                        self.updater.install((**rel).clone(), ui.ctx());
                    }
                    close |= ui.add(theme::secondary("Später")).clicked();
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        if ui.add(theme::ghost("Diese Version überspringen")).clicked() {
                            self.settings.skipped_version = Some(rel.version.to_string());
                            self.settings.save();
                            close = true;
                        }
                    });
                });
            }
            // the check runs again or Cutline is up to date by now
            UpdateState::Checking => progress_line(ui, None),
            UpdateState::Idle | UpdateState::UpToDate => close = true,
        }
        close
    }

    /// „Updates“ section in the settings dialog.
    pub fn updates_settings(&mut self, ui: &mut egui::Ui) {
        theme::row(ui, "Version", |ui| {
            ui.label(RichText::new(current_version()).monospace());
            if ui.add(theme::ghost(format!("{}  Was ist neu", icon::SPARKLE))).clicked() {
                self.updater.view = Some(View::News(embedded_notes()));
            }
        });
        let state = self.updater.get();
        theme::row(ui, "Status", |ui| {
            let (text, color) = match &state {
                UpdateState::Idle => (crate::tr("Noch nicht gesucht"), theme::TEXT_DIM()),
                UpdateState::Checking => (crate::tr("Suche nach Updates …"), theme::TEXT_DIM()),
                UpdateState::UpToDate => (crate::tr(format!("{}  Cutline ist aktuell", icon::CHECK_CIRCLE)), theme::TEXT()),
                UpdateState::Available(rel) => (
                    crate::tr(format!("{}  Version {} ist verfügbar", icon::ARROW_CIRCLE_UP, rel.version)),
                    theme::TEXT(),
                ),
                UpdateState::Downloading(_, done, total) => (progress_text(*done, *total, de()), theme::TEXT_DIM()),
                UpdateState::Installed(v) => (
                    crate::tr(format!("{}  Version {v} ist installiert", icon::CHECK_CIRCLE)),
                    theme::TEXT(),
                ),
                UpdateState::Error(e, _) => (format!("{}  {}", icon::WARNING_CIRCLE, crate::tr(e)), theme::WARN_TEXT()),
            };
            ui.add(egui::Label::new(RichText::new(text).color(color)).truncate());
            if let (Some(t), UpdateState::UpToDate | UpdateState::Available(_) | UpdateState::Error(..)) =
                (self.updater.checked(), &state)
            {
                ui.label(RichText::new(crate::home::ago(t)).size(theme::FS_UI).color(theme::TEXT_FAINT()));
            }
        });
        match &state {
            UpdateState::Checking => progress_line(ui, None),
            UpdateState::Downloading(_, done, total) => progress_line(ui, fraction(*done, *total)),
            _ => {}
        }
        if widgets::toggle_row(
            ui,
            "Beim Start automatisch nach Updates suchen",
            &mut self.settings.auto_update,
        ) {
            self.settings.save();
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| match &state {
            UpdateState::Available(_) | UpdateState::Downloading(..) => {
                if ui.add(theme::primary(format!("{}  Update ansehen", icon::ARROW_CIRCLE_UP))).clicked() {
                    self.updater.open_update();
                }
            }
            UpdateState::Installed(_) => {
                if ui.add(theme::primary(format!("{}  Jetzt neu starten", icon::ARROWS_CLOCKWISE))).clicked() {
                    self.restart_after_update(ui.ctx());
                }
            }
            _ => {
                let busy = matches!(state, UpdateState::Checking);
                if ui
                    .add_enabled(!busy, theme::secondary(format!("{}  Jetzt prüfen", icon::ARROWS_CLOCKWISE)))
                    .clicked()
                {
                    self.settings.skipped_version = None;
                    self.updater.check(ui.ctx());
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_search_keeps_the_raw_error_for_the_log() {
        let raw = "https://api.github.com/x: Connection Failed: tls handshake eof";
        let (st, line) = check_outcome(Err(raw.into()));
        assert!(matches!(&st, UpdateState::Error(m, None) if m == "Keine Verbindung"));
        assert!(line.contains("tls handshake eof"), "{line}");
        assert!(matches!(check_outcome(Ok(None)), (UpdateState::UpToDate, l) if l == "aktuell"));
    }

    #[test]
    fn sizes_and_progress_read_well() {
        assert_eq!(mb_text(44_354_765, true), "42,3 MB");
        assert_eq!(mb_text(44_354_765, false), "42.3 MB");
        assert_eq!(mb_text(0, true), "0,0 MB");
        let total = 100 << 20;
        assert_eq!(progress_text(25 << 20, total, true), "25 % · 25,0 / 100,0 MB");
        assert_eq!(progress_text((25 << 20) - 1, total, true), "24 % · 25,0 / 100,0 MB", "percent rounded down");
        assert_eq!(progress_text(0, total, false), "0 % · 0.0 / 100.0 MB");
        assert_eq!(progress_text(total, total, false), "100 % · 100.0 / 100.0 MB");
        assert_eq!(progress_text(total * 2, total, false), "100 % · 100.0 / 100.0 MB", "never above 100 %");
        assert_eq!(progress_text(3 << 20, 1, true), "3,0 MB", "size unknown");
        assert_eq!(fraction(5, 10), Some(0.5));
        assert_eq!(fraction(20, 10), Some(1.0));
        assert_eq!(fraction(5, 1), None);
        assert_eq!(fraction(0, 0), None);
    }

    #[test]
    fn dates_follow_the_language() {
        assert_eq!(date_text("2026-11-15", true), "15.11.2026");
        assert_eq!(date_text("2026-11-15", false), "2026-11-15");
        assert_eq!(date_text("", true), "");
        assert_eq!(date_text("bald", true), "bald");
    }

    #[test]
    fn update_shows_the_skipped_versions_from_the_release_note() {
        let mut rel = fake_release();
        rel.version = Version(0, 11, 1);
        let got: Vec<_> = release_notes(&rel, Version(0, 10, 0)).iter().filter_map(|n| n.version).collect();
        assert_eq!(got, [Version(0, 11, 1), Version(0, 11, 0)]);
        rel.notes = "**Full Changelog**: https://github.com/x/compare/v0.11.0...v0.11.1".into();
        assert!(release_notes(&rel, Version(0, 11, 0)).is_empty(), "raw text instead of a list");
        // embedded list: only versions up to our own, newest at the top, without „Unreleased“
        let all = embedded_notes();
        assert!(!all.is_empty());
        assert!(all.iter().all(|n| n.version.is_some_and(|v| v <= own_version())));
        assert!(all.windows(2).all(|w| w[0].version > w[1].version));
    }

    /// With `changes.json`, „Was ist neu“ comes from it (also older skipped versions),
    /// not from the release text; if the attachment is missing, the text applies.
    #[test]
    fn update_prefers_changes_json() {
        let mut rel = fake_release();
        rel.version = Version(0, 11, 1);
        rel.notes = "## 0.11.1\n### English\n**New**\n- aus dem Text\n".into();
        let from_text: Vec<_> = release_notes(&rel, Version(0, 9, 0)).iter().map(|n| n.groups(false)[0].entries[0].text.clone()).collect();
        assert_eq!(from_text, ["aus dem Text"]);
        rel.changes = cl_update::changelog::parse_json(
            r#"{"format": 1, "versions": [
                {"version": "0.11.1", "en": [{"group": "New", "items": [{"text": "aus changes.json"}]}]},
                {"version": "0.10.0", "de": [{"group": "Neu", "items": [{"text": "übersprungen"}]}]},
                {"version": "0.9.0", "en": [{"group": "New", "items": [{"text": "schon installiert"}]}]}]}"#,
        );
        let got: Vec<_> = release_notes(&rel, Version(0, 9, 0)).iter().map(|n| n.groups(false)[0].entries[0].text.clone()).collect();
        assert_eq!(got, ["aus changes.json", "übersprungen"]);
        assert!(check_outcome(Ok(Some(rel))).1.contains("changes.json"));
    }
}
