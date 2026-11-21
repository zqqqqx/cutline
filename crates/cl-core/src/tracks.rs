//! Arranging tracks: display order, position for a new track, dragging a track to another position.
//!
//! "Row" = position in the display from the top (0 = topmost track), "gap" `g` = position directly
//! above row `g` (`g` = number of rows: below the lowest). Ordering rules: visual tracks (all
//! but audio) lie above all audio tracks, no visual track lies below the main track.

use crate::document::{Document, OpSource};
use crate::model::*;
use crate::ops::EditOp;
use crate::time::Time;
use crate::{CoreError, Result};

/// Track indices in display order: visual tracks with a higher index on top, below them audio in index order.
pub fn display_order(tracks: &[Track]) -> Vec<usize> {
    let audio = |i: &usize| tracks[*i].kind == TrackKind::Audio;
    let mut o: Vec<usize> = (0..tracks.len()).rev().filter(|i| !audio(i)).collect();
    o.extend((0..tracks.len()).filter(audio));
    o
}

/// Does the track take clips of kind `kind` (matching kind, not locked)?
pub fn fits(t: &Track, kind: TrackKind) -> bool {
    t.kind.accepts(kind) && !t.locked
}

/// Allowed gaps (from, to inclusive) for a track of kind `kind`.
pub fn gap_range(tracks: &[Track], kind: TrackKind) -> (usize, usize) {
    let ord = display_order(tracks);
    let visual = tracks.iter().filter(|t| t.kind != TrackKind::Audio).count();
    if kind == TrackKind::Audio {
        (visual, tracks.len())
    } else {
        let main = ord[..visual].iter().position(|&i| tracks[i].main);
        (0, main.unwrap_or(visual))
    }
}

/// Gap for a new track of kind `kind` when dropped over row `row` (in the upper half:
/// `upper`): directly above or below, otherwise the nearest allowed position.
pub fn new_track_gap(tracks: &[Track], row: usize, upper: bool, kind: TrackKind) -> usize {
    let (lo, hi) = gap_range(tracks, kind);
    (row + usize::from(!upper)).clamp(lo, hi)
}

/// Index for [`EditOp::InsertTrack`] so the new track stands at gap `gap` in the display.
pub fn gap_index(tracks: &[Track], gap: usize, kind: TrackKind) -> usize {
    let ord = display_order(tracks);
    let visual = tracks.iter().filter(|t| t.kind != TrackKind::Audio).count();
    if kind != TrackKind::Audio {
        match gap.min(visual) {
            g if g < visual => ord[g] + 1, // directly above row g
            0 => 0,
            g => ord[g - 1], // below the lowest visual track
        }
    } else {
        match gap.clamp(visual, tracks.len()) {
            g if g < tracks.len() => ord[g], // directly before audio track g
            g if g > visual => ord[g - 1] + 1,
            _ => tracks.len(),
        }
    }
}

/// Target position when dragging the track in row `row` to gap `gap`: the nearest allowed one;
/// `None` if the track may not be moved (main track) or would stay in place.
pub fn track_drop_gap(tracks: &[Track], row: usize, gap: usize) -> Option<usize> {
    let t = &tracks[*display_order(tracks).get(row)?];
    if t.main {
        return None;
    }
    let (lo, hi) = gap_range(tracks, t.kind);
    let g = gap.clamp(lo, hi);
    (g != row && g != row + 1).then_some(g)
}

/// Move the track in row `row` to gap `gap` – as one history step of
/// [`EditOp::MoveTrack`] steps (neighbours of the same kind are skipped).
pub fn move_track_to(tracks: &[Track], row: usize, gap: usize) -> Option<EditOp> {
    let g = track_drop_gap(tracks, row, gap)?;
    let ord = display_order(tracks);
    let t = &tracks[ord[row]];
    let audio = t.kind == TrackKind::Audio;
    let peer = |k: &usize| (tracks[ord[*k]].kind == TrackKind::Audio) == audio && !tracks[ord[*k]].main;
    let (up, n) = if g < row { (true, (g..row).filter(peer).count()) } else { (false, (row + 1..g).filter(peer).count()) };
    Some(EditOp::Batch {
        label: "Spur verschieben".into(),
        ops: (0..n).map(|_| EditOp::MoveTrack { track: t.id, up }).collect(),
    })
}

