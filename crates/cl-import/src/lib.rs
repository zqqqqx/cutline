//! Projekte anderer Schnittprogramme übernehmen.
//!
//! - **CapCut** (Desktop-Entwürfe, `draft_content.json`): Video-, Bild-, Audio- und Textclips mit
//!   Zeiten, Tempo, Lautstärke, Position, Größe, Drehung, Deckkraft, Spiegeln; Textinhalt,
//!   Farbe und Größe.
//! - **FCPXML** (Final Cut Pro, DaVinci Resolve): Clips, Zeiten, Spuren.
//!
//! Eigene Effekte, Filter, Sticker und Vorlagen anderer Programme lassen sich nicht übertragen;
//! der Bericht nennt, was übersprungen wurde.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cl_core::Project;

pub mod capcut;
pub mod fcpxml;

/// Ergebnis eines Imports.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Report {
    pub clips: usize,
    pub texts: usize,
    /// Übersprungene Elemente: (Art, Anzahl)
    pub skipped: Vec<(String, usize)>,
    /// Fehlende Mediendateien
    pub missing: Vec<PathBuf>,
    /// Schriftdateien der Texte (werden in Cutline übernommen, damit die Texte gleich aussehen)
    pub fonts: Vec<PathBuf>,
    /// Vorhandene Dateien, die sich nicht lesen ließen
    pub failed: Vec<PathBuf>,
    /// Aufgelöste zusammengesetzte Clips (verschachtelte Entwürfe)
    pub nested: usize,
}

impl Report {
    pub fn skip(&mut self, what: &str) {
        match self.skipped.iter_mut().find(|(w, _)| w == what) {
            Some((_, n)) => *n += 1,
            None => self.skipped.push((what.to_string(), 1)),
        }
    }
}

pub type Result<T> = std::result::Result<T, String>;

/// Fehlertext eines abgebrochenen Imports
pub const CANCELLED: &str = "Abgebrochen";

/// Schritte eines Imports in ihrer Reihenfolge. `Save` übernimmt die App (Projekt anlegen).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    #[default]
    Read,
    Find,
    Probe,
    Fonts,
    Timeline,
    Save,
}

impl Step {
    pub const ALL: [Step; 6] = [Step::Read, Step::Find, Step::Probe, Step::Fonts, Step::Timeline, Step::Save];
}

/// Stand eines laufenden Imports.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub step: Step,
    /// erledigte Einheiten des Schritts (Dateien, Spuren) von `total`
    pub done: usize,
    pub total: usize,
    /// gerade bearbeitete Datei/Spur
    pub current: String,
}

impl Status {
    /// Fortschritt des ganzen Imports (0…1): jeder Schritt zählt gleich.
    pub fn fraction(&self) -> f32 {
        let i = Step::ALL.iter().position(|s| *s == self.step).unwrap_or(0) as f32;
        let part = if self.total == 0 { 0.0 } else { self.done.min(self.total) as f32 / self.total as f32 };
        (i + part) / Step::ALL.len() as f32
    }
}

/// Fortschritt und Abbruch eines Imports, zwischen Import-Thread und Oberfläche geteilt.
#[derive(Default)]
pub struct Progress {
    status: Mutex<Status>,
    cancel: AtomicBool,
    /// Oberfläche wecken (Neuzeichnen), höchstens etwa alle 30 ms
    wake: Option<Box<dyn Fn() + Send + Sync>>,
    last_wake: Mutex<Option<Instant>>,
}

impl Progress {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self { wake: Some(Box::new(wake)), ..Default::default() }
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Neuer Stand; `current` leer lässt den bisherigen Namen stehen.
    pub fn set(&self, step: Step, done: usize, total: usize, current: &str) {
        let new_step = {
            let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            let new_step = s.step != step;
            if new_step {
                s.current.clear();
            }
            s.step = step;
            s.done = done;
            s.total = total;
            if !current.is_empty() {
                s.current = current.to_string();
            }
            new_step
        };
        // Schrittwechsel und Schrittende immer melden
        self.wake(new_step || done >= total);
    }

    /// Eine Einheit des aktuellen Schritts erledigt.
    pub fn tick(&self, current: &str) {
        let last = {
            let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            s.done += 1;
            if !current.is_empty() {
                s.current = current.to_string();
            }
            s.done >= s.total
        };
        self.wake(last);
    }

