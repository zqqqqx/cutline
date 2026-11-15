//! Epidemic-Sound-Integration in der Oberfläche: Musik im Audio-Tab (Entdecken, Favoriten,
//! „Gefällt mir“), Anmeldung unter Einstellungen → Plugins und Konten, Nutzungsmeldung beim Export.
//! Alles Netzwerk läuft in Hintergrund-Threads; Ergebnisse kommen über einen Kanal zurück.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use cl_epidemic::{AuthMode, Client, Config, Error, Named, Query, Track};
use eframe::egui::{self, vec2, RichText};

use crate::app::{App, DragAsset};
use crate::paths;
use crate::theme::{self, icon};
use crate::widgets;

pub const BUILTIN: &str = "epidemic-sound";

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Discover,
    Favorites,
    Liked,
}

enum Msg {
    Tracks {
        liked: bool,
        append: bool,
        res: Result<Vec<Track>, Error>,
    },
    Filters(Vec<Named>, Vec<Named>),
    User(Result<String, Error>),
    Preview {
        key: String,
        res: Result<cl_core::MediaRef, Error>,
    },
    Downloaded {
        track: Track,
        res: Result<PathBuf, Error>,
        beats: Vec<f64>,
        /// wirklich geladen (nicht aus dem Vorrat)
        fresh: bool,
    },
    LoggedIn(Result<cl_epidemic::Tokens, Error>),
    Reported(Result<usize, Error>),
    /// Aktualisierte Tokens nach einem Refresh im Hintergrund
    Tokens(Option<cl_epidemic::Tokens>),
}

pub struct EsState {
    pub cfg: Config,
    /// Heruntergeladene Dateien → Track-ID (für die Nutzungsmeldung beim Export)
    pub downloads: BTreeMap<PathBuf, String>,
    pub view: View,
    pub query: Query,
    pub results: Vec<Track>,
    pub liked: Vec<Track>,
    pub moods: Vec<Named>,
    pub genres: Vec<Named>,
    pub user: Option<String>,
    pub busy: bool,
    pub loading_track: Option<String>,
    /// Titel, die schon im Vorrat liegen (Dateinamen ohne Endung)
    have: HashSet<String>,
    pub error: Option<String>,
    /// Nach 429: bis dahin keine Suche/Nachladen (Vorhören und Vorrat laufen weiter)
    limited_until: Option<std::time::Instant>,
    pub login_running: bool,
    loaded: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct Stored {
    cfg: Config,
    downloads: BTreeMap<PathBuf, String>,
}

fn store_path() -> PathBuf {
    paths::data_dir()
        .join("Integrations")
        .join("epidemic-sound.json")
}

impl Default for EsState {
    fn default() -> Self {
        let s: Stored = std::fs::read_to_string(store_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let (tx, rx) = channel();
        let mut cfg = s.cfg;
        // feste, anonyme Kennung für x-partner-user-id (einmal erzeugt, dann gespeichert)
        let fresh = cfg.user_id.is_empty();
        cfg.ensure_user_id();
        if let Ok(b) = std::env::var("CUTLINE_EPIDEMIC_BASE") {
            cfg.base_url = Some(b);
        }
        let st = Self {
            cfg,
            downloads: s.downloads,
            view: View::Discover,
            query: Query::default(),
            results: Vec::new(),
            liked: Vec::new(),
            moods: Vec::new(),
            genres: Vec::new(),
            user: None,
            busy: false,
            loading_track: None,
            have: stored_ids(&es_store_dir()),
            error: None,
            limited_until: None,
            login_running: false,
            loaded: false,
            tx,
            rx,
        };
        if fresh {
            st.save();
        }
        st
    }
}

impl EsState {
    pub fn save(&self) {
        let p = store_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let s = Stored {
            cfg: Config {
                base_url: None,
                ..self.cfg.clone()
            },
            downloads: self.downloads.clone(),
        };
        let _ = paths::write_atomic(&p, serde_json::to_string_pretty(&s).unwrap_or_default());
    }

