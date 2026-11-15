//! Projekte im Hintergrund öffnen und Medien im Hintergrund prüfen.
//!
//! Öffnen: Projektdatei lesen und parsen, Medien prüfen (existieren sie? parallel), Zeitleiste
//! vorbereiten (benötigte Schriften, fehlende Inhalte, Animationen der Bilder vorlesen) – alles in
//! einem eigenen Thread. Die Oberfläche zeigt nach 0,3 s einen geführten Ladezustand mit
//! Schritten, Fortschritt und Abbrechen; übernommen wird erst das fertige Ergebnis.
//!
//! Medien-Zustand: ob eine Datei da ist, steht in einem Zwischenspeicher, den ein Hintergrund-
//! Thread alle paar Sekunden (und wenn sich die Medienliste ändert) auffrischt. Die Oberfläche
//! fragt das Dateisystem nie je Bild und je Medium.

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

/// Ab dieser Ladezeit erscheint der Ladezustand (kürzeres Öffnen blitzt nicht auf)
pub const SHOW_AFTER: Duration = Duration::from_millis(300);

/// Gleichzeitige Prüfungen beim Öffnen (Dateisystem, keine Prozesse)
const CHECK_WORKERS: usize = 8;

/// Dateien parallel prüfen: vorhanden? Abbrechbar, Fortschritt je Datei.
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

// ------------------------------------------------------------------ Medien-Zustand

/// Zwischenspeicher „Datei vorhanden?“ je Medienpfad.
#[derive(Default)]
pub struct Presence {
    known: HashMap<PathBuf, bool>,
    /// letzte Prüfung: Zeitpunkt und Anzahl der Medien
    checked: Option<(Instant, usize)>,
    pending: Option<Receiver<HashMap<PathBuf, bool>>>,
}

/// Wie oft im Stand nachgesehen wird (Dateien können außerhalb verschwinden oder auftauchen)
const RECHECK: Duration = Duration::from_secs(3);

impl Presence {
    /// Vorhanden? Noch nie geprüfte Pfade gelten als vorhanden, bis die Prüfung anderes sagt.
    pub fn get(&self, p: &Path) -> bool {
        self.known.get(p).copied().unwrap_or(true)
    }

    /// Ergebnis einer Prüfung übernehmen (Öffnen, Import, Neu verknüpfen).
    pub fn set_all(&mut self, m: HashMap<PathBuf, bool>, media: usize) {
        self.known = m;
        self.checked = Some((Instant::now(), media));
    }

    /// Neue Prüfung fällig? Bei geänderter Medienzahl sofort, sonst alle `RECHECK`.
    pub fn due(checked: Option<(Instant, usize)>, media: usize, pending: bool) -> bool {
        !pending && checked.is_none_or(|(t, n)| n != media || t.elapsed() >= RECHECK)
    }

