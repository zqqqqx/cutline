//! Export dialog: without Pro lock, without „Urheberrecht überprüfen“, with real progress.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cl_core::{Project, Time};
use cl_export::{Codec, Encoder, ExportSettings, Format, Quality};
use eframe::egui::{self, vec2, Color32, Rect, RichText};

use crate::theme::{self, icon};

/// Target platform: sets project format (size, frame rate) and export (codec, quality).
pub struct Platform {
    pub id: &'static str,
    pub name: &'static str,
    pub w: u32,
    pub h: u32,
    pub fps: Option<u32>,
    pub codec: Codec,
    pub quality: Quality,
}

pub const PLATFORMS: &[Platform] = &[
    Platform {
        id: "youtube-1080",
        name: "YouTube 1080p",
        w: 1920,
        h: 1080,
        fps: None,
        codec: Codec::H264,
        quality: Quality::Higher,
    },
    Platform {
        id: "youtube-1440",
        name: "YouTube 1440p",
        w: 2560,
        h: 1440,
        fps: None,
        codec: Codec::H264,
        quality: Quality::Higher,
    },
    Platform {
        id: "youtube-4k",
        name: "YouTube 4K",
        w: 3840,
        h: 2160,
        fps: None,
        codec: Codec::Hevc,
        quality: Quality::Higher,
    },
    Platform {
        id: "shorts",
        name: "Shorts / TikTok / Reels (1080×1920)",
        w: 1080,
        h: 1920,
        fps: Some(30),
        codec: Codec::H264,
        quality: Quality::Higher,
    },
    Platform {
        id: "instagram",
        name: "Instagram-Beitrag (1080×1350)",
        w: 1080,
        h: 1350,
        fps: Some(30),
        codec: Codec::H264,
        quality: Quality::Higher,
    },
    Platform {
        id: "twitch",
        name: "Twitch-Clip (1080p60)",
        w: 1920,
        h: 1080,
        fps: Some(60),
        codec: Codec::H264,
        quality: Quality::Recommended,
    },
    Platform {
        id: "small",
        name: "Kleine Datei zum Teilen (720p)",
        w: 1280,
        h: 720,
        fps: Some(30),
        codec: Codec::H264,
        quality: Quality::Lower,
    },
];

pub fn platform(id: &str) -> Option<&'static Platform> {
    PLATFORMS.iter().find(|p| p.id == id)
}

impl Platform {
    /// Project format for this platform (size, aspect ratio, possibly frame rate).
    pub fn apply_to(&self, s: &mut cl_core::ProjectSettings) {
        s.width = self.w;
        s.height = self.h;
        let g = gcd(self.w, self.h);
        s.aspect = cl_core::AspectPreset::Ratio(self.w / g, self.h / g);
        if let Some(f) = self.fps {
            s.fps = cl_core::Rational::new(f, 1);
        }
        s.platform = Some(self.id.to_string());
    }
    /// Does the project's aspect ratio fit this platform?
    pub fn fits(&self, s: &cl_core::ProjectSettings) -> bool {
        let (a, b) = (
            s.width.max(1) as f32 / s.height.max(1) as f32,
            self.w as f32 / self.h as f32,
        );
        (a - b).abs() < 0.02
    }
}

/// YouTube guide values for SDR uploads (kbit/s) by shorter frame edge: up to 30 fps / above.
pub fn youtube_kbps(short_edge: u32, fps: f64) -> Option<u32> {
    let (lo, hi) = match short_edge {
        1080 => (8_000, 12_000),
        1440 => (16_000, 24_000),
        2160 => (40_000, 60_000),
        _ => return None,
    };
    Some(if fps > 30.5 { hi } else { lo })
}

/// Choice „Seitenverhältnis“ in the export (after „Projekt“)
pub const ASPECTS: [(u32, u32); 4] = [(16, 9), (9, 16), (1, 1), (4, 5)];

/// Aspect ratio of a platform as a choice in the export: 0 (project) if it fits the project
fn platform_aspect(pl: &Platform, s: &cl_core::ProjectSettings) -> usize {
    let g = gcd(pl.w, pl.h);
    if pl.fits(s) {
        0
    } else {
        ASPECTS.iter().position(|a| *a == (pl.w / g, pl.h / g)).map_or(0, |i| i + 1)
    }
}

pub fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a.max(1)
    } else {
        gcd(b, a % b)
    }
}

/// Maximum number of waiting exports. A snapshot is a full `Project` copy without media, measured
/// ≈1.1 KB per clip (see `cl_core::Document`): 3000 clips ≈3.3 MB, 20 snapshots ≈66 MB.
pub const QUEUE_MAX: usize = 20;

/// Export job: snapshot of the project at queueing, so that further editing or a
/// project change no longer alters it.
struct Queued {
    project: Arc<Project>,
    /// Project file (assignment in the exports list)
    project_path: Option<PathBuf>,
    /// Settings at queueing (fingerprint for „Erneut exportieren“)
    settings: ExportSettings,
    /// Target file; with „In Teile aufteilen“ the folder of the parts (one group, one entry)
    out: PathBuf,
    /// exported one after another; a normal export has exactly one part with `out`
    parts: Vec<Part>,
    fonts: HashMap<String, (PathBuf, u32)>,
    /// Project state at queueing
    rev: u64,
    /// Overwriting a group: remove older parts only right before the first part (a still
    /// running or waiting export of the same group would otherwise write them again)
    clean: bool,
}

/// One part of an export: range, target file and side files (subtitles, chapters) with text
/// from queueing; these are written only after the part succeeded.
struct Part {
    range: Option<(Time, Time)>,
    out: PathBuf,
    sidecars: Vec<(PathBuf, String)>,
}

impl Queued {
    /// Parts in their own folder („In Teile aufteilen“)
    fn group(&self) -> bool {
        self.parts.first().is_none_or(|x| x.out != self.out)
    }
}

struct Running {
    progress: Arc<Mutex<f32>>,
    /// running part (from 0) and finished parts
    part: Arc<std::sync::atomic::AtomicUsize>,
    finished: Arc<Mutex<Vec<usize>>>,
    /// known only after the job is built in the background (until then „Wird vorbereitet …“):
    /// whether loudness is measured beforehand (first part of the progress), FFmpeg's preview file
    job: Arc<Mutex<Option<(bool, Option<PathBuf>)>>>,
    result: Arc<Mutex<Option<Result<(), String>>>>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    src: Queued,
    /// loaded live frames; textures are freed with `Running`
    live: Live,
}

impl Running {
    /// Build and run the job in the background; reads only the snapshot, never the open project.
    fn start(ctx: &egui::Context, src: Queued) -> Self {
        let progress = Arc::new(Mutex::new(0.0));
        let job_info = Arc::new(Mutex::new(None));
        let result = Arc::new(Mutex::new(None));
        let cancel = Arc::new(AtomicBool::new(false));
        let (part, finished) = (Arc::new(std::sync::atomic::AtomicUsize::new(0)), Arc::new(Mutex::new(Vec::new())));
        let (pr, ji, rs, cc, c2) = (progress.clone(), job_info.clone(), result.clone(), cancel.clone(), ctx.clone());
        let (pi, fin) = (part.clone(), finished.clone());
        let (p, s2, fonts) = (src.project.clone(), src.settings.clone(), src.fonts.clone());
        let parts: Vec<(Option<(Time, Time)>, PathBuf)> = src.parts.iter().map(|x| (x.range, x.out.clone())).collect();
        let folder = src.group().then(|| src.out.clone());
        let clean = src.clean.then(|| (src.project.name.clone(), src.project_path.clone(), src.settings.format.extension()));
        // still on the UI thread: an aborted predecessor whose thread is still running sees
        // the folder as taken from now on and no longer deletes it
        if let Some(d) = &folder {
            folder_use(d, true, false);
        }
        let resolver = move |fam: Option<&str>| fam.and_then(|f| fonts.get(f).cloned());
        // building the job (seconds for large projects: audio check per file) not on the UI thread
        std::thread::spawn(move || {
            // parts one after another; a failed one does not stop the rest, cancel stops all
            let len = |r: &Option<(Time, Time)>| r.map_or(p.duration(), |(a, b)| b - a).as_secs_f64().max(1e-3);
            let total: f64 = parts.iter().map(|x| len(&x.0)).sum();
            let (mut at, mut first_err) = (0.0, None);
            // after an abort only remove a folder created by this export
            let created = folder.as_ref().is_some_and(|d| !d.exists());
            if let Some(d) = &folder {
                let _ = std::fs::create_dir_all(d);
            }
            for (i, (range, out)) in parts.iter().enumerate() {
                if cc.load(Ordering::Relaxed) {
                    first_err = Some(cl_export::ExportError::Cancelled.to_string());
                    break;
                }
                if let (0, Some(d), Some((name, path, ext))) = (i, &folder, &clean) {
                    let recorded: Vec<PathBuf> = crate::exports::for_project(name, path.as_deref()).into_iter().map(|e| e.path).collect();
                    let keep: Vec<PathBuf> = parts.iter().map(|x| x.1.clone()).collect();
                    for f in stale_parts(d, ext, &keep, &recorded) {
                        let _ = std::fs::remove_file(f);
                    }
                }
                pi.store(i, Ordering::Relaxed);
                let s = ExportSettings { range: *range, ..s2.clone() };
                let w = (len(range) / total) as f32;
                let r = cl_export::build(&p, &s, &resolver, out).and_then(|job| {
                    if cc.load(Ordering::Relaxed) {
                        return Err(cl_export::ExportError::Cancelled);
                    }
                    *ji.lock().unwrap() = Some((job.measures_loudness(), job.preview_path().map(Path::to_path_buf)));
                    c2.request_repaint();
                    let r = cl_export::run(&job, cc.clone(), |x| {
                        *pr.lock().unwrap() = at + x * w;
                        c2.request_repaint();
                    });
                    if let Err(e) = &r { log::error!(target: "export", "{} · Befehl: {}", crate::applog::anon(&e.to_string()), crate::applog::ffmpeg_cmd(e.failed_args().unwrap_or(&job.args))); }
                    r
                });
                match r {
                    Ok(()) => fin.lock().unwrap().push(i),
                    Err(e) => {
                        let stop = matches!(e, cl_export::ExportError::Cancelled);
                        first_err.get_or_insert(e.to_string());
                        if stop {
                            break;
                        }
                    }
                }
                at += w;
            }
            // aborted before a part was finished: no empty folder (not empty or taken by a
            // newer export: stays)
            if let Some(d) = &folder {
                folder_use(d, false, created);
            }
            *rs.lock().unwrap() = Some(first_err.map_or(Ok(()), Err));
            c2.request_repaint();
        });
        Running { progress, part, finished, job: job_info, result, cancel, started: Instant::now(), src, live: Live::default() }
    }

    /// Group: running part and number of parts (from 1)
    fn part_of(&self) -> Option<(usize, usize)> {
        self.src.group().then(|| (self.part.load(Ordering::Relaxed) + 1, self.src.parts.len()))
    }
}

