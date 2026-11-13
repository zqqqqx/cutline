//! App-Einstellungen (dauerhaft in `settings.json`) und der Einstellungsdialog.
//! Der MCP-Server lebt nur hier: aktivieren, Port, Zugriff, Token, Client-Konfiguration.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use eframe::egui::{self, vec2, RichText};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::paths;
use crate::theme::{self, icon};
use crate::widgets::{self, Slider};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub lang: String,
    pub ui_scale: f32,
    /// Startseite beim Programmstart zeigen
    pub start_on_home: bool,
    pub image_duration: f32,
    pub text_duration: f32,
    pub transition_duration: f32,
    pub anim_duration: f32,
    /// Vorschauauflösung: 1 = voll, 2 = halb, 4 = viertel
    pub preview_div: u32,
    pub autosave: bool,
    pub mcp_enabled: bool,
    pub mcp_port: u16,
    pub mcp_read_only: bool,
    /// Zuletzt geöffnete Projekte (auch außerhalb des Projektordners)
    pub recent: Vec<PathBuf>,
    /// Beim Start auf GitHub nach Updates suchen
    pub auto_update: bool,
    /// Vom Benutzer übersprungene Version
    pub skipped_version: Option<String>,
    /// Plugin-ID → aktiv (überschreibt `default_enabled` des Manifests)
    pub plugin_states: std::collections::BTreeMap<String, bool>,
    /// Import aus anderen Schnittprogrammen
    pub import_capcut: bool,
    pub import_fcpxml: bool,
    /// Eigene Tastenkürzel: Aktion → Kombination (leer = keine)
    pub shortcuts: std::collections::BTreeMap<String, String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            lang: cl_i18n::Lang::from_locale(&std::env::var("LANG").unwrap_or_else(|_| "de".into())).code().into(),
            ui_scale: 1.0,
            start_on_home: true,
            image_duration: 5.0,
            text_duration: 3.0,
            transition_duration: 0.5,
            anim_duration: 0.5,
            preview_div: 1,
            autosave: true,
            mcp_enabled: false,
            mcp_port: 7316,
            mcp_read_only: false,
            recent: Vec::new(),
            auto_update: true,
            skipped_version: None,
            plugin_states: Default::default(),
            import_capcut: true,
            import_fcpxml: true,
            shortcuts: Default::default(),
        }
    }
}

impl AppSettings {
    fn path() -> PathBuf {
        paths::data_dir().join("settings.json")
    }
    pub fn load() -> Self {
        let mut s: AppSettings = std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // Übernahme aus Phase 1 (eigene Datei für die Oberflächengröße)
        if !Self::path().is_file() {
            s.ui_scale = crate::chrome::load_ui_scale();
        }
        if let Some(v) = std::env::var("CUTLINE_UI_SCALE").ok().and_then(|v| v.parse::<f32>().ok()) {
            s.ui_scale = v;
        }
        s.ui_scale = s.ui_scale.clamp(0.6, 2.0);
        s
    }
    pub fn save(&self) {
        let _ = std::fs::create_dir_all(paths::data_dir());
        let _ = std::fs::write(Self::path(), serde_json::to_string_pretty(self).unwrap_or_default());
    }
    pub fn lang(&self) -> cl_i18n::Lang {
        cl_i18n::Lang::ALL.into_iter().find(|l| l.code() == self.lang).unwrap_or(cl_i18n::Lang::De)
    }
    pub fn add_recent(&mut self, p: &std::path::Path) {
        self.recent.retain(|r| r != p);
        self.recent.insert(0, p.to_path_buf());
        self.recent.truncate(30);
    }
}

/// Laufender MCP-Server.
pub struct McpRunning {
    pub port: u16,
    pub token: String,
    pub read_only: bool,
    stop: Arc<AtomicBool>,
}

impl Drop for McpRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    General,
    Editing,
    Performance,
    Mcp,
    Shortcuts,
    Models,
    Updates,
    Integrations,
    Import,
    About,
}