    /// Hintergrund-Aufgabe mit eigenem Client (Token-Refresh wird zurückgemeldet).
    fn spawn(&self, ctx: &egui::Context, f: impl FnOnce(&Client) -> Msg + Send + 'static) {
        let mut cfg = self.cfg.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let before = cfg.tokens.clone();
            let msg = match Client::from_config(&mut cfg) {
                Ok(c) => f(&c),
                Err(e) => Msg::Tracks {
                    liked: false,
                    append: false,
                    res: Err(e),
                },
            };
            if cfg.tokens != before {
                let _ = tx.send(Msg::Tokens(cfg.tokens.clone()));
            }
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// Sekunden bis wieder Anfragen für Suche/Liste erlaubt sind (nach 429).
    fn limit_left(&self) -> Option<u64> {
        limit_remaining(self.limited_until, std::time::Instant::now())
    }

    /// Steht ein Limit an, meldet es das (statt anzufragen) und `true`.
    fn blocked_by_limit(&mut self) -> bool {
        let Some(n) = self.limit_left() else { return false };
        self.busy = false;
        self.error = Some(Error::RateLimited(Some(n)).to_string());
        true
    }

    pub fn search(&mut self, ctx: &egui::Context, append: bool) {
        if self.blocked_by_limit() {
            return;
        }
        if !append {
            self.query.offset = 0;
        }
        self.busy = true;
        self.error = None;
        let q = self.query.clone();
        self.spawn(ctx, move |c| Msg::Tracks {
            liked: false,
            append,
            res: c.search(&q).map(|p| p.tracks),
        });
    }

    fn load_liked(&mut self, ctx: &egui::Context) {
        if self.blocked_by_limit() {
            return;
        }
        self.busy = true;
        self.spawn(ctx, |c| Msg::Tracks {
            liked: true,
            append: false,
            res: c.liked(0).map(|p| p.tracks),
        });
    }

    fn first_load(&mut self, ctx: &egui::Context) {
        if self.loaded || !self.cfg.ready() {
            return;
        }
        self.loaded = true;
        self.spawn(ctx, |c| {
            Msg::Filters(
                c.moods().unwrap_or_default(),
                c.genres().unwrap_or_default(),
            )
        });
        if self.cfg.mode == AuthMode::Connect {
            self.spawn(ctx, |c| Msg::User(c.me()));
        }
        self.search(ctx, false);
    }

    /// Nach Änderung der Zugangsdaten neu laden.
    pub fn reset(&mut self) {
        self.loaded = false;
        self.results.clear();
        self.liked.clear();
        self.user = None;
        self.error = None;
        self.save();
    }
}

/// Browser mit einer Adresse öffnen.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    })
    .arg(url)
    .spawn();
}

impl App {
    pub fn epidemic_enabled(&self) -> bool {
        self.plugins.integration_enabled(BUILTIN)
    }

