//! Plugins „Freesound“ (Soundeffekte, CC-Lizenzen) und „Pexels“ (Stock-Videos und -Fotos).
//! Beide eingebaut und standardmäßig aus; sie brauchen einen eigenen API-Schlüssel.
//! Freesound erscheint im Audio-Tab, Pexels im Medien-Tab. Geladene Dateien landen in der
//! eigenen Bibliothek (Sounds) bzw. im Medienordner des Projekts.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::theme::{self, icon};
use crate::widgets;

pub const FREESOUND: &str = "freesound";
pub const PEXELS: &str = "pexels";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Keys {
    pub freesound: String,
    pub pexels: String,
}

fn keys_path() -> PathBuf {
    crate::paths::data_dir()
        .join("Integrations")
        .join("stock.json")
}

pub fn load_keys() -> Keys {
    std::fs::read_to_string(keys_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_keys(k: &Keys) {
    let p = keys_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = crate::paths::write_atomic(&p, serde_json::to_string_pretty(k).unwrap_or_default());
}

/// Ein Suchtreffer (Freesound oder Pexels)
#[derive(Debug, Clone)]
pub struct Hit {
    pub title: String,
    /// ID beim Dienst: macht den Dateinamen eindeutig (Titel wiederholen sich oft)
    pub id: String,
    pub info: String,
    /// Datei zum Laden (MP3-Vorschau bzw. Video/Foto)
    pub url: String,
    pub ext: &'static str,
    pub license: String,
}

enum Msg {
    Results(&'static str, Result<Vec<Hit>, String>),
    Downloaded(&'static str, Result<(PathBuf, String), String>),
}

pub struct StockState {
    pub keys: Keys,
    pub query: [String; 2],
    pub results: [Vec<Hit>; 2],
    pub busy: bool,
    pub photos: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl Default for StockState {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            keys: load_keys(),
            query: Default::default(),
            results: Default::default(),
            busy: false,
            photos: false,
            tx,
            rx,
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(8))
        .timeout_read(std::time::Duration::from_secs(60))
        .user_agent("Cutline")
        .build()
}

fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(401 | 403, _) => "Schlüssel ungültig oder ohne Zugriff".into(),
        ureq::Error::Status(429, _) => "Zu viele Anfragen, bitte kurz warten".into(),
        ureq::Error::Status(c, _) => format!("Dienst antwortet mit {c}"),
        ureq::Error::Transport(t) => format!("Keine Verbindung ({t})"),
    }
}

/// Freesound-Textsuche (APIv2, Token-Authentifizierung; Vorschauen ohne OAuth ladbar)
pub fn freesound_search(key: &str, q: &str) -> Result<Vec<Hit>, String> {
    let v: serde_json::Value = agent()
        .get("https://freesound.org/apiv2/search/text/")
        .query("query", q)
        .query("fields", "id,name,duration,username,license,previews")
        .query("page_size", "30")
        .set("Authorization", &format!("Token {key}"))
        .call()
        .map_err(http_err)?
        .into_json()
        .map_err(|e| e.to_string())?;
    Ok(v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let url = r["previews"]["preview-hq-mp3"].as_str()?.to_string();
            let lic = r["license"]
                .as_str()
                .unwrap_or("")
                .rsplit('/')
                .nth(1)
                .unwrap_or("CC")
                .to_uppercase();
            Some(Hit {
                title: r["name"].as_str().unwrap_or("?").to_string(),
                id: r["id"].to_string(),
                info: format!(
                    "{:.1} s · {}",
                    r["duration"].as_f64().unwrap_or(0.0),
                    r["username"].as_str().unwrap_or("")
                ),
                url,
                ext: "mp3",
                license: lic,
            })
        })
        .collect())
}

