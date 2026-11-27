//! Open projects in the background and check media in the background.
//!
//! Opening: read and parse the project file, check media (do they exist? in parallel), prepare
//! the timeline (needed fonts, missing content, pre-reading image animations) – all in
//! a thread of its own. After 0.3 s the UI shows a guided loading state with
//! steps, progress and cancel; only the finished result is taken over.
//!
//! Media state: whether a file is there is kept in a cache that a background
//! thread refreshes every few seconds (and when the media list changes). The UI
//! never asks the file system per frame and per medium.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_core::{ClipContent, MediaKind, Project};
use cl_import::{Progress, Status, Step};
use eframe::egui::{self, vec2, RichText};

use crate::app::{App, Screen};
use crate::theme::{self, icon};
use crate::widgets;

/// From this load time on the loading state appears (shorter opening does not flash)
pub const SHOW_AFTER: Duration = Duration::from_millis(300);

/// Concurrent checks when opening (file system, no processes)
const CHECK_WORKERS: usize = 8;

/// Check files in parallel: present? Cancelable, progress per file.
pub fn check_files(paths: &[PathBuf], progress: &Progress, step: Step) -> Option<HashMap<PathBuf, bool>> {
    progress.set(step, 0, paths.len(), "");
    let out = std::sync::Mutex::new(HashMap::with_capacity(paths.len()));
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..CHECK_WORKERS.min(paths.len()).max(1) {
            s.spawn(|| loop {
                if progress.cancelled() {
                    return;
                }
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(p) = paths.get(i) else { return };
                let ok = p.is_file();
                out.lock().unwrap_or_else(|e| e.into_inner()).insert(p.clone(), ok);
                progress.tick(&p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            });
        }
    });
    (!progress.cancelled()).then(|| out.into_inner().unwrap_or_else(|e| e.into_inner()))
}

// ------------------------------------------------------------------ media state

/// Cache "file present?" per media path.
#[derive(Default)]
pub struct Presence {
    known: HashMap<PathBuf, bool>,
    /// last check: time and number of media
    checked: Option<(Instant, usize)>,
    pending: Option<Receiver<HashMap<PathBuf, bool>>>,
}

/// How often to look while idle (files can vanish or appear outside)
const RECHECK: Duration = Duration::from_secs(3);

impl Presence {
    /// Present? Never checked paths count as present until the check says otherwise.
    pub fn get(&self, p: &Path) -> bool {
        self.known.get(p).copied().unwrap_or(true)
    }

    /// Take over the result of a check (open, import, relink).
    pub fn set_all(&mut self, m: HashMap<PathBuf, bool>, media: usize) {
        self.known = m;
        self.checked = Some((Instant::now(), media));
        // a running check belongs to the old state (other project): discard its result
        self.pending = None;
    }

    /// New check due? Immediately when the media count changed, otherwise every `RECHECK`.
    pub fn due(checked: Option<(Instant, usize)>, media: usize, pending: bool) -> bool {
        !pending && checked.is_none_or(|(t, n)| n != media || t.elapsed() >= RECHECK)
    }

    /// Per frame: take over a finished check, start due ones in the background. `true` = changed.
    pub fn tick(&mut self, project: &Project, ctx: &egui::Context) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(m) => {
                    changed = m != self.known;
                    self.known = m;
                    self.pending = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.pending = None,
            }
        }
        let n = project.media.len();
        if Self::due(self.checked, n, self.pending.is_some()) {
            self.checked = Some((Instant::now(), n));
            let paths: Vec<PathBuf> = project.media.iter().map(|m| m.path.clone()).collect();
            let (tx, rx) = channel();
            let c = ctx.clone();
            std::thread::spawn(move || {
                let r = check_files(&paths, &Progress::default(), Step::Find).unwrap_or_default();
                let _ = tx.send(r);
                c.request_repaint();
            });
            self.pending = Some(rx);
        }
        changed
    }
}

// ------------------------------------------------------------------ opening