    /// Ergebnisse der Hintergrund-Aufgaben übernehmen (jeden Frame).
    pub fn poll_epidemic(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.es.rx.try_recv() {
            match m {
                Msg::Tracks { liked, append, res } => {
                    self.es.busy = false;
                    match res {
                        Ok(t) if liked => self.es.liked = t,
                        Ok(t) if append => self.es.results.extend(t),
                        Ok(t) => self.es.results = t,
                        Err(e) => {
                            if !self.es_note(&e) {
                                self.es.error = Some(e.to_string());
                            }
                        }
                    }
                }
                Msg::Filters(m, g) => {
                    self.es.moods = m;
                    self.es.genres = g;
                }
                Msg::User(u) => self.es.user = u.ok(),
                Msg::Preview { key, res } => match res {
                    Ok(m) => {
                        self.es.loading_track = None;
                        self.preview_media(key, m);
                    }
                    Err(e) => {
                        self.es.loading_track = None;
                        if !self.es_note(&e) {
                            self.toast_err(e.to_string());
                        }
                    }
                },
                Msg::Downloaded { track, res, beats, fresh } => {
                    self.es.loading_track = None;
                    match res {
                        Ok(path) => {
                            self.es.downloads.insert(path.clone(), track.id.clone());
                            self.es.have.insert(store_name(&track.id));
                            self.es.save();
                            // in die Bibliothek nur beim ersten Laden (sonst entstünden Doppel)
                            if fresh && self.settings.lib_downloads {
                                self.library.import(crate::library::Kind::Sound, "Epidemic Sound", std::slice::from_ref(&path));
                            }
                            if let Some(id) = self.add_file_at(path, self.playhead, None) {
                                if !beats.is_empty() {
                                    if let Some(mut c) =
                                        self.project.find_clip(id).map(|(_, c)| c.clone())
                                    {
                                        c.beats = beats
                                            .iter()
                                            .map(|b| cl_core::Time::from_secs_f64(*b))
                                            .collect();
                                        self.apply(cl_core::EditOp::ReplaceClip { clip: c });
                                    }
                                }
                                self.toast(format!("„{}“ eingefügt", track.title));
                            }
                        }
                        Err(e) => {
                            if !self.es_note(&e) {
                                self.toast_err(e.to_string());
                            }
                        }
                    }
                }
                Msg::LoggedIn(r) => {
                    self.es.login_running = false;
                    match r {
                        Ok(t) => {
                            self.es.cfg.tokens = Some(t);
                            self.es.reset();
                            self.toast("Mit Epidemic Sound verbunden");
                            self.es.first_load(ctx);
                        }
                        Err(e) => self.toast_err(e.to_string()),
                    }
                }
                Msg::Reported(r) => {
                    if let Err(e) = r {
                        log::warn!("Epidemic-Nutzungsmeldung: {e}");
                    }
                }
                Msg::Tokens(t) => {
                    self.es.cfg.tokens = t;
                    self.es.save();
                }
            }
        }
    }

    /// Fehler einer Hintergrundaufgabe auswerten. `true` = schon gemeldet (Anmeldung abgelaufen).
    /// Abgelaufene Anmeldung: Tokens verwerfen, Zustand „nicht verbunden“, genau eine Meldung.
    /// Limit (429): merken, bis wann keine Suche/Nachladen abgeschickt wird.
    fn es_note(&mut self, e: &Error) -> bool {
        match e {
            Error::LoginExpired if self.es.cfg.mode == AuthMode::Connect => {
                // mehrere gleichzeitige Anfragen scheitern zusammen: nur die erste meldet
                if self.es.cfg.tokens.take().is_some() {
                    self.es.busy = false;
                    self.es.reset();
                    self.toast_err(e.to_string());
                }
                true
            }
            Error::RateLimited(s) => {
                self.es.limited_until = Some(std::time::Instant::now() + cl_epidemic::limit_wait(*s));
                false
            }
            _ => false,
        }
    }

