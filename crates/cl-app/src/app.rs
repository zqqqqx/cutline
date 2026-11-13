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
    Plugins,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InspectorTab {
    Project,
    History,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClipTab {
    Basic,
    Text,
    Effects,
    Adjust,
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
    Transition(&'static str),
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
    /// Multicam-Modus: (Blickwinkel-Clips, Wechsel (Zeit, Index))
    pub multicam: Option<(Vec<ClipId>, Vec<(Time, usize)>)>,
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
    pub thumbs: HashMap<MediaId, egui::TextureHandle>,
    pub waves: HashMap<MediaId, Vec<f32>>,
    pub fx_thumbs: HashMap<String, egui::TextureHandle>,
    pub fx_thumbs_requested: HashSet<String>,
    pub sticker_tex: HashMap<&'static str, egui::TextureHandle>,
    pub updater: crate::update::Updater,
    pub tasks: crate::tasks::Tasks,
    pub pick: Option<crate::pro::Pick>,
    pub pick_origin: Option<egui::Pos2>,
    pub frame_cpu: Option<(ClipId, cl_media::RgbaFrame)>,
    pub caption_lang: String,
    pub caption_from_clip: bool,
    registered_fonts: HashSet<String>,
    /// egui übernimmt `set_fonts` erst im nächsten Frame. Bis dahin bleiben neue Familien hier.
    pending_fonts: Option<HashSet<String>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx);
        crate::set_dialog_parent(cc);
        let settings = AppSettings::load();
        let onboard = (!settings.onboarding_done
            && std::env::var_os("CUTLINE_NO_ONBOARDING").is_none())
        .then(|| crate::onboarding::Onboard::new(&settings.use_cases));
        let fonts = cl_fonts::FontLibrary::open(paths::font_dir()).unwrap_or_default();
        let plugin_dirs = paths::plugin_dirs();
        let mut plugins = cl_plugin::Registry::scan(plugin_dirs.iter().map(|p| p.as_path()));
        plugins.apply_states(&settings.plugin_states);
        let lang = settings.lang();
        cl_fx::register_plugin_fx(
            plugins
                .ffmpeg_assets(lang.code())
                .into_iter()
                .filter(|a| {
                    matches!(
                        a.1,
                        cl_plugin::AssetKind::VideoEffect | cl_plugin::AssetKind::Filter
                    )
                })
                .map(|(id, kind, name, chain)| cl_fx::PluginFx {
                    id,
                    name,
                    is_filter: kind == cl_plugin::AssetKind::Filter,
                    chain,
                })
                .collect(),
        );

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
            multicam: None,
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
            thumbs: HashMap::new(),
            waves: HashMap::new(),
            fx_thumbs: HashMap::new(),
            fx_thumbs_requested: HashSet::new(),
            sticker_tex: HashMap::new(),
            updater: Default::default(),
            tasks: Default::default(),
            pick: None,
            pick_origin: None,
            frame_cpu: None,
            caption_lang: "auto".into(),
            caption_from_clip: false,
            registered_fonts: HashSet::new(),
            pending_fonts: None,
        };
        app.register_fonts(&cc.egui_ctx);
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
        let r = self.doc.lock().unwrap().apply(op, OpSource::Ui);
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

    pub fn undo(&mut self) {
        self.live = None;
        let r = self.doc.lock().unwrap().undo();
        match r {
            Ok(l) => self.toast(format!("{}: {}", self.t("undo"), crate::tr(l))),
            Err(e) => self.toast_err(e.to_string()),
        }
    }
    pub fn redo(&mut self) {
        self.live = None;
        let r = self.doc.lock().unwrap().redo();
        match r {
            Ok(l) => self.toast(format!("{}: {}", self.t("redo"), crate::tr(l))),
            Err(e) => self.toast_err(e.to_string()),
        }
    }

    pub fn sync_project(&mut self) {
        let doc = self.doc.lock().unwrap();
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
                if self.playing && self.voiceover.recorder.is_none() && self.play_warmup.is_none() {
                    self.restart_audio(); // hörbare Änderungen sofort hörbar
                }
            }
            for m in &self.project.media {
                if m.has_audio && !self.waves.contains_key(&m.id) {
                    self.waves.insert(m.id, Vec::new()); // Platzhalter: Auftrag nur einmal senden
                    self.decoder.send(Job::Wave {
                        media: m.id,
                        path: m.path.clone(),
                    });
                }
                if m.kind != MediaKind::Audio && !self.thumbs.contains_key(&m.id) {
                    let at = if m.kind == MediaKind::Video {
                        Time::from_secs_f64((m.duration.as_secs_f64() * 0.1).min(1.0))
                    } else {
                        Time::ZERO
                    };
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
            match cl_media::probe(&f) {
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
            .or_else(|| self.doc.lock().unwrap().project().media(media).cloned())?;
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
        let path = paths::data_dir().join("Stickers").join(format!("{id}.png"));
        if !path.is_file() {
            if let Err(e) = cl_fx::stickers::render_png(st, 512, &path) {
                self.toast_err(e);
                return None;
            }
        }
        self.add_image_overlay_at(path, at, track)
    }

    /// Bild als Sticker-Clip (verkleinert) auf eine freie Overlay-Spur legen.
    pub fn add_image_overlay_at(
        &mut self,
        path: PathBuf,
        at: Time,
        track: Option<TrackId>,
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

    pub fn sfx_path(id: &str) -> PathBuf {
        paths::data_dir().join("Sfx").join(format!("{id}.wav"))
    }

    pub fn add_sfx_at(&mut self, id: &str, at: Time, track: Option<TrackId>) -> Option<ClipId> {
        let s = cl_fx::sfx::sfx(id)?;
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
                Some(c) => self.set_transition(c, Some(id)),
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

    /// Effekt oder Filter als eigenes Element (3 s) bei `at` einfügen.
    pub fn add_fx_clip(
        &mut self,
        id: &str,
        filter: bool,
        at: Time,
        track: Option<TrackId>,
    ) -> Option<ClipId> {
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
        let mut clips: Vec<Clip> = self
            .selection()
            .iter()
            .filter_map(|c| self.project.find_clip(*c).map(|(_, c)| c.clone()))
            .collect();
        clips.sort_by_key(|c| c.start);
        if !clips.is_empty() {
            self.toast(format!(
                "{} {} kopiert",
                clips.len(),
                if clips.len() == 1 { "Clip" } else { "Clips" }
            ));
            self.clipboard = clips;
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
        let first = self
            .clipboard
            .iter()
            .map(|c| c.start)
            .min()
            .unwrap_or(Time::ZERO);
        let ops: Vec<EditOp> = self
            .clipboard
            .iter()
            .map(|c| {
                let mut n = c.clone();
                n.id = ClipId::new();
                n.start = self.playhead + (c.start - first);
                EditOp::AddClip {
                    track: None,
                    clip: n,
                }
            })
            .collect();
        if let Some(OpOutput::Clip(c)) = self.apply_batch(ops) {
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
        let name: String = self
            .project
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let dir = paths::data_dir().join("Projects");
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
        std::fs::write(p, self.project.to_json()).map_err(|e| e.to_string())?;
        self.saved_revision = self.revision;
        self.last_saved = Some(Instant::now());
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
        if self.revision == self.saved_revision
            || self.project.duration() == Time::ZERO && self.project.media.is_empty()
        {
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
                    if self.write_project(&p).is_ok() {
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

    fn reset_editor(&mut self) {
        self.stop_playback();
        self.select_only(None);
        self.live = None;
        self.playhead = Time::ZERO;
        self.timeline = TimelineState::default();
        self.frames.clear();
        self.invalidate_preview();
        self.player_zoom = 1.0;
        self.player_pan = egui::Vec2::ZERO;
    }

    pub fn new_project(&mut self) {
        self.flush_before_leaving();
        self.reset_editor();
        let n = crate::home::next_project_name();
        *self.doc.lock().unwrap() = Document::new(Project::new(n));
        self.revision = u64::MAX;
        self.sync_project();
        self.saved_revision = self.revision;
        self.project_path = None;
        self.screen = Screen::Editor;
    }

    pub fn open_project(&mut self, p: &Path) {
        self.flush_before_leaving();
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
                *self.doc.lock().unwrap() = Document::new(proj);
                self.revision = u64::MAX;
                self.sync_project();
                self.saved_revision = self.revision;
                self.project_path = Some(p.to_path_buf());
                self.settings.add_recent(p);
                self.settings.save();
                self.screen = Screen::Editor;
                self.last_backup = Some(Instant::now());
                self.missing_open = self
                    .project
                    .media
                    .iter()
                    .any(|m| !m.path.is_file() && !m.path.to_string_lossy().contains("://"));
                let known = self.font_files();
                let missing: Vec<String> = needed
                    .into_iter()
                    .filter(|f| !known.contains_key(f))
                    .collect();
                if !missing.is_empty() {
                    self.toast_err(format!("Fehlende Schriften: {}", missing.join(", ")));
                }
            }
            Err(e) => self.toast_err(e),
        }
    }

    /// Vor dem Verlassen eines Projekts: offene Änderungen speichern.
    fn flush_before_leaving(&mut self) {
        self.commit_live();
        if self.voiceover.recorder.is_some() {
            self.stop_voiceover();
        }
        if self.revision != self.saved_revision
            && !(self.project.media.is_empty() && self.project.duration() == Time::ZERO)
        {
            let p = self
                .project_path
                .clone()
                .unwrap_or_else(|| self.default_project_path());
            if self.write_project(&p).is_ok() {
                self.project_path = Some(p);
            }
        }
        self.settings.save();
    }

    pub fn go_home(&mut self) {
        self.flush_before_leaving();
        self.stop_playback();
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
            self.stop_playback();
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
            // Erst puffern: Playhead und Ton laufen los, wenn die Bildströme bereit sind
            self.audio = None;
            self.play_clock = None;
            self.play_warmup = Some(Instant::now());
        }
    }

    pub fn stop_playback(&mut self) {
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

    pub fn restart_audio(&mut self) {
        self.audio = None;
        self.play_clock = Some((Instant::now(), self.playhead));
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
            match AudioPlayer::start(&proj, self.playhead) {
                Ok(a) => self.audio = Some(a),
                Err(e) => eprintln!("Ton: {e}"),
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
                self.restart_audio();
            }
            return;
        }
        match &self.audio {
            Some(a) if a.started() => self.playhead = a.position(),
            Some(_) => {} // Ton startet gleich, Bild wartet
            None => {
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

    /// Vorhören eines Soundeffekts bzw. einer Audiodatei in der Liste.
    pub fn preview_sound(&mut self, key: String, path: &Path) {
        if self.sfx_preview.as_ref().is_some_and(|(k, _)| *k == key) {
            self.sfx_preview = None;
            return;
        }
        let Ok(m) = cl_media::probe(path) else { return };
        self.preview_media(key, m);
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
            Ok(a) => self.sfx_preview = Some((key, a)),
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
                    .map(|d| d.join(format!("{}-media", self.project.name)))
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
                        self.select_only(Some(c));
                        self.toast(format!("{} {}", icon::MICROPHONE, self.t("voiceover")));
                    }
                }
            }
            Err(e) => self.toast_err(e),
        }
    }

    // ---------- Tastatur ----------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() || self.settings_open {
            return;
        }
        let fps = self.project.settings.fps;
        let frame = Time::from_secs_f64(1.0 / fps.as_f64());
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
                        .open_for(&self.project, self.project_path.as_deref());
                    self.export.set_range(self.range_in.zip(self.range_out));
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
                "escape" => {
                    if self.pick.is_some() {
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
            .retain(|(_, t, _)| t.elapsed() < Duration::from_secs(4));
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -24.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                // Meldungen nicht Wort für Wort umbrechen (die Fläche hat keine feste Breite)
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                for (msg, _, err) in self.toasts.iter().rev().take(3) {
                    egui::Frame::default()
                        .fill(if *err {
                            Color32::from_rgb(0x3A, 0x1A, 0x1C)
                        } else {
                            theme::BG_RAISED
                        })
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            if *err { theme::DANGER } else { theme::BORDER },
                        ))
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(14, 8))
                        .shadow(egui::epaint::Shadow {
                            offset: [0, 4],
                            blur: 16,
                            spread: 0,
                            color: Color32::from_black_alpha(120),
                        })
                        .show(ui, |ui| {
                            let g = if *err {
                                icon::WARNING_CIRCLE
                            } else {
                                icon::CHECK_CIRCLE
                            };
                            ui.label(RichText::new(crate::tr(format!("{g}  {msg}"))).color(
                                if *err {
                                    Color32::from_rgb(0xFF, 0xB4, 0xB4)
                                } else {
                                    theme::TEXT
                                },
                            ));
                        });
                    ui.add_space(4.0);
                }
            });
        ctx.request_repaint_after(Duration::from_millis(250));
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
        if self.screen == Screen::Home {
            self.new_project();
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

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        dev_screenshot(ctx);
        if std::env::var_os("CUTLINE_PERF").is_some() {
            self.perf_ui += 1;
            let el = self.perf_since.elapsed().as_secs_f64();
            if el >= 1.0 {
                eprintln!(
                    "perf: {:.0} ui/s, {:.0} Strombilder/s, {} Ströme geöffnet, größte Lücke {:.0} ms, playing={}",
                    self.perf_ui as f64 / el,
                    self.perf_frames as f64 / el,
                    self.perf_opens,
                    self.perf_max_gap * 1000.0,
                    self.playing
                );
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
        self.sync_project();
        self.handle_dropped_files(ctx);
        self.receive_decoded(ctx);
        self.poll_tasks();
        self.poll_epidemic(ctx);
        self.poll_stock();
        if self.screen == Screen::Editor {
            self.ensure_proxies(ctx);
        }

        self.window_frame(ctx);
        egui::TopBottomPanel::top("top")
            .exact_height(theme::TITLE_H)
            .frame(
                egui::Frame::default()
                    .fill(theme::BG_APP)
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 0,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show(ctx, |ui| self.title_bar(ui));

        if self.screen == Screen::Home {
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(theme::BG_APP))
                .show(ctx, |ui| self.home_page(ui));
            self.settings_window(ctx);
            self.import_dialogs(ctx);
            self.template_dialog(ctx);
            self.external_preview(ctx);
            self.onboarding(ctx);
            self.draw_toasts(ctx);
            return;
        }

        if !self.export.is_open() && self.onboard.is_none() && self.tour.is_none() {
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
        if self.playing {
            ctx.request_repaint();
        }

        if !self.player_max {
            // Aufteilung relativ zur Fenstergröße: der Player behält immer genug Platz
            let sr = ctx.screen_rect();
            let r = egui::TopBottomPanel::bottom("timeline")
                .resizable(true)
                .default_height((sr.height() * 0.38).clamp(170.0, 420.0))
                .min_height(130.0)
                .max_height((sr.height() * 0.55).max(140.0))
                .frame(theme::card(6))
                .show(ctx, |ui| self.timeline_panel(ui))
                .response
                .rect;
            self.tour_rects.insert("timeline", r);

            let r = egui::SidePanel::left("assets")
                .resizable(true)
                .default_width((sr.width() * 0.27).clamp(280.0, 520.0))
                .min_width(250.0)
                .max_width((sr.width() * 0.34).max(260.0))
                .frame(theme::card(0))
                .show(ctx, |ui| fixed_width(ui, |ui| self.assets_panel(ui)))
                .response
                .rect;
            self.tour_rects.insert("assets", r);

            let r = egui::SidePanel::right("inspector")
                .resizable(true)
                .default_width((sr.width() * 0.22).clamp(250.0, 380.0))
                .min_width(230.0)
                .max_width((sr.width() * 0.3).max(240.0))
                .frame(theme::card(10))
                .show(ctx, |ui| fixed_width(ui, |ui| self.inspector_panel(ui)))
                .response
                .rect;
            self.tour_rects.insert("inspector", r);
        }

        let r = egui::CentralPanel::default()
            .frame(theme::card(8))
            .show(ctx, |ui| self.player(ui))
            .response
            .rect;
        self.tour_rects.insert("player", r);

        self.voiceover_window(ctx);
        self.settings_window(ctx);
        let fonts_owned = self.font_files();
        let tex = self
            .frames
            .values()
            .next()
            .map(|(_, t)| (t.id(), t.size_vec2()));
        if let Some(msg) = self.export.show(ctx, &self.project, &fonts_owned, tex) {
            // Erfolgreicher Export mit Epidemic-Musik wird gemeldet (Nutzungsbedingungen)
            if msg.contains("Export fertig") {
                self.report_epidemic_export(ctx);
            }
            self.toast(msg);
        }
        if let Some(id) = self.export.take_picked_platform() {
            if self.project.settings.platform.as_deref() != Some(id) {
                let mut s = self.project.settings.clone();
                s.platform = Some(id.to_string());
                self.apply(EditOp::SetSettings { settings: s });
            }
        }
        self.lib_dialogs(ctx);
        self.preset_dialogs(ctx);
        self.import_dialogs(ctx);
        self.marker_dialog(ctx);
        self.track_rename_dialog(ctx);
        self.project_tools(ctx);
        self.paste_attrs_dialog(ctx);
        self.template_dialog(ctx);
        self.external_preview(ctx);
        self.onboarding(ctx);
        self.tour_overlay(ctx);
        self.drag_ghost(ctx);
        self.draw_toasts(ctx);
        self.sync_project();
        self.autosave();
    }

    fn on_exit(&mut self) {
        self.flush_before_leaving();
    }
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
                    .fill(theme::BG_RAISED)
                    .stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .shadow(egui::Shadow {
                        offset: [0, 4],
                        blur: 12,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(120),
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(glyph).size(16.0).color(theme::ACCENT));
                            let mut l = label;
                            if l.chars().count() > 28 {
                                l = format!("{}…", l.chars().take(27).collect::<String>());
                            }
                            if !l.is_empty() {
                                ui.label(egui::RichText::new(l).size(11.5).color(theme::TEXT));
                            }
                        });
                    });
            });
    }
}

/// Für Dokumentation: Mit `CUTLINE_SHOTS=<Ordner>` speichert Cutline das Fensterbild als PNG,
/// sobald dort eine Datei `take` liegt (direkt aus der App, ohne überlagernde Fenster).
fn dev_screenshot(ctx: &egui::Context) {
    let Some(dir) = std::env::var_os("CUTLINE_SHOTS").filter(|d| !d.is_empty()) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
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