/// Move clips (id, new start) to new tracks next to row `row`: one track per clip kind
/// at the position from [`new_track_gap`]; clips of the same kind land in it together. If clips
/// of one kind from different tracks would overlap there, that kind gets one new track per source
/// track instead, in the sources' display order (what was above stays above). The result does not
/// depend on the order of `moves`. One history step; if something fails, the project stays unchanged.
pub fn move_to_new_tracks(doc: &mut Document, row: usize, upper: bool, moves: &[(ClipId, Time)]) -> Result<()> {
    let p = doc.project();
    let tracks = &p.timeline.tracks;
    let ord = display_order(tracks);
    // per clip: kind, display row of its source track, new range
    let mut info = Vec::new();
    for (id, start) in moves {
        let (t, c) = p.find_clip(*id).ok_or(CoreError::ClipNotFound(*id))?;
        let src = ord.iter().position(|&i| tracks[i].id == t.id).unwrap_or(0);
        info.push((c.kind(), src, *start, *start + c.duration));
    }
    // per kind: insert index and the source rows that get a track of their own (sorted; one entry = shared track)
    let mut kinds: Vec<(TrackKind, usize, Vec<usize>)> = Vec::new();
    for (k, ..) in &info {
        if kinds.iter().any(|x| x.0 == *k) {
            continue;
        }
        let same: Vec<_> = info.iter().filter(|x| x.0 == *k).collect();
        let clash = same.iter().any(|a| same.iter().any(|b| a.1 != b.1 && a.2 < b.3 && b.2 < a.3));
        let mut rows: Vec<usize> = same.iter().map(|x| x.1).collect();
        rows.sort_unstable();
        rows.dedup();
        if !clash {
            rows.truncate(1);
        }
        kinds.push((*k, gap_index(tracks, new_track_gap(tracks, row, upper, *k), *k), rows));
    }
    // highest index first: later inserts don't shift the ones already created against each other
    kinds.sort_by_key(|k| std::cmp::Reverse(k.1));
    doc.transaction(|d| {
        // per kind: the new tracks in display order, paired with the source rows
        let mut new: Vec<(TrackKind, Vec<usize>, Vec<TrackId>)> = Vec::new();
        for (kind, index, rows) in kinds {
            let mut ids = Vec::new();
            for _ in &rows {
                if let crate::OpOutput::Track(t) = d.apply(EditOp::InsertTrack { index, kind }, OpSource::Ui)? {
                    ids.push(t);
                }
            }
            new.push((kind, rows, ids));
        }
        let tr = &d.project().timeline.tracks;
        let ord = display_order(tr);
        for n in &mut new {
            n.2.sort_by_key(|id| ord.iter().position(|&i| tr[i].id == *id));
        }
        let ops = moves
            .iter()
            .zip(&info)
            .map(|((clip, start), (kind, src, ..))| {
                let track = new.iter().find(|n| n.0 == *kind).and_then(|n| n.2.get(n.1.iter().position(|r| r == src).unwrap_or(0)).copied());
                EditOp::MoveClip { clip: *clip, track, start: *start }
            })
            .collect();
        d.apply(EditOp::Batch { label: "Auf neue Spur verschieben".into(), ops }, OpSource::Ui)?;
        Ok(())
    })
}

/// Rows the track `to` lies `steps` tracks of the same kind below `from` (negative: above), counted
/// in display order among the tracks of `from`'s kind; `None` if `to` is of another kind.
pub fn kind_steps(tracks: &[Track], from: TrackId, to: TrackId) -> Option<isize> {
    let kind = tracks.iter().find(|t| t.id == from)?.kind;
    let peers: Vec<TrackId> = display_order(tracks).into_iter().map(|i| &tracks[i]).filter(|t| t.kind == kind).map(|t| t.id).collect();
    let pos = |id| peers.iter().position(|t| *t == id).map(|i| i as isize);
    Some(pos(to)? - pos(from)?)
}

