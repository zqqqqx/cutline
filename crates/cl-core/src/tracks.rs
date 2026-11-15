//! Spuren anordnen: Anzeige-Reihenfolge, Stelle für eine neue Spur, Spur an eine andere Stelle ziehen.
//!
//! „Zeile“ = Position in der Anzeige von oben (0 = oberste Spur), „Lücke“ `g` = Stelle direkt
//! über Zeile `g` (`g` = Anzahl Zeilen: unter der untersten). Ordnungsregeln: Bildspuren (alles
//! außer Ton) liegen über allen Tonspuren, unter der Hauptspur liegt keine Bildspur.

use crate::document::{Document, OpSource};
use crate::model::*;
use crate::ops::EditOp;
use crate::time::Time;
use crate::{CoreError, Result};

/// Spurindizes in Anzeige-Reihenfolge: Bildspuren mit höherem Index oben, darunter Ton in Indexfolge.
pub fn display_order(tracks: &[Track]) -> Vec<usize> {
    let audio = |i: &usize| tracks[*i].kind == TrackKind::Audio;
    let mut o: Vec<usize> = (0..tracks.len()).rev().filter(|i| !audio(i)).collect();
    o.extend((0..tracks.len()).filter(audio));
    o
}

/// Nimmt die Spur Clips der Art `kind` auf (passende Art, nicht gesperrt)?
pub fn fits(t: &Track, kind: TrackKind) -> bool {
    t.kind.accepts(kind) && !t.locked
}

/// Erlaubte Lücken (von, bis einschließlich) für eine Spur der Art `kind`.
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

/// Lücke für eine neue Spur der Art `kind`, wenn über Zeile `row` (in der oberen Hälfte:
/// `upper`) abgelegt wird: direkt darüber bzw. darunter, sonst die nächste erlaubte Stelle.
pub fn new_track_gap(tracks: &[Track], row: usize, upper: bool, kind: TrackKind) -> usize {
    let (lo, hi) = gap_range(tracks, kind);
    (row + usize::from(!upper)).clamp(lo, hi)
}

/// Index für [`EditOp::InsertTrack`], damit die neue Spur in der Anzeige an Lücke `gap` steht.
pub fn gap_index(tracks: &[Track], gap: usize, kind: TrackKind) -> usize {
    let ord = display_order(tracks);
    let visual = tracks.iter().filter(|t| t.kind != TrackKind::Audio).count();
    if kind != TrackKind::Audio {
        match gap.min(visual) {
            g if g < visual => ord[g] + 1, // direkt über Zeile g
            0 => 0,
            g => ord[g - 1], // unter der untersten Bildspur
        }
    } else {
        match gap.clamp(visual, tracks.len()) {
            g if g < tracks.len() => ord[g], // direkt vor Tonspur g
            g if g > visual => ord[g - 1] + 1,
            _ => tracks.len(),
        }
    }
}

/// Zielstelle beim Ziehen der Spur in Zeile `row` zur Lücke `gap`: die nächste erlaubte;
/// `None`, wenn die Spur nicht verschoben werden darf (Hauptspur) oder an ihrem Platz bliebe.
pub fn track_drop_gap(tracks: &[Track], row: usize, gap: usize) -> Option<usize> {
    let t = &tracks[*display_order(tracks).get(row)?];
    if t.main {
        return None;
    }
    let (lo, hi) = gap_range(tracks, t.kind);
    let g = gap.clamp(lo, hi);
    (g != row && g != row + 1).then_some(g)
}

/// Spur in Zeile `row` an Lücke `gap` verschieben – als ein Verlaufsschritt aus
/// [`EditOp::MoveTrack`]-Schritten (Nachbarn gleicher Art werden übersprungen).
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

