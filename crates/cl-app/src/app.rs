//! App-Zustand und alle Aktionen. Oberfläche: `chrome` (Titelleiste), `home` (Startseite),
//! `assets` (links), `player`, `inspector` (rechts), `timeline`, `settings`, `voiceover`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::result::Result;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cl_core::*;
use cl_i18n::I18n;
use eframe::egui::{self, vec2, Color32, Key, Modifiers, RichText};

use crate::audio::AudioPlayer;
use crate::export_ui::ExportDialog;
use crate::preview::{Decoder, Job, Playback};
use crate::record::Recorder;
use crate::settings::{AppSettings, McpRunning, Section};
use crate::theme::{self, icon};
use crate::{paths, timeline::TimelineState};

/// Fokus aus Textfeldern nehmen (Tastenkürzel gehen dann wieder an die App).
pub fn release_text_focus(ctx: &egui::Context) {
    ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Editor,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LeftTab {
    Media,
    Audio,
    Text,
    Stickers,
    Effects,
    Transitions,
    Filters,
    Captions,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InspectorTab {
    Project,
    History,
    Markers,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClipTab {
    Basic,
    Text,
    Effects,
    Adjust,
    /// Freistellen, Greenscreen, Bildqualität, Stabilisieren & Co. (Video/Bild)
    Smart,
    Animation,
    Speed,
    Audio,
}

/// Was aus dem Asset-Bereich in Zeitleiste oder Player gezogen werden kann.
#[derive(Clone, Debug)]
pub enum DragAsset {
    Media(MediaId),
    /// Text, optional mit Vorlage
    Text(Option<&'static str>),
    Sticker(&'static str),
    /// Eigener Sticker / eigenes Bild aus der Bibliothek
    OwnSticker(PathBuf),
    Effect(String),
    Filter(String),
    /// Eingebauter (`xfade`-Name) oder Paket-Übergang (`<paket-id>/<asset-id>`)
    Transition(String),
    Sfx(&'static str),
    File(PathBuf),
}

/// Zustand des Voiceover-Panels.
pub struct Voiceover {
    pub open: bool,
    pub device: Option<String>,
    pub devices: Vec<String>,
    pub mute_project: bool,
    pub countdown: bool,
    pub gain_db: f32,
    pub countdown_until: Option<Instant>,
    pub recorder: Option<Recorder>,
    /// 0 = Aufnahme, 1 = Einstellungen
    pub tab: usize,
    /// Lautstärke und Ton-Werkzeuge für die neue Aufnahme
    pub volume_db: f32,
    pub audio: AudioFx,
}

pub struct App {
    pub doc: Arc<Mutex<Document>>,
    /// Kopie des Projekts für die Darstellung, erneuert bei jeder Revision.
    pub project: Project,
    pub revision: u64,
    pub screen: Screen,
    pub settings: AppSettings,
    pub settings_open: bool,
    pub settings_section: Section,
    pub home: crate::home::HomeState,
    pub fonts: Arc<Mutex<cl_fonts::FontLibrary>>,
    pub system_font_count: Arc<Mutex<Option<usize>>>,
    pub plugins: cl_plugin::Registry,
    /// Epidemic-Sound-Integration (nur aktiv, wenn das Plugin eingeschaltet ist)
    pub es: crate::epidemic::EsState,
    /// Eigene Sticker und Sounds
    pub library: crate::library::Library,
    pub lib_dialog: Option<crate::library_ui::LibDialog>,
    pub lib_dialog_since: u64,
    /// Gespeicherte Look-Presets
    pub presets: Vec<crate::presets::Preset>,
    pub preset_dialog: Option<crate::presets::PresetDialog>,
    pub preset_dialog_since: u64,
    pub preset_list_open: bool,
    /// Projekt-Import aus anderen Programmen
    pub import: crate::import_ui::ImportState,
    /// Wiedergabegeschwindigkeit (J/K/L): 1 = normal, negativ = rückwärts
    pub play_rate: f64,
    /// In/Out-Bereich und Schleife
    pub range_in: Option<Time>,
    pub range_out: Option<Time>,
    pub loop_range: bool,
    /// Spuren, die solo gehört werden (nur Vorschau, nicht im Export)
    pub solo: HashSet<TrackId>,
    /// Einstellungsseite: Aktion, deren Taste gerade aufgenommen wird
    pub capture_key: Option<&'static str>,
    /// Geöffneter Marker-Editor
    pub marker_edit: Option<u64>,
    pub marker_draft: Option<cl_core::Marker>,
    /// Spur umbenennen: (Spur, Eingabe)
    pub track_rename: Option<(TrackId, String)>,
    /// Pegelanzeige: (Wert, Halten, Haltezeit) links/rechts
    pub meter: [(f32, f32, f32); 2],
    pub paste_attrs: crate::edit_tools::PasteAttrs,
    pub paste_attrs_open: bool,
    pub tpl_dialog: Option<crate::templates::TplDialog>,
    /// Medien, deren Proxy nicht erstellt werden konnte (nicht erneut versuchen)
    pub proxy_failed: HashSet<MediaId>,
    /// Vorschau in eigenem Fenster
    pub ext_preview: bool,
    /// Vorschaufenster im Vollbild (randlos maximiert)
    pub ext_full: bool,
    /// Vorschaugröße beim Start der Wiedergabe (bleibt bis zum Anhalten)
    pub play_canvas_px: Option<egui::Vec2>,
    /// Aufteilung (Medien-, Inspektor-Breite, Zeitleisten-Höhe) als Anteil des Fensters
    pub layout_frac: (f32, f32, f32),
    pub layout_screen: Option<egui::Rect>,
    /// Einführung (erster Start) und Rundgang
    pub onboard: Option<crate::onboarding::Onboard>,
    pub tour: Option<usize>,
    /// Lage der Bereiche für den Rundgang
    pub tour_rects: HashMap<&'static str, egui::Rect>,
    /// Vorschau: maskierte Effekt-Elemente (Clip, zweite Fassung, Maske)
    pub fx_overlays: Vec<(ClipId, ClipId, Option<Mask>, f32)>,
    /// Messung (CUTLINE_PERF): geöffnete Ströme, gezeigte Strom-Bilder, UI-Bilder
    pub perf_opens: u32,
    pub perf_frames: u32,
    pub perf_ui: u32,
    pub perf_since: Instant,
    pub perf_last_frame: Option<Instant>,
    pub perf_max_gap: f64,
    /// Messung: Start der Wiedergabe bzw. des Ton-Neustarts (erstes Bild, erster Ton noch offen?)
    pub perf_play: Option<PerfPlay>,
    /// Feiner System-Takt, solange abgespielt wird (Windows: 1 ms statt 15,6 ms)
    hires_timer: Option<HiresTimer>,
    /// Multicam-Modus: (Blickwinkel-Clips, Wechsel (Zeit, Index))
    pub multicam: Option<(Vec<ClipId>, Vec<(Time, usize)>)>,
    /// Trefferliste von „Highlights finden“ (Fenster offen)
    pub shorts: Option<crate::shorts::ShortsState>,
    /// Neues Seitenverhältnis, das auf die Wahl „Clips neu einpassen?“ wartet
    pub aspect_fit: Option<cl_core::ProjectSettings>,
    /// Hilfslinien für Plattform-Bedienelemente im Player
    pub safe_area: bool,
    /// Multicam: Index des Blickwinkels, dessen Ton durchgehend läuft
    pub multicam_audio: usize,
    /// Quick-Action-Fenster (Pausen, Füllwörter, Ducking, Punch-in)
    pub quick: crate::quick::Quick,
    /// Figuren (Plugin „Story-Werkzeuge“)
    pub characters: Vec<crate::story::Character>,
    /// Plugins Freesound und Pexels
    pub stock: crate::stock::StockState,
    /// Letzte automatische Sicherung
    pub last_backup: Option<Instant>,
    pub backup_revision: u64,
    pub backups_open: bool,
    /// Fehlende Medien nach dem Öffnen
    pub missing_open: bool,
    /// Laufendes Archivieren: Fortschritt/Ergebnis
    pub archive: Option<std::sync::mpsc::Receiver<Result<PathBuf, String>>>,
    pub lib_thumbs: crate::library_ui::Thumbs,
    /// Ablagefläche der Bibliotheksliste im letzten Frame (Dateien dort landen in der Bibliothek)
    pub lib_drop: Option<(crate::library::Kind, egui::Rect)>,
    pub i18n: I18n,
    pub tab: LeftTab,
    pub inspector_tab: InspectorTab,
    pub clip_tab: ClipTab,
    pub asset_cat: HashMap<&'static str, String>,
    /// Zuletzt angeklickter Clip (Inspektor, Player-Rahmen)
    pub selected: Option<ClipId>,
    /// Weitere ausgewählte Clips (Mehrfachauswahl)
    pub sel_extra: HashSet<ClipId>,
    /// Ausgewählter Übergang (Clip, dessen `transition_out` gemeint ist)
    pub selected_transition: Option<ClipId>,
    pub clipboard: Vec<Clip>,
    /// Herkunftsspur je Clip der Zwischenablage (gleiche Reihenfolge)
    pub clipboard_src: Vec<TrackId>,
    /// Spur, in die nach dem Kopieren geklickt wurde: dorthin fügt Strg+V ein (sonst Herkunftsspur)
    pub paste_track: Option<TrackId>,
    /// Beim nächsten Frame eine Markierung in die System-Zwischenablage legen
    pub copy_marker: bool,
    /// Clip mit laufender Bearbeitung im Inspektor (Vorschau, noch nicht im Verlauf)
    pub live: Option<Clip>,
    pub playhead: Time,
    pub playing: bool,
    pub timeline: TimelineState,
    pub project_path: Option<PathBuf>,
    pub saved_revision: u64,
    pub last_saved: Option<Instant>,
    pub font_scope_project: bool,
    pub mcp: Option<McpRunning>,
    pub player_max: bool,
    pub ui_scale: f32,
    pub export: ExportDialog,
    pub gizmo: crate::player_edit::Gizmo,
    pub player_zoom: f32,
    pub player_pan: egui::Vec2,
    pub voiceover: Voiceover,
    pub anim_slot: usize,
    pub focus_text: bool,
    toasts: Vec<(String, Instant, bool)>,
    pub decoder: Decoder,
    /// Vorschau-Texturen je Clip (+ Schlüssel des angezeigten Bildes)
    pub frames: HashMap<ClipId, (u64, egui::TextureHandle)>,
    pub requested: HashMap<ClipId, u64>,
    pub playbacks: HashMap<ClipId, Playback>,
    /// Neu geöffnete Ströme, die den laufenden ersetzen, sobald sie ein Bild haben
    pub playbacks_next: HashMap<ClipId, Playback>,
    /// Fingerabdruck alles Hörbaren (Ton nur bei Änderungen neu starten)
    pub audio_sig: u64,
    pub audio: Option<AudioPlayer>,
    pub play_clock: Option<(Instant, Time)>,
    /// Abspielen gedrückt, Bild und Ton starten, sobald die Ströme das erste Bild haben
    pub play_warmup: Option<Instant>,
    pub sfx_preview: Option<(String, AudioPlayer)>,
    /// Länge des gerade vorgehörten Sounds (Ende erkennen, Fortschritt zeigen)
    pub sfx_preview_len: Time,
    /// Bibliotheks-Sounds: geprüftes Medium und Wellenform (40 Balken), im Hintergrund geladen
    pub sound_meta: HashMap<String, (MediaRef, Vec<f32>)>,
    pub sound_meta_req: HashSet<String>,
    /// Wellenformen der Sound-Kacheln (auf Platte gemerkt, sofort da)
    pub sound_bars: crate::soundbars::SoundBars,
    /// Abspielen, sobald die Daten dieses Sounds da sind
    pub sound_pending: Option<String>,
    pub thumbs: HashMap<MediaId, egui::TextureHandle>,
    pub waves: HashMap<MediaId, Vec<f32>>,
    pub fx_thumbs: HashMap<String, egui::TextureHandle>,
    pub fx_thumbs_requested: HashSet<String>,
    /// Rechtsklick → Eigenschaften
    pub props: Option<crate::props::Props>,
    /// Fenster „Exporte“ für dieses Projekt (Name)
    pub exports_for: Option<crate::exports::ExportsList>,
    /// Sperrdatei des offenen Projekts (siehe `take_lock`)
    lock_file: Option<PathBuf>,
    /// Ton beim Ziehen: (letzte Stelle, Zeitpunkt, Stopp-Signal des laufenden Stücks)
    pub scrub: Option<(Time, Instant, Option<Arc<std::sync::atomic::AtomicBool>>)>,
    /// Pause mit offenen Strömen: (Playhead, Revision) beim Anhalten
    pub paused: Option<(Time, u64)>,
    /// Untertitel, dessen Textfeld im nächsten Bild den Fokus bekommt
    pub caption_focus: Option<ClipId>,
    /// Eingabefehler im gerade offenen Dialog (wird dort angezeigt, der Dialog bleibt offen)
    pub dialog_error: Option<String>,
    /// „Layout bearbeiten“ aktiv; Name für ein neues Profil
    pub layout_edit: bool,
    pub layout_name: String,
    /// Fehlermeldung im Einstellungsfenster (bleibt offen, statt nur eine Meldung unten)
    pub settings_error: Option<String>,
    /// Rückfrage/Fenster der Einstellungsseite „Inhalte“
    pub contents_dialog: Option<crate::contents::ContentsDialog>,
    /// Belegter Cache (zuletzt gezählt, Bytes)
    pub cache_size: Option<(Instant, u64)>,
    pub sticker_tex: HashMap<&'static str, egui::TextureHandle>,
    pub updater: crate::update::Updater,
    pub tasks: crate::tasks::Tasks,
    pub pick: Option<crate::pro::Pick>,
    pub pick_origin: Option<egui::Pos2>,
    pub frame_cpu: Option<(ClipId, cl_media::RgbaFrame)>,
    /// Vorschau von Shader-Übergängen (cl-gpu)
    pub shader_preview: crate::player::ShaderPreview,
    pub caption_lang: String,
    pub caption_from_clip: bool,
    registered_fonts: HashSet<String>,
    /// egui übernimmt `set_fonts` erst im nächsten Frame. Bis dahin bleiben neue Familien hier.
    pending_fonts: Option<HashSet<String>>,
    /// Schriftnamen für den Inspektor (sortiert, höchstens alle 2 s neu gelesen)
    pub font_names: Option<(Instant, Arc<Vec<String>>)>,
    /// Stereo-Prüfung (ein Kanal stumm?) schon angestoßen: Dateien, zuletzt geprüfte Revision
    silent_warmed: (HashSet<PathBuf>, u64),
    /// Letzte Suche nach fehlenden Proxys (Zeit, Anzahl Medien)
    pub proxy_scan: Option<(Instant, usize)>,
    /// Proxy-Ordner zuletzt gezählt: (Zeit, Anzahl, Bytes)
    pub proxy_stats: Option<(Instant, u64, u64)>,
    /// Eigenschaften-Fenster: Zeilen einmal berechnet (Ziel, Revision, Zeilen)
    pub props_cache: Option<(crate::props::Props, u64, crate::props::PropsRows)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        crate::set_dialog_parent(cc);
        let settings = AppSettings::load();
        let layout_frac = settings.layout.frac;
        theme::select(&cc.egui_ctx, &settings.theme);
        theme::apply(&cc.egui_ctx);
        let onboard = (!settings.onboarding_done
            && std::env::var_os("CUTLINE_NO_ONBOARDING").is_none())
        .then(|| crate::onboarding::Onboard::new(&settings.use_cases));
        perf_mark("Einstellungen");
        paths::install_bundled_fonts();
        let fonts = cl_fonts::FontLibrary::open(paths::font_dir()).unwrap_or_default();
        perf_mark("Schriften");
        let plugin_dirs = paths::plugin_dirs();
        let mut plugins = cl_plugin::Registry::scan(plugin_dirs.iter().map(|p| p.as_path()));
        plugins.apply_states(&settings.plugin_states);
        let lang = settings.lang();
        perf_mark("Plugins");
        cl_fx::catalog::load_packs(crate::contents::catalog_packs(&plugins, lang.code()));
        cl_fx::catalog::set_hidden(settings.hidden_packs.clone());
        perf_mark("Katalog");

        let system_font_count = Arc::new(Mutex::new(None));
        {
            let slot = system_font_count.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let n = cl_fonts::system_families().len();
                *slot.lock().unwrap() = Some(n);
                ctx.request_repaint();
            });
        }

        let project = Project::new("Neues Projekt");
        let screen = if open.is_some() || !settings.start_on_home {
            Screen::Editor
        } else {
            Screen::Home
        };
        let mut app = Self {
            doc: Arc::new(Mutex::new(Document::new(project.clone()))),
            project,
            revision: 0,
            screen,
            ui_scale: settings.ui_scale,
            settings,
            settings_open: false,
            settings_section: Section::General,
            home: Default::default(),
            fonts: Arc::new(Mutex::new(fonts)),
            system_font_count,
            plugins,
            es: Default::default(),
            library: crate::library::Library::load(),
            lib_dialog: None,
            lib_dialog_since: 0,
            presets: crate::presets::load(),
            preset_dialog: None,
            preset_dialog_since: 0,
            preset_list_open: false,
            import: Default::default(),
            play_rate: 1.0,
            range_in: None,
            range_out: None,
            loop_range: false,
            solo: HashSet::new(),
            capture_key: None,
            marker_edit: None,
            marker_draft: None,
            track_rename: None,
            meter: [(0.0, 0.0, 0.0); 2],
            paste_attrs: Default::default(),
            paste_attrs_open: false,
            tpl_dialog: None,
            proxy_failed: HashSet::new(),
            ext_preview: false,
            ext_full: false,
            play_canvas_px: None,
            layout_frac,
            layout_screen: None,
            onboard,
            tour: None,
            tour_rects: HashMap::new(),
            fx_overlays: Vec::new(),
            perf_opens: 0,
            perf_frames: 0,
            perf_ui: 0,
            perf_since: Instant::now(),
            perf_last_frame: None,
            perf_max_gap: 0.0,
            perf_play: None,
            hires_timer: None,
            multicam: None,
            shorts: None,
            aspect_fit: None,
            safe_area: false,
            multicam_audio: 0,
            quick: Default::default(),
            characters: crate::story::load(),
            stock: Default::default(),
            last_backup: None,
            backup_revision: 0,
            backups_open: false,
            missing_open: false,
            archive: None,
            lib_thumbs: Default::default(),
            lib_drop: None,
            i18n: I18n::new(lang),
            tab: LeftTab::Media,
            inspector_tab: InspectorTab::Project,
            clip_tab: ClipTab::Basic,
            asset_cat: HashMap::new(),
            selected: None,
            sel_extra: HashSet::new(),
            selected_transition: None,
            clipboard: Vec::new(),
            clipboard_src: Vec::new(),
            paste_track: None,
            copy_marker: false,
            live: None,
            playhead: Time::ZERO,
            playing: false,
            timeline: TimelineState::default(),
            project_path: None,
            saved_revision: 0,
            last_saved: None,
            font_scope_project: false,
            mcp: None,
            player_max: false,
            export: ExportDialog::default(),
            gizmo: Default::default(),
            player_zoom: 1.0,
            player_pan: egui::Vec2::ZERO,
            voiceover: Voiceover {
                open: false,
                device: crate::record::default_input(),
                devices: Vec::new(),
                mute_project: true,
                countdown: true,
                gain_db: 0.0,
                countdown_until: None,
                recorder: None,
                tab: 0,
                volume_db: 0.0,
                audio: AudioFx::default(),
            },
            anim_slot: 0,
            focus_text: false,
            toasts: Vec::new(),
            decoder: Decoder::new(cc.egui_ctx.clone()),
            frames: HashMap::new(),
            requested: HashMap::new(),
            playbacks: HashMap::new(),
            playbacks_next: HashMap::new(),
            audio_sig: 0,
            audio: None,
            play_clock: None,
            play_warmup: None,
            sfx_preview: None,
            sfx_preview_len: Time::ZERO,
            sound_meta: HashMap::new(),
            sound_meta_req: HashSet::new(),
            sound_bars: crate::soundbars::SoundBars::load(paths::data_dir().join("sound-bars.json")),
            sound_pending: None,
            thumbs: HashMap::new(),
            waves: HashMap::new(),
            fx_thumbs: HashMap::new(),
            fx_thumbs_requested: HashSet::new(),
            props: None,
            exports_for: None,
            lock_file: None,
            scrub: None,
            paused: None,
            caption_focus: None,
            dialog_error: None,
            layout_edit: false,
            layout_name: String::new(),
            settings_error: None,
            contents_dialog: None,
            cache_size: None,
            sticker_tex: HashMap::new(),
            updater: Default::default(),
            tasks: Default::default(),
            pick: None,
            pick_origin: None,
            frame_cpu: None,
            shader_preview: Default::default(),
            caption_lang: "auto".into(),
            caption_from_clip: false,
            registered_fonts: HashSet::new(),
            pending_fonts: None,
            font_names: None,
            silent_warmed: (HashSet::new(), u64::MAX),
            proxy_scan: None,
            proxy_stats: None,
            props_cache: None,
        };
        app.register_fonts(&cc.egui_ctx);
        app.prewarm_sounds();
        cc.egui_ctx.set_zoom_factor(app.ui_scale);
        if app.settings.mcp_enabled {
            app.start_mcp();
        }
        crate::update::startup_cleanup();
        if app.settings.auto_update && std::env::var_os("CUTLINE_NO_UPDATE").is_none() {
            app.updater.check(&cc.egui_ctx);
        }
        if let Some(p) = open {
            app.open_project(&p);
        }
        perf_mark("App bereit");
        app
    }

    pub fn t(&self, key: &str) -> String {
        self.i18n.t(key)
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toasts
            .push((crate::tr(msg.into()), Instant::now(), false));
    }
    pub fn toast_err(&mut self, msg: impl Into<String>) {
        self.toasts
            .push((crate::tr(msg.into()), Instant::now(), true));
    }

    /// Einziger Weg für Änderungen aus der Oberfläche.
    pub fn apply(&mut self, op: EditOp) -> Option<OpOutput> {
        let r = self.doc.lock().unwrap_or_else(|e| e.into_inner()).apply(op, OpSource::Ui);
        match r {
            Ok(o) => {
                self.sync_project();
                Some(o)
            }
            Err(e) => {
                self.toast_err(e.to_string());
                None
            }
        }
    }

    /// Mehrere Ops als ein Undo-Schritt.
    pub fn apply_batch(&mut self, ops: Vec<EditOp>) -> Option<OpOutput> {
        match ops.len() {
            0 => None,
            1 => self.apply(ops.into_iter().next().expect("eins")),
            _ => self.apply(EditOp::Batch {
                label: String::new(),
                ops,
            }),
        }
    }

    /// Alles, was `f` am Projekt ändert, als EIN Verlaufsschritt (z. B. neue Spur + Clip darin).
    pub fn one_step<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let rev = self.doc.lock().unwrap_or_else(|e| e.into_inner()).revision();
        let r = f(self);
        self.doc.lock().unwrap_or_else(|e| e.into_inner()).squash_since(rev);
        r
    }

    pub fn undo(&mut self) {
        self.live = None;
        let r = self.doc.lock().unwrap_or_else(|e| e.into_inner()).undo();
        match r {
            Ok(l) => self.toast(format!("{}: {}", self.t("undo"), crate::tr(l))),
            Err(e) => self.toast_err(e.to_string()),
        }
    }
    /// Zeitleiste als Bereich oben oder unten; gibt die Höhe zurück
    fn timeline_area(&mut self, ctx: &egui::Context, id: &'static str, top: bool, sr: egui::Rect, ft: f32) -> f32 {
        let p = if top { egui::TopBottomPanel::top(id) } else { egui::TopBottomPanel::bottom(id) };
        let r = p
            .resizable(!self.settings.layout_locked)
            .default_height((sr.height() * ft).clamp(150.0, 520.0))
            .min_height(120.0)
            .max_height((sr.height() * 0.6).max(140.0))
            .frame(theme::card(theme::PAD_CARD))
            .show(ctx, |ui| self.timeline_panel(ui))
            .response
            .rect;
        self.tour_rects.insert("timeline", r);
        r.height()
    }

    /// Mehrere Schritte auf einmal (negativ = rückgängig)
    pub fn history_step(&mut self, steps: isize) {
        self.commit_live();
        self.live = None;
        let n = self.doc.lock().unwrap_or_else(|e| e.into_inner()).step(steps);
        if n > 0 {
            let what = if steps < 0 { self.t("undo") } else { self.t("redo") };
            self.toast(format!("{what}: {n} {}", crate::tr(if n == 1 { "Schritt" } else { "Schritte" })));
        }
    }

    /// Verlauf rechts im Inspektor zeigen
    pub fn show_history(&mut self) {
        self.select_only(None);
        self.selected_transition = None;
        self.inspector_tab = InspectorTab::History;
    }

    pub fn redo(&mut self) {
        self.live = None;
        let r = self.doc.lock().unwrap_or_else(|e| e.into_inner()).redo();
        match r {
            Ok(l) => self.toast(format!("{}: {}", self.t("redo"), crate::tr(l))),
            Err(e) => self.toast_err(e.to_string()),
        }
    }

    pub fn sync_project(&mut self) {
        let doc = self.doc.lock().unwrap_or_else(|e| e.into_inner());
        if doc.revision() != self.revision {
            self.revision = doc.revision();
            self.project = doc.project().clone();
            drop(doc);
            if let Some(s) = self.selected {
                if self.project.find_clip(s).is_none() {
                    self.selected = None;
                }
            }
            self.sel_extra
                .retain(|c| self.project.find_clip(*c).is_some());
            if let Some(t) = self.selected_transition {
                if self
                    .project
                    .find_clip(t)
                    .is_none_or(|(_, c)| c.transition_out.is_none())
                {
                    self.selected_transition = None;
                }
            }
            if let Some(l) = &self.live {
                if self.project.find_clip(l.id).is_none() {
                    self.live = None;
                }
            }
            // Einzelbilder neu anfordern; laufende Ströme bleiben – ihr Schlüssel enthält Effekte
            // und Timing, geänderte Clips wechseln ohne Stocken auf einen neuen Strom.
            self.requested.clear();
            let sig = audio_signature(&self.project);
            if sig != self.audio_sig {
                self.audio_sig = sig;
                if self.playing && self.voiceover.recorder.is_none() {
                    // hörbare Änderungen sofort hörbar; das Bild läuft weiter, der Ton setzt neu ein
                    self.restart_audio();
                    if perf() && self.play_warmup.is_none() {
                        self.perf_play = Some(PerfPlay::new("Ton neu (Änderung)", self.playhead));
                    }
                }
            }
            for m in self.project.media.iter().filter(|m| m.is_local()) {
                if m.has_audio && !self.waves.contains_key(&m.id) {
                    self.waves.insert(m.id, Vec::new()); // Platzhalter: Auftrag nur einmal senden
                    self.decoder.send(Job::Wave {
                        media: m.id,
                        path: m.path.clone(),
                    });
                }
                if m.kind != MediaKind::Audio && !self.thumbs.contains_key(&m.id) {
                    let anim = if m.kind == MediaKind::Image { cl_media::animation(&m.path) } else { None };
                    let at = thumb_time(m.kind, m.duration, anim.as_ref());
                    self.decoder.send(Job::Thumb {
                        media: m.id,
                        path: m.path.clone(),
                        at,
                    });
                }
            }
        }
    }

    /// Vorschau neu anfordern (Texturen bleiben bis zum neuen Bild stehen).
    pub fn invalidate_preview(&mut self) {
        self.requested.clear();
        self.playbacks.clear();
        self.playbacks_next.clear();
    }

    // ---------- Auswahl ----------

    pub fn selection(&self) -> Vec<ClipId> {
        let mut v: Vec<ClipId> = self.sel_extra.iter().copied().collect();
        if let Some(s) = self.selected {
            if !v.contains(&s) {
                v.push(s);
            }
        }
        v
    }
    pub fn is_selected(&self, c: ClipId) -> bool {
        self.selected == Some(c) || self.sel_extra.contains(&c)
    }
    pub fn select_only(&mut self, c: Option<ClipId>) {
        if self.selected != c {
            self.commit_live();
        }
        self.selected = c;
        self.sel_extra.clear();
        self.selected_transition = None;
    }
    pub fn toggle_select(&mut self, c: ClipId) {
        if self.is_selected(c) {
            self.sel_extra.remove(&c);
            if self.selected == Some(c) {
                self.selected = self.sel_extra.iter().next().copied();
                if let Some(s) = self.selected {
                    self.sel_extra.remove(&s);
                }
            }
        } else {
            if let Some(s) = self.selected {
                self.sel_extra.insert(s);
            }
            self.selected = Some(c);
        }
        self.selected_transition = None;
    }
    pub fn select_all(&mut self) {
        let all: Vec<ClipId> = self
            .project
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect();
        self.selected = all.last().copied();
        self.sel_extra = all.into_iter().collect();
    }

    /// Clip, wie er gerade dargestellt wird (inkl. laufender Inspektor-Bearbeitung).
    pub fn clip_view(&self, id: ClipId) -> Option<Clip> {
        match &self.live {
            Some(l) if l.id == id => Some(l.clone()),
            _ => self.project.find_clip(id).map(|(_, c)| c.clone()),
        }
    }

    /// Wie `clip_view` für einen schon vorliegenden Clip: ohne Suche und ohne Klon
    pub fn clip_ref<'a>(&'a self, c: &'a Clip) -> &'a Clip {
        match &self.live {
            Some(l) if l.id == c.id => l,
            _ => c,
        }
    }

    /// Laufende Inspektor-Bearbeitung in den Verlauf schreiben.
    pub fn commit_live(&mut self) {
        let Some(c) = self.live.take() else {
            return;
        };
        let Some(orig) = self.project.find_clip(c.id).map(|(_, o)| o.clone()) else {
            return;
        };
        if orig == c {
            return;
        }
        // Mehrfachauswahl: dieselben Änderungen auf alle anderen ausgewählten Clips übertragen
        let mut ops = crate::multi_edit::propagate(&self.project, &orig, &c, &self.selection());
        if ops.is_empty() {
            self.apply(EditOp::ReplaceClip { clip: c });
        } else {
            ops.insert(0, EditOp::ReplaceClip { clip: c });
            self.apply_batch(ops);
        }
    }

    /// Clip-Zeit (relativ zum Clipbeginn) des Playheads, auf den Clip begrenzt.
    pub fn rel_time(&self, c: &Clip) -> Time {
        Time((self.playhead - c.start).0.clamp(0, c.duration.0.max(0)))
    }

    /// Eigenschaft setzen: mit Keyframes → Keyframe am Playhead, sonst Grundwert.
    pub fn set_prop(&self, c: &mut Clip, p: Prop, v: f32) {
        if !c.keyframes.get(p).is_empty() {
            let t = self.rel_time(c);
            c.keyframes.set(p, t, v);
            return;
        }
        match p {
            Prop::X => c.transform.x = v,
            Prop::Y => c.transform.y = v,
            Prop::Scale => c.transform.scale = v,
            Prop::Rotation => c.transform.rotation_deg = v,
            Prop::Opacity => c.opacity = v,
            Prop::Volume => c.volume_db = v,
            Prop::Strength => {
                if let ClipContent::Effect(r) | ClipContent::Filter(r) = &mut c.content {
                    r.intensity = v.clamp(0.0, 1.0);
                }
            }
        }
    }

    /// Keyframe am Playhead setzen/entfernen.
    pub fn toggle_keyframe(&mut self, id: ClipId, props: &[Prop]) {
        let Some(mut c) = self.clip_view(id) else {
            return;
        };
        let t = self.rel_time(&c);
        let (w, h) = (
            self.project.settings.width as f32,
            self.project.settings.height as f32,
        );
        let ev = c.eval_at(t, w, h);
        let here = props.iter().any(|p| c.keyframes.has_at(*p, t));
        for p in props {
            if here {
                c.keyframes.remove(*p, t);
            } else {
                let v = match p {
                    Prop::X => ev.x,
                    Prop::Y => ev.y,
                    Prop::Scale => ev.scale,
                    Prop::Rotation => ev.rotation_deg,
                    Prop::Opacity => ev.opacity,
                    Prop::Volume => ev.volume_db,
                    Prop::Strength => c.strength_at(t),
                };
                // Erster Keyframe: Grundwert übernehmen, damit nichts springt
                c.keyframes.set(*p, t, v);
            }
        }
        self.live = None;
        self.apply(EditOp::SetKeyframes {
            clip: id,
            keyframes: c.keyframes,
        });
    }

    /// Transformation aus dem Player übernehmen (Keyframes werden respektiert).
    pub fn commit_transform(&mut self, id: ClipId, tr: Transform) {
        let Some(mut c) = self.clip_view(id) else {
            return;
        };
        let before = c.clone();
        self.set_prop(&mut c, Prop::X, tr.x);
        self.set_prop(&mut c, Prop::Y, tr.y);
        self.set_prop(&mut c, Prop::Scale, tr.scale);
        self.set_prop(&mut c, Prop::Rotation, tr.rotation_deg);
        if c != before {
            self.live = None;
            self.apply(EditOp::ReplaceClip { clip: c });
        }
    }

    // ---------- Schriften ----------

    /// Registriert UI-Schriften, Icons und alle Familien der Bibliothek bei egui.
    pub fn register_fonts(&mut self, ctx: &egui::Context) {
        let lib = self.fonts.lock().unwrap();
        let mut defs = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut defs, egui_phosphor::Variant::Regular);
        let system_ui: [(&str, &[&str]); 2] = [
            (
                "ui-main",
                &[
                    "C:/Windows/Fonts/segoeui.ttf",
                    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                    "/System/Library/Fonts/SFNS.ttf",
                ],
            ),
            (
                "ui-symbols",
                &[
                    "C:/Windows/Fonts/seguisym.ttf",
                    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                ],
            ),
        ];
        for (i, (key, candidates)) in system_ui.iter().enumerate() {
            if let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) {
                defs.font_data
                    .insert(key.to_string(), egui::FontData::from_owned(bytes).into());
                for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                    let list = defs.families.entry(fam.clone()).or_default();
                    if i == 0 && fam == egui::FontFamily::Proportional {
                        list.insert(0, key.to_string());
                    } else {
                        list.push(key.to_string());
                    }
                }
            }
        }
        let fallback = defs
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        let mut names = HashSet::new();
        // Bibliothek + häufige Systemschriften für Textvorlagen
        let mut sources: Vec<(String, PathBuf, u32)> = Vec::new();
        for (family, faces) in lib.families() {
            if let Some(face) = faces
                .iter()
                .min_by_key(|f| (f.info.italic, (f.info.weight as i32 - 400).abs()))
            {
                sources.push((family.clone(), face.path.clone(), face.info.index));
            }
        }
        for (fam, file) in [
            ("Georgia", "georgia.ttf"),
            ("Impact", "impact.ttf"),
            ("Arial", "arial.ttf"),
            ("Segoe UI", "segoeui.ttf"),
            ("Consolas", "consola.ttf"),
            ("Comic Sans MS", "comic.ttf"),
            ("Times New Roman", "times.ttf"),
        ] {
            let p = PathBuf::from("C:/Windows/Fonts").join(file);
            if p.is_file() && !sources.iter().any(|s| s.0 == fam) {
                sources.push((fam.to_string(), p, 0));
            }
        }
        for (family, path, index) in sources {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let key = format!("user:{family}");
            let mut data = egui::FontData::from_owned(bytes);
            data.index = index;
            defs.font_data.insert(key.clone(), data.into());
            let mut chain = vec![key];
            chain.extend(fallback.iter().cloned());
            defs.families
                .insert(egui::FontFamily::Name(family.clone().into()), chain);
            names.insert(family);
        }
        drop(lib);
        ctx.set_fonts(defs);
        self.pending_fonts = Some(names);
    }

    pub fn font_family(&self, family: Option<&str>) -> egui::FontFamily {
        match family {
            Some(f) if self.registered_fonts.contains(f) => egui::FontFamily::Name(f.into()),
            _ => egui::FontFamily::Proportional,
        }
    }

    /// Schriftdatei für den Export: Bibliothek, dann Systemschriften.
    pub fn font_files(&self) -> HashMap<String, (PathBuf, u32)> {
        let lib = self.fonts.lock().unwrap();
        let mut m: HashMap<String, (PathBuf, u32)> = lib
            .families()
            .keys()
            .filter_map(|k| {
                lib.regular_face(k)
                    .map(|f| (k.clone(), (f.path.clone(), f.info.index)))
            })
            .collect();
        for (fam, file) in [
            ("Georgia", "georgia.ttf"),
            ("Impact", "impact.ttf"),
            ("Arial", "arial.ttf"),
            ("Segoe UI", "segoeui.ttf"),
            ("Consolas", "consola.ttf"),
            ("Comic Sans MS", "comic.ttf"),
            ("Times New Roman", "times.ttf"),
        ] {
            let p = PathBuf::from("C:/Windows/Fonts").join(file);
            if p.is_file() {
                m.entry(fam.to_string()).or_insert((p, 0));
            }
        }
        m
    }

    pub fn import_fonts(&mut self, ctx: &egui::Context, files: Vec<PathBuf>) {
        let mut any = false;
        for f in files {
            let res = self.fonts.lock().unwrap().import_file(&f);
            match res {
                Ok(r) if r.duplicate => self.toast(self.t("font-duplicate")),
                Ok(r) => {
                    let fam = r
                        .added
                        .first()
                        .map(|i| i.family.clone())
                        .unwrap_or_default();
                    if self.font_scope_project {
                        if let Some(p) = r.stored_as.clone() {
                            self.apply(EditOp::AddProjectFont { path: p });
                        }
                    }
                    self.toast(self.i18n.t_str("font-imported", "family", &fam));
                    any = true;
                }
                Err(e) => self.toast_err(self.i18n.t_str("font-error", "error", &e.to_string())),
            }
        }
        if any {
            self.register_fonts(ctx);
        }
    }

    pub fn pick_fonts(&mut self, ctx: &egui::Context) {
        if let Some(files) = crate::dialog()
            .add_filter("Fonts", cl_fonts::FONT_EXTENSIONS)
            .pick_files()
        {
            self.import_fonts(ctx, files);
        }
    }

    // ---------- Medien ----------

    pub fn import_media(&mut self, files: Vec<PathBuf>) -> Vec<MediaId> {
        // ffprobe für neue Dateien vorab parallel (bis zu 8 Threads), übernommen wird unten
        // wie bisher der Reihe nach
        let mut todo: Vec<&PathBuf> = files
            .iter()
            .filter(|f| {
                let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                !cl_fonts::FONT_EXTENSIONS.contains(&ext.as_str())
                    && !self.project.media.iter().any(|m| m.path == **f)
            })
            .collect();
        todo.sort();
        todo.dedup();
        let mut probed = HashMap::new();
        std::thread::scope(|sc| {
            let per = todo.len().div_ceil(8).max(1);
            let hs: Vec<_> = todo
                .chunks(per)
                .map(|c| sc.spawn(move || c.iter().map(|f| ((*f).clone(), cl_media::probe(f))).collect::<Vec<_>>()))
                .collect();
            for h in hs {
                probed.extend(h.join().unwrap_or_default());
            }
        });
        let mut ids = Vec::new();
        for f in files {
            let ext = f
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if cl_fonts::FONT_EXTENSIONS.contains(&ext.as_str()) {
                continue;
            }
            // Schon importiert? Dann wiederverwenden
            if let Some(m) = self.project.media.iter().find(|m| m.path == f) {
                ids.push(m.id);
                continue;
            }
            match probed.remove(&f).unwrap_or_else(|| cl_media::probe(&f)) {
                Ok(m) => {
                    let id = m.id;
                    if self.apply(EditOp::ImportMedia { media: m }).is_some() {
                        ids.push(id);
                    }
                }
                Err(e) => {
                    let name = f
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.toast_err(self.i18n.t_args(
                        "media-probe-error",
                        &[
                            ("file", name.as_str().into()),
                            ("error", e.to_string().into()),
                        ],
                    ));
                }
            }
        }
        ids
    }

    pub fn pick_media(&mut self) {
        let exts = cl_media::all_media_extensions();
        if let Some(files) = crate::dialog()
            .add_filter(self.t("tab-media"), &exts)
            .pick_files()
        {
            let n = self.import_media(files).len();
            if n > 0 {
                self.toast(self.i18n.t_num("media-imported", "count", n as i64));
            }
        }
    }

    fn insert_clip(&mut self, clip: Clip, track: Option<TrackId>) -> Option<ClipId> {
        if let Some(OpOutput::Clip(c)) = self.apply(EditOp::AddClip { track, clip }) {
            self.select_only(Some(c));
            return Some(c);
        }
        None
    }

    /// Medium an Position `at` einfügen (Video → Hauptspur, Audio → Audiospur).
    pub fn add_media_at(
        &mut self,
        media: MediaId,
        at: Time,
        track: Option<TrackId>,
    ) -> Option<ClipId> {
        let m = self
            .project
            .media(media)
            .cloned()
            .or_else(|| self.doc.lock().unwrap_or_else(|e| e.into_inner()).project().media(media).cloned())?;
        let dur = if m.kind == MediaKind::Image || m.duration <= Time::ZERO {
            Time::from_secs_f64(self.settings.image_duration as f64)
        } else {
            m.duration
        };
        let clip = Clip::new(
            ClipContent::Media {
                media,
                kind: m.kind,
            },
            at,
            dur,
        );
        self.insert_clip(clip, track)
    }

    pub fn add_text_at(
        &mut self,
        at: Time,
        template: Option<&str>,
        track: Option<TrackId>,
    ) -> Option<ClipId> {
        if let Some(id) = template {
            self.library.mark_used(&format!("tt:{id}"));
        }
        let tpl = template.and_then(|id| cl_fx::text::TEMPLATES.iter().find(|t| t.id == id));
        let style = match tpl {
            Some(t) => t.make(),
            None => TextStyle {
                text: self.t("text-default"),
                ..Default::default()
            },
        };
        let mut clip = Clip::new(
            ClipContent::Text(style),
            at,
            Time::from_secs_f64(self.settings.text_duration as f64),
        );
        if let Some(a) = tpl.and_then(|t| t.intro) {
            clip.anim.intro = Some(AnimRef {
                id: a.into(),
                duration: Time::from_secs_f64(self.settings.anim_duration as f64),
            });
        }
        let id = self.insert_clip(clip, track);
        if id.is_some() {
            self.clip_tab = ClipTab::Text;
        }
        id
    }

    pub fn add_sticker_at(&mut self, id: &str, at: Time, track: Option<TrackId>) -> Option<ClipId> {
        let st = cl_fx::stickers::sticker(id)?;
        self.library.mark_used(&format!("builtin:st:{id}"));
        let path = paths::data_dir().join("Stickers").join(format!("{id}.png"));
        if !path.is_file() {
            if let Err(e) = cl_fx::stickers::render_png(st, 512, &path) {
                self.toast_err(e);
                return None;
            }
        }
        self.add_image_overlay_at(path, at, track)
    }

    /// Bild als Sticker-Clip (verkleinert) auf eine freie Overlay-Spur legen (ein Verlaufsschritt).
    pub fn add_image_overlay_at(
        &mut self,
        path: PathBuf,
        at: Time,
        track: Option<TrackId>,
    ) -> Option<ClipId> {
        self.one_step(|s| s.add_image_overlay_step(path, at, track))
    }

    fn add_image_overlay_step(&mut self, path: PathBuf, at: Time, track: Option<TrackId>) -> Option<ClipId> {
        let media = *self.import_media(vec![path]).first()?;
        let clip = Clip::new(
            ClipContent::Media {
                media,
                kind: MediaKind::Image,
            },
            at,
            Time::from_secs_f64(self.settings.text_duration as f64),
        );
        let mut clip = clip;
        clip.transform.scale = 0.35;
        // Sticker liegen immer über dem Video: freie Overlay-Spur suchen oder anlegen
        let track = track.or_else(|| {
            self.project
                .timeline
                .tracks
                .iter()
                .find(|t| {
                    t.kind == TrackKind::Video
                        && !t.main
                        && !t.locked
                        && t.is_free(clip.start, clip.duration, None)
                })
                .map(|t| t.id)
        });
        let track = match track {
            Some(t) => Some(t),
            None => match self.apply(EditOp::InsertTrack {
                index: self.project.timeline.tracks.len(),
                kind: TrackKind::Video,
            }) {
                Some(OpOutput::Track(t)) => Some(t),
                _ => None,
            },
        };
        self.insert_clip(clip, track)
    }

    /// Datei eines eingebauten Sounds; der Name enthält einen Fingerabdruck der Quelle, damit
    /// verbesserte Sounds neu erzeugt werden
    pub fn sfx_path(id: &str) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        cl_fx::sfx::sfx(id).map(|s| s.source).hash(&mut h);
        paths::data_dir().join("Sfx").join(format!("{id}-{:08x}.wav", h.finish() as u32))
    }

    pub fn add_sfx_at(&mut self, id: &str, at: Time, track: Option<TrackId>) -> Option<ClipId> {
        let s = cl_fx::sfx::sfx(id)?;
        self.library.mark_used(&format!("builtin:sfx:{id}"));
        let path = Self::sfx_path(id);
        if let Err(e) = cl_fx::sfx::render(s, &path) {
            self.toast_err(e);
            return None;
        }
        let media = *self.import_media(vec![path]).first()?;
        self.add_media_at(media, at, track)
    }

    pub fn add_file_at(
        &mut self,
        path: PathBuf,
        at: Time,
        track: Option<TrackId>,
    ) -> Option<ClipId> {
        let media = *self.import_media(vec![path]).first()?;
        self.add_media_at(media, at, track)
    }

    /// Asset ablegen (aus Liste, Zeitleiste oder Player).
    pub fn drop_asset(
        &mut self,
        a: &DragAsset,
        at: Time,
        track: Option<TrackId>,
        on_clip: Option<ClipId>,
    ) {
        match a {
            DragAsset::Media(m) => {
                self.add_media_at(*m, at, track);
            }
            DragAsset::Text(t) => {
                self.add_text_at(at, *t, track);
            }
            DragAsset::Sticker(s) => {
                self.add_sticker_at(s, at, track);
            }
            DragAsset::Sfx(s) => {
                self.add_sfx_at(s, at, track);
            }
            DragAsset::OwnSticker(p) => {
                self.add_own_sticker_at(p, at, track);
            }
            DragAsset::File(p) => {
                self.add_file_at(p.clone(), at, track);
            }
            // Auf einen Clip gezogen: wirkt auf diesen Clip. Ins Freie: eigenes Element auf einer
            // Effekt-/Filterspur, das auf alles darunter wirkt.
            DragAsset::Effect(id) => match self.visual_target(on_clip) {
                Some(c) => self.add_effect(c, id),
                None => {
                    self.add_fx_clip(id, false, at, track);
                }
            },
            DragAsset::Filter(id) => match self.visual_target(on_clip) {
                Some(c) => self.set_filter(c, Some(id.clone())),
                None => {
                    self.add_fx_clip(id, true, at, track);
                }
            },
            DragAsset::Transition(id) => match on_clip {
                Some(c) => self.set_transition(c, Some(id.as_str())),
                None => self.apply_transition_near_playhead(id),
            },
        }
    }

    /// Clip unter dem Mauszeiger, falls er ein Bild hat (Video, Bild, Sticker).
    fn visual_target(&self, on_clip: Option<ClipId>) -> Option<ClipId> {
        on_clip.filter(|c| {
            self.project.find_clip(*c).is_some_and(|(_, c)| {
                matches!(
                    c.content,
                    ClipContent::Media {
                        kind: MediaKind::Video | MediaKind::Image,
                        ..
                    }
                )
            })
        })
    }

    /// Effekt oder Filter als eigenes Element (3 s) bei `at` einfügen; eine dafür angelegte Spur
    /// gehört zum selben Verlaufsschritt.
    pub fn add_fx_clip(
        &mut self,
        id: &str,
        filter: bool,
        at: Time,
        track: Option<TrackId>,
    ) -> Option<ClipId> {
        self.one_step(|s| s.add_fx_clip_step(id, filter, at, track))
    }

    fn add_fx_clip_step(&mut self, id: &str, filter: bool, at: Time, track: Option<TrackId>) -> Option<ClipId> {
        let r = FxRef::new(id);
        let content = if filter {
            ClipContent::Filter(r)
        } else {
            ClipContent::Effect(r)
        };
        let clip = Clip::new(content, at.max(Time::ZERO), Time::from_secs_f64(3.0));
        let kind = clip.kind();
        let free =
            |t: &Track| t.kind == kind && !t.locked && t.is_free(clip.start, clip.duration, None);
        let track = track
            .filter(|t| {
                self.project
                    .timeline
                    .tracks
                    .iter()
                    .any(|x| x.id == *t && free(x))
            })
            .or_else(|| {
                self.project
                    .timeline
                    .tracks
                    .iter()
                    .rev()
                    .find(|t| free(t))
                    .map(|t| t.id)
            });
        let track = match track {
            Some(t) => Some(t),
            None => match self.apply(EditOp::InsertTrack {
                index: self.project.timeline.tracks.len(),
                kind,
            }) {
                Some(OpOutput::Track(t)) => Some(t),
                _ => None,
            },
        };
        let id = self.insert_clip(clip, track)?;
        self.select_only(Some(id));
        Some(id)
    }

    pub fn add_effect(&mut self, clip: ClipId, id: &str) {
        self.library.mark_used(&format!("fx:{id}"));
        let Some(mut c) = self.clip_view(clip) else {
            return;
        };
        if !matches!(
            c.content,
            ClipContent::Media {
                kind: MediaKind::Video | MediaKind::Image,
                ..
            }
        ) {
            self.toast_err("Effekte wirken auf Video-, Bild- und Sticker-Clips");
            return;
        }
        if c.fx.effects.iter().any(|e| e.id == id) {
            return;
        }
        c.fx.effects.push(FxRef::new(id));
        self.live = None;
        self.apply(EditOp::ReplaceClip { clip: c });
        self.select_only(Some(clip));
        self.clip_tab = ClipTab::Effects;
    }

    pub fn set_filter(&mut self, clip: ClipId, id: Option<String>) {
        if let Some(i) = &id {
            self.library.mark_used(&format!("flt:{i}"));
        }
        let Some(mut c) = self.clip_view(clip) else {
            return;
        };
        if !matches!(
            c.content,
            ClipContent::Media {
                kind: MediaKind::Video | MediaKind::Image,
                ..
            }
        ) {
            self.toast_err("Filter wirken auf Video- und Bild-Clips");
            return;
        }
        c.fx.filter = id.map(|i| FxRef {
            id: i,
            intensity: c.fx.filter.as_ref().map(|f| f.intensity).unwrap_or(1.0),
            params: Default::default(),
        });
        self.live = None;
        self.apply(EditOp::ReplaceClip { clip: c });
        self.select_only(Some(clip));
        self.clip_tab = ClipTab::Effects;
    }

    /// Übergang zwischen `clip` und seinem direkten Nachfolger setzen.
    pub fn set_transition(&mut self, clip: ClipId, id: Option<&str>) {
        if let Some(i) = id {
            self.library.mark_used(&format!("tr:{i}"));
        }
        let dur = Time::from_secs_f64(self.settings.transition_duration as f64);
        let has_next = self.next_adjacent(clip).is_some();
        if id.is_some() && !has_next {
            self.toast_err("Übergänge liegen zwischen zwei direkt aufeinanderfolgenden Clips");
            return;
        }
        let keep = self
            .project
            .find_clip(clip)
            .and_then(|(_, c)| c.transition_out.as_ref().map(|t| t.duration));
        let t = id.map(|i| TransitionRef {
            id: i.to_string(),
            duration: keep.unwrap_or(dur),
        });
        if self
            .apply(EditOp::SetTransition {
                clip,
                transition: t,
            })
            .is_some()
        {
            self.selected_transition = id.map(|_| clip);
            if id.is_some() {
                self.selected = None;
                self.sel_extra.clear();
            }
        }
    }

    /// Direkter Nachfolger auf derselben Spur (für Übergänge).
    pub fn next_adjacent(&self, clip: ClipId) -> Option<ClipId> {
        let (tr, c) = self.project.find_clip(clip)?;
        tr.clips
            .iter()
            .find(|n| n.id != clip && (n.start.0 - c.end().0).abs() < 2000)
            .map(|n| n.id)
    }

    /// Übergang am nächstgelegenen Schnitt der Hauptspur (oder am ausgewählten Clip).
    pub fn apply_transition_near_playhead(&mut self, id: &str) {
        if let Some(c) = self.selected.filter(|c| self.next_adjacent(*c).is_some()) {
            self.set_transition(c, Some(id));
            return;
        }
        let ph = self.playhead;
        let best = self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video)
            .flat_map(|t| t.clips.iter())
            .filter(|c| self.next_adjacent(c.id).is_some())
            .min_by_key(|c| (c.end().0 - ph.0).abs())
            .map(|c| c.id);
        match best {
            Some(c) => self.set_transition(c, Some(id)),
            None => self.toast_err("Kein Schnitt zwischen zwei Clips gefunden"),
        }
    }

    // ---------- Bearbeiten ----------

    pub fn split_at_playhead(&mut self) {
        let at = self.playhead.snap_to_frame(self.project.settings.fps);
        // Keine Splitter kürzer als ein halbes Bild (entstehen sonst an Clipgrenzen
        // mit krummer Bildrate und stören den Export)
        let half = Time::from_secs_f64(0.5 / self.project.settings.fps.as_f64());
        let splittable = |c: &Clip| c.contains(at) && at - c.start >= half && c.end() - at >= half;
        let sel = self.selection();
        let mut targets: Vec<ClipId> = sel
            .iter()
            .copied()
            .filter(|s| {
                self.project
                    .find_clip(*s)
                    .is_some_and(|(_, c)| splittable(c))
            })
            .collect();
        if targets.is_empty() {
            targets = self
                .project
                .timeline
                .tracks
                .iter()
                .find(|t| t.main)
                .and_then(|t| t.clips.iter().find(|c| splittable(c)))
                .map(|c| vec![c.id])
                .unwrap_or_default();
        }
        if targets.is_empty() {
            return;
        }
        let ops: Vec<EditOp> = targets
            .iter()
            .map(|c| EditOp::SplitClip { clip: *c, at })
            .collect();
        if let Some(OpOutput::Clip(r)) = self.apply_batch(ops) {
            self.select_only(Some(r));
        }
    }

    /// Löscht die Auswahl. Der Playhead springt an den Anfang des (ersten) gelöschten Clips.
    pub fn delete_selected(&mut self) {
        self.live = None;
        if let Some(t) = self.selected_transition.take() {
            self.apply(EditOp::SetTransition {
                clip: t,
                transition: None,
            });
            return;
        }
        let sel = self.selection();
        if sel.is_empty() {
            return;
        }
        let on_main: Vec<bool> = sel.iter().filter_map(|c| self.project.find_clip(*c).map(|(t, _)| t.main)).collect();
        let s = &self.project.settings;
        if crate::navigate::delete_ripples_all(s.main_track_magnet, s.ripple_all_tracks, &on_main) {
            self.ripple_delete();
            return;
        }
        let first = sel
            .iter()
            .filter_map(|c| self.project.find_clip(*c).map(|(_, c)| c.start))
            .min();
        let ops = sel
            .iter()
            .map(|c| EditOp::RemoveClip { clip: *c })
            .collect();
        self.select_only(None);
        if self.apply_batch(ops).is_some() {
            if let Some(t) = first {
                self.stop_playback();
                self.playhead = t;
            }
        }
    }

    /// Q / W: links bzw. rechts vom Playhead wegschneiden (ausgewählter Clip oder Hauptspur).
    pub fn delete_side(&mut self, left: bool) {
        let at = self.playhead.snap_to_frame(self.project.settings.fps);
        let target = self
            .selected
            .filter(|s| {
                self.project
                    .find_clip(*s)
                    .is_some_and(|(_, c)| c.contains(at))
            })
            .or_else(|| {
                self.project
                    .timeline
                    .tracks
                    .iter()
                    .find(|t| t.main)
                    .and_then(|t| t.clips.iter().find(|c| c.contains(at)))
                    .map(|c| c.id)
            });
        let Some(id) = target else { return };
        let Some((tr, c)) = self
            .project
            .find_clip(id)
            .map(|(t, c)| (t.clone(), c.clone()))
        else {
            return;
        };
        if at <= c.start || at >= c.end() {
            return;
        }
        let op = if left {
            EditOp::TrimClip {
                clip: id,
                edge: Edge::Start,
                to: at,
            }
        } else {
            EditOp::TrimClip {
                clip: id,
                edge: Edge::End,
                to: at,
            }
        };
        if self.apply(op).is_some() && left && tr.main && self.project.settings.main_track_magnet {
            self.playhead = self
                .project
                .find_clip(id)
                .map(|(_, c)| c.start)
                .unwrap_or(self.playhead);
        }
    }

    pub fn copy_selection(&mut self) {
        let mut clips: Vec<(TrackId, Clip)> = self
            .selection()
            .iter()
            .filter_map(|c| self.project.find_clip(*c).map(|(t, c)| (t.id, c.clone())))
            .collect();
        clips.sort_by_key(|c| c.1.start);
        if !clips.is_empty() {
            self.toast(format!(
                "{} {} kopiert",
                clips.len(),
                if clips.len() == 1 { "Clip" } else { "Clips" }
            ));
            // Ziel zurück auf die Herkunftsspur, bis wieder in eine Spur geklickt wird
            self.paste_track = None;
            (self.clipboard_src, self.clipboard) = clips.into_iter().unzip();
            self.copy_marker = true;
        }
    }

    pub fn cut_selection(&mut self) {
        self.copy_selection();
        self.delete_selected();
    }

    pub fn paste(&mut self) {
        if self.clipboard.is_empty() {
            return;
        }
        // Herkunftsspur bzw. zuletzt angeklickte Spur am Playhead; belegt: Nachfolgende schieben
        let clips: Vec<(TrackId, Clip)> = self.clipboard_src.iter().copied().zip(self.clipboard.iter().cloned()).collect();
        let items = cl_core::paste_targets(&self.project, &clips, self.paste_track, self.playhead);
        let out = self.one_step(|s| {
            s.apply_batch(cl_core::paste_splits(&s.project, &items));
            s.apply_batch(cl_core::paste_ops(&s.project, &items))
        });
        if let Some(OpOutput::Clip(c)) = out {
            self.select_only(Some(c));
        }
    }

    pub fn duplicate_selection(&mut self) {
        let clips: Vec<(TrackId, Clip)> = self
            .selection()
            .iter()
            .filter_map(|c| self.project.find_clip(*c).map(|(t, c)| (t.id, c.clone())))
            .collect();
        let ops: Vec<EditOp> = clips
            .iter()
            .map(|(t, c)| {
                let mut n = c.clone();
                n.id = ClipId::new();
                n.start = c.end();
                EditOp::AddClip {
                    track: Some(*t),
                    clip: n,
                }
            })
            .collect();
        if let Some(OpOutput::Clip(c)) = self.apply_batch(ops) {
            self.select_only(Some(c));
        }
    }

    pub fn toggle_disabled(&mut self) {
        let sel = self.selection();
        let any_enabled = sel
            .iter()
            .any(|c| self.project.find_clip(*c).is_some_and(|(_, c)| !c.disabled));
        let ops = sel
            .iter()
            .map(|c| EditOp::SetClipDisabled {
                clip: *c,
                disabled: any_enabled,
            })
            .collect();
        self.apply_batch(ops);
    }

    /// Ton eines Videoclips als eigenen Audioclip anlegen und im Video stummschalten.
    pub fn extract_audio(&mut self, id: ClipId) {
        let Some(c) = self.clip_view(id) else { return };
        let ClipContent::Media {
            media,
            kind: MediaKind::Video,
        } = c.content
        else {
            return;
        };
        if !self.project.media(media).is_some_and(|m| m.has_audio) {
            self.toast_err("Dieser Clip hat keinen Ton");
            return;
        }
        let mut a = c.clone();
        a.id = ClipId::new();
        a.content = ClipContent::Media {
            media,
            kind: MediaKind::Audio,
        };
        a.fx = ClipFx {
            reverse: c.fx.reverse,
            speed_curve: c.fx.speed_curve.clone(),
            ..Default::default()
        };
        a.keyframes = Keyframes::default();
        a.anim = Animations::default();
        a.transition_out = None;
        let mut v = c.clone();
        v.volume_db = -60.0;
        self.apply_batch(vec![
            EditOp::ReplaceClip { clip: v },
            EditOp::AddClip {
                track: None,
                clip: a,
            },
        ]);
        self.toast(format!("{} Ton extrahiert", icon::WAVEFORM));
    }

    /// Standbild: aktuelles Bild als 3-s-Bild am Playhead einfügen.
    pub fn freeze_frame(&mut self) {
        let at = self.playhead.snap_to_frame(self.project.settings.fps);
        let target = self
            .selected
            .and_then(|s| {
                self.project
                    .find_clip(s)
                    .map(|(t, c)| (t.clone(), c.clone()))
            })
            .filter(|(_, c)| c.contains(at))
            .or_else(|| {
                self.project
                    .timeline
                    .tracks
                    .iter()
                    .find(|t| t.main)
                    .and_then(|t| {
                        t.clips
                            .iter()
                            .find(|c| c.contains(at))
                            .map(|c| (t.clone(), c.clone()))
                    })
            });
        let Some((tr, c)) = target else { return };
        let ClipContent::Media {
            media,
            kind: MediaKind::Video,
        } = c.content
        else {
            self.toast_err("Standbilder gibt es nur von Videoclips");
            return;
        };
        let Some(m) = self.project.media(media).cloned() else {
            return;
        };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let out = paths::data_dir()
            .join("Freeze")
            .join(format!("freeze-{stamp}.png"));
        let _ = std::fs::create_dir_all(out.parent().expect("Ordner"));
        let ok = cl_media::ffmpeg_command()
            .ok()
            .and_then(|mut cmd| {
                cmd.args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-y",
                    "-ss",
                    &format!("{:.4}", c.source_time(at).as_secs_f64()),
                ])
                .arg("-i")
                .arg(&m.path)
                .args(["-frames:v", "1"])
                .arg(&out)
                .stdin(std::process::Stdio::null())
                .status()
                .ok()
            })
            .is_some_and(|s| s.success());
        if !ok {
            self.toast_err("Standbild konnte nicht erzeugt werden");
            return;
        }
        let Some(img) = self.import_media(vec![out]).first().copied() else {
            return;
        };
        let mut f = Clip::new(
            ClipContent::Media {
                media: img,
                kind: MediaKind::Image,
            },
            at,
            Time::from_secs_f64(3.0),
        );
        f.transform = c.transform;
        f.fx = ClipFx {
            flip_h: c.fx.flip_h,
            flip_v: c.fx.flip_v,
            filter: c.fx.filter.clone(),
            adjust: c.fx.adjust,
            ..Default::default()
        };
        let mut ops = Vec::new();
        if at > c.start && at < c.end() {
            ops.push(EditOp::SplitClip { clip: c.id, at });
        }
        ops.push(EditOp::AddClip {
            track: Some(tr.id),
            clip: f,
        });
        if let Some(OpOutput::Clip(id)) = self.apply_batch(ops) {
            self.select_only(Some(id));
        }
    }

    /// Schnelle Änderung an allen ausgewählten Clips.
    pub fn modify_selection(&mut self, f: impl Fn(&mut Clip)) {
        self.commit_live();
        let ops: Vec<EditOp> = self
            .selection()
            .iter()
            .filter_map(|id| self.project.find_clip(*id).map(|(_, c)| c.clone()))
            .map(|mut c| {
                f(&mut c);
                EditOp::ReplaceClip { clip: c }
            })
            .collect();
        self.apply_batch(ops);
    }

    pub fn set_speed_selection(&mut self, speed: f32) {
        let ops = self
            .selection()
            .iter()
            .map(|c| EditOp::SetSpeed { clip: *c, speed })
            .collect();
        self.apply_batch(ops);
    }

    // ---------- Projekte ----------

    pub fn default_project_path(&self) -> PathBuf {
        let name: String = paths::safe_name(&self.project.name);
        let dir = paths::projects_dir();
        let mut p = dir.join(format!("{}.cutline", name.trim()));
        let mut n = 2;
        while p.exists() && Some(&p) != self.project_path.as_ref() {
            p = dir.join(format!("{} ({n}).cutline", name.trim()));
            n += 1;
        }
        p
    }

    pub fn write_project(&mut self, p: &Path) -> Result<(), String> {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        paths::write_atomic(p, self.project.to_json()).map_err(|e| e.to_string())?;
        self.saved_revision = self.revision;
        self.last_saved = Some(Instant::now());
        // neues Projekt oder „Speichern unter“: Sperre wandert zur neuen Datei
        if self.lock_file.as_deref() != Some(lock_path(p).as_path()) {
            self.release_lock();
            self.take_lock(p);
        }
        self.settings.add_recent(p);
        // Titelbild für die Startseite
        if let Some((path, at)) = self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video)
            .flat_map(|t| t.clips.iter())
            .min_by_key(|c| c.start)
            .and_then(|c| match &c.content {
                ClipContent::Media { media, .. } => self
                    .project
                    .media(*media)
                    .filter(|m| m.is_local())
                    .map(|m| (m.path.clone(), c.source_time(c.start))),
                _ => None,
            })
        {
            self.decoder.send(Job::ProjectThumb {
                out: crate::home::thumb_path(p),
                path,
                at,
            });
        }
        Ok(())
    }

    pub fn save(&mut self, save_as: bool) {
        self.commit_live();
        let path = match (&self.project_path, save_as) {
            (Some(p), false) => Some(p.clone()),
            (None, false) => Some(self.default_project_path()),
            _ => crate::dialog()
                .add_filter("Cutline-Projekt", &["cutline"])
                .set_file_name(format!("{}.cutline", self.project.name))
                .save_file(),
        };
        if let Some(p) = path {
            match self.write_project(&p) {
                Ok(()) => {
                    self.toast(format!("{} Gespeichert", icon::CHECK));
                    self.project_path = Some(p);
                    self.settings.save();
                }
                Err(e) => self.toast_err(e),
            }
        }
    }

    /// Automatisch speichern (1,5 s nach der letzten Änderung).
    fn autosave(&mut self) {
        if !self.settings.autosave || self.screen != Screen::Editor {
            return;
        }
        if !needs_save(self.revision, self.saved_revision, self.project_path.is_some(), &self.project) {
            return;
        }
        if self.doc.lock().unwrap().revision() != self.revision {
            return;
        }
        static LAST_CHANGE: Mutex<Option<(u64, Instant)>> = Mutex::new(None);
        let mut lc = LAST_CHANGE.lock().unwrap();
        match *lc {
            Some((rev, t)) if rev == self.revision => {
                if t.elapsed() > Duration::from_millis(1500) {
                    drop(lc);
                    let p = self
                        .project_path
                        .clone()
                        .unwrap_or_else(|| self.default_project_path());
                    match self.write_project(&p) {
                        Ok(()) => {
                            self.project_path = Some(p);
                            self.settings.save();
                        }
                        // höchstens einmal pro Minute melden, sonst jeder Frame
                        Err(e) => {
                            static LAST_ERR: Mutex<Option<Instant>> = Mutex::new(None);
                            let mut le = LAST_ERR.lock().unwrap();
                            if le.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
                                *le = Some(Instant::now());
                                drop(le);
                                self.toast_err(format!("Automatisch speichern fehlgeschlagen: {e}"));
                            }
                        }
                    }
                }
            }
            _ => *lc = Some((self.revision, Instant::now())),
        }
    }

    pub fn open_dialog(&mut self) {
        if let Some(p) = crate::dialog()
            .add_filter("Cutline-Projekt", &["cutline", "json"])
            .pick_file()
        {
            self.open_project(&p);
        }
    }

    fn reset_editor(&mut self) {
        self.stop_playback();
        self.select_only(None);
        self.live = None;
        self.playhead = Time::ZERO;
        self.timeline = TimelineState::default();
        self.frames.clear();
        // Zwischenspeicher des alten Projekts freigeben und seine wartenden Aufträge verwerfen;
        // Effekt-/Filter-/Übergangskacheln gelten für alle Projekte und bleiben
        self.decoder.next_generation();
        self.thumbs.clear();
        self.waves.clear();
        let project_key = |k: &String| k.starts_with("strip:") || k.starts_with("lib:");
        self.fx_thumbs.retain(|k, _| !project_key(k));
        self.fx_thumbs_requested.retain(|k| !project_key(k));
        self.sound_meta.clear();
        self.sound_meta_req.clear();
        self.invalidate_preview();
        self.player_zoom = 1.0;
        self.player_pan = egui::Vec2::ZERO;
        // Fenster mit Zeiten und Treffern des alten Projekts schließen
        self.shorts = None;
        self.aspect_fit = None;
        self.quick.open = None;
        self.quick.hits = None;
    }

    /// Beim Beenden: laufende Hintergrundaufgaben abbrechen, damit ihre FFmpeg-Prozesse enden
    pub fn cancel_background_work(&mut self) {
        for t in &self.tasks.list {
            t.cancel();
        }
    }

    /// Stereo-Prüfung der Medien vorab anstoßen (Import, geladenes Projekt): sonst klingt das
    /// erste Abspielen einseitig, solange das Ergebnis fehlt. Nacheinander in einem Thread.
    fn warm_silent_channels(&mut self) {
        if self.silent_warmed.1 == self.revision {
            return;
        }
        self.silent_warmed.1 = self.revision;
        let new: Vec<PathBuf> = self
            .project
            .media
            .iter()
            .filter(|m| m.has_audio && m.kind != MediaKind::Image && m.is_local())
            .filter(|m| self.silent_warmed.0.insert(m.path.clone()))
            .map(|m| m.path.clone())
            .collect();
        if !new.is_empty() {
            std::thread::spawn(move || {
                for p in new {
                    cl_media::silent_channel_opt(&p, true);
                }
            });
        }
    }

    /// Projekt in den Editor setzen. Es gilt als ungespeichert (ein neues `Document` beginnt
    /// wieder bei Revision 0 – sonst hielte man eine Wiederherstellung für gespeichert).
    pub fn set_doc(&mut self, proj: Project) {
        *self.doc.lock().unwrap() = Document::new(proj);
        self.tasks.next_generation();
        self.revision = u64::MAX;
        self.sync_project();
        self.saved_revision = UNSAVED;
    }

    /// `false`: altes Projekt ließ sich nicht speichern, es bleibt offen.
    pub fn new_project(&mut self) -> bool {
        if !self.flush_before_leaving() {
            return false;
        }
        self.reset_editor();
        let n = crate::home::next_project_name();
        let mut p = Project::new(n);
        p.settings.fps = self.settings.new_fps;
        p.settings.channels = self.settings.new_channels;
        self.set_doc(p);
        self.saved_revision = self.revision;
        self.project_path = None;
        self.screen = Screen::Editor;
        true
    }

    /// Projektordner ändern und vorhandene Projekte mitnehmen. Fehler bleiben im
    /// Einstellungsfenster stehen.
    pub fn change_projects_dir(&mut self, new: PathBuf) {
        let old = paths::projects_dir();
        if new == old {
            return;
        }
        // offenes Projekt erst sichern und Dateien freigeben (laufende Wiedergabe hält sie offen)
        self.stop_playback();
        if !self.flush_before_leaving() {
            return;
        }
        let mut reopen = None;
        let (moved, errs) = paths::move_projects(&old, &new);
        // was umgezogen ist, wird nachgetragen – auch wenn anderes fehlschlug
        if !moved.is_empty() {
            crate::exports::move_dir(&old, &new);
            crate::exports::move_projects(&moved);
        }
        for (a, b) in &moved {
            let _ = std::fs::rename(crate::home::thumb_path(a), crate::home::thumb_path(b));
            crate::project_tools::move_backups(a, b);
            for r in &mut self.settings.recent {
                if r == a {
                    *r = b.clone();
                }
            }
            if self.project_path.as_ref() == Some(a) {
                reopen = Some(b.clone());
            }
        }
        // Ordner nur wechseln, wenn nichts zurückgeblieben ist
        if errs.is_empty() {
            let default = paths::data_dir().join("Projects");
            self.settings.projects_dir = (new != default).then_some(new.clone());
            paths::set_projects_dir(self.settings.projects_dir.clone());
            self.settings_error = None;
            self.toast(crate::tr(format!("{} {} Projekte verschoben", icon::CHECK, moved.len())));
        } else {
            self.settings_error = Some(errs.join("\n"));
        }
        self.settings.save();
        self.home.refresh();
        // das offene Projekt vom neuen Ort laden (seine Medienpfade wurden umgeschrieben)
        if let Some(p) = reopen {
            self.open_project(&p);
        }
    }

    pub fn open_project(&mut self, p: &Path) {
        if !self.flush_before_leaving() {
            return;
        }
        match std::fs::read_to_string(p)
            .map_err(|e| e.to_string())
            .and_then(|s| Project::from_json(&s).map_err(|e| e.to_string()))
        {
            Ok(proj) => {
                self.reset_editor();
                let needed: HashSet<String> = proj
                    .timeline
                    .tracks
                    .iter()
                    .flat_map(|t| t.clips.iter())
                    .filter_map(|c| match &c.content {
                        ClipContent::Text(s) => s.font_family.clone(),
                        _ => None,
                    })
                    .collect();
                self.set_doc(proj);
                self.saved_revision = self.revision;
                self.project_path = Some(p.to_path_buf());
                self.take_lock(p);
                self.settings.add_recent(p);
                self.settings.save();
                self.screen = Screen::Editor;
                self.last_backup = Some(Instant::now());
                self.missing_open = self
                    .project
                    .media
                    .iter()
                    .any(|m| !m.is_local());
                let known = self.font_files();
                let missing: Vec<String> = needed
                    .into_iter()
                    .filter(|f| !known.contains_key(f))
                    .collect();
                if !missing.is_empty() {
                    self.toast_err(format!("Fehlende Schriften: {}", missing.join(", ")));
                }
                let mut gone: Vec<String> = cl_fx::catalog::missing(&self.project).into_iter().map(|m| cl_fx::fx_name(&m.0)).collect();
                gone.sort();
                gone.dedup();
                if !gone.is_empty() {
                    self.toast_err(
                        crate::theme::count_text(
                            gone.len(),
                            "{} Inhalt fehlt: {} – Paket in Einstellungen → Inhalte einschalten",
                            "{} Inhalte fehlen: {} – Paket in Einstellungen → Inhalte einschalten",
                        )
                        .replacen("{}", &gone.join(", "), 1),
                    );
                }
            }
            Err(e) => self.toast_err(e),
        }
    }

    /// Vor dem Verlassen eines Projekts: offene Änderungen speichern. `false`, wenn das nicht
    /// ging – dann bleibt das Projekt offen, sonst wären die Änderungen weg.
    fn flush_before_leaving(&mut self) -> bool {
        self.commit_live();
        if self.voiceover.recorder.is_some() {
            self.stop_voiceover();
        }
        if needs_save(self.revision, self.saved_revision, self.project_path.is_some(), &self.project) {
            let p = self
                .project_path
                .clone()
                .unwrap_or_else(|| self.default_project_path());
            if let Err(e) = self.write_project(&p) {
                self.toast_err(format!("Projekt konnte nicht gespeichert werden: {e}"));
                return false;
            }
            self.project_path = Some(p);
        }
        self.settings.save();
        self.release_lock();
        true
    }

    /// Sperrdatei `<Projekt>.lock` (Inhalt: PID): warnt, wenn ein zweites Fenster dasselbe
    /// Projekt öffnet – beide würden sich sonst still gegenseitig überschreiben.
    fn take_lock(&mut self, p: &Path) {
        let lock = lock_path(p);
        let me = std::process::id();
        if paths::lock_held_by_other(&lock, me) {
            self.toast_err("Dieses Projekt ist schon in einem anderen Cutline-Fenster geöffnet");
        }
        let _ = std::fs::write(&lock, me.to_string());
        self.lock_file = Some(lock);
    }

    fn release_lock(&mut self) {
        if let Some(l) = self.lock_file.take() {
            // nur die eigene Sperre entfernen
            if std::fs::read_to_string(&l).is_ok_and(|s| s.trim() == std::process::id().to_string()) {
                let _ = std::fs::remove_file(l);
            }
        }
    }

    pub fn go_home(&mut self) {
        if !self.flush_before_leaving() {
            return;
        }
        // jeder Weg zurück in den Editor setzt ihn ohnehin zurück: Bilder, Texturen und
        // Decoder-Zwischenspeicher des Projekts schon jetzt freigeben
        self.reset_editor();
        self.home.refresh();
        self.screen = Screen::Home;
    }

    // ---------- Wiedergabe ----------

    pub fn last_frame(&self) -> Time {
        Time(
            (self.project.duration()
                - Time::from_secs_f64(1.0 / self.project.settings.fps.as_f64()))
            .0
            .max(0),
        )
    }

    pub fn toggle_play(&mut self) {
        if self.playing {
            self.pause_playback();
        } else if self.try_resume() {
        } else {
            self.play_rate = 1.0;
            self.commit_live();
            // Mit In/Out-Bereich: im Bereich starten
            if let (Some(a), Some(b)) = (self.range_in, self.range_out) {
                if self.loop_range && (self.playhead < a || self.playhead >= b) {
                    self.playhead = a;
                }
            }
            if self.playhead >= self.last_frame() {
                self.playhead = Time::ZERO;
            }
            self.playing = true;
            self.playbacks.clear();
            self.playbacks_next.clear();
            // Erst puffern: der Playhead läuft los, wenn die Bildströme bereit sind. Der Ton wird
            // gleichzeitig vorbereitet und setzt ein, sobald er da ist – das Bild wartet nicht auf ihn.
            self.play_warmup = Some(Instant::now());
            self.restart_audio();
            self.play_clock = None;
            if perf() {
                self.perf_play = Some(PerfPlay::new("Play", self.playhead));
            }
        }
    }

    /// Pause: Bild- und Tonströme bleiben offen, damit es an derselben Stelle ohne neues
    /// Puffern weitergeht (sonst hängt das Bild beim Fortsetzen bis zu einer Sekunde).
    fn pause_playback(&mut self) {
        let simple = (self.play_rate - 1.0).abs() < 1e-6
            && self.play_warmup.is_none()
            && !self.multicam.as_ref().is_some_and(|m| !m.1.is_empty())
            && self.audio.as_ref().is_some_and(|a| a.started() && a.pause());
        if !simple {
            self.stop_playback();
            return;
        }
        self.playing = false;
        self.playbacks_next.clear();
        self.play_clock = None;
        self.paused = Some((self.playhead, self.revision));
        self.perf_last_frame = None;
    }

    /// Nach einer Pause an derselben Stelle und ohne Änderung direkt weiterlaufen.
    /// (Sonst gibt `update` die offenen Ströme gleich frei, siehe [`pause_is_stale`].)
    fn try_resume(&mut self) -> bool {
        let Some((at, rev)) = self.paused.take() else { return false };
        if at != self.playhead || rev != self.revision || self.audio.is_none() {
            self.stop_playback();
            return false;
        }
        for pb in self.playbacks.values_mut() {
            // nächstes Bild des Stroms = jetzige Position
            let fps = pb.stream.fps.as_f64().max(1.0);
            pb.t0 = self.playhead - Time::from_secs_f64(pb.taken as f64 / fps);
            pb.waiting_since = None;
        }
        if !self.audio.as_ref().is_some_and(|a| a.resume()) {
            self.stop_playback();
            return false;
        }
        self.playing = true;
        true
    }

    pub fn stop_playback(&mut self) {
        self.paused = None;
        if self.playing && self.multicam.as_ref().is_some_and(|m| !m.1.is_empty()) {
            self.playing = false;
            self.multicam_finish();
        }
        self.playing = false;
        self.play_rate = 1.0;
        self.audio = None;
        self.playbacks.clear();
        self.playbacks_next.clear();
        self.play_clock = None;
        self.play_warmup = None;
    }

    /// Ton ab dem Playhead neu starten. Die Uhr läuft sofort nach Systemzeit weiter; der Ton setzt
    /// ohne Sprung an der dann aktuellen Stelle ein (während des Pufferns erst mit dessen Ende).
    pub fn restart_audio(&mut self) {
        self.audio = None;
        let now = Instant::now();
        self.play_clock = Some((now, self.playhead));
        let mute = (self.voiceover.recorder.is_some() && self.voiceover.mute_project)
            || (self.play_rate - 1.0).abs() > 1e-6;
        if !mute {
            // Solo: alle anderen Tonspuren stumm (nur in der Vorschau)
            let proj = if self.solo.is_empty() {
                std::borrow::Cow::Borrowed(&self.project)
            } else {
                let mut p = self.project.clone();
                for t in &mut p.timeline.tracks {
                    if !self.solo.contains(&t.id) {
                        t.muted = true;
                    }
                }
                std::borrow::Cow::Owned(p)
            };
            match AudioPlayer::start_synced(&proj, self.playhead) {
                Ok(a) => {
                    if self.play_warmup.is_none() {
                        a.anchor(now);
                    }
                    self.audio = Some(a);
                }
                Err(e) => eprintln!("Ton: {e}"),
            }
        }
    }

    /// Ton beim Ziehen des Playheads: alle 250 ms ein kurzes Stück ab der aktuellen Stelle.
    fn scrub_audio(&mut self, ctx: &egui::Context) {
        let moved = self.scrub.as_ref().is_none_or(|(t, _, _)| *t != self.playhead);
        let dragging = ctx.input(|i| i.pointer.primary_down());
        if self.settings.scrub_audio && !self.playing && dragging && moved {
            let due = self.scrub.as_ref().is_none_or(|(_, at, _)| at.elapsed() > Duration::from_millis(250));
            // erster Aufruf nach dem Start merkt sich nur die Stelle
            if due && self.scrub.is_some() {
                use std::sync::atomic::{AtomicBool, Ordering};
                // Starten (FFmpeg, Audiogerät) und Beenden blockieren: beides im eigenen Thread.
                // Der cpal-Strom ist nicht Send, also lebt das Stück ganz dort; die Oberfläche
                // hält nur das Stopp-Signal und beendet damit das vorige Stück.
                if let Some((_, _, Some(old))) = &self.scrub {
                    old.store(true, Ordering::Relaxed);
                }
                let stop = Arc::new(AtomicBool::new(false));
                let (project, at, s) = (self.project.clone(), self.playhead, stop.clone());
                std::thread::spawn(move || {
                    let a = AudioPlayer::start(&project, at).ok();
                    if perf() {
                        eprintln!("scrub: Ton ab {:.2} s, ok={}", at.as_secs_f64(), a.is_some());
                    }
                    // Stück nach 350 ms beenden (oder früher, wenn das nächste beginnt)
                    let t0 = Instant::now();
                    while a.is_some() && !s.load(Ordering::Relaxed) && t0.elapsed() < Duration::from_millis(350) {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                });
                self.scrub = Some((self.playhead, Instant::now(), Some(stop)));
            } else if self.scrub.is_none() {
                self.scrub = Some((self.playhead, Instant::now(), None));
            }
        }
    }

    /// Playhead fortschreiben: Audio-Clock wenn vorhanden, sonst Systemzeit.
    fn advance_playhead(&mut self) {
        if !self.playing {
            return;
        }
        if let Some(w) = self.play_warmup {
            let now = self.playhead;
            let active = self.playbacks.values().filter(|p| p.t0 <= now);
            let mut any = false;
            let mut ready = true;
            for p in active {
                any = true;
                ready &= p.taken > 0;
            }
            let waited = w.elapsed().as_secs_f64();
            if (ready && (any || waited > 0.05)) || waited > 1.0 {
                self.play_warmup = None;
                self.playbacks.values_mut().for_each(|p| {
                    if p.t0 <= now {
                        p.t0 = now;
                    }
                });
                // Uhr startet jetzt; der vorbereitete Ton setzt an derselben Stelle ein
                let t = Instant::now();
                self.play_clock = Some((t, self.playhead));
                match &self.audio {
                    Some(a) => a.anchor(t),
                    None => self.restart_audio(),
                }
            }
            return;
        }
        // Ausgabe ließ sich nicht öffnen bzw. Ton zu Ende (Fehler): ohne Ton nach Systemzeit weiter
        if self.audio.as_ref().is_some_and(|a| a.failed() || (a.started() && a.finished())) {
            self.audio = None;
            self.play_clock = Some((Instant::now(), self.playhead));
        }
        self.perf_audio();
        match &self.audio {
            // Ton-Uhr; nie zurück (Übergabe von der Systemzeit ist ohne Sprung, Rundung ±1 Rückruf)
            Some(a) if a.started() => self.playhead = a.position().max(self.playhead),
            // Ton kommt gleich: Bild läuft nach Systemzeit (der Ton holt auf)
            Some(_) | None => {
                if let Some((t0, p0)) = self.play_clock {
                    let t = p0.as_secs_f64() + t0.elapsed().as_secs_f64() * self.play_rate;
                    self.playhead = Time::from_secs_f64(t.max(0.0));
                    if t <= 0.0 && self.play_rate < 0.0 {
                        self.stop_playback();
                        self.playhead = Time::ZERO;
                        return;
                    }
                }
            }
        }
        // Schleife über In/Out
        if self.loop_range && self.play_rate > 0.0 {
            if let (Some(a), Some(b)) = (self.range_in, self.range_out) {
                if self.playhead >= b {
                    self.playhead = a;
                    self.playbacks.clear();
                    self.playbacks_next.clear();
                    self.restart_audio();
                    return;
                }
            }
        }
        let recording = self.voiceover.recorder.is_some();
        if self.playhead >= self.project.duration() && !recording {
            self.stop_playback();
            self.playhead = self.last_frame();
        }
    }

    /// Daten eines Bibliotheks-Sounds anfordern (einmal je Schlüssel, im Hintergrund).
    pub fn request_sound_meta(&mut self, key: &str, path: &Path, sfx: Option<&'static str>) {
        if self.sound_meta_req.insert(key.to_string()) {
            self.decoder.send(crate::preview::Job::SoundMeta {
                key: key.to_string(),
                path: path.to_path_buf(),
                sfx,
            });
        }
    }

    /// Vorhören umschalten. Ohne fertige Daten wird angefordert und danach gestartet –
    /// die Oberfläche wartet nicht auf FFmpeg.
    pub fn preview_sound(&mut self, key: String, path: &Path, sfx: Option<&'static str>) {
        if self.sfx_preview.as_ref().is_some_and(|(k, _)| *k == key)
            || self.sound_pending.as_deref() == Some(key.as_str())
        {
            self.sfx_preview = None;
            self.sound_pending = None;
            return;
        }
        self.sfx_preview = None;
        match self.sound_meta.get(&key) {
            Some((m, _)) => {
                let m = m.clone();
                self.preview_media(key, m);
            }
            None => {
                self.request_sound_meta(&key, path, sfx);
                self.sound_pending = Some(key);
            }
        }
    }

    /// Vorhören eines bereits geprüften Mediums (auch Netz-Adressen, z. B. HLS).
    pub fn preview_media(&mut self, key: String, m: MediaRef) {
        let mut p = Project::new("vorhören");
        let id = m.id;
        let dur = m.duration;
        p.media.push(m);
        let mut t = Track::new(TrackKind::Audio);
        t.clips.push(Clip::new(
            ClipContent::Media {
                media: id,
                kind: MediaKind::Audio,
            },
            Time::ZERO,
            dur,
        ));
        p.timeline.tracks.push(t);
        match AudioPlayer::start(&p, Time::ZERO) {
            Ok(a) => {
                self.sfx_preview = Some((key, a));
                self.sfx_preview_len = dur;
            }
            Err(e) => self.toast_err(e),
        }
    }

    // ---------- Voiceover ----------

    pub fn start_voiceover(&mut self) {
        if self.voiceover.countdown && self.voiceover.countdown_until.is_none() {
            self.voiceover.countdown_until = Some(Instant::now() + Duration::from_secs(3));
            return;
        }
        self.voiceover.countdown_until = None;
        let dir = self
            .project_path
            .as_ref()
            .and_then(|p| {
                p.parent()
                    .map(|d| d.join(format!("{}-media", paths::safe_name(&self.project.name))))
            })
            .unwrap_or_else(|| paths::data_dir().join("Recordings"));
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = dir.join(format!("Voiceover-{stamp}.wav"));
        let gain = 10f32.powf(self.voiceover.gain_db / 20.0);
        match Recorder::start(self.voiceover.device.as_deref(), path, self.playhead, gain) {
            Ok(r) => {
                self.voiceover.recorder = Some(r);
                self.stop_playback();
                self.playing = true;
                self.restart_audio();
            }
            Err(e) => self.toast_err(format!("{}: {e}", self.t("voiceover"))),
        }
    }

    pub fn stop_voiceover(&mut self) {
        let Some(rec) = self.voiceover.recorder.take() else {
            return;
        };
        let start = rec.start_at;
        let peaks = rec.peaks();
        self.stop_playback();
        match rec.stop() {
            Ok(path) => {
                let ids = self.import_media(vec![path]);
                if let Some(id) = ids.first() {
                    // Live-Waveform übernehmen, bis die genaue berechnet ist
                    self.waves.entry(*id).or_insert(peaks);
                    if let Some(c) = self.add_media_at(*id, start, None) {
                        // Einstellungen aus dem Einsprech-Fenster übernehmen
                        let tuned = self.voiceover.volume_db.abs() > 0.01 || !self.voiceover.audio.is_empty();
                        if let Some(mut clip) = self.project.find_clip(c).map(|(_, x)| x.clone()).filter(|_| tuned) {
                            clip.volume_db = self.voiceover.volume_db;
                            clip.audio = self.voiceover.audio.clone();
                            self.apply(EditOp::ReplaceClip { clip });
                        }
                        self.select_only(Some(c));
                        self.toast(format!("{} {}", icon::MICROPHONE, self.t("voiceover")));
                    }
                }
            }
            Err(e) => self.toast_err(e),
        }
    }

    // ---------- Tastatur ----------

    /// Esc schließt das oberste offene Fenster (Eigenschaften, Exporte, Sicherungen, Neu verknüpfen).
    /// Läuft vor den Kürzeln, sonst verbraucht dort „escape“ die Taste und hebt nur die Auswahl auf.
    /// Ein Dialog, der Esc/Enter selbst auswertet, ist offen
    fn dialog_open(&self) -> bool {
        self.quick.open.is_some()
            || self.shorts.is_some()
            || self.aspect_fit.is_some()
            || self.tpl_dialog.is_some()
            || self.paste_attrs_open
            || self.marker_edit.is_some()
            || self.track_rename.is_some()
            || self.lib_dialog.is_some()
            || self.preset_dialog.is_some()
    }

    fn esc_closes(&mut self, ctx: &egui::Context) -> bool {
        // darüber liegende Fenster zuerst: sonst schlösse Esc ein verdecktes Fenster dahinter
        if self.settings_open || self.dialog_open() {
            return false;
        }
        let open = [
            ("props", self.props.is_some()),
            ("exports", self.exports_for.is_some()),
            ("backups", self.backups_open),
            ("relink", self.missing_open),
        ];
        if !open.iter().any(|o| o.1) || ctx.wants_keyboard_input() || !ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            return false;
        }
        let top = ctx.memory(|m| m.areas().top_layer_id(egui::Order::Middle));
        let mut ids = open.iter().filter(|o| o.1).map(|o| o.0);
        let first = ids.clone().next();
        let pick = ids.find(|id| top.is_some_and(|l| l.id == egui::Id::new(*id))).or(first);
        match pick {
            Some("props") => self.props = None,
            Some("exports") => self.exports_for = None,
            Some("backups") => self.backups_open = false,
            _ => self.missing_open = false,
        }
        true
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // offenes Kontextmenü: Esc gehört dem Menü (sonst verbraucht das Kürzel die Taste)
        if ctx.wants_keyboard_input() || self.settings_open || self.dialog_open() || ctx.is_context_menu_open() {
            return;
        }
        let mut actions: Vec<&str> = Vec::new();
        let binds = crate::keymap::bindings(&self.settings.shortcuts);
        let multicam_on = self.multicam.is_some();
        ctx.input_mut(|i| {
            for (c, a) in &binds {
                if i.consume_key(c.mods, c.key) {
                    actions.push(a);
                }
            }
            if multicam_on {
                const NUMS: [(Key, &str); 9] = [
                    (Key::Num1, "cam1"),
                    (Key::Num2, "cam2"),
                    (Key::Num3, "cam3"),
                    (Key::Num4, "cam4"),
                    (Key::Num5, "cam5"),
                    (Key::Num6, "cam6"),
                    (Key::Num7, "cam7"),
                    (Key::Num8, "cam8"),
                    (Key::Num9, "cam9"),
                ];
                for (k, a) in NUMS {
                    if i.consume_key(Modifiers::NONE, k) {
                        actions.push(a);
                    }
                }
            }
        });
        // Kopieren/Ausschneiden/Einfügen kommen als Ereignisse
        let (copy, cut, paste) = ctx.input_mut(|i| {
            let mut r = (false, false, false);
            for e in &i.events {
                match e {
                    egui::Event::Copy => r.0 = true,
                    egui::Event::Cut => r.1 = true,
                    egui::Event::Paste(_) => r.2 = true,
                    _ => {}
                }
            }
            // Falls das System kein Kopier-/Einfüge-Ereignis liefert (z. B. leere
            // Zwischenablage), die Tasten direkt auswerten
            r.0 |= i.consume_key(Modifiers::COMMAND, Key::C);
            r.1 |= i.consume_key(Modifiers::COMMAND, Key::X);
            r.2 |= i.consume_key(Modifiers::COMMAND, Key::V);
            // Strg+Alt+V (Windows: AltGr) ist „Attribute einfügen“, kein normales Einfügen
            if r.2 && (i.modifiers.alt || actions.contains(&"paste_attrs")) {
                r.2 = false;
                if !actions.contains(&"paste_attrs") {
                    actions.push("paste_attrs");
                }
            }
            r
        });
        if copy {
            self.copy_selection();
        }
        if std::mem::take(&mut self.copy_marker) {
            // Text in der Zwischenablage sorgt dafür, dass Strg+V ein Einfüge-Ereignis auslöst
            ctx.copy_text("Cutline-Clips".into());
        }
        if cut {
            self.cut_selection();
        }
        if paste {
            self.paste();
        }
        for a in actions {
            self.run_action(ctx, a);
        }
    }

    /// Erste Belegung einer Aktion als Text (für Menüzeilen), leer ohne Belegung.
    pub fn key_of(&self, action: &str) -> String {
        crate::keymap::bindings(&self.settings.shortcuts)
            .into_iter()
            .find(|(_, a)| *a == action)
            .map(|(c, _)| c.label())
            .unwrap_or_default()
    }

    /// Eine Aktion aus `keymap::ACTIONS` ausführen – Tastenkürzel und Menüs teilen sich das.
    pub fn run_action(&mut self, ctx: &egui::Context, a: &str) {
        let frame = Time::from_secs_f64(1.0 / self.project.settings.fps.as_f64());
        {
            match a {
                "play" => self.toggle_play(),
                "split" => self.split_at_playhead(),
                "undo" => self.undo(),
                "redo" => self.redo(),
                "delete" => self.delete_selected(),
                "import" => self.pick_media(),
                "save" => self.save(false),
                "save_as" => self.save(true),
                "open" => self.open_dialog(),
                "export" => {
                    self.commit_live();
                    self.export
                        .open_for(&self.project, self.project_path.as_deref(), self.revision, self.range_in.zip(self.range_out));
                }
                "select_all" => self.select_all(),
                "duplicate" => self.duplicate_selection(),
                "settings" => self.settings_open = true,
                "prev" => self.playhead = Time((self.playhead - frame).0.max(0)),
                "next" => self.playhead = self.playhead + frame,
                "home" => self.playhead = Time::ZERO,
                "end" => self.playhead = self.last_frame(),
                "max" => self.player_max = !self.player_max,
                "voiceover" => self.voiceover.open = !self.voiceover.open,
                "del_left" => self.delete_side(true),
                "del_right" => self.delete_side(false),
                "disable" => self.toggle_disabled(),
                "tool_select" => self.timeline.tool = crate::timeline::Tool::Select,
                "tool_blade" => self.timeline.tool = crate::timeline::Tool::Blade,
                "escape" => {
                    if self.timeline.tool != crate::timeline::Tool::Select {
                        self.timeline.tool = crate::timeline::Tool::Select
                    } else if self.pick.is_some() {
                        self.pick = None
                    } else if self.player_max {
                        self.player_max = false
                    } else {
                        self.select_only(None)
                    }
                }
                "shuttle_fwd" => self.shuttle(1.0),
                "shuttle_back" => self.shuttle(-1.0),
                "shuttle_stop" => self.stop_playback(),
                "mark_in" => self.set_range_point(true),
                "mark_out" => self.set_range_point(false),
                "clear_range" => {
                    self.range_in = None;
                    self.range_out = None;
                }
                "loop" => {
                    self.loop_range = !self.loop_range;
                    self.toast(if self.loop_range {
                        "Schleife an"
                    } else {
                        "Schleife aus"
                    });
                }
                "marker" => self.add_marker(),
                "marker_next" => self.jump_marker(true),
                "marker_prev" => self.jump_marker(false),
                "ripple_delete" => self.ripple_delete(),
                a if a.starts_with("cam") => {
                    let n = a[3..].parse::<usize>().unwrap_or(1);
                    self.multicam_switch(n - 1);
                }
                "nudge_left" => self.nudge(-1),
                "nudge_right" => self.nudge(1),
                "nudge_left10" => self.nudge(-10),
                "nudge_right10" => self.nudge(10),
                "paste_attrs" => self.paste_attrs_open = true,
                "group" => self.group_selection(true),
                "ungroup" => self.group_selection(false),
                "prev_edit" => self.jump_edit(false),
                "next_edit" => self.jump_edit(true),
                "zoom_in" => self.set_ui_scale(ctx, self.ui_scale + 0.1),
                "zoom_out" => self.set_ui_scale(ctx, self.ui_scale - 0.1),
                "zoom_reset" => self.set_ui_scale(ctx, 1.0),
                "magnet" | "snap" => {
                    let mut s = self.project.settings.clone();
                    if a == "magnet" {
                        s.main_track_magnet = !s.main_track_magnet
                    } else {
                        s.snapping = !s.snapping
                    }
                    self.apply(EditOp::SetSettings { settings: s });
                }
                _ => {}
            }
        }
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        self.toasts
            .retain(|(_, t, err)| t.elapsed().as_secs_f32() < toast_secs(*err));
        if self.toasts.is_empty() {
            return;
        }
        // weich ein- (0,15 s) und ausblenden (letzte 0,4 s), ohne Animationen sofort
        let (fade_in, fade_out) = (theme::anim(0.15), theme::anim(0.4));
        let mut close = None;
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -24.0))
            .order(egui::Order::Foreground)
            // nur Fehler-Toasts schließen per Klick; sonst bleibt die Fläche durchklickbar
            .interactable(self.toasts.iter().any(|t| t.2))
            .show(ctx, |ui| {
                // Meldungen nicht Wort für Wort umbrechen (die Fläche hat keine feste Breite)
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                for (i, (msg, at, err)) in self.toasts.iter().enumerate().rev().take(3) {
                    let age = at.elapsed().as_secs_f32();
                    ui.set_opacity(toast_alpha(age, toast_secs(*err), fade_in, fade_out));
                    let r = egui::Frame::default()
                        .fill(if *err {
                            theme::DANGER_BG()
                        } else {
                            theme::BG_RAISED()
                        })
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            if *err { theme::DANGER } else { theme::BORDER() },
                        ))
                        .corner_radius(theme::R_WIN)
                        .inner_margin(egui::Margin::symmetric(theme::PAD_CARD_L, theme::PAD_CARD))
                        .shadow(egui::epaint::Shadow {
                            offset: [0, 4],
                            blur: 16,
                            spread: 0,
                            color: Color32::from_black_alpha(120),
                        })
                        .show(ui, |ui| {
                            ui.label(RichText::new(toast_line(msg, *err)).color(
                                if *err {
                                    theme::DANGER_TEXT()
                                } else {
                                    theme::TEXT()
                                },
                            ));
                        })
                        .response;
                    if *err && r.interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        close = Some(i);
                    }
                    ui.add_space(4.0);
                }
            });
        if let Some(i) = close {
            self.toasts.remove(i);
        }
        let fading = self.toasts.iter().any(|(_, t, err)| {
            let a = t.elapsed().as_secs_f32();
            !(fade_in..=toast_secs(*err) - fade_out).contains(&a)
        });
        ctx.request_repaint_after(Duration::from_millis(if fading { 16 } else { 250 }));
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        // Über der Sticker- bzw. Soundliste abgelegt: in die eigene Bibliothek
        if let (Some((kind, rect)), Some(pos)) =
            (self.lib_drop, ctx.input(|i| i.pointer.hover_pos()))
        {
            if rect.contains(pos) {
                self.lib_import(kind, dropped);
                return;
            }
        }
        let (projects, rest): (Vec<_>, Vec<_>) = dropped
            .into_iter()
            .partition(|p| p.extension().is_some_and(|e| e == "cutline"));
        if let Some(p) = projects.first() {
            self.open_project(p);
            return;
        }
        if self.screen == Screen::Home && !self.new_project() {
            return;
        }
        let (fonts, media): (Vec<_>, Vec<_>) = rest.into_iter().partition(|p| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                cl_fonts::FONT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())
            })
        });
        if !fonts.is_empty() {
            self.import_fonts(ctx, fonts);
        }
        if !media.is_empty() {
            let n = self.import_media(media).len();
            if n > 0 {
                self.toast(self.i18n.t_num("media-imported", "count", n as i64));
            }
        }
    }
}