/// Folders of part exports that are running right now (count per folder). After an abort during
/// preparation `poll` reports at once, but the thread cleans up only afterwards; if meanwhile
/// a new export starts in the same folder, this cleanup must not delete it.
static FOLDERS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Take (`true`) or release a folder; on the last release it is deleted if it is empty
/// and the releaser created it (`created`; never a folder that already existed).
fn folder_use(dir: &Path, take: bool, created: bool) {
    let mut v = FOLDERS.lock().unwrap_or_else(|e| e.into_inner());
    if take {
        v.push(dir.to_path_buf());
    } else if let Some(i) = v.iter().position(|d| d == dir) {
        v.swap_remove(i);
        if created && !v.iter().any(|d| d == dir) {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// Modification time and size of the preview file
type Sig = (std::time::SystemTime, u64);

/// Live preview of the running export: current frame (since when) and the previous one for cross-fading.
#[derive(Default)]
struct Live {
    sig: Option<Sig>,
    checked: Option<Instant>,
    cur: Option<(egui::TextureHandle, Instant)>,
    prev: Option<egui::TextureHandle>,
}

impl Live {
    /// Check at most every 400 ms; only a changed, fully readable file replaces the frame.
    fn poll(&mut self, ctx: &egui::Context, path: &Path) {
        if self.checked.is_some_and(|t| t.elapsed().as_millis() < 400) {
            return;
        }
        self.checked = Some(Instant::now());
        let sig = std::fs::metadata(path).ok().and_then(|m| Some((m.modified().ok()?, m.len())));
        if !changed(self.sig, sig) {
            return;
        }
        // half written: skip silently, the last good frame stays
        let Some(img) = std::fs::read(path).ok().and_then(|b| decode_jpeg(&b)) else {
            return;
        };
        self.sig = sig;
        let tex = ctx.load_texture("export-live", img, egui::TextureOptions::LINEAR);
        self.prev = self.cur.take().map(|c| c.0);
        self.cur = Some((tex, Instant::now()));
    }
}

/// Reload? Only if the file is there and changed since the last load.
fn changed(old: Option<Sig>, new: Option<Sig>) -> bool {
    new.is_some() && new != old
}

/// Share of the new frame when cross-fading (`dur` 0 = animations off: fully at once).
fn blend(age: f32, dur: f32) -> f32 {
    if dur <= 0.0 {
        1.0
    } else {
        (age / dur).clamp(0.0, 1.0)
    }
}

/// Decode only complete JPEGs (end marker FF D9); FFmpeg rewrites the file continuously.
fn decode_jpeg(b: &[u8]) -> Option<egui::ColorImage> {
    if !b.ends_with(&[0xFF, 0xD9]) {
        return None;
    }
    let img = image::load_from_memory_with_format(b, image::ImageFormat::Jpeg).ok()?.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

/// Draws the live frame fitted into `rect` (smoothly cross-faded, thin progress line at the
/// bottom edge). `false`: no frame yet.
fn paint_live(ui: &egui::Ui, rect: Rect, live: &Live, prog: f32) -> bool {
    let Some((cur, at)) = &live.cur else {
        return false;
    };
    let ts = cur.size_vec2();
    let fit = (rect.width() / ts.x).min(rect.height() / ts.y);
    let img = Rect::from_center_size(rect.center(), ts * fit);
    let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let p = ui.painter();
    let k = theme::ease(blend(at.elapsed().as_secs_f32(), theme::anim(theme::MOTION_SLOW)));
    if let Some(prev) = live.prev.as_ref().filter(|_| k < 1.0) {
        p.image(prev.id(), img, uv, Color32::WHITE);
    }
    p.image(cur.id(), img, uv, Color32::WHITE.gamma_multiply(k));
    if k < 1.0 {
        ui.ctx().request_repaint();
    }
    // the same smoothly trailing value as the progress line below
    let line = Rect::from_min_size(egui::pos2(img.left(), img.bottom() - 2.0), vec2(img.width(), 2.0));
    crate::widgets::progress_line(p, egui::Id::new("export-live"), line, prog);
    true
}

/// Running analysis „Person folgen“ (background thread): progress 0…1, cancel, result
/// (tracks, first error)
struct FocusJob {
    progress: Arc<Mutex<f32>>,
    cancel: Arc<AtomicBool>,
    done: Arc<Mutex<Option<FocusFound>>>,
}

type FocusFound = (Vec<(crate::shorts::FocusKey, Vec<(f64, f32, f32)>)>, Option<String>);

impl FocusJob {
    fn start(ctx: &egui::Context, jobs: Vec<(crate::shorts::FocusKey, cl_core::MediaRef)>) -> Self {
        let job = FocusJob { progress: Default::default(), cancel: Default::default(), done: Default::default() };
        let (progress, cancel, done, ctx) = (job.progress.clone(), job.cancel.clone(), job.done.clone(), ctx.clone());
        std::thread::spawn(move || {
            let n = jobs.len() as f32;
            let (mut found, mut err) = (Vec::new(), None);
            for (i, (key, m)) in jobs.into_iter().enumerate() {
                let sub = |v: f32| *progress.lock().unwrap() = (i as f32 + v) / n;
                let r = cl_analyze::reframe::focus_path(&m.path, m.width, m.height, key.2.as_secs_f64(), key.3.as_secs_f64(), &sub, &cancel);
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                // error: clip stays centred, no new attempt until the next change
                found.push((key, r.unwrap_or_else(|e| {
                    err.get_or_insert(e);
                    Vec::new()
                })));
            }
            *done.lock().unwrap() = Some((found, err));
            ctx.request_repaint();
        });
        job
    }
}

#[derive(Default)]
pub struct ExportDialog {
    open: bool,
    name: String,
    folder: PathBuf,
    /// Project file the dialog was opened for (for the exports list)
    project_path: Option<PathBuf>,
    settings: Option<ExportSettings>,
    /// Shorter frame edge (720/1080/1440/2160) or 0 = project
    short_edge: u32,
    custom_kbps: u32,
    /// In/out range of the timeline, if set
    range_avail: Option<(Time, Time)>,
    /// „Seitenverhältnis“: 0 = project, otherwise entry `aspect - 1` of [`ASPECTS`]
    aspect: usize,
    /// „Bild“ with another aspect ratio: 0 Füllen, 1 Person folgen, 2 Einpassen (only in memory)
    fit: usize,
    /// rebuilt snapshot (project state, format, picture, state of the tracks) for preview and export
    pub(crate) framed: Option<((u64, (u32, u32), usize, u64), Arc<Project>)>,
    /// „Person folgen“: track per analysis key (empty: no person or error → centred);
    /// counter rises with each result
    focus: HashMap<crate::shorts::FocusKey, Vec<(f64, f32, f32)>>,
    focus_gen: u64,
    focus_job: Option<FocusJob>,
    /// Chosen platform (remembered in the project if the format fits)
    platform: Option<&'static str>,
    picked_platform: Option<&'static str>,
    also_srt: bool,
    /// Write markers as chapters (Settings → Projects → Export); set by the app each frame
    pub chapters: bool,
    /// Show the hint about chapters under 10 s; „Nicht mehr anzeigen“ reports `chapter_hint_off`
    pub chapter_hint: bool,
    pub chapter_hint_off: bool,
    encoders: Arc<Mutex<Option<Vec<Encoder>>>>,
    /// Encoder chosen by hand in this dialog (otherwise preselection by the settings)
    encoder_picked: bool,
    running: Option<Running>,
    /// waiting exports, the front one starts after the running one.
    /// shortcut: in memory only, does not survive a restart; save it only once that is wanted
    queue: std::collections::VecDeque<Queued>,
    last_output: Option<PathBuf>,
    error: Option<String>,
    /// Effects/filters the catalog does not know (dropped in the export)
    missing: Vec<String>,
    /// Time and duration of the last successful export (short success animation)
    done: Option<(Instant, f32)>,
    /// Project state when the dialog was opened
    rev: u64,
    /// Project state, file and settings of the last finished export („Zur Datei“ instead of „Exportieren“)
    exported: Option<(u64, PathBuf, ExportSettings)>,
    /// Target file exists: ask before starting
    confirm_overwrite: bool,
    /// Question opened only in this frame: do not evaluate Enter/Esc yet
    /// (the Enter that triggered „Exportieren“ is still pressed in the same frame)
    confirm_fresh: bool,
    /// Window should close while an export is running: question open
    quit_ask: bool,
    /// Quitting despite a running export confirmed
    quit_ok: bool,
    /// largest height of the settings shown so far (the dialog does not jump on a format change)
    right_h: f32,
    /// right column in the last frame: visible height, content height (layout test)
    right_view: (f32, f32),
    /// „In Teile aufteilen“: on/off, kind (0 count, 1 length, 2 markers), number, seconds per part
    split_on: bool,
    split_by: usize,
    split_count: u32,
    split_secs: u32,
    /// „Dateigröße: Eigene …“ chosen; value in MB
    size_custom: bool,
    custom_mb: u32,
}

/// Height of the export window without the settings on the right (title, messages, progress,
/// button row, margins): up to the program window height minus this value the right column shows everything,
/// above that it scrolls
const DIALOG_CHROME_H: f32 = 250.0;

/// Name of an encoder (export dialog, Settings → Performance)
pub fn encoder_name(e: Encoder) -> &'static str {
    match e {
        Encoder::Software => "Software (beste Qualität)",
        Encoder::Nvenc => "NVIDIA NVENC (schnell)",
        Encoder::Qsv => "Intel Quick Sync (schnell)",
        Encoder::Amf => "AMD AMF (schnell)",
        Encoder::Mf => "Windows (Media Foundation)",
    }
}

/// May the window close? During an export only after confirmation.
fn close_allowed(running: bool, confirmed: bool) -> bool {
    !running || confirmed
}

impl ExportDialog {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Window close button during an export: hold off closing and ask; „Beenden“ cancels
    /// the export and closes (FFmpeg cleans up in [`Self::cancel_and_wait`]).
    pub fn guard_close(&mut self, ctx: &egui::Context) {
        let pending = self.running.iter().count() + self.queue.len();
        if ctx.input(|i| i.viewport().close_requested()) && !close_allowed(pending > 0, self.quit_ok) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.quit_ask = true;
        }
        if !self.quit_ask {
            return;
        }
        let (mut quit, mut stay) = (false, false);
        let m = crate::dialog::modal(ctx, "export-quit").show(ctx, |ui| {
            ui.set_width(340.0);
            let t = theme::count_text(pending, "Export läuft – wirklich beenden?", "{} Exporte ausstehend – wirklich beenden?");
            stay |= crate::dialog::title(ui, &t);
            let (q, c) = crate::dialog::buttons(ui, theme::danger("Cutline beenden"), true);
            quit = q;
            stay |= c;
        });
        if quit {
            self.quit_ok = true;
            self.quit_ask = false;
            self.queue.clear();
            if let Some(r) = &self.running {
                r.cancel.store(true, Ordering::Relaxed);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if stay || m.should_close() {
            self.quit_ask = false;
        }
    }

    /// Cancel the running export and wait at most `max` until FFmpeg has ended and the
    /// `.part` file is deleted (when quitting the program).
    pub fn cancel_and_wait(&mut self, max: std::time::Duration) {
        self.queue.clear();
        let Some(r) = &self.running else { return };
        r.cancel.store(true, Ordering::Relaxed);
        let t0 = Instant::now();
        while r.result.lock().unwrap().is_none() && t0.elapsed() < max {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }

    /// Open the dialog; `range` = in/out range of the timeline (offered and preselected), regardless of
    /// which way (button, menu, shortcut) the export is started.
    pub fn open_for(&mut self, p: &Project, project_path: Option<&Path>, rev: u64, range: Option<(Time, Time)>) {
        self.rev = rev;
        // set up anew even during an export: the open project can then be queued
        self.open = true;
        self.range_avail = range.filter(|(a, b)| b > a);
        self.error = None;
        self.missing = cl_fx::catalog::missing(p).into_iter().map(|m| cl_fx::fx_name(&m.0)).collect();
        self.missing.sort();
        self.missing.dedup();
        self.last_output = None;
        self.name = p.name.clone();
        self.project_path = project_path.map(Path::to_path_buf);
        if self.folder.as_os_str().is_empty() {
            self.folder = project_path
                .and_then(|pp| pp.parent().map(Path::to_path_buf))
                .or_else(|| {
                    std::env::var_os("USERPROFILE")
                        .or_else(|| std::env::var_os("HOME"))
                        .map(|h| PathBuf::from(h).join("Videos"))
                })
                .unwrap_or_else(|| PathBuf::from("."));
        }
        let mut s = ExportSettings::from_project(p);
        s.range = self.range_avail;
        if let Some(old) = &self.settings {
            s.format = old.format;
            s.codec = old.codec;
            s.quality = old.quality;
            s.include_audio = old.include_audio;
            s.target_mb = old.target_mb;
        }
        // a choice made in the dialog applies only to this export
        self.encoder_picked = false;
        self.short_edge = 0;
        (self.aspect, self.framed) = (0, None);
        self.platform = None;
        if let Some(pl) = p.settings.platform.as_deref().and_then(platform) {
            Self::apply_platform(&mut s, &mut self.short_edge, &mut self.custom_kbps, pl);
            self.platform = Some(pl.id);
            self.aspect = platform_aspect(pl, &p.settings);
        }
        self.settings = Some(s);
        if self.custom_kbps == 0 {
            self.custom_kbps = 12000;
        }
        if self.split_count == 0 {
            (self.split_count, self.split_secs, self.custom_mb) = (2, 60, 200);
        }
        // check hardware encoders once in the background
        if self.encoders.lock().unwrap().is_none() {
            let slot = self.encoders.clone();
            *slot.lock().unwrap() = Some(vec![Encoder::Software]);
            std::thread::spawn(move || {
                let found = cl_export::available_encoders();
                *slot.lock().unwrap() = Some(found);
            });
        }
    }

    fn apply_platform(s: &mut ExportSettings, short_edge: &mut u32, custom_kbps: &mut u32, pl: &Platform) {
        s.format = Format::Mp4;
        s.codec = pl.codec;
        s.quality = pl.quality;
        *short_edge = pl.w.min(pl.h);
        if let Some(f) = pl.fps {
            s.fps = cl_core::Rational::new(f, 1);
        }
        // YouTube: official bitrates, AAC 384 kbit/s, loudness −14 LUFS
        let yt = pl.id.starts_with("youtube");
        s.loudness = yt.then_some(-14.0);
        s.audio_kbps = yt.then_some(384);
        if let Some(k) = youtube_kbps(*short_edge, s.fps.as_f64()).filter(|_| yt) {
            s.quality = Quality::Bitrate(k);
            *custom_kbps = k;
        }
    }

    /// Choose a platform preset: export values and the platform's aspect ratio if the
    /// project has another one (otherwise „Projekt“; only then the project remembers the platform)
    fn pick_platform(&mut self, s: &mut ExportSettings, p: &Project, pl: &'static Platform) {
        Self::apply_platform(s, &mut self.short_edge, &mut self.custom_kbps, pl);
        self.platform = Some(pl.id);
        self.aspect = platform_aspect(pl, &p.settings);
        if pl.fits(&p.settings) {
            self.picked_platform = Some(pl.id);
        }
    }

    /// Format of the export with an aspect ratio other than the project's (shorter edge stays),
    /// otherwise `None`
    fn canvas(&self, p: &Project) -> Option<(u32, u32)> {
        let (a, b) = *ASPECTS.get(self.aspect.checked_sub(1)?)?;
        let mut ps = p.settings.clone();
        ps.apply_aspect(cl_core::AspectPreset::Ratio(a, b));
        let r = |w: u32, h: u32| w as f32 / h.max(1) as f32;
        ((r(ps.width, ps.height) - r(p.settings.width, p.settings.height)).abs() >= 0.02).then_some((ps.width, ps.height))
    }

    /// Snapshot in the chosen aspect ratio (`None`: project format); remembered until the next
    /// change of project or choice
    fn framed(&mut self, p: &Project, fonts: &HashMap<String, (PathBuf, u32)>) -> Option<Arc<Project>> {
        let c = self.canvas(p)?;
        let key = (self.rev, c, self.fit, self.focus_gen);
        if self.framed.as_ref().is_none_or(|f| f.0 != key) {
            let m = crate::shorts::text_measure(fonts);
            let focus: crate::shorts::FocusPaths = match self.fit {
                1 => crate::shorts::follow_jobs(p).into_iter().filter_map(|(k, _)| Some((k.0, self.focus.get(&k)?.clone()))).collect(),
                _ => Default::default(),
            };
            self.framed = Some((key, Arc::new(crate::shorts::reframe(p, c, self.fit != 2, &focus, &m))));
        }
        self.framed.as_ref().map(|f| f.1.clone())
    }

    /// „Person folgen“: take over a finished analysis (error → one message; cancel → „Füllen“),
    /// otherwise compute missing tracks in the background
    fn follow(&mut self, ctx: &egui::Context, p: &Project) -> Option<String> {
        let mut toast = None;
        if let Some(j) = &self.focus_job {
            let (found, err) = j.done.lock().unwrap().take()?;
            let cancelled = j.cancel.load(Ordering::Relaxed);
            self.focus_job = None;
            if cancelled {
                self.fit = 0;
                return None;
            }
            self.focus.extend(found);
            self.focus_gen += 1;
            toast = err.map(|e| format!("{}: {e}", crate::tr("Auto-Reframe")));
        }
        // look for missing tracks only after a change (project, format, choice, result)
        let Some(c) = self.canvas(p).filter(|_| self.fit == 1) else { return toast };
        if self.framed.as_ref().is_some_and(|f| f.0 == (self.rev, c, self.fit, self.focus_gen)) {
            return toast;
        }
        let jobs: Vec<_> = crate::shorts::follow_jobs(p).into_iter().filter(|j| !self.focus.contains_key(&j.0)).collect();
        if !jobs.is_empty() {
            self.focus_job = Some(FocusJob::start(ctx, jobs));
        }
        toast
    }

    /// Platform chosen in the export (collected once) so the project remembers it.
    pub fn take_picked_platform(&mut self) -> Option<&'static str> {
        self.picked_platform.take()
    }

    /// Project, target file and export settings as at the last finished export: „Zur Datei“ and
    /// „Erneut exportieren“ (overwrites without asking) instead of „Exportieren“
    fn unchanged_since_export(&self, s: &ExportSettings) -> bool {
        self.split(s).is_none()
            && self
                .exported
                .as_ref()
                .is_some_and(|(r, f, used)| *r == self.rev && f.is_file() && *f == self.output_path(s.format) && used == s)
    }

    fn output_path(&self, f: Format) -> PathBuf {
        self.folder.join(format!("{}.{}", file_safe(&self.name), f.extension()))
    }

    /// Chosen split (only with picture)
    fn split(&self, s: &ExportSettings) -> Option<Split> {
        (self.split_on && !s.format.audio_only()).then_some(match self.split_by {
            0 => Split::Count(self.split_count),
            1 => Split::Length(self.split_secs),
            _ => Split::Markers,
        })
    }

    /// Target: file or folder with the export name for the parts; plus the parts (range, file)
    fn targets(&self, p: &Project, s: &ExportSettings) -> (PathBuf, Vec<(Option<(Time, Time)>, PathBuf)>) {
        let Some(how) = self.split(s) else {
            let out = self.output_path(s.format);
            return (out.clone(), vec![(s.range, out)]);
        };
        // empty name, "." or ".." (Windows strips trailing dots): otherwise the parts folder would be
        // the chosen folder itself or its parent
        let name = Some(file_safe(&self.name)).filter(|n| !n.trim_end_matches(['.', ' ']).is_empty()).unwrap_or_else(|| "Export".into());
        let dir = self.folder.join(&name);
        let range = s.range.unwrap_or((Time::ZERO, p.duration()));
        let parts = split_parts(how, range, p.settings.fps, &p.timeline.markers, &name, &crate::tr("Teil"), s.format.extension());
        let parts = parts.into_iter().map(|(r, f)| (Some(r), dir.join(f))).collect();
        (dir, parts)
    }

    /// Longest part (seconds) and number of parts (1 without splitting); the target size applies per part
    fn longest_part(&self, p: &Project, s: &ExportSettings) -> (f64, usize) {
        let (_, parts) = self.targets(p, s);
        let len = |r: &Option<(Time, Time)>| r.map_or(p.duration(), |(a, b)| b - a).as_secs_f64();
        (parts.iter().map(|x| len(&x.0)).fold(0.0, f64::max), parts.len())
    }

    /// „Dateigröße“: automatic, fixed target sizes or custom; greyed out without a suitable codec.
    /// Below it the hint if the picture is downscaled for it.
    fn size_rows(&mut self, ui: &mut egui::Ui, p: &Project, s: &mut ExportSettings) {
        let can = s.target_possible();
        if !can {
            s.target_mb = None;
        }
        ui.add_enabled_ui(can, |ui| {
            theme::row(ui, "Dateigröße", |ui| {
                let label = match s.target_mb {
                    None => "Automatisch".to_string(),
                    Some(_) if self.size_custom => "Eigene …".into(),
                    Some(m) => format!("Höchstens {m} MB"),
                };
                egui::ComboBox::from_id_salt("exp-size").selected_text(crate::tr(label)).width(200.0).show_ui(ui, |ui| {
                    if ui.selectable_label(s.target_mb.is_none(), crate::tr("Automatisch")).clicked() {
                        (s.target_mb, self.size_custom) = (None, false);
                    }
                    for m in [10, 25, 50, 100] {
                        if ui.selectable_label(s.target_mb == Some(m) && !self.size_custom, crate::tr(format!("Höchstens {m} MB"))).clicked() {
                            (s.target_mb, self.size_custom) = (Some(m), false);
                        }
                    }
                    if ui.selectable_label(s.target_mb.is_some() && self.size_custom, crate::tr("Eigene …")).clicked() {
                        (s.target_mb, self.size_custom) = (Some(self.custom_mb.max(1)), true);
                    }
                });
            });
        });
        let Some(_) = s.target_mb else { return };
        if self.size_custom {
            theme::row(ui, "", |ui| ui.add(egui::DragValue::new(&mut self.custom_mb).range(1..=100_000).suffix(" MB")));
            s.target_mb = Some(self.custom_mb.max(1));
        }
        // never downscale silently (with parts the longest applies)
        let t = s.for_target(self.longest_part(p, s).0, audible(p, s));
        if let Quality::Bitrate(k) = t.quality {
            if let Some((e, enough)) = cl_export::target_edge(k, (s.width, s.height), s.fps.as_f64()) {
                let h = if enough { format!("Wird in {e}p exportiert") } else { format!("Wird in {e}p exportiert – auch dafür ist die Größe knapp") };
                ui.label(RichText::new(crate::tr(h)).small().color(theme::WARN_TEXT()));
            }
        }
    }

    /// „In Teile aufteilen“ with kind, number or length and result line (section „Video“)
    fn split_rows(&mut self, ui: &mut egui::Ui, p: &Project, s: &ExportSettings) {
        theme::row(ui, "", |ui| crate::widgets::check(ui, &mut self.split_on, crate::tr("In Teile aufteilen")));
        let Some(how) = self.split(s) else { return };
        let range = s.range.unwrap_or((Time::ZERO, p.duration()));
        let parts = |how| split_parts(how, range, p.settings.fps, &p.timeline.markers, "", "", "");
        // „Marker“ only with at least one marker in the range (otherwise one part would remain)
        let marks = parts(Split::Markers).len() > 1;
        if !marks && how == Split::Markers {
            self.split_by = 0;
        }
        theme::row(ui, "Aufteilen nach", |ui| {
            ui.allocate_ui(vec2(200.0, 24.0), |ui| crate::widgets::segmented_some(ui, &["Anzahl", "Länge", "Marker"], &mut self.split_by, &[true, true, marks]));
        });
        theme::row(ui, "", |ui| {
            match self.split_by {
                0 => drop(ui.add(egui::DragValue::new(&mut self.split_count).range(2..=SPLIT_MAX))),
                1 => drop(ui.add(egui::DragValue::new(&mut self.split_secs).range(1..=36_000).suffix(" s"))),
                _ => {}
            }
            if let Some(how) = self.split(s) {
                ui.label(RichText::new(split_summary(&parts(how))).small().color(theme::TEXT_DIM()));
            }
        });
    }

    /// Target already taken: one of the files already exists, or a running or waiting
    /// export writes there (`out` = file or folder of the parts)
    fn taken(&self, out: &Path, files: &[(Option<(Time, Time)>, PathBuf)]) -> bool {
        files.iter().any(|f| f.1.exists()) || self.running.as_ref().is_some_and(|r| r.src.out == out) || self.queue.iter().any(|q| q.out == out)
    }

    /// Running export for the exports list: name (with parts „Name · Teil 3 von 6“) and progress
    pub fn current(&self) -> Option<(String, f32)> {
        self.running.as_ref().map(|r| {
            let mut name = r.src.out.file_name().unwrap_or_default().to_string_lossy().to_string();
            if let Some((i, n)) = r.part_of() {
                name = format!("{name} · {}", crate::tr(format!("Teil {i} von {n}")));
            }
            (name, *r.progress.lock().unwrap())
        })
    }

    /// Waiting exports: target file or folder, settings, number of parts (0 = without parts)
    pub fn waiting(&self) -> impl Iterator<Item = (&Path, &ExportSettings, usize)> {
        self.queue.iter().map(|q| (q.out.as_path(), &q.settings, if q.group() { q.parts.len() } else { 0 }))
    }

    pub fn cancel_current(&self) {
        if let Some(r) = &self.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Remove a waiting export (`None`: all)
    pub fn remove_waiting(&mut self, i: Option<usize>) {
        match i {
            Some(i) => drop(self.queue.remove(i)),
            None => self.queue.clear(),
        }
    }

    /// Start the front waiting export if none is running
    fn start_next(&mut self, ctx: &egui::Context) {
        if self.running.is_none() {
            if let Some(q) = self.queue.pop_front() {
                self.last_output = Some(q.out.clone());
                self.running = Some(Running::start(ctx, q));
            }
        }
    }

    /// Follow the running export (every frame, also on the start page and with the dialog
    /// closed) and then start the next waiting one. Returns a message for a toast
    /// when an export has finished, on success with the project snapshot it comes from.
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<(String, Option<Arc<Project>>)> {
        let mut toast = None;
        let finished = self.running.as_ref().map(|r| {
            let cancelled = r.cancel.load(Ordering::Relaxed);
            let mut res = r.result.lock().unwrap().take();
            // abort during preparation: do not wait for the build (it checks the sound per file
            // and knows no abort flag); the thread ends afterwards without an FFmpeg run
            if res.is_none() && cancelled && r.job.lock().unwrap().is_none() {
                res = Some(Err(cl_export::ExportError::Cancelled.to_string()));
            }
            (res, r.started.elapsed().as_secs_f32(), cancelled)
        });
        match finished {
            Some((Some(res), secs, cancelled)) => {
                let run = self.running.take().expect("läuft");
                let done = run.finished.lock().unwrap().clone();
                let src = run.src;
                let s = &src.settings;
                let res = res.map_err(|e| (!cancelled).then_some(e));
                log::info!(target: "export", "{} nach {secs:.1} s: {} · {} von {} Teilen · {:?}", match res { Ok(()) => "fertig", Err(None) => "abgebrochen", Err(Some(_)) => "Fehler" }, src.out.display(), done.len(), src.parts.len(), (s.width, s.height, s.fps, s.format, s.codec, s.quality, s.encoder, s.include_audio, s.target_mb));
                // finished parts (even if others failed or it was cancelled afterwards):
                // write side files, assigned to the project the export comes from (not
                // the currently open one), under the project name, not the (changeable) file name
                let mut failed = Vec::new();
                for x in done.iter().filter_map(|&i| src.parts.get(i)) {
                    failed.extend(x.sidecars.iter().filter_map(|(f, txt)| std::fs::write(f, txt).err().map(|e| format!("{} ({e})", f.display()))));
                    crate::exports::record(&src.project.name, src.project_path.as_deref(), &x.out);
                }
                let group = src.group().then_some(src.parts.len());
                match res {
                    Ok(()) => {
                        self.done = Some((Instant::now(), secs));
                        // „Zur Datei“ only in the dialog of the project the export comes from; after
                        // parts „Im Explorer zeigen“ (folder) stays
                        if src.project_path == self.project_path && group.is_none() {
                            self.exported = Some((src.rev, src.out.clone(), src.settings.clone()));
                        }
                        // the app translates toasts
                        let what = group.map_or_else(|| "Export fertig".to_string(), |n| format!("{n} Teile fertig"));
                        let mut msg = format!("{} {what} ({})", icon::CHECK_CIRCLE, theme::secs(secs as f64, 1));
                        if !failed.is_empty() {
                            let e = format!("{}: {}", crate::tr("Begleitdatei nicht gespeichert"), failed.join(", "));
                            msg = format!("{} {e}", icon::WARNING_CIRCLE);
                            self.error = Some(e);
                        }
                        toast = Some((msg, Some(src.project)));
                    }
                    Err(Some(e)) => {
                        // group: how many parts finished, plus the first error
                        let e = match group {
                            Some(n) => format!("{}: {e}", crate::tr(format!("{} von {n} Teilen fertig", done.len()))),
                            None => e,
                        };
                        // with the dialog closed the error would not be seen otherwise (the queue keeps running)
                        if !self.open {
                            let f = src.out.file_name().unwrap_or_default().to_string_lossy().to_string();
                            toast = Some((format!("{} {f}: {e}", icon::WARNING_CIRCLE), None));
                        }
                        self.error = Some(e);
                    }
                    Err(None) => {}
                }
            }
            Some((None, ..)) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            None => {}
        }
        // failure or cancel do not stop the queue
        self.start_next(ctx);
        toast
    }

    /// Shows the dialog. Returns a message for a toast („Kapitel kopiert“).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        p: &Project,
        fonts: &HashMap<String, (PathBuf, u32)>,
        cover: Option<(cl_core::ClipId, egui::TextureId, egui::Vec2, Time)>,
        rev: u64,
    ) -> Option<String> {
        // current project state: after a change the last export counts as outdated
        self.rev = rev;
        let mut toast = None;
        if !self.open {
            return toast;
        }
        // Esc closes the dialog (not during a running export), Enter triggers the
        // main action (queues if one is already running); neither while the question is open
        let (esc, enter) = crate::dialog::keys(ctx);
        if self.running.is_none() && !self.confirm_overwrite && esc {
            self.open = false;
            return toast;
        }
        let mut main = !self.confirm_overwrite && enter;
        let Some(mut s) = self.settings.clone() else {
            return toast;
        };
        s.chapters = self.chapters;
        s.chapter_name = crate::tr("Kapitel");
        // preselection by „Hardware-Kodierung“ until something is chosen in the dialog (detection in the background)
        if !self.encoder_picked {
            let found = self.encoders.lock().unwrap().clone().unwrap_or_default();
            s.encoder = cl_export::export_default(&found, &crate::proxy::encoder_pref());
        }
        toast = self.follow(ctx, p);
        // preview from the rebuilt snapshot (choice from the last frame)
        let framed = self.framed(p, fonts);
        let mut open = true;
        let mut start = false;
        // overwrite without asking (confirmed or „Erneut exportieren“)
        let mut overwrite = false;
        let title = format!("{}  Exportieren", icon::EXPORT);
        crate::dialog::window(ctx, crate::tr(title.as_str()), "export-dialog")
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .fixed_size(vec2(720.0, 0.0))
            .show(ctx, |ui| {
                if crate::dialog::title(ui, &title) {
                    open = false;
                }
                ui.horizontal_top(|ui| {
                    // left: cover/preview
                    ui.vertical(|ui| {
                        ui.set_width(250.0);
                        // stay within the 250×300 box without distorting the aspect ratio (9:16 gets narrower)
                        let ar = s.height as f32 / s.width.max(1) as f32;
                        let size = if ar > 1.2 { vec2(300.0 / ar, 300.0) } else { vec2(250.0, 250.0 * ar) };
                        let audio_only = s.format.audio_only();
                        let (rect, _) = ui.allocate_exact_size(if audio_only { vec2(250.0, 0.0) } else { size }, egui::Sense::hover());
                        if !audio_only {
                            ui.painter().rect_filled(rect, theme::R_CARD, Color32::BLACK);
                        }
                        let live = match self.running.as_mut() {
                            Some(r) => {
                                let pv = r.job.lock().unwrap().as_ref().and_then(|j| j.1.clone());
                                if let Some(pv) = pv {
                                    r.live.poll(ui.ctx(), &pv);
                                }
                                let prog = *r.progress.lock().unwrap();
                                paint_live(ui, rect, &r.live, prog)
                            }
                            None => false,
                        };
                        if let Some((cid, id, ts, at)) = cover.filter(|_| !live && !audio_only) {
                            match framed.as_deref().map(|f| cover_layers(f, cid, at)).filter(|l| !l.is_empty()) {
                                Some(layers) => paint_cover(ui, rect, id, &layers),
                                // picture fills the project format (portrait shows portrait, no bars)
                                None => drop(ui.painter().image(id, rect, fill_uv(ts, rect.size()), Color32::WHITE)),
                            }
                        }
                        ui.add_space(8.0);
                        let secs = s.range.map_or(p.duration(), |(a, b)| b - a).as_secs_f64();
                        // target size applies per part: size of the picture as for the longest part
                        let (longest, n) = self.longest_part(p, &s);
                        let info = if audio_only {
                            format!("{} {}", icon::CLOCK, theme::secs(secs, 1))
                        } else {
                            let (w, h) = s.for_target(longest, audible(p, &s)).output_size();
                            format!("{} {}  ·  {w}×{h}", icon::CLOCK, theme::secs(secs, 1))
                        };
                        ui.label(RichText::new(crate::tr(info)).small().color(theme::TEXT_DIM()));
                        if !audio_only {
                            let (est, exact) = estimate_mb(&s, p.duration());
                            let txt = match s.target_mb.filter(|_| s.target_possible()) {
                                Some(m) if n > 1 => format!("{} {n} × {m} MB", icon::HARD_DRIVES),
                                Some(m) => format!("{} {m} MB", icon::HARD_DRIVES),
                                None if exact => format!("{} {est:.0} MB", icon::HARD_DRIVES),
                                None => format!("{} ca. {est:.0} MB", icon::HARD_DRIVES),
                            };
                            ui.label(RichText::new(crate::tr(txt)).small().color(theme::TEXT_DIM()));
                        }
                        if live {
                            ui.label(RichText::new(crate::tr("Live-Vorschau")).size(theme::FS_HINT).color(theme::TEXT_DIM()));
                        }
                    });
                    ui.add_space(12.0);
                    // right: settings
                    ui.vertical(|ui| {
                        // as tall as the content (largest version shown so far: does not jump when
                        // switching), at most until the window fits into the program window; then it scrolls.
                        // Without a minimum height the area would take only the height of the left column.
                        let max_h = (ctx.screen_rect().height() - DIALOG_CHROME_H).max(160.0);
                        let sc = egui::ScrollArea::vertical()
                            .id_salt("export-settings")
                            .max_height(max_h)
                            .min_scrolled_height(self.right_h.min(max_h))
                            .show(ui, |ui| {
                        // a format change hides rows: keep the height so buttons do not jump
                        ui.set_min_height(self.right_h);
                        // adjustable during an export too: the next one is queued
                        ui.scope(|ui| {
                            theme::row(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(300.0)));
                            theme::row(ui, "Speichern in", |ui| {
                                // leave room for the folder button, otherwise the row reaches beyond the window
                                let w = (ui.available_width() - 32.0).max(40.0);
                                ui.allocate_ui(vec2(w, 20.0), |ui| {
                                    ui.add(egui::Label::new(RichText::new(crate::paths::shown(&self.folder)).small()).truncate());
                                });
                                if theme::icon_button(ui, icon::FOLDER_OPEN, "Ordner wählen", true, false).clicked() {
                                    if let Some(d) = crate::dialog().set_directory(&self.folder).pick_folder() {
                                        self.folder = d;
                                    }
                                }
                            });
                            theme::row(ui, "Format", |ui| {
                                egui::ComboBox::from_id_salt("exp-format").selected_text(crate::tr(s.format.label())).width(200.0).show_ui(ui, |ui| {
                                    for f in Format::ALL {
                                        ui.selectable_value(&mut s.format, f, crate::tr(f.label()));
                                    }
                                });
                            });
                            if let Some((a, b)) = self.range_avail {
                                let mut only = s.range.is_some();
                                let fmt = |t: Time| format!("{:02}:{:02}.{:02}", (t.as_secs_f64() / 60.0) as u32, t.as_secs_f64() as u32 % 60, ((t.as_secs_f64().fract()) * 100.0) as u32);
                                theme::row(ui, "", |ui| {
                                    if crate::widgets::check(ui, &mut only, crate::tr(format!("Nur In/Out-Bereich ({} – {})", fmt(a), fmt(b)))).changed() {
                                        s.range = only.then_some((a, b));
                                    }
                                });
                            }
                            if !s.format.audio_only() {
                                theme::section(ui, "Video");
                                // platform presets: set format, codec, size, frame rate and quality
                                theme::row(ui, "Plattform", |ui| {
                                    let sel = self.platform.and_then(platform).map(|pl| crate::tr(pl.name)).unwrap_or_else(|| crate::tr("Plattform wählen …"));
                                    egui::ComboBox::from_id_salt("exp-preset").selected_text(sel).width(200.0).show_ui(ui, |ui| {
                                        for pl in PLATFORMS {
                                            if ui.selectable_label(self.platform == Some(pl.id), crate::tr(pl.name)).clicked() {
                                                self.pick_platform(&mut s, p, pl);
                                            }
                                        }
                                    });
                                });
                                // other aspect ratio: the snapshot is rebuilt, the project stays
                                theme::row(ui, "Seitenverhältnis", |ui| {
                                    let label = |i: usize| ASPECTS.get(i.wrapping_sub(1)).map_or_else(|| crate::tr("Projekt"), |(a, b)| format!("{a}:{b}"));
                                    egui::ComboBox::from_id_salt("exp-aspect").selected_text(label(self.aspect)).width(200.0).show_ui(ui, |ui| {
                                        for i in 0..=ASPECTS.len() {
                                            ui.selectable_value(&mut self.aspect, i, label(i));
                                        }
                                    });
                                });
                                let canvas = self.canvas(p);
                                if canvas.is_some() {
                                    theme::row(ui, "Bild", |ui| {
                                        ui.allocate_ui(vec2(300.0, 24.0), |ui| crate::widgets::segmented(ui, &["Füllen", "Auto-Reframe", "Einpassen"], &mut self.fit));
                                    });
                                    if let Some(j) = &self.focus_job {
                                        ui.horizontal(|ui| {
                                            let (r, _) = ui.allocate_exact_size(vec2((ui.available_width() - 36.0).max(20.0), 3.0), egui::Sense::hover());
                                            crate::widgets::task_line(ui.painter(), egui::Id::new("exp-follow"), r, *j.progress.lock().unwrap());
                                            if theme::icon_button(ui, icon::X, "Abbrechen", true, false).clicked() {
                                                j.cancel.store(true, Ordering::Relaxed);
                                            }
                                        });
                                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                                    }
                                }
                                theme::row(ui, "Auflösung", |ui| {
                                    let (pw, ph) = canvas.unwrap_or((p.settings.width, p.settings.height));
                                    let label = |e: u32| match e {
                                        0 => format!("Projekt ({pw}×{ph})"),
                                        2160 => "4K (2160p)".into(),
                                        e => format!("{e}p"),
                                    };
                                    egui::ComboBox::from_id_salt("exp-res").selected_text(crate::tr(label(self.short_edge))).width(200.0).show_ui(ui, |ui| {
                                        for e in [0, 480, 720, 1080, 1440, 2160] {
                                            ui.selectable_value(&mut self.short_edge, e, crate::tr(label(e)));
                                        }
                                    });
                                });
                                let (pw, ph) = canvas.unwrap_or((p.settings.width, p.settings.height));
                                let (pw, ph) = (pw.max(2), ph.max(2));
                                if self.short_edge == 0 {
                                    s.width = pw;
                                    s.height = ph;
                                } else {
                                    let f = self.short_edge as f64 / pw.min(ph) as f64;
                                    s.width = ((pw as f64 * f).round() as u32) & !1;
                                    s.height = ((ph as f64 * f).round() as u32) & !1;
                                }
                                self.size_rows(ui, p, &mut s);
                                if s.format != Format::Gif {
                                    theme::row(ui, "Codec", |ui| {
                                        if !Codec::for_format(s.format).contains(&s.codec) {
                                            s.codec = Codec::for_format(s.format)[0];
                                        }
                                        egui::ComboBox::from_id_salt("exp-codec").selected_text(crate::tr(s.codec.label())).width(200.0).show_ui(ui, |ui| {
                                            for c in Codec::for_format(s.format) {
                                                ui.selectable_value(&mut s.codec, *c, crate::tr(c.label()));
                                            }
                                        });
                                    });
                                    if s.alpha_possible() {
                                        theme::row(ui, "Hintergrund", |ui| {
                                            crate::widgets::check(ui, &mut s.transparent, crate::tr(if s.codec == Codec::ProRes { "Transparent (ProRes 4444)" } else { "Transparent (VP9 mit Alpha)" }))
                                                .on_hover_text(crate::tr("Leere Bereiche und freigestellte Personen bleiben durchsichtig, z. B. für Overlays in anderen Programmen"));
                                        });
                                    }
                                    // with a target size it determines the bitrate
                                    ui.add_enabled_ui(s.target_mb.is_none(), |ui| theme::row(ui, "Qualität", |ui| {
                                        let label = match s.quality {
                                            Quality::Lower => "Kleinere Datei".to_string(),
                                            Quality::Recommended => "Empfohlen".into(),
                                            Quality::Higher => "Höher".into(),
                                            Quality::Bitrate(k) => format!("{k} kbit/s"),
                                        };
                                        egui::ComboBox::from_id_salt("exp-q").selected_text(crate::tr(label)).width(200.0).show_ui(ui, |ui| {
                                            ui.selectable_value(&mut s.quality, Quality::Lower, crate::tr("Kleinere Datei"));
                                            ui.selectable_value(&mut s.quality, Quality::Recommended, crate::tr("Empfohlen"));
                                            ui.selectable_value(&mut s.quality, Quality::Higher, crate::tr("Höher"));
                                            ui.selectable_value(&mut s.quality, Quality::Bitrate(self.custom_kbps), crate::tr("Benutzerdefinierte Bitrate"));
                                        });
                                    }));
                                    if let Quality::Bitrate(_) = s.quality {
                                        theme::row(ui, "", |ui| {
                                            ui.add(egui::DragValue::new(&mut self.custom_kbps).range(500..=200_000).suffix(" kbit/s").speed(100));
                                        });
                                        s.quality = Quality::Bitrate(self.custom_kbps);
                                    }
                                    theme::row(ui, "Bildrate", |ui| {
                                        egui::ComboBox::from_id_salt("exp-fps").selected_text(crate::tr(format!("{} fps", theme::dec(s.fps.as_f64(), 3)))).width(200.0).show_ui(ui, |ui| {
                                            for f in ["23.976", "24", "25", "29.97", "30", "50", "59.94", "60"] {
                                                let r = cl_core::Rational::parse(f).unwrap();
                                                ui.selectable_value(&mut s.fps, r, crate::tr(format!("{f} fps")));
                                            }
                                        });
                                    });
                                    let encs = self.encoders.lock().unwrap().clone().unwrap_or_default();
                                    // with a target size always software (two passes); the choice stays for afterwards
                                    let target = s.target_mb.is_some();
                                    ui.add_enabled_ui(!target, |ui| theme::row(ui, "Encoder", |ui| {
                                        let name = encoder_name;
                                        let shown = if target { Encoder::Software } else { s.encoder };
                                        egui::ComboBox::from_id_salt("exp-enc").selected_text(crate::tr(name(shown))).width(200.0).show_ui(ui, |ui| {
                                            for e in encs {
                                                if ui.selectable_value(&mut s.encoder, e, crate::tr(name(e))).clicked() {
                                                    self.encoder_picked = true;
                                                }
                                            }
                                        });
                                    }));
                                    self.split_rows(ui, p, &s);
                                    theme::section(ui, "Ton");
                                    crate::widgets::check(ui, &mut s.include_audio, crate::tr(format!("Ton exportieren (AAC {} kbit/s, bei WebM Opus)", s.audio_kbps.unwrap_or(256))));
                                    ui.add_enabled_ui(s.include_audio, |ui| loudness_row(ui, &mut s.loudness));
                                } else {
                                    self.split_rows(ui, p, &s);
                                }
                            } else {
                                loudness_row(ui, &mut s.loudness);
                            }
                            if !s.format.audio_only() {
                                crate::widgets::check(ui, &mut self.also_srt, crate::tr("Texte zusätzlich als Untertiteldateien (SRT und VTT) speichern"));
                                let ms = &p.timeline.markers;
                                if self.chapters && !ms.is_empty() {
                                    theme::section(ui, "Kapitel");
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(format!("{} {}", ms.len(), crate::tr("aus Markern"))).small().color(theme::TEXT_DIM()));
                                        if ui.add(theme::secondary(format!("{}  Kapitel kopieren", icon::COPY))).clicked() {
                                            ui.ctx().copy_text(crate::navigate::chapters(ms));
                                            toast = Some(crate::tr("Kapitel in die Zwischenablage kopiert"));
                                        }
                                    });
                                    let (a, b) = s.range.unwrap_or((Time::ZERO, p.duration()));
                                    if let Some(w) = crate::navigate::chapter_issue(ms, a, b).filter(|_| self.chapter_hint) {
                                        if crate::navigate::chapter_warning(ui, &w) {
                                            self.chapter_hint = false;
                                            self.chapter_hint_off = true;
                                        }
                                    }
                                }
                            }
                        });
                        });
                        self.right_view = (sc.inner_rect.height(), sc.content_size.y);
                        self.right_h = self.right_h.max(sc.content_size.y);
                    });
                });

                ui.add_space(12.0);
                if !self.missing.is_empty() {
                    ui.label(
                        RichText::new(crate::tr(format!(
                            "{}  {}",
                            icon::WARNING,
                            theme::count_text(
                                self.missing.len(),
                                "{} Inhalt fehlt und wird ohne ihn exportiert: {}",
                                "{} Inhalte fehlen und werden ohne sie exportiert: {}",
                            )
                            .replacen("{}", &self.missing.join(", "), 1)
                        )))
                        .small()
                        .color(theme::WARN_TEXT()),
                    );
                }
                if let Some(e) = &self.error {
                    ui.label(RichText::new(crate::tr(format!("{} {e}", icon::WARNING_CIRCLE))).color(theme::DANGER_TEXT()));
                }
                if let Some(r) = &self.running {
                    let prog = *r.progress.lock().unwrap();
                    let measures = r.job.lock().unwrap().as_ref().map(|j| j.0);
                    match measures {
                        Some(m) => {
                            // parts: progress over all, the measuring run per part is not shown separately
                            let part = r.part_of();
                            export_progress(ui, prog, r.started.elapsed().as_secs_f32(), m && prog < cl_export::MEASURE_SHARE && part.is_none(), part, self.queue.len())
                        }
                        None => {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(crate::tr("Wird vorbereitet …")).color(theme::TEXT()));
                                waiting_count(ui, self.queue.len());
                            });
                            ui.add_space(4.0);
                            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
                            crate::widgets::busy_line(ui.painter(), rect);
                            ui.ctx().request_repaint();
                        }
                    }
                } else if let Some((at, secs)) = self.done {
                    if !export_done(ui, at, secs) {
                        self.done = None;
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if let Some(out) = &self.last_output {
                        if self.running.is_none() && out.exists() && self.exported.as_ref().is_none_or(|(r, f, used)| *r != self.rev || f != out || *used != s) {
                            if ui.add(theme::secondary(format!("{}  Im Explorer zeigen", icon::FOLDER_OPEN))).clicked() {
                                crate::home::reveal(out);
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // „Person folgen“ still computing: export only afterwards
                        let full = self.queue.len() >= QUEUE_MAX || self.focus_job.is_some();
                        if self.running.is_none() && self.unchanged_since_export(&s) {
                            // unchanged since the last export: straight to the finished file
                            if ui.add(theme::primary(format!("{}  Zur Datei", icon::FOLDER_OPEN))).clicked() || std::mem::take(&mut main) {
                                if let Some((_, f, _)) = &self.exported {
                                    crate::home::reveal(f);
                                }
                            }
                            if ui.add(theme::secondary(format!("{}  Erneut exportieren", icon::ARROW_CLOCKWISE))).clicked() {
                                start = true;
                                overwrite = true;
                            }
                        } else {
                            // if one is already running, „Exportieren“ queues at the back too
                            let tip = crate::tr(format!("Höchstens {QUEUE_MAX} wartende Exporte"));
                            if ui.add_enabled(!full, theme::primary(format!("{}  Exportieren", icon::EXPORT))).on_disabled_hover_text(&tip).clicked() || (!full && std::mem::take(&mut main)) {
                                start = true;
                            }
                            if ui.add_enabled(!full, theme::secondary(format!("{}  In Warteschlange", icon::LIST_PLUS))).on_disabled_hover_text(&tip).clicked() {
                                // like „Exportieren“: starts at once if none is running; the window stays open
                                start = true;
                            }
                        }
                        if self.running.is_some() {
                            if ui.add(theme::secondary(format!("{}  Abbrechen", icon::X))).clicked() {
                                self.cancel_current();
                            }
                        } else {
                            let close_label = if self.done.is_some() || self.unchanged_since_export(&s) { "Schließen" } else { "Abbrechen" };
                            if ui.add(theme::secondary(close_label)).clicked() {
                                open = false;
                            }
                        }
                    });
                });
            });
        self.settings = Some(s.clone());
        if !open {
            self.open = false;
        }
        let (out, parts) = self.targets(p, &s);
        let mut clean = false;
        start &= self.focus_job.is_none();
        // with parts: one question for the whole group
        if start && !overwrite && self.taken(&out, &parts) {
            self.confirm_overwrite = true;
            self.confirm_fresh = true;
            start = false;
        }
        if self.confirm_overwrite {
            // 0 = overwrite, 1 = other name, 2 = cancel
            let mut choice = None;
            let file = out.clone();
            let exists = parts.iter().any(|f| f.1.exists());
            // parts of an earlier run that no longer exist this time (e.g. „Teil 06“ of 6, now 4)
            let keep: Vec<PathBuf> = parts.iter().map(|f| f.1.clone()).collect();
            let stale = if self.split(&s).is_some() {
                let recorded: Vec<PathBuf> = crate::exports::for_project(&p.name, self.project_path.as_deref()).into_iter().map(|e| e.path).collect();
                stale_parts(&out, s.format.extension(), &keep, &recorded)
            } else {
                Vec::new()
            };
            let old = stale.iter().filter(|f| f.extension().is_some_and(|e| e == s.format.extension())).count();
            let m = crate::dialog::modal(ctx, "export-overwrite").show(ctx, |ui| {
                ui.set_width(380.0);
                let t = match (exists, parts.len() > 1) {
                    (false, _) => "Name schon in der Warteschlange",
                    (true, false) => "Datei existiert bereits",
                    (true, true) => "Dateien existieren bereits",
                };
                if crate::dialog::title(ui, t) {
                    choice = Some(2);
                }
                ui.label(RichText::new(crate::paths::shown(&file)).small().color(theme::TEXT_DIM()));
                if old > 0 {
                    ui.label(RichText::new(crate::tr(format!("Beim Überschreiben werden {old} ältere Teile entfernt"))).small().color(theme::TEXT_DIM()));
                }
                crate::dialog::footer(ui, |ui| {
                    if ui.add(theme::primary("Anderen Namen")).clicked() {
                        choice = Some(1);
                    }
                    if ui.add(theme::danger("Überschreiben")).clicked() {
                        choice = Some(0);
                    }
                    if ui.add(theme::secondary("Abbrechen")).clicked() {
                        choice = Some(2);
                    }
                });
            });
            let fresh = std::mem::take(&mut self.confirm_fresh);
            if fresh {
                ctx.request_repaint();
            } else if choice.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                // Enter never overwrites: harmless main action
                choice = Some(1);
            } else if choice.is_none() && m.should_close() {
                choice = Some(2);
            }
            match choice {
                Some(0) => {
                    // removed only by the export itself before its first part (also parts of a
                    // still running or waiting export that do not exist yet)
                    clean = self.split(&s).is_some();
                    start = true;
                }
                Some(1) => {
                    let base = self.name.trim().to_string();
                    for n in 2.. {
                        self.name = format!("{base} ({n})");
                        let (o, f) = self.targets(p, &s);
                        if !self.taken(&o, &f) {
                            break;
                        }
                    }
                    start = true;
                }
                _ => {}
            }
            if choice.is_some() {
                self.confirm_overwrite = false;
            }
        }
        if start && self.queue.len() < QUEUE_MAX {
            self.error = None;
            self.done = None;
            // anew after „Anderen Namen“
            let (out, parts) = self.targets(p, &s);
            let parts = parts
                .into_iter()
                .map(|(range, f)| Part { sidecars: sidecars(p, &ExportSettings { range, ..s.clone() }, self.also_srt, &f), range, out: f })
                .collect();
            let project = self.framed(p, fonts).unwrap_or_else(|| Arc::new(p.clone()));
            self.queue.push_back(Queued {
                project,
                project_path: self.project_path.clone(),
                settings: s.clone(),
                parts,
                out,
                fonts: fonts.clone(),
                rev: self.rev,
                clean,
            });
            self.start_next(ctx);
        }
        toast
    }
}

