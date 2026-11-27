//! Cutline core: data model, edit operations and undo history.
//!
//! Every change to the project goes through [`EditOp`] and [`Document::apply`], whether it
//! comes from the UI, a plugin or the MCP endpoint. That makes everything undoable
//! and visible in the history with its source.

mod document;
mod fx;
pub mod legacy;
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
    track_drop_gap, kind_steps, shift_tracks, shift_targets,
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("Clip {0} not found")]
    ClipNotFound(ClipId),
    #[error("Track {0} not found")]
    TrackNotFound(TrackId),
    #[error("Media {0} not found")]
    MediaNotFound(MediaId),
    #[error("Track is locked")]
    TrackLocked,
    #[error("Split point is outside the clip")]
    InvalidSplit,
    #[error("Invalid duration")]
    InvalidDuration,
    #[error("Invalid value")]
    InvalidValue,
    #[error("Clip type does not fit the track ({clip:?} on {track:?})")]
    KindMismatch { clip: TrackKind, track: TrackKind },
    #[error("Nothing to undo")]
    NothingToUndo,
    #[error("Nothing to redo")]
    NothingToRedo,
}

pub type Result<T> = std::result::Result<T, CoreError>;