    /// Je Bild: fertige Prüfung übernehmen, fällige im Hintergrund starten. `true` = geändert.
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

// ------------------------------------------------------------------ Öffnen

/// Ergebnis des Ladens
pub struct Loaded {
    pub project: Project,
    pub present: HashMap<PathBuf, bool>,
    /// Schriften der Texte (Familien)
    pub fonts: HashSet<String>,
    /// fehlende Inhalte (Effekte, Filter …) mit Namen
    pub gone: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadMode {
    /// Projekt öffnen
    Open,
    /// Sicherung in das offene Projekt zurückholen (Pfad bleibt)
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

/// Projekt lesen und vorbereiten (Hintergrund-Thread).
pub fn load(path: &Path, progress: &Progress) -> Result<Loaded, String> {
    progress.set(Step::Read, 0, 1, &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    progress.check()?;
    let project = Project::from_json(&text).map_err(|e| e.to_string())?;
    drop(text);
    progress.set(Step::Read, 1, 1, "");
    progress.check()?;
    let paths: Vec<PathBuf> = project.media.iter().map(|m| m.path.clone()).collect::<HashSet<_>>().into_iter().collect();
    let present = check_files(&paths, progress, Step::Find).ok_or(cl_import::CANCELLED)?;
    // Zeitleiste vorbereiten: Animationen der Bilder (Vorschaubild-Zeit), Schriften, Inhalte
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
                    eprintln!("perf: Projekt laden {:.0} ms im Hintergrund", t.elapsed().as_secs_f64() * 1000.0);
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

/// Schritte des Öffnens (Anzeige)
const LOAD_STEPS: [(Step, &str); 3] = [(Step::Read, "Projekt lesen"), (Step::Find, "Medien prüfen"), (Step::Timeline, "Zeitleiste aufbauen")];

/// Liste der Schritte: erledigte mit Häkchen, der aktuelle hervorgehoben mit Fortschrittslinie
/// und Zähler, darunter klein die aktuelle Datei.
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
    /// Projekt öffnen (im Hintergrund; übernommen wird es in [`App::poll_load`]).
    pub fn start_load(&mut self, p: &Path, mode: LoadMode, ctx: &egui::Context) {
        if let Some(l) = &self.load {
            l.progress.cancel();
        }
        self.load = Some(Load::start(p, mode, ctx));
    }

    /// Fertiges Laden übernehmen und den Ladezustand zeigen (jedes Bild).
    pub fn poll_load(&mut self, ctx: &egui::Context) {
        let _s = crate::app::section("Projekt laden");
        let Some(l) = &self.load else { return };
        let res = match l.rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => {
                self.load_dialog(ctx);
                return;
            }
            Err(TryRecvError::Disconnected) => Err("Projekt konnte nicht geladen werden (interner Fehler)".into()),
        };
        let Some(l) = self.load.take() else { return };
        match res {
            Ok(done) => match l.mode {
                LoadMode::Open => self.apply_loaded(&l.path, done),
                LoadMode::Restore => {
                    // aktuellen Stand vorher sichern, dann ersetzen (Pfad bleibt)
                    if let Some(path) = self.project_path.clone() {
                        self.backup_now(&path);
                    }
                    self.presence.set_all(done.present, done.project.media.len());
                    self.set_doc(done.project);
                    self.backups_open = false;
                    self.toast("Sicherung wiederhergestellt");
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
        let title = crate::tr(format!("„{}“ wird geöffnet", l.name));
        let mut cancel = false;
        crate::dialog::modal(ctx, "project-load").show(ctx, |ui| {
            ui.set_width(420.0);
            cancel |= crate::dialog::title(ui, &title);
            steps_view(ui, egui::Id::new("project-load"), &LOAD_STEPS, &st);
            cancel |= crate::dialog::footer(ui, |ui| ui.add(theme::secondary("Abbrechen")).clicked());
        });
        if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some(l) = self.load.take() {
                l.progress.cancel();
            }
        }
    }

    /// Geladenes Projekt in den Editor setzen.
    pub fn apply_loaded(&mut self, p: &Path, done: Loaded) {
        if !self.flush_before_leaving() {
            return;
        }
        self.reset_editor();
        let n = done.project.media.len();
        self.missing_open = done.present.values().any(|ok| !ok);
        self.presence.set_all(done.present, n);
        self.set_doc(done.project);
        self.saved_revision = self.revision;
        self.project_path = Some(p.to_path_buf());
        self.take_lock(p);
        self.settings.add_recent(p);
        self.settings.save();
        self.screen = Screen::Editor;
        self.last_backup = Some(Instant::now());
        let known = self.font_files();
        let mut missing: Vec<String> = done.fonts.into_iter().filter(|f| !known.contains_key(f)).collect();
        missing.sort();
        if !missing.is_empty() {
            self.toast_err(format!("Fehlende Schriften: {}", missing.join(", ")));
        }
        if !done.gone.is_empty() {
            self.toast_err(
                crate::theme::count_text(
                    done.gone.len(),
                    "{} Inhalt fehlt: {} – Paket in Einstellungen → Inhalte einschalten",
                    "{} Inhalte fehlen: {} – Paket in Einstellungen → Inhalte einschalten",
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
        let mut p = Project::new("Laden");
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
        // kaputte Datei: Fehler, kein Absturz
        std::fs::write(dir.path().join("kaputt.cutline"), "{").unwrap();
        assert!(load(&dir.path().join("kaputt.cutline"), &Progress::default()).is_err());
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
        assert!(pr.get(Path::new("nie/geprüft.mp4")), "unbekannt gilt als vorhanden");
        pr.set_all(HashMap::from([(PathBuf::from("a"), false)]), 1);
        assert!(!pr.get(Path::new("a")));
        // frisch geprüft: nicht fällig; andere Medienzahl oder laufende Prüfung
        assert!(!Presence::due(pr.checked, 1, false));
        assert!(Presence::due(pr.checked, 2, false));
        assert!(!Presence::due(pr.checked, 2, true));
        assert!(Presence::due(None, 0, false));
        let old = Instant::now().checked_sub(RECHECK + Duration::from_millis(1)).map(|t| (t, 1));
        assert!(Presence::due(old, 1, false));
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
}