impl App {
    /// Bildtakt der Oberfläche beim Abspielen: etwa zwei UI-Bilder pro Videobild.
    /// egui zieht von der Wartezeit 1/60 s ab, deshalb wird sie hier aufgeschlagen
    /// (sonst zeichnet die Oberfläche ungebremst mehrere hundert Mal pro Sekunde).
    /// Genau auf das nächste Videobild zu zielen verliert Bilder (Windows-Timer ~15 ms).
    /// Wartezeit bis zum nächsten fälligen Bild (Projekt-Bildrate); beim Puffern kurz nachsehen.
    pub fn play_repaint_delay(&self) -> Duration {
        if self.play_warmup.is_some() {
            return Duration::from_millis(10);
        }
        next_frame_delay(self.playhead, self.project.settings.fps, self.play_rate)
    }

    /// Messpunkte der Wiedergabe (`CUTLINE_PERF`): Play → erster Ton, Ton-Aussetzer
    fn perf_audio(&mut self) {
        if !perf() {
            return;
        }
        let Some(a) = &self.audio else { return };
        if let (Some(p), Some(t)) = (&mut self.perf_play, a.first_sound()) {
            if !p.sound {
                p.sound = true;
                eprintln!(
                    "perf: {} → erster Ton {:.0} ms",
                    p.what,
                    t.saturating_duration_since(p.pressed).as_secs_f64() * 1000.0
                );
            }
        }
        let (n, d) = a.take_underruns();
        if n > 0 {
            eprintln!("perf: Ton-Aussetzer {n}× zusammen {:.0} ms bei {:.2} s", d.as_secs_f64() * 1000.0, self.playhead.as_secs_f64());
        }
    }

