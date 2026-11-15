//! Speichern im Hintergrund: Projekt, Autosave und Sicherungen werden als Momentaufnahme an
//! einen eigenen Thread gegeben, der serialisiert und atomisch schreibt. Die Oberfläche wartet
//! nie auf die Platte. Mehrere wartende Speicherungen derselben Datei: nur die neueste zählt.
//! Schlägt das Schreiben fehl, landet der Stand in den Sicherungen (kein Datenverlust).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_core::Project;
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Projektdatei (Speichern, Autosave)
    Project,
    /// Sicherung im Sicherungsordner des Projekts, höchstens `keep` behalten
    Backup { keep: usize },
}

pub struct Job {
    pub kind: Kind,
    /// Projektdatei (bei Sicherungen: das Projekt, zu dem sie gehört)
    pub path: PathBuf,
    pub project: Project,
    pub revision: u64,
}

#[derive(Debug)]
pub enum Done {
    Saved,
    /// `rescued`: wohin der Stand stattdessen geschrieben wurde
    Failed { kind: Kind, revision: u64, err: String, rescued: Option<PathBuf> },
}

/// Wartende Aufträge zusammenfassen: von mehreren Speicherungen derselben Projektdatei bleibt
/// nur die letzte (an deren Stelle); Sicherungen bleiben alle.
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
            crate::paths::write_atomic(&job.path, json).map_err(|e| e.to_string())
        }
        Kind::Backup { keep } => crate::project_tools::write_backup(&job.path, &json, keep).map(|_| ()).map_err(|e| e.to_string()),
    }
}

/// Fehlgeschlagene Projektdatei: Stand in die Sicherungen retten
fn rescue(job: &Job) -> Option<PathBuf> {
    let dir = crate::project_tools::backup_dir(&job.path);
    std::fs::create_dir_all(&dir).ok()?;
    let p = dir.join(format!("unsaved-{}.cutline", crate::paths::unix_secs()));
    crate::paths::write_atomic(&p, job.project.to_json()).ok().map(|_| p)
}

fn run(job: Job) -> Done {
    let t = Instant::now();
    let r = write(&job);
    if crate::app::perf() {
        eprintln!("perf: speichern {:?} {:.0} ms im Hintergrund", job.kind, t.elapsed().as_secs_f64() * 1000.0);
    }
    match r {
        Ok(()) => Done::Saved,
        Err(err) => {
            let rescued = (job.kind == Kind::Project).then(|| rescue(&job)).flatten();
            Done::Failed { kind: job.kind, revision: job.revision, err, rescued }
        }
    }
}

pub struct Saver {
    tx: Sender<Job>,
    rx: Receiver<Done>,
    /// noch nicht geschriebene Aufträge
    pending: Arc<AtomicUsize>,
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
                while let Ok(first) = jobs.recv() {
                    let mut batch = vec![first];
                    batch.extend(jobs.try_iter());
                    let n = batch.len();
                    for job in coalesce(batch) {
                        let _ = done.send(run(job));
                    }
                    p.fetch_sub(n, Ordering::SeqCst);
                    ctx.request_repaint();
                }
            })
            .expect("Speicher-Thread");
        Self { tx, rx, pending }
    }

    pub fn send(&self, job: Job) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        if let Err(e) = self.tx.send(job) {
            // Thread weg (sollte nie passieren): dann eben hier schreiben
            self.pending.fetch_sub(1, Ordering::SeqCst);
            let _ = run(e.0);
        }
    }

    pub fn busy(&self) -> bool {
        self.pending.load(Ordering::SeqCst) > 0
    }

    /// Fertige Speicherungen abholen.
    pub fn poll(&self) -> Vec<Done> {
        self.rx.try_iter().collect()
    }

    /// Beim Beenden: warten, bis alles geschrieben ist (höchstens `max`).
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
    /// Fertige Speicherungen abholen (jedes Bild): Fehler melden (höchstens einmal je Minute),
    /// das Projekt gilt dann wieder als ungespeichert.
    pub fn poll_saves(&mut self) {
        static LAST_ERR: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
        for d in self.saver.poll() {
            let Done::Failed { kind, revision, err, rescued, .. } = d else { continue };
            if kind == Kind::Project && self.saved_revision == revision {
                self.saved_revision = crate::app::UNSAVED;
            }
            let mut le = LAST_ERR.lock().unwrap_or_else(|e| e.into_inner());
            if le.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
                *le = Some(Instant::now());
                drop(le);
                self.toast_err(format!("Projekt konnte nicht gespeichert werden: {err}"));
                if let Some(r) = rescued {
                    self.toast_err(format!("Stand gesichert in {}", r.display()));
                }
            }
        }
    }

    /// Sicherung des aktuellen Stands (im Hintergrund).
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
mod tests {
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
        // Ziel ist ein Ordner: Schreiben scheitert, der Stand landet in den Sicherungen
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
}