/// Result of loading
pub struct Loaded {
    pub project: Project,
    pub present: HashMap<PathBuf, bool>,
    /// Fonts of the texts (families)
    pub fonts: HashSet<String>,
    /// missing content (effects, filters …) with names
    pub gone: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadMode {
    /// Open a project
    Open,
    /// Restore a backup into the open project (path stays)
    Restore,
}

pub struct Load {
    pub path: PathBuf,
    pub name: String,
    pub mode: LoadMode,
    pub progress: Arc<Progress>,
    pub since: Instant,
    rx: Receiver<Result<Loaded, String>>,
}

/// A project saved by a version with the old ids of bundled content (see `cl_core::legacy`) was
/// converted while loading: the file is written in the current version at once, so it is converted
/// only this one time. The file as it was stays beside it (`<name>.cutline.v6`, never overwritten).
fn convert_old_file(path: &Path, text: &str, project: &Project) {
    let Some(v) = Project::file_version(text).filter(|v| *v <= cl_core::LAST_GERMAN_ID_VERSION) else { return };
    let mut keep = path.as_os_str().to_owned();
    keep.push(format!(".v{v}"));
    let keep = PathBuf::from(keep);
    if !keep.exists() {
        if let Err(e) = std::fs::copy(path, &keep) {
            // without the copy the file is not touched (it converts again at the next opening)
            log::warn!(target: "project", "old project not converted on disk, copy failed: {e}");
            return;
        }
    }
    match crate::paths::write_atomic(path, project.to_json()) {
        Ok(()) => log::info!(target: "project", "project of version {v} converted to version {}", cl_core::PROJECT_SCHEMA_VERSION),
        Err(e) => log::warn!(target: "project", "converted project not written: {e}"),
    }
}

/// Read and prepare a project (background thread).
pub fn load(path: &Path, progress: &Progress) -> Result<Loaded, String> {
    progress.set(Step::Read, 0, 1, &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    // The message names the file (otherwise only "Path not found" shows)
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {} ({e})", crate::tr("Project could not be opened"), path.display()))?;
    progress.check()?;
    let mut project = Project::from_json(&text).map_err(|e| e.to_string())?;
    convert_old_file(path, &text, &project);
    drop(text);
    progress.set(Step::Read, 1, 1, "");
    progress.check()?;
    let paths: Vec<PathBuf> = project.media.iter().map(|m| m.path.clone()).collect::<HashSet<_>>().into_iter().collect();
    let mut present = check_files(&paths, progress, Step::Find).ok_or(cl_import::CANCELLED)?;
    // a moved archive (`project_tools::archive`): missing files sit in `Media/`/`Fonts/` next to
    // the project. Only the loaded copy changes; the file on disk follows on the next save.
    // (archives made by versions up to 0.19.3 call the folders `Medien` and `Schriften`)
    let beside = |subs: [&str; 2], p: &Path| subs.iter().find_map(|sub| Some(path.parent()?.join(sub).join(p.file_name()?)).filter(|c| c.is_file()));
    for m in &mut project.media {
        if present.get(&m.path) == Some(&false) {
            if let Some(c) = beside(["Media", cl_core::legacy::old("archive_media")], &m.path) {
                present.insert(c.clone(), true);
                m.path = c;
            }
        }
    }
    present.retain(|p, _| project.media.iter().any(|m| &m.path == p));
    for f in &mut project.project_fonts {
        if !f.is_file() {
            if let Some(c) = beside(["Fonts", cl_core::legacy::old("archive_fonts")], f) {
                *f = c;
            }
        }
    }
    reprobe_alpha(&mut project, &present);
    progress.check()?;
    // Prepare the timeline: image animations (thumbnail time), fonts, content
    let images: Vec<&cl_core::MediaRef> = project.media.iter().filter(|m| m.kind == MediaKind::Image && present.get(&m.path) == Some(&true)).collect();
    progress.set(Step::Timeline, 0, images.len() + 1, "");
    for m in images {
        progress.check()?;
        let _ = cl_media::animation(&m.path);
        progress.tick("");
    }
    let fonts = project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter())
        .filter_map(|c| match &c.content {
            ClipContent::Text(s) => s.font_family.clone(),
            _ => None,
        })
        .collect();
    let mut gone: Vec<String> = cl_fx::catalog::missing(&project).into_iter().map(|m| cl_fx::fx_name(&m.0)).collect();
    gone.sort();
    gone.dedup();
    progress.tick("");
    progress.check()?;
    Ok(Loaded { project, present, fonts, gone })
}

/// Codecs that can carry alpha: projects saved before `MediaRef::has_alpha` existed say `false`
/// for every file, so these are probed again (a short FFprobe each, in parallel). The rest stays.
const ALPHA_CODECS: &[&str] = &["prores", "qtrle", "png", "vp8", "vp9", "hevc", "ffv1", "utvideo", "rawvideo", "hap"];

fn reprobe_alpha(project: &mut Project, present: &HashMap<PathBuf, bool>) {
    let mut todo: Vec<&mut cl_core::MediaRef> = project
        .media
        .iter_mut()
        .filter(|m| m.kind == MediaKind::Video && !m.has_alpha && present.get(&m.path) == Some(&true))
        .filter(|m| m.video_codec.as_deref().is_some_and(|c| ALPHA_CODECS.contains(&c)))
        .collect();
    let n = todo.len();
    std::thread::scope(|s| {
        for chunk in todo.chunks_mut(n.div_ceil(CHECK_WORKERS).max(1)) {
            s.spawn(|| {
                for m in chunk {
                    m.has_alpha = cl_media::probe_alpha(&m.path).unwrap_or(false);
                }
            });
        }
    });
}

impl Load {
    pub fn start(path: &Path, mode: LoadMode, ctx: &egui::Context) -> Self {
        let c = ctx.clone();
        let progress = Arc::new(Progress::new(move || c.request_repaint()));
        let (tx, rx) = channel();
        let (p, pr, c) = (path.to_path_buf(), progress.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("cl-load".into())
            .spawn(move || {
                let t = Instant::now();
                let r = load(&p, &pr);
                if crate::app::perf() {
                    eprintln!("perf: project load {:.0} ms in the background", t.elapsed().as_secs_f64() * 1000.0);
                }
                let _ = tx.send(r);
                c.request_repaint();
            })
            .expect("Lade-Thread");
        Self {
            name: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            path: path.to_path_buf(),
            mode,
            progress,
            since: Instant::now(),
            rx,
        }
    }
}

/// Steps of opening (display)
const LOAD_STEPS: [(Step, &str); 3] = [(Step::Read, "Read project"), (Step::Find, "Check media"), (Step::Timeline, "Build timeline")];

/// List of steps: done ones with a check mark, the current one highlighted with a progress line
/// and counter, below it the current file in small text.
pub fn steps_view(ui: &mut egui::Ui, id: egui::Id, steps: &[(Step, &str)], st: &Status) {
    for (i, (step, name)) in steps.iter().enumerate() {
        let done = *step < st.step;
        let now = *step == st.step;
        ui.horizontal(|ui| {
            ui.set_min_height(22.0);
            let (glyph, color) = if done {
                (icon::CHECK_CIRCLE, theme::ACCENT())
            } else if now {
                (icon::CIRCLE_NOTCH, theme::TEXT())
            } else {
                (icon::CIRCLE, theme::TEXT_DIM())
            };
            ui.label(RichText::new(glyph).color(color));
            let mut t = RichText::new(crate::tr(*name)).color(if now { theme::TEXT() } else { theme::TEXT_DIM() });
            if now {
                t = t.strong();
            }
            ui.label(t);
            if now && st.total > 1 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{} / {}", st.done.min(st.total), st.total)).monospace().color(theme::TEXT_DIM()));
                });
            }
        });
        if now {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 3.0), egui::Sense::hover());
            let frac = if st.total == 0 { 0.0 } else { st.done as f32 / st.total as f32 };
            widgets::task_line(ui.painter(), id.with(i), r, frac);
            ui.add_space(2.0);
            let font = egui::FontId::proportional(theme::FS_HINT);
            let w = ui.available_width();
            let shown = theme::elide_middle(&st.current, w, |t| theme::text_width(ui.ctx(), t, font.clone()));
            ui.label(RichText::new(if shown.is_empty() { " ".to_string() } else { shown }).size(theme::FS_HINT).color(theme::TEXT_DIM()));
        }
    }
}

