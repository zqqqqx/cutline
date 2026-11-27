//! Paste from the clipboard: target track per clip and "push following clips".
//!
//! Flow: [`paste_targets`] decides target track and position, [`paste_splits`] splits a clip that
//! lies at the paste position, then (on the split state) [`paste_ops`] push the later
//! clips of the target track right and paste. Both together are one history step.

use crate::model::*;
use crate::ops::EditOp;
use crate::time::Time;

/// Index of track `t` if it takes clips of kind `k` (matching kind, not locked).
fn fits(p: &Project, t: TrackId, k: TrackKind) -> Option<usize> {
    p.timeline.tracks.iter().position(|x| x.id == t && x.kind.accepts(k) && !x.locked)
}

/// Target track (`None` = previous automatic choice) and clip at its new position (new id) per
/// copied clip. `clips`: (source track, clip); `target`: track clicked after copying
/// (otherwise `None` = source track); `at`: playhead. The earliest clip goes to the target track, the
/// others keep their track distance to it; if a track does not fit, the source track applies.
pub fn paste_targets(
    p: &Project,
    clips: &[(TrackId, Clip)],
    target: Option<TrackId>,
    at: Time,
) -> Vec<(Option<TrackId>, Clip)> {
    let Some(first) = (0..clips.len()).min_by_key(|&i| clips[i].1.start) else {
        return Vec::new();
    };
    let t0 = clips[first].1.start;
    let idx = |t: TrackId| p.timeline.tracks.iter().position(|x| x.id == t);
    // track offset of the target track relative to the source track of the first clip
    let shift = target
        .and_then(|t| fits(p, t, clips[first].1.kind()))
        .zip(idx(clips[first].0))
        .map(|(ti, si)| ti as isize - si as isize);
    clips
        .iter()
        .enumerate()
        .map(|(i, (src, c))| {
            let kind = c.kind();
            let wanted = if i == first {
                target
            } else {
                shift.zip(idx(*src)).and_then(|(d, si)| {
                    let j = si as isize + d;
                    (j >= 0).then(|| p.timeline.tracks.get(j as usize).map(|t| t.id)).flatten()
                })
            };
            let dest = wanted
                .filter(|t| fits(p, *t, kind).is_some())
                .or_else(|| fits(p, *src, kind).map(|_| *src));
            let mut n = c.clone();
            n.id = ClipId::new();
            n.start = at + (c.start - t0);
            (dest, n)
        })
        .collect()
}

/// Per target track: (track, start, end) of the pasted clips.
fn spans(items: &[(Option<TrackId>, Clip)]) -> Vec<(TrackId, Time, Time)> {
    let mut out: Vec<(TrackId, Time, Time)> = Vec::new();
    for (t, c) in items {
        let Some(t) = *t else { continue };
        match out.iter_mut().find(|s| s.0 == t) {
            Some(s) => {
                s.1 = s.1.min(c.start);
                s.2 = s.2.max(c.end());
            }
            None => out.push((t, c.start, c.end())),
        }
    }
    out
}

/// Step 1: split clips that lie at the paste position of a target track (playhead inside the clip) there.
pub fn paste_splits(p: &Project, items: &[(Option<TrackId>, Clip)]) -> Vec<EditOp> {
    spans(items)
        .into_iter()
        .filter_map(|(t, g0, _)| {
            let tr = p.timeline.tracks.iter().find(|x| x.id == t)?;
            let c = tr.clips.iter().find(|c| c.start < g0 && c.end() > g0)?;
            Some(EditOp::SplitClip { clip: c.id, at: g0 })
        })
        .collect()
}