    /// Nach einem erfolgreichen Export: verwendete Epidemic-Titel melden.
    pub fn report_epidemic_export(&mut self, ctx: &egui::Context) {
        if !self.epidemic_enabled() || !self.es.cfg.ready() {
            return;
        }
        let ids: Vec<String> = self
            .project
            .media
            .iter()
            .filter(|m| self.project.timeline.tracks.iter().any(|t| t.clips.iter().any(|c| matches!(&c.content, cl_core::ClipContent::Media { media, .. } if *media == m.id))))
            .filter_map(|m| self.es.downloads.get(&m.path).cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if ids.is_empty() {
            return;
        }
        let n = ids.len();
        self.es.spawn(ctx, move |c| {
            Msg::Reported(c.report_export(&ids, "LOCAL").map(|_| n))
        });
    }

    fn es_media_dir(&self) -> PathBuf {
        self.project_path
            .as_ref()
            .and_then(|p| {
                p.parent()
                    .map(|d| d.join(format!("{}-media", paths::safe_name(&self.project.name))))
            })
            .unwrap_or_else(|| paths::data_dir().join("Epidemic Sound"))
            .join("Epidemic Sound")
    }

    /// Bereich „Epidemic Sound“ im Audio-Tab.
    pub fn epidemic_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.es.cfg.ready() {
            theme::empty_state(
                ui,
                icon::MUSIC_NOTES,
                "Epidemic Sound ist noch nicht verbunden",
                "Melde dich unter Einstellungen → Plugins und Konten an",
            );
            ui.vertical_centered(|ui| {
                if ui.add(theme::primary("Verbinden …")).clicked() {
                    self.settings_open = true;
                    self.settings_section = crate::settings::Section::Integrations;
                }
            });
            return;
        }
        self.es.first_load(&ctx);
        // Ansicht
        let mut v = match self.es.view {
            View::Discover => 0,
            View::Favorites => 1,
            View::Liked => 2,
        };
        let connect = self.es.cfg.mode == AuthMode::Connect;
        let items: &[&str] = if connect {
            &["Entdecken", "Favoriten", "Gefällt mir"]
        } else {
            &["Entdecken", "Favoriten"]
        };
        if widgets::segmented(ui, items, &mut v) {
            self.es.view = [View::Discover, View::Favorites, View::Liked][v];
            if self.es.view == View::Liked && self.es.liked.is_empty() {
                self.es.load_liked(&ctx);
            }
        }
        ui.add_space(6.0);
        if self.es.view == View::Discover {
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.es.query.term)
                        .hint_text(crate::tr(format!(
                            "{}  Titel, Stimmung, Genre …",
                            icon::MAGNIFYING_GLASS
                        )))
                        .desired_width(ui.available_width() - 34.0),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.es.search(&ctx, false);
                }
                if theme::icon_button(ui, icon::MAGNIFYING_GLASS, "Suchen", !self.es.busy, false)
                    .clicked()
                {
                    self.es.search(&ctx, false);
                }
            });
            // Filter: Stimmungen und Genres als Chips, Instrumental
            let mut changed = false;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                let mut nv = self.es.query.no_vocals;
                if theme::cat_chip(ui, "Instrumental", nv).clicked() {
                    nv = !nv;
                    changed = true;
                }
                self.es.query.no_vocals = nv;
                for m in self.es.moods.clone().iter().take(10) {
                    let on = self.es.query.moods.contains(&m.id);
                    if theme::cat_chip(ui, &m.name, on).clicked() {
                        if on {
                            self.es.query.moods.retain(|x| *x != m.id);
                        } else {
                            self.es.query.moods.push(m.id.clone());
                        }
                        changed = true;
                    }
                }
                for g in self.es.genres.clone().iter().take(10) {
                    let on = self.es.query.genres.contains(&g.id);
                    if theme::cat_chip(ui, &g.name, on).clicked() {
                        if on {
                            self.es.query.genres.retain(|x| *x != g.id);
                        } else {
                            self.es.query.genres.push(g.id.clone());
                        }
                        changed = true;
                    }
                }
            });
            if changed {
                self.es.search(&ctx, false);
            }
            ui.add_space(4.0);
        }
        if let Some(e) = &self.es.error {
            ui.label(
                RichText::new(crate::tr(format!("{}  {e}", icon::WARNING)))
                    .small()
                    .color(theme::WARN_TEXT()),
            );
        }
        widgets::busy_row(ui, egui::Id::new("es-busy"), self.es.busy);
        let list: Vec<Track> = match self.es.view {
            View::Discover => self.es.results.clone(),
            View::Favorites => self.es.cfg.favorites.clone(),
            View::Liked => self.es.liked.clone(),
        };
        if list.is_empty() {
            if self.es.busy {
                // Platzhalter in Zeilengröße, bis die ersten Titel da sind
                for i in 0..6 {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), egui::Sense::hover());
                    widgets::skeleton(ui.painter(), egui::Id::new(("es-skel", i)), r.shrink2(vec2(0.0, 2.0)), theme::R_CARD);
                }
            } else if self.es.view == View::Favorites {
                theme::empty_state(
                    ui,
                    icon::STAR,
                    "Noch keine Favoriten",
                    "Markiere Titel mit dem Stern",
                );
            } else {
                theme::empty_state(
                    ui,
                    icon::MUSIC_NOTES,
                    "Keine Titel gefunden",
                    "Andere Suche oder weniger Filter probieren",
                );
            }
            return;
        }
        let mut load_more = false;
        egui::ScrollArea::vertical()
            .id_salt("es-list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for t in &list {
                    self.es_row(ui, &ctx, t);
                }
                if self.es.view == View::Discover
                    && list.len() >= 40
                    && list.len().is_multiple_of(40)
                {
                    ui.vertical_centered(|ui| {
                        if ui
                            .add_enabled(!self.es.busy, theme::ghost("Mehr laden"))
                            .clicked()
                        {
                            load_more = true;
                        }
                    });
                }
            });
        if load_more {
            self.es.query.offset = self.es.results.len() as u32;
            self.es.search(&ctx, true);
        }
    }

    fn es_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, t: &Track) {
        let key = format!("es:{}", t.id);
        let playing = self.sfx_preview.as_ref().is_some_and(|(k, _)| *k == key);
        let loading = self.es.loading_track.as_deref() == Some(t.id.as_str());
        let mut info = t.artists();
        if let Some(b) = t.bpm {
            info.push_str(&format!(" · {b:.0} BPM"));
        }
        if let Some(s) = t.seconds() {
            info.push_str(&format!(" · {}", crate::home::fmt_dur(cl_core::Time::from_secs_f64(s))));
        }
        if let Some(m) = t.moods.first() {
            info.push_str(&format!(" · {}", m.name));
        }
        ui.horizontal(|ui| {
            ui.set_min_height(34.0);
            let glyph = if loading {
                icon::HOURGLASS
            } else if playing {
                icon::STOP
            } else {
                icon::PLAY
            };
            if theme::icon_button(
                ui,
                glyph,
                if playing { "Stopp" } else { "Vorhören" },
                !loading,
                playing,
            )
            .clicked()
            {
                if playing {
                    self.sfx_preview = None;
                } else {
                    self.es.loading_track = Some(t.id.clone());
                    let id = t.id.clone();
                    let k = key.clone();
                    self.es.spawn(ctx, move |c| Msg::Preview {
                        key: k,
                        res: c.stream_url(&id).and_then(|u| {
                            cl_media::probe(std::path::Path::new(&u)).map_err(|e| e.to_string().into())
                        }),
                    });
                }
            }
            let fav = self.es.cfg.is_favorite(&t.id);
            ui.vertical(|ui| {
                // Platz für Stern und Plus rechts lassen, sonst wächst die Leiste jeden Frame
                let sp = ui.spacing().item_spacing.x;
                ui.set_width((ui.available_width() - 2.0 * 24.0 - 3.0 * sp - 6.0).max(60.0));
                ui.add(egui::Label::new(RichText::new(&t.title).size(theme::FS_TEXT)).truncate());
                ui.add(
                    egui::Label::new(RichText::new(info).size(theme::FS_HINT).color(theme::TEXT_DIM()))
                        .truncate(),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let add = theme::icon_button(ui, icon::PLUS, "Am Playhead einfügen", !loading, false).clicked();
                if self.es.have.contains(&store_name(&t.id)) {
                    ui.label(RichText::new(icon::CHECK_CIRCLE).size(theme::FS_HINT).color(theme::TEXT_DIM()))
                        .on_hover_text(crate::tr("Lokal vorrätig"));
                }
                if add {
                    // `isPreviewOnly` ist laut Epidemic kein verlässliches Zeichen: immer versuchen – ist der
                    // Titel im Plan nicht enthalten, lehnt der Dienst ab (403) und das steht dann in der Meldung
                    {
                        self.es.loading_track = Some(t.id.clone());
                        let track = t.clone();
                        let safe: String = format!("{} - {}", t.artists(), t.title)
                            .chars()
                            .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
                            .collect();
                        let dest = self.es_media_dir().join(format!("{safe}.mp3"));
                        let store = es_store(&es_store_dir(), &t.id);
                        // früher geladene Datei desselben Titels (anderes Projekt) als Quelle für den Vorrat
                        let known = self
                            .es
                            .downloads
                            .iter()
                            .find(|(p, id)| **id == t.id && p.is_file())
                            .map(|(p, _)| p.clone());
                        self.es.spawn(ctx, move |c| {
                            let res = fetch_track(&dest, &store, known.as_deref(), |p| c.download(&track.id, p));
                            let beats = if res.is_ok() {
                                cached_beats(&store, || c.beats(&track.id))
                            } else {
                                Vec::new()
                            };
                            Msg::Downloaded {
                                track,
                                fresh: res == Ok(true),
                                res: res.map(|_| dest),
                                beats,
                            }
                        });
                    }
                }
                if theme::icon_button(
                    ui,
                    icon::STAR,
                    if fav {
                        "Aus Favoriten entfernen"
                    } else {
                        "Zu Favoriten"
                    },
                    true,
                    fav,
                )
                .clicked()
                {
                    self.es.cfg.toggle_favorite(t);
                    self.es.save();
                }
            });
        });
        let _ = DragAsset::Sfx;
    }

    /// Abschnitt „Plugins und Konten“ in den Einstellungen.
    pub fn integrations_settings(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.plugins_settings(ui);
        self.story_settings(ui);
        self.stock_settings(ui);
        if !self.epidemic_enabled() {
            return;
        }
        theme::section(ui, "Epidemic Sound");
        let mut mode = if self.es.cfg.mode == AuthMode::Connect {
            0
        } else {
            1
        };
        theme::row(ui, "Anmeldung", |ui| {
            if widgets::segmented(ui, &["Epidemic-Konto", "API-Schlüssel"], &mut mode) {
                self.es.cfg.mode = if mode == 0 {
                    AuthMode::Connect
                } else {
                    AuthMode::ApiKey
                };
                self.es.reset();
            }
        });
        match self.es.cfg.mode {
            AuthMode::Connect => {
                theme::row(ui, "Client-ID", |ui| {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.es.cfg.client_id)
                                .desired_width(ui.available_width() - 12.0),
                        )
                        .lost_focus()
                    {
                        self.es.save();
                    }
                });
                theme::row(ui, "Weiterleitung", |ui| ui.monospace(cl_epidemic::redirect_uri()));
                ui.add_space(4.0);
                if let Some(t) = self.es.cfg.tokens.clone() {
                    theme::row(ui, "Status", |ui| {
                        ui.label(
                            RichText::new(crate::tr(format!(
                                "{}  Verbunden{}",
                                icon::CHECK_CIRCLE,
                                self.es
                                    .user
                                    .as_ref()
                                    .map(|u| format!(" als {u}"))
                                    .unwrap_or_default()
                            )))
                            .color(theme::TEXT_DIM()),
                        );
                    });
                    if ui.add(theme::secondary("Abmelden")).clicked() {
                        let (cid, rt) = (self.es.cfg.client_id.clone(), t.refresh_token.clone());
                        std::thread::spawn(move || cl_epidemic::logout(&cid, &rt));
                        self.es.cfg.tokens = None;
                        self.es.reset();
                    }
                } else if self.es.login_running {
                    ui.label(
                        RichText::new(crate::tr("Anmeldung im Browser abschließen …"))
                            .color(theme::TEXT_DIM()),
                    );
                    widgets::busy_row(ui, egui::Id::new("es-login"), true);
                } else if ui
                    .add_enabled(
                        !self.es.cfg.client_id.trim().is_empty(),
                        theme::primary(format!("{}  Mit Epidemic Sound verbinden", icon::SIGN_IN)),
                    )
                    .clicked()
                {
                    match cl_epidemic::start_login(&self.es.cfg.client_id) {
                        Ok(login) => {
                            open_url(&login.url);
                            self.es.login_running = true;
                            let tx = self.es.tx.clone();
                            let ctx = ctx.clone();
                            std::thread::spawn(move || {
                                let _ = tx.send(Msg::LoggedIn(
                                    login.finish(std::time::Duration::from_secs(300)),
                                ));
                                ctx.request_repaint();
                            });
                        }
                        Err(e) => self.toast_err(e.to_string()),
                    }
                }
            }
            AuthMode::ApiKey => {
                theme::row(ui, "API-Schlüssel", |ui| {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.es.cfg.api_key)
                                .password(true)
                                .hint_text("epidemic_live_…")
                                .desired_width(ui.available_width() - 12.0),
                        )
                        .lost_focus()
                    {
                        self.es.reset();
                    }
                });
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .add(theme::ghost(format!(
                    "{}  Developer Portal",
                    icon::ARROW_SQUARE_OUT
                )))
                .clicked()
            {
                open_url("https://developers.epidemicsound.com/");
            }
        });
        ui.label(RichText::new(crate::tr("Exporte mit Epidemic-Musik werden an Epidemic Sound gemeldet (nur Titel-IDs).")).small().color(theme::TEXT_DIM()));
    }

    /// Plugin ein-/ausschalten und dauerhaft merken.
    pub fn set_plugin_enabled(&mut self, id: &str, on: bool) {
        if let Some(p) = self
            .plugins
            .plugins
            .iter_mut()
            .find(|p| p.manifest.plugin.id == id)
        {
            p.enabled = on;
        }
        self.settings.plugin_states.insert(id.to_string(), on);
        self.settings.save();
        self.refresh_catalog();
    }
}

