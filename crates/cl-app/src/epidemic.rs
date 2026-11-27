//! Epidemic Sound integration in the UI: music in the Audio tab (discover, favorites,
//! „Liked“), sign-in under Settings → Plugins and accounts, usage report on export.
//! All networking runs in background threads; results come back through a channel.

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
        /// really downloaded (not from the store)
        fresh: bool,
    },
    LoggedIn(Result<cl_epidemic::Tokens, Error>),
    Reported(Result<usize, Error>),
    /// Updated tokens after a refresh in the background
    Tokens(Option<cl_epidemic::Tokens>),
}

pub struct EsState {
    pub cfg: Config,
    /// Downloaded files → track ID (for the usage report on export)
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
    /// The running download started in a project that has since been left: do not insert anymore
    pub stray: bool,
    /// Tracks already in the store (file names without extension)
    have: HashSet<String>,
    pub error: Option<String>,
    /// After 429: no search/loading more until then (preview and store keep running)
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

/// API URL: only in developer builds (debug or `CUTLINE_SHOTS`) from `CUTLINE_EPIDEMIC_BASE`, never from
/// the saved file – otherwise token and key would go to a foreign address.
fn dev_base(dev: bool, var: Option<String>) -> Option<String> {
    var.filter(|b| dev && !b.is_empty())
}

/// File with tokens and key: on Unix readable only by the owner (Windows: profile ACL)
fn write_private(p: &Path, data: String) -> std::io::Result<()> {
    paths::write_atomic(p, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
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
        // fixed, anonymous ID for x-partner-user-id (created once, then saved)
        let fresh = cfg.user_id.is_empty();
        cfg.ensure_user_id();
        let dev = cfg!(debug_assertions) || std::env::var_os("CUTLINE_SHOTS").is_some();
        cfg.base_url = dev_base(dev, std::env::var("CUTLINE_EPIDEMIC_BASE").ok());
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
            stray: false,
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
        let _ = write_private(&p, serde_json::to_string_pretty(&s).unwrap_or_default());
    }

    /// Background task with its own client (token refresh is reported back).
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

    /// Seconds until requests for search/list are allowed again (after 429).
    fn limit_left(&self) -> Option<u64> {
        limit_remaining(self.limited_until, std::time::Instant::now())
    }

    /// If a limit is pending, reports it (instead of requesting) and returns `true`.
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

    /// Reload after the credentials change.
    pub fn reset(&mut self) {
        self.loaded = false;
        self.results.clear();
        self.liked.clear();
        self.user = None;
        self.error = None;
        self.save();
    }
}

/// Open the browser with a URL.
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

