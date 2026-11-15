//! Cutline-Kern: Datenmodell, Edit-Operationen und Undo-Verlauf.
//!
//! Jede Änderung am Projekt läuft über [`EditOp`] und [`Document::apply`], egal ob sie
//! aus der UI, einem Plugin oder dem MCP-Endpunkt kommt. Dadurch ist alles rückgängig
//! machbar und im Verlauf mit seiner Quelle sichtbar.

mod document;
mod fx;
mod model;
mod ops;
mod paste;
mod time;
mod tracks;

pub use document::{Document, HistoryEntry, OpSource};
pub use fx::*;
pub use model::*;
pub use ops::{gaps, ripple_ops, Edge, EditOp, OpOutput};
pub use paste::{paste_ops, paste_splits, paste_targets};
pub use time::{Rational, Time};
pub use tracks::{
    display_order, fits, gap_index, gap_range, move_to_new_tracks, move_track_to, new_track_gap,
    track_drop_gap,
};

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
    #[error("Ungültiger Wert")]
    InvalidValue,
    #[error("Clip-Typ passt nicht zur Spur ({clip:?} auf {track:?})")]
    KindMismatch { clip: TrackKind, track: TrackKind },
    #[error("Nichts zum Rückgängigmachen")]
    NothingToUndo,
    #[error("Nichts zum Wiederherstellen")]
    NothingToRedo,
}

pub type Result<T> = std::result::Result<T, CoreError>;