/// Clips (Kennung, neuer Beginn) in neue Spuren neben Zeile `row` verschieben: je Clip-Art eine
/// Spur an der Stelle aus [`new_track_gap`]; Clips derselben Art landen gemeinsam darin. Ein
/// Verlaufsschritt; schlägt etwas fehl, bleibt das Projekt unverändert.
pub fn move_to_new_tracks(doc: &mut Document, row: usize, upper: bool, moves: &[(ClipId, Time)]) -> Result<()> {
    let p = doc.project();
    let tracks = &p.timeline.tracks;
    let mut kinds: Vec<(TrackKind, usize)> = Vec::new();
    for (id, _) in moves {
        let k = p.find_clip(*id).ok_or(CoreError::ClipNotFound(*id))?.1.kind();
        if !kinds.iter().any(|x| x.0 == k) {
            kinds.push((k, gap_index(tracks, new_track_gap(tracks, row, upper, k), k)));
        }
    }
    // höchster Index zuerst: spätere Einfügungen verschieben die schon angelegten nicht gegeneinander
    kinds.sort_by_key(|k| std::cmp::Reverse(k.1));
    doc.transaction(|d| {
        let mut new = Vec::new();
        for (kind, index) in kinds {
            if let crate::OpOutput::Track(t) = d.apply(EditOp::InsertTrack { index, kind }, OpSource::Ui)? {
                new.push((kind, t));
            }
        }
        let ops = moves
            .iter()
            .map(|(clip, start)| {
                let kind = d.project().find_clip(*clip).map(|c| c.1.kind());
                let track = new.iter().find(|n| Some(n.0) == kind).map(|n| n.1);
                EditOp::MoveClip { clip: *clip, track, start: *start }
            })
            .collect();
        d.apply(EditOp::Batch { label: "Auf neue Spur verschieben".into(), ops }, OpSource::Ui)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use TrackKind::*;

    /// Spuren in Anzeige-Reihenfolge von oben; `M` = Hauptspur (Video), `L` = gesperrt.
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
        // Video über Text: obere Hälfte darüber, untere darunter
        assert_eq!(new_track_gap(t, 0, true, Video), 0);
        assert_eq!(new_track_gap(t, 0, false, Video), 1);
        // Text über der Hauptspur (untere Hälfte): darunter wäre unter der Hauptspur → direkt darüber
        assert_eq!(new_track_gap(t, 2, false, Text), 2);
        // Ton über Bildspuren: oberste erlaubte Stelle im Tonbereich
        assert_eq!(new_track_gap(t, 0, true, Audio), 3);
        // Bild über Ton: direkt über der Hauptspur
        assert_eq!(new_track_gap(t, 4, false, Effect), 2);
        // Ränder: ganz unten bzw. ganz oben
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
            // ohne Hauptspur darf Bild ganz unten in den Bildbereich
            (&["T", "V", "A1"][..], 2, true, Effect, vec!["T", "V", "neu Effect", "A1"]),
            // nur Ton: Bildspur kommt darüber
            (&["A1"][..], 0, false, Text, vec!["neu Text", "A1"]),
            // nur Bild: Ton darunter
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
        clip(&mut p, 4, 5.0); // A2 behält einen Clip (leer gewordene Spuren entfernt MoveClip wie bisher)
        let mut d = Document::new(p);
        let before = d.project().clone();
        // Mehrfachauswahl über der gesperrten Tonspur A1 (untere Hälfte): Bild über die Hauptspur,
        // Ton direkt unter A1; Abstände bleiben
        let s = |x: f64| Time::from_secs_f64(x);
        move_to_new_tracks(&mut d, 3, false, &[(v, s(10.0)), (v2, s(13.0)), (a, s(11.0))]).unwrap();
        assert_eq!(names(d.project()), ["T", "V", "neu Video", "M", "A1L", "neu Audio", "A2"]);
        let tr = &d.project().timeline.tracks;
        let ord = display_order(tr);
        let starts = |row: usize| tr[ord[row]].clips.iter().map(|c| c.start.as_secs_f64()).collect::<Vec<_>>();
        assert_eq!(starts(2), [10.0, 13.0]);
        assert_eq!(starts(5), [11.0]);
        assert_eq!(starts(1), [6.0], "nicht gezogene Clips bleiben");
        assert_eq!(d.undo_entries().len(), 1);
        d.undo().unwrap();
        assert_eq!(d.project(), &before, "vollständig umkehrbar");
        d.redo().unwrap();
        assert_eq!(names(d.project()).len(), 7);
    }

    #[test]
    fn failed_move_leaves_nothing_behind() {
        let mut p = proj(&["VL", "M", "A1"]);
        let v = clip(&mut p, 0, 0.0); // liegt auf gesperrter Spur: Verschieben scheitert
        let mut d = Document::new(p);
        let before = d.project().clone();
        assert!(move_to_new_tracks(&mut d, 2, true, &[(v, Time::from_secs_f64(2.0))]).is_err());
        assert_eq!(d.project(), &before, "keine neue Spur");
        assert!(d.undo_entries().is_empty(), "kein Verlaufsschritt");
    }

    #[test]
    fn track_drag_target_skips_forbidden_spots() {
        let p = proj(&["T", "V", "M", "A1", "A2"]);
        let t = &p.timeline.tracks;
        assert_eq!(track_drop_gap(t, 0, 2), Some(2), "Text unter V");
        assert_eq!(track_drop_gap(t, 0, 4), Some(2), "nicht unter die Hauptspur");
        assert_eq!(track_drop_gap(t, 0, 0), None, "auf sich selbst");
        assert_eq!(track_drop_gap(t, 0, 1), None, "auf sich selbst (darunter)");
        assert_eq!(track_drop_gap(t, 2, 0), None, "Hauptspur bleibt");
        assert_eq!(track_drop_gap(t, 4, 0), Some(3), "Ton bleibt unter den Bildspuren");
        assert_eq!(track_drop_gap(t, 3, 5), Some(5));
        assert_eq!(track_drop_gap(t, 9, 0), None, "keine solche Zeile");
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

    #[test]
    fn clip_moves_within_its_own_track_whatever_the_kind() {
        // Bild-Clips (Art „Video“) auf einer Sticker-Spur, wie in erzeugten Projekten
        let mut p = proj(&["S", "V", "M"]);
        let png = clip(&mut p, 0, 0.0);
        // Altlast: Text-Clip auf einer Bildspur – in seiner Spur trotzdem verschiebbar
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
        // Spurwechsel prüft weiter die Art: Text gehört nicht auf die Sticker-Spur
        let sticker = d.project().timeline.tracks.iter().find(|t| t.kind == Sticker).unwrap().id;
        assert!(d.apply(EditOp::MoveClip { clip: txt, track: Some(sticker), start: s(1.0) }, OpSource::Ui).is_err());
        assert!(fits(&d.project().timeline.tracks[display_order(&d.project().timeline.tracks)[0]], Video));
    }
}
