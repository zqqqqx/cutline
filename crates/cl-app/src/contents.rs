//! Einstellungen → Inhalte: alle Pakete an einer Stelle (Grundausstattung und Asset-Pakete),
//! ein-/ausschalten, hinzufügen (ZIP), entfernen, Quellen und Lizenzen.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use cl_fx::catalog::{self, Entry, Kind, Pack, PackInfo, Payload};
use cl_update::packs::{self, IndexError, IndexPack, Source};
use eframe::egui::{self, vec2, RichText};

use crate::app::App;
use crate::paths;
use crate::theme::{self, icon};
use crate::timeline::menu_item;
use crate::widgets;

/// Rückfrage bzw. Fenster der Seite „Inhalte“.
#[derive(Debug, Clone)]
pub enum ContentsDialog {
    /// Paket gleicher ID ist schon installiert: ersetzen?
    Replace { zip: PathBuf, id: String },
    /// Eigenes Paket löschen?
    Remove { dir: PathBuf, name: String },
    /// Danksagung (Markdown)
    Credits(String),
}

/// Anzeigename eines Medienpfads, den Cutline selbst angelegt hat oder der aus einem Paket kommt:
/// Standbild, eingebautes Element/Geräusch (Datei heißt nach der Kennung) oder Katalog-Eintrag.
/// `None` für eigene Dateien (dann bleibt der Dateiname).
fn media_title(path: &Path, data: &Path, pack: impl Fn(&Path) -> Option<String>) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let dir = path.parent()?;
    if dir == data.join("Freeze") {
        Some("Standbild".into())
    } else if dir == data.join("Stickers") {
        cl_fx::stickers::sticker(stem).map(|s| s.name.to_string())
    } else if dir == data.join("Sfx") {
        cl_fx::sfx::sfx(stem.rsplit_once('-')?.0).map(|s| s.name.to_string())
    } else {
        pack(path)
    }
}

/// Paketdateien → Name (wird bei `refresh_catalog` geleert)
static PACK_NAMES: std::sync::Mutex<Option<std::collections::HashMap<PathBuf, String>>> = std::sync::Mutex::new(None);

/// Name eines Clips für Zeitleiste und Inspektor (übersetzt): Katalogname bzw. „Standbild“ statt
/// technischer Dateinamen, sonst wie [`cl_core::Clip::label`].
pub fn clip_title(c: &cl_core::Clip, p: &cl_core::Project) -> String {
    if let cl_core::ClipContent::Media { media, .. } = &c.content {
        if let Some(m) = p.media(*media) {
            let pack = |path: &Path| {
                let mut g = PACK_NAMES.lock().unwrap_or_else(|e| e.into_inner());
                g.get_or_insert_with(|| {
                    [Kind::Sticker, Kind::Sound]
                        .into_iter()
                        .flat_map(catalog::list_all)
                        .filter_map(|e| match e.payload {
                            Payload::File(f) => Some((f, e.name)),
                            _ => None,
                        })
                        .collect()
                })
                .get(path)
                .cloned()
            };
            if let Some(t) = media_title(&m.path, &paths::data_dir(), pack) {
                return crate::tr(t);
            }
        }
    }
    c.label(p)
}

/// Ordner für eigene Pakete
pub fn user_root() -> PathBuf {
    paths::data_dir().join("Plugins")
}

fn localized_or_empty(t: &cl_plugin::LocalizedText, lang: &str) -> String {
    if t.is_empty() {
        String::new()
    } else {
        cl_plugin::localized(t, lang).to_string()
    }
}

/// Katalog-Pakete aus den geladenen Asset-Paketen (auch ausgeschaltete, mit Schalterstand).
/// IDs der Einträge: `<paket-id>/<asset-id>` (wie bisher bei Plugin-Effekten).
pub fn catalog_packs(reg: &cl_plugin::Registry, lang: &str) -> Vec<Pack> {
    use cl_plugin::AssetKind as A;
    reg.plugins
        .iter()
        .filter(|p| p.manifest.plugin.kind == cl_plugin::PluginKind::AssetPack)
        .map(|p| {
            let m = &p.manifest.plugin;
            let info = PackInfo {
                id: m.id.clone(),
                name: cl_plugin::localized(&m.name, lang).to_string(),
                version: m.version.clone(),
                description: localized_or_empty(&m.description, lang),
                license: m.license.clone(),
                author: m.authors.join(", "),
                source: m.source.clone(),
            };
            let entries = p
                .manifest
                .assets
                .iter()
                .filter_map(|a| {
                    let file = || a.file.as_ref().map(|f| Payload::File(p.dir.join(f)));
                    let chain = || {
                        a.ffmpeg_filled()
                            .filter(|c| cl_plugin::ffmpeg_chain_allowed(c))
                            .map(Payload::Ffmpeg)
                    };
                    let (kind, payload) = match a.kind {
                        A::VideoEffect => (Kind::Effect, chain()?),
                        A::Filter => (Kind::Filter, chain()?),
                        A::Sound => (Kind::Sound, file()?),
                        A::Sticker => (Kind::Sticker, file()?),
                        A::Transition => (
                            Kind::Transition,
                            Payload::Shader {
                                file: p.dir.join(a.shader.as_ref()?),
                                // unbekannter Rückfall: Überblenden (der Katalog prüft noch einmal)
                                fallback: a
                                    .fallback
                                    .clone()
                                    .filter(|f| cl_fx::TRANSITIONS.iter().any(|t| t.0 == f))
                                    .unwrap_or_else(|| "fade".into()),
                                params: a.shader_params().ok()?,
                            },
                        ),
                        // shortcut: Shader-Assets und weitere Arten erscheinen erst mit ihrer Render-Stufe
                        _ => return None,
                    };
                    Some(Entry {
                        id: format!("{}/{}", m.id, a.id),
                        kind,
                        name: cl_plugin::localized(&a.name, lang).to_string(),
                        // ohne Kategorie: eigener Ordner mit dem Paketnamen
                        categories: if a.category.is_empty() {
                            vec![info.name.clone()]
                        } else {
                            a.category.clone()
                        },
                        pack: m.id.clone(),
                        license: a.license.clone().unwrap_or_else(|| info.license.clone()),
                        author: a.author.clone().unwrap_or_else(|| info.author.clone()),
                        source: a.source.clone().or_else(|| info.source.clone()),
                        tags: a.tags.clone(),
                        payload,
                    })
                })
                .collect();
            Pack {
                info,
                enabled: p.enabled,
                entries,
            }
        })
        .collect()
}