/// Pexels-Suche nach Videos oder Fotos
pub fn pexels_search(key: &str, q: &str, photos: bool) -> Result<Vec<Hit>, String> {
    let url = if photos {
        "https://api.pexels.com/v1/search"
    } else {
        "https://api.pexels.com/videos/search"
    };
    let v: serde_json::Value = agent()
        .get(url)
        .query("query", q)
        .query("per_page", "30")
        .set("Authorization", key)
        .call()
        .map_err(http_err)?
        .into_json()
        .map_err(|e| e.to_string())?;
    if photos {
        return Ok(v["photos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                Some(Hit {
                    title: p["alt"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Foto")
                        .to_string(),
                    id: p["id"].to_string(),
                    info: format!(
                        "{}×{} · {}",
                        p["width"],
                        p["height"],
                        p["photographer"].as_str().unwrap_or("")
                    ),
                    url: p["src"]["large2x"].as_str()?.to_string(),
                    ext: "jpg",
                    license: "Pexels".into(),
                })
            })
            .collect());
    }
    Ok(v["videos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|vd| {
            // beste Datei bis 1080p
            let files = vd["video_files"].as_array()?;
            let f = files
                .iter()
                .filter(|f| f["height"].as_u64().unwrap_or(0) <= 1080)
                .max_by_key(|f| f["height"].as_u64().unwrap_or(0))?;
            Some(Hit {
                title: vd["url"]
                    .as_str()
                    .and_then(|u| u.trim_end_matches('/').rsplit('/').next())
                    .map(|s| {
                        s.rsplit_once('-')
                            .map(|(a, _)| a.replace('-', " "))
                            .unwrap_or_else(|| s.to_string())
                    })
                    .unwrap_or_else(|| "Video".into()),
                id: vd["id"].to_string(),
                info: format!(
                    "{} s · {}×{} · {}",
                    vd["duration"],
                    f["width"],
                    f["height"],
                    vd["user"]["name"].as_str().unwrap_or("")
                ),
                url: f["link"].as_str()?.to_string(),
                ext: "mp4",
                license: "Pexels".into(),
            })
        })
        .collect())
}

fn safe(s: &str) -> String {
    s.chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .take(60)
        .collect::<String>()
        .trim()
        .to_string()
}

impl App {
    pub fn stock_enabled(&self, which: &str) -> bool {
        self.plugins.integration_enabled(which)
    }

    pub fn poll_stock(&mut self) {
        while let Ok(m) = self.stock.rx.try_recv() {
            self.stock.busy = false;
            match m {
                Msg::Results(w, Ok(r)) => {
                    self.stock.results[if w == FREESOUND { 0 } else { 1 }] = r
                }
                Msg::Results(_, Err(e)) => self.toast_err(e),
                Msg::Downloaded(w, Ok((p, title))) => {
                    if w == FREESOUND {
                        self.library.invalidate();
                        self.add_file_at(p, self.playhead, None);
                    } else if let Some(m) = self.import_media(vec![p.clone()]).first().copied() {
                        if self.settings.lib_downloads {
                            self.library.import(crate::library::Kind::Media, "Pexels", &[p]);
                        }
                        self.add_media_at(m, self.playhead, None);
                    }
                    self.toast(format!("„{title}“ eingefügt"));
                }
                Msg::Downloaded(_, Err(e)) => self.toast_err(e),
            }
        }
    }