/// Step 2 (on the split state): if the position is occupied, all later clips of the
/// target track move right by the pasted length (this track only); then paste. The magnet main track
/// lines up on paste anyway.
pub fn paste_ops(p: &Project, items: &[(Option<TrackId>, Clip)]) -> Vec<EditOp> {
    let mut ops = Vec::new();
    for (t, g0, g1) in spans(items) {
        let Some(tr) = p.timeline.tracks.iter().find(|x| x.id == t) else { continue };
        if tr.main && p.settings.main_track_magnet {
            continue;
        }
        if tr.is_free(g0, g1 - g0, None) {
            continue;
        }
        let mut later: Vec<&Clip> = tr.clips.iter().filter(|c| c.start >= g0).collect();
        // from the back: each clip moves into space already freed
        later.sort_by_key(|c| std::cmp::Reverse(c.start));
        ops.extend(later.into_iter().map(|c| EditOp::MoveClip {
            clip: c.id,
            track: Some(t),
            start: c.start + (g1 - g0),
        }));
    }
    ops.extend(items.iter().map(|(t, c)| EditOp::AddClip { track: *t, clip: c.clone() }));
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, OpOutput, OpSource};

    fn s(v: f64) -> Time {
        Time::from_secs_f64(v)
    }
    fn text(at: f64, len: f64) -> Clip {
        Clip::new(ClipContent::Text(TextStyle::default()), s(at), s(len))
    }
    fn track(d: &mut Document, kind: TrackKind) -> TrackId {
        let n = d.project().timeline.tracks.len();
        let Ok(OpOutput::Track(t)) = d.apply(EditOp::InsertTrack { index: n, kind }, OpSource::Ui) else { panic!() };
        t
    }
    fn add(d: &mut Document, t: TrackId, c: Clip) -> ClipId {
        let Ok(OpOutput::Clip(id)) = d.apply(EditOp::AddClip { track: Some(t), clip: c }, OpSource::Ui) else { panic!() };
        id
    }
    /// Like the app: split, then push + paste, as one step.
    fn paste(d: &mut Document, clips: &[(TrackId, Clip)], target: Option<TrackId>, at: f64) {
        let rev = d.revision();
        let items = paste_targets(d.project(), clips, target, s(at));
        let split = paste_splits(d.project(), &items);
        d.apply(EditOp::Batch { label: String::new(), ops: split }, OpSource::Ui).unwrap();
        let ops = paste_ops(d.project(), &items);
        d.apply(EditOp::Batch { label: String::new(), ops }, OpSource::Ui).unwrap();
        d.squash_since(rev);
    }
    fn starts(d: &Document, t: TrackId) -> Vec<f64> {
        let tr = d.project().timeline.tracks.iter().find(|x| x.id == t).unwrap();
        tr.clips.iter().map(|c| (c.start.as_secs_f64() * 10.0).round() / 10.0).collect()
    }

    #[test]
    fn pastes_into_the_source_track_at_the_playhead() {
        let mut d = Document::new(Project::new("T"));
        let (a, b) = (track(&mut d, TrackKind::Text), track(&mut d, TrackKind::Text));
        let c = text(0.0, 1.0);
        add(&mut d, b, c.clone());
        // track a is free and would come first automatically: source track b anyway
        paste(&mut d, &[(b, c.clone())], None, 5.0);
        assert_eq!(starts(&d, b), vec![0.0, 5.0]);
        assert!(starts(&d, a).is_empty());
        // clicked track a after copying: goes there
        paste(&mut d, &[(b, c)], Some(a), 2.0);
        assert_eq!(starts(&d, a), vec![2.0]);
    }

    #[test]
    fn occupied_spot_pushes_later_clips_of_that_track_only() {
        let mut d = Document::new(Project::new("T"));
        let (a, b) = (track(&mut d, TrackKind::Text), track(&mut d, TrackKind::Text));
        let c = text(0.0, 2.0);
        add(&mut d, a, c.clone());
        add(&mut d, a, text(3.0, 1.0));
        add(&mut d, b, text(1.0, 4.0));
        let before = d.project().clone();
        // playhead at the start of a clip: it and all later ones move 2 s
        paste(&mut d, &[(a, c.clone())], None, 0.0);
        assert_eq!(starts(&d, a), vec![0.0, 2.0, 5.0]);
        assert_eq!(starts(&d, b), vec![1.0], "other tracks stay");
        // one history step: undo restores everything
        d.undo().unwrap();
        assert_eq!(d.project(), &before);
        // playhead inside the clip: split there, the right part moves along
        paste(&mut d, &[(a, c)], None, 1.0);
        assert_eq!(starts(&d, a), vec![0.0, 1.0, 3.0, 5.0]);
        let total: f64 = d.project().timeline.tracks.iter().find(|t| t.id == a).unwrap().clips.iter().map(|c| c.duration.as_secs_f64()).sum();
        assert!((total - 5.0).abs() < 1e-6, "nothing gets lost");
        d.undo().unwrap();
        assert_eq!(d.project(), &before);
    }

    #[test]
    fn unsuitable_or_locked_target_falls_back_to_the_source_track() {
        let mut d = Document::new(Project::new("T"));
        let (txt, fx) = (track(&mut d, TrackKind::Text), track(&mut d, TrackKind::Effect));
        let other = track(&mut d, TrackKind::Text);
        let c = text(0.0, 1.0);
        add(&mut d, txt, c.clone());
        let it = paste_targets(d.project(), &[(txt, c.clone())], Some(fx), s(4.0));
        assert_eq!(it[0].0, Some(txt), "text does not belong on an effect track");
        d.apply(EditOp::SetTrackFlags { track: other, locked: true, hidden: false, muted: false }, OpSource::Ui).unwrap();
        let it = paste_targets(d.project(), &[(txt, c.clone())], Some(other), s(4.0));
        assert_eq!(it[0].0, Some(txt), "locked track");
        // source track gone: previous automatic choice
        let it = paste_targets(d.project(), &[(TrackId::new(), c)], None, s(4.0));
        assert_eq!(it[0].0, None);
        assert_eq!(it[0].1.start, s(4.0));
    }

    #[test]
    fn clips_from_two_tracks_keep_their_distances() {
        let mut d = Document::new(Project::new("T"));
        let (a, b, c) = (track(&mut d, TrackKind::Text), track(&mut d, TrackKind::Text), track(&mut d, TrackKind::Text));
        let (x, y) = (text(1.0, 1.0), text(3.0, 1.0));
        add(&mut d, a, x.clone());
        add(&mut d, b, y.clone());
        let clips = [(b, y), (a, x)];
        // target b for the earliest (from a): the one from b lands one track further in c, 2 s later
        let it = paste_targets(d.project(), &clips, Some(b), s(10.0));
        assert_eq!((it[0].0, it[0].1.start), (Some(c), s(12.0)));
        assert_eq!((it[1].0, it[1].1.start), (Some(b), s(10.0)));
        // no track above c: source track
        let it = paste_targets(d.project(), &clips, Some(c), s(10.0));
        assert_eq!((it[0].0, it[1].0), (Some(b), Some(c)));
        assert!(it.iter().all(|(_, n)| n.id != clips[0].1.id && n.id != clips[1].1.id), "neue Kennungen");
    }
}