    /// Messpunkt: ein Strombild der Hauptebene wurde gezeigt (Play → erstes weitergeschaltetes Bild)
    pub fn perf_frame_shown(&mut self) {
        if let Some(p) = self.perf_play.as_mut().filter(|p| !p.frame && self.playhead > p.at) {
            p.frame = true;
            eprintln!("perf: {} → erstes Bild weitergeschaltet {:.0} ms", p.what, p.pressed.elapsed().as_secs_f64() * 1000.0);
        }
    }

    /// Bild-Ton-Versatz (ms): wie weit das gezeigte Strombild hinter der Uhr liegt (größter Wert
    /// über alle Ebenen; im Takt ist er kleiner als ein Bildintervall)
    fn perf_av_offset(&self) -> Option<f64> {
        self.playbacks
            .values()
            .filter(|p| p.taken > 0 && p.t0 <= self.playhead)
            .map(|p| {
                let shown = p.t0.as_secs_f64() + (p.taken - 1) as f64 / p.stream.fps.as_f64().max(1.0);
                (self.playhead.as_secs_f64() - shown) * 1000.0
            })
            .reduce(f64::max)
    }
}

impl eframe::App for App {
    /// Fenstergrund in der Farbe des Themes (sichtbar in den Lücken zwischen den Karten)
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    /// Klicktests ohne echten Mauszeiger (nur mit `CUTLINE_SHOTS`), siehe `devinput.rs`
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        crate::devinput::feed(ctx, raw);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        dev_screenshot(ctx);
        // Oberflächengröße an die Fenstergröße binden: Dialoge passen immer ins Fenster
        let zoom = ctx.zoom_factor();
        let want = crate::chrome::effective_ui_scale(self.ui_scale, ctx.screen_rect().size() * zoom);
        if (want - zoom).abs() > 0.005 {
            ctx.set_zoom_factor(want);
        }
        limit_frame_rate();
        let _frame_time = perf().then(|| PerfFrame(Instant::now()));
        if perf() {
            static FIRST: std::sync::Once = std::sync::Once::new();
            FIRST.call_once(|| perf_mark("erstes Bild"));
            self.perf_ui += 1;
            let el = self.perf_since.elapsed().as_secs_f64();
            if el >= 1.0 {
                eprintln!(
                    "perf: {:.0} ui/s, {:.0} Strombilder/s, {} Ströme geöffnet, größte Lücke {:.0} ms, playing={}, Bildzeit Ø {:.2} / max {:.1} ms",
                    self.perf_ui as f64 / el,
                    self.perf_frames as f64 / el,
                    self.perf_opens,
                    self.perf_max_gap * 1000.0,
                    self.playing,
                    PERF_SUM_US.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0 / self.perf_ui.max(1) as f64,
                    PERF_MAX_US.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0
                );
                if self.playing && self.play_warmup.is_none() {
                    if let Some(ms) = self.perf_av_offset() {
                        let clock = if self.audio.as_ref().is_some_and(|a| a.started()) { "Ton-Uhr" } else { "Systemuhr" };
                        eprintln!("perf: Bild-Ton-Versatz {ms:.0} ms ({clock})");
                    }
                }
                self.perf_max_gap = 0.0;
                self.perf_ui = 0;
                self.perf_frames = 0;
                self.perf_opens = 0;
                self.perf_since = Instant::now();
            }
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("CUTLINE_DEBUG_HITS").is_some() {
            ctx.style_mut(|s| s.debug.show_widget_hits = true);
        }
        if let Some(f) = self.pending_fonts.take() {
            let known = ctx.fonts(|fo| fo.families());
            self.registered_fonts = f
                .into_iter()
                .filter(|n| known.contains(&egui::FontFamily::Name(n.clone().into())))
                .collect();
        }
        // „System“ folgt einem Wechsel des Betriebssystems sofort
        theme::select(ctx, &self.settings.theme);
        // Vorhören zu Ende → Knopf zurück auf „Abspielen“
        if let Some((_, a)) = &self.sfx_preview {
            if a.started() && a.position() + Time::from_millis(40) >= self.sfx_preview_len {
                self.sfx_preview = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(33));
            }
        }
        if self.fx_thumbs_requested.is_empty() {
            self.preload_fx_thumbs();
        }
        self.sync_project();
        self.handle_dropped_files(ctx);
        self.warm_silent_channels();
        self.receive_decoded(ctx);
        self.poll_tasks();
        self.poll_epidemic(ctx);
        self.poll_stock();
        if self.screen == Screen::Editor {
            self.ensure_proxies(ctx);
        }

