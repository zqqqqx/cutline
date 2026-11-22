//! Adopt projects from other editing programs.
//!
//! - **CapCut** (desktop drafts, `draft_content.json`): video, image, audio and text clips with
//!   times, speed, volume, position, size, rotation, opacity, flip; text content,
//!   color and size.
//! - **FCPXML** (Final Cut Pro, DaVinci Resolve): clips, times, tracks.
//!
//! Custom effects, filters, stickers and templates of other programs cannot be transferred;
//! the report names what was skipped.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cl_core::Project;

pub mod capcut;
pub mod fcpxml;

/// Result of an import.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Report {
    pub clips: usize,
    pub texts: usize,
    /// Skipped elements: (kind, count)
    pub skipped: Vec<(String, usize)>,
    /// Missing media files
    pub missing: Vec<PathBuf>,
    /// Font files of the texts (adopted into Cutline so the texts look the same)
    pub fonts: Vec<PathBuf>,
    /// Existing files that could not be read
    pub failed: Vec<PathBuf>,
    /// Resolved compound clips (nested drafts)
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

/// Error text of a cancelled import
pub const CANCELLED: &str = "Abgebrochen";

/// Steps of an import in their order. The app takes care of `Save` (creating the project).
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

/// State of a running import.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub step: Step,
    /// units of the step done (files, tracks) out of `total`
    pub done: usize,
    pub total: usize,
    /// file/track currently being processed
    pub current: String,
}

impl Status {
    /// Progress of the whole import (0…1): every step counts the same.
    pub fn fraction(&self) -> f32 {
        let i = Step::ALL.iter().position(|s| *s == self.step).unwrap_or(0) as f32;
        let part = if self.total == 0 { 0.0 } else { self.done.min(self.total) as f32 / self.total as f32 };
        (i + part) / Step::ALL.len() as f32
    }
}

/// Progress and cancel of an import, shared between the import thread and the UI.
#[derive(Default)]
pub struct Progress {
    status: Mutex<Status>,
    cancel: AtomicBool,
    /// Wake the UI (repaint), at most about every 30 ms
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

    /// New state; an empty `current` keeps the previous name.
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
        // always report step changes and step ends
        self.wake(new_step || done >= total);
    }

    /// One unit of the current step done.
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

    /// `Err(CANCELLED)` as soon as cancelled (call between two units).
    pub fn check(&self) -> Result<()> {
        if self.cancelled() {
            Err(CANCELLED.into())
        } else {
            Ok(())
        }
    }
}

/// Probe files in parallel (ffprobe), at most `workers` processes at once. Result in
/// the order of `paths`; cancelled → `Err(CANCELLED)`.
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

/// Media for a file missing at import: the clip is kept, „Medien neu verknüpfen“
/// inserts the file later (details from the foreign project, not from the file).
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
        has_alpha: false,
    }
}

/// Simultaneous ffprobe processes during import
pub fn probe_workers() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8)
}

/// Frame rate from a foreign file; 0, negative, infinite or absurd values → `None`.
pub(crate) fn fps_from(fps: f64) -> Option<cl_core::Rational> {
    cl_core::Rational::parse(&fps.to_string()).filter(|r| r.as_f64() <= 1000.0)
}

/// Detect the format from the file and import.
pub fn import(path: &Path) -> Result<(Project, Report)> {
    import_with(path, &Progress::default())
}

/// Like [`import`], with progress and cancel (for the background thread).
pub fn import_with(path: &Path, progress: &Progress) -> Result<(Project, Report)> {
    let (mut p, rep) = import_any(path, progress)?;
    keep_gaps(&mut p);
    Ok((p, rep))
}

/// Keep adopted gaps of the main track: the magnet would push them together on the first change,
/// the secondary tracks would stay put – picture and audio would drift apart.
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