/// Audible sound in the export range? (target size: otherwise nothing is planned for sound)
fn audible(p: &Project, s: &ExportSettings) -> bool {
    let (a, b) = s.range.unwrap_or((Time::ZERO, p.duration()));
    cl_export::audible_clips(p, a, b).next().is_some()
}

/// Name for files and folders: characters Windows forbids become `_`; length and
/// device names as with project files ([`crate::paths::fit_name`]).
fn file_safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    crate::paths::fit_name(&s)
}

/// Files of an earlier part group in the parts folder `dir` that do not belong to the new parts `keep`.
/// Only exactly this group's pattern: `Name – Teil NN`, `Name – Part NN` or
/// `Name – NN[ Marker]` (Name = folder name) with the format's extension or as a side file
/// (.srt, .vtt, .chapters.txt), and only parts Cutline wrote itself (`recorded`,
/// from the exports list) together with their side files; nothing else.
fn stale_parts(dir: &Path, ext: &str, keep: &[PathBuf], recorded: &[PathBuf]) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let Some(name) = dir.file_name() else { return Vec::new() };
    let ends = [format!(".{ext}"), ".srt".into(), ".vtt".into(), ".chapters.txt".into()];
    let prefix = format!("{} – ", name.to_string_lossy());
    let nr = |t: &str| t.len() >= 2 && t.bytes().all(|b| b.is_ascii_digit());
    let ours = |n: &str| {
        let Some(base) = ends.iter().find_map(|e| n.strip_suffix(e.as_str())) else { return false };
        let Some(rest) = base.strip_prefix(&prefix).filter(|_| recorded.contains(&dir.join(format!("{base}.{ext}")))) else {
            return false;
        };
        match rest.split_once(' ') {
            Some(("Teil" | "Part", t)) => nr(t),
            Some((h, t)) => nr(h) && !t.trim().is_empty(),
            None => nr(rest),
        }
    };
    let kept = |f: &Path| keep.iter().any(|k| ["srt", "vtt", "chapters.txt"].iter().any(|e| k.with_extension(e) == f) || k == f);
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|f| f.is_file() && f.file_name().is_some_and(|n| ours(&n.to_string_lossy())) && !kept(f))
        .collect();
    v.sort();
    v
}

