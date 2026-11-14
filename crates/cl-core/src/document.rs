use crate::model::Project;
use crate::ops::{EditOp, OpOutput};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// Woher eine Änderung kam. Wird im Verlauf angezeigt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "name", rename_all = "snake_case")]
pub enum OpSource {
    Ui,
    Mcp(String),
    Plugin(String),
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub label: &'static str,
    pub source: OpSource,
    /// Zeitpunkt der Änderung
    pub at: std::time::SystemTime,
    snapshot: Project,
}

/// Projekt + Undo/Redo-Verlauf.
///
/// Phase 0 speichert Snapshots (Projekte sind kleine JSON-Bäume). Für sehr große
/// Projekte werden später inverse Ops gespeichert. Die API bleibt dabei gleich.
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
            max_history: 500,
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Wird bei jeder Änderung erhöht. Nützlich, um Caches/Ansichten zu invalidieren.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn apply(&mut self, op: EditOp, source: OpSource) -> Result<OpOutput> {
        let label = op.label();
        let before = self.project.clone();
        match op.apply(&mut self.project) {
            Ok(out) => {
                self.undo.push(HistoryEntry {
                    label,
                    source,
                    at: std::time::SystemTime::now(),
                    snapshot: before,
                });
                if self.undo.len() > self.max_history {
                    self.undo.remove(0);
                }
                self.redo.clear();
                self.revision += 1;
                Ok(out)
            }
            Err(e) => {
                self.project = before; // atomar: bei Fehler unverändert
                Err(e)
            }
        }
    }

    pub fn undo(&mut self) -> Result<&'static str> {
        let entry = self.undo.pop().ok_or(CoreError::NothingToUndo)?;
        let label = entry.label;
        let current = std::mem::replace(&mut self.project, entry.snapshot);
        self.redo.push(HistoryEntry {
            label,
            source: entry.source,
            at: entry.at,
            snapshot: current,
        });
        self.revision += 1;
        Ok(label)
    }

    pub fn redo(&mut self) -> Result<&'static str> {
        let entry = self.redo.pop().ok_or(CoreError::NothingToRedo)?;
        let label = entry.label;
        let current = std::mem::replace(&mut self.project, entry.snapshot);
        self.undo.push(HistoryEntry {
            label,
            source: entry.source,
            at: entry.at,
            snapshot: current,
        });
        self.revision += 1;
        Ok(label)
    }

    /// Mehrere Schritte auf einmal: `steps` < 0 rückgängig, > 0 wiederholen. Gibt die Anzahl
    /// der tatsächlich ausgeführten Schritte zurück.
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

    /// Rückgängig machbare Schritte (älteste zuerst)
    pub fn undo_entries(&self) -> &[HistoryEntry] {
        &self.undo
    }

    /// Wiederholbare Schritte (der nächste zuletzt)
    pub fn redo_entries(&self) -> &[HistoryEntry] {
        &self.redo
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Verlauf (älteste zuerst) als (Bezeichnung, Quelle).
    pub fn history(&self) -> impl Iterator<Item = (&'static str, &OpSource)> {
        self.undo.iter().map(|e| (e.label, &e.source))
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
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
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
    fn video_goes_to_main_track_and_magnet_appends() {
        let (mut d, _) = doc_with_clip();
        let m = d.project().media[0].clone();
        // Startzeit 99 s wird vom Magnet auf 10 s gezogen.
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
        // Ablage bei 0 s → vor den bestehenden Clip
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
        // Ablage bei 9 s (hinter der Mitte des 10-s-Clips, der jetzt bei 2–12 liegt) → davor, denn Mitte = 7 < 9 → dahinter
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
        // Verschieben an den Anfang
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
    fn reversed_clip_keeps_frames_on_split_and_trim() {
        let (mut d, id) = doc_with_clip(); // 0–10 s, Quelle 10 s
        let before = |t: f64| Time::from_secs_f64(9.0 - t); // Quellzeit an Zeitleistenposition t
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
            source_in: Time::from_secs_f64(1.0), // Quelle 1–9 s, rückwärts
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
            assert_eq!(src(&d, t), before(t), "nach Teilen bei {t} s");
        }
        // Ende des rechten Teils um 1 s kürzen, Anfang des linken um 1 s kürzen
        d.apply(EditOp::TrimClip { clip: right, edge: Edge::End, to: Time::from_secs_f64(7.0) }, OpSource::Ui).unwrap();
        let (_, r) = d.project().find_clip(right).unwrap();
        assert_eq!(r.source_time(Time::from_secs_f64(3.0)), before(3.0));
        assert_eq!(r.source_in, Time::from_secs_f64(2.0));
        d.apply(EditOp::TrimClip { clip: id, edge: Edge::Start, to: Time::from_secs_f64(1.0) }, OpSource::Ui).unwrap();
        let (_, l) = d.project().find_clip(id).unwrap();
        assert_eq!(l.source_time(l.start), before(1.0));
        // Ende verlängern: höchstens bis Quellzeit 0, der Anfang bleibt gleich
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
        // Text liegt oben; nach unten → unter die Videospur
        d.apply(EditOp::MoveTrack { track: t1, up: false }, OpSource::Ui).unwrap();
        assert_eq!(ids(&d)[..2], [t1, v1]);
        // ganz unten: bleibt
        d.apply(EditOp::MoveTrack { track: t1, up: false }, OpSource::Ui).unwrap();
        assert_eq!(ids(&d)[..2], [t1, v1]);
        // Tonspur a2 (unten) nach oben → vor a1, aber nicht zwischen die Bildspuren
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
        assert_eq!(clips(&d), 2); // nur der erste Schnitt trifft den (immer kürzeren) linken Teil
        let total = d.undo_entries().len();
        assert_eq!(d.step(-(total as isize)), total);
        assert_eq!(d.undo_entries().len(), 0);
        assert_eq!(d.redo_entries().len(), total);
        assert_eq!(d.step(-1), 0);
        assert_eq!(d.step(2), 2.min(total));
        assert!(d.undo_entries().iter().all(|e| e.at <= std::time::SystemTime::now()));
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
        // Anfang des ersten Clips um 1 s kürzen → alles rückt nach links.
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
        // Video über das Quellende hinaus: still auf das Ende begrenzt (kein Fehler).
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
        // Teilen bei 2 s → rechter Teil beginnt in der Quelle bei 4 s
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
        // Ins Leere ziehen → neue Spur
        d.apply(
            EditOp::MoveClipToNewTrack {
                clip: tc,
                index: 2,
                start: Time::from_secs_f64(1.0),
            },
            OpSource::Ui,
        )
        .unwrap();
        // alte, jetzt leere Textspur wurde entfernt
        assert_eq!(d.project().timeline.tracks.len(), 2);
        assert_eq!(
            d.project().find_clip(tc).unwrap().1.start,
            Time::from_secs_f64(1.0)
        );
        // Spur einfügen und wieder löschen
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
            "nichts unter die Hauptspur"
        );
        d.apply(EditOp::RemoveTrack { track: nt }, OpSource::Ui)
            .unwrap();
        assert_eq!(d.project().timeline.tracks.len(), 2);
        // Clip löschen entfernt seine leere Spur
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
        // alte Projekte ohne neue Felder laden weiter
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
            OpSource::Mcp("claude".into()),
        )
        .unwrap();
        assert!(d.project().find_clip(id).is_none());
        assert_eq!(
            d.history().last().unwrap().1,
            &OpSource::Mcp("claude".into())
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
}