        self.window_frame(ctx);
        // Fenster-Kreuz während eines Exports: erst nachfragen
        self.export.guard_close(ctx);
        egui::TopBottomPanel::top("top")
            .exact_height(theme::TITLE_H)
            .frame(
                egui::Frame::default()
                    .fill(theme::BG_APP())
                    .inner_margin(egui::Margin {
                        left: theme::PAD_CARD_L,
                        right: 0,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show(ctx, |ui| self.title_bar(ui));

        if self.screen == Screen::Home {
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(theme::BG_APP()))
                .show(ctx, |ui| self.home_page(ui));
            self.settings_window(ctx);
            if !self.settings_open {
                self.esc_closes(ctx);
            }
            self.import_dialogs(ctx);
            self.template_dialog(ctx);
            self.external_preview(ctx);
            self.props_window(ctx);
            self.exports_window(ctx);
            self.onboarding(ctx);
            self.draw_toasts(ctx);
            return;
        }

        if !self.export.is_open()
            && self.onboard.is_none()
            && self.tour.is_none()
            && !self.esc_closes(ctx)
            // Esc im Bearbeiten-Modus der Reihenfolge beendet nur diesen
            && (self.settings_open || self.dialog_open() || !crate::library_ui::esc_order_edit(ctx))
        {
            self.shortcuts(ctx);
        }
        // Countdown vor der Voiceover-Aufnahme
        if let Some(until) = self.voiceover.countdown_until {
            if Instant::now() >= until {
                self.start_voiceover();
            }
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.advance_playhead();
        if pause_is_stale(self.paused, self.playhead, self.revision) {
            self.stop_playback();
        }
        if self.playing != self.hires_timer.is_some() {
            self.hires_timer = self.playing.then(HiresTimer::new);
        }
        if self.playing {
            ctx.request_repaint_after(self.play_repaint_delay());
        }
        self.scrub_audio(ctx);

        if !self.player_max {
            // Aufteilung relativ zur Fenstergröße: der Player behält immer genug Platz
            let sr = ctx.screen_rect();
            // Bereiche als Anteil der Fenstergröße: Wird das Fenster kleiner und wieder größer,
            // wachsen Medien, Inspektor und Zeitleiste wieder mit (statt gestaucht zu bleiben).
            let resized = self
                .layout_screen
                .is_some_and(|s| (s.width() - sr.width()).abs() > 1.0 || (s.height() - sr.height()).abs() > 1.0);
            let (fa, fi, ft) = self.layout_frac;
            let lay = self.settings.layout.clone();
            // eigene Kennungen je Seite, damit gemerkte Größen nicht zwischen den Seiten springen
            let (id_a, id_i, id_t) = (
                if lay.assets_right { "assets-r" } else { "assets" },
                if lay.assets_right { "inspector-l" } else { "inspector" },
                if lay.timeline_top { "timeline-top" } else { "timeline" },
            );
            if resized {
                let set = |id: &str, w: Option<f32>, h: Option<f32>| {
                    let id = egui::Id::new(id);
                    if let Some(mut st) = egui::containers::panel::PanelState::load(ctx, id) {
                        let size = vec2(w.unwrap_or(st.rect.width()), h.unwrap_or(st.rect.height()));
                        st.rect = egui::Rect::from_min_size(st.rect.min, size);
                        ctx.data_mut(|d| d.insert_persisted(id, st));
                    }
                };
                set(id_a, Some(sr.width() * fa), None);
                set(id_i, Some(sr.width() * fi), None);
                set(id_t, None, Some(sr.height() * ft));
            }
            let small = sr.width() < 1200.0;
            let mut th = 0.0;
            // Zeitleiste über die volle Breite: vor den Seitenbereichen anlegen
            if lay.timeline_full {
                th = self.timeline_area(ctx, id_t, lay.timeline_top, sr, ft);
            }
            let assets = if lay.assets_right { egui::SidePanel::right(id_a) } else { egui::SidePanel::left(id_a) };
            let r = assets
                .resizable(!self.settings.layout_locked)
                .default_width((sr.width() * fa).clamp(200.0, 560.0))
                .min_width(if small { 180.0 } else { 240.0 })
                .max_width((sr.width() * 0.38).max(200.0))
                .frame(theme::card(0))
                .show(ctx, |ui| fixed_width(ui, |ui| self.assets_panel(ui)))
                .response
                .rect;
            self.tour_rects.insert("assets", r);
            let aw = r.width();

            let insp = if lay.assets_right { egui::SidePanel::left(id_i) } else { egui::SidePanel::right(id_i) };
            let r = insp
                .resizable(!self.settings.layout_locked)
                .default_width((sr.width() * fi).clamp(200.0, 420.0))
                .min_width(if small { 180.0 } else { 220.0 })
                .max_width((sr.width() * 0.32).max(200.0))
                .frame(theme::card(theme::PAD_CARD_L))
                .show(ctx, |ui| fixed_width(ui, |ui| self.inspector_panel(ui)))
                .response
                .rect;
            self.tour_rects.insert("inspector", r);
            if !lay.timeline_full {
                th = self.timeline_area(ctx, id_t, lay.timeline_top, sr, ft);
            }
            // Vom Nutzer gezogene Breiten als Anteil merken (nur beim Ziehen und wenn das Fenster
            // gleich blieb: sonst überschreiben Mindestbreiten nach Zoom/Größenwechsel die Anteile)
            if !resized && sr.width() > 0.0 && sr.height() > 0.0 && ctx.input(|i| i.pointer.primary_down()) {
                self.layout_frac = (aw / sr.width(), r.width() / sr.width(), th / sr.height());
            }
            self.layout_screen = Some(sr);
        }

        let r = egui::CentralPanel::default()
            .frame(theme::card(theme::PAD_CARD))
            .show(ctx, |ui| self.player(ui))
            .response
            .rect;
        self.tour_rects.insert("player", r);

        self.layout_editor(ctx);
        self.voiceover_window(ctx);
        self.settings_window(ctx);
        // nur der offene Dialog startet Exporte und braucht die Schriften
        let fonts_owned = if self.export.is_open() { self.font_files() } else { HashMap::new() };
        // Titelbild: der Clip der Hauptspur am Playhead (sonst irgendein sichtbares Bild)
        let main = self
            .project
            .timeline
            .tracks
            .iter()
            .find(|t| t.main)
            .and_then(|t| t.clips.iter().find(|c| c.contains(self.playhead)))
            .and_then(|c| self.frames.get(&c.id));
        // nur Bilder von Clips, die es noch gibt (sonst zeigt ein leeres Projekt ein altes Bild)
        let tex = main
            .or_else(|| self.project.timeline.tracks.iter().flat_map(|t| &t.clips).find_map(|c| self.frames.get(&c.id)))
            .map(|(_, t)| (t.id(), t.size_vec2()));
        if std::mem::take(&mut self.export.chapter_hint_off) {
            self.settings.chapter_hint = false;
            self.settings.save();
        }
        self.export.chapters = self.settings.export_chapters;
        self.export.chapter_hint = self.settings.chapter_hint;
        if let Some(msg) = self.export.show(ctx, &self.project, &fonts_owned, tex, self.revision) {
            // Erfolgreicher Export mit Epidemic-Musik wird gemeldet (Nutzungsbedingungen)
            if msg.contains("Export fertig") {
                self.report_epidemic_export(ctx);
                // nur wenn nichts vorgehört wird: `preview_sound` schaltet um und würde es abbrechen
                if self.settings.export_sound
                    && !self.playing
                    && self.sfx_preview.is_none()
                    && self.sound_pending.is_none()
                {
                    self.preview_sound("builtin:sfx:notify".into(), &Self::sfx_path("notify"), Some("notify"));
                }
            }
            self.toast(msg);
        }
        // „Projekt auf 9:16 umstellen“ im Export-Dialog (die Vorgabe merkt sich der Block darunter)
        if self.export.take_switch_aspect() {
            self.set_aspect(cl_core::AspectPreset::Ratio(9, 16));
        }
        if let Some(id) = self.export.take_picked_platform() {
            if let Some(s) = &mut self.aspect_fit {
                // Formatwechsel wartet auf „Clips neu einpassen?“: Plattform kommt mit (Esc verwirft beides)
                s.platform = Some(id.to_string());
            } else if self.project.settings.platform.as_deref() != Some(id) {
                let mut s = self.project.settings.clone();
                s.platform = Some(id.to_string());
                self.apply(EditOp::SetSettings { settings: s });
            }
        }
        self.lib_dialogs(ctx);
        self.preset_dialogs(ctx);
        self.import_dialogs(ctx);
        self.marker_dialog(ctx);
        self.quick_dialog(ctx);
        self.track_rename_dialog(ctx);
        self.project_tools(ctx);
        self.paste_attrs_dialog(ctx);
        self.template_dialog(ctx);
        self.external_preview(ctx);
        self.onboarding(ctx);
        self.tour_overlay(ctx);
        self.drag_ghost(ctx);
        self.draw_toasts(ctx);
        theme::focus_ring(ctx);
        self.sync_project();
        self.autosave();
    }

    fn on_exit(&mut self) {
        // laufenden Export sauber abbrechen: kein verwaistes FFmpeg, keine .part-Datei
        self.export.cancel_and_wait(Duration::from_secs(5));
        self.cancel_background_work();
        if !self.flush_before_leaving() {
            // letzter Ausweg: Stand in die Sicherungen schreiben
            let dir = match &self.project_path {
                Some(p) => crate::project_tools::backup_dir(p),
                None => paths::data_dir().join("Backups").join(paths::safe_name(&self.project.name)),
            };
            let _ = std::fs::create_dir_all(&dir);
            let p = dir.join(format!("exit-{}.cutline", paths::unix_secs()));
            if let Err(e) = paths::write_atomic(&p, self.project.to_json()) {
                eprintln!("{}: {e}", p.display());
            }
            self.release_lock();
        }
    }
}

fn lock_path(p: &Path) -> PathBuf {
    let mut l = p.as_os_str().to_owned();
    l.push(".lock");
    PathBuf::from(l)
}

/// Fingerabdruck aller hörbaren Eigenschaften (Spuren, Tonclips, Lautstärke, Tempo, Ton-Effekte).
fn audio_signature(p: &Project) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for t in &p.timeline.tracks {
        if !matches!(t.kind, TrackKind::Video | TrackKind::Audio) {
            continue;
        }
        (t.id, t.muted, t.hidden).hash(&mut h);
        for c in &t.clips {
            let ClipContent::Media { media, .. } = &c.content else {
                continue;
            };
            (
                c.id,
                media,
                c.start,
                c.duration,
                c.source_in,
                c.disabled,
                c.fade_in,
                c.fade_out,
                c.fx.reverse,
            )
                .hash(&mut h);
            (c.speed.to_bits(), c.volume_db.to_bits()).hash(&mut h);
            serde_json::to_string(&c.audio)
                .unwrap_or_default()
                .hash(&mut h);
            serde_json::to_string(&c.fx.speed_curve)
                .unwrap_or_default()
                .hash(&mut h);
            serde_json::to_string(c.keyframes.get(Prop::Volume))
                .unwrap_or_default()
                .hash(&mut h);
        }
    }
    h.finish()
}

impl App {
    /// Beim Ziehen aus der Bibliothek: Symbol und Name am Mauszeiger, damit klar ist, was bewegt wird.
    fn drag_ghost(&self, ctx: &egui::Context) {
        let Some(a) = egui::DragAndDrop::payload::<DragAsset>(ctx) else {
            return;
        };
        let Some(pos) = ctx.pointer_latest_pos() else {
            return;
        };
        if !ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let name = |p: &std::path::Path| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let (glyph, label) = match &*a {
            DragAsset::Media(id) => match self.project.media(*id) {
                Some(m) => (
                    match m.kind {
                        MediaKind::Video => icon::FILM_STRIP,
                        MediaKind::Image => icon::IMAGE,
                        MediaKind::Audio => icon::MUSIC_NOTE,
                    },
                    name(&m.path),
                ),
                None => (icon::FILE, String::new()),
            },
            DragAsset::Text(_) => (icon::TEXT_T, crate::tr("Text")),
            DragAsset::Sticker(id) => (
                icon::STICKER,
                cl_fx::stickers::sticker(id)
                    .map(|s| crate::tr(s.name))
                    .unwrap_or_default(),
            ),
            DragAsset::OwnSticker(p) => (icon::STICKER, name(p)),
            DragAsset::Effect(id) => (icon::SPARKLE, crate::tr(cl_fx::fx_name(id))),
            DragAsset::Filter(id) => (icon::CIRCLE_HALF, crate::tr(cl_fx::fx_name(id))),
            DragAsset::Transition(id) => (
                icon::ARROWS_LEFT_RIGHT,
                crate::tr(cl_fx::transition_name(id)),
            ),
            DragAsset::Sfx(id) => (
                icon::WAVEFORM,
                cl_fx::sfx::sfx(id)
                    .map(|s| crate::tr(s.name))
                    .unwrap_or_default(),
            ),
            DragAsset::File(p) => (icon::FILE, name(p)),
        };
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        egui::Area::new(egui::Id::new("drag-ghost"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos + egui::vec2(14.0, 12.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(theme::BG_RAISED())
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER()))
                    .corner_radius(theme::R_WIN)
                    .inner_margin(egui::Margin::symmetric(theme::PAD_CARD, 4))
                    .shadow(egui::Shadow {
                        offset: [0, 4],
                        blur: 12,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(120),
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Icon-Glyphe
                            ui.label(egui::RichText::new(glyph).size(16.0).color(theme::TEXT()));
                            let mut l = label;
                            if l.chars().count() > 28 {
                                l = format!("{}…", l.chars().take(27).collect::<String>());
                            }
                            if !l.is_empty() {
                                ui.label(egui::RichText::new(l).size(theme::FS_UI).color(theme::TEXT()));
                            }
                        });
                    });
            });
    }
}

