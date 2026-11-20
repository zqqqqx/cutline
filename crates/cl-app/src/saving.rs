//! Saving in the background: project, autosave and backups are handed as a snapshot to
//! a thread of their own that serializes and writes atomically. The UI never waits
//! for the disk. Several waiting saves of the same file: only the newest counts.
//! If writing fails, the state ends up in the backups (no data loss).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_core::Project;
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Project file (save, autosave)
    Project,
    /// Backup in the project's backup folder, keep at most `keep`
    Backup { keep: usize },
}

pub struct Job {
    pub kind: Kind,
    /// Project file (for backups: the project they belong to)
    pub path: PathBuf,
    pub project: Project,
    pub revision: u64,
}

#[derive(Debug)]
pub enum Done {
    Saved,
    /// `rescued`: where the state was written instead
    Failed { kind: Kind, revision: u64, err: String, rescued: Option<PathBuf> },
}

/// Merge waiting jobs: of several saves of the same project file only the last
/// remains (in its place); backups all remain.
pub fn coalesce(jobs: Vec<Job>) -> Vec<Job> {
    let last: Vec<bool> = jobs
        .iter()
        .enumerate()
        .map(|(i, j)| j.kind != Kind::Project || !jobs[i + 1..].iter().any(|k| k.kind == Kind::Project && k.path == j.path))
        .collect();
    jobs.into_iter().zip(last).filter(|(_, keep)| *keep).map(|(j, _)| j).collect()
}

fn write(job: &Job) -> Result<(), String> {
    let json = job.project.to_json();
    match job.kind {
        Kind::Project => {
            if let Some(d) = job.path.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            crate::paths::write_atomic(&job.path, json).map_err(|e| e.to_string())?;
            // touch our own lock so its 12 h rule means "last saved", not "opened"
            let lock = crate::paths::lock_path(&job.path);
            let me = std::process::id().to_string();
            if std::fs::read_to_string(&lock).is_ok_and(|s| s.trim() == me) {
                let _ = std::fs::write(&lock, me);
            }
            Ok(())
        }
        Kind::Backup { keep } => crate::project_tools::write_backup(&job.path, &json, keep).map(|_| ()).map_err(|e| e.to_string()),
    }
}

/// Failed project file: rescue the state into the backups
pub(crate) fn rescue(job: &Job) -> Option<PathBuf> {
    let dir = crate::project_tools::backup_dir(&job.path);
    std::fs::create_dir_all(&dir).ok()?;
    let p = dir.join(format!("unsaved-{}.cutline", crate::paths::unix_secs()));
    crate::paths::write_atomic(&p, job.project.to_json()).ok().map(|_| p)
}

/// `rescued`: already rescued states (path, revision) – only one rescue file per state, even
/// if writing fails again and again
fn run(job: Job, rescued: &mut HashSet<(PathBuf, u64)>) -> Done {
    let t = Instant::now();
    let r = write(&job);
    if crate::app::perf() {
        eprintln!("perf: speichern {:?} {:.0} ms im Hintergrund", job.kind, t.elapsed().as_secs_f64() * 1000.0);
    }
    match r {
        Ok(()) => {
            // Revisions restart for each opened project
            rescued.retain(|(p, _)| *p != job.path);
            Done::Saved
        }
        Err(err) => {
            let first = job.kind == Kind::Project && rescued.insert((job.path.clone(), job.revision));
            let rescued = first.then(|| rescue(&job)).flatten();
            Done::Failed { kind: job.kind, revision: job.revision, err, rescued }
        }
    }
}

pub struct Saver {
    tx: Sender<Job>,
    rx: Receiver<Done>,
    /// jobs not yet written
    pending: Arc<AtomicUsize>,
    /// last error message (at most one per minute)
    pub(crate) last_err: Option<Instant>,
    /// last failure of a project file (autosave pauses then)
    failed: Option<Instant>,
}