/// Zentraler Vorrat geladener Titel (eine Datei je Titel): Ein Titel wird nur einmal geladen und in
/// jedes weitere Projekt kopiert. Liegt nicht im Cache, damit „Cache leeren" ihn nicht entfernt.
fn es_store_dir() -> PathBuf {
    paths::data_dir().join("Epidemic Sound").join("Vorrat")
}

fn store_name(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn es_store(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{}.mp3", store_name(id)))
}

fn stored_ids(dir: &Path) -> HashSet<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mp3"))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect()
}

/// Legt den Titel nach `dest`. Reihenfolge: liegt schon dort → nichts; Vorrat → kopieren; früher
/// geladene Datei (`known`) → in den Vorrat übernehmen und kopieren; sonst laden. `true` = geladen.
fn fetch_track(
    dest: &Path,
    store: &Path,
    known: Option<&Path>,
    download: impl FnOnce(&Path) -> Result<(), Error>,
) -> Result<bool, Error> {
    let io = |e: std::io::Error| Error::from(e.to_string());
    let seed = |from: &Path| -> Result<(), Error> {
        if let Some(d) = store.parent() {
            std::fs::create_dir_all(d).map_err(io)?;
        }
        // erst daneben schreiben: ein abgebrochenes Kopieren hinterlässt keinen halben Vorrat
        let tmp = store.with_extension("part");
        std::fs::copy(from, &tmp).map_err(io)?;
        std::fs::rename(&tmp, store).map_err(io)
    };
    if dest.is_file() {
        if !store.is_file() {
            let _ = seed(dest);
        }
        return Ok(false);
    }
    let mut fresh = false;
    if !store.is_file() {
        match known.filter(|k| k.is_file()) {
            Some(k) => seed(k)?,
            None => {
                download(store)?;
                fresh = true;
            }
        }
    }
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(io)?;
    }
    std::fs::copy(store, dest).map_err(io)?;
    Ok(fresh)
}