/// Messung mit `CUTLINE_PERF` (Variable einmal gelesen, nicht in jedem Bild)
pub fn perf() -> bool {
    static P: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *P.get_or_init(|| std::env::var_os("CUTLINE_PERF").is_some())
}

/// Messung: Rechenzeit je UI-Bild (Summe und Maximum seit der letzten `perf`-Zeile, µs)
static PERF_SUM_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PERF_MAX_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
struct PerfFrame(Instant);
impl Drop for PerfFrame {
    fn drop(&mut self) {
        let us = self.0.elapsed().as_micros() as u64;
        PERF_SUM_US.fetch_add(us, std::sync::atomic::Ordering::Relaxed);
        PERF_MAX_US.fetch_max(us, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Messung (`CUTLINE_PERF`): eine Wiedergabe bzw. ein Ton-Neustart
pub struct PerfPlay {
    pub what: &'static str,
    pub pressed: Instant,
    /// Playhead beim Start (erstes Bild „weitergeschaltet“ = Playhead danach)
    pub at: Time,
    pub frame: bool,
    pub sound: bool,
}

impl PerfPlay {
    fn new(what: &'static str, at: Time) -> Self {
        Self { what, pressed: Instant::now(), at, frame: what != "Play", sound: false }
    }
}

/// Feiner Zeitgeber, solange er lebt (Windows: `timeBeginPeriod(1)`; die Ereignisschleife wartet
/// sonst im 15,6-ms-Raster und Bilder kommen zu spät). Andere Systeme sind ohnehin genau.
struct HiresTimer {
    #[cfg(windows)]
    ok: bool,
}

#[cfg(windows)]
#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}

impl HiresTimer {
    fn new() -> Self {
        // SAFETY: einfacher Systemaufruf ohne Zeiger; 0 = Erfolg (sonst bleibt der grobe Takt)
        #[cfg(windows)]
        return Self { ok: unsafe { timeBeginPeriod(1) } == 0 };
        #[cfg(not(windows))]
        Self {}
    }
}

impl Drop for HiresTimer {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.ok {
            // SAFETY: Gegenstück zum erfolgreichen timeBeginPeriod(1)
            unsafe { timeEndPeriod(1) };
        }
    }
}

/// Beginn von Bild `n` bei `fps` (µs, aufgerundet: ab dann ist das Bild fällig)
fn frame_start(n: i128, fps: Rational) -> i128 {
    let (num, den) = (fps.num.max(1) as i128, fps.den.max(1) as i128);
    (n * den * 1_000_000).div_euclid(num) + i128::from((n * den * 1_000_000).rem_euclid(num) != 0)
}

/// Nächster Bildwechsel nach `pos` in Laufrichtung (aus der Bildnummer, also ohne Drift)
fn next_frame_at(pos: Time, fps: Rational, forward: bool) -> Time {
    let (num, den) = (fps.num.max(1) as i128, fps.den.max(1) as i128);
    let n = (pos.0 as i128 * num).div_euclid(den * 1_000_000);
    let mut at = if forward { frame_start(n + 1, fps) } else { frame_start(n, fps) };
    if !forward && at >= pos.0 as i128 {
        at = frame_start(n - 1, fps);
    }
    Time(at as i64)
}

/// Wartezeit bis zum nächsten Bildwechsel bei Abspieltempo `rate` (+1 ms, damit die Uhr dann
/// sicher hinter der Grenze steht; ein zu frühes Bild zeigte nichts Neues)
fn next_frame_delay(pos: Time, fps: Rational, rate: f64) -> Duration {
    let rate = if rate.abs() < 1e-6 { 1.0 } else { rate };
    let at = next_frame_at(pos, fps, rate > 0.0);
    let secs = ((at - pos).as_secs_f64() / rate).abs();
    Duration::from_secs_f64(secs.min(1.0)) + Duration::from_millis(1)
}

/// Pause mit offenen Strömen, aber Playhead verschoben oder Projekt geändert: Fortsetzen geht
/// nicht mehr, die wartenden FFmpeg-Prozesse (Bild und Ton) können sofort enden.
fn pause_is_stale(paused: Option<(Time, u64)>, playhead: Time, revision: u64) -> bool {
    paused.is_some_and(|(at, rev)| at != playhead || rev != revision)
}

/// Kürzester Abstand zweier UI-Bilder (höchstens 125 je Sekunde). Ohne VSync (siehe `main.rs`)
/// zeichnet egui bei jeder Animation (Hover-Kacheln, Einblenden) sonst ungebremst mehrere hundert
/// Bilder je Sekunde und belegt einen ganzen Kern. Abspielen fordert seltener an (`play_repaint_delay`).
const MIN_FRAME: Duration = Duration::from_millis(8);

/// Wartezeit vor dem nächsten UI-Bild, wenn das letzte vor `since_last` begann
fn frame_wait(since_last: Duration) -> Duration {
    MIN_FRAME.saturating_sub(since_last)
}

fn limit_frame_rate() {
    thread_local!(static LAST: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) });
    LAST.with(|last| {
        if let Some(wait) = last.get().map(|t| frame_wait(t.elapsed())).filter(|w| !w.is_zero()) {
            std::thread::sleep(wait);
        }
        last.set(Some(Instant::now()));
    });
}

