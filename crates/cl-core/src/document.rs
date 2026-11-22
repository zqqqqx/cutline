use crate::model::Project;
use crate::ops::{EditOp, OpOutput};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// Where a change came from. Shown in the history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "name", rename_all = "snake_case")]
pub enum OpSource {
    Ui,
    Mcp(String),
    Plugin(String),
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub label: String,
    pub source: OpSource,
    /// Time of the change
    pub at: std::time::SystemTime,
    snapshot: Project,
}

/// Project + undo/redo history.
///
/// Stores full snapshots; large projects get fewer steps for it (see `apply`).
#[derive(Debug, Clone)]
pub struct Document {
    project: Project,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    revision: u64,
    pub max_history: usize,
}

impl Document {
    pub fn new(project: Project) -> Self {
        Self {
            project,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            max_history: 100,
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Incremented on every change. Useful to invalidate caches/views.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn apply(&mut self, op: EditOp, source: OpSource) -> Result<OpOutput> {
        // NaN/infinity would be saved as `null`, the project file would no longer load
        let json = serde_json::to_value(&op).map_err(|_| CoreError::InvalidValue)?;
        if serde_json::from_value::<EditOp>(json).is_err() {
            return Err(CoreError::InvalidValue);
        }
        let label = op.label();
        let before = self.project.clone();
        match op.apply(&mut self.project) {
            // Nothing changed (e.g. a field writes the same value again): no history step
            Ok(out) if self.project == before => Ok(out),
            Ok(out) => {
                self.undo.push(HistoryEntry {
                    label,
                    source,
                    at: std::time::SystemTime::now(),
                    snapshot: before,
                });
                // A step is a full copy (measured ≈1.1 KB per clip): at most ≈100,000
                // clip copies (≈110 MB), at least 10 steps; up to 1000 clips it stays at 100
                let clips: usize = self.project.timeline.tracks.iter().map(|t| t.clips.len()).sum();
                let cap = (100_000 / clips.max(1)).max(10).min(self.max_history);
                if self.undo.len() > cap {
                    self.undo.drain(..self.undo.len() - cap);
                }
                self.redo.clear();
                self.revision += 1;
                Ok(out)
            }
            Err(e) => {
                self.project = before; // atomic: unchanged on error
                Err(e)
            }
        }
    }

    pub fn undo(&mut self) -> Result<String> {
        let entry = self.undo.pop().ok_or(CoreError::NothingToUndo)?;
        let label = entry.label;
        let current = std::mem::replace(&mut self.project, entry.snapshot);
        self.redo.push(HistoryEntry {
            label: label.clone(),
            source: entry.source,
            at: entry.at,
            snapshot: current,
        });
        self.revision += 1;
        Ok(label)
    }

    pub fn redo(&mut self) -> Result<String> {
        let entry = self.redo.pop().ok_or(CoreError::NothingToRedo)?;
        let label = entry.label;
        let current = std::mem::replace(&mut self.project, entry.snapshot);
        self.undo.push(HistoryEntry {
            label: label.clone(),
            source: entry.source,
            at: entry.at,
            snapshot: current,
        });
        self.revision += 1;
        Ok(label)
    }

    /// Several steps at once: `steps` < 0 undo, > 0 redo. Returns the number
    /// of steps actually performed.
    pub fn step(&mut self, steps: isize) -> usize {
        let mut n = 0;
        for _ in 0..steps.unsigned_abs() {
            let ok = if steps < 0 { self.undo().is_ok() } else { self.redo().is_ok() };
            if !ok {
                break;
            }
            n += 1;
        }
        n
    }

    /// Merge all steps since revision `rev` (noted before with [`Self::revision`]) into ONE,
    /// e.g. new track + clip in it: undo then takes back both.
    /// Label and source of the last step stay. No undo/redo in between.
    pub fn squash_since(&mut self, rev: u64) {
        let n = (self.revision.saturating_sub(rev) as usize).min(self.undo.len());
        if n < 2 {
            return;
        }
        let mut tail = self.undo.split_off(self.undo.len() - n);
        let mut last = tail.pop().expect("n >= 2");
        last.snapshot = tail.swap_remove(0).snapshot;
        self.undo.push(last);
    }

    /// `f` as ONE history step; if `f` fails, the project stays as before and without
    /// a history entry (an already cleared redo stack stays empty).
    pub fn transaction<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let rev = self.revision;
        let before = self.project.clone();
        let r = f(self);
        match r {
            Ok(_) => self.squash_since(rev),
            Err(_) => {
                let n = (self.revision.saturating_sub(rev) as usize).min(self.undo.len());
                self.undo.truncate(self.undo.len() - n);
                if self.project != before {
                    self.project = before;
                    self.revision += 1;
                }
            }
        }
        r
    }

    /// Undoable steps (oldest first)
    pub fn undo_entries(&self) -> &[HistoryEntry] {
        &self.undo
    }

    /// Redoable steps (the next one last)
    pub fn redo_entries(&self) -> &[HistoryEntry] {
        &self.redo
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// History (oldest first) as (label, source).
    pub fn history(&self) -> impl Iterator<Item = (&str, &OpSource)> {
        self.undo.iter().map(|e| (e.label.as_str(), &e.source))
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
    use std::path::PathBuf;

    fn media(dur_s: f64) -> MediaRef {
        MediaRef {
            id: MediaId::new(),
            path: PathBuf::from("clip.mp4"),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(dur_s),
            width: 1920,
            height: 1080,
            fps: Some(Rational::new(30, 1)),
            has_audio: true,
            audio_streams: 1,
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            has_alpha: false,
        }
    }

    fn video_clip(m: &MediaRef, start: f64, dur: f64) -> Clip {
        Clip::new(
            ClipContent::Media {
                media: m.id,
                kind: MediaKind::Video,
            },
            Time::from_secs_f64(start),
            Time::from_secs_f64(dur),
        )
    }

    fn doc_with_clip() -> (Document, ClipId) {
        let mut d = Document::new(Project::new("Test"));
        let m = media(10.0);
        d.apply(EditOp::ImportMedia { media: m.clone() }, OpSource::Ui)
            .unwrap();
        let out = d
            .apply(
                EditOp::AddClip {
                    track: None,
                    clip: video_clip(&m, 0.0, 10.0),
                },
                OpSource::Ui,
            )
            .unwrap();
        let OpOutput::Clip(id) = out else { panic!() };
        (d, id)
    }

    #[test]
    fn timing_duration_stops_at_source_end() {
        let (mut d, id) = doc_with_clip(); // source 10 s
        let s = Time::from_secs_f64;
        let set = |d: &mut Document, dur: f64, speed_src: f64| {
            d.apply(
                EditOp::SetClipTiming { clip: id, start: Time::ZERO, duration: s(dur), source_in: s(speed_src) },
                OpSource::Ui,
            )
            .unwrap();
            d.project().find_clip(id).unwrap().1.clone()
        };
        let c = set(&mut d, 99999.0, 4.0);
        assert_eq!((c.duration, c.source_in), (s(6.0), s(4.0)));
        // speed 2×: only half as long on the timeline
        d.apply(EditOp::SetSpeed { clip: c.id, speed: 2.0 }, OpSource::Ui).unwrap();
        assert_eq!(set(&mut d, 99999.0, 4.0).duration, s(3.0));
        // paste (MCP, plugins) likewise
        let m = d.project().media[0].clone();
        let OpOutput::Clip(n) = d.apply(EditOp::AddClip { track: None, clip: video_clip(&m, 50.0, 99999.0) }, OpSource::Ui).unwrap() else {
            panic!()
        };
        assert_eq!(d.project().find_clip(n).unwrap().1.duration, s(10.0));
    }

    #[test]
    fn unchanged_value_adds_no_history_step() {
        let (mut d, id) = doc_with_clip();
        let n = d.undo_entries().len();
        let c = d.project().find_clip(id).unwrap().1.clone();
        for _ in 0..5 {
            let op = EditOp::SetClipTiming { clip: id, start: c.start, duration: c.duration, source_in: c.source_in };
            d.apply(op, OpSource::Ui).unwrap();
        }
        assert_eq!(d.undo_entries().len(), n);
    }

    #[test]
    fn add_clip_without_track_respects_locked_main() {
        let (mut d, _) = doc_with_clip();
        let m = d.project().media[0].clone();
        let main = d.project().timeline.tracks[0].id;
        d.apply(EditOp::SetTrackFlags { track: main, locked: true, hidden: false, muted: false }, OpSource::Ui)
            .unwrap();
        let r = d.apply(EditOp::AddClip { track: None, clip: video_clip(&m, 99.0, 5.0) }, OpSource::Ui);
        assert!(matches!(r, Err(CoreError::TrackLocked)), "{r:?}");
        assert_eq!(d.project().timeline.tracks[0].clips.len(), 1);
    }

    #[test]
    fn video_goes_to_main_track_and_magnet_appends() {
        let (mut d, _) = doc_with_clip();
        let m = d.project().media[0].clone();
        // Start time 99 s is pulled to 10 s by the magnet.
        d.apply(
            EditOp::AddClip {
                track: None,
                clip: video_clip(&m, 99.0, 5.0),
            },
            OpSource::Ui,
        )
        .unwrap();
        let main = &d.project().timeline.tracks[0];
        assert!(main.main);
        assert_eq!(main.clips.len(), 2);
        assert_eq!(main.clips[1].start, Time::from_secs_f64(10.0));
        assert_eq!(d.project().duration(), Time::from_secs_f64(15.0));
    }

    #[test]
    fn magnet_insert_position_follows_drop_point() {
        let (mut d, first) = doc_with_clip(); // 0–10 s
        let m = d.project().media[0].clone();
        // drop at 0 s → before the existing clip
        let OpOutput::Clip(a) = d
            .apply(
                EditOp::AddClip {
                    track: None,
                    clip: video_clip(&m, 0.0, 2.0),
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        // drop at 9 s: the 10 s clip now lies at 2–12, its middle 7 < 9 → after it
        let OpOutput::Clip(b) = d
            .apply(
                EditOp::AddClip {
                    track: None,
                    clip: video_clip(&m, 9.0, 1.0),
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        let order: Vec<ClipId> = d.project().timeline.tracks[0]
            .clips
            .iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(order, vec![a, first, b]);
        assert_eq!(
            d.project().timeline.tracks[0].clips[2].start,
            Time::from_secs_f64(12.0)
        );
        // move to the start
        d.apply(
            EditOp::MoveClip {
                clip: b,
                track: None,
                start: Time::ZERO,
            },
            OpSource::Ui,
        )
        .unwrap();
        let order: Vec<ClipId> = d.project().timeline.tracks[0]
            .clips
            .iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(order, vec![b, a, first]);
    }

    #[test]
    fn split_and_source_offsets() {
        let (mut d, id) = doc_with_clip();
        let OpOutput::Clip(right) = d
            .apply(
                EditOp::SplitClip {
                    clip: id,
                    at: Time::from_secs_f64(4.0),
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        let (_, l) = d.project().find_clip(id).unwrap();
        let (_, r) = d.project().find_clip(right).unwrap();
        assert_eq!(l.duration, Time::from_secs_f64(4.0));
        assert_eq!(r.start, Time::from_secs_f64(4.0));
        assert_eq!(r.source_in, Time::from_secs_f64(4.0));
        assert_eq!((l.speed, r.speed), (1.0, 1.0), "without curve the speed stays");
        assert_eq!(
            d.apply(
                EditOp::SplitClip {
                    clip: id,
                    at: Time::from_secs_f64(9.0)
                },
                OpSource::Ui
            ),
            Err(CoreError::InvalidSplit)
        );
    }

    #[test]
    fn split_with_speed_curve_keeps_source_time_at_cut() {
        for reverse in [false, true] {
            let (mut d, id) = doc_with_clip(); // 0–10 s, source 10 s
            let mut c = d.project().find_clip(id).unwrap().1.clone();
            c.fx.speed_curve = Some("montage".into());
            c.fx.reverse = reverse;
            d.apply(EditOp::ReplaceClip { clip: c.clone() }, OpSource::Ui).unwrap();
            let at = Time::from_secs_f64(4.0);
            let want = c.source_time(at);
            let OpOutput::Clip(right) = d.apply(EditOp::SplitClip { clip: id, at }, OpSource::Ui).unwrap() else {
                panic!()
            };
            let p = d.project();
            let (l, r) = (p.find_clip(id).unwrap().1, p.find_clip(right).unwrap().1);
            assert!(l.fx.speed_curve.is_none() && r.fx.speed_curve.is_none());
            let near = |a: Time, b: Time| assert!((a.0 - b.0).abs() <= 2, "{a:?} vs {b:?} (reversed {reverse})");
            near(l.source_time(l.end()), want);
            near(r.source_time(r.start), want);
            // outer edges stay where they were
            near(l.source_time(l.start), c.source_time(c.start));
            near(r.source_time(r.end()), c.source_time(c.end()));
        }
    }

    #[test]
    fn reversed_clip_keeps_frames_on_split_and_trim() {
        let (mut d, id) = doc_with_clip(); // 0–10 s, source 10 s
        let before = |t: f64| Time::from_secs_f64(9.0 - t); // source time at timeline position t
        let src = |d: &Document, t: f64| {
            let p = d.project();
            let at = Time::from_secs_f64(t);
            let c = p.timeline.tracks[0].clips.iter().find(|c| c.contains(at)).unwrap();
            c.source_time(at)
        };
        let timing = EditOp::SetClipTiming {
            clip: id,
            start: Time::ZERO,
            duration: Time::from_secs_f64(8.0),
            source_in: Time::from_secs_f64(1.0), // source 1–9 s, reversed
        };
        d.apply(timing, OpSource::Ui).unwrap();
        let mut c = d.project().find_clip(id).unwrap().1.clone();
        c.fx.reverse = true;
        d.apply(EditOp::ReplaceClip { clip: c }, OpSource::Ui).unwrap();
        assert_eq!(src(&d, 0.0), before(0.0));
        let OpOutput::Clip(right) = d
            .apply(EditOp::SplitClip { clip: id, at: Time::from_secs_f64(3.0) }, OpSource::Ui)
            .unwrap()
        else {
            panic!()
        };
        for t in [0.0, 2.0, 3.0, 5.0, 7.5] {
            assert_eq!(src(&d, t), before(t), "after split at {t} s");
        }
        // shorten end of the right part by 1 s, start of the left by 1 s
        d.apply(EditOp::TrimClip { clip: right, edge: Edge::End, to: Time::from_secs_f64(7.0) }, OpSource::Ui).unwrap();
        let (_, r) = d.project().find_clip(right).unwrap();
        assert_eq!(r.source_time(Time::from_secs_f64(3.0)), before(3.0));
        assert_eq!(r.source_in, Time::from_secs_f64(2.0));
        d.apply(EditOp::TrimClip { clip: id, edge: Edge::Start, to: Time::from_secs_f64(1.0) }, OpSource::Ui).unwrap();
        let (_, l) = d.project().find_clip(id).unwrap();
        assert_eq!(l.source_time(l.start), before(1.0));
        // extend the end: at most to source time 0, the start stays the same
        d.apply(EditOp::TrimClip { clip: right, edge: Edge::End, to: Time::from_secs_f64(30.0) }, OpSource::Ui).unwrap();
        let (_, r) = d.project().find_clip(right).unwrap();
        assert_eq!(r.source_in, Time::ZERO);
        assert_eq!(r.source_time(r.start), before(3.0));
    }

    #[test]
    fn move_track_stays_in_its_group() {
        let mut d = Document::new(Project::new("T"));
        let ins = |d: &mut Document, kind| {
            let n = d.project().timeline.tracks.len();
            match d.apply(EditOp::InsertTrack { index: n, kind }, OpSource::Ui).unwrap() {
                OpOutput::Track(t) => t,
                _ => panic!(),
            }
        };
        let v1 = ins(&mut d, TrackKind::Video);
        let t1 = ins(&mut d, TrackKind::Text);
        let a1 = ins(&mut d, TrackKind::Audio);
        let a2 = ins(&mut d, TrackKind::Audio);
        let ids = |d: &Document| d.project().timeline.tracks.iter().map(|t| t.id).collect::<Vec<_>>();
        // text is on top; down → below the video track
        d.apply(EditOp::MoveTrack { track: t1, up: false }, OpSource::Ui).unwrap();
        assert_eq!(ids(&d)[..2], [t1, v1]);
        // at the very bottom: stays
        d.apply(EditOp::MoveTrack { track: t1, up: false }, OpSource::Ui).unwrap();
        assert_eq!(ids(&d)[..2], [t1, v1]);
        // audio track a2 (bottom) up → before a1, but not between the visual tracks
        d.apply(EditOp::MoveTrack { track: a2, up: true }, OpSource::Ui).unwrap();
        assert_eq!(ids(&d)[2..], [a2, a1]);
        d.apply(EditOp::MoveTrack { track: a2, up: true }, OpSource::Ui).unwrap();
        assert_eq!(ids(&d)[2..], [a2, a1]);
    }

    #[test]
    fn step_moves_through_history_both_ways() {
        let (mut d, id) = doc_with_clip();
        for at in [2.0, 4.0, 6.0] {
            d.apply(EditOp::SplitClip { clip: id, at: Time::from_secs_f64(at) }, OpSource::Ui).ok();
        }
        let clips = |d: &Document| d.project().timeline.tracks[0].clips.len();
        assert_eq!(clips(&d), 2); // only the first cut hits the (ever shorter) left part
        let total = d.undo_entries().len();
        assert_eq!(d.step(-(total as isize)), total);
        assert_eq!(d.undo_entries().len(), 0);
        assert_eq!(d.redo_entries().len(), total);
        assert_eq!(d.step(-1), 0);
        assert_eq!(d.step(2), 2.min(total));
    }

    #[test]
    fn trim_start_ripples_main_track() {
        let (mut d, id) = doc_with_clip();
        let OpOutput::Clip(right) = d
            .apply(
                EditOp::SplitClip {
                    clip: id,
                    at: Time::from_secs_f64(4.0),
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        // shorten the start of the first clip by 1 s → everything moves left.
        d.apply(
            EditOp::TrimClip {
                clip: id,
                edge: Edge::Start,
                to: Time::from_secs_f64(1.0),
            },
            OpSource::Ui,
        )
        .unwrap();
        let (_, l) = d.project().find_clip(id).unwrap();
        let (_, r) = d.project().find_clip(right).unwrap();
        assert_eq!(l.start, Time::ZERO);
        assert_eq!(l.source_in, Time::from_secs_f64(1.0));
        assert_eq!(r.start, Time::from_secs_f64(3.0));
        // video past the source end: silently clamped to the end (no error).
        d.apply(
            EditOp::TrimClip {
                clip: right,
                edge: Edge::End,
                to: Time::from_secs_f64(30.0),
            },
            OpSource::Ui,
        )
        .unwrap();
        let (_, r) = d.project().find_clip(right).unwrap();
        assert_eq!(r.source_in + r.duration, Time::from_secs_f64(10.0));
    }

    #[test]
    fn audio_can_be_extended_with_silence() {
        let mut d = Document::new(Project::new("T"));
        let mut m = media(2.0);
        m.kind = MediaKind::Audio;
        d.apply(EditOp::ImportMedia { media: m.clone() }, OpSource::Ui)
            .unwrap();
        let c = Clip::new(
            ClipContent::Media {
                media: m.id,
                kind: MediaKind::Audio,
            },
            Time::ZERO,
            Time::from_secs_f64(2.0),
        );
        let OpOutput::Clip(id) = d
            .apply(
                EditOp::AddClip {
                    track: None,
                    clip: c,
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        d.apply(
            EditOp::TrimClip {
                clip: id,
                edge: Edge::End,
                to: Time::from_secs_f64(5.0),
            },
            OpSource::Ui,
        )
        .unwrap();
        assert_eq!(
            d.project().find_clip(id).unwrap().1.duration,
            Time::from_secs_f64(5.0)
        );
    }

    #[test]
    fn speed_changes_duration_and_source_mapping() {
        let (mut d, id) = doc_with_clip(); // 10 s
        d.apply(
            EditOp::SetSpeed {
                clip: id,
                speed: 2.0,
            },
            OpSource::Ui,
        )
        .unwrap();
        let (_, c) = d.project().find_clip(id).unwrap();
        assert_eq!(c.duration, Time::from_secs_f64(5.0));
        assert_eq!(
            c.source_time(Time::from_secs_f64(1.0)),
            Time::from_secs_f64(2.0)
        );
        // split at 2 s → right part starts at 4 s in the source
        let OpOutput::Clip(r) = d
            .apply(
                EditOp::SplitClip {
                    clip: id,
                    at: Time::from_secs_f64(2.0),
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            d.project().find_clip(r).unwrap().1.source_in,
            Time::from_secs_f64(4.0)
        );
    }

    #[test]
    fn tracks_insert_remove_and_prune() {
        let (mut d, _) = doc_with_clip();
        let t = Clip::new(
            ClipContent::Text(TextStyle::default()),
            Time::ZERO,
            Time::from_secs_f64(2.0),
        );
        let OpOutput::Clip(tc) = d
            .apply(
                EditOp::AddClip {
                    track: None,
                    clip: t,
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(d.project().timeline.tracks.len(), 2);
        // drag into empty space → new track
        d.apply(
            EditOp::MoveClipToNewTrack {
                clip: tc,
                index: 2,
                start: Time::from_secs_f64(1.0),
            },
            OpSource::Ui,
        )
        .unwrap();
        // old, now empty text track was removed
        assert_eq!(d.project().timeline.tracks.len(), 2);
        assert_eq!(
            d.project().find_clip(tc).unwrap().1.start,
            Time::from_secs_f64(1.0)
        );
        // insert a track and delete it again
        let OpOutput::Track(nt) = d
            .apply(
                EditOp::InsertTrack {
                    index: 0,
                    kind: TrackKind::Text,
                },
                OpSource::Ui,
            )
            .unwrap()
        else {
            panic!()
        };
        assert!(
            d.project().timeline.tracks[0].main,
            "nothing below the main track"
        );
        d.apply(EditOp::RemoveTrack { track: nt }, OpSource::Ui)
            .unwrap();
        assert_eq!(d.project().timeline.tracks.len(), 2);
        // deleting a clip removes its empty track
        d.apply(EditOp::RemoveClip { clip: tc }, OpSource::Ui)
            .unwrap();
        assert_eq!(d.project().timeline.tracks.len(), 1);
    }

    #[test]
    fn batch_is_one_undo_step_and_gaps_close() {
        let mut d = Document::new(Project::new("T"));
        let t = |s| {
            Clip::new(
                ClipContent::Text(TextStyle::default()),
                Time::from_secs_f64(s),
                Time::from_secs_f64(1.0),
            )
        };
        let (a, b) = (t(2.0), t(5.0));
        let (ia, ib) = (a.id, b.id);
        d.apply(
            EditOp::AddClip {
                track: None,
                clip: a,
            },
            OpSource::Ui,
        )
        .unwrap();
        let tr = d.project().timeline.tracks[0].id;
        d.apply(
            EditOp::AddClip {
                track: Some(tr),
                clip: b,
            },
            OpSource::Ui,
        )
        .unwrap();
        d.apply(EditOp::CloseGaps { track: Some(tr) }, OpSource::Ui)
            .unwrap();
        assert_eq!(
            d.project().find_clip(ib).unwrap().1.start,
            Time::from_secs_f64(1.0)
        );
        let n = d.history().count();
        d.apply(
            EditOp::Batch {
                label: String::new(),
                ops: vec![
                    EditOp::RemoveClip { clip: ia },
                    EditOp::RemoveClip { clip: ib },
                ],
            },
            OpSource::Ui,
        )
        .unwrap();
        assert_eq!(d.history().count(), n + 1);
        assert!(d.project().timeline.tracks.is_empty());
        d.undo().unwrap();
        assert_eq!(d.project().timeline.tracks[0].clips.len(), 2);
    }

    #[test]
    fn new_track_and_its_clip_are_undone_together() {
        let mut d = Document::new(Project::new("T"));
        let start = d.revision();
        let OpOutput::Track(t) = d
            .apply(EditOp::InsertTrack { index: 0, kind: TrackKind::Effect }, OpSource::Ui)
            .unwrap()
        else {
            panic!()
        };
        let c = Clip::new(ClipContent::Effect(FxRef::new("glow")), Time::ZERO, Time::from_secs_f64(3.0));
        d.apply(EditOp::AddClip { track: Some(t), clip: c }, OpSource::Ui).unwrap();
        d.squash_since(start);
        assert_eq!(d.undo_entries().len(), 1);
        assert_eq!(d.undo_entries()[0].label, "Clip hinzufügen");
        // undo: no empty track is left behind; redo brings back both
        d.undo().unwrap();
        assert!(d.project().timeline.tracks.is_empty());
        d.redo().unwrap();
        assert_eq!(d.project().timeline.tracks[0].clips.len(), 1);
        // only one step since `rev`: nothing to merge
        let rev = d.revision();
        d.apply(EditOp::InsertTrack { index: 0, kind: TrackKind::Text }, OpSource::Ui).unwrap();
        d.squash_since(rev);
        assert_eq!(d.undo_entries().len(), 2);
    }

    #[test]
    fn batch_label_uses_given_name() {
        let ops = || vec![EditOp::RemoveClip { clip: ClipId::new() }];
        let named = EditOp::Batch { label: "Pausen entfernen".into(), ops: ops() };
        assert_eq!(named.label(), "Pausen entfernen");
        // without name: guessed from the first step as before
        let plain = EditOp::Batch { label: String::new(), ops: ops() };
        assert_eq!(plain.label(), "Clips löschen");
        assert_eq!(EditOp::Batch { label: String::new(), ops: vec![] }.label(), "Mehrere Änderungen");
    }


    #[test]
    fn eval_combines_keyframes_and_animation() {
        let m = media(10.0);
        let mut c = video_clip(&m, 0.0, 4.0);
        c.keyframes.set(Prop::Opacity, Time::ZERO, 0.0);
        c.keyframes
            .set(Prop::Opacity, Time::from_secs_f64(2.0), 1.0);
        assert!((c.eval_at(Time::from_secs_f64(1.0), 1920.0, 1080.0).opacity - 0.5).abs() < 1e-3);
        c.keyframes = Keyframes::default();
        c.anim.intro = Some(AnimRef {
            id: "slide-left".into(),
            duration: Time::from_secs_f64(1.0),
        });
        let e0 = c.eval_at(Time::ZERO, 1920.0, 1080.0);
        assert!((e0.x + 1920.0).abs() < 1.0);
        assert_eq!(c.eval_at(Time::from_secs_f64(2.0), 1920.0, 1080.0).x, 0.0);
        // old projects without new fields still load
        let mut v: serde_json::Value = serde_json::to_value(&c).unwrap();
        for k in [
            "speed",
            "disabled",
            "fx",
            "keyframes",
            "anim",
            "transition_out",
            "fade_in",
            "fade_out",
            "audio",
            "beats",
        ] {
            v.as_object_mut().unwrap().remove(k);
        }
        let back: Clip = serde_json::from_value(v).unwrap();
        assert_eq!(back.speed, 1.0);
    }

    #[test]
    fn undo_redo_roundtrip_and_sources() {
        let (mut d, id) = doc_with_clip();
        let before = d.project().clone();
        d.apply(
            EditOp::RemoveClip { clip: id },
            OpSource::Mcp("test-client".into()),
        )
        .unwrap();
        assert!(d.project().find_clip(id).is_none());
        assert_eq!(
            d.history().last().unwrap().1,
            &OpSource::Mcp("test-client".into())
        );
        assert_eq!(d.undo().unwrap(), "Clip löschen");
        assert_eq!(d.project(), &before);
        d.redo().unwrap();
        assert!(d.project().find_clip(id).is_none());
    }

    #[test]
    fn text_goes_to_own_track_and_overlap_creates_new_track() {
        let (mut d, _) = doc_with_clip();
        let t = |s| {
            Clip::new(
                ClipContent::Text(TextStyle::default()),
                Time::from_secs_f64(s),
                Time::from_secs_f64(3.0),
            )
        };
        d.apply(
            EditOp::AddClip {
                track: None,
                clip: t(1.0),
            },
            OpSource::Ui,
        )
        .unwrap();
        d.apply(
            EditOp::AddClip {
                track: None,
                clip: t(2.0),
            },
            OpSource::Ui,
        )
        .unwrap();
        let text_tracks = d
            .project()
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Text)
            .count();
        assert_eq!(text_tracks, 2);
    }

    #[test]
    fn failed_op_leaves_project_untouched_and_json_roundtrip() {
        let (mut d, _) = doc_with_clip();
        let snap = d.project().clone();
        assert!(d
            .apply(
                EditOp::RemoveClip {
                    clip: ClipId::new()
                },
                OpSource::Ui
            )
            .is_err());
        assert_eq!(d.project(), &snap);
        let json = d.project().to_json();
        assert_eq!(Project::from_json(&json).unwrap(), snap);
        let op: EditOp = serde_json::from_str(r#"{"op":"rename","name":"Neu"}"#).unwrap();
        d.apply(op, OpSource::Ui).unwrap();
        assert_eq!(d.project().name, "Neu");
    }

    #[test]
    fn aspect_keeps_short_edge() {
        let mut s = ProjectSettings::default();
        s.apply_aspect(AspectPreset::Ratio(9, 16));
        assert_eq!((s.width, s.height), (1080, 1920));
        s.apply_aspect(AspectPreset::Ratio(1, 1));
        assert_eq!((s.width, s.height), (1080, 1080));
    }
    #[test]
    fn split_keeps_fades_only_at_outer_edges() {
        let (mut d, id) = doc_with_clip();
        let fades = EditOp::SetFades { clip: id, fade_in: Time::from_secs_f64(2.0), fade_out: Time::from_secs_f64(2.0) };
        d.apply(fades, OpSource::Ui).unwrap();
        let OpOutput::Clip(right) = d
            .apply(EditOp::SplitClip { clip: id, at: Time::from_secs_f64(1.0) }, OpSource::Ui)
            .unwrap()
        else {
            panic!()
        };
        let (_, l) = d.project().find_clip(id).unwrap();
        assert_eq!((l.fade_in, l.fade_out), (Time::from_secs_f64(2.0), Time::ZERO));
        let (_, r) = d.project().find_clip(right).unwrap();
        assert_eq!((r.fade_in, r.fade_out), (Time::ZERO, Time::from_secs_f64(2.0)));
    }

    #[test]
    fn replace_clip_rejects_unknown_media() {
        let (mut d, id) = doc_with_clip();
        let mut c = d.project().find_clip(id).unwrap().1.clone();
        c.content = ClipContent::Media { media: MediaId::new(), kind: MediaKind::Video };
        assert!(matches!(
            d.apply(EditOp::ReplaceClip { clip: c }, OpSource::Ui),
            Err(CoreError::MediaNotFound(_))
        ));
    }

    #[test]
    fn start_trim_moves_source_with_unknown_duration() {
        // probe without duration stores 0: source must move along anyway
        let mut d = Document::new(Project::new("Test"));
        let m = media(0.0);
        d.apply(EditOp::ImportMedia { media: m.clone() }, OpSource::Ui).unwrap();
        let OpOutput::Clip(id) = d
            .apply(EditOp::AddClip { track: None, clip: video_clip(&m, 0.0, 5.0) }, OpSource::Ui)
            .unwrap()
        else {
            panic!()
        };
        d.apply(EditOp::TrimClip { clip: id, edge: Edge::Start, to: Time::from_secs_f64(2.0) }, OpSource::Ui)
            .unwrap();
        let (_, c) = d.project().find_clip(id).unwrap();
        assert_eq!(c.source_time(c.start), Time::from_secs_f64(2.0));
    }

    #[test]
    fn ripple_range_all_tracks_markers_lock_and_undo() {
        let s = Time::from_secs_f64;
        let (mut d, vid) = doc_with_clip(); // main track 0–10 s
        let text = |a: f64, b: f64| {
            Clip::new(ClipContent::Text(TextStyle::default()), s(a), s(b - a))
        };
        let (a, b) = (text(3.0, 6.0), text(7.0, 9.0));
        let (ia, ib) = (a.id, b.id);
        for c in [a, b] {
            d.apply(EditOp::AddClip { track: None, clip: c }, OpSource::Ui).unwrap();
        }
        let Some(OpOutput::Track(lt)) = d
            .apply(EditOp::InsertTrack { index: 9, kind: TrackKind::Text }, OpSource::Ui)
            .ok()
        else {
            panic!()
        };
        let l = text(4.0, 12.0);
        let il = l.id;
        d.apply(EditOp::AddClip { track: Some(lt), clip: l }, OpSource::Ui).unwrap();
        d.apply(EditOp::SetTrackFlags { track: lt, locked: true, hidden: false, muted: false }, OpSource::Ui)
            .unwrap();
        let markers = [1.0, 5.0, 9.0]
            .iter()
            .enumerate()
            .map(|(i, t)| Marker {
                id: i as u64,
                at: s(*t),
                duration: Time::ZERO,
                name: String::new(),
                note: String::new(),
                color: 0,
            })
            .collect();
        d.apply(EditOp::SetMarkers { markers }, OpSource::Ui).unwrap();
        let main = d.project().find_clip(vid).unwrap().0.id;
        let before = d.project().clone();

        // only its own track: text stays, markers move anyway
        d.apply(EditOp::RippleRange { track: main, from: s(4.0), to: s(6.0), all_tracks: false }, OpSource::Ui)
            .unwrap();
        assert_eq!(d.project().find_clip(ib).unwrap().1.start, s(7.0));
        let at: Vec<Time> = d.project().timeline.markers.iter().map(|m| m.at).collect();
        assert_eq!(at, vec![s(1.0), s(4.0), s(7.0)]);
        d.undo().unwrap();
        assert_eq!(*d.project(), before);

        d.apply(EditOp::RippleRange { track: main, from: s(4.0), to: s(6.0), all_tracks: true }, OpSource::Ui)
            .unwrap();
        let p = d.project();
        let mt = p.timeline.tracks.iter().find(|t| t.main).unwrap();
        let spans: Vec<(Time, Time, Time)> =
            mt.clips.iter().map(|c| (c.start, c.duration, c.source_in)).collect();
        assert_eq!(spans, vec![(s(0.0), s(4.0), s(0.0)), (s(4.0), s(4.0), s(6.0))]);
        let ca = p.find_clip(ia).unwrap().1;
        assert_eq!((ca.start, ca.end()), (s(3.0), s(4.0)));
        assert_eq!(p.find_clip(ib).unwrap().1.start, s(5.0));
        let cl = p.find_clip(il).unwrap().1;
        assert_eq!((cl.start, cl.end()), (s(4.0), s(12.0)), "locked track stays");
        let at: Vec<Time> = p.timeline.markers.iter().map(|m| m.at).collect();
        assert_eq!(at, vec![s(1.0), s(4.0), s(7.0)]);
        d.undo().unwrap();
        assert_eq!(*d.project(), before);
    }

    #[test]
    fn ripple_ops_merge_and_run_back_to_front() {
        let s = Time::from_secs_f64;
        let (a, b) = (TrackId::new(), TrackId::new());
        let ops = ripple_ops(vec![(a, s(1.0), s(3.0)), (b, s(2.0), s(4.0)), (a, s(6.0), s(7.0))], true);
        let r: Vec<(Time, Time)> = ops
            .iter()
            .map(|o| match o {
                EditOp::RippleRange { from, to, .. } => (*from, *to),
                _ => panic!(),
            })
            .collect();
        assert_eq!(r, vec![(s(6.0), s(7.0)), (s(1.0), s(4.0))]);
        assert_eq!(ripple_ops(vec![(a, s(1.0), s(3.0)), (b, s(2.0), s(4.0))], false).len(), 2);
        let c = |x: f64, y: f64| Clip::new(ClipContent::Text(TextStyle::default()), s(x), s(y - x));
        assert_eq!(gaps(&[c(1.0, 2.0), c(4.0, 5.0), c(1.5, 3.0)]), vec![(s(0.0), s(1.0)), (s(3.0), s(4.0))]);
    }
}
