//! Cutline-Kern: Datenmodell, Edit-Operationen und Undo-Verlauf.
//!
//! Jede Änderung am Projekt läuft über [`EditOp`] und [`Document::apply`], egal ob sie
//! aus der UI, einem Plugin oder dem MCP-Endpunkt kommt. Dadurch ist alles rückgängig
//! machbar und im Verlauf mit seiner Quelle sichtbar.

mod document;
mod fx;
mod model;
mod ops;
mod time;

pub use document::{Document, HistoryEntry, OpSource};
pub use fx::*;
pub use model::*;
pub use ops::{Edge, EditOp, OpOutput};
pub use time::{Rational, Time};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("Clip {0} nicht gefunden")]
    ClipNotFound(ClipId),
    #[error("Spur {0} nicht gefunden")]
    TrackNotFound(TrackId),
    #[error("Medium {0} nicht gefunden")]
    MediaNotFound(MediaId),
    #[error("Spur ist gesperrt")]
    TrackLocked,
    #[error("Schnittpunkt liegt nicht innerhalb des Clips")]
    InvalidSplit,
    #[error("Ungültige Dauer")]
    InvalidDuration,
    #[error("Clip-Typ passt nicht zur Spur ({clip:?} auf {track:?})")]
    KindMismatch { clip: TrackKind, track: TrackKind },
    #[error("Nichts zum Rückgängigmachen")]
    NothingToUndo,
    #[error("Nichts zum Wiederherstellen")]
    NothingToRedo,
}

pub type Result<T> = std::result::Result<T, CoreError>;