    /// Suchliste (Freesound im Audio-Tab, Pexels im Medien-Tab)
    pub fn stock_panel(&mut self, ui: &mut egui::Ui, which: &'static str) {
        let ctx = ui.ctx().clone();
        let i = if which == FREESOUND { 0 } else { 1 };
        let key = if i == 0 {
            self.stock.keys.freesound.clone()
        } else {
            self.stock.keys.pexels.clone()
        };
        if key.trim().is_empty() {
            theme::empty_state(
                ui,
                icon::KEY,
                "API-Schlüssel fehlt",
                "Unter Einstellungen → Plugins und Konten eintragen",
            );
            ui.vertical_centered(|ui| {
                if ui.add(theme::primary("Einrichten …")).clicked() {
                    self.settings_section = crate::settings::Section::Integrations;
                    self.settings_open = true;
                }
            });
            return;
        }
        let mut go = false;
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 40.0).max(60.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.stock.query[i])
                    .hint_text(crate::tr(format!("{}  Suchen", icon::MAGNIFYING_GLASS)))
                    .desired_width(w),
            );
            go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if self.stock.busy {
                ui.add(egui::Spinner::new().color(theme::ACCENT()));
            }
        });
        if i == 1 {
            let mut sel = self.stock.photos as usize;
            if widgets::segmented(ui, &["Videos", "Fotos"], &mut sel) {
                self.stock.photos = sel == 1;
                go = true;
            }
        }
        if go && !self.stock.query[i].trim().is_empty() {
            self.stock.busy = true;
            let (q, tx, photos, c) = (
                self.stock.query[i].clone(),
                self.stock.tx.clone(),
                self.stock.photos,
                ctx.clone(),
            );
            std::thread::spawn(move || {
                let r = if which == FREESOUND {
                    freesound_search(&key, &q)
                } else {
                    pexels_search(&key, &q, photos)
                };
                let _ = tx.send(Msg::Results(which, r));
                c.request_repaint();
            });
        }
        ui.add_space(4.0);
        let hits = self.stock.results[i].clone();
        if hits.is_empty() && !self.stock.busy {
            ui.label(
                RichText::new(crate::tr("Suchbegriff eingeben und Enter drücken"))
                    .small()
                    .color(theme::TEXT_DIM()),
            );
        }
        let mut get: Option<Hit> = None;
        egui::ScrollArea::vertical()
            .id_salt(("stock", which))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for h in &hits {
                    ui.horizontal(|ui| {
                        ui.set_min_height(32.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(
                                ui,
                                icon::PLUS,
                                "Laden und am Playhead einfügen",
                                !self.stock.busy,
                                false,
                            )
                            .clicked()
                            {
                                get = Some(h.clone());
                            }
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(&h.title).size(theme::FS_TEXT)).truncate(),
                                );
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("{} · {}", h.info, h.license))
                                            .size(theme::FS_HINT)
                                            .color(theme::TEXT_DIM()),
                                    )
                                    .truncate(),
                                );
                            });
                        });
                    });
                }
            });
        if let Some(h) = get {
            self.stock.busy = true;
            let dest = if which == FREESOUND {
                crate::library::Kind::Sound
                    .dir()
                    .join("Freesound")
                    .join(format!("{} {}.{}", safe(&h.title), h.id, h.ext))
            } else {
                let base = self
                    .project_path
                    .as_ref()
                    .and_then(|p| {
                        p.parent()
                            .map(|d| d.join(format!("{}-media", crate::paths::safe_name(&self.project.name))))
                    })
                    .unwrap_or_else(|| crate::paths::data_dir().join("Stock"));
                base.join("Pexels")
                    .join(format!("{} {}.{}", safe(&h.title), h.id, h.ext))
            };
            let (tx, c) = (self.stock.tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let r = if dest.is_file() {
                    Ok(())
                } else {
                    // schreibt erst *.part: ein abgebrochener Download wird nie wiederverwendet
                    cl_update::download_to(&h.url, &dest, |_, _| {}, &Default::default())
                };
                let _ = tx.send(Msg::Downloaded(which, r.map(|_| (dest, h.title.clone()))));
                c.request_repaint();
            });
        }
    }

    /// Abschnitte in Einstellungen → Plugins und Konten
    pub fn stock_settings(&mut self, ui: &mut egui::Ui) {
        for (which, title, about, url) in [
            (FREESOUND, "Freesound", "Hunderttausende Soundeffekte unter Creative-Commons-Lizenzen. Lizenz und Urheber stehen bei jedem Treffer.", "https://freesound.org/apiv2/apply/"),
            (PEXELS, "Pexels", "Kostenlose Stock-Videos und -Fotos (Pexels-Lizenz, Nennung freiwillig).", "https://www.pexels.com/api/"),
        ] {
            theme::section(ui, title);
            let mut on = self.stock_enabled(which);
            if widgets::toggle_row(ui, "Plugin aktiv", &mut on) {
                let id = self.plugins.plugins.iter().find(|p| p.manifest.plugin.entry.as_deref() == Some(&format!("builtin:{which}"))).map(|p| p.manifest.plugin.id.clone());
                if let Some(id) = id {
                    self.set_plugin_enabled(&id, on);
                }
            }
            ui.label(RichText::new(crate::tr(about)).small().color(theme::TEXT_DIM()));
            if on {
                let k = if which == FREESOUND { &mut self.stock.keys.freesound } else { &mut self.stock.keys.pexels };
                let mut changed = false;
                theme::row(ui, "API-Schlüssel", |ui| {
                    changed = ui.add(egui::TextEdit::singleline(k).password(true).desired_width(ui.available_width() - 12.0)).changed();
                });
                if changed {
                    save_keys(&self.stock.keys);
                }
                if ui.add(theme::ghost(format!("{}  Schlüssel beantragen", icon::ARROW_SQUARE_OUT))).clicked() {
                    crate::epidemic::open_url(url);
                }
            }
        }
    }
}
