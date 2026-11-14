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
    /// Proxys für große Videos in der Vorschau
    pub use_proxies: bool,
    /// Einführung abgeschlossen oder übersprungen
    pub onboarding_done: bool,
    /// Antworten auf „Wofür nutzt du Cutline?“
    pub use_cases: Vec<String>,
    /// Design: „system“, „dark“, „darkgray“, „gray“, „light“
    pub theme: String,
    /// Kleine Animationen (Hover, Einblenden, Export)
    pub animations: bool,
    /// Eigener Projektordner (`None` = im Datenordner)
    pub projects_dir: Option<PathBuf>,
    /// Automatische Sicherung: alle n Minuten, höchstens m Stück je Projekt
    pub backup_minutes: u32,
    pub backup_keep: usize,
    /// Neue Projekte: Bildrate und Umgang mit Tonkanälen
    pub new_fps: u32,
    pub new_channels: cl_core::Channels,
    /// Höhe der Proxys (540, 720, 1080)
    pub proxy_h: u32,
    /// Downloads aus Plugins (Epidemic Sound, Pexels) auch in die Bibliothek legen
    pub lib_downloads: bool,
    /// Gewählter Untertitel-Stil (`captions::PRESETS`)
    pub caption_preset: String,
    /// Hinweiston, wenn ein Export fertig ist
    pub export_sound: bool,
    /// Cache-Dateien, die älter sind als so viele Tage, beim Start löschen (0 = nie)
    pub cache_days: u32,
    /// Beim Ziehen des Playheads kurz den Ton an der Stelle abspielen
    pub scrub_audio: bool,
    /// Ausgabegerät für die Wiedergabe (`None` = Systemstandard)
    pub audio_device: Option<String>,
    /// Aktuelles Layout und eigene Layout-Profile
    pub layout: crate::layout::LayoutProfile,
    pub layouts: Vec<crate::layout::LayoutProfile>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            lang: cl_i18n::Lang::from_locale(
                &std::env::var("LANG").unwrap_or_else(|_| "de".into()),
            )
            .code()
            .into(),
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
            use_proxies: true,
            onboarding_done: false,
            use_cases: Vec::new(),
            theme: "dark".into(),
            animations: true,
            projects_dir: None,
            backup_minutes: 5,
            backup_keep: 30,
            new_fps: 30,
            new_channels: Default::default(),
            proxy_h: 720,
            lib_downloads: true,
            caption_preset: "classic".into(),
            export_sound: true,
            cache_days: 0,
            scrub_audio: false,
            audio_device: None,
            layout: Default::default(),
            layouts: Vec::new(),
        }
    }
}

