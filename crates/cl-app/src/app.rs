//! App state and all actions. UI: `chrome` (title bar), `home` (start page),
//! `assets` (left), `player`, `inspector` (right), `timeline`, `settings`, `voiceover`.

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
use crate::playcore::{AudioIn, Cmd, Mode, PlayCore, Tick};
use crate::preview::{Decoder, Job, Starter};
use crate::record::Recorder;
use crate::settings::{AppSettings, McpRunning, Section};
use crate::theme::{self, icon};
use crate::{paths, timeline::TimelineState};

/// Take focus away from text fields (shortcuts then go to the app again).
/// Is text being typed right now? Only a focused text field (also a number field in
/// input mode) – not just any widget with focus: a focused button or tile
/// (e.g. after dragging a transition) would otherwise swallow all shortcuts, the space bar too.
pub fn typing(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.focused()).is_some_and(|id| egui::text_edit::TextEditState::load(ctx, id).is_some())
}

/// What the shortcuts do with the focus
#[derive(Debug, PartialEq, Eq)]
enum KeyFocus {
    /// Keys belong to the focused element (text field; Tab navigation, arrows on a slider)
    Keep,
    /// Release focus, then shortcuts (space = play instead of clicking the focused button)
    Release,
    /// nothing focused: shortcuts
    Shortcuts,
}

/// `typing`: a text field has the focus; `focused`: any element has it; `space`: space bar pressed.
fn key_focus(typing: bool, focused: bool, space: bool) -> KeyFocus {
    match (typing, focused, space) {
        (true, ..) | (false, true, false) => KeyFocus::Keep,
        (false, true, true) => KeyFocus::Release,
        (false, false, _) => KeyFocus::Shortcuts,
    }
}

/// Take the space bar out of the input before drawing unless a text field has the focus: egui would
/// otherwise trigger the focused button (after Tab e.g. the window X – the app closed). The focus
/// is released; the shortcuts get the key back (Play/Pause). Enter still triggers focused
/// buttons.
pub fn hold_space(ctx: &egui::Context, raw: &mut egui::RawInput) -> Vec<egui::Event> {
    if typing(ctx) {
        return Vec::new();
    }
    let (held, rest) = std::mem::take(&mut raw.events).into_iter().partition(|e| matches!(e, egui::Event::Key { key: Key::Space, .. }));
    raw.events = rest;
    let held: Vec<egui::Event> = held;
    if !held.is_empty() {
        release_text_focus(ctx);
    }
    held
}

/// Esc in a text or value field only discards its input: the shortcut „Auswahl aufheben" must
/// no longer act in the same frame (the field releases focus on Esc, afterwards it would see the key).
fn esc_for_field(typing: bool, esc_pressed: bool) -> bool {
    typing && esc_pressed
}

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
    /// Cut-out, green screen, image quality, stabilize & co. (video/image)
    Smart,
    Animation,
    Speed,
    Audio,
}

/// What can be dragged from the asset area into the timeline or player.
#[derive(Clone, Debug)]
pub enum DragAsset {
    Media(MediaId),
    /// Text, optionally with a template
    Text(Option<&'static str>),
    Sticker(&'static str),
    /// Own sticker / own image from the library
    OwnSticker(PathBuf),
    Effect(String),
    Filter(String),
    /// Built-in (`xfade` name) or package transition (`<paket-id>/<asset-id>`)
    Transition(String),
    Sfx(&'static str),
    File(PathBuf),
}

/// State of the voice-over panel.
pub struct Voiceover {
    pub open: bool,
    pub device: Option<String>,
    pub devices: Vec<String>,
    pub mute_project: bool,
    pub countdown: bool,
    pub gain_db: f32,
    pub countdown_until: Option<Instant>,
    pub recorder: Option<Recorder>,
    /// 0 = recording, 1 = settings
    pub tab: usize,
    /// Volume and audio tools for the new recording
    pub volume_db: f32,
    pub audio: AudioFx,
}

pub struct App {
    pub doc: Arc<Mutex<Document>>,
    /// Copy of the project for display, renewed on every revision.
    pub project: Project,
    pub revision: u64,
    /// Counts every change of `project` (also a new document with the same revision)
    pub project_epoch: u64,
    pub screen: Screen,
    pub settings: AppSettings,
    pub settings_open: bool,
    pub settings_section: Section,
    pub home: crate::home::HomeState,
    pub fonts: Arc<Mutex<cl_fonts::FontLibrary>>,
    pub system_font_count: Arc<Mutex<Option<usize>>>,
    pub plugins: cl_plugin::Registry,
    /// Epidemic Sound integration (only active when the plugin is switched on)
    pub es: crate::epidemic::EsState,
    /// Own stickers and sounds
    pub library: crate::library::Library,
    /// The user's colours, fonts and logo (`brand.json`)
    pub brand: crate::brand::BrandKit,
    /// `brand` changed, `brand.json` not yet written (see `brand_flush`)
    pub brand_unsaved: bool,
    pub lib_dialog: Option<crate::library_ui::LibDialog>,
    pub lib_dialog_since: u64,
    /// Saved look presets
    pub presets: Vec<crate::presets::Preset>,
    pub preset_dialog: Option<crate::presets::PresetDialog>,
    pub preset_dialog_since: u64,
    pub preset_list_open: bool,
    /// Project import from other programs
    pub import: crate::import_ui::ImportState,
    /// In/out range and loop
    pub range_in: Option<Time>,
    pub range_out: Option<Time>,
    pub loop_range: bool,
    /// Tracks heard solo (preview only, not in the export)
    pub solo: HashSet<TrackId>,
    /// Settings page: action whose key is being recorded right now
    pub capture_key: Option<&'static str>,
    /// Open marker editor
    pub marker_edit: Option<u64>,
    pub marker_draft: Option<cl_core::Marker>,
    /// Rename track: (track, input)
    pub track_rename: Option<(TrackId, String)>,
    /// Level meter: (value, hold, hold time) left/right
    pub meter: [(f32, f32, f32); 2],
    pub paste_attrs: crate::edit_tools::PasteAttrs,
    pub paste_attrs_open: bool,
    pub tpl_dialog: Option<crate::templates::TplDialog>,
    /// Media whose proxy could not be created (do not retry)
    pub proxy_failed: HashSet<MediaId>,
    /// Preview in its own window
    pub ext_preview: bool,
    /// Preview window in full screen (borderless maximized)
    pub ext_full: bool,
    /// Preview size at the start of playback (stays until paused)
    pub play_canvas_px: Option<egui::Vec2>,
    /// stream of a heavy layer opened ahead while stopped (see `App::update_preroll`)
    pub preroll: Option<crate::player::Preroll>,
    /// Layout (media width, inspector width, timeline height) as a share of the window
    pub layout_frac: (f32, f32, f32),
    pub layout_screen: Option<egui::Rect>,
    /// Introduction (first start) and tour
    pub onboard: Option<crate::onboarding::Onboard>,
    pub tour: Option<usize>,
    /// Position of the areas for the tour
    pub tour_rects: HashMap<&'static str, egui::Rect>,
    /// Preview: masked effect elements (clip, second version, mask)
    pub fx_overlays: Vec<(ClipId, ClipId, Option<Mask>, f32)>,
    /// Measurement (CUTLINE_PERF): UI frames; streams and frames are counted by the player core
    pub perf_ui: u32,
    pub perf_since: Instant,
    /// Measurement: start of playback or of the audio restart (first frame, first sound still pending?)
    pub perf_play: Option<PerfPlay>,
    /// running playback for the normal log (one line when it ends)
    pub play_log: Option<PlayLog>,
    /// Fine system timer while playing (Windows: 1 ms instead of 15.6 ms)
    hires_timer: Option<HiresTimer>,
    /// Multicam mode: (angle clips, switches (time, index))
    pub multicam: Option<(Vec<ClipId>, Vec<(Time, usize)>)>,
    /// Result list of „Highlights finden“ (window open)
    pub shorts: Option<crate::shorts::ShortsState>,
    /// Window „Thumbnail“ (open) and its last used settings
    pub thumb: Option<crate::thumb::ThumbWin>,
    pub thumb_opts: crate::thumb::Opts,
    /// New aspect ratio waiting for the choice „Clips neu einpassen?“
    pub aspect_fit: Option<cl_core::ProjectSettings>,
    /// Guides for platform controls in the player
    pub safe_area: bool,
    /// Playback volume (slider in the player, monitoring only)
    pub volume: crate::audio::Volume,
    /// Multicam: index of the angle whose sound runs throughout
    pub multicam_audio: usize,
    /// Quick action window (pauses, filler words, ducking, punch-in)
    pub quick: crate::quick::Quick,
    /// Characters (plugin „Story-Werkzeuge“)
    pub characters: Vec<crate::story::Character>,
    /// Plugins Freesound and Pexels
    pub stock: crate::stock::StockState,
    /// Last automatic backup
    pub last_backup: Option<Instant>,
    pub backup_revision: u64,
    pub backups_open: bool,
    /// Missing media after opening
    pub missing_open: bool,
    /// Running archiving: progress/result
    pub archive: Option<std::sync::mpsc::Receiver<Result<PathBuf, String>>>,
    pub lib_thumbs: crate::library_ui::Thumbs,
    /// Drop area of the library list in the last frame (files there land in the library)
    pub lib_drop: Option<(crate::library::Kind, egui::Rect)>,
    pub i18n: I18n,
    pub tab: LeftTab,
    pub inspector_tab: InspectorTab,
    pub clip_tab: ClipTab,
    pub asset_cat: HashMap<&'static str, String>,
    /// Last clicked clip (inspector, player frame)
    pub selected: Option<ClipId>,
    /// Further selected clips (multi-selection)
    pub sel_extra: HashSet<ClipId>,
    /// Selected transition (clip whose `transition_out` is meant)
    pub selected_transition: Option<ClipId>,
    pub clipboard: Vec<Clip>,
    /// Origin track per clip of the clipboard (same order)
    pub clipboard_src: Vec<TrackId>,
    /// Track clicked after copying: Ctrl+V pastes there (otherwise origin track)
    pub paste_track: Option<TrackId>,
    /// Put a marker into the system clipboard on the next frame
    pub copy_marker: bool,
    /// Clip with a running edit in the inspector (preview, not yet in the history)
    pub live: Option<Clip>,
    /// Space bar of this frame, intercepted before drawing (see [`hold_space`])
    held_keys: Vec<egui::Event>,
    /// Esc came while a text field had the focus: applies only to the field (see [`esc_for_field`])
    esc_field: bool,
    /// Command search (Ctrl+K)
    pub palette: crate::palette::Palette,
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
    pub(crate) toasts: Vec<(String, Instant, bool)>,
    pub decoder: Decoder,
    /// Preview textures per clip (+ key of the displayed frame)
    pub frames: HashMap<ClipId, (u64, egui::TextureHandle)>,
    /// Player core: playback state, streams, clock, automatic quality (`playcore`);
    /// `playing` is only its mirror for the UI
    pub core: PlayCore<cl_media::FrameStream>,
    /// opens playback streams in the background
    pub starter: Starter,
    /// Reference point of the core's time
    pub core_t0: Instant,
    /// Fingerprint of the effects per clip for the edit state (`player::fx_hash`)
    pub fx_hashes: (u64, HashMap<ClipId, u64>),
    /// Fingerprint of everything audible (restart sound only on changes)
    pub audio_sig: u64,
    pub audio: Option<AudioPlayer>,
    pub sfx_preview: Option<(String, AudioPlayer)>,
    /// Length of the sound being previewed (detect end, show progress)
    pub sfx_preview_len: Time,
    /// Library sounds: checked medium and waveform (40 bars), loaded in the background
    pub sound_meta: HashMap<String, (MediaRef, Vec<f32>)>,
    pub sound_meta_req: HashSet<String>,
    /// Waveforms of the sound tiles (remembered on disk, there at once)
    pub sound_bars: crate::soundbars::SoundBars,
    /// Play as soon as this sound's data is there
    pub sound_pending: Option<String>,
    pub thumbs: HashMap<MediaId, egui::TextureHandle>,
    /// Waveform per file and audio track (0 = first)
    pub waves: HashMap<(MediaId, u32), Vec<f32>>,
    pub fx_thumbs: HashMap<String, egui::TextureHandle>,
    pub fx_thumbs_requested: HashSet<String>,
    /// Right click → properties
    pub props: Option<crate::props::Props>,
    /// Window „Exporte“ for this project (name)
    pub exports_for: Option<crate::exports::ExportsList>,
    /// Lock file of the open project (see `take_lock`)
    lock_file: Option<PathBuf>,
    /// Project file whose lock another running window held when this one opened it: never written
    pub(crate) locked_out: Option<PathBuf>,
    /// Sound while dragging: (last place, time, stop signal of the running snippet)
    pub scrub: Option<(Time, Instant, Option<Arc<std::sync::atomic::AtomicBool>>)>,
    /// Subtitle whose text field gets the focus in the next frame
    pub caption_focus: Option<ClipId>,
    /// Input error in the currently open dialog (shown there, the dialog stays open)
    pub dialog_error: Option<String>,
    /// „Layout bearbeiten“ active; name for a new profile
    pub layout_edit: bool,
    pub layout_name: String,
    /// Error message in the settings window (stays open instead of only a message at the bottom)
    pub settings_error: Option<String>,
    /// Question/window of the settings page „Inhalte“
    pub contents_dialog: Option<crate::contents::ContentsDialog>,
    /// Downloadable packages: list and downloads
    pub packs: crate::contents::PackDownloads,
    /// Used cache (last counted, bytes)
    pub cache_size: Option<(Instant, u64)>,
    pub sticker_tex: HashMap<&'static str, egui::TextureHandle>,
    pub updater: crate::update::Updater,
    pub tasks: crate::tasks::Tasks,
    pub pick: Option<crate::pro::Pick>,
    pub pick_origin: Option<egui::Pos2>,
    pub censor_drag: Option<crate::pro::CensorDrag>,
    pub frame_cpu: Option<(ClipId, cl_media::RgbaFrame)>,
    /// Preview of shader transitions (cl-gpu)
    pub shader_preview: crate::player::ShaderPreview,
    pub caption_lang: String,
    pub caption_from_clip: bool,
    registered_fonts: HashSet<String>,
    /// egui takes over `set_fonts` only in the next frame. Until then new families stay here.
    pending_fonts: Option<HashSet<String>>,
    /// Font names for the inspector (sorted, re-read at most every 2 s)
    pub font_names: Option<(Instant, Arc<Vec<String>>)>,
    /// Stereo check (one channel silent?) already started: files, last checked revision
    silent_warmed: (HashSet<PathBuf>, u64),
    /// Proxy folder last counted: (time, count, bytes)
    pub proxy_stats: Option<(Instant, u64, u64)>,
    /// Source per running stream (original or proxy parts), see `proxy::App::stream_pick`
    pub proxy_picks: HashMap<u128, crate::proxy::Pick>,
    /// Properties window: rows computed once (target, revision, rows)
    pub props_cache: Option<(crate::props::Props, u64, crate::props::PropsRows)>,
    /// Project currently being opened in the background
    pub load: Option<crate::loading::Load>,
    /// Media present? (checked in the background, never per frame)
    pub presence: crate::loading::Presence,
    /// Drag files from outside into the timeline
    pub drops: crate::file_drop::Drops,
    /// Saving, autosave and backups in the background
    pub saver: crate::saving::Saver,
    /// Thumbnail already requested (medium, path): not again on every change
    thumb_requested: HashSet<(MediaId, PathBuf)>,
    /// Proxy build: cache, counters, pause
    pub proxies: crate::proxy::ProxyState,
    /// to wake the UI from background jobs
    pub ui_ctx: egui::Context,
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
        let last_used = crate::quick::load_last_used();
        crate::audio::set_volume(last_used.3);
        let mut app = Self {
            volume: last_used.3,
            doc: Arc::new(Mutex::new(Document::new(project.clone()))),
            project,
            revision: 0,
            project_epoch: 0,
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
            brand: crate::brand::BrandKit::load(),
            brand_unsaved: false,
            lib_dialog: None,
            lib_dialog_since: 0,
            presets: crate::presets::load(),
            preset_dialog: None,
            preset_dialog_since: 0,
            preset_list_open: false,
            import: Default::default(),
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
            preroll: None,
            layout_frac,
            layout_screen: None,
            onboard,
            tour: None,
            tour_rects: HashMap::new(),
            fx_overlays: Vec::new(),
            perf_ui: 0,
            perf_since: Instant::now(),
            perf_play: None,
            play_log: None,
            hires_timer: None,
            multicam: None,
            shorts: None,
            thumb: None,
            thumb_opts: last_used.2,
            aspect_fit: None,
            safe_area: last_used.1,
            multicam_audio: 0,
            quick: last_used.0,
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
            held_keys: Vec::new(),
            esc_field: false,
            palette: Default::default(),
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
            core: PlayCore::default(),
            starter: Starter::new(cc.egui_ctx.clone()),
            core_t0: Instant::now(),
            fx_hashes: (u64::MAX, HashMap::new()),
            audio_sig: 0,
            audio: None,
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
            locked_out: None,
            scrub: None,
            caption_focus: None,
            dialog_error: None,
            layout_edit: false,
            layout_name: String::new(),
            settings_error: None,
            contents_dialog: None,
            packs: Default::default(),
            cache_size: None,
            sticker_tex: HashMap::new(),
            updater: Default::default(),
            tasks: Default::default(),
            pick: None,
            pick_origin: None,
            censor_drag: None,
            frame_cpu: None,
            shader_preview: Default::default(),
            caption_lang: "auto".into(),
            caption_from_clip: false,
            registered_fonts: HashSet::new(),
            pending_fonts: None,
            font_names: None,
            silent_warmed: (HashSet::new(), u64::MAX),
            proxy_stats: None,
            proxy_picks: HashMap::new(),
            props_cache: None,
            load: None,
            presence: Default::default(),
            drops: Default::default(),
            saver: crate::saving::Saver::new(cc.egui_ctx.clone()),
            thumb_requested: HashSet::new(),
            proxies: Default::default(),
            ui_ctx: cc.egui_ctx.clone(),
        };
        app.register_fonts(&cc.egui_ctx);
        crate::brand::publish(&cc.egui_ctx, &app.brand);
        app.prewarm_sounds();
        cc.egui_ctx.set_zoom_factor(app.ui_scale);
        if app.settings.mcp_enabled {
            app.start_mcp();
        }
        crate::update::startup_cleanup();
        if app.settings.auto_update && std::env::var_os("CUTLINE_NO_UPDATE").is_none() {
            app.updater.check(&cc.egui_ctx);
        }
        app.note_started_version();
        crate::applog::startup(cc, &app.settings);
        if let Some(p) = open {
            app.open_project(&p);
        }
        // click test (only with CUTLINE_SHOTS): import a draft without a file dialog
        if std::env::var_os("CUTLINE_SHOTS").is_some() {
            if let Some(d) = std::env::var_os("CUTLINE_IMPORT_DRAFT") {
                app.import.open = true;
                app.start_import(PathBuf::from(d), &cc.egui_ctx);
            }
        }
        perf_mark("App bereit");
        app
    }

    pub fn t(&self, key: &str) -> String {
        self.i18n.t(key)
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::info!(target: "meldung", "{}", crate::applog::anon(&msg));
        self.toasts
            .push((crate::tr(msg), Instant::now(), false));
    }
    pub fn toast_err(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::error!(target: "meldung", "{}", crate::applog::anon(&msg));
        self.toasts
            .push((crate::tr(msg), Instant::now(), true));
    }

    /// The only way for changes from the UI.
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

    /// Several ops as one undo step.
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

    /// Everything `f` changes in the project as ONE history step (e.g. new track + clip in it).
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
    /// Timeline as an area at the top or bottom; returns the height
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

    /// Several steps at once (negative = undo)
    pub fn history_step(&mut self, steps: isize) {
        self.commit_live();
        self.live = None;
        let n = self.doc.lock().unwrap_or_else(|e| e.into_inner()).step(steps);
        if n > 0 {
            let what = if steps < 0 { self.t("undo") } else { self.t("redo") };
            self.toast(format!("{what}: {n} {}", crate::tr(if n == 1 { "Schritt" } else { "Schritte" })));
        }
    }

    /// Show the history on the right in the inspector
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
        let _s = section("Projekt abgleichen");
        let doc = self.doc.lock().unwrap_or_else(|e| e.into_inner());
        if doc.revision() != self.revision {
            self.revision = doc.revision();
            self.project = doc.project().clone();
            self.project_epoch += 1;
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
            // request single frames anew; running streams stay – their key contains effects
            // and timing, changed clips switch to a new stream without stalling.
            self.core.forget_stills();
            let sig = audio_signature(&self.project);
            if sig != self.audio_sig {
                self.audio_sig = sig;
                if self.playing && self.voiceover.recorder.is_none() {
                    // audible changes audible shortly after (dragging a slider does not restart the sound at
                    // every step); the picture keeps running
                    self.core.edit(true, self.core_now());
                    if perf() && self.core.mode() == Mode::Spielen {
                        self.perf_play = Some(PerfPlay::new("Ton neu (Änderung)", self.playhead));
                    }
                }
            }
            self.request_media_previews();
        }
    }

    /// Request thumbnails and waveforms of the present media once each (after changes
    /// and when the file check reports something new). No file system access: presence from
    /// the cache, the loading already read the animations of the images.
    fn request_media_previews(&mut self) {
        let _s = section("Vorschaubilder anfordern");
        // thumbnails first: they are fast and should not wait behind the waveforms of long
        // files (the jobs run in order)
        for m in self.project.media.iter().filter(|m| self.presence.get(&m.path)) {
            if m.kind != MediaKind::Audio && !self.thumbs.contains_key(&m.id) && self.thumb_requested.insert((m.id, m.path.clone())) {
                let anim = if m.kind == MediaKind::Image { cl_media::animation(&m.path) } else { None };
                let at = thumb_time(m.kind, m.duration, anim.as_ref());
                self.decoder.send(Job::Thumb {
                    media: m.id,
                    path: m.path.clone(),
                    at,
                    size: (m.width, m.height),
                });
            }
        }
        for m in self.project.media.iter().filter(|m| self.presence.get(&m.path)) {
            if m.has_audio && !self.waves.contains_key(&(m.id, 0)) {
                self.waves.insert((m.id, 0), Vec::new()); // placeholder: send the job only once
                self.decoder.send(Job::Wave {
                    media: m.id,
                    path: m.path.clone(),
                    stream: 0,
                });
            }
        }
        // further audio tracks only if a clip plays them
        for c in self.project.timeline.tracks.iter().flat_map(|t| t.clips.iter()) {
            let (ClipContent::Media { media, .. }, Some(n)) = (&c.content, c.audio_stream) else { continue };
            let Some(m) = self.project.media(*media).filter(|m| n > 0 && n < m.audio_streams && self.presence.get(&m.path)) else { continue };
            if !self.waves.contains_key(&(m.id, n)) {
                self.waves.insert((m.id, n), Vec::new());
                self.decoder.send(Job::Wave { media: m.id, path: m.path.clone(), stream: n });
            }
        }
    }

    /// Request the preview anew (textures stay until the new frame).
    pub fn invalidate_preview(&mut self) {
        self.core.invalidate();
    }

    // ---------- Selection ----------

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

    /// Clip as it is currently displayed (incl. a running inspector edit).
    pub fn clip_view(&self, id: ClipId) -> Option<Clip> {
        match &self.live {
            Some(l) if l.id == id => Some(l.clone()),
            _ => self.project.find_clip(id).map(|(_, c)| c.clone()),
        }
    }

    /// Like `clip_view` for a clip already at hand: without search and without clone
    pub fn clip_ref<'a>(&'a self, c: &'a Clip) -> &'a Clip {
        match &self.live {
            Some(l) if l.id == c.id => l,
            _ => c,
        }
    }

    /// Write the running inspector edit into the history.
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
        // multi-selection: apply the same changes to all other selected clips
        let mut ops = crate::multi_edit::propagate(&self.project, &orig, &c, &self.selection());
        if ops.is_empty() {
            self.apply(EditOp::ReplaceClip { clip: c });
        } else {
            ops.insert(0, EditOp::ReplaceClip { clip: c });
            self.apply_batch(ops);
        }
    }

