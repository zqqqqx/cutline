//! Zufallsfolgen von Operationen (feste Seeds): nach jedem Schritt gelten die Invarianten,
//! fehlgeschlagene Ops ändern nichts, gesperrte Spuren bleiben unverändert, Rückgängig bis
//! zum Anfang ergibt das Ausgangsprojekt, Speichern+Laden ist verlustfrei.

mod common;
use cl_core::*;
use common::*;

fn pick_clip(p: &Project, r: &mut Lcg) -> ClipId {
    let all: Vec<ClipId> = p.timeline.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.id)).collect();
    if all.is_empty() || r.chance(5) {
        ClipId::new()
    } else {
        all[r.below(all.len())]
    }
}

fn pick_track(p: &Project, r: &mut Lcg) -> TrackId {
    let t = &p.timeline.tracks;
    if t.is_empty() || r.chance(5) {
        TrackId::new()
    } else {
        t[r.below(t.len())].id
    }
}

fn time(r: &mut Lcg) -> Time {
    match r.below(20) {
        0 => Time(-r.range(0.0, 5e6) as i64),
        1 => Time(i64::MAX - r.below(1000) as i64),
        2 => Time::ZERO,
        _ => s(r.range(0.0, 40.0)),
    }
}

fn random_op(p: &Project, media: &[MediaRef], r: &mut Lcg, depth: u32) -> EditOp {
    let clip = pick_clip(p, r);
    let track = pick_track(p, r);
    let opt_track = if r.chance(50) { None } else { Some(track) };
    match r.below(26) {
        0..=2 => {
            let start = time(r);
            let dur = if r.chance(5) { Time(r.below(3) as i64 - 1) } else { s(r.range(0.04, 8.0)) };
            let mut c = match r.below(5) {
                0 => text_clip(0.0, 1.0),
                1 => Clip::new(ClipContent::Effect(FxRef::new("blur")), Time::ZERO, Time(1)),
                _ => media_clip(&media[r.below(media.len())], 0.0, 1.0),
            };
            c.start = start;
            c.duration = dur;
            c.source_in = s(r.range(0.0, 3.0));
            EditOp::AddClip { track: opt_track, clip: c }
        }
        3 => EditOp::RemoveClip { clip },
        4 | 5 => EditOp::SplitClip { clip, at: s(r.range(0.0, 40.0)) },
        6 | 7 => EditOp::TrimClip { clip, edge: if r.chance(50) { Edge::Start } else { Edge::End }, to: time(r) },
        8 | 9 => EditOp::MoveClip { clip, track: opt_track, start: time(r) },
        10 => EditOp::SetSpeed { clip, speed: r.range(0.05, 12.0) as f32 },
        11 => {
            // Nur kürzen: freies Setzen darf (für Rollen im Batch) vorübergehend überlappen
            let Some((_, c)) = p.find_clip(clip) else { return EditOp::RemoveClip { clip } };
            let dur = Time((c.duration.0 as f64 * r.range(0.1, 1.0)) as i64);
            EditOp::SetClipTiming { clip, start: c.start, duration: dur, source_in: c.source_in }
        }
        12 => {
            let t = p.timeline.tracks.iter().find(|t| t.id == track);
            EditOp::SetTrackFlags { track, locked: r.chance(40) && t.is_some(), hidden: r.chance(20), muted: r.chance(20) }
        }
        13 => {
            let kinds = [TrackKind::Video, TrackKind::Audio, TrackKind::Text, TrackKind::Effect];
            EditOp::InsertTrack { index: r.below(p.timeline.tracks.len() + 3), kind: kinds[r.below(4)] }
        }
        14 => EditOp::RemoveTrack { track },
        15 => EditOp::MoveTrack { track, up: r.chance(50) },
        16 => EditOp::MoveClipToNewTrack { clip, index: r.below(p.timeline.tracks.len() + 2), start: time(r) },
        17 => EditOp::CloseGaps { track: opt_track },
        18 | 19 => {
            let a = r.range(0.0, 30.0);
            EditOp::RippleRange { track, from: s(a), to: s(a + r.range(-1.0, 6.0)), all_tracks: r.chance(50) }
        }
        20 => {
            let mut settings = p.settings.clone();
            settings.main_track_magnet = !settings.main_track_magnet;
            EditOp::SetSettings { settings }
        }
        21 => EditOp::SetFades { clip, fade_in: time(r), fade_out: time(r) },
        22 => {
            let Some((_, c)) = p.find_clip(clip) else { return EditOp::SetOpacity { clip, opacity: 0.5 } };
            let mut c = c.clone();
            c.fx.reverse = !c.fx.reverse;
            c.start = time(r); // wird ignoriert: Lage bleibt
            EditOp::ReplaceClip { clip: c }
        }
        23 => {
            let markers = (0..r.below(4))
                .map(|i| Marker { id: i as u64, at: s(r.range(0.0, 30.0)), duration: Time::ZERO, name: "🎬".into(), note: String::new(), color: 0 })
                .collect();
            EditOp::SetMarkers { markers }
        }
        24 => EditOp::SetTransition {
            clip,
            transition: r.chance(70).then(|| TransitionRef { id: "dissolve".into(), duration: time(r) }),
        },
        _ if depth == 0 => EditOp::Batch {
            label: String::new(),
            ops: (0..1 + r.below(3)).map(|_| random_op(p, media, r, 1)).collect(),
        },
        _ => EditOp::Rename { name: "Ä".repeat(r.below(5)) },
    }
}