/// Messpunkt beim Start (`CUTLINE_PERF`): Zeit seit dem ersten Aufruf (in `main`)
pub fn perf_mark(what: &str) {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let t0 = *T0.get_or_init(Instant::now);
    if perf() {
        eprintln!("perf: Start {what} nach {} ms", t0.elapsed().as_millis());
    }
}

/// Für Dokumentation: Mit `CUTLINE_SHOTS=<Ordner>` speichert Cutline das Fensterbild als PNG,
/// sobald dort eine Datei `take` liegt (direkt aus der App, ohne überlagernde Fenster).
fn dev_screenshot(ctx: &egui::Context) {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    let Some(dir) = DIR
        .get_or_init(|| std::env::var_os("CUTLINE_SHOTS").filter(|d| !d.is_empty()).map(PathBuf::from))
        .as_ref()
    else {
        return;
    };
    let trigger = dir.join("take");
    if trigger.is_file() {
        let _ = std::fs::remove_file(&trigger);
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(300));
    let shots: Vec<std::sync::Arc<egui::ColorImage>> = ctx.input(|i| {
        i.raw
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
            .collect()
    });
    for img in shots {
        let [w, h] = img.size;
        let buf: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
        let n = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
        if let Some(im) = image::RgbaImage::from_raw(w as u32, h as u32, buf) {
            let _ = im.save(dir.join(format!("shot-{n:02}.png")));
        }
    }
}