    /// Clip time (relative to the clip start) of the playhead, limited to the clip.
    pub fn rel_time(&self, c: &Clip) -> Time {
        Time((self.playhead - c.start).0.clamp(0, c.duration.0.max(0)))
    }

    /// Set a property: with keyframes → keyframe at the playhead, otherwise base value.
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

    /// Set/remove a keyframe at the playhead.
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
                // first keyframe: take over the base value so nothing jumps
                c.keyframes.set(*p, t, v);
            }
        }
        self.live = None;
        self.apply(EditOp::SetKeyframes {
            clip: id,
            keyframes: c.keyframes,
        });
    }

    /// Apply the transformation from the player (keyframes are respected).
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

    // ---------- Fonts ----------

    /// Registers UI fonts, icons and all families of the library with egui.
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
        // library + common system fonts for text templates
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

    /// Each frame: take over newly loaded fonts (only now egui knows them)
    pub(crate) fn apply_pending_fonts(&mut self, ctx: &egui::Context) {
        if let Some(f) = self.pending_fonts.take() {
            let known = ctx.fonts(|fo| fo.families());
            self.registered_fonts = f
                .into_iter()
                .filter(|n| known.contains(&egui::FontFamily::Name(n.clone().into())))
                .collect();
            // lay out the timeline's text clips anew (their cache hangs on the epoch)
            self.project_epoch += 1;
        }
    }

    pub fn font_family(&self, family: Option<&str>) -> egui::FontFamily {
        match family {
            Some(f) if self.registered_fonts.contains(f) => egui::FontFamily::Name(f.into()),
            _ => egui::FontFamily::Proportional,
        }
    }

    /// Font file for the export: library, then system fonts.
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

    // ---------- Media ----------

    pub fn import_media(&mut self, files: Vec<PathBuf>) -> Vec<MediaId> {
        // ffprobe for new files in advance in parallel (up to 8 threads), taken over below
        // in order as before
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
            // already imported? Then reuse
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
                    self.probe_failed(&name, &e.to_string());
                }
            }
        }
        log::info!(target: "import", "{} Medien übernommen", ids.len());
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

    /// Insert a medium at position `at` (video → main track, audio → audio track).
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
                font_family: self.brand.font(crate::brand::TEXT).cloned(),
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

    /// Put an image as a sticker clip (shrunk) on a free overlay track (one history step).
    pub fn add_image_overlay_at(
        &mut self,
        path: PathBuf,
        at: Time,
        track: Option<TrackId>,
    ) -> Option<ClipId> {
        self.one_step(|s| s.add_image_overlay_step(path, at, track, |_, _| {}))
    }

    /// `place` changes duration and position before the track choice (logo from the brand kit).
    pub(crate) fn add_image_overlay_step(
        &mut self,
        path: PathBuf,
        at: Time,
        track: Option<TrackId>,
        place: impl FnOnce(&Self, &mut Clip),
    ) -> Option<ClipId> {
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
        place(self, &mut clip);
        // stickers always lie above the video: find or create a free overlay track
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

    /// File of a built-in sound; the name contains a fingerprint of the source so
    /// improved sounds are generated anew
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

    /// Drop an asset (from the list, timeline or player).
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
            // dragged onto a clip: acts on that clip. Into free space: own element on an
            // effect/filter track that acts on everything below.
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

    /// Clip under the mouse pointer if it has a picture (video, image, sticker).
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

    /// Insert an effect or filter as its own element (3 s) at `at`; a track created for it
    /// belongs to the same history step.
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

    /// Set the transition between `clip` and its direct successor.
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

    /// Direct successor on the same track (for transitions).
    pub fn next_adjacent(&self, clip: ClipId) -> Option<ClipId> {
        let (tr, c) = self.project.find_clip(clip)?;
        tr.clips
            .iter()
            .find(|n| n.id != clip && (n.start.0 - c.end().0).abs() < 2000)
            .map(|n| n.id)
    }

    /// Transition at the nearest cut of the main track (or at the selected clip).
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

    // ---------- Edit ----------

    pub fn split_at_playhead(&mut self) {
        let at = self.playhead.snap_to_frame(self.project.settings.fps);
        let targets = self.split_targets();
        if targets.is_empty() {
            return;
        }
        // one history step; every split clip is replaced in the selection by its right piece (as for
        // a single clip), selected clips the playhead missed stay
        let rights: Vec<(ClipId, ClipId)> = self.one_step(|s| {
            targets
                .iter()
                .filter_map(|c| match s.apply(EditOp::SplitClip { clip: *c, at }) {
                    Some(OpOutput::Clip(r)) => Some((*c, r)),
                    _ => None,
                })
                .collect()
        });
        let piece = |c: ClipId| rights.iter().find(|r| r.0 == c).map_or(c, |r| r.1);
        let primary = self.selected.map(piece);
        let keep: Vec<ClipId> = self.selection().into_iter().filter(|c| !rights.iter().any(|r| r.0 == *c)).collect();
        let Some(first) = primary.or(rights.first().map(|r| r.1)) else { return };
        self.select_only(Some(first));
        self.sel_extra = rights.iter().map(|r| r.1).chain(keep).filter(|c| *c != first).collect();
    }

    /// Clips that „Teilen“ hits at the playhead: the selection, otherwise the main track's clip
    pub fn split_targets(&self) -> Vec<ClipId> {
        let at = self.playhead.snap_to_frame(self.project.settings.fps);
        // no splinters shorter than half a frame (they come about at clip boundaries
        // with odd frame rates and disturb the export)
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
        targets
    }

    /// Deletes the selection. The playhead jumps to the start of the (first) deleted clip.
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

    /// Q / W: cut away left or right of the playhead – on every selected clip the playhead lies in
    /// (one history step), without such a selection on the main track's clip.
    pub fn delete_side(&mut self, left: bool) {
        let at = self.playhead.snap_to_frame(self.project.settings.fps);
        let inside = |c: &Clip| at > c.start && at < c.end();
        let mut targets: Vec<(ClipId, bool)> = self
            .selection()
            .into_iter()
            .filter_map(|s| self.project.find_clip(s).filter(|(_, c)| inside(c)).map(|(t, _)| (s, t.main)))
            .collect();
        if targets.is_empty() {
            targets = self
                .project
                .timeline
                .tracks
                .iter()
                .find(|t| t.main)
                .and_then(|t| t.clips.iter().find(|c| inside(c)))
                .map(|c| vec![(c.id, true)])
                .unwrap_or_default();
        }
        let main = targets.iter().find(|t| t.1).map(|t| t.0);
        let edge = if left { Edge::Start } else { Edge::End };
        let ops = targets.iter().map(|(clip, _)| EditOp::TrimClip { clip: *clip, edge, to: at }).collect();
        if self.apply_batch(ops).is_some() && left && self.project.settings.main_track_magnet {
            // the main track closed the gap: the playhead goes along to the cut
            if let Some((_, c)) = main.and_then(|id| self.project.find_clip(id)) {
                self.playhead = c.start;
            }
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
            // target back to the origin track until a track is clicked again
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
        // origin track or last clicked track at the playhead; taken: push the following ones
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

    /// Create the audio stream a video clip plays as its own audio clip and silence that stream in the video.
    pub fn extract_audio(&mut self, id: ClipId) {
        let Some(c) = self.clip_view(id) else { return };
        let ClipContent::Media {
            media,
            kind: MediaKind::Video,
        } = c.content
        else {
            return;
        };
        let Some(m) = self.project.media(media).filter(|m| m.has_audio) else {
            self.toast_err("Dieser Clip hat keinen Ton");
            return;
        };
        // only the stream the clip plays; the video keeps its volume for the other streams
        let n = c.played_stream(m);
        if c.extracted_streams.contains(&n) {
            self.toast_err("Diese Tonspur ist schon extrahiert");
            return;
        }
        let mut a = c.clone();
        a.id = ClipId::new();
        a.extracted_streams.clear();
        a.content = ClipContent::Media {
            media,
            kind: MediaKind::Audio,
        };
        a.fx = ClipFx {
            reverse: c.fx.reverse,
            speed_curve: c.fx.speed_curve.clone(),
            ..Default::default()
        };
        // the volume curve belongs to the sound, the picture keyframes stay with the video
        a.keyframes.0.retain(|p, _| *p == Prop::Volume);
        a.anim = Animations::default();
        a.transition_out = None;
        let mut v = c.clone();
        v.extracted_streams.push(n);
        self.apply_batch(vec![
            EditOp::ReplaceClip { clip: v },
            EditOp::AddClip {
                track: None,
                clip: a,
            },
        ]);
        self.toast(format!("{} Ton extrahiert", icon::WAVEFORM));
    }

    /// Still frame: insert the current frame as a 3 s image at the playhead.
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
        let f = freeze_clip(&c, img, at);
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

    /// Quick change to all selected clips.
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

    // ---------- Projects ----------

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

    /// Save the project: snapshot to the save thread (serializes and writes
    /// atomically in the background). An error comes later via [`App::poll_saves`].
    /// `false`: refused, another window owns the file (see [`paths::may_write`]).
    pub fn write_project(&mut self, p: &Path) -> bool {
        if !paths::may_write(p, self.locked_out.as_deref(), std::process::id()) {
            return false;
        }
        self.saver.send(crate::saving::Job {
            kind: crate::saving::Kind::Project,
            path: p.to_path_buf(),
            project: self.project.clone(),
            revision: self.revision,
        });
        self.saved_revision = self.revision;
        self.last_saved = Some(Instant::now());
        // new project or „Speichern unter“: the lock moves to the new file
        if self.lock_file.as_deref() != Some(paths::lock_path(p).as_path()) {
            self.release_lock();
            self.take_lock(p);
        }
        self.settings.add_recent(p);
        self.send_project_thumb(p);
        true
    }

    /// Request the cover image for the start page (first frame of the picture tracks).
    pub fn send_project_thumb(&mut self, p: &Path) {
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
                    .filter(|m| self.presence.get(&m.path))
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
    }

    pub fn save(&mut self, save_as: bool) {
        let t0 = Instant::now();
        self.commit_live();
        // an explicit save always reports its own failure (the throttle is for autosave)
        self.saver.last_err = None;
        let path = match (&self.project_path, save_as) {
            (Some(p), false) => Some(p.clone()),
            (None, false) => Some(self.default_project_path()),
            _ => crate::dialog()
                .add_filter("Cutline-Projekt", &["cutline"])
                .set_file_name(format!("{}.cutline", self.project.name))
                .save_file(),
        };
        if let Some(p) = path {
            if !self.write_project(&p) {
                self.toast_err("Dieses Projekt ist schon in einem anderen Cutline-Fenster geöffnet");
                return;
            }
            crate::applog::project("gespeichert", &self.project, Some(t0));
            self.toast(format!("{} Gespeichert", icon::CHECK));
            self.project_path = Some(p);
            self.settings.save();
        }
    }

    /// Save automatically (1.5 s after the last change).
    fn autosave(&mut self) {
        let _s = section("Autosave");
        if !self.settings.autosave || self.screen != Screen::Editor {
            return;
        }
        if !needs_save(self.revision, self.saved_revision, self.project_path.is_some(), &self.project) {
            return;
        }
        if self.doc.lock().unwrap_or_else(|e| e.into_inner()).revision() != self.revision || self.saver.autosave_paused() {
            return;
        }
        if self.locked_out.is_some() && self.locked_out == self.project_path {
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
                    if self.write_project(&p) {
                        self.project_path = Some(p);
                        self.settings.save();
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

    pub(crate) fn reset_editor(&mut self) {
        self.stop_playback();
        // results of running drops and tasks belong to the left project
        self.drops.list.clear();
        self.tasks.next_generation();
        self.stock.stray = self.stock.busy;
        self.es.stray = self.es.loading_track.is_some();
        self.core.new_project();
        self.select_only(None);
        self.live = None;
        self.playhead = Time::ZERO;
        self.timeline = TimelineState::default();
        self.frames.clear();
        // free the old project's caches and discard its waiting jobs;
        // effect/filter/transition tiles apply to all projects and stay
        self.decoder.next_generation();
        self.thumbs.clear();
        self.thumb_requested.clear();
        self.proxies.reset();
        self.waves.clear();
        let project_key = |k: &String| k.starts_with("strip:") || k.starts_with("lib:");
        self.fx_thumbs.retain(|k, _| !project_key(k));
        self.fx_thumbs_requested.retain(|k| !project_key(k));
        self.sound_meta.clear();
        self.sound_meta_req.clear();
        self.invalidate_preview();
        self.player_zoom = 1.0;
        self.player_pan = egui::Vec2::ZERO;
        // close windows with times and hits of the old project
        self.shorts = None;
        self.thumb = None;
        self.aspect_fit = None;
        self.quick.open = None;
        self.quick.hits = None;
        // rebuild snapshot remembered by project state, which starts at 0 again per project
        self.export.framed = None;
    }

    /// On quit: cancel running background tasks so their FFmpeg processes end
    pub fn cancel_background_work(&mut self) {
        for t in &self.tasks.list {
            t.cancel();
        }
    }

    /// Start the stereo check of the media in advance (import, loaded project): otherwise the
    /// first playback sounds one-sided while the result is missing. One after another in a thread.
    fn warm_silent_channels(&mut self) {
        if self.silent_warmed.1 == self.revision {
            return;
        }
        self.silent_warmed.1 = self.revision;
        let new: Vec<PathBuf> = self
            .project
            .media
            .iter()
            .filter(|m| m.has_audio && m.kind != MediaKind::Image && self.presence.get(&m.path))
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

    /// Put a project into the editor. It counts as unsaved (a new `Document` starts
    /// at revision 0 again – otherwise a restore would be taken as saved).
    pub fn set_doc(&mut self, proj: Project) {
        *self.doc.lock().unwrap_or_else(|e| e.into_inner()) = Document::new(proj);
        self.tasks.next_generation();
        self.drops.list.clear();
        self.revision = u64::MAX;
        self.sync_project();
        self.saved_revision = UNSAVED;
    }

    /// `false`: the old project could not be saved, it stays open.
    pub fn new_project(&mut self) -> bool {
        if !self.flush_before_leaving() {
            return false;
        }
        self.cancel_load();
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

    /// Change the project folder and take existing projects along. Errors stay in
    /// the settings window.
    pub fn change_projects_dir(&mut self, new: PathBuf) {
        let old = paths::projects_dir();
        if new == old {
            return;
        }
        // save the open project first and release files (running playback keeps them open)
        self.stop_playback();
        if !self.flush_before_leaving() {
            return;
        }
        let mut reopen = None;
        let (moved, errs) = paths::move_projects(&old, &new);
        // what moved is recorded – even if something else failed
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
        // switch the folder only if nothing stayed behind
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
        // load the open project from the new place (its media paths were rewritten)
        if let Some(p) = reopen {
            self.open_project(&p);
        }
    }

    /// Open a project: reading, checking and preparing in the background (see `loading.rs`); the
    /// open project stays until the new one is ready (cancelling changes nothing).
    pub fn open_project(&mut self, p: &Path) {
        // the same project once more: write the open state first – loading reads the
        // file at once and would otherwise take the old state as saved
        if self.project_path.as_deref() == Some(p) && self.screen == Screen::Editor {
            if !self.flush_before_leaving() {
                return;
            }
            // the project stays open until loading is done (or cancelled)
            self.take_lock(p);
        }
        let ctx = self.ui_ctx.clone();
        self.start_load(p, crate::loading::LoadMode::Open, &ctx);
    }

    /// Before leaving a project: save open changes. `false` if that did not
    /// work – then the project stays open, otherwise the changes would be lost.
    pub(crate) fn flush_before_leaving(&mut self) -> bool {
        self.commit_live();
        self.finish_file_drops(Duration::from_secs(10));
        if self.voiceover.recorder.is_some() {
            self.stop_voiceover();
        }
        if needs_save(self.revision, self.saved_revision, self.project_path.is_some(), &self.project) {
            let p = self
                .project_path
                .clone()
                .unwrap_or_else(|| self.default_project_path());
            if self.write_project(&p) {
                self.project_path = Some(p);
            } else {
                // another window owns the file: this window's edits go to the backups instead
                let job = crate::saving::Job { kind: crate::saving::Kind::Project, path: p, project: self.project.clone(), revision: self.revision };
                if let Some(r) = crate::saving::rescue(&job) {
                    self.toast_err(format!("Stand gesichert in {}", r.display()));
                    self.saved_revision = self.revision;
                }
            }
        }
        // wait for the save thread: only once the state is on disk may the project close
        // (lock free, editor empty, file moved/renamed)
        let idle = self.saver.wait_idle(Duration::from_secs(30));
        self.saver.last_err = None;
        self.poll_saves();
        if !idle {
            self.toast_err(format!("Projekt konnte nicht gespeichert werden: {}", crate::tr("Speichern dauert zu lange")));
        }
        if !idle || needs_save(self.revision, self.saved_revision, self.project_path.is_some(), &self.project) {
            return false;
        }
        self.settings.save();
        self.release_lock();
        true
    }

    /// Lock file `<project>.lock` (content: PID): warns when a second window opens the same
    /// project – both would otherwise silently overwrite each other.
    pub(crate) fn take_lock(&mut self, p: &Path) {
        let lock = paths::lock_path(p);
        let me = std::process::id();
        if paths::lock_held_by_other(&lock, me) {
            // the other window keeps its lock; this one does not write the file (see `write_project`)
            self.toast_err("Dieses Projekt ist schon in einem anderen Cutline-Fenster geöffnet");
            self.locked_out = Some(p.to_path_buf());
            return;
        }
        self.locked_out = None;
        let _ = std::fs::write(&lock, me.to_string());
        self.lock_file = Some(lock);
    }

    fn release_lock(&mut self) {
        if let Some(l) = self.lock_file.take() {
            // remove only our own lock
            if std::fs::read_to_string(&l).is_ok_and(|s| s.trim() == std::process::id().to_string()) {
                let _ = std::fs::remove_file(l);
            }
        }
    }

    /// Window close (X, Alt+F4, close request of the system) with „Beim Schließen zur
    /// Projektübersicht“: back to the start page instead of quitting. Runs after the export's
    /// own question. If leaving fails, the close goes through as before (`on_exit` rescues the
    /// state), so a close request is never blocked for good.
    fn close_to_home(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        let explicit = self.export.owns_close() || ctx.data(|d| d.get_temp::<bool>(egui::Id::new(EXPLICIT_QUIT))).unwrap_or(false);
        if !close_goes_home(self.settings.close_to_home, self.screen == Screen::Editor, explicit) {
            return;
        }
        self.go_home();
        if self.screen == Screen::Home {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }

    pub fn go_home(&mut self) {
        if !self.flush_before_leaving() {
            return;
        }
        self.cancel_load();
        crate::applog::project("geschlossen", &self.project, None);
        // every way back into the editor resets it anyway: free the project's frames, textures and
        // decoder caches right now
        self.reset_editor();
        self.home.refresh();
        self.screen = Screen::Home;
    }

    // ---------- Playback ----------

    pub fn last_frame(&self) -> Time {
        Time(
            (self.project.duration()
                - Time::from_secs_f64(1.0 / self.project.settings.fps.as_f64()))
            .0
            .max(0),
        )
    }

    /// Time for the player core (seconds since the start)
    pub fn core_now(&self) -> f64 {
        self.core_t0.elapsed().as_secs_f64()
    }

    /// Carry out the core's audio commands and mirror its state for the UI.
    pub fn run_core(&mut self) {
        // start of a playback before the audio start (which blocks a little)
        let t0 = self.play_log.is_none().then(Instant::now);
        loop {
            let cmds = self.core.cmds();
            if cmds.is_empty() {
                break;
            }
            for c in cmds {
                match c {
                    Cmd::StartAudio { at, anchor } => self.start_audio(at, anchor),
                    Cmd::AnchorAudio => {
                        if let Some(a) = &self.audio {
                            a.anchor(Instant::now());
                        }
                    }
                    Cmd::StopAudio => self.audio = None,
                    Cmd::PauseAudio => {
                        if !self.audio.as_ref().is_some_and(|a| a.pause()) {
                            self.core.stop();
                        }
                    }
                    Cmd::ResumeAudio => {
                        if !self.audio.as_ref().is_some_and(|a| a.resume()) {
                            // sound could not be resumed: buffer anew at the same place
                            let at = self.core.pos();
                            self.core.stop();
                            self.core.play(at, self.revision, self.core_now());
                        }
                    }
                }
            }
        }
        let playing = self.core.playing();
        // multicam: take over cuts as soon as playback ends (stop, end, pause)
        if self.playing && !playing && self.multicam.as_ref().is_some_and(|m| !m.1.is_empty()) {
            self.playing = false;
            self.multicam_finish();
        }
        self.playing = playing;
        self.play_log_step(playing, t0);
    }

    /// One line per playback in the log: begins with playing, ends with stop, pause,
    /// end or jump. Only a few assignments per UI frame.
    fn play_log_step(&mut self, playing: bool, t0: Option<Instant>) {
        if !playing {
            self.end_play_log();
            return;
        }
        if let Some(l) = &mut self.play_log {
            l.quality = l.quality.max(self.core.quality.level);
            return;
        }
        if !perf() {
            // without `CUTLINE_PERF` nobody reads the counters otherwise: count from here
            // shortcut: with `CUTLINE_PERF` it counts up to 1 s before the start too; enough for the line
            self.core.take_stats();
        }
        self.play_log = Some(PlayLog::new(self.playhead, t0.unwrap_or_else(Instant::now)));
    }

    fn end_play_log(&mut self) {
        if let Some(mut l) = self.play_log.take() {
            l.stats.add(&self.core.stats());
            if let Some(s) = l.line(l.pressed.elapsed(), self.settings.use_proxies) {
                log::info!(target: "wiedergabe", "{s}");
            }
        }
    }

    pub fn toggle_play(&mut self) {
        let now = self.core_now();
        if self.core.playing() {
            // pause with open streams and sound (continue at the same place without new buffering);
            // without running sound, with J/K/L or multicam: stop
            let running = self.audio.as_ref().is_some_and(|a| a.started());
            let hold = !self.multicam.as_ref().is_some_and(|m| !m.1.is_empty());
            self.core.pause(self.revision, running, hold);
        } else if self.core.can_resume(self.playhead, self.revision) {
            self.core.play(self.playhead, self.revision, now);
        } else {
            self.commit_live();
            // with an in/out range: start in the range
            if let (Some(a), Some(b)) = (self.range_in, self.range_out) {
                if self.loop_range && (self.playhead < a || self.playhead >= b) {
                    self.playhead = a;
                }
            }
            if self.playhead >= self.last_frame() {
                self.playhead = Time::ZERO;
            }
            // buffer first: the playhead starts when the frame streams are ready. The sound is
            // prepared at the same time and comes in as soon as it is there – the picture does not wait for it.
            self.core.play(self.playhead, self.revision, now);
            if perf() {
                self.perf_play = Some(PerfPlay::new("Play", self.playhead));
            }
        }
        self.run_core();
    }

    /// J/K/L: play at speed `rate` (single frames, without sound)
    pub fn start_shuttle(&mut self, rate: f64) {
        self.core.shuttle(rate, self.playhead, self.core_now());
        self.run_core();
    }

    pub fn stop_playback(&mut self) {
        self.core.stop();
        self.run_core();
    }

    /// Restart the sound from the playhead (solo, output device). The clock keeps running; the sound comes in
    /// without a jump at the then current place (while buffering only when that ends).
    pub fn restart_audio(&mut self) {
        self.core.restart_audio(self.core_now());
        self.run_core();
    }

    /// Prepare sound from `at`; `anchor`: come in at the running clock at once
    fn start_audio(&mut self, at: Time, anchor: bool) {
        self.audio = None;
        if self.voiceover.recorder.is_some() && self.voiceover.mute_project {
            return;
        }
        // solo: all other audio tracks muted (only in the preview)
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
        match AudioPlayer::start_synced(proj, at) {
            Ok(a) => {
                if anchor {
                    a.anchor(Instant::now());
                }
                self.audio = Some(a);
            }
            Err(e) => log::warn!("Ton: {e}"),
        }
    }

    /// Sound while dragging the playhead: every 250 ms a short snippet from the current place.
    fn scrub_audio(&mut self, ctx: &egui::Context) {
        let moved = self.scrub.as_ref().is_none_or(|(t, _, _)| *t != self.playhead);
        let dragging = ctx.input(|i| i.pointer.primary_down());
        if self.settings.scrub_audio && !self.playing && dragging && moved {
            let due = self.scrub.as_ref().is_none_or(|(_, at, _)| at.elapsed() > Duration::from_millis(250));
            // the first call after the start only remembers the place
            if due && self.scrub.is_some() {
                use std::sync::atomic::{AtomicBool, Ordering};
                // starting (FFmpeg, audio device) and stopping block: both in their own thread.
                // The cpal stream is not Send, so the snippet lives entirely there; the UI
                // only holds the stop signal and ends the previous snippet with it.
                if let Some((_, _, Some(old))) = &self.scrub {
                    old.store(true, Ordering::Relaxed);
                }
                let stop = Arc::new(AtomicBool::new(false));
                let (project, at, s) = (self.project.clone(), self.playhead, stop.clone());
                std::thread::spawn(move || {
                    let a = AudioPlayer::start(&project, at).ok();
                    if perf() {
                        crate::applog::perf_line!("scrub: Ton ab {:.2} s, ok={}", at.as_secs_f64(), a.is_some());
                    }
                    // end the snippet after 350 ms (or earlier when the next begins)
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

    /// Sound as seen by the player core
    fn audio_in(&self) -> AudioIn {
        match &self.audio {
            None => AudioIn::None,
            Some(a) if a.failed() => AudioIn::Failed,
            Some(a) if a.started() && a.finished() => AudioIn::Finished,
            Some(a) if a.started() => AudioIn::Running(a.position()),
            Some(a) if !a.ready() => AudioIn::Loading,
            Some(_) => AudioIn::Pending,
        }
    }

    /// Levels of the automatic quality (divisor, half frame rate); the first is the chosen one
    pub fn quality_steps(&self) -> Vec<(u32, bool)> {
        crate::playcore::quality_steps(self.settings.preview_div.max(1), self.project.settings.fps.as_f64())
    }

    /// Tick of the player core: advance the playhead (audio clock, otherwise system time), end
    /// buffering, end/loop, discard the pause when playhead or project changed.
    fn advance_playhead(&mut self) {
        // preview sound: unreadable files drop out one by one, hint once per file
        for w in cl_export::take_audio_warnings() {
            self.toast_err(w);
        }
        self.perf_audio();
        let max_q = if self.settings.auto_quality { self.quality_steps().len() - 1 } else { 0 };
        let t = Tick {
            now: self.core_now(),
            playhead: self.playhead,
            revision: self.revision,
            audio: self.audio_in(),
            end: self.project.duration(),
            fps: self.project.settings.fps.as_f64(),
            loop_range: if self.loop_range { self.range_in.zip(self.range_out) } else { None },
            recording: self.voiceover.recorder.is_some(),
            max_quality_level: max_q,
        };
        // a jump during playback ends the playback for the log (the next begins there)
        if self.core.playing() && self.playhead != self.core.pos() {
            self.end_play_log();
        }
        self.playhead = self.core.tick(&t);
        self.run_core();
    }

    /// Request the data of a library sound (once per key, in the background).
    pub fn request_sound_meta(&mut self, key: &str, path: &Path, sfx: Option<&'static str>) {
        if self.sound_meta_req.insert(key.to_string()) {
            self.decoder.send(crate::preview::Job::SoundMeta {
                key: key.to_string(),
                path: path.to_path_buf(),
                sfx,
            });
        }
    }

    /// Toggle the preview. Without finished data it is requested and started afterwards –
    /// the UI does not wait for FFmpeg.
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

    /// Preview an already checked medium (also network addresses, e.g. HLS).
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

    // ---------- Voice-over ----------

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
                    // take over the live waveform until the exact one is computed
                    self.waves.entry((*id, 0)).or_insert(peaks);
                    if let Some(c) = self.add_media_at(*id, start, None) {
                        // take over the settings from the voice-over window
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

    // ---------- Keyboard ----------

    /// Esc closes the topmost open window (properties, exports, backups, relink).
    /// Runs before the shortcuts, otherwise „escape“ uses up the key there and only clears the selection.
    /// A dialog that evaluates Esc/Enter itself is open
    fn dialog_open(&self) -> bool {
        self.quick.open.is_some()
            || self.shorts.is_some()
            || self.thumb.is_some()
            || self.aspect_fit.is_some()
            || self.tpl_dialog.is_some()
            || self.paste_attrs_open
            || self.marker_edit.is_some()
            || self.track_rename.is_some()
            || self.lib_dialog.is_some()
            || self.preset_dialog.is_some()
            || self.updater.view.is_some()
            || self.palette.open
    }

    fn esc_closes(&mut self, ctx: &egui::Context) -> bool {
        // windows on top first: otherwise Esc would close a hidden window behind
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
        // open context menu: Esc belongs to the menu (otherwise the shortcut uses up the key)
        if self.settings_open || self.dialog_open() || ctx.is_context_menu_open() {
            return;
        }
        let held = std::mem::take(&mut self.held_keys);
        let space = !held.is_empty() || ctx.input(|i| i.key_pressed(Key::Space));
        ctx.input_mut(|i| i.events.extend(held));
        if std::mem::take(&mut self.esc_field) {
            // the field has discarded the input and released the focus – the selection stays
            ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
            crate::widgets::mark_field_esc(ctx);
        }
        match key_focus(typing(ctx), ctx.wants_keyboard_input(), space) {
            KeyFocus::Keep => return,
            KeyFocus::Release => release_text_focus(ctx),
            KeyFocus::Shortcuts => {}
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
        // copy/cut/paste come as events
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
            // if the system delivers no copy/paste event (e.g. empty
            // clipboard), evaluate the keys directly
            r.0 |= i.consume_key(Modifiers::COMMAND, Key::C);
            r.1 |= i.consume_key(Modifiers::COMMAND, Key::X);
            r.2 |= i.consume_key(Modifiers::COMMAND, Key::V);
            // Ctrl+Alt+V (Windows: AltGr) is „Attribute einfügen“, not a normal paste
            if r.2 && (i.modifiers.alt || actions.contains(&"paste_attrs")) {
                r.2 = false;
                if !actions.contains(&"paste_attrs") {
                    actions.push("paste_attrs");
                }
            }
            // if the key itself came (binding „copy“/„cut“/„paste“), do not run it twice
            r.0 &= !actions.contains(&"copy");
            r.1 &= !actions.contains(&"cut");
            r.2 &= !actions.contains(&"paste");
            r
        });
        if copy {
            self.copy_selection();
        }
        if std::mem::take(&mut self.copy_marker) {
            // text in the clipboard makes Ctrl+V trigger a paste event
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

    /// First binding of an action as text (for menu rows), empty without a binding.
    pub fn key_of(&self, action: &str) -> String {
        crate::keymap::bindings(&self.settings.shortcuts)
            .into_iter()
            .find(|(_, a)| *a == action)
            .map(|(c, _)| c.label())
            .unwrap_or_default()
    }

    /// Run an action from `keymap::ACTIONS` – shortcuts and menus share this.
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
                "playback_mute" => self.set_volume(crate::audio::Volume { muted: !self.volume.muted, ..self.volume }, true),
                "playback_louder" => self.step_volume(true),
                "playback_quieter" => self.step_volume(false),
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
                "command_search" => self.palette.show(),
                "copy" => self.copy_selection(),
                "cut" => self.cut_selection(),
                "paste" => self.paste(),
                // menu at the top left
                "new_project" => {
                    self.new_project();
                }
                "import_project" => self.import.open = true,
                "import_fonts" => self.pick_fonts(ctx),
                "exports" => self.exports_for = Some(crate::exports::ExportsList::new(&self.project.name, self.project_path.as_deref())),
                "history" => self.show_history(),
                "layout_edit" => {
                    if self.settings.layout_locked {
                        self.toast("Layout ist gesperrt (Einstellungen → Allgemein → Anordnung)");
                    } else {
                        self.player_max = false;
                        self.layout_edit = true;
                    }
                }
                "template_save" => self.tpl_dialog = Some(crate::templates::TplDialog::Save { name: self.project.name.clone() }),
                "template_insert" => self.tpl_dialog = Some(crate::templates::TplDialog::Insert(crate::templates::load_all())),
                "archive" => self.start_archive(ctx),
                "relink" => self.missing_open = true,
                "backups" => self.backups_open = true,
                "go_home" => self.go_home(),
                "tour" => self.start_tour(),
                "onboarding" => self.start_onboarding(),
                // quick actions
                "quick_pauses" => self.quick_open(crate::quick::QuickKind::Pauses),
                "quick_fillers" => self.quick_open(crate::quick::QuickKind::Fillers),
                "quick_ducking" => self.quick_open(crate::quick::QuickKind::Ducking),
                "quick_highlights" => self.tool_highlights(ctx),
                "quick_sync" => self.tool_sync(ctx),
                "quick_multicam" => self.tool_multicam_start(),
                "quick_speakers" => self.tool_speakers(ctx),
                "quick_punch" => self.quick_open(crate::quick::QuickKind::Punch),
                "quick_vertical" => self.tool_vertical(),
                "quick_thumbnail" => self.thumb_open(),
                "quick_recap" => self.tool_recap(),
                "censor" => {
                    if let Some(id) = self.selected.filter(|_| self.action_enabled("censor")) {
                        self.clip_tab = ClipTab::Smart;
                        self.pick = Some(crate::pro::Pick::Censor(id));
                    }
                }
                "brand_logo" => {
                    self.insert_logo();
                }
                l if l.starts_with("quick_layout_") => self.tool_layout(l),
                _ => {}
            }
        }
    }

    /// Is the action possible right now? Only the checks that menus and toolbar do
    /// anyway; everything else counts as possible.
    pub fn action_enabled(&self, a: &str) -> bool {
        match a {
            "undo" | "redo" => {
                let d = self.doc.lock().unwrap_or_else(|e| e.into_inner());
                if a == "undo" { d.can_undo() } else { d.can_redo() }
            }
            "split" => !self.split_targets().is_empty(),
            "del_left" | "del_right" => !self.project.timeline.tracks.is_empty(),
            "delete" => self.selected.is_some() || !self.sel_extra.is_empty() || self.selected_transition.is_some(),
            "paste" | "paste_attrs" => !self.clipboard.is_empty(),
            "group" => self.selection().len() >= 2,
            "ungroup" => self.selected.and_then(|s| self.project.find_clip(s)).is_some_and(|(_, c)| c.group.is_some()),
            "import_project" => self.import_enabled(),
            "censor" => self.selected.and_then(|s| self.project.find_clip(s)).is_some_and(|(_, c)| {
                matches!(c.content, ClipContent::Media { kind: MediaKind::Video | MediaKind::Image, .. })
            }),
            "brand_logo" => self.brand.logo.is_some(),
            q if q.starts_with("quick_") => self.quick_check(q).0,
            _ => true,
        }
    }

    fn draw_toasts(&mut self, ctx: &egui::Context) {
        crate::applog::crash_notice(ctx, self);
        // once: the graphics card could encode in hardware, only the driver is too old
        if !self.settings.driver_hint_shown && cl_export::old_driver_found() {
            self.settings.driver_hint_shown = true;
            self.settings.save();
            self.toast_err(crate::tr("Grafiktreiber veraltet: Hardware-Kodierung geht erst nach einem Treiber-Update, bis dahin wird in Software kodiert"));
        }
        if let Some(v) = crate::applog::take_old_ffmpeg() {
            self.toast_err(format!("FFmpeg {v} gefunden – Cutline braucht {} oder neuer", crate::applog::MIN_FFMPEG));
        }
        self.toasts
            .retain(|(_, t, err)| t.elapsed().as_secs_f32() < toast_secs(*err));
        if self.toasts.is_empty() {
            return;
        }
        // glide in from below and fade in, fade out at the end; without animations at once
        let (fade_in, fade_out) = (theme::anim(theme::MOTION), theme::anim(theme::MOTION_SLOW));
        let newest = self.toasts.last().map_or(1.0, |t| t.1.elapsed().as_secs_f32());
        let rise = if fade_in > 0.0 { 1.0 - theme::ease(newest / fade_in) } else { 0.0 };
        ctx.set_transform_layer(
            egui::LayerId::new(egui::Order::Foreground, egui::Id::new("toasts")),
            egui::emath::TSTransform::from_translation(vec2(0.0, 12.0 * rise)),
        );
        let mut close = None;
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -24.0))
            .order(egui::Order::Foreground)
            // only error toasts close on click; otherwise the area stays click-through
            .interactable(self.toasts.iter().any(|t| t.2))
            .show(ctx, |ui| {
                // do not wrap messages word by word (the area has no fixed width)
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                for (i, (msg, at, err)) in self.toasts.iter().enumerate().rev().take(3) {
                    let age = at.elapsed().as_secs_f32();
                    ui.set_opacity(theme::ease(toast_alpha(age, toast_secs(*err), fade_in, fade_out)));
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
                            // overlong message (long template name): shorten with „…“ within the window
                            ui.add(egui::Label::new(RichText::new(toast_line(msg, *err)).color(
                                if *err {
                                    theme::DANGER_TEXT()
                                } else {
                                    theme::TEXT()
                                },
                            )).truncate());
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
        // dropped over the sticker or sound list: into the own library
        if let (Some((kind, rect)), Some(pos)) =
            (self.lib_drop, ctx.input(|i| i.pointer.hover_pos()).or(self.drops.pos))
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
        // media files in the background, dropped over the timeline also right there
        let (files, other): (Vec<_>, Vec<_>) = media.into_iter().partition(|p| crate::file_drop::is_media(p));
        let target = if self.screen == Screen::Editor { self.drop_target() } else { None };
        if !files.is_empty() {
            self.start_file_drop(ctx, files, target);
        }
        if !other.is_empty() {
            let n = self.import_media(other).len();
            if n > 0 {
                self.toast(self.i18n.t_num("media-imported", "count", n as i64));
            }
        }
    }
}

impl App {
    /// Frame pacing of the UI during playback: about two UI frames per video frame.
    /// Waiting time for `request_repaint_after` until the next due frame (project frame rate);
    /// while buffering check shortly. egui subtracts the expected frame time from every waiting time
    /// (`predicted_dt`, fixed 1/60 s), so it is added here. The deadline is only exact
    /// with the fine timer ([`HiresTimer`]); otherwise Windows snaps to ~15.6 ms.
    pub fn play_repaint_delay(&self) -> Duration {
        let wait = if self.core.mode() == Mode::Puffern {
            Duration::from_millis(10)
        } else {
            next_frame_delay(self.playhead, self.project.settings.fps, self.core.rate())
        };
        wait + EGUI_PREDICTED_DT
    }

    /// Playback measurement points: Play → first sound (also for the normal log), audio dropouts (`CUTLINE_PERF`)
    fn perf_audio(&mut self) {
        if let (Some(l), Some(t)) = (&mut self.play_log, self.audio.as_ref().and_then(|a| a.first_sound())) {
            // after a pause the sound from before does not count
            if l.sound.is_none() && t >= l.pressed {
                l.sound = Some(t - l.pressed);
            }
        }
        if !perf() {
            return;
        }
        let Some(a) = &self.audio else { return };
        if let (Some(p), Some(t)) = (&mut self.perf_play, a.first_sound()) {
            if !p.sound {
                p.sound = true;
                crate::applog::perf_line!(
                    "perf: {} → erster Ton {:.0} ms",
                    p.what,
                    t.saturating_duration_since(p.pressed).as_secs_f64() * 1000.0
                );
            }
        }
        let (n, d) = a.take_underruns();
        if n > 0 {
            crate::applog::perf_line!("perf: Ton-Aussetzer {n}× zusammen {:.0} ms bei {:.2} s", d.as_secs_f64() * 1000.0, self.playhead.as_secs_f64());
        }
    }

    /// Measurement point: a stream frame of the main layer was shown (Play → first advanced frame)
    pub fn perf_frame_shown(&mut self) {
        if let Some(l) = self.play_log.as_mut().filter(|l| l.frame.is_none() && self.playhead > l.at) {
            l.frame = Some(l.pressed.elapsed());
        }
        if let Some(p) = self.perf_play.as_mut().filter(|p| !p.frame && self.playhead > p.at) {
            p.frame = true;
            crate::applog::perf_line!("perf: {} → erstes Bild weitergeschaltet {:.0} ms", p.what, p.pressed.elapsed().as_secs_f64() * 1000.0);
        }
    }
}

impl eframe::App for App {
    /// Window background in the theme colour (visible in the gaps between the cards)
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    /// Click tests without a real mouse pointer (only with `CUTLINE_SHOTS`), see `devinput.rs`
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        crate::devinput::feed(ctx, raw);
        self.held_keys = hold_space(ctx, raw);
        let esc = raw.events.iter().any(|e| matches!(e, egui::Event::Key { key: Key::Escape, pressed: true, .. }));
        self.esc_field = esc_for_field(typing(ctx), esc);
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        dev_screenshot(ctx);
        // bind the UI size to the window size: dialogs always fit into the window
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
                let st = self.core.take_stats();
                if let Some(l) = &mut self.play_log {
                    l.stats.add(&st);
                }
                let ord = std::sync::atomic::Ordering::Relaxed;
                crate::applog::perf_line!(
                    "perf: {:.0} ui/s, {:.0} Strombilder/s, {} Ströme geöffnet, größte Lücke {:.0} ms, playing={}, Bildzeit Ø {:.2} / max {:.1} ms, Player Ø {:.2} / max {:.1} ms",
                    self.perf_ui as f64 / el,
                    st.frames as f64 / el,
                    st.opens,
                    st.max_gap * 1000.0,
                    self.playing,
                    PERF_SUM_US.swap(0, ord) as f64 / 1000.0 / self.perf_ui.max(1) as f64,
                    PERF_MAX_US.swap(0, ord) as f64 / 1000.0,
                    crate::player::PERF_PLAYER_SUM_US.swap(0, ord) as f64 / 1000.0 / self.perf_ui.max(1) as f64,
                    crate::player::PERF_PLAYER_MAX_US.swap(0, ord) as f64 / 1000.0
                );
                if self.core.mode() == Mode::Spielen {
                    if let Some(ms) = self.core.av_offset_ms() {
                        let clock = if self.audio.as_ref().is_some_and(|a| a.started()) { "Ton-Uhr" } else { "Systemuhr" };
                        crate::applog::perf_line!("perf: Bild-Ton-Versatz {ms:.0} ms ({clock})");
                    }
                    crate::applog::perf_line!(
                        "perf: verworfen {:.0}/s, Rückstand max {:.0} ms, aufgeholt {}×, hängend {}×, Qualitätsstufe {}",
                        st.dropped as f64 / el,
                        st.max_lag * 1000.0,
                        st.catch_ups,
                        st.stalls,
                        self.core.quality.level
                    );
                }
                self.perf_ui = 0;
                self.perf_since = Instant::now();
            }
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("CUTLINE_DEBUG_HITS").is_some() {
            ctx.style_mut(|s| s.debug.show_widget_hits = true);
        }
        self.apply_pending_fonts(ctx);
        // „System“ follows a change of the operating system at once
        theme::select(ctx, &self.settings.theme);
        // preview finished → button back to „Abspielen“
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
        if self.presence.tick(&self.project, ctx) {
            self.request_media_previews();
        }
        self.poll_saves();
        self.poll_load(ctx);
        self.drops_frame(ctx, frame);
        self.handle_dropped_files(ctx);
        self.warm_silent_channels();
        {
            let _s = section("Vorschaubilder empfangen");
            self.receive_decoded(ctx);
        }
        {
            let _s = section("Aufgaben");
            self.poll_tasks();
            self.poll_file_drops();
        }
        self.poll_epidemic(ctx);
        self.poll_stock();
        // exports keep running on the start page too, the next waiting one starts by itself
        if let Some((msg, done)) = self.export.poll(ctx) {
            // a successful export with Epidemic music is reported (terms of use) – for the
            // project it comes from
            if let Some(p) = done.filter(|_| msg.contains("Export fertig")) {
                self.report_epidemic_export(ctx, &p);
                // only if nothing is being previewed: `preview_sound` toggles and would abort it
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
        if self.screen == Screen::Editor {
            self.ensure_proxies(ctx);
        }

        self.window_frame(ctx);
        // window close button during an export: ask first
        self.export.guard_close(ctx);
        self.close_to_home(ctx);
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
            self.update_dialog(ctx);
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
            // Esc in the reorder edit mode only ends that
            && (self.settings_open || self.dialog_open() || !crate::library_ui::esc_order_edit(ctx))
        {
            self.shortcuts(ctx);
        }
        self.command_palette(ctx);
        // countdown before the voice-over recording
        if let Some(until) = self.voiceover.countdown_until {
            if Instant::now() >= until {
                self.start_voiceover();
            }
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.advance_playhead();
        if self.playing != self.hires_timer.is_some() {
            self.hires_timer = self.playing.then(HiresTimer::new);
        }
        if self.playing {
            ctx.request_repaint_after(self.play_repaint_delay());
        }
        self.scrub_audio(ctx);

        if !self.player_max {
            // layout relative to the window size: the player always keeps enough room
            let sr = ctx.screen_rect();
            // areas as a share of the window size: if the window gets smaller and larger again,
            // media, inspector and timeline grow along again (instead of staying squeezed).
            let resized = self
                .layout_screen
                .is_some_and(|s| (s.width() - sr.width()).abs() > 1.0 || (s.height() - sr.height()).abs() > 1.0);
            let (fa, fi, ft) = self.layout_frac;
            let lay = self.settings.layout.clone();
            // own ids per side so remembered sizes do not jump between the sides
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
            // timeline across the full width: create before the side areas
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
            // remember widths dragged by the user as a share (only while dragging and if the window
            // stayed the same: otherwise minimum widths after zoom/resize would overwrite the shares)
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
        self.update_dialog(ctx);
        // only the open dialog starts exports and needs the fonts
        let fonts_owned = if self.export.is_open() { self.font_files() } else { HashMap::new() };
        // cover image: the main track's clip at the playhead (otherwise any visible frame)
        let main = self
            .project
            .timeline
            .tracks
            .iter()
            .find(|t| t.main)
            .and_then(|t| t.clips.iter().find(|c| c.contains(self.playhead)))
            .and_then(|c| self.frames.get(&c.id).map(|f| (c.id, f)));
        // only frames of clips that still exist (otherwise an empty project shows an old frame)
        let tex = main
            .or_else(|| self.project.timeline.tracks.iter().flat_map(|t| &t.clips).find_map(|c| self.frames.get(&c.id).map(|f| (c.id, f))))
            .map(|(id, (_, t))| (id, t.id(), t.size_vec2(), self.playhead));
        if std::mem::take(&mut self.export.chapter_hint_off) {
            self.settings.chapter_hint = false;
            self.settings.save();
        }
        self.export.chapters = self.settings.export_chapters;
        self.export.chapter_hint = self.settings.chapter_hint;
        if let Some(msg) = self.export.show(ctx, &self.project, &fonts_owned, tex, self.revision) {
            self.toast(msg);
        }
        if let Some(id) = self.export.take_picked_platform() {
            if let Some(s) = &mut self.aspect_fit {
                // format change waits for „Clips neu einpassen?“: the platform comes along (Esc discards both)
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
        // cancel the running export cleanly: no orphaned FFmpeg, no .part file
        self.export.cancel_and_wait(Duration::from_secs(5));
        self.cancel_background_work();
        // a running import cleans away its half project, ffprobe checks end
        self.cancel_import();
        self.brand_flush();
        let flushed = self.flush_before_leaving();
        // let everything waiting be written (save thread) before the program ends
        if !self.saver.wait_idle(Duration::from_secs(60)) {
            eprintln!("Speichern beim Beenden dauert zu lange");
        }
        self.poll_saves();
        if !flushed || self.saved_revision == UNSAVED {
            // last resort: write the state into the backups
            let dir = match &self.project_path {
                Some(p) => crate::project_tools::backup_dir(p),
                None => paths::data_dir().join("Backups").join(paths::safe_name(&self.project.name)),
            };
            let _ = std::fs::create_dir_all(&dir);
            let p = dir.join(format!("exit-{}.cutline", paths::unix_secs()));
            if let Err(e) = paths::write_atomic(&p, self.project.to_json()) {
                log::error!("{}: {e}", p.display());
            }
            self.release_lock();
        }
        crate::applog::session_end();
    }
}

/// egui temp flag: the next close request really quits (restart after an update)
pub(crate) const EXPLICIT_QUIT: &str = "explicit-quit";

/// Does a window close request lead to the start page instead of quitting?
fn close_goes_home(setting: bool, project_open: bool, explicit_quit: bool) -> bool {
    setting && project_open && !explicit_quit
}

/// Fingerprint of all audible properties (tracks, audio clips, volume, speed, audio effects).
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
            (c.speed.to_bits(), c.volume_db.to_bits(), c.audio_stream, &c.extracted_streams).hash(&mut h);
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
    /// When dragging from the library: icon and name at the mouse pointer, so it is clear what is being moved.
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
                            // icon glyph
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

/// Measurement with `CUTLINE_PERF` (variable read once, not in every frame)
pub fn perf() -> bool {
    static P: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *P.get_or_init(|| std::env::var_os("CUTLINE_PERF").is_some())
}

/// Measurement: compute time per UI frame (sum and maximum since the last `perf` line, µs)
static PERF_SUM_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PERF_MAX_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
struct PerfFrame(Instant);
impl Drop for PerfFrame {
    fn drop(&mut self) {
        let took = self.0.elapsed();
        let us = took.as_micros() as u64;
        PERF_SUM_US.fetch_add(us, std::sync::atomic::Ordering::Relaxed);
        PERF_MAX_US.fetch_max(us, std::sync::atomic::Ordering::Relaxed);
        let sections = PERF_SECTIONS.with(|s| std::mem::take(&mut *s.borrow_mut()));
        if took > SLOW_FRAME {
            let (what, d) = slowest_section(took, &sections);
            eprintln!("perf: langes Bild {:.0} ms, davon {what} {:.0} ms", took.as_secs_f64() * 1000.0, d.as_secs_f64() * 1000.0);
        }
        PERF_LAST.with(|l| *l.borrow_mut() = (took, sections));
    }
}

/// From here on `CUTLINE_PERF` reports a frame as too long.
const SLOW_FRAME: Duration = Duration::from_millis(100);
thread_local! {
    /// Sections of the running UI frame (only with `CUTLINE_PERF`)
    static PERF_SECTIONS: std::cell::RefCell<Vec<(&'static str, Duration)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Duration and sections of the last UI frame (for measurement tests)
    static PERF_LAST: std::cell::RefCell<(Duration, Vec<(&'static str, Duration)>)> = const { std::cell::RefCell::new((Duration::ZERO, Vec::new())) };
}

/// Measure a section of a UI frame (`CUTLINE_PERF`) until the guard drops:
/// `let _s = section("Proxys");`. Without measurement it costs nothing.
pub fn section(what: &'static str) -> PerfSection {
    PerfSection(what, perf().then(Instant::now))
}
pub struct PerfSection(&'static str, Option<Instant>);
impl Drop for PerfSection {
    fn drop(&mut self) {
        if let Some(t) = self.1 {
            PERF_SECTIONS.with(|s| s.borrow_mut().push((self.0, t.elapsed())));
        }
    }
}

/// Still-frame clip (3 s, image `img`) from clip `c` at time `at`: same position, same
/// crop, mirroring, filters and the censoring at this place.
fn freeze_clip(c: &Clip, img: MediaId, at: Time) -> Clip {
    let mut f = Clip::new(
        ClipContent::Media {
            media: img,
            kind: MediaKind::Image,
        },
        at,
        Time::from_secs_f64(3.0),
    );
    f.transform = c.transform;
    f.crop = c.crop;
    f.fx = ClipFx {
        flip_h: c.fx.flip_h,
        flip_v: c.fx.flip_v,
        filter: c.fx.filter.clone(),
        adjust: c.fx.adjust,
        // still frame from the source: the censoring at this place must come along
        censor: c
            .fx
            .censor
            .iter()
            .map(|cs| Censor { path: vec![CensorKey { t: Time::ZERO, ..cs.at(c.source_time(at)) }], track: false, ..cs.clone() })
            .collect(),
        ..Default::default()
    };
    f
}

/// Split-screen column → still frame stays in the column (crop comes along)
#[test]
fn freeze_frame_keeps_crop() {
    let mut c = Clip::new(ClipContent::Media { media: MediaId::new(), kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(5.0));
    c.transform.scale = 1.0;
    c.crop = Crop { left: 1.0 / 3.0, right: 1.0 / 3.0, top: 0.0, bottom: 0.0 };
    let f = freeze_clip(&c, MediaId::new(), Time::from_secs_f64(1.0));
    assert_eq!((f.transform, f.crop), (c.transform, c.crop));
}

/// Duration and sections of the last UI frame (measurement tests)
#[cfg(test)]
pub fn last_frame() -> (Duration, Vec<(&'static str, Duration)>) {
    PERF_LAST.with(|l| l.borrow().clone())
}

/// Most expensive section of a frame; whatever belongs to no section counts as „übrige Oberfläche“.
fn slowest_section(total: Duration, sections: &[(&'static str, Duration)]) -> (&'static str, Duration) {
    let mut sum: std::collections::HashMap<&'static str, Duration> = std::collections::HashMap::new();
    for (w, d) in sections {
        *sum.entry(w).or_default() += *d;
    }
    let rest = total.saturating_sub(sum.values().sum());
    sum.into_iter()
        .chain([("übrige Oberfläche", rest)])
        .max_by_key(|(_, d)| *d)
        .unwrap_or(("übrige Oberfläche", rest))
}

/// Measurement (`CUTLINE_PERF`): one playback or one audio restart
pub struct PerfPlay {
    pub what: &'static str,
    pub pressed: Instant,
    /// Playhead at the start (first frame "advanced" = playhead after it)
    pub at: Time,
    pub frame: bool,
    pub sound: bool,
}

impl PerfPlay {
    fn new(what: &'static str, at: Time) -> Self {
        Self { what, pressed: Instant::now(), at, frame: what != "Play", sound: false }
    }
}

/// One playback for the normal log: one line when it ends (stop, pause, end, jump).
/// The counters are those of `playcore::Stats`, the times those of the `CUTLINE_PERF` measurement points.
pub struct PlayLog {
    pressed: Instant,
    /// Playhead at the start (first frame "advanced" = playhead after it)
    at: Time,
    frame: Option<Duration>,
    sound: Option<Duration>,
    stats: crate::playcore::Stats,
    /// highest quality level reached
    quality: usize,
}

/// Shorter playbacks (double tap on the space bar) write no line
const PLAY_LOG_MIN: Duration = Duration::from_millis(300);

impl PlayLog {
    fn new(at: Time, pressed: Instant) -> Self {
        Self { pressed, at, frame: None, sound: None, stats: Default::default(), quality: 0 }
    }

    /// The line, only measurements (no names, no paths); `None` for a very short playback
    fn line(&self, took: Duration, proxies: bool) -> Option<String> {
        if took < PLAY_LOG_MIN {
            return None;
        }
        let ms = |d: Option<Duration>| d.map_or_else(|| "–".to_string(), |d| format!("{} ms", d.as_millis()));
        let s = &self.stats;
        Some(format!(
            "{:.1} s · erstes Bild {} · erster Ton {} · {} Ströme geöffnet · {} verworfen · Rückstand max {:.0} ms · Lücke max {:.0} ms · aufgeholt {}× · hängend {}× · Qualitätsstufe max {} · Proxys {}",
            took.as_secs_f64(),
            ms(self.frame),
            ms(self.sound),
            s.opens,
            s.dropped,
            s.max_lag * 1000.0,
            s.max_gap * 1000.0,
            s.catch_ups,
            s.stalls,
            self.quality,
            if proxies { "an" } else { "aus" }
        ))
    }
}

#[cfg(test)]
mod play_log_tests {
    use super::*;

    #[test]
    fn line_has_the_values_and_no_names() {
        let mut l = PlayLog::new(Time::ZERO, Instant::now());
        l.frame = Some(Duration::from_millis(180));
        l.stats = crate::playcore::Stats { opens: 2, frames: 100, dropped: 3, catch_ups: 1, stalls: 0, max_gap: 0.09, max_lag: 0.12 };
        l.stats.add(&crate::playcore::Stats { opens: 1, max_gap: 0.05, max_lag: 0.2, ..Default::default() });
        l.quality = 1;
        assert_eq!(
            l.line(Duration::from_millis(4200), true).as_deref(),
            Some("4.2 s · erstes Bild 180 ms · erster Ton – · 3 Ströme geöffnet · 3 verworfen · Rückstand max 200 ms · Lücke max 90 ms · aufgeholt 1× · hängend 0× · Qualitätsstufe max 1 · Proxys an")
        );
    }

    #[test]
    fn very_short_playback_writes_nothing() {
        let l = PlayLog::new(Time::ZERO, Instant::now());
        assert_eq!(l.line(Duration::from_millis(299), false), None);
        assert!(l.line(PLAY_LOG_MIN, false).is_some());
    }

    /// Flow: Play begins the playback, the quality level is tracked, stop ends it
    #[test]
    fn playback_starts_and_ends_the_record() {
        let mut app = crate::saving::tests::test_app();
        app.core.play(Time::ZERO, 0, 0.0);
        let _ = app.core.cmds();
        let on = app.core.playing();
        app.play_log_step(on, None);
        assert!(app.play_log.is_some());
        app.core.quality.level = 2;
        app.play_log_step(true, None);
        app.core.quality.level = 0;
        app.play_log_step(true, None);
        assert_eq!(app.play_log.as_ref().map(|l| l.quality), Some(2));
        app.core.stop();
        let _ = app.core.cmds();
        let on = app.core.playing();
        app.play_log_step(on, None);
        assert!(app.play_log.is_none());
    }
}

/// Fine timer while it lives (Windows: `timeBeginPeriod(1)`; the event loop otherwise waits
/// on the 15.6 ms grid and frames come too late). Other systems are exact anyway.
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
        Self {
            // SAFETY: simple system call without pointers; 0 = success (otherwise the coarse timer stays)
            #[cfg(windows)]
            ok: unsafe { timeBeginPeriod(1) } == 0,
        }
    }
}

impl Drop for HiresTimer {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.ok {
            // SAFETY: counterpart to the successful timeBeginPeriod(1)
            unsafe { timeEndPeriod(1) };
        }
    }
}

/// Frame time egui subtracts per `request_repaint_after` (`RawInput::predicted_dt`, eframe never
/// sets it differently)
const EGUI_PREDICTED_DT: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Start of frame `n` at `fps` (µs, rounded up: from then on the frame is due)
fn frame_start(n: i128, fps: Rational) -> i128 {
    let (num, den) = (fps.num.max(1) as i128, fps.den.max(1) as i128);
    (n * den * 1_000_000).div_euclid(num) + i128::from((n * den * 1_000_000).rem_euclid(num) != 0)
}

/// Next frame change after `pos` in the play direction (from the frame number, so without drift)
fn next_frame_at(pos: Time, fps: Rational, forward: bool) -> Time {
    let (num, den) = (fps.num.max(1) as i128, fps.den.max(1) as i128);
    let n = (pos.0 as i128 * num).div_euclid(den * 1_000_000);
    let mut at = if forward { frame_start(n + 1, fps) } else { frame_start(n, fps) };
    if !forward && at >= pos.0 as i128 {
        at = frame_start(n - 1, fps);
    }
    Time(at as i64)
}

/// Waiting time until the next frame change at play speed `rate` (+1 ms, so the clock is then
/// surely past the boundary; a frame too early showed nothing new)
fn next_frame_delay(pos: Time, fps: Rational, rate: f64) -> Duration {
    let rate = if rate.abs() < 1e-6 { 1.0 } else { rate };
    let at = next_frame_at(pos, fps, rate > 0.0);
    let secs = ((at - pos).as_secs_f64() / rate).abs();
    Duration::from_secs_f64(secs.min(1.0)) + Duration::from_millis(1)
}

/// Shortest distance between two UI frames (at most 125 per second). Without VSync (see `main.rs`)
/// egui otherwise draws several hundred frames per second unthrottled on every animation (hover tiles, fade-ins)
/// and occupies a whole core. Playback requests less often (`play_repaint_delay`).
const MIN_FRAME: Duration = Duration::from_millis(8);

/// Waiting time before the next UI frame if the last one began `since_last` ago
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

/// Measurement point at startup (`CUTLINE_PERF`): time since the first call (in `main`)
pub fn perf_mark(what: &str) {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let t0 = *T0.get_or_init(Instant::now);
    if perf() {
        crate::applog::perf_line!("perf: Start {what} nach {} ms", t0.elapsed().as_millis());
    }
}

/// For documentation: with `CUTLINE_SHOTS=<folder>` Cutline saves the window image as PNG
/// as soon as a file `take` lies there (straight from the app, without overlapping windows).
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

/// Draw content in exactly the available area. egui's side panels otherwise grow with
/// their widest content, and switching tabs makes the whole UI jump.
/// This way only the user (dragging the edge) determines the width.
pub fn fixed_width(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let rect = ui.available_rect_before_wrap();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(*ui.layout()));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    add(&mut child);
    ui.allocate_rect(rect, egui::Sense::hover());
}

/// Display time of a toast: errors stay longer (and close on click)
/// Text of a toast with an icon in front; if the message already brings its own icon,
/// that one stays (otherwise two check marks would stand before „Gespeichert“).
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

/// Opacity of a toast at age `age` (s); fade in/out duration 0 = there/gone at once
fn toast_alpha(age: f32, life: f32, fade_in: f32, fade_out: f32) -> f32 {
    let a_in = if fade_in > 0.0 { age / fade_in } else { 1.0 };
    let a_out = if fade_out > 0.0 { (life - age) / fade_out } else if age < life { 1.0 } else { 0.0 };
    a_in.min(a_out).clamp(0.0, 1.0)
}

/// `saved_revision` after `set_doc`: no real revision hits this value.
pub const UNSAVED: u64 = u64::MAX - 1;

/// Write unsaved changes? An empty new project creates no file; a
/// saved one that was emptied is saved (otherwise the old state would come back).
fn needs_save(rev: u64, saved: u64, has_file: bool, p: &Project) -> bool {
    rev != saved && (has_file || !(p.media.is_empty() && p.duration() == Time::ZERO))
}

/// Time of a medium's thumbnail: video shortly after the start, animated image (APNG/GIF) in
/// the middle – many elements begin with an empty frame and only fade in then.
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
        // restore/template/review replace the document: revision starts at 0
        let p = Project::new("x");
        let rev = Document::new(p.clone()).revision();
        assert!(needs_save(rev, UNSAVED, true, &p));
    }

    #[test]
    fn emptied_saved_project_is_saved() {
        let empty = Project::new("x");
        assert!(needs_save(5, 3, true, &empty), "saved and then emptied");
        assert!(!needs_save(5, 3, false, &empty), "new empty project: no file");
        assert!(!needs_save(3, 3, true, &empty));
    }

    #[test]
    fn toast_shows_one_symbol() {
        let saved = toast_line(&format!("{} Gespeichert", icon::CHECK), false);
        assert_eq!(saved.matches(icon::CHECK).count() + saved.matches(icon::CHECK_CIRCLE).count(), 1, "{saved}");
        assert_eq!(toast_line(&format!("{} Export fertig (1.0 s)", icon::CHECK_CIRCLE), false).matches(icon::CHECK_CIRCLE).count(), 1);
        // without its own icon: check mark or warning in front
        assert_eq!(toast_line("Kopiert", false), format!("{}  Kopiert", icon::CHECK_CIRCLE));
        assert!(toast_line("Fehler", true).starts_with(icon::WARNING_CIRCLE));
    }

    #[test]
    fn toast_alpha_without_animation() {
        assert_eq!(toast_alpha(0.0, 4.0, 0.0, 0.0), 1.0);
        assert_eq!(toast_alpha(3.99, 4.0, 0.0, 0.0), 1.0);
        assert_eq!(toast_alpha(4.0, 4.0, 0.0, 0.0), 0.0);
        // with animations as before
        assert_eq!(toast_alpha(0.0, 4.0, 0.15, 0.4), 0.0);
        assert!((toast_alpha(3.8, 4.0, 0.15, 0.4) - 0.5).abs() < 1e-4);
        assert!(toast_secs(true) > toast_secs(false));
    }
}

#[cfg(test)]
mod frame_rate_tests {
    use super::*;

    /// Flaw from the click test: the space bar did not start playback while a button/tile had the
    /// focus; a text field with focus keeps the keys, on the other hand.
    #[test]
    fn only_text_fields_block_shortcuts() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        let mut pass = |ctx: &egui::Context| {
            let mut ids = (egui::Id::NULL, egui::Id::NULL);
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ids.0 = ui.button("Kachel").id;
                    ids.1 = ui.text_edit_singleline(&mut text).id;
                });
            });
            ids
        };
        let ids = pass(&ctx);
        ctx.memory_mut(|m| m.request_focus(ids.0));
        pass(&ctx);
        assert!(ctx.wants_keyboard_input() && !typing(&ctx), "a button with focus is no text input");
        ctx.memory_mut(|m| m.request_focus(ids.1));
        pass(&ctx);
        assert!(typing(&ctx), "text field with focus");
    }

    /// Review finding: the focus was released every frame – Tab navigation and arrows
    /// on a slider no longer worked. Release only when the space bar comes (which always plays).
    /// Click test K5: Tab up to the window X, then space – the window closed because egui
    /// triggered the focused button in the same frame. Now the key never reaches the button
    /// but the shortcuts (Play/Pause); in the text field it stays text.
    #[test]
    fn space_never_clicks_a_focused_button() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        let space = egui::Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        let frame = |ctx: &egui::Context, events: Vec<egui::Event>, hold: bool, text: &mut String| {
            let mut raw = egui::RawInput { events, ..Default::default() };
            let held = if hold { hold_space(ctx, &mut raw) } else { Vec::new() };
            let mut ids = (egui::Id::NULL, egui::Id::NULL, false, false);
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let b = ui.button("X");
                    ids.0 = b.id;
                    ids.2 = b.clicked();
                    ids.1 = ui.text_edit_singleline(text).id;
                });
                // shortcuts: the returned key triggers Play
                ctx.input_mut(|i| i.events.extend(held.clone()));
                ids.3 = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Space));
            });
            ids
        };
        let ids = frame(&ctx, vec![], true, &mut text);
        ctx.memory_mut(|m| m.request_focus(ids.0));
        frame(&ctx, vec![], true, &mut text);
        // without intercepting: the button would be triggered (that is how the window closed)
        let before = frame(&ctx, vec![space.clone()], false, &mut text);
        assert!(before.2, "precondition: egui clicks the focused button with the space bar");
        ctx.memory_mut(|m| m.request_focus(ids.0));
        frame(&ctx, vec![], true, &mut text);
        let after = frame(&ctx, vec![space.clone()], true, &mut text);
        assert!(!after.2, "space bar does not click the button");
        assert!(after.3, "space bar goes to Play/Pause");
        assert!(ctx.memory(|m| m.focused()).is_none(), "focus released");
        // text field with focus: the space bar stays with the field
        ctx.memory_mut(|m| m.request_focus(ids.1));
        frame(&ctx, vec![], true, &mut text);
        let mut raw = egui::RawInput { events: vec![space], ..Default::default() };
        assert!(hold_space(&ctx, &mut raw).is_empty() && raw.events.len() == 1);
    }

    #[test]
    fn focus_is_kept_except_for_space() {
        assert_eq!(key_focus(false, false, false), KeyFocus::Shortcuts, "nothing focused");
        assert_eq!(key_focus(false, false, true), KeyFocus::Shortcuts);
        assert_eq!(key_focus(false, true, true), KeyFocus::Release, "button + space: play");
        assert_eq!(key_focus(false, true, false), KeyFocus::Keep, "slider + arrow, Tab: stays");
        assert_eq!(key_focus(true, true, true), KeyFocus::Keep, "text field + space: text");
        assert_eq!(key_focus(true, true, false), KeyFocus::Keep, "text field + arrow");
    }

    #[test]
    fn next_frame_is_due_on_the_frame_grid_without_drift() {
        for fps in [Rational::new(24, 1), Rational::new(25, 1), Rational::new(30, 1), Rational::new(50, 1), Rational::new(60, 1), Rational::new(30000, 1001)] {
            let f = fps.as_f64();
            let mut pos = Time::ZERO;
            for n in 1..=10_000i64 {
                pos = next_frame_at(pos, fps, true);
                // exactly the (rounded up) frame start, regardless of history
                let exact = n as f64 / f * 1e6;
                assert!(pos.0 as f64 >= exact - 1e-6 && (pos.0 as f64) < exact + 1.0, "{fps:?} frame {n}: {pos:?}");
                // the clock really lies in frame n there
                assert_eq!((pos.as_secs_f64() * f + 1e-9).floor() as i64, n, "{fps:?} {n}");
            }
            // in the middle of a frame: wait until the end of the frame (+1 ms), never longer than a frame
            let mid = Time::from_secs_f64(100.5 / f);
            let d = next_frame_delay(mid, fps, 1.0).as_secs_f64();
            assert!((d - (0.5 / f + 0.001)).abs() < 2e-6, "{fps:?} {d}");
            assert!(next_frame_delay(Time::from_secs_f64(7.0 / f), fps, 1.0).as_secs_f64() <= 1.0 / f + 0.0011);
        }
        // 30 fps: largest gap 33.3 ms + 1 ms ≤ 40 ms
        let worst = (0..1000)
            .map(|i| next_frame_delay(Time(i * 997), Rational::new(30, 1), 1.0))
            .max()
            .unwrap();
        assert!(worst <= Duration::from_millis(40), "{worst:?}");
        // double speed: half the waiting time; reverse: until the start of the current frame
        let fps = Rational::new(30, 1);
        let t = Time::from_secs_f64(10.5 / 30.0);
        assert!((next_frame_delay(t, fps, 2.0).as_secs_f64() - (0.25 / 30.0 + 0.001)).abs() < 2e-6);
        assert_eq!(next_frame_at(t, fps, false), Time(frame_start(10, fps) as i64));
        assert_eq!(next_frame_at(Time(frame_start(10, fps) as i64), fps, false), Time(frame_start(9, fps) as i64));
        assert!((next_frame_delay(t, fps, -1.0).as_secs_f64() - (0.5 / 30.0 + 0.001)).abs() < 2e-6);
    }

    /// The last line pins a decision: the UI draws at most 125 frames/s (`MIN_FRAME`).
    #[test]
    fn frame_wait_caps_ui_rate() {
        assert_eq!(frame_wait(Duration::from_millis(2)), Duration::from_millis(6));
        assert_eq!(frame_wait(Duration::ZERO), MIN_FRAME);
        // rare frames (idle, playback) never wait
        assert_eq!(frame_wait(Duration::from_millis(30)), Duration::ZERO);
        assert!(1.0 / MIN_FRAME.as_secs_f64() <= 125.0);
    }
}

#[cfg(test)]
#[path = "load_perf.rs"]
mod load_perf;

/// Unreadable file: short message with the name, no raw FFmpeg text in the UI
#[test]
fn unreadable_file_gives_a_short_message() {
    let mut app = crate::saving::tests::test_app();
    app.set_doc(Project::new("Kaputt"));
    let f = app.dir().join("kaputt.mp4");
    std::fs::write(&f, b"kein Video").unwrap();
    assert!(app.import_media(vec![f]).is_empty());
    let msgs: Vec<&str> = app.toasts.iter().map(|t| t.0.as_str()).collect();
    assert_eq!(msgs, ["Datei kann nicht gelesen werden: „kaputt.mp4“"]);
}

/// A stock download still running when the project changes is not inserted into the new project
#[test]
fn download_running_at_a_project_change_is_not_inserted() {
    let mut app = crate::saving::tests::test_app();
    app.set_doc(Project::new("Alt"));
    app.stock.busy = true;
    app.es.loading_track = Some("t1".into());
    app.reset_editor();
    assert!(app.stock.stray && app.es.stray);
    app.set_doc(Project::new("Neu"));
    app.stock.test_downloaded("Klang");
    app.poll_stock();
    assert!(app.project.media.is_empty() && app.project.timeline.tracks.iter().all(|t| t.clips.is_empty()));
    assert!(!app.stock.stray && app.toasts.iter().any(|t| t.0.contains("Klang")));
}

/// Project change: the export's rebuild snapshot belongs to the old project (same state 0,
/// same format would otherwise hit the same key)
#[test]
fn export_reframe_snapshot_is_dropped_with_the_project() {
    let mut app = crate::saving::tests::test_app();
    app.export.framed = Some(((0, (1080, 1920), 0, 0), std::sync::Arc::new(Project::new("Alt"))));
    app.reset_editor();
    assert!(app.export.framed.is_none());
}

/// Command search in the flow of a frame: Ctrl+K opens, the field has the focus, the space bar types
/// a space (no playback), Esc closes only the search (selection stays), Enter runs.
#[test]
fn command_search_keys_stay_in_the_search() {
    let mut app = crate::saving::tests::test_app();
    app.set_doc(Project::new("Suche"));
    let ctx = egui::Context::default();
    let key = |key: Key, modifiers: Modifiers| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
    let frame = |app: &mut App, events: Vec<egui::Event>| {
        let mut raw = egui::RawInput { events, ..Default::default() };
        eframe::App::raw_input_hook(app, &ctx, &mut raw);
        let _ = ctx.run(raw, |ctx| {
            if !app.esc_closes(ctx) {
                app.shortcuts(ctx);
            }
            app.command_palette(ctx);
        });
    };
    let clip = ClipId::new();
    app.selected = Some(clip);
    frame(&mut app, vec![key(Key::K, Modifiers::COMMAND)]);
    assert!(app.palette.open, "Ctrl+K opens");
    frame(&mut app, vec![]);
    assert!(typing(&ctx), "the field has the focus");
    frame(&mut app, vec![key(Key::Space, Modifiers::NONE), egui::Event::Text(" ".into())]);
    assert!(!app.core.playing() && app.palette.open, "space bar does not play");
    assert_eq!(app.palette.query, " ", "… but types a space");
    frame(&mut app, vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(!app.palette.open, "Esc closes the search");
    assert_eq!(app.selected, Some(clip), "… and leaves the selection alone");
    frame(&mut app, vec![]);
    assert_eq!(app.selected, Some(clip));
    // open again (field empty), type, Enter runs the first hit
    frame(&mut app, vec![key(Key::K, Modifiers::COMMAND)]);
    frame(&mut app, vec![egui::Event::Text("einstellungen".into())]);
    frame(&mut app, vec![key(Key::Enter, Modifiers::NONE)]);
    assert!(app.settings_open && !app.palette.open);
    assert_eq!(app.selected, Some(clip));
}

/// Esc in the field only discards the input
#[test]
fn esc_in_a_field_is_not_a_shortcut() {
    assert!(esc_for_field(true, true));
    assert!(!esc_for_field(false, true), "without a field Esc clears the selection");
    assert!(!esc_for_field(true, false));
}

/// A message with a very long name stays in the window (shortened) instead of being cut off left and right
#[test]
fn long_toast_stays_inside_the_window() {
    let mut app = crate::saving::tests::test_app();
    app.toast(format!("Vorlage „{}“ gespeichert", "sehr lang ".repeat(40)));
    let ctx = egui::Context::default();
    let raw = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0))), ..Default::default() };
    for _ in 0..3 {
        let _ = ctx.run(raw(), |ctx| app.draw_toasts(ctx));
    }
    let r = ctx.memory(|m| m.area_rect(egui::Id::new("toasts"))).unwrap();
    assert!(r.left() >= 0.0 && r.right() <= 800.0, "{r:?}");
}

/// Q / W with clips on several tracks selected: every selected clip under the playhead is cut,
/// as one undo step (before: only the primary selection, the clip clicked last)
#[test]
fn cut_left_and_right_hit_every_selected_clip() {
    let mut app = crate::saving::tests::test_app();
    let t = Time::from_secs_f64;
    let mut p = Project::new("QW");
    let mut ids = Vec::new();
    for _ in 0..3 {
        let mut tr = cl_core::Track::new(cl_core::TrackKind::Text);
        let c = Clip::new(ClipContent::Text(cl_core::TextStyle::default()), t(0.0), t(4.0));
        ids.push(c.id);
        tr.clips.push(c);
        p.timeline.tracks.push(tr);
    }
    app.set_doc(p);
    let spans = |app: &App| ids.iter().map(|id| app.project.find_clip(*id).map(|(_, c)| (c.start, c.end())).unwrap()).collect::<Vec<_>>();
    for left in [true, false] {
        app.select_all();
        app.playhead = t(1.0);
        app.delete_side(left);
        app.sync_project();
        let want = if left { (t(1.0), t(4.0)) } else { (t(0.0), t(1.0)) };
        assert_eq!(spans(&app), vec![want; 3], "left {left}");
        app.undo();
        app.sync_project();
        assert_eq!(spans(&app), vec![(t(0.0), t(4.0)); 3], "one undo restores all (left {left})");
    }
}

#[test]
fn close_goes_home_only_with_the_setting_and_an_open_project() {
    for setting in [false, true] {
        for open in [false, true] {
            for explicit in [false, true] {
                assert_eq!(close_goes_home(setting, open, explicit), setting && open && !explicit, "{setting} {open} {explicit}");
            }
        }
    }
}

/// Closing the window with „Beim Schließen zur Projektübersicht“ on: start page, project saved,
/// lock released, the close cancelled; off: the close goes through and the project stays
#[test]
fn window_close_goes_to_the_start_page_when_switched_on() {
    let ctx = egui::Context::default();
    let close = || {
        let mut raw = egui::RawInput::default();
        raw.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { events: vec![egui::ViewportEvent::Close], ..Default::default() });
        raw
    };
    for on in [true, false] {
        let mut app = crate::saving::tests::test_app();
        app.settings.close_to_home = on;
        app.set_doc(Project::new("Schließen"));
        let p = app.dir().join("schliessen.cutline");
        app.project_path = Some(p.clone());
        app.take_lock(&p);
        app.screen = Screen::Editor;
        app.apply(EditOp::Rename { name: "Geändert".into() });
        let out = ctx.run(close(), |ctx| {
            app.export.guard_close(ctx);
            app.close_to_home(ctx);
        });
        let cancelled = out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::CancelClose);
        assert_eq!(cancelled, on, "{on}");
        if on {
            assert!(app.screen == Screen::Home);
            assert_eq!(Project::from_json(&std::fs::read_to_string(&p).unwrap()).unwrap().name, "Geändert");
            assert!(!paths::lock_path(&p).exists(), "lock released");
            // on the start page the next close quits
            let out = ctx.run(close(), |ctx| app.close_to_home(ctx));
            assert!(!out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::CancelClose));
        } else {
            assert!(app.screen == Screen::Editor && paths::lock_path(&p).exists());
        }
    }
}

/// Splitting several selected clips: afterwards one piece of every one is selected (the right one,
/// as for a single clip); a selected clip the playhead misses stays selected
#[test]
fn split_keeps_every_selected_clip_selected() {
    let mut app = crate::saving::tests::test_app();
    let t = Time::from_secs_f64;
    let mut p = Project::new("Teilen");
    let mut ids = Vec::new();
    for at in [0.0, 0.0, 0.0, 10.0] {
        let mut tr = cl_core::Track::new(cl_core::TrackKind::Text);
        let c = Clip::new(ClipContent::Text(cl_core::TextStyle::default()), t(at), t(4.0));
        ids.push(c.id);
        tr.clips.push(c);
        p.timeline.tracks.push(tr);
    }
    app.set_doc(p);
    app.select_all();
    let primary = app.selected.unwrap();
    app.playhead = t(1.0);
    app.split_at_playhead();
    app.sync_project();
    let sel = app.selection();
    assert_eq!(sel.len(), 4, "{sel:?}");
    let mut per_track: Vec<(TrackId, Time)> = sel.iter().map(|c| app.project.find_clip(*c).map(|(tr, c)| (tr.id, c.start)).unwrap()).collect();
    per_track.sort_by_key(|x| x.1);
    assert_eq!(per_track.iter().map(|x| x.1).collect::<Vec<_>>(), [t(1.0), t(1.0), t(1.0), t(10.0)], "right halves plus the untouched clip");
    let tracks: HashSet<TrackId> = per_track.iter().map(|x| x.0).collect();
    assert_eq!(tracks.len(), 4, "one per track");
    assert!(sel.contains(&ids[3]));
    // the primary selection is the piece of the clip that was primary
    let (pt, _) = app.project.find_clip(app.selected.unwrap()).map(|(tr, c)| (tr.id, c.start)).unwrap();
    assert_eq!(Some(pt), app.project.timeline.tracks.iter().find(|tr| tr.clips.iter().any(|c| c.id == primary)).map(|tr| tr.id));
}

/// File with two audio streams: extracting stream 1 and then stream 2 gives two audible clips at
/// the video's volume (no −60 dB inherited), the video only loses the extracted streams, the same
/// stream is not extracted twice, and each extraction is one undo step.
#[test]
fn extracting_two_audio_streams_keeps_both_audible() {
    let mut app = crate::saving::tests::test_app();
    let path = app.dir().join("zwei spuren.mp4");
    std::fs::write(&path, b"x").unwrap();
    let m = MediaRef { id: MediaId::new(), path, kind: MediaKind::Video, duration: Time::from_secs_f64(9.0), width: 1920, height: 1080, fps: None, has_audio: true, audio_streams: 2, video_codec: None, audio_codec: None };
    let mut p = Project::new("Spuren");
    let mut t = Track::new(TrackKind::Video);
    t.main = true;
    let mut v = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(4.0));
    v.volume_db = -3.0;
    let vid = v.id;
    t.clips.push(v);
    p.timeline.tracks.push(t);
    p.media.push(m);
    app.set_doc(p);
    let video = |app: &App| app.project.find_clip(vid).unwrap().1.clone();
    // audible (clip id, stream) pairs, as the preview and export mix see them
    let audible = |app: &App| {
        let mut v: Vec<(bool, u32, f32)> = cl_export::audible_clips(&app.project, Time::ZERO, Time::from_secs_f64(4.0)).map(|(c, m)| (c.id == vid, c.played_stream(m), c.volume_db)).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v
    };
    assert_eq!(audible(&app), [(true, 0, -3.0)]);
    let before = app.project.clone();
    app.extract_audio(vid);
    assert_eq!(audible(&app), [(false, 0, -3.0)], "stream 1 now only in its own clip, at the video's volume");
    assert_eq!((video(&app).volume_db, video(&app).extracted_streams), (-3.0, vec![0]));
    app.undo();
    app.sync_project();
    assert_eq!(app.project.timeline, before.timeline, "one undo step");
    app.redo();
    app.sync_project();
    // the same stream again: refused, no second copy
    let n = app.project.timeline.tracks.iter().map(|t| t.clips.len()).sum::<usize>();
    app.extract_audio(vid);
    assert_eq!(app.project.timeline.tracks.iter().map(|t| t.clips.len()).sum::<usize>(), n);
    // the video switched to stream 2 (inspector): audible again at its own volume
    let mut c = video(&app);
    c.audio_stream = Some(1);
    app.apply(EditOp::ReplaceClip { clip: c });
    assert_eq!(audible(&app), [(false, 0, -3.0), (true, 1, -3.0)]);
    let before = app.project.clone();
    app.extract_audio(vid);
    assert_eq!(audible(&app), [(false, 0, -3.0), (false, 1, -3.0)], "stream 2 extracted audible");
    assert_eq!(video(&app).extracted_streams, [0, 1]);
    app.undo();
    app.sync_project();
    assert_eq!(app.project.timeline, before.timeline, "one undo step");
}