    fn wake(&self, force: bool) {
        let Some(w) = &self.wake else { return };
        let mut l = self.last_wake.lock().unwrap_or_else(|e| e.into_inner());
        if force || l.is_none_or(|t| t.elapsed() >= Duration::from_millis(30)) {
            *l = Some(Instant::now());
            drop(l);
            w();
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// `Err(CANCELLED)`, sobald abgebrochen wurde (zwischen zwei Einheiten aufrufen).
    pub fn check(&self) -> Result<()> {
        if self.cancelled() {
            Err(CANCELLED.into())
        } else {
            Ok(())
        }
    }
}

/// Dateien parallel prüfen (ffprobe), höchstens `workers` Prozesse gleichzeitig. Ergebnis in
/// der Reihenfolge von `paths`; abgebrochen → `Err(CANCELLED)`.
pub fn probe_all(
    paths: &[PathBuf],
    workers: usize,
    progress: &Progress,
    probe: impl Fn(&Path) -> Result<cl_core::MediaRef> + Sync,
) -> Result<Vec<Result<cl_core::MediaRef>>> {
    let next = AtomicUsize::new(0);
    let out: Vec<Mutex<Option<Result<cl_core::MediaRef>>>> = paths.iter().map(|_| Mutex::new(None)).collect();
    progress.set(Step::Probe, 0, paths.len(), "");
    std::thread::scope(|s| {
        for _ in 0..workers.clamp(1, paths.len().max(1)) {
            s.spawn(|| loop {
                if progress.cancelled() {
                    return;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(p) = paths.get(i) else { return };
                let r = probe(p);
                *out[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                progress.tick(&p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            });
        }
    });
    progress.check()?;
    Ok(out
        .into_iter()
        .map(|m| m.into_inner().unwrap_or_else(|e| e.into_inner()).unwrap_or_else(|| Err("nicht geprüft".into())))
        .collect())
}

/// Medium für eine beim Import fehlende Datei: der Clip bleibt erhalten, „Medien neu verknüpfen“
/// setzt die Datei später ein (Angaben aus dem Fremdprojekt, nicht aus der Datei).
pub(crate) fn placeholder(path: PathBuf, kind: cl_core::MediaKind, duration: cl_core::Time, width: u32, height: u32) -> cl_core::MediaRef {
    let sound = kind != cl_core::MediaKind::Image;
    cl_core::MediaRef {
        id: cl_core::MediaId::new(),
        path,
        kind,
        duration,
        width,
        height,
        fps: None,
        has_audio: sound,
        audio_streams: u32::from(sound),
        video_codec: None,
        audio_codec: None,
    }
}

/// Gleichzeitige ffprobe-Prozesse beim Import
pub fn probe_workers() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8)
}

/// Bildrate aus einer Fremddatei; 0, negative, unendliche oder absurde Werte → `None`.
pub(crate) fn fps_from(fps: f64) -> Option<cl_core::Rational> {
    cl_core::Rational::parse(&fps.to_string()).filter(|r| r.as_f64() <= 1000.0)
}

/// Format anhand der Datei erkennen und importieren.
pub fn import(path: &Path) -> Result<(Project, Report)> {
    import_with(path, &Progress::default())
}

/// Wie [`import`], mit Fortschritt und Abbruch (für den Hintergrund-Thread).
pub fn import_with(path: &Path, progress: &Progress) -> Result<(Project, Report)> {
    let (mut p, rep) = import_any(path, progress)?;
    keep_gaps(&mut p);
    Ok((p, rep))
}

/// Übernommene Lücken der Hauptspur behalten: der Magnet schöbe sie bei der ersten Änderung
/// zusammen, die Nebenspuren blieben stehen – Bild und Ton liefen auseinander.
fn keep_gaps(p: &mut Project) {
    if let Some(t) = p.timeline.tracks.iter().find(|t| t.main) {
        let mut cursor = cl_core::Time::ZERO;
        if t.clips.iter().any(|c| std::mem::replace(&mut cursor, c.end()) != c.start) {
            p.settings.main_track_magnet = false;
        }
    }
}

fn import_any(path: &Path, progress: &Progress) -> Result<(Project, Report)> {
    let name = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if path.is_dir()
        || path
            .file_name()
            .is_some_and(|n| n == "draft_content.json" || n == "draft_info.json")
    {
        return capcut::import_with(path, progress);
    }
    match name.as_str() {
        "fcpxml" | "xml" => fcpxml::import_with(path, progress),
        "json" => capcut::import_with(path, progress),
        _ => Err("Unbekanntes Projektformat".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::{Clip, ClipContent, Document, Edge, EditOp, OpSource, TextStyle, Time, Track, TrackKind};

    #[test]
    fn imported_gaps_survive_first_edit() {
        let s = Time::from_secs_f64;
        let mut p = Project::new("x");
        let mut t = Track::new(TrackKind::Video);
        t.main = true;
        for at in [0.0, 5.0] {
            t.clips.push(Clip::new(ClipContent::Text(TextStyle::default()), s(at), s(2.0)));
        }
        let (a, b) = (t.clips[0].id, t.clips[1].id);
        p.timeline.tracks.push(t);
        keep_gaps(&mut p);
        let mut d = Document::new(p);
        d.apply(EditOp::TrimClip { clip: a, edge: Edge::End, to: s(1.0) }, OpSource::Ui).unwrap();
        assert_eq!(d.project().find_clip(b).unwrap().1.start, s(5.0));
    }
}