fn run(seed: u64, steps: usize) {
    let mut r = Lcg(seed);
    let rc = rich();
    let media = [rc.video.clone(), rc.audio.clone(), rc.image.clone()];
    // eigener Verlauf ab dem fertigen Testprojekt: Rückgängig kann nicht in den Aufbau laufen
    let mut d = Document::new(rc.doc.project().clone());
    d.max_history = usize::MAX;
    let start = d.project().clone();
    let (mut ok, mut err) = (0, 0);
    for step in 0..steps {
        let ctx = format!("Seed {seed}, Schritt {step}");
        let before = d.project().clone();
        let hist = d.undo_entries().len();
        match r.below(20) {
            0 => {
                let _ = d.undo();
                check(d.project(), &ctx);
                continue;
            }
            1 => {
                let _ = d.redo();
                check(d.project(), &ctx);
                continue;
            }
            _ => {}
        }
        let op = random_op(&before, &media, &mut r, 0);
        let op_dbg = format!("{op:?}");
        let desc: String = op_dbg.chars().take(300).collect();
        let desc = &desc;
        match d.apply(op, OpSource::Ui) {
            Ok(_) => {
                ok += 1;
                let after = d.project();
                check(after, &format!("{ctx}: {desc}"));
                assert_eq!(d.undo_entries().len(), hist + 1, "{ctx}");
                // gesperrte Spuren: Clips unverändert, Spur bleibt bestehen
                // (eine Op, die selbst Sperren ändert, z. B. Batch „entsperren + löschen“, ausgenommen)
                let flags = op_dbg.contains("SetTrackFlags");
                for t in before.timeline.tracks.iter().filter(|t| t.locked && !flags) {
                    let now = after.timeline.tracks.iter().find(|x| x.id == t.id);
                    let now = now.unwrap_or_else(|| panic!("{ctx}: gesperrte Spur verschwunden: {desc}"));
                    if now.locked {
                        assert_eq!(now.clips, t.clips, "{ctx}: gesperrte Spur verändert: {desc}");
                    }
                }
                if step % 50 == 0 {
                    assert_eq!(&roundtrip(after), after, "{ctx}: Speichern+Laden");
                }
            }
            Err(e) => {
                err += 1;
                assert_eq!(d.project(), &before, "{ctx}: {e} ließ Reste zurück: {desc}");
                assert_eq!(d.undo_entries().len(), hist, "{ctx}");
            }
        }
    }
    assert!(ok > steps / 4 && err > 0, "Seed {seed}: zu einseitig ({ok} ok, {err} Fehler)");
    let end = d.project().clone();
    assert_eq!(&roundtrip(&end), &end);
    let n = d.undo_entries().len();
    assert_eq!(d.step(-(n as isize)), n);
    assert_eq!(d.project(), &start, "Seed {seed}: Rückgängig bis zum Anfang");
    assert_eq!(d.step(n as isize), n);
    assert_eq!(d.project(), &end, "Seed {seed}: alles wiederholen");
}

#[test]
fn random_op_sequences_keep_invariants() {
    for seed in 1..=12 {
        run(seed, 600);
    }
}

/// Langer Lauf: `cargo test --release -p cl-core -- --ignored long_random`
#[test]
#[ignore]
fn long_random_op_sequences() {
    for seed in 100..400 {
        run(seed, 3000);
    }
}