impl App {
    /// Open a project (in the background; taken over in [`App::poll_load`]).
    pub fn start_load(&mut self, p: &Path, mode: LoadMode, ctx: &egui::Context) {
        if let Some(l) = &self.load {
            l.progress.cancel();
        }
        self.load = Some(Load::start(p, mode, ctx));
    }

    /// Take over finished loading and show the loading state (every frame).
    pub fn poll_load(&mut self, ctx: &egui::Context) {
        let _s = crate::app::section("load project");
        let Some(l) = &self.load else { return };
        let res = match l.rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => {
                self.load_dialog(ctx);
                return;
            }
            Err(TryRecvError::Disconnected) => Err("Project could not be loaded (internal error)".into()),
        };
        let Some(l) = self.load.take() else { return };
        match res {
            Ok(done) => match l.mode {
                LoadMode::Open => self.apply_loaded(&l.path, done, Some(l.since)),
                LoadMode::Restore => {
                    // save the current state first, then replace (path stays)
                    if let Some(path) = self.project_path.clone() {
                        self.backup_now(&path);
                    }
                    self.presence.set_all(done.present, done.project.media.len());
                    self.set_doc(done.project);
                    self.backups_open = false;
                    self.toast("Backup restored");
                }
            },
            Err(e) if e == cl_import::CANCELLED => {}
            Err(e) => self.toast_err(e),
        }
    }

    fn load_dialog(&mut self, ctx: &egui::Context) {
        let Some(l) = &self.load else { return };
        if l.since.elapsed() < SHOW_AFTER {
            ctx.request_repaint_after(SHOW_AFTER);
            return;
        }
        let st = l.progress.status();
        let title = crate::tr(format!("Opening “{}”", l.name));
        let mut cancel = false;
        crate::dialog::modal(ctx, "project-load").show(ctx, |ui| {
            ui.set_width(420.0);
            cancel |= crate::dialog::title(ui, &title);
            steps_view(ui, egui::Id::new("project-load"), &LOAD_STEPS, &st);
            cancel |= crate::dialog::footer(ui, |ui| ui.add(theme::secondary("Cancel")).clicked());
        });
        if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.cancel_load();
        }
    }

    /// Cancel a running load (its result would otherwise be taken over later).
    pub fn cancel_load(&mut self) {
        if let Some(l) = self.load.take() {
            l.progress.cancel();
        }
    }

    /// Put a loaded project into the editor.
    pub fn apply_loaded(&mut self, p: &Path, done: Loaded, t0: Option<Instant>) {
        let s = crate::app::section("load: leave the old project");
        if !self.flush_before_leaving() {
            return;
        }
        self.reset_editor();
        drop(s);
        let s = crate::app::section("load: set the project");
        let n = done.project.media.len();
        self.missing_open = done.present.values().any(|ok| !ok);
        self.presence.set_all(done.present, n);
        self.set_doc(done.project);
        crate::applog::project("opened", &self.project, t0);
        self.saved_revision = self.revision;
        self.project_path = Some(p.to_path_buf());
        drop(s);
        let s = crate::app::section("load: lock and settings");
        self.take_lock(p);
        self.settings.add_recent(p);
        self.settings.save();
        self.screen = Screen::Editor;
        self.last_backup = Some(Instant::now());
        drop(s);
        let _s = crate::app::section("load: fonts");
        let known = self.font_files();
        let mut missing: Vec<String> = done.fonts.into_iter().filter(|f| !known.contains_key(f)).collect();
        missing.sort();
        if !missing.is_empty() {
            self.toast_err(format!("Missing fonts: {}", missing.join(", ")));
        }
        if !done.gone.is_empty() {
            self.toast_err(
                crate::theme::count_text(
                    done.gone.len(),
                    "{} item is missing: {} – turn on its pack in Settings → Content",
                    "{} items are missing: {} – turn on their pack in Settings → Content",
                )
                .replacen("{}", &done.gone.join(", "), 1),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_file(dir: &Path, media: usize) -> PathBuf {
        let mut p = Project::new("Download");
        for i in 0..media {
            let path = dir.join(format!("m{i}.png"));
            if i % 3 != 0 {
                std::fs::write(&path, b"x").unwrap();
            }
            p.media.push(serde_json::from_value(serde_json::json!({
                "id": cl_core::MediaId::new(), "path": path, "kind": "Image", "duration": 1000000,
                "width": 4, "height": 4, "fps": null, "has_audio": false, "video_codec": null, "audio_codec": null
            })).unwrap());
        }
        let f = dir.join("p.cutline");
        std::fs::write(&f, p.to_json()).unwrap();
        f
    }

    /// Missing project file: the message says which one (not just "Path not found")
    #[test]
    fn missing_project_file_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let e = load(&dir.path().join("weg").join("Urlaub.cutline"), &Progress::default()).err().unwrap();
        assert!(e.contains("Urlaub.cutline") && e.starts_with("Project could not be opened"), "{e}");
    }

    #[test]
    fn load_reports_each_step_and_finds_missing_media() {
        let dir = tempfile::tempdir().unwrap();
        let f = project_file(dir.path(), 30);
        let log = Arc::new(std::sync::Mutex::new(Vec::<Status>::new()));
        let slot: Arc<std::sync::OnceLock<std::sync::Weak<Progress>>> = Arc::default();
        let (l, sl) = (log.clone(), slot.clone());
        let pr = Arc::new(Progress::new(move || {
            if let Some(p) = sl.get().and_then(|w| w.upgrade()) {
                l.lock().unwrap().push(p.status());
            }
        }));
        slot.set(Arc::downgrade(&pr)).unwrap();
        let done = load(&f, &pr).unwrap();
        assert_eq!(done.project.media.len(), 30);
        assert_eq!(done.present.values().filter(|v| !**v).count(), 10);
        let log = log.lock().unwrap();
        for s in [Step::Read, Step::Find, Step::Timeline] {
            assert!(log.iter().any(|x| x.step == s), "{s:?}");
        }
        let f: Vec<f32> = log.iter().map(|s| s.fraction()).collect();
        assert!(f.windows(2).all(|w| w[0] <= w[1] + 1e-6), "{f:?}");
        // broken file: error, no crash
        std::fs::write(dir.path().join("broken.cutline"), "{").unwrap();
        assert!(load(&dir.path().join("broken.cutline"), &Progress::default()).is_err());
    }

    /// A project saved before `has_alpha` existed: transparent videos get it on load, opaque ones not
    #[test]
    fn old_project_learns_alpha_on_load() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut files = cl_media::write_alpha_samples(dir.path());
        let opaque = dir.path().join("prores422.mov");
        let st = cl_media::ffmpeg_command().unwrap().args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=s=64x36:d=1", "-c:v", "prores_ks", "-profile:v", "2"]).arg(&opaque).status().unwrap();
        assert!(st.success());
        files.push(opaque.clone());
        let mut p = Project::new("Alt");
        for f in &files {
            p.media.push(cl_media::probe(f).unwrap());
        }
        let mut json: serde_json::Value = serde_json::from_str(&p.to_json()).unwrap();
        for m in json["media"].as_array_mut().unwrap() {
            assert!(m.as_object_mut().unwrap().remove("has_alpha").is_some());
        }
        let file = dir.path().join("old.cutline");
        std::fs::write(&file, json.to_string()).unwrap();
        let done = load(&file, &Progress::default()).unwrap();
        for m in &done.project.media {
            assert_eq!(m.has_alpha, m.path != opaque, "{}", m.path.display());
        }
    }

    #[test]
    fn cancelled_load_returns_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let f = project_file(dir.path(), 5);
        let p = Progress::default();
        p.cancel();
        assert_eq!(load(&f, &p).err().as_deref(), Some(cl_import::CANCELLED));
    }

    #[test]
    fn presence_is_cached_and_rechecked_on_change() {
        let mut pr = Presence::default();
        assert!(pr.get(Path::new("never/checked.mp4")), "unknown counts as present");
        pr.set_all(HashMap::from([(PathBuf::from("a"), false)]), 1);
        assert!(!pr.get(Path::new("a")));
        // freshly checked: not due; other media count or running check
        assert!(!Presence::due(pr.checked, 1, false));
        assert!(Presence::due(pr.checked, 2, false));
        assert!(!Presence::due(pr.checked, 2, true));
        assert!(Presence::due(None, 0, false));
        let old = Instant::now().checked_sub(RECHECK + Duration::from_millis(1)).map(|t| (t, 1));
        assert!(Presence::due(old, 1, false));
    }

    /// The old project's check is still running, the new one sets its state: the old result
    /// must not overwrite it (otherwise missing files would count as present).
    #[test]
    fn old_check_does_not_overwrite_new_project() {
        let mut pr = Presence::default();
        let (tx, rx) = channel();
        tx.send(HashMap::from([(PathBuf::from("old"), true)])).unwrap();
        pr.pending = Some(rx);
        pr.set_all(HashMap::from([(PathBuf::from("new"), false)]), 1);
        let mut p = Project::new("new");
        p.media.push(serde_json::from_value(serde_json::json!({
            "id": cl_core::MediaId::new(), "path": "new", "kind": "Image", "duration": 0,
            "width": 4, "height": 4, "fps": null, "has_audio": false, "video_codec": null, "audio_codec": null
        })).unwrap());
        pr.tick(&p, &egui::Context::default());
        assert!(!pr.get(Path::new("new")));
    }

    /// Opening the same project again: the open state is already on disk before
    /// loading reads the file. New project and start page cancel a running load.
    #[test]
    fn reopen_writes_first_and_leaving_cancels_load() {
        let mut app = crate::saving::tests::test_app();
        let f = app.dir().join("a.cutline");
        app.set_doc(Project::new("A"));
        app.project_path = Some(f.clone());
        app.screen = Screen::Editor;
        assert!(app.flush_before_leaving());
        app.apply(cl_core::EditOp::Rename { name: "A new".into() });
        app.open_project(&f);
        assert_eq!(Project::from_json(&std::fs::read_to_string(&f).unwrap()).unwrap().name, "A new");
        assert!(app.load.is_some());
        app.go_home();
        assert!(app.load.is_none(), "start page cancels loading");
        app.open_project(&f);
        assert!(app.new_project());
        assert!(app.load.is_none(), "new project cancels loading");
    }

    #[test]
    fn moved_archive_finds_its_media_and_fonts() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("clip.mp4"), b"v").unwrap();
        std::fs::write(src.join("font.ttf"), b"f").unwrap();
        let mut p = Project::new("Film: Part 1");
        p.media.push(serde_json::from_value(serde_json::json!({
            "id": cl_core::MediaId::new(), "path": src.join("clip.mp4"), "kind": "Video", "duration": 1000000,
            "width": 2, "height": 2, "fps": null, "has_audio": false, "video_codec": null, "audio_codec": null
        })).unwrap());
        let out = crate::project_tools::archive(&p, &[src.join("font.ttf")], &dir.path().join("out")).unwrap();
        let file = std::fs::read_dir(&out).unwrap().flatten().map(|e| e.path()).find(|f| f.extension().is_some_and(|x| x == "cutline")).unwrap();
        // the archive goes to another place (stick, other computer)
        let moved = dir.path().join("woanders");
        std::fs::rename(&out, &moved).unwrap();
        let l = load(&moved.join(file.file_name().unwrap()), &Progress::default()).unwrap();
        assert_eq!(l.project.media[0].path, moved.join("Media").join("clip.mp4"));
        assert_eq!(l.present.len(), 1);
        assert!(l.present.values().all(|ok| *ok));
        assert_eq!(l.project.project_fonts, vec![moved.join("Fonts").join("font.ttf")]);
    }

    #[test]
    fn check_files_in_parallel() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<PathBuf> = (0..50).map(|i| dir.path().join(format!("{i}"))).collect();
        for p in paths.iter().step_by(2) {
            std::fs::write(p, b"").unwrap();
        }
        let pr = Progress::default();
        let m = check_files(&paths, &pr, Step::Find).unwrap();
        assert_eq!(m.values().filter(|v| **v).count(), 25);
        assert_eq!(pr.status().done, 50);
    }
    /// A project of version 6 (up to 0.19.3) is written in the current version when it is first
    /// opened; the file as it was stays beside it and a second opening converts nothing.
    #[test]
    fn old_project_is_converted_on_disk_once() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("Old.cutline");
        let mut p = Project::new("Old");
        let mut t = cl_core::Track::new(cl_core::TrackKind::Effect);
        t.clips.push(cl_core::Clip::new(ClipContent::Effect(cl_core::FxRef::new("cutline.effekte/hologramm")), cl_core::Time::ZERO, cl_core::Time::from_secs_f64(1.0)));
        p.timeline.tracks.push(t);
        let v6 = p.to_json().replace(&format!("\"schema_version\": {}", cl_core::PROJECT_SCHEMA_VERSION), "\"schema_version\": 6");
        std::fs::write(&f, &v6).unwrap();
        let id = |p: &Project| match &p.timeline.tracks[0].clips[0].content {
            ClipContent::Effect(fx) => fx.id.clone(),
            _ => String::new(),
        };
        let l = load(&f, &Progress::default()).unwrap();
        assert_eq!(id(&l.project), "cutline.effects/hologram");
        let on_disk = std::fs::read_to_string(&f).unwrap();
        assert_eq!(Project::file_version(&on_disk), Some(cl_core::PROJECT_SCHEMA_VERSION));
        assert!(on_disk.contains("cutline.effects/hologram") && !on_disk.contains("hologramm"));
        let keep = dir.path().join("Old.cutline.v6");
        assert_eq!(std::fs::read_to_string(&keep).unwrap(), v6, "the file as it was");
        // opened again: a version 7 file, nothing is converted and the copy stays as it is
        std::fs::write(&keep, "untouched").unwrap();
        let again = load(&f, &Progress::default()).unwrap();
        assert_eq!(again.project, l.project);
        assert_eq!(std::fs::read_to_string(&keep).unwrap(), "untouched");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), on_disk);
    }

    /// An archive made by a version up to 0.19.3 keeps its files in `Medien` and `Schriften`
    #[test]
    fn moved_archive_of_an_older_version_finds_its_media() {
        let dir = tempfile::tempdir().unwrap();
        let moved = dir.path().join("moved");
        std::fs::create_dir_all(moved.join("Medien")).unwrap();
        std::fs::write(moved.join("Medien").join("clip.mp4"), b"x").unwrap();
        let mut p = Project::new("A");
        p.media.push(cl_core::MediaRef {
            id: cl_core::MediaId::new(),
            path: dir.path().join("gone").join("Medien").join("clip.mp4"),
            kind: MediaKind::Video,
            duration: cl_core::Time::from_secs_f64(1.0),
            width: 16,
            height: 16,
            fps: None,
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
            has_alpha: false,
        });
        std::fs::write(moved.join("A.cutline"), p.to_json()).unwrap();
        let l = load(&moved.join("A.cutline"), &Progress::default()).unwrap();
        assert_eq!(l.project.media[0].path, moved.join("Medien").join("clip.mp4"));
    }
}