/// „In Teile aufteilen“: equally long parts, parts of fixed length (seconds) or cut at markers
#[derive(Debug, Clone, Copy, PartialEq)]
enum Split {
    Count(u32),
    Length(u32),
    Markers,
}

/// Maximum number of parts (count, length, markers)
const SPLIT_MAX: u32 = 50;
/// A shorter remainder with „Länge“ is added to the second-to-last part
const SPLIT_MIN_REST: Time = Time(3_000_000);

/// Parts of an export from `a` to `b`: range and file name per part, gapless and without
/// overlap; cuts lie on frame boundaries of the project (`fps`). `name` is already
/// file-safe, `part` is the word „Teil“ in the UI language.
fn split_parts(how: Split, (a, b): (Time, Time), fps: cl_core::Rational, markers: &[cl_core::Marker], name: &str, part: &str, ext: &str) -> Vec<((Time, Time), String)> {
    let snap = |t: Time| t.snap_to_frame(fps);
    // cut points with marker name (empty without marker)
    let mut cuts: Vec<(Time, &str)> = match how {
        Split::Count(n) => {
            let n = n.clamp(2, SPLIT_MAX) as i64;
            (1..n).map(|k| (snap(a + Time((b - a).0 * k / n)), "")).collect()
        }
        Split::Length(secs) => {
            // at most SPLIT_MAX parts: length raised to whole seconds if necessary
            let min = ((b - a).0.max(0) as u64).div_ceil(SPLIT_MAX as u64 * 1_000_000) as i64;
            let step = Time((secs as i64).max(min).max(1) * 1_000_000);
            let mut v: Vec<(Time, &str)> = (1..).map(|k| a + Time(step.0 * k)).take_while(|t| *t < b).map(|t| (snap(t), "")).collect();
            if v.last().is_some_and(|(t, _)| b - *t < SPLIT_MIN_REST) {
                v.pop();
            }
            v
        }
        Split::Markers => markers.iter().map(|m| (snap(m.at), m.name.as_str())).collect(),
    };
    cuts.sort_by_key(|c| c.0);
    // a marker exactly at the start names the first part
    let first = cuts.iter().find(|c| c.0 == snap(a) || c.0 == a).map_or("", |c| c.1);
    // edge not on the frame grid: no splinter part between the edge and the rounded cut
    cuts.retain(|c| c.0 > a.max(snap(a)) && c.0 < b.min(snap(b)));
    cuts.dedup_by_key(|c| c.0);
    // at most SPLIT_MAX parts (markers): later cuts are dropped, the last part runs to the end
    cuts.truncate(SPLIT_MAX as usize - 1);
    let starts: Vec<(Time, &str)> = std::iter::once((a, first)).chain(cuts).collect();
    let n = starts.len();
    let digits = n.to_string().len().max(2);
    starts
        .iter()
        .enumerate()
        .map(|(i, &(t, label))| {
            let end = starts.get(i + 1).map_or(b, |s| s.0);
            let nr = format!("{:0digits$}", i + 1);
            let file = match (how, file_safe(label)) {
                (Split::Markers, l) if l.is_empty() => format!("{name} – {nr}.{ext}"),
                (Split::Markers, l) => format!("{name} – {nr} {l}.{ext}"),
                _ => format!("{name} – {part} {nr}.{ext}"),
            };
            ((t, end), file)
        })
        .collect()
}

/// Result line under „Aufteilen nach“: „6 Teile · je 1:00“ or „4 Teile · 0:42 bis 2:10“
fn split_summary(parts: &[((Time, Time), String)]) -> String {
    let durs = parts.iter().map(|((a, b), _)| *b - *a);
    // rounded to whole seconds: 59.97 s (frame grid) shows 1:00 like the other parts
    let t = |d: Option<Time>| crate::navigate::chapter_time(Time((d.unwrap_or_default().0 + 500_000) / 1_000_000 * 1_000_000));
    let (lo, hi) = (t(durs.clone().min()), t(durs.max()));
    if lo == hi {
        crate::tr(format!("{} Teile · je {lo}", parts.len()))
    } else {
        crate::tr(format!("{} Teile · {lo} bis {hi}", parts.len()))
    }
}

/// Side files next to `out` (path, content) for the export range: subtitles (SRT, VTT)
/// and chapters like „Kapitel kopieren“ (only if switched on), times from the range start.
fn sidecars(p: &Project, s: &ExportSettings, subs: bool, out: &Path) -> Vec<(PathBuf, String)> {
    let mut v = Vec::new();
    if s.format.audio_only() {
        return v;
    }
    let (a, b) = s.range.unwrap_or((Time::ZERO, p.duration()));
    if subs {
        v.push((out.with_extension("srt"), cl_export::to_srt_in(p, a, b)));
        v.push((out.with_extension("vtt"), cl_export::to_vtt_in(p, a, b)));
    }
    let ms = &p.timeline.markers;
    if s.chapters && ms.iter().any(|m| m.at >= a && m.at < b) {
        let txt = cl_export::chapter_list(ms, a, b, &s.chapter_name)
            .iter()
            .map(|(t, n)| format!("{} {n}", crate::navigate::chapter_time(*t)))
            .collect::<Vec<_>>()
            .join("\n");
        v.push((out.with_extension("chapters.txt"), txt));
    }
    v
}

/// Progress during export: text with percent and remaining time, the progress line below.
fn export_progress(ui: &mut egui::Ui, prog: f32, elapsed: f32, measuring: bool, part: Option<(usize, usize)>, waiting: usize) {
    ui.horizontal(|ui| {
        let what = match part {
            _ if measuring => "Lautheit messen …".to_string(),
            Some((i, n)) => format!("Exportiere Teil {i} von {n} …"),
            None => "Exportiere …".into(),
        };
        ui.label(RichText::new(crate::tr(what)).color(theme::TEXT()));
        waiting_count(ui, waiting);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if prog > 0.03 && elapsed > 2.0 {
                let left = (elapsed / prog * (1.0 - prog)).max(0.0) as u32;
                ui.label(
                    RichText::new(crate::tr(format!("noch ca. {}:{:02}", left / 60, left % 60)))
                        .color(theme::TEXT_DIM()),
                );
            }
            ui.label(RichText::new(format!("{:.0} %", prog * 100.0)).strong());
        });
    });
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
    crate::widgets::progress_line(ui.painter(), egui::Id::new("export-prog"), rect, prog);
    // remaining time keeps running even without a new progress report
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
}

/// Number of waiting exports behind the running one („+2“)
fn waiting_count(ui: &mut egui::Ui, n: usize) {
    if n > 0 {
        ui.label(RichText::new(format!("+{n}")).color(theme::TEXT_DIM()));
    }
}

/// Success: the check mark grows smoothly (`MOTION_SLOW`), stays visible for 6 s. `false` = hidden.
fn export_done(ui: &mut egui::Ui, at: Instant, secs: f32) -> bool {
    let age = at.elapsed().as_secs_f32();
    if age > 6.0 {
        return false;
    }
    let dur = theme::anim(theme::MOTION_SLOW);
    let pop = if dur > 0.0 { 0.6 + 0.4 * theme::ease(age / dur) } else { 1.0 };
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), egui::Sense::hover());
        let p = ui.painter();
        p.circle_filled(r.center(), 11.0 * pop, theme::SUCCESS);
        p.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            icon::CHECK,
            egui::FontId::proportional(13.0 * pop.max(0.01)),
            theme::SUCCESS_BG,
        );
        ui.label(RichText::new(crate::tr(format!("Fertig in {}", theme::secs(secs as f64, 1)))).strong());
    });
    if age < dur {
        ui.ctx().request_repaint();
    } else {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
    true
}