/// Inhalt in genau der verfügbaren Fläche zeichnen. Seitenleisten von egui wachsen sonst mit
/// ihrem breitesten Inhalt mit, und beim Tab-Wechsel springt die ganze Oberfläche.
/// So bestimmt nur der Nutzer (Ziehen am Rand) die Breite.
pub fn fixed_width(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let rect = ui.available_rect_before_wrap();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(*ui.layout()));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    add(&mut child);
    ui.allocate_rect(rect, egui::Sense::hover());
}

/// Standzeit eines Toasts: Fehler bleiben länger stehen (und schließen per Klick)
/// Text eines Toasts mit Symbol davor; bringt die Meldung schon ein eigenes Symbol mit, bleibt
/// es bei diesem (sonst stünden zwei Häkchen vor „Gespeichert“).
fn toast_line(msg: &str, err: bool) -> String {
    if msg.starts_with(|c: char| ('\u{E000}'..='\u{F8FF}').contains(&c)) {
        return msg.to_string();
    }
    let g = if err { icon::WARNING_CIRCLE } else { icon::CHECK_CIRCLE };
    format!("{g}  {msg}")
}

fn toast_secs(err: bool) -> f32 {
    if err { 10.0 } else { 4.0 }
}

/// Deckkraft eines Toasts im Alter `age` (s); Ein-/Ausblenddauer 0 = sofort da/weg
fn toast_alpha(age: f32, life: f32, fade_in: f32, fade_out: f32) -> f32 {
    let a_in = if fade_in > 0.0 { age / fade_in } else { 1.0 };
    let a_out = if fade_out > 0.0 { (life - age) / fade_out } else if age < life { 1.0 } else { 0.0 };
    a_in.min(a_out).clamp(0.0, 1.0)
}