impl AppSettings {
    fn path() -> PathBuf {
        paths::data_dir().join("settings.json")
    }
    pub fn load() -> Self {
        let mut s: AppSettings = paths::read_json_or_quarantine(&Self::path());
        // Übernahme aus Phase 1 (eigene Datei für die Oberflächengröße)
        if !Self::path().is_file() {
            s.ui_scale = crate::chrome::load_ui_scale();
        }
        if let Some(v) = std::env::var("CUTLINE_UI_SCALE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            s.ui_scale = v;
        }
        s.ui_scale = s.ui_scale.clamp(0.6, 2.0);
        s.backup_minutes = s.backup_minutes.clamp(1, 120);
        s.backup_keep = s.backup_keep.clamp(1, 500);
        paths::set_projects_dir(s.projects_dir.clone());
        crate::theme::set_animations(s.animations);
        crate::audio::set_output_device(s.audio_device.clone());
        if s.cache_days > 0 {
            let days = s.cache_days;
            std::thread::spawn(move || {
                let age = std::time::Duration::from_secs(days as u64 * 86_400);
                paths::remove_older(&paths::data_dir().join("cache"), age, paths::CACHE_KEEP);
                paths::remove_older(&paths::data_dir().join("thumbs"), age, &[]);
            });
        }
        s
    }
    pub fn save(&self) {
        let _ = std::fs::create_dir_all(paths::data_dir());
        let _ = paths::write_atomic(
            &Self::path(),
            serde_json::to_string_pretty(self).unwrap_or_default(),
        );
    }
    pub fn lang(&self) -> cl_i18n::Lang {
        cl_i18n::Lang::ALL
            .into_iter()
            .find(|l| l.code() == self.lang)
            .unwrap_or(cl_i18n::Lang::De)
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
    Library,
    Projects,
    Audio,
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

const SECTIONS: [(Section, &str, &str); 13] = [
    (Section::Library, icon::BOOKMARKS, "Bibliothek"),
    (Section::General, icon::GEAR_SIX, "Allgemein"),
    (Section::Projects, icon::FOLDERS, "Projekte"),
    (Section::Editing, icon::SCISSORS, "Bearbeiten"),
    (Section::Audio, icon::SPEAKER_HIGH, "Audio"),
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
                let _ = std::fs::write(
                    paths::data_dir().join("mcp.json"),
                    serde_json::to_string_pretty(&info).unwrap_or_default(),
                );
                self.mcp = Some(McpRunning {
                    port: addr.port(),
                    token,
                    read_only: self.settings.mcp_read_only,
                    stop,
                });
            }
            Err(e) => {
                self.settings.mcp_enabled = false;
                self.settings_error = Some(format!("MCP: {e}"));
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
        let modal = egui::Modal::new(egui::Id::new("settings"))
            .frame(egui::Frame::window(&ctx.style()).inner_margin(0))
            .show(ctx, |ui| {
                ui.set_width(700.0);
                ui.set_height(470.0);
                ui.horizontal_top(|ui| {
                    // Navigation
                    egui::Frame::default()
                        .fill(theme::BG_APP())
                        .corner_radius(egui::CornerRadius {
                            nw: 10,
                            sw: 10,
                            ne: 0,
                            se: 0,
                        })
                        .inner_margin(12)
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(170.0);
                                ui.set_height(446.0);
                                ui.label(
                                    RichText::new(crate::tr("Einstellungen"))
                                        .strong()
                                        .size(14.0),
                                );
                                ui.add_space(12.0);
                                for (s, g, label) in SECTIONS {
                                    let active = self.settings_section == s;
                                    let (r, resp) = ui.allocate_exact_size(
                                        vec2(ui.available_width(), 28.0),
                                        egui::Sense::click(),
                                    );
                                    if active {
                                        ui.painter().rect_filled(r, 6.0, theme::BG_RAISED());
                                    } else if resp.hovered() {
                                        ui.painter().rect_filled(r, 6.0, theme::BG_CARD());
                                    }
                                    let col = if active {
                                        theme::ACCENT()
                                    } else {
                                        theme::TEXT_DIM()
                                    };
                                    ui.painter().text(
                                        r.left_center() + vec2(10.0, 0.0),
                                        egui::Align2::LEFT_CENTER,
                                        crate::tr(g),
                                        egui::FontId::proportional(14.0),
                                        col,
                                    );
                                    ui.painter().text(
                                        r.left_center() + vec2(32.0, 0.0),
                                        egui::Align2::LEFT_CENTER,
                                        crate::tr(label),
                                        egui::FontId::proportional(12.0),
                                        col,
                                    );
                                    if resp.clicked() {
                                        self.settings_section = s;
                                    }
                                }
                            })
                        });
                    // Inhalt
                    ui.vertical(|ui| {
                        // feste Breite, sonst springt das Fenster beim Bereichswechsel
                        ui.set_width(514.0);
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            let title = SECTIONS
                                .iter()
                                .find(|s| s.0 == self.settings_section)
                                .map(|s| s.2)
                                .unwrap_or("");
                            ui.label(RichText::new(crate::tr(title)).strong().size(14.0));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_space(10.0);
                                    if theme::icon_button(
                                        ui,
                                        icon::X,
                                        "Schließen (Esc)",
                                        true,
                                        false,
                                    )
                                    .clicked()
                                    {
                                        self.settings_open = false;
                                    }
                                },
                            );
                        });
                        ui.add_space(8.0);
                        // Feste Fläche in einem eigenen Ui: breiter oder kurzer Inhalt ändert
                        // die Fenstergröße nicht mehr
                        let area = egui::Rect::from_min_size(ui.cursor().min, vec2(514.0, 410.0));
                        ui.allocate_rect(area, egui::Sense::hover());
                        let mut body = ui.new_child(egui::UiBuilder::new().max_rect(area));
                        body.set_clip_rect(area.intersect(ui.clip_rect()));
                        let ui = &mut body;
                        egui::ScrollArea::vertical()
                            .id_salt("settings-body")
                            .auto_shrink([false, false])
                            .max_height(410.0)
                            .show(ui, |ui| {
                                egui::Frame::default()
                                    .inner_margin(egui::Margin {
                                        left: 8,
                                        right: 16,
                                        top: 0,
                                        bottom: 8,
                                    })
                                    .show(ui, |ui| {
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
        if let Some(e) = self.settings_error.clone() {
            egui::Frame::default()
                .fill(theme::DANGER.linear_multiply(0.15))
                .corner_radius(6)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}  {e}", icon::WARNING_CIRCLE)).color(theme::DANGER));
                        if theme::icon_button(ui, icon::X, "", true, false).clicked() {
                            self.settings_error = None;
                        }
                    });
                });
            ui.add_space(6.0);
        }
        match self.settings_section {
            Section::General => {
                theme::row(ui, "Sprache", |ui| {
                    egui::ComboBox::from_id_salt("set-lang")
                        .selected_text(crate::tr(self.i18n.lang().native_name()))
                        .show_ui(ui, |ui| {
                            for l in cl_i18n::Lang::ALL {
                                if ui
                                    .selectable_label(
                                        self.i18n.lang() == l,
                                        crate::tr(l.native_name()),
                                    )
                                    .clicked()
                                {
                                    self.i18n.set_lang(l);
                                    self.settings.lang = l.code().into();
                                }
                            }
                        });
                });
                theme::row(ui, "Design", |ui| {
                    ui.horizontal_wrapped(|ui| {
                        let mut opts: Vec<(&str, &str, Option<&theme::Palette>)> =
                            vec![("system", "System", None)];
                        opts.extend(theme::PALETTES.iter().map(|p| (p.id, p.name, Some(p))));
                        for (id, name, pal) in opts {
                            let on = self.settings.theme == id;
                            let (r, resp) = ui.allocate_exact_size(egui::vec2(66.0, 50.0), egui::Sense::click());
                            let pnt = ui.painter();
                            let sw = egui::Rect::from_min_size(r.min, egui::vec2(66.0, 34.0));
                            match pal {
                                Some(p) => {
                                    pnt.rect_filled(sw, 6.0, p.bg_app);
                                    pnt.rect_filled(sw.shrink2(egui::vec2(8.0, 7.0)), 3.0, p.bg_card);
                                    pnt.rect_filled(egui::Rect::from_min_size(sw.min + egui::vec2(14.0, 12.0), egui::vec2(26.0, 4.0)), 2.0, p.text);
                                    pnt.rect_filled(egui::Rect::from_min_size(sw.min + egui::vec2(14.0, 19.0), egui::vec2(18.0, 3.0)), 1.5, p.text_dim);
                                }
                                None => {
                                    let (a, b) = (&theme::PALETTES[0], &theme::PALETTES[3]);
                                    pnt.rect_filled(sw, 6.0, b.bg_app);
                                    let left = egui::Rect::from_min_max(sw.min, egui::pos2(sw.center().x, sw.bottom()));
                                    pnt.with_clip_rect(left).rect_filled(sw, 6.0, a.bg_app);
                                    pnt.text(sw.center(), egui::Align2::CENTER_CENTER, icon::CIRCLE_HALF, egui::FontId::proportional(15.0), egui::Color32::from_gray(140));
                                }
                            }
                            let stroke = if on {
                                egui::Stroke::new(2.0_f32, theme::ACCENT())
                            } else if resp.hovered() {
                                egui::Stroke::new(1.0_f32, theme::TEXT_DIM())
                            } else {
                                egui::Stroke::new(1.0_f32, theme::BORDER())
                            };
                            pnt.rect_stroke(sw, 6.0, stroke, egui::StrokeKind::Outside);
                            pnt.text(
                                egui::pos2(sw.center().x, r.bottom() - 6.0),
                                egui::Align2::CENTER_CENTER,
                                crate::tr(name),
                                egui::FontId::proportional(10.5),
                                if on { theme::TEXT() } else { theme::TEXT_DIM() },
                            );
                            if resp.clicked() {
                                self.settings.theme = id.to_string();
                                theme::select(ui.ctx(), id);
                                self.settings.save();
                            }
                        }
                    });
                });
                if widgets::toggle_row(ui, "Animationen", &mut self.settings.animations) {
                    theme::set_animations(self.settings.animations);
                    let ctx = ui.ctx().clone();
                    theme::apply(&ctx);
                }
                theme::row(ui, "Oberfläche", |ui| {
                    let mut v = self.settings.ui_scale;
                    let e = Slider::new(&mut v, 0.6..=2.0)
                        .percent()
                        .default(1.0)
                        .show(ui);
                    if e.commit {
                        let ctx = ui.ctx().clone();
                        self.set_ui_scale(&ctx, v);
                    }
                });
                widgets::toggle_row(
                    ui,
                    "Beim Start die Projektübersicht zeigen",
                    &mut self.settings.start_on_home,
                );
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::secondary(format!(
                            "{}  Einführung wiederholen",
                            icon::HAND_WAVING
                        )))
                        .clicked()
                    {
                        self.settings_open = false;
                        self.start_onboarding();
                    }
                    if ui
                        .add(theme::secondary(format!("{}  Rundgang", icon::COMPASS)))
                        .clicked()
                    {
                        self.settings_open = false;
                        self.start_tour();
                    }
                });
            }
            Section::Projects => {
                theme::row(ui, "Projektordner", |ui| {
                    let d = paths::projects_dir();
                    let full = d.display().to_string();
                    let n = full.chars().count();
                    let short = if n > 40 {
                        format!("…{}", full.chars().skip(n - 38).collect::<String>())
                    } else {
                        full.clone()
                    };
                    if ui
                        .add(theme::ghost(format!("{}  {short}", icon::FOLDER_OPEN)))
                        .on_hover_text(format!("{}\n{full}", crate::tr("Im Explorer öffnen")))
                        .clicked()
                    {
                        let _ = std::fs::create_dir_all(&d);
                        crate::home::reveal(&d);
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(theme::LABEL_W + 6.0);
                    if ui.add(theme::secondary(format!("{}  Ändern …", icon::FOLDER_SIMPLE_PLUS))).clicked() {
                        if let Some(new) = crate::dialog().set_title(crate::tr("Neuer Projektordner")).pick_folder() {
                            self.change_projects_dir(new);
                        }
                    }
                    if self.settings.projects_dir.is_some()
                        && ui.add(theme::ghost("Standard")).on_hover_text(crate::tr("Zurück in den Datenordner")).clicked()
                    {
                        self.change_projects_dir(paths::data_dir().join("Projects"));
                    }
                });
                ui.label(RichText::new(crate::tr("Vorhandene Projekte werden mit verschoben.")).small().color(theme::TEXT_FAINT()));
                theme::section(ui, "Speichern");
                widgets::toggle_row(ui, "Automatisch speichern", &mut self.settings.autosave);
                theme::row(ui, "Sicherung alle", |ui| {
                    let mut v = self.settings.backup_minutes as f32;
                    if Slider::new(&mut v, 1.0..=60.0).suffix(" min").decimals(0).default(5.0).show(ui).changed {
                        self.settings.backup_minutes = v.round() as u32;
                    }
                });
                theme::row(ui, "Sicherungen behalten", |ui| {
                    let mut v = self.settings.backup_keep as f32;
                    if Slider::new(&mut v, 3.0..=100.0).decimals(0).default(30.0).show(ui).changed {
                        self.settings.backup_keep = v.round() as usize;
                    }
                });
                theme::section(ui, "Neue Projekte");
                theme::row(ui, "Bildrate", |ui| {
                    let opts = [24u32, 25, 30, 50, 60];
                    let names: Vec<String> = opts.iter().map(|f| format!("{f}")).collect();
                    let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                    let mut i = opts.iter().position(|f| *f == self.settings.new_fps).unwrap_or(2);
                    if widgets::segmented(ui, &names, &mut i) {
                        self.settings.new_fps = opts[i];
                    }
                });
            }
            Section::Library => {
                ui.label(RichText::new(crate::tr("Verknüpfte Ordner erscheinen in allen Projekten (Medien → Bibliothek, Sticker, Audio → Sounds). Die Dateien bleiben, wo sie sind.")).small().color(theme::TEXT_DIM()));
                for (k, name) in crate::library::Kind::ALL {
                    theme::section(ui, name);
                    for d in self.library.linked(k) {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{}  {}", icon::LINK_SIMPLE, d.display())).small());
                            if theme::icon_button(ui, icon::X, "Verknüpfung lösen", true, false).clicked() {
                                self.library.unlink_folder(k, &d);
                            }
                        });
                    }
                    if ui.add(theme::secondary(format!("{}  Ordner verknüpfen …", icon::FOLDER_SIMPLE_PLUS))).clicked() {
                        if let Some(d) = crate::dialog().pick_folder() {
                            self.library.link_folder(k, d);
                        }
                    }
                }
                theme::section(ui, "Plugins");
                widgets::toggle_row(ui, "Downloads in die Bibliothek übernehmen", &mut self.settings.lib_downloads);
                ui.label(RichText::new(crate::tr("Musik von Epidemic Sound und Videos von Pexels stehen dann auch in anderen Projekten bereit. Freesound-Sounds landen immer in der Bibliothek.")).small().color(theme::TEXT_FAINT()));
            }
            Section::Audio => {
                // Geräteliste einmal je Öffnen abfragen (die Abfrage ist nicht billig)
                let list_id = egui::Id::new("audio-devices");
                let devices: Vec<String> = ui.data(|d| d.get_temp(list_id)).unwrap_or_else(|| {
                    let v = crate::audio::output_devices();
                    ui.data_mut(|d| d.insert_temp(list_id, v.clone()));
                    v
                });
                let before = self.settings.audio_device.clone();
                theme::row(ui, "Ausgabegerät", |ui| {
                    egui::ComboBox::from_id_salt("set-audio-out")
                        .width(300.0)
                        .selected_text(self.settings.audio_device.clone().unwrap_or_else(|| crate::tr("Systemstandard")))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.settings.audio_device, None, crate::tr("Systemstandard"));
                            for d in &devices {
                                ui.selectable_value(&mut self.settings.audio_device, Some(d.clone()), d);
                            }
                        });
                });
                if before != self.settings.audio_device {
                    crate::audio::set_output_device(self.settings.audio_device.clone());
                    if self.playing {
                        self.restart_audio();
                    }
                }
                widgets::toggle_row(ui, "Ton beim Ziehen des Playheads", &mut self.settings.scrub_audio);
                widgets::toggle_row(ui, "Hinweiston, wenn ein Export fertig ist", &mut self.settings.export_sound);
                ui.add_space(6.0);
                theme::row(ui, "Kanäle", |ui| {
                    egui::ComboBox::from_id_salt("set-channels")
                        .selected_text(crate::tr(self.settings.new_channels.name()))
                        .show_ui(ui, |ui| {
                            for (c, n) in cl_core::Channels::ALL {
                                ui.selectable_value(&mut self.settings.new_channels, c, crate::tr(n));
                            }
                        });
                });
                ui.label(RichText::new(crate::tr("Standard für neue Projekte. „Automatisch“ legt Ton, der nur auf einer Seite aufgenommen wurde, auf beide Ohren. Pro Clip änderbar.")).small().color(theme::TEXT_FAINT()));
                ui.add_space(6.0);
                let mut s = self.project.settings.clone();
                theme::row(ui, "Dieses Projekt", |ui| {
                    egui::ComboBox::from_id_salt("proj-channels")
                        .selected_text(crate::tr(s.channels.name()))
                        .show_ui(ui, |ui| {
                            for (c, n) in cl_core::Channels::ALL {
                                ui.selectable_value(&mut s.channels, c, crate::tr(n));
                            }
                        });
                });
                if s != self.project.settings {
                    self.apply(cl_core::EditOp::SetSettings { settings: s });
                }
            }
            Section::Editing => {
                theme::row(ui, "Bilddauer", |ui| {
                    Slider::new(&mut self.settings.image_duration, 0.5..=30.0)
                        .suffix(" s")
                        .decimals(1)
                        .default(5.0)
                        .show(ui)
                });
                theme::row(ui, "Textdauer", |ui| {
                    Slider::new(&mut self.settings.text_duration, 0.5..=30.0)
                        .suffix(" s")
                        .decimals(1)
                        .default(3.0)
                        .show(ui)
                });
                theme::row(ui, "Übergang", |ui| {
                    Slider::new(&mut self.settings.transition_duration, 0.1..=3.0)
                        .suffix(" s")
                        .decimals(1)
                        .default(0.5)
                        .show(ui)
                });
                theme::row(ui, "Animation", |ui| {
                    Slider::new(&mut self.settings.anim_duration, 0.1..=3.0)
                        .suffix(" s")
                        .decimals(1)
                        .default(0.5)
                        .show(ui)
                });
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
                ui.label(RichText::new(crate::tr("Export immer in voller Auflösung")).small().color(theme::TEXT_DIM()));
                self.proxy_settings(ui);
                ui.add_space(8.0);
                theme::section(ui, "Cache");
                // Größe höchstens alle 2 s neu zählen
                if self.cache_size.is_none_or(|(t, _)| t.elapsed().as_secs_f32() > 2.0) {
                    let n = paths::dir_size(&paths::data_dir().join("cache"))
                        + paths::dir_size(&paths::data_dir().join("thumbs"));
                    self.cache_size = Some((std::time::Instant::now(), n));
                }
                let bytes = self.cache_size.map(|c| c.1).unwrap_or(0);
                theme::row(ui, "Belegt", |ui| {
                    ui.label(RichText::new(crate::props::fmt_bytes(bytes)).strong());
                    if ui
                        .add(theme::secondary(format!("{}  Cache leeren", icon::BROOM)))
                        .on_hover_text(crate::tr("Proxys, Vorschaubilder und Projektbilder. Werden bei Bedarf neu erzeugt."))
                        .clicked()
                    {
                        // Masken und SVG-Bilder älterer Projekte liegen auch im Cache: stehen lassen
                        paths::clear_dir(&paths::data_dir().join("cache"), paths::CACHE_KEEP);
                        paths::clear_dir(&paths::data_dir().join("thumbs"), &[]);
                        self.fx_thumbs.clear();
                        self.fx_thumbs_requested.clear();
                        self.cache_size = None;
                        self.invalidate_preview();
                        self.toast(format!("{} {}", icon::CHECK, crate::tr("Cache geleert")));
                    }
                });
                theme::row(ui, "Aufräumen", |ui| {
                    let name = |d: u32| match d {
                        0 => crate::tr("Nie"),
                        d => crate::tr(format!("Älter als {d} Tage")),
                    };
                    egui::ComboBox::from_id_salt("set-cache-days")
                        .selected_text(name(self.settings.cache_days))
                        .show_ui(ui, |ui| {
                            for d in [0, 7, 30, 90] {
                                ui.selectable_value(&mut self.settings.cache_days, d, name(d));
                            }
                        });
                });
                ui.label(RichText::new(crate::tr("Wird beim Programmstart geprüft.")).small().color(theme::TEXT_FAINT()));
            }
            Section::Mcp => {
                ui.label(RichText::new(crate::tr("Nur auf diesem Rechner erreichbar (127.0.0.1), mit Token. Jede Änderung lässt sich rückgängig machen.")).small().color(theme::TEXT_DIM()));
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
                let running = self
                    .mcp
                    .as_ref()
                    .map(|m| (m.port, m.token.clone(), m.read_only));
                ui.add_enabled_ui(running.is_none(), |ui| {
                    theme::row(ui, "Port", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.settings.mcp_port).range(1024..=65535),
                        )
                    });
                    widgets::toggle_row(
                        ui,
                        "Nur lesen (keine Änderungen erlauben)",
                        &mut self.settings.mcp_read_only,
                    );
                });
                if let Some((port, token, ro)) = running {
                    theme::section(ui, "Verbindung");
                    theme::row(ui, "Status", |ui| {
                        ui.label(
                            RichText::new(crate::tr(format!(
                                "{}  aktiv auf Port {port}{}",
                                icon::CIRCLE,
                                if ro { " · nur lesen" } else { "" }
                            )))
                            .color(theme::ACCENT()),
                        )
                    });
                    theme::row(ui, "Adresse", |ui| {
                        ui.monospace(format!("http://127.0.0.1:{port}/mcp"))
                    });
                    theme::row(ui, "Token", |ui| {
                        ui.monospace(format!("{}…", &token[..10.min(token.len())]));
                        if theme::icon_button(
                            ui,
                            icon::ARROWS_CLOCKWISE,
                            "Neues Token erzeugen (alte Clients verlieren den Zugriff)",
                            true,
                            false,
                        )
                        .clicked()
                        {
                            self.start_mcp();
                        }
                    });
                    ui.add_space(6.0);
                    if ui
                        .add(theme::primary(format!(
                            "{}  Client-Konfiguration kopieren",
                            icon::COPY
                        )))
                        .clicked()
                    {
                        let cfg = serde_json::json!({ "mcpServers": { "cutline": {
                            "type": "http", "url": format!("http://127.0.0.1:{port}/mcp"),
                            "headers": { "Authorization": format!("Bearer {token}") } } } });
                        ui.ctx()
                            .copy_text(serde_json::to_string_pretty(&cfg).unwrap_or_default());
                        self.toast("MCP-Konfiguration kopiert");
                    }
                    ui.label(
                        RichText::new(crate::tr(format!(
                            "Auch gespeichert in {}",
                            paths::data_dir().join("mcp.json").display()
                        )))
                        .small()
                        .color(theme::TEXT_FAINT()),
                    );
                } else if self.settings.mcp_enabled {
                    ui.label(
                        RichText::new(crate::tr("Server startet …"))
                            .small()
                            .color(theme::TEXT_DIM()),
                    );
                }
                theme::section(ui, "Ohne laufende App (stdio)");
                ui.label(
                    RichText::new(crate::tr("cutline --mcp projekt.cutline"))
                        .monospace()
                        .color(theme::TEXT_DIM()),
                );
            }
            Section::Shortcuts => self.shortcuts_settings(ui),
            Section::Updates => self.updates_settings(ui),
            Section::Integrations => self.integrations_settings(ui),
            Section::Import => self.import_settings(ui),
            Section::Models => self.models_settings(ui),
            Section::About => {
                ui.horizontal(|ui| theme::wordmark(ui, 34.0));
                ui.add_space(8.0);
                ui.label(crate::tr(format!(
                    "Version {} · MPL-2.0",
                    env!("CARGO_PKG_VERSION")
                )));
                ui.label(RichText::new(crate::tr("Das offene Schnittprogramm für Creator. Einfach zu bedienen, schnell, komplett auf deinem Rechner. Lokale KI für Untertitel, Freistellen und Entrauschen – nichts wird hochgeladen; keine erzeugende KI.")).color(theme::TEXT_DIM()));
                ui.add_space(8.0);
                let (e, f, t) = (
                    cl_fx::EFFECTS.len(),
                    cl_fx::FILTERS.len(),
                    cl_fx::TRANSITIONS.len(),
                );
                ui.label(RichText::new(crate::tr(format!(
                    "{e} Effekte · {f} Filter · {t} Übergänge · {} Sticker · {} Soundeffekte · {} Textvorlagen",
                    cl_fx::stickers::STICKERS.len(),
                    cl_fx::sfx::SFX.len(),
                    cl_fx::text::TEMPLATES.len()
                )))
                .small()
                .color(theme::TEXT_DIM()));
                ui.label(
                    RichText::new(crate::tr(format!(
                        "{} Plugins geladen",
                        self.plugins.plugins.len()
                    )))
                    .small()
                    .color(theme::TEXT_DIM()),
                );
            }
        }
    }
}