/// „Lautheit angleichen“: YouTube (−14 LUFS), speech (−16 LUFS) or off.
fn loudness_row(ui: &mut egui::Ui, l: &mut Option<f32>) {
    let name = |l: Option<f32>| match l {
        Some(x) if x > -15.0 => "YouTube (−14 LUFS)",
        Some(_) => "Sprache (−16 LUFS)",
        None => "Aus",
    };
    theme::row(ui, "Lautheit", |ui| {
        egui::ComboBox::from_id_salt("exp-loud").selected_text(crate::tr(name(*l))).width(200.0).show_ui(ui, |ui| {
            for v in [Some(-14.0), Some(-16.0), None] {
                ui.selectable_value(l, v, crate::tr(name(v)));
            }
        })
        .response
        .on_hover_text(crate::tr("Misst den Ton vorab und hebt oder senkt ihn gleichmäßig, Spitzen höchstens −1 dBTP"));
    });
}

/// Estimate the file size; `true` = from a fixed target bitrate (video + sound) × duration.
/// With constant quality (CRF) roughly from pixels × frame rate, calibrated to
/// 1080p30 „Empfohlen“ ≈ 23 Mbit/s (what libx264 actually delivers with CRF 20);
/// each +6 CRF about half the size (Kleiner = CRF 26, Höher = CRF 16). Other codecs relative
/// to it (VP9/AV1 with their CRF values about 0.3, HEVC 0.5), ProRes fixed ≈ 220 Mbit/s (HQ),
/// GIF from area × frames (palette with dithering, for film material ≈ 0.6 bytes per pixel).
fn estimate_mb(s: &ExportSettings, d: Time) -> (f64, bool) {
    let secs = s.range.map_or(d.as_secs_f64(), |(a, b)| (b.as_secs_f64() - a.as_secs_f64()).max(0.0));
    let (w, h) = s.output_size();
    let px = (w * h) as f64 / (1920.0 * 1080.0);
    if s.format == Format::Gif {
        let frames = s.fps.as_f64().min(cl_export::GIF_FPS as f64) * secs;
        return (px * 1920.0 * 1080.0 * frames * 0.6 / 1e6, false);
    }
    let fps = s.fps.as_f64() / 30.0;
    let kbps = match (s.codec, s.quality) {
        (Codec::ProRes, _) => px * fps * if s.alpha() { 330_000.0 } else { 220_000.0 },
        (_, Quality::Bitrate(k)) => k as f64,
        (c, q) => {
            let codec = match c {
                Codec::Vp9 | Codec::Av1 => 0.3,
                Codec::Hevc => 0.5,
                _ => 1.0,
            };
            px * 23_000.0 * fps * codec * match q {
                Quality::Lower => 0.5,
                Quality::Higher => 2f64.powf(4.0 / 6.0),
                _ => 1.0,
            }
        }
    };
    let exact = matches!(s.quality, Quality::Bitrate(_)) && s.codec != Codec::ProRes;
    ((kbps + s.audio_bitrate() as f64) * secs / 8.0 / 1000.0, exact)
}

/// Layers of the cover image (clip `id`) in the snapshot `p`: blurred background if present, then
/// the clip; each with position as a share of the picture area, UV section (crop), blurred
fn cover_layers(p: &Project, id: cl_core::ClipId, at: Time) -> Vec<(Rect, Rect, bool)> {
    let (w, h) = (p.settings.width.max(2) as f32, p.settings.height.max(2) as f32);
    [(id.derived(crate::shorts::BACK_SALT), true), (id, false)]
        .into_iter()
        .filter_map(|(id, blur)| {
            let c = p.find_clip(id)?.1;
            let cl_core::ClipContent::Media { media, .. } = &c.content else { return None };
            let m = p.media(*media).filter(|m| m.width > 0 && m.height > 0)?;
            // position at the cover image (keyframes, e.g. „Person folgen“)
            let e = c.eval_at(Time((at - c.start).0.max(0)), w, h);
            let d = e.scale * (w / m.width as f32).min(h / m.height as f32);
            let size = vec2(m.width as f32 * d / w, m.height as f32 * d / h);
            let full = Rect::from_center_size(egui::pos2(0.5 + e.x / w, 0.5 + e.y / h), size);
            let cr = c.crop.clamped();
            let uv = Rect::from_min_max(egui::pos2(cr.left, cr.top), egui::pos2(1.0 - cr.right, 1.0 - cr.bottom));
            Some((Rect::from_min_max(full.lerp_inside(uv.min.to_vec2()), full.lerp_inside(uv.max.to_vec2())), uv, blur))
        })
        .collect()
}

/// Cover image per [`cover_layers`] in `rect`; blurred as the mean of 16 offset copies
fn paint_cover(ui: &egui::Ui, rect: Rect, tex: egui::TextureId, layers: &[(Rect, Rect, bool)]) {
    let p = ui.painter().with_clip_rect(rect);
    for (r, uv, blur) in layers {
        let r = Rect::from_min_max(rect.lerp_inside(r.min.to_vec2()), rect.lerp_inside(r.max.to_vec2()));
        if !*blur {
            p.image(tex, r, *uv, Color32::WHITE);
            continue;
        }
        let rad = rect.width().min(rect.height()) * 0.04;
        for i in 0..16 {
            let a = i as f32 * std::f32::consts::TAU / 16.0;
            let off = vec2(a.cos(), a.sin()) * rad * if i % 2 == 0 { 1.0 } else { 0.5 };
            // share 1/(i+1): gives the mean of all copies
            p.image(tex, r.translate(off), *uv, Color32::WHITE.gamma_multiply(1.0 / (i + 1) as f32));
        }
    }
}

/// UV section of an image of size `ts` that fills `area` without bars (centre-cropped).
fn fill_uv(ts: egui::Vec2, area: egui::Vec2) -> Rect {
    let (img, want) = (ts.x / ts.y.max(1.0), area.x / area.y.max(1.0));
    let size = if img > want { vec2(want / img, 1.0) } else { vec2(1.0, img / want) };
    Rect::from_center_size(egui::pos2(0.5, 0.5), size)
}

#[test]
fn youtube_presets_hit_official_bitrates() {
    let mut s = ExportSettings::from_project(&Project::new("t"));
    let (mut edge, mut kbps) = (0, 0);
    for (id, fps, want) in [
        ("youtube-1080", "30", 8_000),
        ("youtube-1080", "60", 12_000),
        ("youtube-1440", "29.97", 16_000),
        ("youtube-1440", "50", 24_000),
        ("youtube-4k", "24", 40_000),
        ("youtube-4k", "60", 60_000),
    ] {
        s.fps = cl_core::Rational::parse(fps).unwrap();
        ExportDialog::apply_platform(&mut s, &mut edge, &mut kbps, platform(id).unwrap());
        assert_eq!((s.quality, kbps), (Quality::Bitrate(want), want), "{id} {fps}");
        assert_eq!((s.audio_kbps, s.loudness), (Some(384), Some(-14.0)));
    }
    // other presets: own quality, loudness off
    ExportDialog::apply_platform(&mut s, &mut edge, &mut kbps, platform("small").unwrap());
    assert_eq!((s.quality, s.audio_kbps, s.loudness), (Quality::Lower, None, None));
    // size from target bitrate: (8000 + 384) kbit/s × 60 s
    s.quality = Quality::Bitrate(8_000);
    s.audio_kbps = Some(384);
    let (mb, exact) = estimate_mb(&s, Time::from_secs_f64(60.0));
    assert!(exact && (mb - 62.88).abs() < 0.01, "{mb}");
}

#[test]
fn estimate_per_format_within_factor_two_of_real_exports() {
    // real exports from the click test (beach video 1080p30, in/out range)
    let within = |mb: f64, real: f64| (real / 2.0..=real * 2.0).contains(&mb);
    let mut s = ExportSettings::from_project(&Project::new("t"));
    (s.width, s.height, s.fps) = (1920, 1080, cl_core::Rational::parse("30").unwrap());
    s.range = Some((Time::from_secs_f64(5.03), Time::from_secs_f64(9.038)));
    // WebM/VP9 „Empfohlen“ + Opus, 4.008 s: 3.73 MB (estimated 12 MB before)
    (s.format, s.codec) = (Format::WebM, Codec::Vp9);
    let (mb, _) = estimate_mb(&s, Time::ZERO);
    assert!(within(mb, 3.73), "WebM {mb}");
    // GIF 480p chosen (852×480 → 720×406, 15 fps), 4.0 s: 11.1 MB (2 MB before)
    (s.format, s.width, s.height) = (Format::Gif, 852, 480);
    s.range = Some((Time::from_secs_f64(5.0), Time::from_secs_f64(9.0)));
    let (mb, exact) = estimate_mb(&s, Time::ZERO);
    assert!(within(mb, 11.08) && !exact, "GIF {mb}");
    // the dialog shows the same size the export writes
    assert_eq!(s.output_size(), (720, 406));
    // portrait Shorts H.264 „Höher“, 18 s: 17.6 MB – still images, CRF estimates too high here;
    // only roughly plausible (not an order of magnitude off)
    (s.format, s.codec, s.quality, s.width, s.height) = (Format::Mp4, Codec::H264, Quality::Higher, 1080, 1920);
    s.range = None;
    let (mb, _) = estimate_mb(&s, Time::from_secs_f64(18.0));
    assert!(mb > 17.6 && mb < 17.6 * 10.0, "Shorts {mb}");
}

#[test]
fn every_text_style_reaches_the_export_like_the_preview() {
    use cl_core::{Clip, ClipContent, TextStyle, Track, TrackKind};
    let styles: Vec<(String, TextStyle)> = crate::captions::PRESETS
        .iter()
        .map(|c| (c.name.to_string(), (c.style)()))
        .chain(cl_fx::text::TEMPLATES.iter().map(|t| (t.name.to_string(), t.make())))
        .collect();
    assert!(styles.iter().any(|(_, s)| s.italic) && styles.iter().any(|(_, s)| s.bold));
    let dir = tempfile::tempdir().unwrap();
    let none = |_: Option<&str>| None;
    for (name, mut st) in styles {
        // subtitle styles have no text; empty lines are not drawn
        if st.text.trim().is_empty() {
            st.text = "Probe".into();
        }
        let mut p = Project::new("t");
        let mut t = Track::new(TrackKind::Text);
        t.clips.push(Clip::new(ClipContent::Text(st.clone()), Time::ZERO, Time::from_secs_f64(1.0)));
        p.timeline.tracks.push(t);
        let job = cl_export::build(&p, &ExportSettings::from_project(&p), &none, &dir.path().join("o.mp4")).unwrap();
        let g = std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap();
        let [r, gr, b, a] = st.color;
        assert!(g.contains(&format!("fontcolor=0x{r:02x}{gr:02x}{b:02x}@{:.3}", a as f32 / 255.0)), "{name}: colour");
        // face from the same toggles as the preview: bold = second print, italic = shear
        // one print per line; karaoke: second version in the highlight colour
        let zeilen = st.text.lines().filter(|l| !l.trim().is_empty()).count();
        let fassungen = if st.karaoke.is_some() { 2 } else { 1 };
        assert_eq!(g.matches("drawtext=").count(), zeilen * fassungen * if st.bold { 2 } else { 1 }, "{name}: bold");
        assert_eq!(g.contains("shear=shx=0.25"), st.italic, "{name}: italic");
        assert_eq!(g.contains(":borderw="), st.outline.is_some(), "{name}: outline");
        // box as one rectangle around all lines
        assert_eq!(g.contains("drawbox="), st.background.is_some(), "{name}: background");
        assert_eq!(g.contains("shadowx="), st.shadow, "{name}: shadow");
    }
}

#[test]
fn cover_fills_portrait_frame_without_bars() {
    // 16:9 image in a 9:16 area: centred vertical strip, full height
    let uv = fill_uv(vec2(1920.0, 1080.0), vec2(250.0, 444.0));
    assert!((uv.height() - 1.0).abs() < 1e-4 && (uv.width() - 0.3166).abs() < 1e-3 && (uv.center().x - 0.5).abs() < 1e-6, "{uv:?}");
    // same format: whole image
    assert_eq!(fill_uv(vec2(1920.0, 1080.0), vec2(250.0, 140.625)), Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)));
}

#[test]
fn estimate_matches_a_real_1080p30_export() {
    let mut s = ExportSettings::from_project(&Project::new("t"));
    (s.width, s.height, s.fps) = (1920, 1080, cl_core::Rational::parse("30").unwrap());
    let (mb, exact) = estimate_mb(&s, Time::from_secs_f64(40.0));
    assert!(!exact);
    // real file: 117.6 MB
    assert!((80.0..=160.0).contains(&mb), "{mb}");
}

#[cfg(test)]
fn frame(d: &mut ExportDialog, ctx: &egui::Context, p: &Project, enter: bool) {
    frame_toast(d, ctx, p, enter);
}

#[test]
fn cancel_while_preparing_ends_at_once_without_error() {
    let (ctx, p) = (egui::Context::default(), Project::new("t"));
    let mut d = ExportDialog::default();
    // build still running (no job, no result): cancel takes effect in the next frame
    d.running = Some(running(None));
    frame(&mut d, &ctx, &p, false);
    assert!(d.running.is_some());
    d.running.as_ref().unwrap().cancel.store(true, Ordering::Relaxed);
    frame(&mut d, &ctx, &p, false);
    assert!(d.running.is_none() && d.error.is_none());
    // abort while FFmpeg runs: the thread reports „Abgebrochen“, which is not an error
    let r = running(Some(Err("Abgebrochen".into())));
    r.cancel.store(true, Ordering::Relaxed);
    d.running = Some(r);
    frame(&mut d, &ctx, &p, false);
    assert!(d.running.is_none() && d.error.is_none());
    // a real error stays one
    d.running = Some(running(Some(Err("FFmpeg: kaputt".into()))));
    frame(&mut d, &ctx, &p, false);
    assert_eq!(d.error.as_deref(), Some("FFmpeg: kaputt"));
}

#[test]
fn in_out_range_is_preselected_on_every_open() {
    let p = Project::new("t");
    let mut d = ExportDialog::default();
    let r = (Time::from_secs_f64(2.0), Time::from_secs_f64(5.0));
    d.open_for(&p, None, 0, Some(r));
    assert_eq!(d.settings.as_ref().unwrap().range, Some(r));
    // range deleted meanwhile: no old range any more
    d.open_for(&p, None, 0, None);
    assert_eq!((d.settings.as_ref().unwrap().range, d.range_avail), (None, None));
}

#[test]
fn enter_that_opened_overwrite_prompt_does_not_confirm_it() {
    let (ctx, p) = (egui::Context::default(), Project::new("t"));
    let mut d = ExportDialog::default();
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    d.open_for(&p, None, 0, None);
    // as after Enter on „Exportieren“: question opened in the same frame, Enter still pressed
    (d.confirm_overwrite, d.confirm_fresh) = (true, true);
    frame(&mut d, &ctx, &p, true);
    assert!(d.confirm_overwrite && d.error.is_none());
    // only a new Enter confirms – with „Anderen Namen“, never overwrite
    // (empty project: start fails with an error)
    frame(&mut d, &ctx, &p, true);
    assert!(!d.confirm_overwrite);
    finish(&mut d, &ctx, &p);
    assert!(d.error.is_some());
    assert!(d.name.starts_with("t ("), "{}", d.name);
}

/// Draw frames until the export thread is done
#[cfg(test)]
fn finish(d: &mut ExportDialog, ctx: &egui::Context, p: &Project) {
    let t0 = Instant::now();
    while d.running.is_some() && t0.elapsed().as_secs() < 30 {
        std::thread::yield_now();
        frame(d, ctx, p, false);
    }
}

#[test]
fn export_job_is_built_off_the_ui_thread() {
    // large draft: the build took seconds on the UI thread (audio check per file).
    // Here the build fails (empty project) – the result still comes from the thread only.
    let (ctx, p) = (egui::Context::default(), Project::new("t"));
    let dir = tempfile::tempdir().unwrap();
    let mut d = ExportDialog::default();
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    d.open_for(&p, None, 0, None);
    d.folder = dir.path().to_path_buf();
    frame(&mut d, &ctx, &p, true);
    assert!(d.running.is_some() && d.error.is_none(), "job built in the caller");
    finish(&mut d, &ctx, &p);
    assert_eq!(d.error.as_deref(), Some("Das Projekt ist leer"));
}

#[test]
fn reexport_only_with_unchanged_settings() {
    let (dir, p) = (tempfile::tempdir().unwrap(), Project::new("t"));
    let mut d = ExportDialog { folder: dir.path().to_path_buf(), name: "t".into(), ..Default::default() };
    let s = ExportSettings::from_project(&p);
    let out = d.output_path(s.format);
    std::fs::write(&out, b"x").unwrap();
    d.exported = Some((0, out, s.clone()));
    assert!(d.unchanged_since_export(&s));
    // other quality: „Exportieren“ again (asks if the file exists)
    let mut q = s.clone();
    q.quality = Quality::Bitrate(1234);
    assert!(!d.unchanged_since_export(&q));
    d.rev = 1;
    assert!(!d.unchanged_since_export(&s));
}

#[test]
fn live_preview_reload_blend_and_partial_jpeg() {
    let t = std::time::SystemTime::UNIX_EPOCH;
    assert!(!changed(None, None) && !changed(Some((t, 5)), None) && !changed(Some((t, 5)), Some((t, 5))));
    assert!(changed(None, Some((t, 5))) && changed(Some((t, 5)), Some((t, 6))));
    assert_eq!((blend(0.0, 0.25), blend(0.125, 0.25), blend(1.0, 0.25), blend(0.0, 0.0)), (0.0, 0.5, 1.0, 1.0));
    let mut jpg = Vec::new();
    image::RgbImage::from_pixel(48, 27, image::Rgb([200, 30, 30]))
        .write_to(&mut std::io::Cursor::new(&mut jpg), image::ImageFormat::Jpeg)
        .unwrap();
    assert_eq!(decode_jpeg(&jpg).unwrap().size, [48, 27]);
    // half written file: skipped
    assert!(decode_jpeg(&jpg[..jpg.len() / 2]).is_none() && decode_jpeg(&[]).is_none());
    // file still missing / unchanged: no frame, nothing loaded
    let ctx = egui::Context::default();
    let dir = tempfile::tempdir().unwrap();
    let pv = dir.path().join("preview.jpg");
    let mut live = Live::default();
    live.poll(&ctx, &pv);
    assert!(live.cur.is_none());
    std::fs::write(&pv, &jpg).unwrap();
    live.checked = None;
    live.poll(&ctx, &pv);
    assert!(live.cur.is_some() && live.prev.is_none());
    // overwritten broken: the last good frame stays
    std::fs::write(&pv, &jpg[..10]).unwrap();
    live.checked = None;
    live.poll(&ctx, &pv);
    assert!(live.cur.is_some() && live.prev.is_none());
}