const SECTIONS: [(Section, &str, &str); 10] = [
    (Section::General, icon::GEAR_SIX, "Allgemein"),
    (Section::Editing, icon::SCISSORS, "Bearbeiten"),
    (Section::Performance, icon::GAUGE, "Leistung"),
    (Section::Mcp, icon::PLUGS_CONNECTED, "MCP-Schnittstelle"),
    (Section::Shortcuts, icon::KEYBOARD, "Tastenkürzel"),
    (Section::Models, icon::CPU, "Modelle (lokale KI)"),
    (Section::Updates, icon::ARROW_CIRCLE_UP, "Updates"),
    (Section::Integrations, icon::PUZZLE_PIECE, "Integrationen"),
    (Section::Import, icon::DOWNLOAD_SIMPLE, "Import"),
    (Section::About, icon::INFO, "Über Cutline"),
];


impl App {
    pub fn start_mcp(&mut self) {
        self.mcp = None;
        let token = cl_mcp::new_token();
        let mut srv = cl_mcp::McpServer::new(self.doc.clone(), Some(self.fonts.clone()));
        srv.read_only = self.settings.mcp_read_only;
        let stop = Arc::new(AtomicBool::new(false));
        match srv.serve_http_until(self.settings.mcp_port, token.clone(), stop.clone()) {
            Ok((addr, _)) => {
                let info = serde_json::json!({ "url": format!("http://127.0.0.1:{}/mcp", addr.port()), "token": token, "read_only": self.settings.mcp_read_only });
                let _ = std::fs::create_dir_all(paths::data_dir());
                let _ = std::fs::write(paths::data_dir().join("mcp.json"), serde_json::to_string_pretty(&info).unwrap_or_default());
                self.mcp = Some(McpRunning { port: addr.port(), token, read_only: self.settings.mcp_read_only, stop });
            }
            Err(e) => {
                self.settings.mcp_enabled = false;
                self.toast_err(format!("MCP: {e}"));
            }
        }
    }

    pub fn stop_mcp(&mut self) {
        self.mcp = None;
        let _ = std::fs::remove_file(paths::data_dir().join("mcp.json"));
    }