/// Dragging a selection (clip, new start) `steps` tracks down (negative: up): every clip goes the
/// same number of tracks among the tracks of its own track's kind, so video stays on video, audio
/// on audio and the arrangement of the group stays. All or nothing: `None` if a clip would leave
/// the tracks of its kind, land on or leave a locked track, or overlap a clip that stays on the
/// target (the main track with magnet makes room itself). Returns the target per clip and the
/// move as one batch.
pub fn shift_tracks(p: &Project, moves: &[(ClipId, Time)], steps: isize) -> Option<(Vec<(ClipId, TrackId)>, EditOp)> {
    let tracks = &p.timeline.tracks;
    let ord = display_order(tracks);
    let row = |id: TrackId| ord.iter().position(|&i| tracks[i].id == id);
    let moving: Vec<ClipId> = moves.iter().map(|m| m.0).collect();
    let mut plan = Vec::new();
    for (id, start) in moves {
        let (src, c) = p.find_clip(*id)?;
        let peers: Vec<&Track> = ord.iter().map(|&i| &tracks[i]).filter(|t| t.kind == src.kind).collect();
        let at = peers.iter().position(|t| t.id == src.id)?;
        let t = *peers.get(at.checked_add_signed(steps)?)?;
        let end = *start + c.duration;
        let free = t.clips.iter().filter(|o| !moving.contains(&o.id)).all(|o| end <= o.start || *start >= o.end());
        // its own track keeps a clip of another kind (legacy projects), as a single move does
        if src.locked || !(t.id == src.id || fits(t, c.kind())) || !(free || (t.main && p.settings.main_track_magnet)) {
            return None;
        }
        plan.push((*id, c.start, *start, src.id, t.id));
    }
    // first park every moving clip on its own track behind everything, then move them over: a clip
    // never meets one that is about to leave, and a track that is also a target never runs empty
    // (`MoveClip` removes emptied tracks). Tracks are filled against the move direction.
    let end = tracks.iter().flat_map(|t| &t.clips).map(|c| c.end()).chain(moves.iter().map(|m| m.1)).max().unwrap_or(Time::ZERO);
    let park = end + Time::from_secs_f64(3600.0);
    plan.sort_by_key(|x| row(x.3));
    if steps < 0 {
        plan.reverse();
    }
    let mut ops: Vec<EditOp> = plan.iter().map(|x| EditOp::MoveClip { clip: x.0, track: None, start: park + x.1 }).collect();
    let magnet = tracks.iter().find(|t| t.main && p.settings.main_track_magnet).filter(|m| plan.iter().any(|x| x.4 == m.id));
    ops.extend(plan.iter().filter(|x| magnet.is_none_or(|m| x.4 != m.id)).map(|x| EditOp::MoveClip { clip: x.0, track: Some(x.4), start: x.2 }));
    if let Some(m) = magnet {
        // magnet main track: only the order counts (one by one, the insert by middles of a packed
        // track can swap the group) – append all its clips in the order of their new starts
        let mut line: Vec<(Time, ClipId)> = m.clips.iter().filter(|c| !moving.contains(&c.id)).map(|c| (c.start, c.id)).collect();
        line.extend(plan.iter().filter(|x| x.4 == m.id).map(|x| (x.2, x.0)));
        line.sort_by_key(|x| x.0);
        ops.extend(line.into_iter().map(|(_, clip)| EditOp::MoveClip { clip, track: Some(m.id), start: park + park }));
    }
    Some((plan.iter().map(|x| (x.0, x.4)).collect(), EditOp::Batch { label: "Clips verschieben".into(), ops }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use TrackKind::*;

    /// Tracks in display order from the top; `M` = main track (video), `L` = locked.
    fn proj(spec: &[&str]) -> Project {
        let mut p = Project::new("T");
        let mut vis = Vec::new();
        let mut aud = Vec::new();
        for s in spec {
            let mut t = Track::new(match s.chars().next() {
                Some('A') => Audio,
                Some('T') => Text,
                Some('E') => Effect,
                Some('F') => Filter,
                Some('S') => Sticker,
                _ => Video,
            });
            t.main = s.starts_with('M');
            t.locked = s.ends_with('L');
            t.name = Some(s.to_string());
            if t.kind == Audio {
                aud.push(t)
            } else {
                vis.push(t)
            }
        }
        vis.reverse();
        p.timeline.tracks = vis;
        p.timeline.tracks.extend(aud);
        p
    }
    fn names(p: &Project) -> Vec<String> {
        let t = &p.timeline.tracks;
        display_order(t).into_iter().map(|i| t[i].name.clone().unwrap_or_else(|| format!("neu {:?}", t[i].kind))).collect()
    }
    fn clip(p: &mut Project, row: usize, at: f64) -> ClipId {
        let i = display_order(&p.timeline.tracks)[row];
        let kind = p.timeline.tracks[i].kind;
        let content = match kind {
            Text => ClipContent::Text(TextStyle::default()),
            _ => {
                let mk = if kind == Audio { MediaKind::Audio } else { MediaKind::Image };
                let m = MediaRef {
                    id: MediaId::new(),
                    path: "x".into(),
                    kind: mk,
                    duration: Time::from_secs_f64(100.0),
                    width: 1,
                    height: 1,
                    fps: None,
                    has_audio: kind == Audio,
                    audio_streams: (kind == Audio) as u32,
                    video_codec: None,
                    audio_codec: None,
                };
                p.media.push(m.clone());
                ClipContent::Media { media: m.id, kind: mk }
            }
        };
        let t = &mut p.timeline.tracks[i];
        let c = Clip::new(content, Time::from_secs_f64(at), Time::from_secs_f64(1.0));
        let id = c.id;
        t.clips.push(c);
        id
    }

    #[test]
    fn display_order_matches_spec() {
        let p = proj(&["T", "V", "M", "A1", "A2"]);
        assert_eq!(names(&p), ["T", "V", "M", "A1", "A2"]);
    }

    #[test]
    fn new_track_lands_above_or_below_the_row() {
        let p = proj(&["T", "V", "M", "A1", "A2"]);
        let t = &p.timeline.tracks;
        // video over text: upper half above, lower half below
        assert_eq!(new_track_gap(t, 0, true, Video), 0);
        assert_eq!(new_track_gap(t, 0, false, Video), 1);
        // text over the main track (lower half): below would be under the main track → directly above
        assert_eq!(new_track_gap(t, 2, false, Text), 2);
        // audio over visual tracks: topmost allowed position in the audio area
        assert_eq!(new_track_gap(t, 0, true, Audio), 3);
        // image over audio: directly above the main track
        assert_eq!(new_track_gap(t, 4, false, Effect), 2);
        // edges: at the very bottom or very top
        assert_eq!(new_track_gap(t, 4, false, Audio), 5);
        assert_eq!(new_track_gap(t, 3, true, Audio), 3);
    }

    #[test]
    fn insert_index_puts_the_track_at_the_gap() {
        for (spec, row, upper, kind, want) in [
            (&["T", "V", "M", "A1", "A2"][..], 0, true, Video, vec!["neu Video", "T", "V", "M", "A1", "A2"]),
            (&["T", "V", "M", "A1", "A2"][..], 0, false, Audio, vec!["T", "V", "M", "neu Audio", "A1", "A2"]),
            (&["T", "V", "M", "A1", "A2"][..], 3, false, Text, vec!["T", "V", "neu Text", "M", "A1", "A2"]),
            (&["T", "V", "M", "A1", "A2"][..], 3, false, Audio, vec!["T", "V", "M", "A1", "neu Audio", "A2"]),
            (&["T", "V", "M", "A1", "A2"][..], 4, false, Audio, vec!["T", "V", "M", "A1", "A2", "neu Audio"]),
            (&["T", "V", "M", "A1", "A2"][..], 1, false, Text, vec!["T", "V", "neu Text", "M", "A1", "A2"]),
            // without main track an image may go to the very bottom of the visual area
            (&["T", "V", "A1"][..], 2, true, Effect, vec!["T", "V", "neu Effect", "A1"]),
            // audio only: visual track goes above
            (&["A1"][..], 0, false, Text, vec!["neu Text", "A1"]),
            // visual only: audio below
            (&["T"][..], 0, true, Audio, vec!["T", "neu Audio"]),
        ] {
            let mut d = Document::new(proj(spec));
            let t = d.project().timeline.tracks.clone();
            let index = gap_index(&t, new_track_gap(&t, row, upper, kind), kind);
            d.apply(EditOp::InsertTrack { index, kind }, OpSource::Ui).unwrap();
            assert_eq!(names(d.project()), want, "{spec:?} Zeile {row} {upper} {kind:?}");
        }
    }

    #[test]
    fn moving_clips_onto_unfitting_or_locked_rows_makes_one_undo_step() {
        let mut p = proj(&["T", "V", "M", "A1L", "A2"]);
        let v = clip(&mut p, 1, 0.0);
        let v2 = clip(&mut p, 1, 3.0);
        let a = clip(&mut p, 4, 1.0);
        clip(&mut p, 1, 6.0);
        clip(&mut p, 4, 5.0); // A2 keeps a clip (MoveClip removes tracks that became empty, as before)
        let mut d = Document::new(p);
        let before = d.project().clone();
        // multi-selection over the locked audio track A1 (lower half): image above the main track,
        // audio directly below A1; distances stay
        let s = |x: f64| Time::from_secs_f64(x);
        move_to_new_tracks(&mut d, 3, false, &[(v, s(10.0)), (v2, s(13.0)), (a, s(11.0))]).unwrap();
        assert_eq!(names(d.project()), ["T", "V", "neu Video", "M", "A1L", "neu Audio", "A2"]);
        let tr = &d.project().timeline.tracks;
        let ord = display_order(tr);
        let starts = |row: usize| tr[ord[row]].clips.iter().map(|c| c.start.as_secs_f64()).collect::<Vec<_>>();
        assert_eq!(starts(2), [10.0, 13.0]);
        assert_eq!(starts(5), [11.0]);
        assert_eq!(starts(1), [6.0], "clips not dragged stay");
        assert_eq!(d.undo_entries().len(), 1);
        d.undo().unwrap();
        assert_eq!(d.project(), &before, "fully reversible");
        d.redo().unwrap();
        assert_eq!(names(d.project()).len(), 7);
    }

    #[test]
    fn stacked_clips_keep_their_order_on_new_tracks_whatever_the_move_order() {
        let s = |x: f64| Time::from_secs_f64(x);
        for n in [2usize, 3] {
            let spec = ["V1", "V2", "V3", "M"];
            let mut p = proj(&spec[3 - n..]);
            // one stacked clip per video track, plus a clip that keeps each source track alive
            let ids: Vec<ClipId> = (0..n).map(|r| clip(&mut p, r, 0.0)).collect();
            for r in 0..n {
                clip(&mut p, r, 50.0);
            }
            let perms: Vec<Vec<usize>> = if n == 2 {
                vec![vec![0, 1], vec![1, 0]]
            } else {
                vec![vec![0, 1, 2], vec![0, 2, 1], vec![1, 0, 2], vec![1, 2, 0], vec![2, 0, 1], vec![2, 1, 0]]
            };
            for perm in perms {
                let mut d = Document::new(p.clone());
                let before = d.project().clone();
                let moves: Vec<(ClipId, Time)> = perm.iter().map(|&i| (ids[i], s(5.0))).collect();
                // over the main track, upper half: the new tracks go directly above it
                move_to_new_tracks(&mut d, n, true, &moves).unwrap();
                let tr = &d.project().timeline.tracks;
                let ord = display_order(tr);
                assert_eq!(ord.len(), 2 * n + 1, "one new track per source track ({perm:?})");
                for (i, id) in ids.iter().enumerate() {
                    let t = &tr[ord[n + i]];
                    assert!(t.clips.iter().any(|c| c.id == *id && c.start == s(5.0)), "clip {i} not on new row {} ({perm:?})", n + i);
                }
                assert_eq!(d.undo_entries().len(), 1);
                d.undo().unwrap();
                assert_eq!(d.project(), &before, "one undo restores everything ({perm:?})");
            }
        }
    }

    #[test]
    fn clips_from_different_tracks_that_do_not_collide_share_one_new_track() {
        let s = |x: f64| Time::from_secs_f64(x);
        let mut p = proj(&["V1", "V2", "M"]);
        let a = clip(&mut p, 0, 0.0);
        let b = clip(&mut p, 1, 0.0);
        clip(&mut p, 0, 50.0);
        clip(&mut p, 1, 50.0);
        let mut d = Document::new(p);
        move_to_new_tracks(&mut d, 2, true, &[(b, s(7.0)), (a, s(5.0))]).unwrap();
        assert_eq!(names(d.project()), ["V1", "V2", "neu Video", "M"]);
    }

    #[test]
    fn failed_move_leaves_nothing_behind() {
        let mut p = proj(&["VL", "M", "A1"]);
        let v = clip(&mut p, 0, 0.0); // lies on a locked track: moving fails
        let mut d = Document::new(p);
        let before = d.project().clone();
        assert!(move_to_new_tracks(&mut d, 2, true, &[(v, Time::from_secs_f64(2.0))]).is_err());
        assert_eq!(d.project(), &before, "no new track");
        assert!(d.undo_entries().is_empty(), "no history step");
    }

    #[test]
    fn track_drag_target_skips_forbidden_spots() {
        let p = proj(&["T", "V", "M", "A1", "A2"]);
        let t = &p.timeline.tracks;
        assert_eq!(track_drop_gap(t, 0, 2), Some(2), "Text unter V");
        assert_eq!(track_drop_gap(t, 0, 4), Some(2), "not below the main track");
        assert_eq!(track_drop_gap(t, 0, 0), None, "onto itself");
        assert_eq!(track_drop_gap(t, 0, 1), None, "onto itself (below)");
        assert_eq!(track_drop_gap(t, 2, 0), None, "main track stays");
        assert_eq!(track_drop_gap(t, 4, 0), Some(3), "audio stays below the visual tracks");
        assert_eq!(track_drop_gap(t, 3, 5), Some(5));
        assert_eq!(track_drop_gap(t, 9, 0), None, "no such row");
    }

    #[test]
    fn moving_a_track_is_one_reversible_step() {
        let mut d = Document::new(proj(&["T", "E", "V", "M", "A1", "A2", "A3"]));
        let before = d.project().clone();
        let op = move_track_to(&d.project().timeline.tracks, 0, 3).unwrap();
        d.apply(op, OpSource::Ui).unwrap();
        assert_eq!(names(d.project()), ["E", "V", "T", "M", "A1", "A2", "A3"]);
        let op = move_track_to(&d.project().timeline.tracks, 6, 0).unwrap();
        d.apply(op, OpSource::Ui).unwrap();
        assert_eq!(names(d.project()), ["E", "V", "T", "M", "A3", "A1", "A2"]);
        let op = move_track_to(&d.project().timeline.tracks, 1, 0).unwrap();
        d.apply(op, OpSource::Ui).unwrap();
        assert_eq!(names(d.project()), ["V", "E", "T", "M", "A3", "A1", "A2"]);
        assert_eq!(d.undo_entries().len(), 3);
        for _ in 0..3 {
            d.undo().unwrap();
        }
        assert_eq!(d.project(), &before);
    }

    #[test]
    fn many_tracks() {
        let spec: Vec<String> = (0..48).map(|i| format!("T{i}")).chain(["M".into(), "A".into()]).collect();
        let spec: Vec<&str> = spec.iter().map(|s| s.as_str()).collect();
        let mut d = Document::new(proj(&spec));
        let op = move_track_to(&d.project().timeline.tracks, 0, 48).unwrap();
        d.apply(op, OpSource::Ui).unwrap();
        let n = names(d.project());
        assert_eq!((n[0].as_str(), n[47].as_str(), n[48].as_str()), ("T1", "T0", "M"));
    }

    /// where each clip is: (track name, start)
    fn at(d: &Document, id: ClipId) -> (String, f64) {
        d.project().find_clip(id).map(|(t, c)| (t.name.clone().unwrap_or_default(), c.start.as_secs_f64())).unwrap()
    }

    #[test]
    fn group_moves_tracks_within_each_kind() {
        let mut p = proj(&["V2", "V1", "M", "A2", "A1"]);
        let (v, v2) = (clip(&mut p, 1, 0.0), clip(&mut p, 1, 3.0));
        let (a, a2) = (clip(&mut p, 4, 0.0), clip(&mut p, 4, 3.0));
        clip(&mut p, 0, 8.0);
        clip(&mut p, 3, 8.0);
        let mut d = Document::new(p);
        let before = d.project().clone();
        let s = |x: f64| Time::from_secs_f64(x);
        let moves = [(v, s(0.0)), (v2, s(3.0)), (a, s(0.0)), (a2, s(3.0))];
        // one track up: video on V2, audio on A2, times unchanged
        let (targets, op) = shift_tracks(d.project(), &moves, -1).unwrap();
        assert_eq!(targets.len(), 4);
        d.apply(op, OpSource::Ui).unwrap();
        assert_eq!([at(&d, v), at(&d, v2), at(&d, a), at(&d, a2)], [("V2".into(), 0.0), ("V2".into(), 3.0), ("A2".into(), 0.0), ("A2".into(), 3.0)]);
        assert_eq!(d.undo_entries().len(), 1);
        d.undo().unwrap();
        assert_eq!(d.project(), &before, "one undo restores all");
        // a clip on the target track in the way: nothing moves
        let mut p = before.clone();
        clip(&mut p, 0, 3.5);
        assert!(shift_tracks(&p, &moves, -1).is_none());
        // audio already on the top audio track: the group does not change tracks (video could)
        let (_, op) = shift_tracks(&before, &moves, -1).unwrap();
        let mut d = Document::new(before.clone());
        d.apply(op, OpSource::Ui).unwrap();
        let ids = [(v, 0.0), (v2, 3.0), (a, 0.0), (a2, 3.0)].map(|x| (x.0, s(x.1)));
        assert!(shift_tracks(d.project(), &ids, -1).is_none(), "A2 is the top audio track");
        assert!(shift_tracks(d.project(), &ids, 2).is_none(), "below A1 there is none");
        // locked target: refused
        let mut p = before.clone();
        p.timeline.tracks.iter_mut().find(|t| t.name.as_deref() == Some("A2")).unwrap().locked = true;
        assert!(shift_tracks(&p, &moves, -1).is_none());
    }

    #[test]
    fn group_shift_through_its_own_tracks() {
        // clips on V3 and V2 at the same time, one track down: V3 → V2 (still occupied by the leaving
        // clip while planning), V2 → V1 next to a clip that stays
        let mut p = proj(&["V3", "V2", "V1", "M", "A1"]);
        let (x, y) = (clip(&mut p, 0, 0.0), clip(&mut p, 1, 0.0));
        clip(&mut p, 2, 5.0);
        clip(&mut p, 0, 5.0); // V3 keeps a clip, otherwise it is removed when emptied
        let mut d = Document::new(p);
        let s = |x: f64| Time::from_secs_f64(x);
        let t = &d.project().timeline.tracks;
        let id = |n: &str| t.iter().find(|t| t.name.as_deref() == Some(n)).unwrap().id;
        assert_eq!(kind_steps(t, id("V3"), id("V1")), Some(2));
        assert_eq!(kind_steps(t, id("V1"), id("A1")), None);
        let (_, op) = shift_tracks(d.project(), &[(x, s(0.0)), (y, s(0.0))], 1).unwrap();
        d.apply(op, OpSource::Ui).unwrap();
        assert_eq!((at(&d, x), at(&d, y)), (("V2".into(), 0.0), ("V1".into(), 0.0)));
        // back up again (other order)
        let (_, op) = shift_tracks(d.project(), &[(x, s(0.0)), (y, s(0.0))], -1).unwrap();
        d.apply(op, OpSource::Ui).unwrap();
        assert_eq!((at(&d, x).0, at(&d, y).0), ("V3".into(), "V2".into()));
    }

    /// Group moved in time on the magnet main track (steps 0): the clips line up by their new starts
    /// – all selected keeps the order, a part dragged past a clip goes behind it (one by one, the
    /// insert by middles of the packed track swapped the group)
    #[test]
    fn group_on_the_magnet_main_track_keeps_its_order() {
        let mut p = proj(&["M", "A1"]);
        let ids: Vec<ClipId> = (0..3).map(|i| clip(&mut p, 0, i as f64)).collect();
        let s = |x: f64| Time::from_secs_f64(x);
        let order = |d: &Document| d.project().timeline.tracks.iter().find(|t| t.main).unwrap().clips.iter().map(|c| c.id).collect::<Vec<_>>();
        for sel in [[0, 1, 2], [2, 0, 1], [1, 2, 0]] {
            let moves: Vec<(ClipId, Time)> = sel.iter().map(|&i| (ids[i], s(i as f64 + 2.5))).collect();
            let (_, op) = shift_tracks(&p, &moves, 0).unwrap();
            let mut d = Document::new(p.clone());
            d.apply(op, OpSource::Ui).unwrap();
            assert_eq!(order(&d), ids, "{sel:?}");
        }
        // first two dragged past the third clip's start
        let (_, op) = shift_tracks(&p, &[(ids[1], s(3.5)), (ids[0], s(2.5))], 0).unwrap();
        let mut d = Document::new(p.clone());
        d.apply(op, OpSource::Ui).unwrap();
        assert_eq!(order(&d), [ids[2], ids[0], ids[1]]);
        assert_eq!(d.undo_entries().len(), 1);
    }

    #[test]
    fn clip_moves_within_its_own_track_whatever_the_kind() {
        // image clips (kind `Video`) on a sticker track, as in generated projects
        let mut p = proj(&["S", "V", "M"]);
        let png = clip(&mut p, 0, 0.0);
        // legacy: text clip on a visual track – still movable within its track
        let c = Clip::new(ClipContent::Text(TextStyle::default()), Time::from_secs_f64(3.0), Time::from_secs_f64(1.0));
        let txt = c.id;
        let mi = p.timeline.tracks.iter().position(|t| t.name.as_deref() == Some("V")).unwrap();
        p.timeline.tracks[mi].clips.push(c);
        let mut d = Document::new(p);
        let s = |x: f64| Time::from_secs_f64(x);
        d.apply(EditOp::MoveClip { clip: png, track: None, start: s(5.0) }, OpSource::Ui).unwrap();
        d.apply(EditOp::MoveClip { clip: txt, track: None, start: s(8.0) }, OpSource::Ui).unwrap();
        let start = |id| d.project().find_clip(id).map(|(t, c)| (t.name.clone().unwrap(), c.start.as_secs_f64()));
        assert_eq!(start(png), Some(("S".into(), 5.0)));
        assert_eq!(start(txt), Some(("V".into(), 8.0)));
        // track change still checks the kind: text does not belong on the sticker track
        let sticker = d.project().timeline.tracks.iter().find(|t| t.kind == Sticker).unwrap().id;
        assert!(d.apply(EditOp::MoveClip { clip: txt, track: Some(sticker), start: s(1.0) }, OpSource::Ui).is_err());
        assert!(fits(&d.project().timeline.tracks[display_order(&d.project().timeline.tracks)[0]], Video));
    }
}