#[cfg(test)]
fn queued(p: &Project, out: &Path) -> Queued {
    Queued {
        project: Arc::new(p.clone()),
        project_path: None,
        settings: ExportSettings::from_project(p),
        out: out.to_path_buf(),
        parts: vec![Part { range: None, out: out.to_path_buf(), sidecars: Vec::new() }],
        fonts: HashMap::new(),
        rev: 0,
        clean: false,
    }
}

#[cfg(test)]
fn running(result: Option<Result<(), String>>) -> Running {
    Running {
        progress: Default::default(),
        part: Default::default(),
        finished: Default::default(),
        job: Default::default(),
        result: Arc::new(Mutex::new(result)),
        cancel: Default::default(),
        started: Instant::now(),
        src: queued(&Project::new("t"), Path::new("t.mp4")),
        live: Live::default(),
    }
}

#[test]
fn window_close_during_export_asks_and_cancels() {
    assert!(close_allowed(false, false) && close_allowed(true, true) && !close_allowed(true, false));
    let ctx = egui::Context::default();
    let close = || {
        let mut raw = egui::RawInput::default();
        raw.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo { events: vec![egui::ViewportEvent::Close], ..Default::default() },
        );
        raw
    };
    // without export: no holding off
    let mut d = ExportDialog::default();
    let out = ctx.run(close(), |ctx| d.guard_close(ctx));
    assert!(!d.quit_ask && !out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::CancelClose));
    // with a running export: closing held off, question open
    d.running = Some(running(None));
    let out = ctx.run(close(), |ctx| d.guard_close(ctx));
    assert!(d.quit_ask && out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::CancelClose));
    // quitting confirmed: the second close request goes through
    d.quit_ok = true;
    d.quit_ask = false;
    let out = ctx.run(close(), |ctx| d.guard_close(ctx));
    assert!(!d.quit_ask && !out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::CancelClose));
    // quitting waits until the export thread has reported the abort
    let r = running(None);
    let (cancel, result) = (r.cancel.clone(), r.result.clone());
    d.running = Some(r);
    std::thread::spawn(move || {
        while !cancel.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        *result.lock().unwrap() = Some(Err("Abgebrochen".into()));
    });
    d.cancel_and_wait(std::time::Duration::from_secs(5));
    assert!(d.running.as_ref().unwrap().result.lock().unwrap().is_some());
}

#[test]
fn sidecars_follow_range_and_wait_for_success() {
    use cl_core::{Clip, ClipContent, Marker, TextStyle, Track, TrackKind};
    let mut p = Project::new("t");
    let mut t = Track::new(TrackKind::Text);
    let txt = |s: &str| ClipContent::Text(TextStyle { text: s.into(), ..Default::default() });
    t.clips.push(Clip::new(txt("drin"), Time::from_secs_f64(50.0), Time::from_secs_f64(20.0)));
    t.clips.push(Clip::new(txt("draußen"), Time::from_secs_f64(130.0), Time::from_secs_f64(5.0)));
    p.timeline.tracks.push(t);
    let mk = |id, at: f64, name: &str| Marker { id, at: Time::from_secs_f64(at), duration: Time::ZERO, name: name.into(), note: String::new(), color: 0 };
    p.timeline.markers = vec![mk(1, 75.0, "Mitte"), mk(2, 125.0, "Später")];
    let mut s = ExportSettings::from_project(&p);
    s.range = Some((Time::from_secs_f64(60.0), Time::from_secs_f64(120.0)));
    let dir = tempfile::tempdir().unwrap();
    let v = sidecars(&p, &s, true, &dir.path().join("a.mp4"));
    let get = |ext: &str| v.iter().find(|(f, _)| f.to_string_lossy().ends_with(ext)).unwrap().1.clone();
    assert_eq!(get(".chapters.txt"), "0:00 Intro\n0:15 Mitte");
    // chapters switched off: no chapter file, subtitles stay
    let off = sidecars(&p, &ExportSettings { chapters: false, ..s.clone() }, true, &dir.path().join("a.mp4"));
    assert_eq!(off.len(), 2);
    assert!(off.iter().all(|(f, _)| !f.to_string_lossy().ends_with(".chapters.txt")));
    assert_eq!(get(".srt"), "1\n00:00:00,000 --> 00:00:10,000\ndrin\n");
    // write side files only after a successful export (success records in `exports.json`)
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("CUTLINE_HOME", dir.path());
    let (ctx, mut d) = (egui::Context::default(), ExportDialog::default());
    for ok in [false, true] {
        let r = running(Some(if ok { Ok(()) } else { Err("x".into()) }));
        let out = dir.path().join("a.mp4");
        let parts = vec![Part { range: s.range, out: out.clone(), sidecars: v.clone() }];
        // finished is reported per part
        r.finished.lock().unwrap().extend(ok.then_some(0));
        d.running = Some(Running { src: Queued { parts, settings: s.clone(), out, ..r.src }, ..r });
        frame(&mut d, &ctx, &p, false);
        assert_eq!(v.iter().all(|(f, _)| f.is_file()), ok);
    }
    std::env::remove_var("CUTLINE_HOME");
}

#[test]
fn queue_takes_turns_names_limit_and_keeps_its_snapshot() {
    let (ctx, dir, p) = (egui::Context::default(), tempfile::tempdir().unwrap(), Project::new("t"));
    let mut d = ExportDialog::default();
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    d.open_for(&p, None, 0, None);
    d.folder = dir.path().to_path_buf();
    // one already running: „Exportieren“ (Enter) queues at the back instead of being rejected
    d.running = Some(running(None));
    frame(&mut d, &ctx, &p, true);
    assert_eq!(d.queue.len(), 1);
    assert_eq!(d.queue[0].out, dir.path().join("t.mp4"));
    // same target path as a queued one: question as for an existing file,
    // Enter chooses „Anderen Namen“
    frame(&mut d, &ctx, &p, true);
    assert!(d.confirm_overwrite && d.queue.len() == 1);
    frame(&mut d, &ctx, &p, true);
    assert!(!d.confirm_overwrite);
    assert_eq!(d.queue.iter().map(|q| q.out.clone()).collect::<Vec<_>>(), [dir.path().join("t.mp4"), dir.path().join("t (2).mp4")]);
    // the open project changes: the snapshot stays
    let mut changed = p.clone();
    changed.name = "geändert".into();
    frame(&mut d, &ctx, &changed, false);
    assert!(d.queue.iter().all(|q| *q.project == p));
    // remove a single waiting one
    d.queue.push_back(queued(&p, &dir.path().join("c.mp4")));
    d.remove_waiting(Some(1));
    assert_eq!(d.waiting().map(|(f, _, _)| f.to_path_buf()).collect::<Vec<_>>(), [dir.path().join("t.mp4"), dir.path().join("c.mp4")]);
    // limit: neither Enter nor the button queue any more
    while d.queue.len() < QUEUE_MAX {
        d.queue.push_back(queued(&p, &dir.path().join(format!("{}.mp4", d.queue.len()))));
    }
    d.name = "frei".into();
    frame(&mut d, &ctx, &p, true);
    assert!(d.queue.len() == QUEUE_MAX && !d.confirm_overwrite);
    // „Alle wartenden entfernen“: the running one stays
    d.remove_waiting(None);
    assert!(d.queue.is_empty() && d.running.is_some());
    // cancelling the running one starts the next; failures do not stop the queue
    // (empty project: the build fails at once)
    d.queue.extend([queued(&p, &dir.path().join("x.mp4")), queued(&p, &dir.path().join("y.mp4"))]);
    d.cancel_current();
    frame(&mut d, &ctx, &p, false);
    assert_eq!((d.current().map(|c| c.0), d.queue.len()), (Some("x.mp4".to_string()), 1));
    finish(&mut d, &ctx, &p);
    assert!(d.running.is_none() && d.queue.is_empty());
    assert_eq!(d.error.as_deref(), Some("Das Projekt ist leer"));
    // quitting with waiting exports: the question names the number, closing clears the queue
    d.running = Some(running(None));
    d.queue.push_back(queued(&p, &dir.path().join("z.mp4")));
    let mut raw = egui::RawInput::default();
    raw.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { events: vec![egui::ViewportEvent::Close], ..Default::default() });
    let _ = ctx.run(raw, |ctx| d.guard_close(ctx));
    assert!(d.quit_ask);
    d.running.as_ref().unwrap().result.lock().unwrap().replace(Err("Abgebrochen".into()));
    d.cancel_and_wait(std::time::Duration::from_secs(1));
    assert!(d.queue.is_empty());
}

#[test]
fn queued_exports_run_in_turn_from_their_own_snapshot() {
    use cl_core::{Clip, ClipContent, TextStyle, Track, TrackKind};
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let project = |name: &str, txt: &str, secs: f64| {
        let mut p = Project::new(name);
        (p.settings.width, p.settings.height) = (320, 180);
        let mut t = Track::new(TrackKind::Text);
        t.clips.push(Clip::new(ClipContent::Text(TextStyle { text: txt.into(), ..Default::default() }), Time::ZERO, Time::from_secs_f64(secs)));
        p.timeline.tracks.push(t);
        p
    };
    let (ctx, mut d) = (egui::Context::default(), ExportDialog::default());
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    let (p, pa) = (project("A", "eins", 1.0), dir.path().join("A.cutline"));
    d.open_for(&p, Some(&pa), 0, None);
    d.folder = dir.path().to_path_buf();
    d.also_srt = true;
    // an export (never finished) runs: both versions wait
    d.running = Some(running(None));
    frame(&mut d, &ctx, &p, true);
    d.name = "A kurz".into();
    frame(&mut d, &ctx, &p, true);
    assert_eq!(d.queue.len(), 2);
    // another project open and longer, only then do the two run
    let other = project("Anders", "zwei", 3.0);
    let po = dir.path().join("Anders.cutline");
    d.open_for(&other, Some(&po), 7, None);
    d.cancel_current();
    let t0 = Instant::now();
    while (d.running.is_some() || !d.queue.is_empty()) && t0.elapsed().as_secs() < 60 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        frame(&mut d, &ctx, &other, false);
    }
    assert_eq!(d.error, None);
    for f in ["A.mp4", "A kurz.mp4"] {
        let m = cl_media::probe(&dir.path().join(f)).unwrap();
        // length and size from the snapshot (1 s, 320×180), not from the open project (3 s)
        assert!((m.duration.as_secs_f64() - 1.0).abs() < 0.2 && m.width == 320, "{f}: {:?}", m.duration);
    }
    // side file from the snapshot
    assert!(std::fs::read_to_string(dir.path().join("A kurz.srt")).unwrap().contains("eins"));
    // exports list: assigned to the project they come from
    assert_eq!(crate::exports::for_project("A", Some(&pa)).len(), 2);
    assert!(crate::exports::for_project("Anders", Some(&po)).is_empty());
    std::env::remove_var("CUTLINE_HOME");
}

/// Test project 320×180, one text over `secs` seconds
#[cfg(test)]
fn text_project(name: &str, txt: &str, secs: f64) -> Project {
    use cl_core::{Clip, ClipContent, TextStyle, Track, TrackKind};
    let mut p = Project::new(name);
    (p.settings.width, p.settings.height) = (320, 180);
    let mut t = Track::new(TrackKind::Text);
    t.clips.push(Clip::new(ClipContent::Text(TextStyle { text: txt.into(), ..Default::default() }), Time::ZERO, Time::from_secs_f64(secs)));
    p.timeline.tracks.push(t);
    p
}

/// Dialog with „In Teile aufteilen“ (count) for `p`, target `dir`
#[cfg(test)]
fn split_dialog(p: &Project, dir: &Path, n: u32) -> ExportDialog {
    let mut d = ExportDialog::default();
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    d.open_for(p, Some(&dir.join("p.cutline")), 0, None);
    d.folder = dir.to_path_buf();
    (d.split_on, d.split_by, d.split_count) = (true, 0, n);
    d
}

/// Draw frames until export and queue are empty
#[cfg(test)]
fn drain(d: &mut ExportDialog, ctx: &egui::Context, p: &Project) {
    let t0 = Instant::now();
    while (d.running.is_some() || !d.queue.is_empty()) && t0.elapsed().as_secs() < 120 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        frame(d, ctx, p, false);
    }
}

#[test]
fn split_export_writes_parts_into_a_folder_as_one_entry() {
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    let p = text_project("Film", "eins", 4.0);
    let mut d = split_dialog(&p, dir.path(), 4);
    d.also_srt = true;
    // one already running: the group waits as one entry
    d.running = Some(running(None));
    frame(&mut d, &ctx, &p, true);
    assert_eq!(d.queue.len(), 1);
    assert_eq!(d.waiting().map(|(f, _, n)| (f.to_path_buf(), n)).collect::<Vec<_>>(), [(dir.path().join("Film"), 4)]);
    d.cancel_current();
    frame(&mut d, &ctx, &p, false);
    assert_eq!(d.current().unwrap().0, "Film · Teil 1 von 4");
    drain(&mut d, &ctx, &p);
    assert_eq!(d.error, None);
    let folder = dir.path().join("Film");
    let mut names: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).filter(|n| n.ends_with(".mp4")).collect();
    names.sort();
    assert_eq!(names, ["Film – Teil 01.mp4", "Film – Teil 02.mp4", "Film – Teil 03.mp4", "Film – Teil 04.mp4"]);
    for n in &names {
        let m = cl_media::probe(&folder.join(n)).unwrap();
        assert!((m.duration.as_secs_f64() - 1.0).abs() < 0.1, "{n}: {:?}", m.duration);
    }
    // subtitles per part, times from the part start
    assert_eq!(std::fs::read_to_string(folder.join("Film – Teil 03.srt")).unwrap(), "1\n00:00:00,000 --> 00:00:01,000\neins\n");
    // exports list: every part
    assert_eq!(crate::exports::for_project("Film", Some(&dir.path().join("p.cutline"))).len(), 4);
    // once more: one question for the whole group, „Anderen Namen“ (Enter) → new folder
    frame(&mut d, &ctx, &p, true);
    assert!(d.confirm_overwrite && d.queue.is_empty());
    frame(&mut d, &ctx, &p, true);
    assert!(!d.confirm_overwrite);
    assert_eq!(d.running.as_ref().map(|r| r.src.out.clone()), Some(dir.path().join("Film (2)")));
    drain(&mut d, &ctx, &p);
    assert!(dir.path().join("Film (2)").join("Film (2) – Teil 04.mp4").is_file());
    std::env::remove_var("CUTLINE_HOME");
}

#[test]
fn failed_part_does_not_stop_the_others() {
    use cl_core::{Clip, ClipContent, MediaKind, Track, TrackKind};
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    // part 3 (2–3 s) needs a video that is missing by now
    let mut p = text_project("Film", "eins", 4.0);
    let v = dir.path().join("weg.mp4");
    assert!(cl_media::ffmpeg_command().unwrap().args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=red:size=320x180:rate=30:duration=1"]).arg(&v).status().unwrap().success());
    let m = cl_media::probe(&v).unwrap();
    std::fs::remove_file(&v).unwrap();
    let mut t = Track::new(TrackKind::Video);
    t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::from_secs_f64(2.0), Time::from_secs_f64(1.0)));
    p.media.push(m);
    p.timeline.tracks.push(t);
    let mut d = split_dialog(&p, dir.path(), 4);
    frame(&mut d, &ctx, &p, true);
    drain(&mut d, &ctx, &p);
    let e = d.error.clone().unwrap_or_default();
    assert!(e.starts_with("3 von 4 Teilen fertig: ") && e.contains("weg.mp4"), "{e}");
    let folder = dir.path().join("Film");
    for (n, there) in [(1, true), (2, true), (3, false), (4, true)] {
        assert_eq!(folder.join(format!("Film – Teil 0{n}.mp4")).is_file(), there, "part {n}");
    }
    std::env::remove_var("CUTLINE_HOME");
}

#[test]
fn cancel_stops_the_whole_group_without_half_files() {
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    let p = text_project("Film", "eins", 8.0);
    let folder = dir.path().join("Film");
    // cancelled at once (during preparation): `poll` reports right away, the thread cleans up only
    // afterwards. A new export started at once into the same folder must not be hit by this cleanup
    // (before: folder gone, all parts failed)
    let mut d = split_dialog(&p, dir.path(), 8);
    frame(&mut d, &ctx, &p, true);
    d.cancel_current();
    drain(&mut d, &ctx, &p);
    assert!(d.error.is_none());
    // right away again: cancelled after the first part → finished parts stay, no half file, rest missing
    frame(&mut d, &ctx, &p, true);
    let t0 = Instant::now();
    while d.running.as_ref().is_some_and(|r| r.finished.lock().unwrap().is_empty()) && t0.elapsed().as_secs() < 60 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    d.cancel_current();
    drain(&mut d, &ctx, &p);
    assert!(d.error.is_none(), "{:?}", d.error);
    let names: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert!(names.contains(&"Film – Teil 01.mp4".to_string()) && !names.contains(&"Film – Teil 08.mp4".to_string()), "{names:?}");
    assert!(names.iter().all(|n| !n.contains(".part")), "{names:?}");
    std::env::remove_var("CUTLINE_HOME");
}

/// Height of the export window without a real window: draw two frames in the program window `w`×`h`
/// (the first does not know the content height yet). Returns the window rect and (visible height,
/// content height) of the settings on the right.
#[cfg(test)]
fn dialog_layout(d: &mut ExportDialog, p: &Project, w: f32, h: f32) -> (Rect, (f32, f32)) {
    let ctx = egui::Context::default();
    // opening animation off
    ctx.style_mut(|s| s.animation_time = 0.0);
    for _ in 0..4 {
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(w, h))), ..Default::default() };
        let _ = ctx.run(raw, |ctx| {
            d.show(ctx, p, &HashMap::new(), None, 0);
        });
    }
    (ctx.memory(|m| m.area_rect(egui::Id::new("export-dialog"))).expect("window drawn"), d.right_view)
}

#[test]
fn export_dialog_shows_all_settings_when_there_is_room() {
    let mut p = text_project("Film", "eins", 600.0);
    (p.settings.width, p.settings.height) = (1920, 1080);
    let mut d = ExportDialog::default();
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    d.open_for(&p, None, 0, None);
    // full version: parts with result line, target size with hint („wird in 360p exportiert“)
    d.split_on = true;
    d.settings.as_mut().unwrap().target_mb = Some(10);
    // other aspect ratio: rows „Seitenverhältnis“ and „Bild“
    d.aspect = 2;
    // 1440×900: everything visible, nothing scrolls
    let (win, (shown, content)) = dialog_layout(&mut d, &p, 1440.0, 900.0);
    assert!(content > 400.0, "{content}");
    assert!(shown >= content - 1.0, "right column scrolls: {shown} of {content}");
    assert!(win.bottom() <= 900.0 && win.top() >= 0.0, "{win:?}");
    // 1280×720: scrolls, but the window (title and buttons) stays in the program window
    let (win, (shown, content)) = dialog_layout(&mut d, &p, 1280.0, 720.0);
    assert!(win.top() >= 0.0 && win.bottom() <= 720.0, "{win:?}");
    assert!(shown > 300.0 && shown <= content, "{shown} of {content}");
}