/// Verbleibende Sekunden (aufgerundet) bis `until`; `None` = kein Limit (mehr).
fn limit_remaining(until: Option<std::time::Instant>, now: std::time::Instant) -> Option<u64> {
    let left = until?.checked_duration_since(now)?;
    if left.is_zero() {
        return None;
    }
    Some(left.as_secs() + u64::from(left.subsec_nanos() > 0))
}

/// Schläge eines Titels: einmal geholt und neben der Vorratsdatei gemerkt (leere Antworten nicht).
fn cached_beats(store: &Path, fetch: impl FnOnce() -> Vec<f64>) -> Vec<f64> {
    let f = store.with_extension("beats.json");
    if let Some(b) = std::fs::read(&f).ok().and_then(|d| serde_json::from_slice::<Vec<f64>>(&d).ok()) {
        return b;
    }
    let b = fetch();
    if !b.is_empty() {
        if let Ok(d) = serde_json::to_vec(&b) {
            let _ = std::fs::write(&f, d);
        }
    }
    b
}

#[cfg(test)]
mod store_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_track_is_downloaded_once_and_copied_into_other_projects() {
        let t = tempfile::tempdir().unwrap();
        let store = es_store(&t.path().join("Vorrat"), "abc/123");
        assert!(store.ends_with("abc_123.mp3"));
        let n = Cell::new(0);
        let dl = |p: &Path| {
            n.set(n.get() + 1);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"mp3").map_err(|e| Error::from(e.to_string()))
        };
        // erstes Projekt: wird geladen
        let a = t.path().join("A-media/x.mp3");
        assert_eq!(fetch_track(&a, &store, None, dl), Ok(true));
        // zweites Projekt: kommt aus dem Vorrat, kein zweiter Abruf
        let b = t.path().join("B-media/x.mp3");
        assert_eq!(fetch_track(&b, &store, None, dl), Ok(false));
        assert_eq!((n.get(), std::fs::read(&b).unwrap()), (1, b"mp3".to_vec()));
        // schon im Projekt: nichts zu tun
        assert_eq!(fetch_track(&b, &store, None, dl), Ok(false));
        assert_eq!(n.get(), 1);
        assert_eq!(stored_ids(store.parent().unwrap()), HashSet::from(["abc_123".to_string()]));
        // Titel aus einer älteren Version (nur im Projekt A): füllt den Vorrat ohne Abruf
        let store2 = es_store(&t.path().join("Vorrat"), "alt");
        let c = t.path().join("C-media/y.mp3");
        assert_eq!(fetch_track(&c, &store2, Some(&a), dl), Ok(false));
        assert!(store2.is_file() && n.get() == 1);
        // gescheiterter Abruf hinterlässt nichts
        let store3 = es_store(&t.path().join("Vorrat"), "kaputt");
        let d = t.path().join("D-media/z.mp3");
        assert!(fetch_track(&d, &store3, None, |_| Err("offline".into())).is_err());
        assert!(!store3.exists() && !d.exists());
    }

    #[test]
    fn a_rate_limit_blocks_until_it_has_passed() {
        let t0 = std::time::Instant::now();
        let s = std::time::Duration::from_secs;
        assert_eq!(limit_remaining(None, t0), None);
        assert_eq!(limit_remaining(Some(t0 + s(10)), t0), Some(10));
        assert_eq!(limit_remaining(Some(t0 + s(10)), t0 + s(4) + std::time::Duration::from_millis(1)), Some(6));
        assert_eq!(limit_remaining(Some(t0 + s(10)), t0 + s(10)), None);
        assert_eq!(limit_remaining(Some(t0 + s(10)), t0 + s(11)), None);
    }

    #[test]
    fn beats_are_fetched_once_and_empty_answers_are_not_kept() {
        let t = tempfile::tempdir().unwrap();
        let store = t.path().join("a.mp3");
        let n = Cell::new(0);
        let empty = || {
            n.set(n.get() + 1);
            Vec::new()
        };
        assert!(cached_beats(&store, empty).is_empty() && cached_beats(&store, empty).is_empty());
        assert_eq!(n.get(), 2);
        let some = || {
            n.set(n.get() + 1);
            vec![0.5, 1.0]
        };
        assert_eq!(cached_beats(&store, some), [0.5, 1.0]);
        assert_eq!(cached_beats(&store, some), [0.5, 1.0]);
        assert_eq!(n.get(), 3);
    }
}