impl Saver {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, jobs) = channel::<Job>();
        let (done, rx) = channel();
        let pending = Arc::new(AtomicUsize::new(0));
        let p = pending.clone();
        std::thread::Builder::new()
            .name("cl-save".into())
            .spawn(move || {
                let mut rescued = HashSet::new();
                while let Ok(first) = jobs.recv() {
                    let mut batch = vec![first];
                    batch.extend(jobs.try_iter());
                    let n = batch.len();
                    for job in coalesce(batch) {
                        let _ = done.send(run(job, &mut rescued));
                    }
                    p.fetch_sub(n, Ordering::SeqCst);
                    ctx.request_repaint();
                }
            })
            .expect("Speicher-Thread");
        Self { tx, rx, pending, last_err: None, failed: None }
    }

    pub fn send(&self, job: Job) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        if let Err(e) = self.tx.send(job) {
            // thread gone (should never happen): then write here
            self.pending.fetch_sub(1, Ordering::SeqCst);
            let _ = run(e.0, &mut HashSet::new());
        }
    }

    /// After a write error autosave pauses for a minute (otherwise it would rewrite every frame,
    /// e.g. with an unplugged stick); manual save and quitting try immediately.
    pub fn autosave_paused(&self) -> bool {
        self.failed.is_some_and(|t| t.elapsed() < Duration::from_secs(60))
    }

    pub fn busy(&self) -> bool {
        self.pending.load(Ordering::SeqCst) > 0
    }

    /// Collect finished saves.
    pub fn poll(&self) -> Vec<Done> {
        self.rx.try_iter().collect()
    }

    /// On exit: wait until everything is written (at most `max`).
    pub fn wait_idle(&self, max: Duration) -> bool {
        let t = Instant::now();
        while self.busy() {
            if t.elapsed() > max {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }
}

impl crate::app::App {
    /// Collect finished saves (every frame): report errors (at most once per minute),
    /// the project then counts as unsaved again.
    pub fn poll_saves(&mut self) {
        for d in self.saver.poll() {
            let Done::Failed { kind, revision, err, rescued, .. } = d else { continue };
            if kind == Kind::Project {
                self.saver.failed = Some(Instant::now());
                if self.saved_revision == revision {
                    self.saved_revision = crate::app::UNSAVED;
                }
            }
            if self.saver.last_err.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
                self.saver.last_err = Some(Instant::now());
                self.toast_err(format!("Projekt konnte nicht gespeichert werden: {err}"));
                if let Some(r) = rescued {
                    self.toast_err(format!("Stand gesichert in {}", r.display()));
                }
            }
        }
    }

    /// Backup of the current state (in the background).
    pub fn backup_now(&mut self, path: &Path) {
        self.saver.send(Job {
            kind: Kind::Backup { keep: self.settings.backup_keep },
            path: path.to_path_buf(),
            project: self.project.clone(),
            revision: self.revision,
        });
        self.backup_revision = self.revision;
        self.last_backup = Some(Instant::now());
    }
}

