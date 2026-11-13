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

/// Format anhand der Datei erkennen und importieren.
pub fn import(path: &Path) -> Result<(Project, Report)> {
    let name = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if path.is_dir() || path.file_name().is_some_and(|n| n == "draft_content.json" || n == "draft_info.json") {
        return capcut::import(path);
    }
    match name.as_str() {
        "fcpxml" | "xml" => fcpxml::import(path),
        "json" => capcut::import(path),
        _ => Err("Unbekanntes Projektformat".into()),
    }
}

/// Dateiendungen für den Öffnen-Dialog
pub const EXTENSIONS: &[&str] = &["json", "fcpxml", "xml"];
