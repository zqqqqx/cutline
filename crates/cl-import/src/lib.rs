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

/// Bildrate aus einer Fremddatei; 0, negative, unendliche oder absurde Werte → `None`.
pub(crate) fn fps_from(fps: f64) -> Option<cl_core::Rational> {
    cl_core::Rational::parse(&fps.to_string()).filter(|r| r.as_f64() <= 1000.0)
}

/// Format anhand der Datei erkennen und importieren.
pub fn import(path: &Path) -> Result<(Project, Report)> {
    let (mut p, rep) = import_any(path)?;
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

fn import_any(path: &Path) -> Result<(Project, Report)> {
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
        return capcut::import(path);
    }
    match name.as_str() {
        "fcpxml" | "xml" => fcpxml::import(path),
        "json" => capcut::import(path),
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