    /// Take over results of the background tasks (every frame).
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
                    let stray = std::mem::take(&mut self.es.stray);
                    match res {
                        Ok(path) => {
                            self.es.downloads.insert(path.clone(), track.id.clone());
                            self.es.have.insert(store_name(&track.id));
                            self.es.save();
                            // into the library only on the first download (otherwise duplicates would arise)
                            if fresh && self.settings.lib_downloads {
                                self.library.import(crate::library::Kind::Sound, "Epidemic Sound", std::slice::from_ref(&path));
                            }
                            if stray {
                                // the track stays in the store but does not belong to the project open now
                                self.toast(format!("“{}” downloaded", track.title));
                            } else if let Some(id) = self.add_file_at(path, self.playhead, None) {
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
                                self.toast(format!("“{}” added", track.title));
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
                            self.toast("Connected to Epidemic Sound");
                            self.es.first_load(ctx);
                        }
                        Err(e) => self.toast_err(e.to_string()),
                    }
                }
                Msg::Reported(r) => {
                    if let Err(e) = r {
                        log::warn!("Epidemic usage report: {e}");
                    }
                }
                Msg::Tokens(t) => {
                    self.es.cfg.tokens = t;
                    self.es.save();
                }
            }
        }
    }

    /// Evaluate a background task's error. `true` = already reported (sign-in expired).
    /// Expired sign-in: discard tokens, state "not connected", exactly one message.
    /// Limit (429): remember until when no search/loading is sent.
    fn es_note(&mut self, e: &Error) -> bool {
        match e {
            Error::LoginExpired if self.es.cfg.mode == AuthMode::Connect => {
                // several concurrent requests fail together: only the first reports
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

    /// After a successful export: report the Epidemic tracks used.
    pub fn report_epidemic_export(&mut self, ctx: &egui::Context, p: &cl_core::Project) {
        if !self.epidemic_enabled() || !self.es.cfg.ready() {
            return;
        }
        let ids: Vec<String> = p
            .media
            .iter()
            .filter(|m| p.timeline.tracks.iter().any(|t| t.clips.iter().any(|c| matches!(&c.content, cl_core::ClipContent::Media { media, .. } if *media == m.id))))
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

    /// „Epidemic Sound“ section in the Audio tab.
    pub fn epidemic_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.es.cfg.ready() {
            theme::empty_state(
                ui,
                icon::MUSIC_NOTES,
                "Epidemic Sound isn't connected yet",
                "Sign in under Settings → Plugins and accounts",
            );
            ui.vertical_centered(|ui| {
                if ui.add(theme::primary("Connect …")).clicked() {
                    self.settings_open = true;
                    self.settings_section = crate::settings::Section::Integrations;
                }
            });
            return;
        }
        self.es.first_load(&ctx);
        // View
        let mut v = match self.es.view {
            View::Discover => 0,
            View::Favorites => 1,
            View::Liked => 2,
        };
        let connect = self.es.cfg.mode == AuthMode::Connect;
        let items: &[&str] = if connect {
            &["Discover", "Favorites", "Liked"]
        } else {
            &["Discover", "Favorites"]
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
                            "{}  Title, mood, genre …",
                            icon::MAGNIFYING_GLASS
                        )))
                        .desired_width(ui.available_width() - 34.0),
                );
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.es.search(&ctx, false);
                }
                if theme::icon_button(ui, icon::MAGNIFYING_GLASS, "Search", !self.es.busy, false)
                    .clicked()
                {
                    self.es.search(&ctx, false);
                }
            });
            // Filter: moods and genres as chips, instrumental
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
                // Placeholders at row size until the first tracks are there
                for i in 0..6 {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), egui::Sense::hover());
                    widgets::skeleton(ui.painter(), egui::Id::new(("es-skel", i)), r.shrink2(vec2(0.0, 2.0)), theme::R_CARD);
                }
            } else if self.es.view == View::Favorites {
                theme::empty_state(
                    ui,
                    icon::STAR,
                    "No favorites yet",
                    "Mark tracks with the star",
                );
            } else {
                theme::empty_state(
                    ui,
                    icon::MUSIC_NOTES,
                    "No tracks found",
                    "Try another search or fewer filters",
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
                            .add_enabled(!self.es.busy, theme::ghost("Load more"))
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
                if playing { "Stop" } else { "Preview~listen" },
                !loading,
                playing,
            )
            .clicked()
            {
                if playing {
                    self.sfx_preview = None;
                } else {
                    self.es.stray = false; self.es.loading_track = Some(t.id.clone());
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
                // leave room for star and plus on the right, otherwise the bar grows every frame
                let sp = ui.spacing().item_spacing.x;
                ui.set_width((ui.available_width() - 2.0 * 24.0 - 3.0 * sp - 6.0).max(60.0));
                ui.add(egui::Label::new(RichText::new(&t.title).size(theme::FS_TEXT)).truncate());
                ui.add(
                    egui::Label::new(RichText::new(info).size(theme::FS_HINT).color(theme::TEXT_DIM()))
                        .truncate(),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let add = theme::icon_button(ui, icon::PLUS, "Insert at playhead", !loading, false).clicked();
                if self.es.have.contains(&store_name(&t.id)) {
                    ui.label(RichText::new(icon::CHECK_CIRCLE).size(theme::FS_HINT).color(theme::TEXT_DIM()))
                        .on_hover_text(crate::tr("Available locally"));
                }
                if add {
                    // According to Epidemic, `isPreviewOnly` is not a reliable sign: always try – if the
                    // track is not in the plan, the service refuses (403) and that then shows in the message
                    {
                        self.es.stray = false; self.es.loading_track = Some(t.id.clone());
                        let track = t.clone();
                        let (dest, ours) = track_dest(
                            &self.es_media_dir(),
                            &format!("{} - {}", t.artists(), t.title),
                            &t.id,
                            &self.es.downloads,
                        );
                        let store = es_store(&es_store_dir(), &t.id);
                        // an earlier downloaded file of the same track (other project) as the source for the store
                        let known = self
                            .es
                            .downloads
                            .iter()
                            .find(|(p, id)| **id == t.id && p.is_file())
                            .map(|(p, _)| p.clone());
                        self.es.spawn(ctx, move |c| {
                            let res = fetch_track(&dest, ours, &store, known.as_deref(), |p| c.download(&track.id, p));
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
                        "Remove from favorites"
                    } else {
                        "Add to favorites"
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

    /// „Plugins and accounts“ section in the settings.
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
        theme::row(ui, "Sign-in", |ui| {
            if widgets::segmented(ui, &["Epidemic account", "API key"], &mut mode) {
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
                theme::row(ui, "Redirect URI", |ui| ui.monospace(cl_epidemic::redirect_uri()));
                ui.add_space(4.0);
                if let Some(t) = self.es.cfg.tokens.clone() {
                    theme::row(ui, "Status", |ui| {
                        ui.label(
                            RichText::new(crate::tr(format!(
                                "{}  Connected{}",
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
                    if ui.add(theme::secondary("Sign out")).clicked() {
                        let (cid, rt) = (self.es.cfg.client_id.clone(), t.refresh_token.clone());
                        std::thread::spawn(move || cl_epidemic::logout(&cid, &rt));
                        self.es.cfg.tokens = None;
                        self.es.reset();
                    }
                } else if self.es.login_running {
                    ui.label(
                        RichText::new(crate::tr("Finish signing in in your browser …"))
                            .color(theme::TEXT_DIM()),
                    );
                    widgets::busy_row(ui, egui::Id::new("es-login"), true);
                } else if ui
                    .add_enabled(
                        !self.es.cfg.client_id.trim().is_empty(),
                        theme::primary(format!("{}  Connect to Epidemic Sound", icon::SIGN_IN)),
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
                theme::row(ui, "API key", |ui| {
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
        ui.label(RichText::new(crate::tr("Exports with Epidemic music are reported to Epidemic Sound (track IDs only).")).small().color(theme::TEXT_DIM()));
    }

    /// Switch a plugin on/off and remember it permanently.
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

/// Central store of downloaded tracks (one file per track): a track is downloaded only once and
/// copied into every further project. Not in the cache, so that „Clear cache“ does not remove it.
fn es_store_dir() -> PathBuf {
    paths::data_dir().join("Epidemic Sound").join(cl_core::legacy::TRACKS_DIR)
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

/// Target file in the project folder and whether it provably (fully downloaded) belongs to `id`. The old name
/// "Artist - Title.mp3" only stays for files that already belong to this ID (older projects); otherwise
/// the track ID goes into the name, so that two tracks with the same name do not share a file.
fn track_dest(dir: &Path, name: &str, id: &str, downloads: &BTreeMap<PathBuf, String>) -> (PathBuf, bool) {
    let safe: String = name.chars().map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let old = dir.join(format!("{safe}.mp3"));
    if downloads.get(&old).is_some_and(|i| i == id) {
        return (old, true);
    }
    let new = dir.join(format!("{safe} [{}].mp3", store_name(id)));
    let ours = downloads.get(&new).is_some_and(|i| i == id);
    (new, ours)
}

/// Copies via `<target>.part` + rename: an aborted copy leaves no half file behind.
fn copy_whole(from: &Path, to: &Path) -> Result<(), Error> {
    let io = |e: std::io::Error| Error::from(e.to_string());
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d).map_err(io)?;
    }
    let tmp = to.with_extension("part");
    if let Err(e) = std::fs::copy(from, &tmp).and_then(|_| std::fs::rename(&tmp, to)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(e));
    }
    Ok(())
}

/// Puts the track at `dest`. Order: already there and belongs to this track (`ours`) →
/// nothing; store → copy; earlier downloaded file (`known`) → take it into the store and copy;
/// otherwise download. `true` = downloaded.
fn fetch_track(
    dest: &Path,
    ours: bool,
    store: &Path,
    known: Option<&Path>,
    download: impl FnOnce(&Path) -> Result<(), Error>,
) -> Result<bool, Error> {
    let len = |p: &Path| std::fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| m.len());
    // a project file only counts if it belongs to this ID and (if in the store) has the same size
    if ours && len(dest).is_some() && (len(store).is_none() || len(store) == len(dest)) {
        if len(store).is_none() {
            let _ = copy_whole(dest, store);
        }
        return Ok(false);
    }
    let mut fresh = false;
    if !store.is_file() {
        match known.filter(|k| k.is_file()) {
            Some(k) => copy_whole(k, store)?,
            None => {
                download(store)?;
                fresh = true;
            }
        }
    }
    copy_whole(store, dest)?;
    Ok(fresh)
}

/// Remaining seconds (rounded up) until `until`; `None` = no limit (anymore).
fn limit_remaining(until: Option<std::time::Instant>, now: std::time::Instant) -> Option<u64> {
    let left = until?.checked_duration_since(now)?;
    if left.is_zero() {
        return None;
    }
    Some(left.as_secs() + u64::from(left.subsec_nanos() > 0))
}

/// Beats of a track: fetched once and remembered next to the store file (empty replies are not).
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
        let store = es_store(&t.path().join("Tracks"), "abc/123");
        assert!(store.ends_with("abc_123.mp3"));
        let n = Cell::new(0);
        let dl = |p: &Path| {
            n.set(n.get() + 1);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"mp3").map_err(|e| Error::from(e.to_string()))
        };
        // first project: gets downloaded
        let a = t.path().join("A-media/x.mp3");
        assert_eq!(fetch_track(&a, false, &store, None, dl), Ok(true));
        // second project: comes from the store, no second fetch
        let b = t.path().join("B-media/x.mp3");
        assert_eq!(fetch_track(&b, true, &store, None, dl), Ok(false));
        assert_eq!((n.get(), std::fs::read(&b).unwrap()), (1, b"mp3".to_vec()));
        // already in the project: nothing to do
        assert_eq!(fetch_track(&b, true, &store, None, dl), Ok(false));
        assert_eq!(n.get(), 1);
        assert_eq!(stored_ids(store.parent().unwrap()), HashSet::from(["abc_123".to_string()]));
        // track from an older version (only in project A): fills the store without a fetch
        let store2 = es_store(&t.path().join("Tracks"), "old");
        let c = t.path().join("C-media/y.mp3");
        assert_eq!(fetch_track(&c, false, &store2, Some(&a), dl), Ok(false));
        assert!(store2.is_file() && n.get() == 1);
        // a failed fetch leaves nothing behind
        let store3 = es_store(&t.path().join("Tracks"), "broken");
        let d = t.path().join("D-media/z.mp3");
        assert!(fetch_track(&d, false, &store3, None, |_| Err("offline".into())).is_err());
        assert!(!store3.exists() && !d.exists());
    }

    #[test]
    fn a_half_or_foreign_project_file_is_never_taken_as_the_track() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("media");
        let store_dir = t.path().join("Tracks");
        let dl = |b: &'static [u8]| move |p: &Path| std::fs::write(p, b).map_err(|e| Error::from(e.to_string()));
        let _ = std::fs::create_dir_all(&store_dir);
        // two versions with the same "Artist - Title" get different files
        let mut downloads = BTreeMap::new();
        let (d1, ours1) = track_dest(&dir, "A - Song", "id-1", &downloads);
        assert!(!ours1);
        assert_eq!(fetch_track(&d1, ours1, &es_store(&store_dir, "id-1"), None, dl(b"four")), Ok(true));
        downloads.insert(d1.clone(), "id-1".to_string());
        let (d2, ours2) = track_dest(&dir, "A - Song", "id-2", &downloads);
        assert!(d1 != d2 && !ours2);
        assert_eq!(fetch_track(&d2, ours2, &es_store(&store_dir, "id-2"), None, dl(b"five")), Ok(true));
        assert_eq!(std::fs::read(&d2).unwrap(), b"five");
        assert_eq!(std::fs::read(es_store(&store_dir, "id-2")).unwrap(), b"five");
        assert_eq!(track_dest(&dir, "A - Song", "id-1", &downloads), (d1.clone(), true));
        // old projects: the former name still applies if it belongs to this ID
        let old = dir.join("A - Song.mp3");
        downloads.insert(old.clone(), "id-1".to_string());
        assert_eq!(track_dest(&dir, "A - Song", "id-1", &downloads), (old.clone(), true));
        assert_eq!(track_dest(&dir, "A - Song", "id-2", &downloads).0, d2);
        // a half file (other project, aborted copy) is replaced, not taken over
        std::fs::write(&old, b"ei").unwrap();
        assert_eq!(fetch_track(&old, true, &es_store(&store_dir, "id-1"), None, dl(b"x")), Ok(false));
        assert_eq!(std::fs::read(&old).unwrap(), b"four");
        // a foreign file under the target name never fills the store
        let d3 = dir.join("B - Song [id-3].mp3");
        std::fs::write(&d3, b"other").unwrap();
        assert_eq!(fetch_track(&d3, false, &es_store(&store_dir, "id-3"), None, dl(b"nine")), Ok(true));
        assert_eq!(std::fs::read(&d3).unwrap(), b"nine");
        assert!(!d3.with_extension("part").exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_token_file_is_only_readable_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("epidemic-sound.json");
        write_private(&p, "{\"api_key\":\"epidemic_live_TEST\"}".into()).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn the_api_address_is_only_redirected_in_dev_builds() {
        let b = || Some("http://127.0.0.1:9".to_string());
        assert_eq!(dev_base(false, b()), None);
        assert_eq!(dev_base(true, b()), b());
        assert_eq!(dev_base(true, Some(String::new())), None);
        assert_eq!(dev_base(true, None), None);
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