#[test]
fn overwriting_a_group_finds_only_its_own_older_parts() {
    let dir = tempfile::tempdir().unwrap();
    let d = &dir.path().join("Film");
    std::fs::create_dir(d).unwrap();
    let files = [
        "Film – Teil 01.mp4", "Film – Teil 04.mp4", "Film – Teil 05.mp4", "Film – Teil 06.mp4", "Film – Teil 06.srt",
        "Film – Teil 06.vtt", "Film – Teil 06.chapters.txt", "Film – Part 07.mp4", "Film – 08 Ende.mp4", "Film – 09.mp4",
        // foreign: other extension, other name, suffix, single-digit number, notes
        "Film – Teil 06.mov", "Film – Teil 06 Kopie.mp4", "Anderer – Teil 06.mp4", "Film – Teil 6.mp4", "Notizen.txt", "Film.mp4",
        "Film – 10 .mp4",
        // fits the pattern but not written by Cutline (own file, part without entry)
        "Film – 2023 Sommer.mp4", "Film – Teil 12.mp4", "Film – Teil 12.srt",
    ];
    for f in files {
        std::fs::write(d.join(f), b"x").unwrap();
    }
    std::fs::create_dir(d.join("Film – Teil 11.mp4")).unwrap();
    let keep: Vec<PathBuf> = (1..=4).map(|i| d.join(format!("Film – Teil 0{i}.mp4"))).collect();
    // exports list: all videos except the two foreign ones
    let recorded: Vec<PathBuf> = files.iter().filter(|f| f.ends_with(".mp4") && !["Film – 2023 Sommer.mp4", "Film – Teil 12.mp4"].contains(f)).map(|f| d.join(f)).collect();
    let stale: Vec<String> = stale_parts(d, "mp4", &keep, &recorded).iter().map(|f| f.file_name().unwrap().to_string_lossy().to_string()).collect();
    assert_eq!(
        stale,
        ["Film – 08 Ende.mp4", "Film – 09.mp4", "Film – Part 07.mp4", "Film – Teil 05.mp4", "Film – Teil 06.chapters.txt", "Film – Teil 06.mp4", "Film – Teil 06.srt", "Film – Teil 06.vtt"]
    );
    // side file of a remaining part stays
    std::fs::write(d.join("Film – Teil 01.srt"), b"x").unwrap();
    assert!(!stale_parts(d, "mp4", &keep, &recorded).iter().any(|f| f.ends_with("Film – Teil 01.srt")));
    // without folder: nothing
    assert!(stale_parts(&d.join("fehlt"), "mp4", &keep, &recorded).is_empty());
}

/// Overwriting a group: older parts are removed only by the new export when its first part
/// begins – a previously running or waiting export of the same group would otherwise write them again.
#[test]
fn overwrite_removes_old_parts_when_its_export_starts() {
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    let p = text_project("Film", "eins", 2.0);
    let mut d = split_dialog(&p, dir.path(), 2);
    let folder = dir.path().join("Film");
    std::fs::create_dir(&folder).unwrap();
    for f in ["Film – Teil 03.mp4", "Film – Teil 03.srt", "Film – 2023 Sommer.mp4"] {
        std::fs::write(folder.join(f), b"x").unwrap();
    }
    let pp = dir.path().join("p.cutline");
    crate::exports::record("Film", Some(&pp), &folder.join("Film – Teil 03.mp4"));
    // another export is running: the overwriting one waits, nothing removed yet
    d.running = Some(running(None));
    let (out, parts) = d.targets(&p, d.settings.as_ref().unwrap());
    let parts = parts.into_iter().map(|(range, out)| Part { range, out, sidecars: Vec::new() }).collect();
    d.queue.push_back(Queued { parts, project_path: Some(pp), clean: true, ..queued(&p, &out) });
    assert!(folder.join("Film – Teil 03.mp4").is_file());
    d.cancel_current();
    drain(&mut d, &ctx, &p);
    assert_eq!(d.error, None);
    let mut names: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    names.sort();
    assert_eq!(names, ["Film – 2023 Sommer.mp4", "Film – Teil 01.mp4", "Film – Teil 02.mp4"]);
    std::env::remove_var("CUTLINE_HOME");
}

#[test]
fn cleanup_of_a_cancelled_group_spares_a_newer_export_in_the_same_folder() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("Film");
    std::fs::create_dir(&f).unwrap();
    // old export takes it, new one takes it, the old one cleans up afterwards: folder stays (still empty)
    folder_use(&f, true, false);
    folder_use(&f, true, false);
    folder_use(&f, false, true);
    assert!(f.is_dir());
    // the last one releases: empty folder gone; not empty: stays
    folder_use(&f, false, true);
    assert!(!f.exists());
    std::fs::create_dir(&f).unwrap();
    std::fs::write(f.join("x.mp4"), b"x").unwrap();
    folder_use(&f, true, false);
    folder_use(&f, false, true);
    assert!(f.join("x.mp4").is_file());
}

/// The parts folder is never the chosen folder itself, and a cancel removes only a folder
/// the export created.
#[test]
fn split_folder_is_never_the_chosen_folder_and_survives_cancel() {
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    let p = text_project("Film", "eins", 8.0);
    let chosen = dir.path().join("Ziel");
    std::fs::create_dir(&chosen).unwrap();
    let mut d = split_dialog(&p, &chosen, 8);
    for n in ["", "  ", ".", "..", "..."] {
        d.name = n.into();
        let (out, parts) = d.targets(&p, d.settings.as_ref().unwrap());
        assert_eq!(out, chosen.join("Export"), "{n:?}");
        assert!(parts.iter().all(|f| f.1.parent() == Some(out.as_path())), "{n:?}");
    }
    // existing empty folder with the export name: still there after an immediate cancel
    d.name = "Film".into();
    std::fs::create_dir(chosen.join("Film")).unwrap();
    frame(&mut d, &ctx, &p, true);
    d.cancel_current();
    drain(&mut d, &ctx, &p);
    assert!(chosen.join("Film").is_dir());
    std::env::remove_var("CUTLINE_HOME");
}

/// Target size applies per part: bitrate from the length of the part.
#[test]
fn target_size_applies_per_part() {
    use cl_core::{Clip, ClipContent, MediaKind, Track, TrackKind};
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    // strong noise (otherwise the encoder undercuts any bitrate) and sound (the planned
    // share would otherwise stay unused)
    let v = dir.path().join("rauschen.mp4");
    let st = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=480x270:rate=30:duration=4,noise=alls=40:allf=t"])
        .args(["-f", "lavfi", "-i", "sine=f=440:d=4"])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "8", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&v)
        .status()
        .unwrap();
    assert!(st.success());
    let m = cl_media::probe(&v).unwrap();
    let mut p = Project::new("Klein");
    (p.settings.width, p.settings.height) = (480, 270);
    let mut t = Track::new(TrackKind::Video);
    t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(4.0)));
    p.media.push(m);
    p.timeline.tracks.push(t);
    let mut d = split_dialog(&p, dir.path(), 2);
    d.settings.as_mut().unwrap().target_mb = Some(1);
    frame(&mut d, &ctx, &p, true);
    drain(&mut d, &ctx, &p);
    assert_eq!(d.error, None);
    for n in ["Klein – Teil 01.mp4", "Klein – Teil 02.mp4"] {
        let size = std::fs::metadata(dir.path().join("Klein").join(n)).unwrap().len();
        eprintln!("Zielgröße 1 MB je Teil (2 s): {n} {size} Byte");
        assert!((850_000..=1_000_000).contains(&size), "{n}: {size}");
    }
    std::env::remove_var("CUTLINE_HOME");
}

#[test]
fn split_parts_are_gapless_on_frames_with_names() {
    let fps = cl_core::Rational::new(30, 1);
    let s = Time::from_secs_f64;
    let parts = |how, a: f64, b: f64, ms: &[cl_core::Marker]| split_parts(how, (s(a), s(b)), fps, ms, "Film", "Teil", "mp4");
    // gapless, without overlap, sum = range, cuts on frame boundaries
    let check = |v: &[((Time, Time), String)], a: f64, b: f64| {
        assert_eq!((v[0].0 .0, v.last().unwrap().0 .1), (s(a), s(b)));
        for w in v.windows(2) {
            assert_eq!(w[0].0 .1, w[1].0 .0);
            let t = w[0].0 .1;
            assert_eq!(t, t.snap_to_frame(fps), "{t:?}");
            assert!(w[0].0 .0 < w[0].0 .1);
        }
    };
    // count: 10 s (in/out from 1 s) into 3 parts → 3.333 s rounded to frames
    let v = parts(Split::Count(3), 1.0, 11.0, &[]);
    check(&v, 1.0, 11.0);
    assert_eq!(v.iter().map(|p| p.1.as_str()).collect::<Vec<_>>(), ["Film – Teil 01.mp4", "Film – Teil 02.mp4", "Film – Teil 03.mp4"]);
    assert_eq!(v[0].0 .1, Time(4_333_333));
    assert_eq!(split_summary(&v), "3 Teile · je 0:03");
    // at most 50, at least 2 parts; digits grow with the number (at least two)
    assert_eq!(parts(Split::Count(80), 0.0, 100.0, &[]).len(), 50);
    assert_eq!(parts(Split::Count(1), 0.0, 100.0, &[]).len(), 2);
    // 1 s per part over 120 s: at most 50 parts, length raised to 3 s → 40 parts
    let v = parts(Split::Length(1), 0.0, 120.0, &[]);
    check(&v, 0.0, 120.0);
    assert_eq!((v.len(), v[0].1.as_str(), v[39].1.as_str()), (40, "Film – Teil 01.mp4", "Film – Teil 40.mp4"));
    assert_eq!(split_summary(&v), "40 Teile · je 0:03");
    // 2 hours in 1 s parts: not 7199 parts
    assert!(parts(Split::Length(1), 0.0, 7200.0, &[]).len() <= SPLIT_MAX as usize);
    // length: a remainder from 3 s stays its own part, below that it goes to the second-to-last
    let v = parts(Split::Length(60), 0.0, 363.0, &[]);
    check(&v, 0.0, 363.0);
    assert_eq!(v.iter().map(|p| (p.0 .1 - p.0 .0).as_secs_f64()).collect::<Vec<_>>(), [60.0, 60.0, 60.0, 60.0, 60.0, 60.0, 3.0]);
    assert_eq!(split_summary(&v), "7 Teile · 0:03 bis 1:00");
    let v = parts(Split::Length(60), 0.0, 362.9, &[]);
    assert_eq!((v.len(), (v[5].0 .1 - v[5].0 .0).as_secs_f64()), (6, 62.9));
    // shorter than one part: one part
    assert_eq!(parts(Split::Length(60), 0.0, 30.0, &[]).len(), 1);
    // markers: outside and exactly at the end do not count, exactly at the start names part 1;
    // names cleaned, empty → number only, duplicate names stay unique thanks to the number
    let mk = |at: f64, name: &str| cl_core::Marker { id: 0, at: s(at), duration: Time::ZERO, name: name.into(), note: String::new(), color: 0 };
    let ms = [mk(1.0, "Vorher"), mk(2.0, "Start"), mk(4.01, "a/b: c?"), mk(6.0, ""), mk(7.0, "Ende"), mk(7.0, "Ende"), mk(9.0, "Ende"), mk(10.0, "Rand"), mk(12.0, "Danach")];
    let v = parts(Split::Markers, 2.0, 10.0, &ms);
    check(&v, 2.0, 10.0);
    assert_eq!(
        v.iter().map(|p| p.1.as_str()).collect::<Vec<_>>(),
        ["Film – 01 Start.mp4", "Film – 02 a_b_ c_.mp4", "Film – 03.mp4", "Film – 04 Ende.mp4", "Film – 05 Ende.mp4"]
    );
    // a marker at 4.01 s snaps to the frame at 4 s
    assert_eq!(v[1].0 .0, s(4.0));
    // device names and control characters
    let v = parts(Split::Markers, 0.0, 10.0, &[mk(5.0, "CON"), mk(6.0, "x\ty")]);
    assert_eq!(v.iter().map(|p| p.1.as_str()).collect::<Vec<_>>(), ["Film – 01.mp4", "Film – 02 CON_.mp4", "Film – 03 x_y.mp4"]);
    // without markers in the range: only one part (choice greyed out)
    assert_eq!(parts(Split::Markers, 2.0, 10.0, &[mk(1.0, "x"), mk(10.0, "y"), mk(2.0, "z")]).len(), 1);
    // 29.97 fps: cuts on the frame grid, range stays exact
    let ntsc = cl_core::Rational::new(30000, 1001);
    let v = split_parts(Split::Count(4), (s(0.0), s(7.0)), ntsc, &[], "F", "Part", "webm");
    assert_eq!((v.len(), v[3].0 .1, v[0].1.as_str()), (4, s(7.0), "F – Part 01.webm"));
    for w in v.windows(2) {
        assert_eq!(w[0].0 .1, w[1].0 .0);
        assert_eq!(w[0].0 .1, w[0].0 .1.snap_to_frame(ntsc));
    }
}

#[test]
fn split_markers_capped_at_split_max() {
    let fps = cl_core::Rational::new(30, 1);
    let s = Time::from_secs_f64;
    // 200 markers → 50 parts; markers after the 49th cut do not count, the last part runs to the end
    let ms: Vec<cl_core::Marker> = (1..=200).map(|i| cl_core::Marker { id: i, at: s(i as f64), duration: Time::ZERO, name: String::new(), note: String::new(), color: 0 }).collect();
    let v = split_parts(Split::Markers, (s(0.0), s(210.0)), fps, &ms, "Film", "Teil", "mp4");
    assert_eq!(v.len(), SPLIT_MAX as usize);
    assert_eq!((v[0].0 .0, v[48].0, v[49].0 .1), (s(0.0), (s(48.0), s(49.0)), s(210.0)));
    for w in v.windows(2) {
        assert_eq!(w[0].0 .1, w[1].0 .0);
    }
    assert_eq!((v[0].1.as_str(), v[49].1.as_str()), ("Film – 01.mp4", "Film – 50.mp4"));
    assert_eq!(split_summary(&v), "50 Teile · 0:01 bis 2:41");
}

/// Test image 320×180: red circle (radius 30) in the middle on a grey 4 px checkerboard, 2 s with sound
#[cfg(test)]
fn circle_video(dir: &Path) -> cl_core::MediaRef {
    let path = dir.join("testbild.mp4");
    let gray = "64+128*mod(floor(X/4)+floor(Y/4)\\,2)";
    let inside = "lt(hypot(X-160\\,Y-90)\\,30)";
    let vf = format!("format=rgb24,geq=r='if({inside}\\,255\\,{gray})':g='if({inside}\\,0\\,{gray})':b='if({inside}\\,0\\,{gray})'");
    let st = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y", "-threads", "2", "-f", "lavfi", "-i", "color=c=black:s=320x180:r=30:d=2", "-f", "lavfi", "-i", "sine=f=440:d=2"])
        .args(["-vf", &vf, "-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv444p", "-c:a", "aac", "-shortest"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(st.success());
    cl_media::probe(&path).unwrap()
}

/// 16:9 project 320×180 with the test image filling the frame (length `secs`, clip repeated)
#[cfg(test)]
fn circle_project(dir: &Path, secs: f64) -> Project {
    use cl_core::{Clip, ClipContent, MediaKind, Track, TrackKind};
    let m = circle_video(dir);
    let mut p = Project::new("Kreis");
    (p.settings.width, p.settings.height) = (320, 180);
    let mut t = Track::new(TrackKind::Video);
    t.main = true;
    let mut at = 0.0;
    while at < secs - 1e-6 {
        let len = (secs - at).min(2.0);
        t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::from_secs_f64(at), Time::from_secs_f64(len)));
        at += len;
    }
    p.media.push(m);
    p.timeline.tracks.push(t);
    p
}