/// `saved_revision` nach `set_doc`: keine echte Revision trifft diesen Wert.
pub const UNSAVED: u64 = u64::MAX - 1;

/// Ungespeicherte Änderungen schreiben? Ein leeres neues Projekt legt keine Datei an; ein
/// gespeichertes, das leer geräumt wurde, wird gespeichert (sonst käme der alte Stand wieder).
fn needs_save(rev: u64, saved: u64, has_file: bool, p: &Project) -> bool {
    rev != saved && (has_file || !(p.media.is_empty() && p.duration() == Time::ZERO))
}

/// Zeitpunkt des Vorschaubilds eines Mediums: Video kurz nach Beginn, bewegtes Bild (APNG/GIF) in
/// der Mitte – viele Elemente beginnen mit einem leeren Bild und blenden erst dann ein.
fn thumb_time(kind: MediaKind, dur: Time, anim: Option<&cl_media::Animation>) -> Time {
    match (kind, anim) {
        (MediaKind::Video, _) => Time::from_secs_f64((dur.as_secs_f64() * 0.1).min(1.0)),
        (_, Some(a)) => a.seek_time(Time(a.duration.0 / 2)),
        _ => Time::ZERO,
    }
}

#[cfg(test)]
mod toast_tests {
    use super::*;

    #[test]
    fn animated_image_thumb_is_not_first_frame() {
        let a = cl_media::Animation {
            starts: (0..10).map(|i| Time::from_secs_f64(i as f64 * 0.1)).collect(),
            duration: Time::from_secs_f64(1.0),
            looping: false,
        };
        assert!(thumb_time(MediaKind::Image, Time::ZERO, Some(&a)) >= Time::from_secs_f64(0.4));
        assert_eq!(thumb_time(MediaKind::Image, Time::ZERO, None), Time::ZERO);
        assert_eq!(thumb_time(MediaKind::Video, Time::from_secs_f64(60.0), None), Time::from_secs_f64(1.0));
    }

    #[test]
    fn replaced_document_counts_as_unsaved() {
        // Wiederherstellen/Vorlage/Rückblick ersetzen das Dokument: Revision beginnt bei 0
        let p = Project::new("x");
        let rev = Document::new(p.clone()).revision();
        assert!(needs_save(rev, UNSAVED, true, &p));
    }

    #[test]
    fn emptied_saved_project_is_saved() {
        let empty = Project::new("x");
        assert!(needs_save(5, 3, true, &empty), "gespeichert und leer geräumt");
        assert!(!needs_save(5, 3, false, &empty), "neues leeres Projekt: keine Datei");
        assert!(!needs_save(3, 3, true, &empty));
    }

    #[test]
    fn toast_shows_one_symbol() {
        let saved = toast_line(&format!("{} Gespeichert", icon::CHECK), false);
        assert_eq!(saved.matches(icon::CHECK).count() + saved.matches(icon::CHECK_CIRCLE).count(), 1, "{saved}");
        assert_eq!(toast_line(&format!("{} Export fertig (1.0 s)", icon::CHECK_CIRCLE), false).matches(icon::CHECK_CIRCLE).count(), 1);
        // ohne eigenes Symbol: Häkchen bzw. Warnung davor
        assert_eq!(toast_line("Kopiert", false), format!("{}  Kopiert", icon::CHECK_CIRCLE));
        assert!(toast_line("Fehler", true).starts_with(icon::WARNING_CIRCLE));
    }

    #[test]
    fn toast_alpha_without_animation() {
        assert_eq!(toast_alpha(0.0, 4.0, 0.0, 0.0), 1.0);
        assert_eq!(toast_alpha(3.99, 4.0, 0.0, 0.0), 1.0);
        assert_eq!(toast_alpha(4.0, 4.0, 0.0, 0.0), 0.0);
        // mit Animationen wie bisher
        assert_eq!(toast_alpha(0.0, 4.0, 0.15, 0.4), 0.0);
        assert!((toast_alpha(3.8, 4.0, 0.15, 0.4) - 0.5).abs() < 1e-4);
        assert!(toast_secs(true) > toast_secs(false));
    }
}

#[cfg(test)]
mod frame_rate_tests {
    use super::*;

    #[test]
    fn moved_or_edited_pause_releases_streams() {
        let t = Time::from_millis(500);
        assert!(!pause_is_stale(None, t, 3));
        assert!(!pause_is_stale(Some((t, 3)), t, 3));
        assert!(pause_is_stale(Some((t, 3)), Time::from_millis(900), 3));
        assert!(pause_is_stale(Some((t, 3)), t, 4));
    }

    #[test]
    fn next_frame_is_due_on_the_frame_grid_without_drift() {
        for fps in [Rational::new(24, 1), Rational::new(25, 1), Rational::new(30, 1), Rational::new(50, 1), Rational::new(60, 1), Rational::new(30000, 1001)] {
            let f = fps.as_f64();
            let mut pos = Time::ZERO;
            for n in 1..=10_000i64 {
                pos = next_frame_at(pos, fps, true);
                // genau der (aufgerundete) Bildbeginn, unabhängig von der Vorgeschichte
                let exact = n as f64 / f * 1e6;
                assert!(pos.0 as f64 >= exact - 1e-6 && (pos.0 as f64) < exact + 1.0, "{fps:?} Bild {n}: {pos:?}");
                // die Uhr liegt dort wirklich im Bild n
                assert_eq!((pos.as_secs_f64() * f + 1e-9).floor() as i64, n, "{fps:?} {n}");
            }
            // mitten im Bild: Wartezeit bis zum Ende des Bildes (+1 ms), nie länger als ein Bild
            let mid = Time::from_secs_f64(100.5 / f);
            let d = next_frame_delay(mid, fps, 1.0).as_secs_f64();
            assert!((d - (0.5 / f + 0.001)).abs() < 2e-6, "{fps:?} {d}");
            assert!(next_frame_delay(Time::from_secs_f64(7.0 / f), fps, 1.0).as_secs_f64() <= 1.0 / f + 0.0011);
        }
        // 30 fps: größte Lücke 33,3 ms + 1 ms ≤ 40 ms
        let worst = (0..1000)
            .map(|i| next_frame_delay(Time(i * 997), Rational::new(30, 1), 1.0))
            .max()
            .unwrap();
        assert!(worst <= Duration::from_millis(40), "{worst:?}");
        // doppeltes Tempo: halbe Wartezeit; rückwärts: bis zum Beginn des laufenden Bildes
        let fps = Rational::new(30, 1);
        let t = Time::from_secs_f64(10.5 / 30.0);
        assert!((next_frame_delay(t, fps, 2.0).as_secs_f64() - (0.25 / 30.0 + 0.001)).abs() < 2e-6);
        assert_eq!(next_frame_at(t, fps, false), Time(frame_start(10, fps) as i64));
        assert_eq!(next_frame_at(Time(frame_start(10, fps) as i64), fps, false), Time(frame_start(9, fps) as i64));
        assert!((next_frame_delay(t, fps, -1.0).as_secs_f64() - (0.5 / 30.0 + 0.001)).abs() < 2e-6);
    }

    #[test]
    fn frame_wait_caps_ui_rate() {
        assert_eq!(frame_wait(Duration::from_millis(2)), Duration::from_millis(6));
        assert_eq!(frame_wait(Duration::ZERO), MIN_FRAME);
        // seltene Bilder (Leerlauf, Abspielen) warten nie
        assert_eq!(frame_wait(Duration::from_millis(30)), Duration::ZERO);
        assert!(1.0 / MIN_FRAME.as_secs_f64() <= 125.0);
    }
}