#[cfg(test)]
fn job(kind: Kind, path: &Path, rev: u64) -> Job {
    Job { kind, path: path.to_path_buf(), project: Project::new(format!("r{rev}")), revision: rev }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn only_the_newest_save_of_a_file_is_written() {
        let (a, b) = (Path::new("a.cutline"), Path::new("b.cutline"));
        let jobs = vec![
            job(Kind::Project, a, 1),
            job(Kind::Backup { keep: 3 }, a, 1),
            job(Kind::Project, b, 2),
            job(Kind::Project, a, 3),
            job(Kind::Backup { keep: 3 }, a, 3),
        ];
        let got: Vec<(Kind, u64)> = coalesce(jobs).into_iter().map(|j| (j.kind, j.revision)).collect();
        assert_eq!(got, [(Kind::Backup { keep: 3 }, 1), (Kind::Project, 2), (Kind::Project, 3), (Kind::Backup { keep: 3 }, 3)]);
    }

    #[test]
    fn saves_in_background_and_rescues_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CUTLINE_HOME", dir.path());
        let s = Saver::new(egui::Context::default());
        let p = dir.path().join("x.cutline");
        for rev in 1..=5 {
            s.send(job(Kind::Project, &p, rev));
        }
        assert!(s.wait_idle(Duration::from_secs(10)));
        let done = s.poll();
        assert!(matches!(done.last(), Some(Done::Saved)), "{done:?}");
        assert_eq!(Project::from_json(&std::fs::read_to_string(&p).unwrap()).unwrap().name, "r5");
        // Target is a folder: writing fails, the state ends up in the backups
        let bad = dir.path().join("ordner.cutline");
        std::fs::create_dir_all(&bad).unwrap();
        s.send(job(Kind::Project, &bad, 7));
        assert!(s.wait_idle(Duration::from_secs(10)));
        match s.poll().pop() {
            Some(Done::Failed { rescued: Some(r), revision: 7, .. }) => {
                assert_eq!(Project::from_json(&std::fs::read_to_string(r).unwrap()).unwrap().name, "r7")
            }
            d => panic!("{d:?}"),
        }
    }

    /// If the same state fails again and again (autosave, unplugged stick), only one
    /// rescue file is created – not one per attempt.
    #[test]
    fn repeated_failure_of_one_state_rescues_once() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("CUTLINE_HOME", dir.path());
        let s = Saver::new(egui::Context::default());
        let bad = dir.path().join("ordner.cutline");
        std::fs::create_dir_all(&bad).unwrap();
        let mut rescued = Vec::new();
        for _ in 0..3 {
            s.send(job(Kind::Project, &bad, 4));
            assert!(s.wait_idle(Duration::from_secs(10)));
            for d in s.poll() {
                if let Done::Failed { rescued: r, .. } = d {
                    rescued.push(r.is_some());
                }
            }
        }
        assert_eq!(rescued, [true, false, false]);
        // new state: rescued again
        s.send(job(Kind::Project, &bad, 5));
        assert!(s.wait_idle(Duration::from_secs(10)));
        assert!(matches!(s.poll().pop(), Some(Done::Failed { rescued: Some(_), .. })));
    }

    pub(crate) fn test_app() -> (crate::app::App, tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
        let g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        std::env::set_var("CUTLINE_NO_UPDATE", "1");
        std::env::set_var("CUTLINE_NO_ONBOARDING", "1");
        let app = crate::app::App::new(&eframe::CreationContext::_new_kittest(egui::Context::default()), None);
        (app, home, g)
    }

    /// Quit only once the state is really on disk; if writing fails,
    /// the project stays open (otherwise the changes would be gone after the reset).
    #[test]
    fn leaving_waits_for_the_write_and_stays_open_on_failure() {
        let (mut app, home, _g) = test_app();
        app.set_doc(Project::new("Verlassen"));
        let bad = home.path().join("ordner.cutline");
        std::fs::create_dir_all(&bad).unwrap();
        app.project_path = Some(bad);
        assert!(!app.flush_before_leaving());
        assert!(app.saver.autosave_paused(), "after the error autosave pauses");
        let good = home.path().join("gut.cutline");
        app.project_path = Some(good.clone());
        assert!(app.flush_before_leaving());
        assert_eq!(Project::from_json(&std::fs::read_to_string(&good).unwrap()).unwrap().name, "Verlassen");
    }

    /// A second window on a project that another running window has open keeps its hands off
    /// the file and the lock; its edits go to the backups, „Speichern unter“ elsewhere works.
    #[test]
    fn second_window_never_writes_a_project_open_elsewhere() {
        let (mut app, home, _g) = test_app();
        // another running Cutline: a child process that waits on its stdin
        let mut other = if cfg!(windows) { std::process::Command::new("cmd") } else { std::process::Command::new("cat") };
        if cfg!(windows) {
            other.args(["/c", "pause"]);
        }
        let mut other = other.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn().unwrap();
        let p = home.path().join("a.cutline");
        std::fs::write(&p, Project::new("A").to_json()).unwrap();
        let lock = crate::paths::lock_path(&p);
        std::fs::write(&lock, other.id().to_string()).unwrap();
        app.set_doc(Project::new("B"));
        app.project_path = Some(p.clone());
        app.take_lock(&p);
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), other.id().to_string(), "lock stays with the other window");
        app.save(false);
        assert!(app.flush_before_leaving(), "leaving must not get stuck");
        let _ = other.kill();
        let _ = other.wait();
        // even once the other window is gone, the stale state is not written back
        assert!(!app.write_project(&p));
        assert_eq!(Project::from_json(&std::fs::read_to_string(&p).unwrap()).unwrap().name, "A");
        let rescued = std::fs::read_dir(crate::project_tools::backup_dir(&p)).unwrap().flatten().next().unwrap().path();
        assert_eq!(Project::from_json(&std::fs::read_to_string(rescued).unwrap()).unwrap().name, "B");
        // another path is written normally and its lock taken; every save refreshes the lock
        let q = home.path().join("b.cutline");
        assert!(app.write_project(&q));
        assert!(app.saver.wait_idle(Duration::from_secs(10)));
        let ql = crate::paths::lock_path(&q);
        assert_eq!(std::fs::read_to_string(&ql).unwrap(), std::process::id().to_string());
        let old = std::time::SystemTime::now() - Duration::from_secs(13 * 3600);
        std::fs::File::options().write(true).open(&ql).unwrap().set_modified(old).unwrap();
        assert!(app.write_project(&q));
        assert!(app.saver.wait_idle(Duration::from_secs(10)));
        assert!(std::fs::metadata(&ql).unwrap().modified().unwrap().elapsed().unwrap() < Duration::from_secs(3600));
    }
}