/// „12 Effekte · 3 Sounds“
pub fn counts_text(counts: &BTreeMap<Kind, usize>) -> String {
    counts
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(k, n)| {
            let one = *n == 1;
            crate::tr(match k {
                Kind::Effect if one => format!("{n} Effekt"),
                Kind::Effect => format!("{n} Effekte"),
                // „1 Filter“/„1 Sticker“: eigene Wörterbuchzeilen (deutsch gleich, englisch Einzahl)
                Kind::Filter => format!("{n} Filter"),
                Kind::Transition if one => format!("{n} Übergang"),
                Kind::Transition => format!("{n} Übergänge"),
                Kind::Sound if one => format!("{n} Sound"),
                Kind::Sound => format!("{n} Sounds"),
                Kind::Sticker => format!("{n} Sticker"),
            })
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Zähler der Liste (`"sound": 213`) → Arten des Katalogs; Unbekanntes fällt weg.
fn index_counts(c: &BTreeMap<String, usize>) -> BTreeMap<Kind, usize> {
    c.iter()
        .filter_map(|(k, n)| {
            let k = match k.as_str() {
                "effect" => Kind::Effect,
                "filter" => Kind::Filter,
                "transition" => Kind::Transition,
                "sound" => Kind::Sound,
                "sticker" => Kind::Sticker,
                _ => return None,
            };
            Some((k, *n))
        })
        .collect()
}

/// „22 KB“ bzw. „1,3 MB“
fn size_text(bytes: u64, comma: bool) -> String {
    if bytes < 1 << 20 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        crate::update::mb_text(bytes, comma)
    }
}

/// „42 % · 0,6 / 1,4 MB“ bzw. „50 % · 11 / 22 KB“
fn progress_text(done: u64, total: u64, comma: bool) -> String {
    let total = total.max(1);
    let done = done.min(total);
    if total < 1 << 20 {
        format!("{} % · {} / {} KB", done * 100 / total, done.div_ceil(1024), total.div_ceil(1024))
    } else {
        crate::update::progress_text(done, total, comma)
    }
}

/// Quelle der Pakete: mit `CUTLINE_NO_UPDATE` keine; in Entwickler-Builds (Debug oder
/// `CUTLINE_SHOTS`) ersetzt `CUTLINE_PACKS_DIR=<Ordner mit index.json und ZIPs>` die Adresse.
fn source_from(dev: bool, dir: Option<std::ffi::OsString>, no_update: bool) -> Option<Source> {
    match dir.filter(|d| dev && !d.is_empty()) {
        Some(d) => Some(Source::Dir(d.into())),
        None => (!no_update).then_some(Source::Github),
    }
}

fn source() -> Option<Source> {
    let dev = cfg!(debug_assertions) || std::env::var_os("CUTLINE_SHOTS").is_some();
    source_from(dev, std::env::var_os("CUTLINE_PACKS_DIR"), std::env::var_os("CUTLINE_NO_UPDATE").is_some())
}

/// Zwischendateien der Downloads (Reste beendeter Läufe räumt der Start weg)
pub fn download_dir() -> PathBuf {
    paths::data_dir().join("cache").join("downloads")
}

/// Laden, prüfen, nach `root` einbauen. Die Zwischendatei verschwindet in jedem Fall.
fn fetch_install(src: &Source, p: &IndexPack, tmp: &Path, root: &Path, progress: impl FnMut(u64, u64), cancel: &AtomicBool) -> Result<(), String> {
    let f = packs::download_pack(src, p, tmp, progress, cancel)?;
    let r = cl_plugin::install_zip_id(&f, root, &p.id);
    let _ = std::fs::remove_file(&f);
    r
}

/// Liste der nachladbaren Pakete
#[derive(Debug, Clone, Default)]
pub enum ListState {
    #[default]
    Idle,
    Loading,
    /// Nachladen ausgeschaltet (`CUTLINE_NO_UPDATE`)
    Off,
    Ready(Vec<IndexPack>),
    /// Unbekanntes Listenformat
    Newer,
    Failed(String),
}

#[derive(Debug, Clone, Default)]
struct Shared {
    list: ListState,
    queue: VecDeque<IndexPack>,
    /// laufender Download: ID, Bytes, Gesamt
    active: Option<(String, u64, u64)>,
    /// eingebaut, Katalog noch nicht neu eingelesen
    installed: Vec<IndexPack>,
    errors: BTreeMap<String, String>,
    worker: bool,
}

/// Liste und Download-Warteschlange (nacheinander, ein Hintergrund-Thread)
#[derive(Default)]
pub struct PackDownloads {
    shared: Arc<Mutex<Shared>>,
    cancel: Arc<AtomicBool>,
}

fn lock(s: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

impl PackDownloads {
    fn snapshot(&self) -> Shared {
        lock(&self.shared).clone()
    }

    fn fetch(&self, ctx: &egui::Context) {
        let Some(src) = source() else {
            lock(&self.shared).list = ListState::Off;
            return;
        };
        {
            let mut s = lock(&self.shared);
            if matches!(s.list, ListState::Loading) {
                return;
            }
            s.list = ListState::Loading;
        }
        let (shared, ctx) = (self.shared.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = match packs::fetch_index(&src) {
                Ok(v) => ListState::Ready(v),
                Err(IndexError::Newer) => ListState::Newer,
                Err(IndexError::Unreachable(e) | IndexError::Broken(e)) => ListState::Failed(cl_update::friendly(&crate::applog::warned("paket", e))),
            };
            lock(&shared).list = r;
            ctx.request_repaint();
        });
    }

    fn enqueue(&self, p: IndexPack, ctx: &egui::Context) {
        let mut s = lock(&self.shared);
        if s.queue.iter().any(|q| q.id == p.id) || s.active.as_ref().is_some_and(|a| a.0 == p.id) {
            return;
        }
        s.errors.remove(&p.id);
        s.queue.push_back(p);
        if std::mem::replace(&mut s.worker, true) {
            return;
        }
        drop(s);
        let (shared, cancel, ctx) = (self.shared.clone(), self.cancel.clone(), ctx.clone());
        std::thread::spawn(move || loop {
            let p = {
                let mut s = lock(&shared);
                let Some(p) = s.queue.pop_front() else {
                    s.worker = false;
                    return;
                };
                s.active = Some((p.id.clone(), 0, p.size));
                cancel.store(false, Ordering::Relaxed);
                p
            };
            ctx.request_repaint();
            let progress = |d, t| {
                lock(&shared).active = Some((p.id.clone(), d, t));
                ctx.request_repaint();
            };
            let r = match source() {
                Some(src) => fetch_install(&src, &p, &download_dir(), &user_root(), progress, &cancel),
                None => Err("Nachladen ist ausgeschaltet".into()),
            };
            let mut s = lock(&shared);
            s.active = None;
            match r {
                Ok(()) => s.installed.push(p),
                Err(_) if cancel.load(Ordering::Relaxed) => {}
                Err(e) => drop(s.errors.insert(p.id.clone(), cl_update::friendly(&crate::applog::warned("paket", e)))),
            }
            drop(s);
            ctx.request_repaint();
        });
    }

    fn cancel(&self, id: &str) {
        let mut s = lock(&self.shared);
        if s.active.as_ref().is_some_and(|a| a.0 == id) {
            self.cancel.store(true, Ordering::Relaxed);
        } else {
            s.queue.retain(|q| q.id != id);
        }
    }
}

/// Knopf einer Karte: was geklickt wurde
enum PackAction {
    Load(IndexPack),
    Cancel(String),
}

/// Rechter Knopf: „Laden“/„Aktualisieren“, während Warteschlange oder Download „Abbrechen“.
fn pack_button(ui: &mut egui::Ui, snap: &Shared, p: &IndexPack, label: String) -> Option<PackAction> {
    let busy = snap.queue.iter().any(|q| q.id == p.id) || snap.active.as_ref().is_some_and(|a| a.0 == p.id);
    if busy {
        return ui.add(theme::secondary("Abbrechen")).clicked().then(|| PackAction::Cancel(p.id.clone()));
    }
    let ok = p.api == cl_plugin::API_VERSION;
    ui.add_enabled(ok, theme::secondary(label)).clicked().then(|| PackAction::Load(p.clone()))
}

/// Zeilen unter einer Karte: Fortschritt, Warten, Fehler
fn job_lines(ui: &mut egui::Ui, snap: &Shared, id: &str) {
    let comma = cl_i18n::ui_lang() == cl_i18n::Lang::De;
    if let Some((_, done, total)) = snap.active.as_ref().filter(|a| a.0 == id) {
        ui.add_space(4.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
        widgets::progress_line(ui.painter(), ui.id().with(("pack", id)), r, (*done as f64 / (*total).max(1) as f64) as f32);
        ui.label(RichText::new(progress_text(*done, *total, comma)).small().color(theme::TEXT_DIM()));
    } else if snap.queue.iter().any(|q| q.id == id) {
        ui.label(RichText::new(crate::tr("Wartet …")).small().color(theme::TEXT_DIM()));
    } else if let Some(e) = snap.errors.get(id) {
        ui.label(RichText::new(format!("{}  {}", icon::WARNING_CIRCLE, crate::tr(e))).small().color(theme::WARN_TEXT()));
    }
}

impl App {
    /// Fertige Downloads übernehmen: Katalog neu einlesen, Meldung
    pub fn poll_pack_downloads(&mut self) {
        let done = std::mem::take(&mut lock(&self.packs.shared).installed);
        if done.is_empty() {
            return;
        }
        self.reload_plugins();
        let lang = self.i18n.lang().code();
        for p in done {
            let name = packs::text(&p.name, lang, &p.id).to_string();
            self.toast(format!("{}  Paket „{name}“ installiert", icon::CHECK));
        }
    }

    /// Abschnitt „Verfügbar“: Pakete der Liste, die nicht installiert sind
    fn available_packs(&mut self, ui: &mut egui::Ui, snap: &Shared, avail: &[&IndexPack]) {
        ui.add_space(6.0);
        let r = ui.available_rect_before_wrap();
        ui.painter().line_segment([r.left_top(), r.right_top()], egui::Stroke::new(1.0_f32, theme::BORDER()));
        ui.add_space(9.0);
        let loading = matches!(snap.list, ListState::Idle | ListState::Loading);
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::tr("Verfügbar")).size(theme::FS_UI).color(theme::TEXT()).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, "Liste neu laden", !loading, false).clicked() {
                    self.packs.fetch(ui.ctx());
                }
            });
        });
        ui.add_space(3.0);
        let skeleton = theme::loading(ui.ctx(), ui.id().with("packs-list"), loading);
        if loading || skeleton {
            for i in 0..3 {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), egui::Sense::hover());
                if skeleton {
                    widgets::skeleton(ui.painter(), ui.id().with(("pack-skeleton", i)), r, theme::R_CARD);
                }
                ui.add_space(6.0);
            }
            return;
        }
        let mut action = None;
        match &snap.list {
            ListState::Off => theme::empty_state(ui, icon::CLOUD_SLASH, "Nachladen ist ausgeschaltet", ""),
            ListState::Newer => {
                ui.label(RichText::new(format!("{}  {}", icon::WARNING_CIRCLE, crate::tr("Neuere Version nötig"))).small().color(theme::WARN_TEXT()));
            }
            ListState::Failed(e) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{}  {}", icon::WARNING_CIRCLE, crate::tr("Liste nicht erreichbar"))).small().color(theme::WARN_TEXT()))
                        .on_hover_text(crate::tr(e));
                    if ui.add(theme::ghost(format!("{}  Erneut versuchen", icon::ARROWS_CLOCKWISE))).clicked() {
                        self.packs.fetch(ui.ctx());
                    }
                });
            }
            ListState::Ready(_) if avail.is_empty() => theme::empty_state(ui, icon::CHECK_CIRCLE, "Alle Pakete sind installiert", ""),
            _ => {
                let lang = self.i18n.lang().code();
                let comma = cl_i18n::ui_lang() == cl_i18n::Lang::De;
                for p in avail {
                    egui::Frame::default()
                        .fill(theme::BG_RAISED())
                        .corner_radius(theme::R_CARD)
                        .inner_margin(theme::PAD_CARD_L)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(packs::text(&p.name, lang, &p.id)).strong());
                                let meta = if p.license.is_empty() { format!("v{}", p.version) } else { format!("v{} · {}", p.version, p.license) };
                                ui.label(RichText::new(meta).small().color(theme::TEXT_DIM()));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let label = format!("{}  {}", icon::DOWNLOAD_SIMPLE, crate::tr("Laden"));
                                    action = action.take().or(pack_button(ui, snap, p, label));
                                });
                            });
                            let desc = packs::text(&p.description, lang, "");
                            if !desc.is_empty() {
                                ui.label(RichText::new(desc).small().color(theme::TEXT_DIM()));
                            }
                            let counts = counts_text(&index_counts(&p.counts));
                            let size = size_text(p.size, comma);
                            let line = if counts.is_empty() { size } else { format!("{counts} · {size}") };
                            ui.label(RichText::new(line).small().color(theme::TEXT_DIM()));
                            if p.api > cl_plugin::API_VERSION {
                                ui.label(RichText::new(crate::tr("Braucht eine neuere Version von Cutline")).small().color(theme::WARN_TEXT()));
                            }
                            job_lines(ui, snap, &p.id);
                        });
                    ui.add_space(6.0);
                }
            }
        }
        self.pack_action(action, ui.ctx());
    }

    fn pack_action(&self, a: Option<PackAction>, ctx: &egui::Context) {
        match a {
            Some(PackAction::Load(p)) => self.packs.enqueue(p, ctx),
            Some(PackAction::Cancel(id)) => self.packs.cancel(&id),
            None => {}
        }
    }

    /// Katalog neu anmelden (nach Ein-/Ausschalten, Sprachwechsel, neuen Paketen).
    pub fn refresh_catalog(&mut self) {
        catalog::load_packs(catalog_packs(&self.plugins, self.i18n.lang().code()));
        catalog::set_hidden(self.settings.hidden_packs.clone());
        *PACK_NAMES.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.invalidate_preview();
        self.prewarm_sounds();
    }

    /// Plugin-Ordner neu einlesen (nach Hinzufügen oder Entfernen).
    fn reload_plugins(&mut self) {
        let dirs = paths::plugin_dirs();
        self.plugins = cl_plugin::Registry::scan(dirs.iter().map(|p| p.as_path()));
        self.plugins.apply_states(&self.settings.plugin_states);
        self.refresh_catalog();
    }

    fn add_pack(&mut self, zip: &Path, replace: bool) {
        let root = user_root();
        match cl_plugin::install_zip(zip, &root, replace) {
            Ok(cl_plugin::Installed::Exists(id)) => {
                self.contents_dialog = Some(ContentsDialog::Replace {
                    zip: zip.to_path_buf(),
                    id,
                });
            }
            Ok(cl_plugin::Installed::Done(id)) => {
                // ein mitgeliefertes Paket gleicher ID gewinnt beim Laden: dann nicht behalten
                let bundled = self
                    .plugins
                    .plugins
                    .iter()
                    .any(|p| p.manifest.plugin.id == id && !p.dir.starts_with(&root));
                if bundled {
                    let _ = cl_plugin::remove_pack(&root, &root.join(&id));
                    self.settings_error = Some(crate::tr(format!("„{id}“ ist schon mitgeliefert")));
                    return;
                }
                self.reload_plugins();
                self.toast(format!("{}  Paket „{id}“ hinzugefügt", icon::CHECK));
            }
            Err(e) => self.settings_error = Some(crate::applog::warned("paket", e)),
        }
    }

    pub fn contents_settings(&mut self, ui: &mut egui::Ui) {
        let root = user_root();
        let snap = self.packs.snapshot();
        if matches!(snap.list, ListState::Idle) {
            self.packs.fetch(ui.ctx());
        }
        let installed: Vec<packs::Installed> = self
            .plugins
            .plugins
            .iter()
            .map(|p| packs::Installed { id: &p.manifest.plugin.id, version: &p.manifest.plugin.version, own: p.dir.starts_with(&root) })
            .collect();
        let index = match &snap.list {
            ListState::Ready(v) => v.as_slice(),
            _ => &[],
        };
        let (avail, updates) = packs::plan(index, &installed, cl_plugin::API_VERSION, self.i18n.lang().code());
        drop(installed);
        let mut action = None;
        let mut toggled = None;
        let mut hide = None;
        let mut remove = None;
        for s in catalog::packs() {
            let standard = s.info.id == catalog::STANDARD;
            let hidden = self.settings.hidden_packs.contains(&s.info.id);
            let own = self
                .plugins
                .plugins
                .iter()
                .find(|p| p.manifest.plugin.id == s.info.id)
                .filter(|p| p.dir.starts_with(&root))
                .map(|p| p.dir.clone());
            let mut on = s.enabled;
            egui::Frame::default()
                .fill(theme::BG_RAISED())
                .corner_radius(theme::R_CARD)
                .inner_margin(theme::PAD_CARD_L)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if !on {
                        ui.set_opacity(0.45);
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(crate::tr(&s.info.name)).strong());
                        ui.label(
                            RichText::new(format!("v{} · {}", s.info.version, s.info.license))
                                .small()
                                .color(theme::TEXT_DIM()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.set_opacity(1.0);
                            if !standard && widgets::toggle(ui, &mut on) {
                                toggled = Some((s.info.id.clone(), on));
                            }
                            let (glyph, tip) = if hidden {
                                (icon::EYE_SLASH, "Ausgeblendet")
                            } else {
                                (icon::EYE, "In den Tabs anzeigen")
                            };
                            if theme::icon_button(ui, glyph, tip, on, false).clicked() {
                                hide = Some(s.info.id.clone());
                            }
                            if let Some(dir) = &own {
                                ui.menu_button(icon::DOTS_THREE, |ui| {
                                    if menu_item(ui, icon::TRASH, "Entfernen", "") {
                                        remove = Some((dir.clone(), s.info.name.clone()));
                                        ui.close_menu();
                                    }
                                });
                            }
                            if let Some(p) = updates.get(&s.info.id) {
                                let label = format!("{}  {}", icon::ARROW_CIRCLE_UP, crate::tr("Aktualisieren"));
                                action = action.take().or(pack_button(ui, &snap, p, label));
                            }
                        });
                    });
                    if !s.info.description.is_empty() {
                        ui.label(RichText::new(crate::tr(&s.info.description)).small().color(theme::TEXT_DIM()));
                    }
                    let mut counts = counts_text(&s.counts);
                    if hidden {
                        counts = format!("{counts} · {}", crate::tr("Ausgeblendet"));
                    }
                    ui.label(RichText::new(counts).small().color(theme::TEXT_DIM()));
                    if updates.contains_key(&s.info.id) {
                        job_lines(ui, &snap, &s.info.id);
                    }
                });
            ui.add_space(6.0);
        }
        self.pack_action(action, ui.ctx());
        if let Some((id, on)) = toggled {
            self.set_plugin_enabled(&id, on);
        }
        if let Some(id) = hide {
            let h = &mut self.settings.hidden_packs;
            match h.iter().position(|x| *x == id) {
                Some(i) => drop(h.remove(i)),
                None => h.push(id),
            }
            self.settings.save();
            catalog::set_hidden(self.settings.hidden_packs.clone());
        }
        if let Some((dir, name)) = remove {
            self.contents_dialog = Some(ContentsDialog::Remove { dir, name });
        }
        for e in &self.plugins.errors {
            ui.label(RichText::new(format!("{} {e}", icon::WARNING)).small().color(theme::WARN_TEXT()));
        }
        ui.horizontal_wrapped(|ui| {
            if ui.add(theme::secondary(format!("{}  Paket hinzufügen …", icon::PLUS))).clicked() {
                if let Some(f) = crate::dialog().add_filter("ZIP", &["zip"]).pick_file() {
                    self.add_pack(&f, false);
                }
            }
            if ui.add(theme::ghost(format!("{}  Paket-Ordner öffnen", icon::FOLDER_OPEN))).clicked() {
                let _ = std::fs::create_dir_all(&root);
                crate::home::reveal(&root);
            }
            if ui.add(theme::ghost(format!("{}  Quellen und Lizenzen", icon::INFO))).clicked() {
                let packs = catalog_packs(&self.plugins, self.i18n.lang().code());
                self.contents_dialog = Some(ContentsDialog::Credits(catalog::credits(&packs, &|s| crate::tr(s))));
            }
        });
        self.available_packs(ui, &snap, &avail);
    }

    /// Rückfragen und Danksagung über den Einstellungen.
    pub fn contents_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = self.contents_dialog.clone() else {
            return;
        };
        let (mut ok, mut close) = (false, false);
        let modal = crate::dialog::modal(ctx, "contents-dialog").show(ctx, |ui| match &d {
            ContentsDialog::Replace { id, .. } => {
                ui.set_width(340.0);
                close |= crate::dialog::title(ui, "Paket ersetzen?");
                ui.label(crate::tr(format!("„{id}“ ist schon installiert. Durch das neue Paket ersetzen?")));
                let (m, c) = crate::dialog::buttons(ui, theme::primary("Ersetzen"), true);
                ok |= m;
                close |= c;
            }
            ContentsDialog::Remove { name, .. } => {
                ui.set_width(340.0);
                close |= crate::dialog::title(ui, "Paket entfernen?");
                ui.label(crate::tr(format!(
                    "„{name}“ wird gelöscht. Projekte, die seine Inhalte nutzen, melden sie danach als fehlend."
                )));
                let (m, c) = crate::dialog::buttons(ui, theme::danger("Entfernen"), true);
                ok |= m;
                close |= c;
            }
            ContentsDialog::Credits(text) => {
                ui.set_width(520.0);
                close |= crate::dialog::title(ui, "Quellen und Lizenzen");
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    ui.label(RichText::new(text).monospace().size(theme::FS_HINT));
                });
                crate::dialog::footer(ui, |ui| {
                    if ui.add(theme::primary(format!("{}  Kopieren", icon::COPY))).clicked() {
                        ui.ctx().copy_text(text.clone());
                        ok = true;
                    }
                });
            }
        });
        if modal.should_close() {
            close = true;
        }
        if ok || close {
            self.contents_dialog = None;
        }
        if ok {
            match d {
                ContentsDialog::Replace { zip, .. } => self.add_pack(&zip, true),
                ContentsDialog::Remove { dir, .. } => match cl_plugin::remove_pack(&user_root(), &dir) {
                    Ok(()) => {
                        self.reload_plugins();
                        self.toast(format!("{}  Paket entfernt", icon::CHECK));
                    }
                    Err(e) => self.settings_error = Some(crate::applog::warned("paket", e)),
                },
                ContentsDialog::Credits(_) => self.toast(format!("{}  In die Zwischenablage kopiert", icon::CHECK)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_media_get_readable_titles() {
        let data = Path::new("C:/daten");
        let none = |_: &Path| None;
        assert_eq!(media_title(&data.join("Freeze").join("freeze-1794710580502.png"), data, none).as_deref(), Some("Standbild"));
        let st = &cl_fx::stickers::STICKERS[0];
        assert_eq!(media_title(&data.join("Stickers").join(format!("{}.png", st.id)), data, none).as_deref(), Some(st.name));
        assert_eq!(media_title(&data.join("Sfx").join("riser-0a1b2c3d.wav"), data, none).as_deref(), Some("Anstieg"));
        let pack = |p: &Path| p.ends_with("abonnieren-rot.png").then(|| "Abonnieren (rot)".to_string());
        assert_eq!(media_title(Path::new("D:/p/elemente/abonnieren-rot.png"), data, pack).as_deref(), Some("Abonnieren (rot)"));
        // eigene Datei: Dateiname bleibt
        assert_eq!(media_title(Path::new("D:/Videos/urlaub.mp4"), data, none), None);
    }

    fn bundled() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins")
    }

    #[test]
    fn shipped_pack_effects_land_in_builtin_folders() {
        let reg = cl_plugin::Registry::scan([bundled().as_path()]);
        let packs = catalog_packs(&reg, "de");
        let basis = packs.iter().find(|p| p.info.id == "cutline.basis").unwrap();
        assert!(!basis.entries.is_empty());
        for e in &basis.entries {
            let builtin: &[&str] = match e.kind {
                Kind::Effect => &cl_fx::EFFECT_CATEGORIES,
                Kind::Filter => &cl_fx::FILTER_CATEGORIES,
                _ => &[],
            };
            assert!(builtin.contains(&e.category()), "{} in {}", e.id, e.category());
        }
    }

    /// Danksagung im Repo ist aktuell; jedes mitgelieferte Paket nennt eine Lizenz.
    /// Neu erzeugen: `CUTLINE_WRITE_THIRD_PARTY=1 cargo test -p cl-app third_party`
    #[test]
    fn third_party_file_is_current() {
        let reg = cl_plugin::Registry::scan([bundled().as_path()]);
        let packs = catalog_packs(&reg, "de");
        for p in &packs {
            assert!(!p.info.license.trim().is_empty(), "{} ohne Lizenz", p.info.id);
            assert!(p.entries.iter().all(|e| !e.license.trim().is_empty()), "{}", p.info.id);
        }
        let want = catalog::credits(&packs, &|s| s.to_string());
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../THIRD_PARTY.md");
        if std::env::var_os("CUTLINE_WRITE_THIRD_PARTY").is_some() {
            std::fs::write(&file, &want).unwrap();
        }
        let have = std::fs::read_to_string(&file).unwrap_or_default().replace("\r\n", "\n");
        assert_eq!(have, want, "THIRD_PARTY.md veraltet (siehe Kommentar am Test)");
    }

    /// Kleines Testpaket mit Sound (WAV) und Sticker (SVG): gelistet, ausgeschaltet weg,
    /// Pfad-Ausbruch abgelehnt.
    #[test]
    fn pack_sounds_and_stickers_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let pack = dir.path().join("pack");
        std::fs::create_dir_all(&pack).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(pack.join("ding.wav"), spec).unwrap();
        for i in 0..800 {
            w.write_sample(((i as f32 * 0.3).sin() * 8000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        std::fs::write(
            pack.join("star.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4"/></svg>"#,
        )
        .unwrap();
        let manifest = r#"
[plugin]
id = "test.pack"
version = "1.0.0"
api = 1
kind = "asset-pack"
name = { de = "Testpaket", en = "Test pack" }
license = "CC0-1.0"
authors = ["Jemand"]

[[asset]]
id = "ding"
kind = "sound"
name = { de = "Ding" }
category = ["Oberfläche"]
file = "ding.wav"
tags = ["Klingel"]

[[asset]]
id = "star"
kind = "sticker"
name = { de = "Stern" }
file = "star.svg"
license = "CC-BY-4.0"
"#;
        std::fs::write(pack.join("plugin.toml"), manifest).unwrap();
        let mut reg = cl_plugin::Registry::scan([dir.path()]);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        let packs = catalog_packs(&reg, "en");
        let snd = catalog::list_in(&packs, Kind::Sound);
        let e = snd.last().unwrap();
        assert_eq!(e.id, "test.pack/ding");
        assert_eq!(e.category(), "Oberfläche");
        assert_eq!(e.payload, Payload::File(pack.join("ding.wav")));
        assert_eq!(e.tags, ["Klingel"], "Schlagworte kommen in der Suche an");
        let st = catalog::get_in(&packs, Kind::Sticker, "test.pack/star").unwrap();
        assert_eq!((st.license.as_str(), st.author.as_str()), ("CC-BY-4.0", "Jemand"));
        assert_eq!(st.category(), "Test pack", "ohne Kategorie: Ordner mit dem Paketnamen");
        assert_eq!(counts_text(&catalog::packs_in(&packs)[1].counts), "1 Sound · 1 Sticker");
        let dict: std::collections::HashMap<_, _> = cl_i18n::ui_entries().into_iter().collect();
        assert_eq!((dict.get("1 Filter"), dict.get("1 Sticker")), (Some(&"1 filter"), Some(&"1 sticker")), "englische Einzahl");
        // ausgeschaltet: verschwunden
        reg.apply_states(&[("test.pack".to_string(), false)].into_iter().collect());
        let packs = catalog_packs(&reg, "en");
        assert_eq!(catalog::list_in(&packs, Kind::Sound).len(), cl_fx::sfx::SFX.len());
        assert!(catalog::get_in(&packs, Kind::Sticker, "test.pack/star").is_none());
        // Pfad-Ausbruch: Paket wird nicht geladen
        std::fs::write(dir.path().join("x.wav"), b"RIFF").unwrap();
        std::fs::write(pack.join("plugin.toml"), manifest.replace("file = \"ding.wav\"", "file = \"../x.wav\"")).unwrap();
        let reg = cl_plugin::Registry::scan([dir.path()]);
        assert!(reg.plugins.is_empty());
        assert!(reg.errors[0].contains("innerhalb"), "{:?}", reg.errors);
    }

    /// Die Endungen der Pakete sind eine Teilmenge dessen, was die eigene Bibliothek kann.
    #[test]
    fn pack_file_types_match_library() {
        for e in cl_plugin::SOUND_EXT {
            assert!(crate::library::SOUND_EXT.contains(e), "{e}");
        }
        for e in cl_plugin::STICKER_EXT {
            assert!(crate::library::STICKER_EXT.contains(e), "{e}");
        }
    }

    #[test]
    fn pack_card_texts() {
        let c: BTreeMap<String, usize> = [("sound".into(), 213), ("sticker".into(), 1), ("neuart".into(), 4)].into_iter().collect();
        assert_eq!(index_counts(&c), [(Kind::Sound, 213), (Kind::Sticker, 1)].into_iter().collect());
        assert_eq!(counts_text(&index_counts(&c)), "213 Sounds · 1 Sticker");
        assert_eq!(size_text(22_707, true), "23 KB");
        assert_eq!(size_text(1_351_277, true), "1,3 MB");
        assert_eq!(size_text(1_351_277, false), "1.3 MB");
        assert_eq!(size_text(0, true), "0 KB");
        assert_eq!(progress_text(11_000, 22_000, true), "50 % · 11 / 22 KB");
        assert_eq!(progress_text(30_000, 22_000, true), "100 % · 22 / 22 KB", "nie über 100 %");
        assert_eq!(progress_text(0, 0, true), "0 % · 0 / 1 KB");
        assert_eq!(progress_text(629_146, 1_468_006, true), "42 % · 0,6 / 1,4 MB");
    }

    #[test]
    fn pack_source_follows_environment() {
        let d = || Some(std::ffi::OsString::from("D:/pakete"));
        assert_eq!(source_from(false, None, false), Some(Source::Github));
        assert_eq!(source_from(false, None, true), None, "CUTLINE_NO_UPDATE");
        assert_eq!(source_from(false, d(), false), Some(Source::Github), "Ordner nur in Entwickler-Builds");
        assert_eq!(source_from(true, d(), true), Some(Source::Dir("D:/pakete".into())));
        assert_eq!(source_from(true, Some("".into()), false), Some(Source::Github));
    }

    /// Kleines Paket als ZIP in `dir` und sein Eintrag in der Liste
    fn test_pack(dir: &Path, id: &str, file: &str) -> IndexPack {
        use std::io::Write;
        let manifest = r#"
[plugin]
id = "test.laden"
version = "1.0.0"
api = 1
kind = "asset-pack"
name = { de = "Laden" }
license = "CC0-1.0"

[[asset]]
id = "star"
kind = "sticker"
name = { de = "Stern" }
file = "star.svg"
"#;
        let zp = dir.join(file);
        let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        w.start_file("plugin.toml", o).unwrap();
        w.write_all(manifest.as_bytes()).unwrap();
        w.start_file("star.svg", o).unwrap();
        w.write_all(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4"/></svg>"#).unwrap();
        // größer als die Toleranz der Größenprüfung
        w.start_file("lizenz.txt", o.compression_method(zip::CompressionMethod::Stored)).unwrap();
        w.write_all(&(0..20_000u32).map(|i| (i % 251) as u8).collect::<Vec<_>>()).unwrap();
        w.finish().unwrap();
        let data = std::fs::read(&zp).unwrap();
        IndexPack {
            id: id.into(),
            name: [("de".to_string(), "Laden".to_string())].into_iter().collect(),
            version: "1.0.0".into(),
            api: 1,
            description: BTreeMap::new(),
            file: file.into(),
            size: data.len() as u64,
            sha256: cl_update::sha256_hex(&data),
            counts: BTreeMap::new(),
            license: "CC0-1.0".into(),
        }
    }

    fn files_in(d: &Path) -> usize {
        std::fs::read_dir(d).map(|r| r.count()).unwrap_or(0)
    }

    /// Laden und Einbauen über einen lokalen Ordner statt GitHub: Erfolg, falsche Prüfsumme,
    /// zu groß, falsche ID, Abbruch – die Zwischendatei bleibt nie liegen.
    #[test]
    fn packs_download_and_install() {
        let t = tempfile::tempdir().unwrap();
        let (srcd, tmp, root) = (t.path().join("src"), t.path().join("tmp"), t.path().join("Plugins"));
        std::fs::create_dir_all(&srcd).unwrap();
        let src = Source::Dir(srcd.clone());
        let p = test_pack(&srcd, "test.laden", "laden.zip");
        let no = AtomicBool::new(false);
        // falsche Prüfsumme: nichts eingebaut
        let mut bad = p.clone();
        bad.sha256 = cl_update::sha256_hex(b"anders");
        assert!(fetch_install(&src, &bad, &tmp, &root, |_, _| {}, &no).unwrap_err().contains("Prüfsumme"));
        assert!(!root.join("test.laden").exists());
        assert_eq!(files_in(&tmp), 0);
        // größer als in der Liste
        let mut small = p.clone();
        small.size = 10;
        assert!(fetch_install(&src, &small, &tmp, &root, |_, _| {}, &no).is_err());
        assert!(!root.join("test.laden").exists());
        assert_eq!(files_in(&tmp), 0);
        // ID im Paket passt nicht zur Liste
        let other = IndexPack { id: "test.anders".into(), ..p.clone() };
        assert!(fetch_install(&src, &other, &tmp, &root, |_, _| {}, &no).unwrap_err().contains("passt nicht"));
        assert_eq!(files_in(&root), 0);
        assert_eq!(files_in(&tmp), 0);
        // abgebrochen
        let cancel = AtomicBool::new(true);
        assert_eq!(fetch_install(&src, &p, &tmp, &root, |_, _| {}, &cancel).unwrap_err(), "Abgebrochen");
        assert_eq!(files_in(&tmp), 0);
        // Liste nicht erreichbar
        assert!(matches!(packs::fetch_index(&src), Err(IndexError::Unreachable(_))));
        // richtig: eingebaut, im Katalog, nicht mehr „verfügbar“
        let mut seen = 0;
        fetch_install(&src, &p, &tmp, &root, |d, _| seen = d, &no).unwrap();
        assert_eq!(seen, p.size);
        assert_eq!(files_in(&tmp), 0);
        let index = [p.clone()];
        let avail = |reg: &cl_plugin::Registry| {
            let inst: Vec<_> = reg.plugins.iter().map(|x| packs::Installed { id: &x.manifest.plugin.id, version: &x.manifest.plugin.version, own: true }).collect();
            packs::plan(&index, &inst, cl_plugin::API_VERSION, "de").0.len()
        };
        let reg = cl_plugin::Registry::scan([root.as_path()]);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        assert!(catalog::get_in(&catalog_packs(&reg, "de"), Kind::Sticker, "test.laden/star").is_some());
        assert_eq!(avail(&reg), 0);
        // Entfernen über den Weg der Karte: wieder verfügbar
        cl_plugin::remove_pack(&root, &root.join("test.laden")).unwrap();
        assert_eq!(avail(&cl_plugin::Registry::scan([root.as_path()])), 1);
    }

    /// Jede ZIP der lokalen Testkopie passt zur Liste und lässt sich einbauen.
    #[test]
    fn real_packs_install() {
        let dir = Path::new("F:/cutline-pakete/fertig");
        if !dir.join(packs::INDEX_FILE).is_file() {
            return;
        }
        let src = Source::Dir(dir.into());
        let list = packs::fetch_index(&src).unwrap();
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("Plugins");
        for p in &list {
            fetch_install(&src, p, &t.path().join("tmp"), &root, |_, _| {}, &AtomicBool::new(false)).unwrap_or_else(|e| panic!("{}: {e}", p.id));
        }
        assert_eq!(files_in(&t.path().join("tmp")), 0);
        let reg = cl_plugin::Registry::scan([root.as_path()]);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        assert_eq!(reg.plugins.len(), list.len());
        for p in catalog_packs(&reg, "de") {
            let want = index_counts(&list.iter().find(|x| x.id == p.info.id).unwrap().counts);
            assert_eq!(catalog::packs_in(&[p.clone()])[1].counts, want, "{}", p.info.id);
        }
    }

    /// Shader-Übergänge der mitgelieferten Pakete erscheinen im Katalog, mit Rückfall.
    #[test]
    fn pack_transitions_are_listed_with_fallback() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let mut reg = cl_plugin::Registry::scan([root.as_path()]);
        let packs = catalog_packs(&reg, "de");
        let tr = catalog::list_in(&packs, Kind::Transition);
        let own: Vec<_> = tr.iter().filter(|e| e.pack == "cutline.uebergaenge").collect();
        assert!(own.len() >= 25, "{}", own.len());
        assert!(own.iter().all(|e| matches!(e.payload, Payload::Shader { .. })));
        let plan = catalog::transition_plan_in(&packs, "cutline.uebergaenge/wuerfel");
        assert_eq!(plan.xfade, "slideright");
        assert!(plan.shader.is_some_and(|(f, _)| f.is_file()));
        // Paket aus: nur noch der Rückfall
        reg.apply_states(&[("cutline.uebergaenge".to_string(), false)].into_iter().collect());
        let packs = catalog_packs(&reg, "de");
        let plan = catalog::transition_plan_in(&packs, "cutline.uebergaenge/wuerfel");
        assert_eq!((plan.xfade, plan.shader), ("slideright", None));
    }
}
