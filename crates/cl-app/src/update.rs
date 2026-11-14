//! Update-Prüfung im Hintergrund (GitHub Releases) und Installation per Knopfdruck.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
    Downloading(f32),
    /// Installiert, Neustart nötig
    Installed(Version),
    Error(String),
}

pub struct Updater {
    pub state: Arc<Mutex<UpdateState>>,
    cancel: Arc<AtomicBool>,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(UpdateState::Idle)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Entwicklungs-Builds (cargo run) werden nicht überschrieben.
pub fn can_install() -> bool {
    !cfg!(debug_assertions)
}

fn exe_dir() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|p| p.to_path_buf())
}

impl Updater {
    pub fn get(&self) -> UpdateState {
        self.state
            .lock()
            .map(|s| s.clone())
            .unwrap_or(UpdateState::Idle)
    }

    pub fn check(&self, ctx: &egui::Context) {
        if matches!(
            self.get(),
            UpdateState::Checking | UpdateState::Downloading(_)
        ) {
            return;
        }
        *self.state.lock().unwrap() = UpdateState::Checking;
        let (state, ctx) = (self.state.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = match cl_update::check(current_version()) {
                Ok(Some(rel)) => UpdateState::Available(Box::new(rel)),
                Ok(None) => UpdateState::UpToDate,
                Err(e) => UpdateState::Error(e),
            };
            *state.lock().unwrap() = r;
            ctx.request_repaint();
        });
    }

    pub fn install(&self, rel: Release, ctx: &egui::Context) {
        let Some(dir) = exe_dir() else { return };
        self.cancel.store(false, Ordering::Relaxed);
        *self.state.lock().unwrap() = UpdateState::Downloading(0.0);
        let (state, cancel, ctx) = (self.state.clone(), self.cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let tmp = std::env::temp_dir().join("cutline-update");
            let st = state.clone();
            let c2 = ctx.clone();
            let res = cl_update::download(
                &rel,
                &tmp,
                |p| {
                    *st.lock().unwrap() = UpdateState::Downloading(p);
                    c2.request_repaint();
                },
                &cancel,
            )
            .and_then(|zip| {
                let n = cl_update::install(&zip, &dir);
                let _ = std::fs::remove_file(&zip);
                n
            });
            *state.lock().unwrap() = match res {
                Ok(_) => UpdateState::Installed(rel.version),
                Err(e) => UpdateState::Error(e),
            };
            ctx.request_repaint();
        });
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Programm neu starten (nach der Installation).
pub fn restart(ctx: &egui::Context) {
    if let Ok(exe) = std::env::current_exe() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if std::process::Command::new(exe).args(args).spawn().is_ok() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// Beim Start: Reste alter Updates entfernen.
pub fn startup_cleanup() {
    if let Some(d) = exe_dir() {
        cl_update::cleanup_old(&d);
    }
}

impl App {
    /// Kleiner Hinweis in der Titelleiste (nur wenn es etwas zu tun gibt).
    pub fn update_badge(&mut self, ui: &mut egui::Ui) {
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
                    self.settings_section = crate::settings::Section::Updates;
                    self.settings_open = true;
                }
            }
            UpdateState::Downloading(p) => {
                ui.label(
                    RichText::new(crate::tr(format!(
                        "{}  Update {:.0} %",
                        icon::DOWNLOAD_SIMPLE,
                        p * 100.0
                    )))
                    .size(theme::FS_UI)
                    .color(theme::TEXT_DIM()),
                );
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

    /// Abschnitt „Updates“ im Einstellungsdialog.
    pub fn updates_settings(&mut self, ui: &mut egui::Ui) {
        theme::row(ui, "Version", |ui| {
            ui.label(RichText::new(crate::tr(current_version())).monospace())
        });
        theme::row(ui, "Quelle", |ui| {
            ui.label(
                RichText::new(crate::tr(format!("github.com/{}", cl_update::REPO)))
                    .color(theme::TEXT_DIM()),
            )
        });
        if widgets::toggle_row(
            ui,
            "Beim Start automatisch nach Updates suchen",
            &mut self.settings.auto_update,
        ) {
            self.settings.save();
        }
        ui.label(RichText::new(crate::tr("Dabei wird nur die Release-Liste auf GitHub abgefragt. Installiert wird erst nach deiner Bestätigung, jedes Paket wird per SHA-256 geprüft.")).small().color(theme::TEXT_DIM()));
        theme::section(ui, "Status");
        let state = self.updater.get();
        match &state {
            UpdateState::Idle => {
                ui.label(RichText::new(crate::tr("Noch nicht gesucht")).color(theme::TEXT_DIM()));
            }
            UpdateState::Checking => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().color(theme::ACCENT()));
                    ui.label(crate::tr("Suche nach Updates …"));
                });
            }
            UpdateState::UpToDate => {
                ui.label(
                    RichText::new(crate::tr(format!(
                        "{}  Cutline ist aktuell",
                        icon::CHECK_CIRCLE
                    )))
                    .color(theme::TEXT()),
                );
            }
            UpdateState::Error(e) => {
                ui.label(
                    RichText::new(crate::tr(format!("{}  {e}", icon::WARNING_CIRCLE)))
                        .color(theme::TEXT_DIM()),
                );
            }
            UpdateState::Available(rel) => {
                ui.label(
                    RichText::new(crate::tr(format!(
                        "{}  Version {} ist verfügbar",
                        icon::ARROW_CIRCLE_UP,
                        rel.version
                    )))
                    .strong(),
                );
                if rel.asset_size > 0 {
                    ui.label(
                        RichText::new(crate::tr(format!(
                            "{} · {:.1} MB",
                            rel.name,
                            rel.asset_size as f64 / 1_048_576.0
                        )))
                        .small()
                        .color(theme::TEXT_DIM()),
                    );
                }
                if !rel.notes.trim().is_empty() {
                    ui.add_space(4.0);
                    egui::Frame::default()
                        .fill(theme::BG_INPUT())
                        .corner_radius(theme::R_CARD)
                        .inner_margin(theme::PAD_CARD_L)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical()
                                .id_salt("notes")
                                .max_height(140.0)
                                .show(ui, |ui| {
                                    ui.label(RichText::new(crate::tr(rel.notes.trim())).small());
                                });
                        });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if can_install() {
                        if ui
                            .add(theme::primary(format!(
                                "{}  Herunterladen und installieren",
                                icon::DOWNLOAD_SIMPLE
                            )))
                            .clicked()
                        {
                            self.updater.install((**rel).clone(), ui.ctx());
                        }
                    } else {
                        ui.label(
                            RichText::new(crate::tr(
                                "Entwicklungs-Build: bitte über Git aktualisieren",
                            ))
                            .small()
                            .color(theme::TEXT_DIM()),
                        );
                    }
                    if ui.add(theme::secondary("Auf GitHub ansehen")).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(&rel.page));
                    }
                    if ui.add(theme::ghost("Diese Version überspringen")).clicked() {
                        self.settings.skipped_version = Some(rel.version.to_string());
                        self.settings.save();
                    }
                });
            }
            UpdateState::Downloading(p) => {
                ui.add(
                    egui::ProgressBar::new(*p).fill(theme::ACCENT())
                        .show_percentage()
                        .desired_height(6.0),
                );
                if ui.add(theme::secondary("Abbrechen")).clicked() {
                    self.updater.cancel();
                }
            }
            UpdateState::Installed(v) => {
                ui.label(
                    RichText::new(crate::tr(format!(
                        "{}  Version {v} ist installiert",
                        icon::CHECK_CIRCLE
                    )))
                    .color(theme::TEXT()),
                );
                if ui
                    .add(theme::primary(format!(
                        "{}  Jetzt neu starten",
                        icon::ARROWS_CLOCKWISE
                    )))
                    .clicked()
                {
                    self.restart_after_update(ui.ctx());
                }
            }
        }
        if !matches!(
            state,
            UpdateState::Checking | UpdateState::Downloading(_) | UpdateState::Installed(_)
        ) {
            ui.add_space(8.0);
            if ui
                .add(theme::secondary(format!(
                    "{}  Jetzt nach Updates suchen",
                    icon::ARROWS_CLOCKWISE
                )))
                .clicked()
            {
                self.settings.skipped_version = None;
                self.updater.check(ui.ctx());
            }
        }
    }
}