/// Frame `n` of a file as RGBA (width, height, pixels)
#[cfg(test)]
fn grab(path: &Path, n: usize) -> (usize, usize, Vec<u8>) {
    let m = cl_media::probe(path).unwrap();
    let o = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-threads", "2", "-i"])
        .arg(path)
        .args(["-vf", &format!("select=eq(n\\,{n})"), "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .output()
        .unwrap();
    let (w, h) = (m.width as usize, m.height as usize);
    assert_eq!(o.stdout.len(), w * h * 4);
    (w, h, o.stdout)
}

/// Mean brightness and fineness (difference of neighbouring pixels) in the area
#[cfg(test)]
fn region(img: &(usize, usize, Vec<u8>), x: std::ops::Range<usize>, y: std::ops::Range<usize>) -> (f64, f64) {
    let (w, _, px) = img;
    let at = |x: usize, y: usize| px[(y * w + x) * 4 + 1] as f64;
    let (mut light, mut d, mut n) = (0.0, 0.0, 0.0);
    for yy in y.clone() {
        for xx in x.clone() {
            light += at(xx, yy);
            d += (at(xx, yy) - at(xx + 1, yy)).abs() + (at(xx, yy) - at(xx, yy + 1)).abs();
            n += 1.0;
        }
    }
    (light / n, d / n)
}

/// Red pixels (circle) in the row range: width and height of their bounding box
#[cfg(test)]
fn red_box(img: &(usize, usize, Vec<u8>), rows: std::ops::Range<usize>) -> (usize, usize) {
    let (w, _, px) = img;
    let (mut x0, mut x1, mut y0, mut y1) = (usize::MAX, 0, usize::MAX, 0);
    for y in rows {
        for x in 0..*w {
            let p = &px[(y * w + x) * 4..];
            if p[0] > 180 && p[1] < 70 && p[2] < 70 {
                (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
            }
        }
    }
    (x1.saturating_sub(x0) + 1, y1.saturating_sub(y0) + 1)
}

/// Export window for `p` to `dir`, aspect ratio 9:16 via the preset „Shorts“ at
/// project size (shorter edge stays 180)
#[cfg(test)]
fn shorts_dialog(p: &Project, dir: &Path) -> ExportDialog {
    let mut d = ExportDialog::default();
    *d.encoders.lock().unwrap() = Some(vec![Encoder::Software]);
    d.open_for(p, Some(&dir.join("p.cutline")), 0, None);
    d.folder = dir.to_path_buf();
    let mut s = d.settings.clone().unwrap();
    d.pick_platform(&mut s, p, platform("shorts").unwrap());
    d.settings = Some(s);
    d.short_edge = 0;
    d
}

#[test]
fn platform_template_sets_the_aspect_and_leaves_the_project_alone() {
    let (ctx, p) = (egui::Context::default(), text_project("Film", "eins", 2.0));
    let mut wide = p.clone();
    (wide.settings.width, wide.settings.height) = (1920, 1080);
    let mut d = ExportDialog::default();
    d.open_for(&wide, None, 0, None);
    let mut s = d.settings.clone().unwrap();
    // Shorts at 16:9: 9:16 with 1080×1920, the project does not remember the platform
    d.pick_platform(&mut s, &wide, platform("shorts").unwrap());
    d.settings = Some(s.clone());
    assert_eq!(ASPECTS[d.aspect - 1], (9, 16));
    assert_eq!(d.take_picked_platform(), None);
    frame(&mut d, &ctx, &wide, false);
    let s = d.settings.clone().unwrap();
    assert_eq!((s.width, s.height), (1080, 1920));
    // Instagram: 4:5; preset in the project format: „Projekt“, remembered in the project
    let mut s = d.settings.clone().unwrap();
    d.pick_platform(&mut s, &wide, platform("instagram").unwrap());
    assert_eq!(ASPECTS[d.aspect - 1], (4, 5));
    d.pick_platform(&mut s, &wide, platform("youtube-1080").unwrap());
    assert_eq!((d.aspect, d.canvas(&wide)), (0, None));
    assert_eq!(d.take_picked_platform(), Some("youtube-1080"));
    d.settings = Some(s);
    frame(&mut d, &ctx, &wide, false);
    let s = d.settings.clone().unwrap();
    assert_eq!((s.width, s.height), (1920, 1080));
    // project with platform „shorts“, but 16:9: 9:16 right on opening
    wide.settings.platform = Some("shorts".into());
    d.open_for(&wide, None, 0, None);
    assert_eq!(d.canvas(&wide), Some((1080, 1920)));
}

/// Preview on the left: cover image from the rebuilt snapshot
#[test]
fn cover_preview_comes_from_the_reframed_snapshot() {
    use cl_core::{Clip, ClipContent, MediaKind, MediaRef, Track, TrackKind};
    let mut p = Project::new("t");
    let m = MediaRef { id: cl_core::MediaId::new(), path: "a.mp4".into(), kind: MediaKind::Video, duration: Time::from_secs_f64(9.0), width: 1920, height: 1080, fps: None, has_audio: true, audio_streams: 1, video_codec: None, audio_codec: None };
    let mut t = Track::new(TrackKind::Video);
    let c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(9.0));
    let id = c.id;
    t.clips.push(c);
    (p.media, p.timeline.tracks) = (vec![m], vec![t]);
    let mut d = ExportDialog::default();
    d.open_for(&p, None, 0, None);
    assert!(d.framed(&p, &HashMap::new()).is_none(), "project format: no rebuild");
    d.aspect = 2;
    let unit = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    // „Füllen“: one layer, covers the area (cropped at the sides)
    let f = d.framed(&p, &HashMap::new()).unwrap();
    assert_eq!((f.settings.width, f.settings.height), (1080, 1920));
    let l = cover_layers(&f, id, Time::ZERO);
    assert_eq!(l.len(), 1);
    assert!(l[0].0.contains_rect(unit) && (l[0].0.height() - 1.0).abs() < 1e-3, "{l:?}");
    // „Einpassen“: blurred, opaque background, the whole picture on top
    d.fit = 2;
    let l = cover_layers(&d.framed(&p, &HashMap::new()).unwrap(), id, Time::ZERO);
    assert_eq!(l.iter().map(|x| x.2).collect::<Vec<_>>(), [true, false]);
    assert!(l[0].0.contains_rect(unit));
    assert!(unit.expand(1e-3).contains_rect(l[1].0) && (l[1].0.width() - 1.0).abs() < 1e-3, "{l:?}");
}

/// Real export 16:9 → 9:16: portrait file, „Füllen“ without bars, „Einpassen“ with
/// blurred background; the circle stays round; the project stays unchanged.
#[test]
fn vertical_export_fills_or_fits_with_a_blurred_background() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    let p = circle_project(dir.path(), 2.0);
    let before = p.clone();
    for fit in [0, 2] {
        let mut d = shorts_dialog(&p, dir.path());
        d.fit = fit;
        d.name = format!("Kreis {fit}");
        frame(&mut d, &ctx, &p, true);
        drain(&mut d, &ctx, &p);
        assert_eq!(d.error, None);
        assert_eq!(d.take_picked_platform(), None, "no history step in the project");
        let img = grab(&dir.path().join(format!("Kreis {fit}.mp4")), 15);
        assert_eq!((img.0, img.1), (180, 320));
        let (top, mid, bottom) = (region(&img, 2..170, 2..60), region(&img, 2..170, 115..140), region(&img, 2..170, 262..316));
        let edges = [region(&img, 0..3, 0..319), region(&img, 176..179, 0..319)];
        eprintln!("fit={fit}: oben {top:?}, Mitte {mid:?}, unten {bottom:?}, Ränder {edges:?}");
        assert!(top.0 > 40.0 && bottom.0 > 40.0, "fit={fit}: top/bottom black");
        assert!(edges.iter().all(|e| e.0 > 40.0), "fit={fit}: sides black");
        if fit == 0 {
            // „Füllen“: the checkerboard everywhere (sharp)
            assert!(top.1 > 20.0 && bottom.1 > 20.0, "fit=0: top/bottom not the picture");
            let (w, h) = red_box(&img, 0..320);
            assert!((95..=115).contains(&w) && w.abs_diff(h) <= 3, "fit=0: circle {w}×{h}");
        } else {
            // „Einpassen“: blurred top/bottom, the sharp picture in the middle
            assert!(top.1 < 8.0 && bottom.1 < 8.0 && mid.1 > 20.0, "fit=2: background not soft");
            let (w, h) = red_box(&img, 112..208);
            assert!((30..=40).contains(&w) && w.abs_diff(h) <= 3, "fit=2: circle {w}×{h}");
        }
    }
    assert_eq!(p, before);
    std::env::remove_var("CUTLINE_HOME");
}

/// Censoring still on the subject after the rebuild (pixel test as in the player)
#[test]
fn censor_stays_on_its_subject_after_reframing() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    let mut p = circle_project(dir.path(), 2.0);
    // source x 136–184, y 18–54 → in the 9:16 image (factor 16/9) about x 47–132, y 32–96
    p.timeline.tracks[0].clips[0].fx.censor =
        vec![cl_core::Censor { path: vec![cl_core::CensorKey { t: Time::ZERO, cx: 0.5, cy: 0.2, w: 0.15, h: 0.2 }], ..Default::default() }];
    let mut d = shorts_dialog(&p, dir.path());
    frame(&mut d, &ctx, &p, true);
    drain(&mut d, &ctx, &p);
    assert_eq!(d.error, None);
    let img = grab(&dir.path().join("Kreis.mp4"), 15);
    let (inside, beside) = (region(&img, 58..122, 40..88), region(&img, 3..36, 40..88));
    eprintln!("Zensur: innen {inside:?}, daneben {beside:?}");
    assert!(inside.1 < 8.0, "pattern still there under the area: {inside:?}");
    assert!(beside.1 > 20.0, "pattern next to the area missing: {beside:?}");
    std::env::remove_var("CUTLINE_HOME");
}

/// Seams: 4 s with frame counter into 4 parts – each frame exactly once, none duplicated or lost
#[test]
fn split_parts_have_every_frame_exactly_once() {
    use cl_core::{Clip, ClipContent, MediaKind, Track, TrackKind};
    if !cl_media::ffmpeg_available() {
        return;
    }
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    // frame number in binary as 8 stripes of 40 px (white = 1, least significant bit on the left)
    let src = dir.path().join("zaehler.mp4");
    let st = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y", "-threads", "2", "-f", "lavfi", "-i", "color=c=black:s=320x180:r=30:d=4"])
        .args(["-vf", "format=gray,geq=lum='if(mod(floor(N/pow(2\\,floor(X/40)))\\,2)\\,255\\,0)'", "-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv444p"])
        .arg(&src)
        .status()
        .unwrap();
    assert!(st.success());
    let m = cl_media::probe(&src).unwrap();
    let mut p = Project::new("Zaehler");
    (p.settings.width, p.settings.height, p.settings.fps) = (320, 180, cl_core::Rational::new(30, 1));
    let mut t = Track::new(TrackKind::Video);
    t.main = true;
    t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(4.0)));
    p.media.push(m);
    p.timeline.tracks.push(t);
    let ctx = egui::Context::default();
    let mut d = split_dialog(&p, dir.path(), 4);
    frame(&mut d, &ctx, &p, true);
    drain(&mut d, &ctx, &p);
    assert_eq!(d.error, None);
    let mut seen = Vec::new();
    for n in 1..=4 {
        let f = dir.path().join("Zaehler").join(format!("Zaehler – Teil {n:02}.mp4"));
        let o = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-threads", "2", "-i"])
            .arg(&f)
            .args(["-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
            .output()
            .unwrap();
        let part: Vec<usize> = o.stdout.chunks_exact(320 * 180).map(|px| (0..8).filter(|b| px[90 * 320 + b * 40 + 20] > 128).map(|b| 1 << b).sum()).collect();
        eprintln!("Teil {n}: {part:?}");
        seen.extend(part);
    }
    assert_eq!(seen, (0..120).collect::<Vec<_>>());
    std::env::remove_var("CUTLINE_HOME");
}

/// Portrait, in 2 parts, target size: all in one export
#[test]
fn vertical_split_with_target_size() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    let p = circle_project(dir.path(), 4.0);
    let mut d = shorts_dialog(&p, dir.path());
    (d.split_on, d.split_by, d.split_count) = (true, 0, 2);
    d.settings.as_mut().unwrap().target_mb = Some(1);
    frame(&mut d, &ctx, &p, true);
    drain(&mut d, &ctx, &p);
    assert_eq!(d.error, None);
    for n in ["Kreis – Teil 01.mp4", "Kreis – Teil 02.mp4"] {
        let f = dir.path().join("Kreis").join(n);
        let m = cl_media::probe(&f).unwrap();
        let size = std::fs::metadata(&f).unwrap().len();
        assert_eq!((m.width, m.height), (180, 320), "{n}");
        assert!((m.duration.as_secs_f64() - 2.0).abs() < 0.1 && size <= 1_000_000, "{n}: {:?} {size}", m.duration);
    }
    assert_eq!(crate::exports::for_project("Kreis", Some(&dir.path().join("p.cutline"))).len(), 2);
    std::env::remove_var("CUTLINE_HOME");
}

/// One frame of the dialog; returns the message (toast)
#[cfg(test)]
fn frame_toast(d: &mut ExportDialog, ctx: &egui::Context, p: &Project, enter: bool) -> Option<String> {
    let events = if enter {
        vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]
    } else {
        Vec::new()
    };
    let mut toast = None;
    let _ = ctx.run(egui::RawInput { events, ..Default::default() }, |ctx| {
        d.poll(ctx);
        toast = d.show(ctx, p, &HashMap::new(), None, 0);
    });
    toast
}

/// „Person folgen“, analysis fails (file missing): no crash, at most one message, clip centred like
/// „Füllen“, no new attempt
#[test]
fn follow_person_failure_keeps_the_clip_centred_with_one_toast() {
    use cl_core::{Clip, ClipContent, MediaKind, MediaRef, Track, TrackKind};
    let dir = tempfile::tempdir().unwrap();
    let mut p = Project::new("t");
    let m = MediaRef { id: cl_core::MediaId::new(), path: dir.path().join("fehlt.mp4"), kind: MediaKind::Video, duration: Time::from_secs_f64(9.0), width: 1920, height: 1080, fps: None, has_audio: true, audio_streams: 1, video_codec: None, audio_codec: None };
    let mut t = Track::new(TrackKind::Video);
    let c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(9.0));
    let id = c.id;
    t.clips.push(c);
    (p.media, p.timeline.tracks) = (vec![m], vec![t]);
    let ctx = egui::Context::default();
    let mut d = ExportDialog::default();
    d.open_for(&p, None, 0, None);
    (d.aspect, d.fit) = (2, 0);
    let plain = cover_layers(&d.framed(&p, &HashMap::new()).unwrap(), id, Time::ZERO);
    d.fit = 1;
    let mut toasts = Vec::new();
    toasts.extend(frame_toast(&mut d, &ctx, &p, false));
    assert!(d.focus_job.is_some(), "analysis starts");
    let t0 = Instant::now();
    while d.focus_job.is_some() && t0.elapsed().as_secs() < 60 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        toasts.extend(frame_toast(&mut d, &ctx, &p, false));
    }
    for _ in 0..3 {
        toasts.extend(frame_toast(&mut d, &ctx, &p, false));
    }
    assert!(d.focus_job.is_none(), "no new attempt");
    // a missing file gives an empty track without FFmpeg an error: at most one message
    assert!(toasts.len() <= 1, "{toasts:?}");
    let f = d.framed(&p, &HashMap::new()).unwrap();
    assert!(f.find_clip(id).unwrap().1.keyframes.0.is_empty());
    assert_eq!(cover_layers(&f, id, Time::ZERO), plain);
}

/// Test image 320×180: red circle (radius 20) moves on a grey background from x=60 to x=260, 2 s with sound
#[cfg(test)]
fn walker_video(dir: &Path) -> cl_core::MediaRef {
    let path = dir.join("laeufer.mp4");
    let inside = "lt(hypot(X-(60+100*T)\\,Y-90)\\,20)";
    let vf = format!("format=rgb24,geq=r='if({inside}\\,255\\,128)':g='if({inside}\\,0\\,128)':b='if({inside}\\,0\\,128)'");
    let st = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y", "-threads", "2", "-f", "lavfi", "-i", "color=c=black:s=320x180:r=30:d=2", "-f", "lavfi", "-i", "sine=f=440:d=2"])
        .args(["-vf", &vf, "-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv444p", "-c:a", "aac", "-shortest"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(st.success());
    cl_media::probe(&path).unwrap()
}

/// Horizontal centre of the red pixels
#[cfg(test)]
fn red_center_x(img: &(usize, usize, Vec<u8>)) -> f64 {
    let (w, _, px) = img;
    let xs: Vec<usize> = px.chunks(4).enumerate().filter(|(_, p)| p[0] > 180 && p[1] < 70 && p[2] < 70).map(|(i, _)| i % w).collect();
    assert!(!xs.is_empty(), "no red in the picture");
    xs.iter().sum::<usize>() as f64 / xs.len() as f64
}

/// Portrait with „Person folgen“, in 2 parts with target size: the export waits for the analysis, the
/// moving circle stays centred in both parts (centre-cropped it would be at the edge at 0.6 s)
#[test]
fn follow_person_with_split_parts_and_target_size() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    use cl_core::{Clip, ClipContent, MediaKind, Track, TrackKind};
    let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("CUTLINE_HOME", dir.path());
    let ctx = egui::Context::default();
    let m = walker_video(dir.path());
    let mut p = Project::new("Lauf");
    (p.settings.width, p.settings.height) = (320, 180);
    let mut t = Track::new(TrackKind::Video);
    t.main = true;
    t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(2.0)));
    p.media.push(m);
    p.timeline.tracks.push(t);
    let mut d = shorts_dialog(&p, dir.path());
    (d.fit, d.split_on, d.split_by, d.split_count) = (1, true, 0, 2);
    d.settings.as_mut().unwrap().target_mb = Some(1);
    // Enter while the analysis runs: nothing starts
    frame(&mut d, &ctx, &p, true);
    assert!(d.focus_job.is_some() && d.running.is_none() && d.queue.is_empty());
    let t0 = Instant::now();
    while d.focus_job.is_some() && t0.elapsed().as_secs() < 60 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        frame(&mut d, &ctx, &p, false);
    }
    frame(&mut d, &ctx, &p, true);
    drain(&mut d, &ctx, &p);
    assert_eq!(d.error, None);
    for (n, at) in [("Lauf – Teil 01.mp4", 18), ("Lauf – Teil 02.mp4", 12)] {
        let f = dir.path().join("Lauf").join(n);
        let size = std::fs::metadata(&f).unwrap().len();
        assert!(size <= 1_000_000, "{n}: {size}");
        let img = grab(&f, at);
        assert_eq!((img.0, img.1), (180, 320), "{n}");
        let x = red_center_x(&img);
        assert!((x - 90.0).abs() < 30.0, "{n}: circle at x={x}");
    }
    std::env::remove_var("CUTLINE_HOME");
}

/// Cover image with „Person folgen“: section at the frame of the cover image (start left, end right)
#[test]
fn cover_preview_shows_the_followed_crop_at_its_frame() {
    use cl_core::{Clip, ClipContent, MediaKind, MediaRef, Track, TrackKind};
    let mut p = Project::new("t");
    let m = MediaRef { id: cl_core::MediaId::new(), path: "a.mp4".into(), kind: MediaKind::Video, duration: Time::from_secs_f64(9.0), width: 1920, height: 1080, fps: None, has_audio: true, audio_streams: 1, video_codec: None, audio_codec: None };
    let mut t = Track::new(TrackKind::Video);
    let c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::from_secs_f64(1.0), Time::from_secs_f64(4.0));
    let (id, key) = (c.id, crate::shorts::focus_key(&c, &m));
    t.clips.push(c);
    (p.media, p.timeline.tracks) = (vec![m], vec![t]);
    let mut d = ExportDialog::default();
    d.open_for(&p, None, 0, None);
    (d.aspect, d.fit) = (2, 1);
    d.focus.insert(key, vec![(0.0, 0.2, 0.5), (4.0, 0.8, 0.5)]);
    let f = d.framed(&p, &HashMap::new()).unwrap();
    // point of the person (share in the source image) in the preview
    let at = |s: f64, fx: f32| {
        let l = cover_layers(&f, id, Time::from_secs_f64(s));
        l[0].0.left() + fx * l[0].0.width()
    };
    assert!((at(1.0, 0.2) - 0.5).abs() < 0.01 && (at(5.0, 0.8) - 0.5).abs() < 0.01, "{} {}", at(1.0, 0.2), at(5.0, 0.8));
    // trimmed in the editor: the track belongs to the old source range and no longer applies
    p.timeline.tracks[0].clips[0].source_in = Time::from_secs_f64(1.0);
    d.rev = 1;
    assert!(d.framed(&p, &HashMap::new()).unwrap().find_clip(id).unwrap().1.keyframes.0.is_empty());
}