    pub fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let modal = egui::Modal::new(egui::Id::new("settings")).frame(egui::Frame::window(&ctx.style()).inner_margin(0)).show(ctx, |ui| {
            ui.set_width(700.0);
            ui.set_height(470.0);
            ui.horizontal_top(|ui| {
                // Navigation
                egui::Frame::default().fill(theme::BG_APP).corner_radius(egui::CornerRadius { nw: 10, sw: 10, ne: 0, se: 0 }).inner_margin(12).show(ui, |ui| ui.vertical(|ui| {
                    ui.set_width(170.0);
                    ui.set_height(446.0);
                    ui.label(RichText::new(crate::tr("Einstellungen")).strong().size(14.0));
                    ui.add_space(12.0);
                    for (s, g, label) in SECTIONS {
                        let active = self.settings_section == s;
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), egui::Sense::click());
                        if active {
                            ui.painter().rect_filled(r, 6.0, theme::BG_RAISED);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(r, 6.0, theme::BG_CARD);
                        }
                        let col = if active { theme::ACCENT } else { theme::TEXT_DIM };
                        ui.painter().text(r.left_center() + vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, crate::tr(g), egui::FontId::proportional(14.0), col);
                        ui.painter().text(r.left_center() + vec2(32.0, 0.0), egui::Align2::LEFT_CENTER, crate::tr(label), egui::FontId::proportional(12.0), col);
                        if resp.clicked() {
                            self.settings_section = s;
                        }
                    }
                }));
                // Inhalt
                ui.vertical(|ui| {
                    // feste Breite, sonst springt das Fenster beim Bereichswechsel
                    ui.set_width(514.0);
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let title = SECTIONS.iter().find(|s| s.0 == self.settings_section).map(|s| s.2).unwrap_or("");
                        ui.label(RichText::new(crate::tr(title)).strong().size(14.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(10.0);
                            if theme::icon_button(ui, icon::X, "Schließen (Esc)", true, false).clicked() {
                                self.settings_open = false;
                            }
                        });
                    });
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical().id_salt("settings-body").auto_shrink([false, true]).max_height(410.0).show(ui, |ui| {
                        egui::Frame::default().inner_margin(egui::Margin { left: 8, right: 16, top: 0, bottom: 8 }).show(ui, |ui| {
                            ui.set_width(490.0);
                            self.settings_body(ui);
                        });
                    });
                });
            });
        });
        if modal.should_close() {
            self.settings_open = false;
        }
        if !self.settings_open {
            self.settings.save();
        }
    }

    fn settings_body(&mut self, ui: &mut egui::Ui) {
        match self.settings_section {
            Section::General => {
                theme::row(ui, "Sprache", |ui| {
                    egui::ComboBox::from_id_salt("set-lang").selected_text(crate::tr(self.i18n.lang().native_name())).show_ui(ui, |ui| {
                        for l in cl_i18n::Lang::ALL {
                            if ui.selectable_label(self.i18n.lang() == l, crate::tr(l.native_name())).clicked() {
                                self.i18n.set_lang(l);
                                self.settings.lang = l.code().into();
                            }
                        }
                    });
                });
                theme::row(ui, "Oberfläche", |ui| {
                    let mut v = self.settings.ui_scale;
                    let e = Slider::new(&mut v, 0.6..=2.0).percent().default(1.0).show(ui);
                    if e.commit {
                        let ctx = ui.ctx().clone();
                        self.set_ui_scale(&ctx, v);
                    }
                });
                widgets::toggle_row(ui, "Beim Start die Projektübersicht zeigen", &mut self.settings.start_on_home);
                widgets::toggle_row(ui, "Automatisch speichern", &mut self.settings.autosave);
                theme::row(ui, "Projektordner", |ui| {
                    let d = paths::data_dir().join("Projects");
                    let full = d.display().to_string();
                    let n = full.chars().count();
                    let short = if n > 52 { format!("…{}", full.chars().skip(n - 50).collect::<String>()) } else { full.clone() };
                    if ui.add(theme::ghost(format!("{}  {short}", icon::FOLDER_OPEN))).on_hover_text(format!("{}
{full}", crate::tr("Im Explorer öffnen"))).clicked() {
                        let _ = std::fs::create_dir_all(&d);
                        crate::home::reveal(&d);
                    }
                });
            }
            Section::Editing => {
                theme::row(ui, "Bilddauer", |ui| Slider::new(&mut self.settings.image_duration, 0.5..=30.0).suffix(" s").decimals(1).default(5.0).show(ui));
                theme::row(ui, "Textdauer", |ui| Slider::new(&mut self.settings.text_duration, 0.5..=30.0).suffix(" s").decimals(1).default(3.0).show(ui));
                theme::row(ui, "Übergang", |ui| Slider::new(&mut self.settings.transition_duration, 0.1..=3.0).suffix(" s").decimals(1).default(0.5).show(ui));
                theme::row(ui, "Animation", |ui| Slider::new(&mut self.settings.anim_duration, 0.1..=3.0).suffix(" s").decimals(1).default(0.5).show(ui));
                ui.add_space(6.0);
                let mut s = self.project.settings.clone();
                let a = widgets::toggle_row(ui, "Hauptspur-Magnet (P)", &mut s.main_track_magnet);
                let b = widgets::toggle_row(ui, "Einrasten (N)", &mut s.snapping);
                if a || b {
                    self.apply(cl_core::EditOp::SetSettings { settings: s });
                }
            }
            Section::Performance => {
                theme::row(ui, "Vorschau", |ui| {
                    let mut i = match self.settings.preview_div {
                        1 => 0,
                        2 => 1,
                        _ => 2,
                    };
                    if widgets::segmented(ui, &["Voll", "Halb", "Viertel"], &mut i) {
                        self.settings.preview_div = [1, 2, 4][i];
                        self.invalidate_preview();
                    }
                });
                ui.label(RichText::new(crate::tr("Geringere Vorschauauflösung macht die Wiedergabe mit Effekten flüssiger. Der Export nutzt immer die volle Auflösung.")).small().color(theme::TEXT_DIM));
                ui.add_space(8.0);
                theme::row(ui, "Effekt-Cache", |ui| {
                    if ui.add(theme::secondary(format!("{}  Vorschaubilder neu erzeugen", icon::ARROWS_CLOCKWISE))).clicked() {
                        let _ = std::fs::remove_dir_all(paths::data_dir().join("cache"));
                        self.fx_thumbs.clear();
                        self.fx_thumbs_requested.clear();
                    }
                });
            }
            Section::Mcp => {
                ui.label(RichText::new(crate::tr("Über das Model Context Protocol können lokale KI-Assistenten und Skripte das offene Projekt lesen und bearbeiten. Jede Änderung landet im Verlauf und lässt sich rückgängig machen. Der Server ist nur auf diesem Rechner erreichbar (127.0.0.1) und verlangt ein Token.")).small().color(theme::TEXT_DIM));
                ui.add_space(10.0);
                let mut on = self.settings.mcp_enabled;
                if widgets::toggle_row(ui, "MCP-Server aktivieren", &mut on) {
                    self.settings.mcp_enabled = on;
                    if on {
                        self.start_mcp();
                    } else {
                        self.stop_mcp();
                    }
                }
                let running = self.mcp.as_ref().map(|m| (m.port, m.token.clone(), m.read_only));
                ui.add_enabled_ui(running.is_none(), |ui| {
                    theme::row(ui, "Port", |ui| ui.add(egui::DragValue::new(&mut self.settings.mcp_port).range(1024..=65535)));
                    widgets::toggle_row(ui, "Nur lesen (keine Änderungen erlauben)", &mut self.settings.mcp_read_only);
                });
                if let Some((port, token, ro)) = running {
                    theme::section(ui, "Verbindung");
                    theme::row(ui, "Status", |ui| ui.label(RichText::new(crate::tr(format!("{}  aktiv auf Port {port}{}", icon::CIRCLE, if ro { " · nur lesen" } else { "" }))).color(theme::ACCENT)));
                    theme::row(ui, "Adresse", |ui| ui.monospace(format!("http://127.0.0.1:{port}/mcp")));
                    theme::row(ui, "Token", |ui| {
                        ui.monospace(format!("{}…", &token[..10.min(token.len())]));
                        if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, "Neues Token erzeugen (alte Clients verlieren den Zugriff)", true, false).clicked() {
                            self.start_mcp();
                        }
                    });
                    ui.add_space(6.0);
                    if ui.add(theme::primary(format!("{}  Client-Konfiguration kopieren", icon::COPY))).clicked() {
                        let cfg = serde_json::json!({ "mcpServers": { "cutline": {
                            "type": "http", "url": format!("http://127.0.0.1:{port}/mcp"),
                            "headers": { "Authorization": format!("Bearer {token}") } } } });
                        ui.ctx().copy_text(serde_json::to_string_pretty(&cfg).unwrap_or_default());
                        self.toast("MCP-Konfiguration kopiert");
                    }
                    ui.label(RichText::new(crate::tr(format!("Auch gespeichert in {}", paths::data_dir().join("mcp.json").display()))).small().color(theme::TEXT_FAINT));
                } else if self.settings.mcp_enabled {
                    ui.label(RichText::new(crate::tr("Server startet …")).small().color(theme::TEXT_DIM));
                }
                theme::section(ui, "Ohne laufende App (stdio)");
                ui.label(RichText::new(crate::tr("cutline --mcp projekt.cutline")).monospace().color(theme::TEXT_DIM));
                ui.label(RichText::new(crate::tr("Werkzeuge: Projekt lesen, Medien importieren, Clips einfügen, teilen, trimmen, löschen, Format setzen, beliebige EditOps, Rückgängig/Wiederholen.")).small().color(theme::TEXT_DIM));
            }
            Section::Shortcuts => self.shortcuts_settings(ui),
            Section::Updates => self.updates_settings(ui),
            Section::Integrations => self.integrations_settings(ui),
            Section::Import => self.import_settings(ui),
            Section::Models => self.models_settings(ui),
            Section::About => {
                ui.horizontal(|ui| theme::wordmark(ui, 34.0));
                ui.add_space(8.0);
                ui.label(crate::tr(format!("Version {} · MPL-2.0", env!("CARGO_PKG_VERSION"))));
                ui.label(RichText::new(crate::tr("Open-Source-Videoeditor in Rust. Lokal, ohne Konto, ohne Abo, ohne Werbung, ohne generative KI.")).color(theme::TEXT_DIM));
                ui.add_space(8.0);
                let (e, f, t) = (cl_fx::EFFECTS.len(), cl_fx::FILTERS.len(), cl_fx::TRANSITIONS.len());
                ui.label(RichText::new(crate::tr(format!(
                    "{e} Effekte · {f} Filter · {t} Übergänge · {} Sticker · {} Soundeffekte · {} Textvorlagen",
                    cl_fx::stickers::STICKERS.len(),
                    cl_fx::sfx::SFX.len(),
                    cl_fx::text::TEMPLATES.len()
                )))
                .small()
                .color(theme::TEXT_DIM));
                ui.label(RichText::new(crate::tr(format!("{} Plugins geladen", self.plugins.plugins.len()))).small().color(theme::TEXT_DIM));
            }
        }
    }
}
