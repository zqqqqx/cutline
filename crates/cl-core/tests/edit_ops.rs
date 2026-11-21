//! Every EditOp variant: apply, undo, redo yield exactly the initial or
//! target state; plus targeted edge cases and bugs found here.

mod common;
use cl_core::*;
use common::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Forces an extension of this test when a variant is added (no `_` arm).
fn variant(op: &EditOp) -> &'static str {
    match op {
        EditOp::ImportMedia { .. } => "ImportMedia",
        EditOp::AddTrack { .. } => "AddTrack",
        EditOp::SetTrackFlags { .. } => "SetTrackFlags",
        EditOp::SetTrackView { .. } => "SetTrackView",
        EditOp::AddClip { .. } => "AddClip",
        EditOp::RemoveClip { .. } => "RemoveClip",
        EditOp::SplitClip { .. } => "SplitClip",
        EditOp::TrimClip { .. } => "TrimClip",
        EditOp::MoveClip { .. } => "MoveClip",
        EditOp::SetText { .. } => "SetText",
        EditOp::SetTransform { .. } => "SetTransform",
        EditOp::SetCrop { .. } => "SetCrop",
        EditOp::SetOpacity { .. } => "SetOpacity",
        EditOp::SetVolume { .. } => "SetVolume",
        EditOp::SetSettings { .. } => "SetSettings",
        EditOp::AddProjectFont { .. } => "AddProjectFont",
        EditOp::Rename { .. } => "Rename",
        EditOp::SetSpeed { .. } => "SetSpeed",
        EditOp::SetFx { .. } => "SetFx",
        EditOp::SetKeyframes { .. } => "SetKeyframes",
        EditOp::SetAnimations { .. } => "SetAnimations",
        EditOp::SetTransition { .. } => "SetTransition",
        EditOp::SetClipDisabled { .. } => "SetClipDisabled",
        EditOp::SetFades { .. } => "SetFades",
        EditOp::ReplaceClip { .. } => "ReplaceClip",
        EditOp::InsertTrack { .. } => "InsertTrack",
        EditOp::RemoveTrack { .. } => "RemoveTrack",
        EditOp::MoveTrack { .. } => "MoveTrack",
        EditOp::MoveClipToNewTrack { .. } => "MoveClipToNewTrack",
        EditOp::CloseGaps { .. } => "CloseGaps",
        EditOp::SetClipTiming { .. } => "SetClipTiming",
        EditOp::RelinkMedia { .. } => "RelinkMedia",
        EditOp::SetMarkers { .. } => "SetMarkers",
        EditOp::RenameTrack { .. } => "RenameTrack",
        EditOp::RippleRange { .. } => "RippleRange",
        EditOp::Batch { .. } => "Batch",
    }
}
const VARIANTS: usize = 36;

fn track_of(p: &Project, c: ClipId) -> TrackId {
    p.find_clip(c).unwrap().0.id
}

#[test]
fn every_op_applies_undoes_and_redoes_exactly() {
    let r = rich();
    let mut d = r.doc;
    let clip = |p: &Project, id: ClipId| p.find_clip(id).unwrap().1.clone();
    type Make = Box<dyn Fn(&Project) -> EditOp>;
    let (v1, v2, a1, t1, t2, fx) = (r.v1, r.v2, r.a1, r.t1, r.t2, r.fx);
    let video = r.video.clone();
    let audio = r.audio.clone();
    let ops: Vec<Make> = vec![
        Box::new(|_| EditOp::ImportMedia { media: media(MediaKind::Video, 3.0) }),
        Box::new(|_| EditOp::AddTrack { kind: TrackKind::Sticker }),
        Box::new(move |p| EditOp::SetTrackFlags { track: track_of(p, a1), locked: false, hidden: true, muted: true }),
        Box::new(move |p| EditOp::SetTrackView { track: track_of(p, a1), collapsed: true, height: 1.5 }),
        Box::new(move |_| EditOp::AddClip { track: None, clip: media_clip(&audio, 10.0, 1.0) }),
        Box::new(move |_| EditOp::RemoveClip { clip: t2 }),
        Box::new(move |_| EditOp::SplitClip { clip: v1, at: s(2.0) }),
        Box::new(move |_| EditOp::TrimClip { clip: a1, edge: Edge::End, to: s(5.0) }),
        Box::new(move |_| EditOp::MoveClip { clip: t1, track: None, start: s(8.0) }),
        Box::new(move |_| EditOp::SetText { clip: t1, style: TextStyle { text: "Neu 🎬 שלום".into(), ..Default::default() } }),
        Box::new(move |_| EditOp::SetTransform { clip: v2, transform: Transform { x: 10.0, y: -5.0, scale: 1.5, rotation_deg: 30.0 } }),
        Box::new(move |_| EditOp::SetCrop { clip: v2, crop: Crop { left: 0.1, right: 0.2, top: 0.05, bottom: 0.25 } }),
        Box::new(move |_| EditOp::SetOpacity { clip: v2, opacity: 0.5 }),
        Box::new(move |_| EditOp::SetVolume { clip: a1, volume_db: -6.0 }),
        Box::new(|p| {
            let mut settings = p.settings.clone();
            settings.apply_aspect(AspectPreset::Ratio(9, 16));
            EditOp::SetSettings { settings }
        }),
        Box::new(|_| EditOp::AddProjectFont { path: PathBuf::from("fonts/Ä b.ttf") }),
        Box::new(|_| EditOp::Rename { name: "Ümlaut 🎞".into() }),
        Box::new(move |_| EditOp::SetSpeed { clip: v2, speed: 2.0 }),
        Box::new(move |_| EditOp::SetFx { clip: v2, fx: ClipFx { reverse: true, denoise: 0.3, ..Default::default() } }),
        Box::new(move |_| {
            let mut k = Keyframes::default();
            k.set(Prop::Scale, Time::ZERO, 1.0);
            k.set(Prop::Scale, s(1.0), 2.0);
            EditOp::SetKeyframes { clip: v2, keyframes: k }
        }),
        Box::new(move |_| EditOp::SetAnimations {
            clip: t1,
            anim: Animations { intro: Some(AnimRef { id: "fade".into(), duration: s(0.5) }), ..Default::default() },
        }),
        Box::new(move |p| EditOp::SetTransition {
            clip: p.timeline.tracks.iter().find(|t| t.main).unwrap().clips[0].id,
            transition: Some(TransitionRef { id: "dissolve".into(), duration: s(0.5) }),
        }),
        Box::new(move |_| EditOp::SetClipDisabled { clip: fx, disabled: true }),
        Box::new(move |_| EditOp::SetFades { clip: a1, fade_in: s(0.5), fade_out: s(1.0) }),
        Box::new(move |p| {
            let mut c = clip(p, a1);
            c.label = Some(3);
            c.group = Some(7);
            EditOp::ReplaceClip { clip: c }
        }),
        Box::new(|p| EditOp::InsertTrack { index: p.timeline.tracks.len(), kind: TrackKind::Text }),
        Box::new(|p| {
            let t = p.timeline.tracks.iter().rev().find(|t| t.kind == TrackKind::Sticker).unwrap();
            EditOp::RemoveTrack { track: t.id }
        }),
        Box::new(|p| {
            let t = p.timeline.tracks.iter().find(|t| t.kind == TrackKind::Text && t.clips.is_empty()).unwrap();
            EditOp::MoveTrack { track: t.id, up: false }
        }),
        Box::new(move |p| EditOp::MoveClipToNewTrack { clip: fx, index: p.timeline.tracks.len(), start: s(1.0) }),
        Box::new(|p| EditOp::CloseGaps { track: Some(p.find_clip_by_kind(TrackKind::Text)) }),
        Box::new(move |p| {
            let c = clip(p, a1);
            EditOp::SetClipTiming { clip: a1, start: c.start + s(0.5), duration: c.duration - s(1.0), source_in: s(0.25) }
        }),
        Box::new(move |_| EditOp::RelinkMedia { media: video.id, path: PathBuf::from("neu/ört.mov") }),
        Box::new(|_| EditOp::SetMarkers {
            markers: vec![Marker { id: 9, at: s(3.0), duration: s(1.0), name: "Kapitel".into(), note: "Notiz".into(), color: 2 }],
        }),
        Box::new(move |p| EditOp::RenameTrack { track: track_of(p, a1), name: Some("  Sprache  ".into()) }),
        Box::new(move |p| EditOp::RippleRange { track: track_of(p, v2), from: s(1.0), to: s(2.0), all_tracks: true }),
        Box::new(move |_| EditOp::Batch {
            label: "Zwei".into(),
            ops: vec![EditOp::SetOpacity { clip: v2, opacity: 0.2 }, EditOp::Rename { name: "B".into() }],
        }),
    ];
    let mut seen = BTreeSet::new();
    for make in &ops {
        let op = make(d.project());
        let name = variant(&op);
        seen.insert(name);
        let before = d.project().clone();
        let undo_len = d.undo_entries().len();
        apply(&mut d, op.clone());
        let after = d.project().clone();
        assert_ne!(after, before, "{name} changed nothing");
        check(&after, name);
        assert_eq!(roundtrip(&after), after, "{name}: Speichern+Laden");
        // the op itself is serializable (MCP/plugins send JSON)
        let back: EditOp = serde_json::from_str(&serde_json::to_string(&op).unwrap()).unwrap();
        assert_eq!(back, op, "{name}: Op-JSON");
        assert_eq!(d.undo_entries().len(), undo_len + 1, "{name}: one history step");
        assert_eq!(d.undo().unwrap(), op.label(), "{name}");
        assert_eq!(d.project(), &before, "{name}: undo");
        d.redo().unwrap();
        assert_eq!(d.project(), &after, "{name}: Wiederholen");
    }
    assert_eq!(seen.len(), VARIANTS, "jede Variante einmal: {seen:?}");
    // all the way back gives the initial project, all the way forward the final state again
    let end = d.project().clone();
    let n = d.undo_entries().len();
    assert_eq!(d.step(-(n as isize)), n);
    assert_eq!(d.project(), &Document::new(Project::new("Reich")).project().clone());
    assert_eq!(d.step(n as isize), n);
    assert_eq!(d.project(), &end);
}

trait FindKind {
    fn find_clip_by_kind(&self, k: TrackKind) -> TrackId;
}
impl FindKind for Project {
    fn find_clip_by_kind(&self, k: TrackKind) -> TrackId {
        self.timeline.tracks.iter().find(|t| t.kind == k && !t.clips.is_empty()).unwrap().id
    }
}

#[test]
fn same_op_twice_deleted_clips_and_missing_tracks_fail_cleanly() {
    let r = rich();
    let mut d = r.doc;
    apply(&mut d, EditOp::RemoveClip { clip: r.t1 });
    let snap = d.project().clone();
    let hist = d.undo_entries().len();
    let gone = TrackId::new();
    for op in [
        EditOp::RemoveClip { clip: r.t1 },
        EditOp::SplitClip { clip: r.t1, at: s(1.0) },
        EditOp::TrimClip { clip: r.t1, edge: Edge::End, to: s(9.0) },
        EditOp::MoveClip { clip: r.t1, track: None, start: s(1.0) },
        EditOp::SetOpacity { clip: r.t1, opacity: 0.1 },
        EditOp::ReplaceClip { clip: text_clip(0.0, 1.0) },
        EditOp::SetText { clip: r.v1, style: TextStyle::default() },
        EditOp::RemoveTrack { track: gone },
        EditOp::MoveTrack { track: gone, up: true },
        EditOp::RenameTrack { track: gone, name: None },
        EditOp::CloseGaps { track: Some(gone) },
        EditOp::RippleRange { track: gone, from: s(0.0), to: s(1.0), all_tracks: true },
        EditOp::AddClip { track: Some(gone), clip: text_clip(0.0, 1.0) },
        EditOp::MoveClip { clip: r.t2, track: Some(gone), start: s(0.0) },
        EditOp::RelinkMedia { media: MediaId::new(), path: PathBuf::new() },
        EditOp::AddClip { track: None, clip: media_clip(&media(MediaKind::Video, 1.0), 0.0, 1.0) },
        // batch: second op fails → the first one is taken back too
        EditOp::Batch { label: String::new(), ops: vec![EditOp::Rename { name: "x".into() }, EditOp::RemoveClip { clip: r.t1 }] },
    ] {
        let name = format!("{op:?}");
        assert!(d.apply(op, OpSource::Ui).is_err(), "{name}");
        assert_eq!(d.project(), &snap, "{name} changed the project");
        assert_eq!(d.undo_entries().len(), hist, "{name} steht im Verlauf");
    }
}

#[test]
fn undo_and_redo_on_empty_history() {
    let mut d = Document::new(Project::new("Leer"));
    assert_eq!(d.undo(), Err(CoreError::NothingToUndo));
    assert_eq!(d.redo(), Err(CoreError::NothingToRedo));
    assert_eq!(d.step(-5), 0);
    assert_eq!(d.step(5), 0);
    assert_eq!(d.revision(), 0);
    // batch without ops is valid but changes nothing: no history step
    apply(&mut d, EditOp::Batch { label: String::new(), ops: vec![] });
    assert_eq!(d.undo(), Err(CoreError::NothingToUndo));
    // history is limited: the oldest step drops out
    d.max_history = 3;
    for i in 0..5 {
        apply(&mut d, EditOp::Rename { name: format!("{i}") });
    }
    assert_eq!(d.undo_entries().len(), 3);
    assert_eq!(d.step(-10), 3);
    assert_eq!(d.project().name, "1");
}

/// Every step is a full project copy: large projects keep fewer steps.
#[test]
fn history_shrinks_for_big_projects() {
    let mut p = Project::new("Groß");
    let mut t = Track::new(TrackKind::Video);
    t.clips = (0..5000).map(|i| Clip::new(ClipContent::Text(TextStyle::default()), s(i as f64), s(1.0))).collect();
    p.timeline.tracks.push(t);
    let mut d = Document::new(p);
    for i in 0..30 {
        apply(&mut d, EditOp::Rename { name: format!("{i}") });
    }
    assert_eq!(d.undo_entries().len(), 20);
    // small projects: 100 steps as before
    let mut d = Document::new(Project::new("Klein"));
    for i in 0..120 {
        apply(&mut d, EditOp::Rename { name: format!("{i}") });
    }
    assert_eq!(d.undo_entries().len(), 100);
}

#[test]
fn locked_track_rejects_every_clip_edit() {
    let r = rich();
    let mut d = r.doc;
    let tr = track_of(d.project(), r.a1);
    apply(&mut d, EditOp::SetTrackFlags { track: tr, locked: true, hidden: false, muted: false });
    let snap = d.project().clone();
    let c = d.project().find_clip(r.a1).unwrap().1.clone();
    for op in [
        EditOp::RemoveClip { clip: r.a1 },
        EditOp::SplitClip { clip: r.a1, at: s(2.0) },
        EditOp::TrimClip { clip: r.a1, edge: Edge::Start, to: s(2.0) },
        EditOp::MoveClip { clip: r.a1, track: None, start: s(0.0) },
        EditOp::SetSpeed { clip: r.a1, speed: 2.0 },
        EditOp::SetVolume { clip: r.a1, volume_db: 3.0 },
        EditOp::SetFades { clip: r.a1, fade_in: s(1.0), fade_out: s(1.0) },
        EditOp::ReplaceClip { clip: c.clone() },
        EditOp::SetClipTiming { clip: r.a1, start: s(0.0), duration: s(1.0), source_in: s(0.0) },
        EditOp::MoveClipToNewTrack { clip: r.a1, index: 0, start: s(0.0) },
        EditOp::RemoveTrack { track: tr },
        EditOp::AddClip { track: Some(tr), clip: media_clip(&r.audio, 20.0, 1.0) },
        EditOp::RippleRange { track: tr, from: s(0.0), to: s(1.0), all_tracks: false },
    ] {
        let name = format!("{op:?}");
        assert_eq!(d.apply(op, OpSource::Ui), Err(CoreError::TrackLocked), "{name}");
        assert_eq!(d.project(), &snap);
    }
    // closing gaps skips the locked track
    apply(&mut d, EditOp::CloseGaps { track: None });
    assert_eq!(d.project().find_clip(r.a1).unwrap().1.start, s(1.0));
}

/// Bug: without magnet a clip could be trimmed past its neighbour (overlap).
#[test]
fn trim_stops_at_neighbour_on_free_tracks() {
    let mut d = Document::new(Project::new("T"));
    let a = clip_out(apply(&mut d, EditOp::AddClip { track: None, clip: text_clip(0.0, 2.0) }));
    let tr = track_of(d.project(), a);
    let b = clip_out(apply(&mut d, EditOp::AddClip { track: Some(tr), clip: text_clip(3.0, 2.0) }));
    apply(&mut d, EditOp::TrimClip { clip: a, edge: Edge::End, to: s(10.0) });
    assert_eq!(d.project().find_clip(a).unwrap().1.end(), s(3.0));
    apply(&mut d, EditOp::TrimClip { clip: b, edge: Edge::Start, to: s(0.0) });
    assert_eq!(d.project().find_clip(b).unwrap().1.start, s(3.0));
    check(d.project(), "trim");
    // main track without magnet likewise
    let mut d = rich().doc;
    let mut st = d.project().settings.clone();
    st.main_track_magnet = false;
    apply(&mut d, EditOp::SetSettings { settings: st });
    let first = d.project().timeline.tracks.iter().find(|t| t.main).unwrap().clips[0].id;
    apply(&mut d, EditOp::TrimClip { clip: first, edge: Edge::End, to: s(30.0) });
    check(d.project(), "main track without magnet");
}

/// Bug: setting time (numeric entry, MCP) pushed a clip into its neighbour (overlap).
#[test]
fn set_timing_rejects_overlap_on_free_tracks() {
    let mut d = Document::new(Project::new("T"));
    let a = clip_out(apply(&mut d, EditOp::AddClip { track: None, clip: text_clip(0.0, 2.0) }));
    let tr = track_of(d.project(), a);
    let b = clip_out(apply(&mut d, EditOp::AddClip { track: Some(tr), clip: text_clip(3.0, 2.0) }));
    let snap = d.project().clone();
    for (clip, start, dur) in [(a, 0.0, 4.0), (b, 1.0, 2.0), (a, 2.5, 1.0)] {
        let op = EditOp::SetClipTiming { clip, start: s(start), duration: s(dur), source_in: Time::ZERO };
        assert_eq!(d.apply(op, OpSource::Ui), Err(CoreError::InvalidDuration), "{start} +{dur}");
        assert_eq!(d.project(), &snap);
    }
    // up to the neighbour works
    apply(&mut d, EditOp::SetClipTiming { clip: a, start: s(0.0), duration: s(3.0), source_in: Time::ZERO });
    check(d.project(), "set time");
}

/// Bug: after speed or trim the fades were longer than the clip. As when setting
/// (SetFades), at most half the length each.
#[test]
fn shortening_clamps_fades_like_set_fades() {
    let mut d = Document::new(Project::new("F"));
    let a = clip_out(apply(&mut d, EditOp::AddClip { track: None, clip: text_clip(0.0, 8.0) }));
    let half = |d: &Document| {
        let c = d.project().find_clip(a).unwrap().1;
        let h = Time(c.duration.0 / 2);
        assert!(c.fade_in <= h && c.fade_out <= h, "{:?}+{:?} at {:?}", c.fade_in, c.fade_out, c.duration);
        assert!(c.fade_in > Time::ZERO && c.fade_out > Time::ZERO, "not entirely gone");
    };
    let fades = || EditOp::SetFades { clip: a, fade_in: s(4.0), fade_out: s(4.0) };
    apply(&mut d, fades());
    apply(&mut d, EditOp::SetSpeed { clip: a, speed: 4.0 });
    half(&d);
    apply(&mut d, EditOp::SetSpeed { clip: a, speed: 1.0 });
    apply(&mut d, fades());
    apply(&mut d, EditOp::TrimClip { clip: a, edge: Edge::End, to: s(3.0) });
    half(&d);
    apply(&mut d, fades());
    apply(&mut d, EditOp::TrimClip { clip: a, edge: Edge::Start, to: s(2.0) });
    half(&d);
    apply(&mut d, EditOp::SetClipTiming { clip: a, start: s(2.0), duration: s(0.5), source_in: Time::ZERO });
    half(&d);
}

/// Bug: slowing down on the main track without magnet covered the next clip.
#[test]
fn slowing_down_on_main_without_magnet_pushes_the_next_clip() {
    let r = rich();
    let mut d = r.doc;
    let mut st = d.project().settings.clone();
    st.main_track_magnet = false;
    apply(&mut d, EditOp::SetSettings { settings: st });
    apply(&mut d, EditOp::SetSpeed { clip: r.v1, speed: 0.5 });
    check(d.project(), "speed");
    assert_eq!(d.project().find_clip(r.v2).unwrap().1.start, s(8.0));
}

/// Bug: turning on the magnet pushed together the clips of a locked main track.
#[test]
fn enabling_magnet_leaves_locked_main_track_alone() {
    let r = rich();
    let mut d = r.doc;
    let main = track_of(d.project(), r.v1);
    let mut st = d.project().settings.clone();
    st.main_track_magnet = false;
    apply(&mut d, EditOp::SetSettings { settings: st.clone() });
    apply(&mut d, EditOp::MoveClip { clip: r.img, track: Some(main), start: s(20.0) });
    apply(&mut d, EditOp::SetTrackFlags { track: main, locked: true, hidden: false, muted: false });
    let before = d.project().find_clip(r.img).unwrap().1.start;
    st.main_track_magnet = true;
    apply(&mut d, EditOp::SetSettings { settings: st });
    assert_eq!(d.project().find_clip(r.img).unwrap().1.start, before);
}

/// Bug: AddClip accepted negative start times, duplicate IDs and times at the i64 end.
#[test]
fn add_clip_sanitizes_start_id_and_overflow() {
    let mut d = Document::new(Project::new("T"));
    let mut c = text_clip(-5.0, 2.0);
    c.source_in = s(-1.0);
    let a = clip_out(apply(&mut d, EditOp::AddClip { track: None, clip: c.clone() }));
    let p = d.project();
    assert_eq!(p.find_clip(a).unwrap().1.start, Time::ZERO);
    assert_eq!(p.find_clip(a).unwrap().1.source_in, Time::ZERO);
    // the same op again (same clip ID): second clip with its own ID
    let b = clip_out(apply(&mut d, EditOp::AddClip { track: None, clip: c }));
    assert_ne!(a, b);
    check(d.project(), "duplicate");
    let snap = d.project().clone();
    for (start, dur) in [(i64::MAX, 1), (i64::MAX - 10, 100), (1, i64::MAX), (Time::LIMIT.0 + 1, 1), (0, Time::LIMIT.0 + 1)] {
        let mut c = text_clip(0.0, 1.0);
        c.start = Time(start);
        c.duration = Time(dur);
        assert_eq!(d.apply(EditOp::AddClip { track: None, clip: c }, OpSource::Ui), Err(CoreError::InvalidDuration));
        assert_eq!(d.project(), &snap);
    }
    let op = EditOp::SetClipTiming { clip: a, start: Time(i64::MAX), duration: Time(5), source_in: Time::ZERO };
    assert_eq!(d.apply(op, OpSource::Ui), Err(CoreError::InvalidDuration));
    // largest allowed position works, and end/duration of the timeline are right
    let mut c = text_clip(0.0, 1.0);
    c.start = Time::LIMIT;
    c.duration = Time::LIMIT;
    let m = clip_out(apply(&mut d, EditOp::AddClip { track: None, clip: c }));
    assert_eq!(d.project().find_clip(m).unwrap().1.end(), Time(2 * Time::LIMIT.0));
    assert_eq!(d.project().duration(), Time(2 * Time::LIMIT.0));
    // trimming to the i64 end is pulled to the limit
    apply(&mut d, EditOp::TrimClip { clip: a, edge: Edge::End, to: Time(i64::MAX) });
    assert!(d.project().find_clip(a).unwrap().1.end() <= Time::LIMIT);
    // magnet insert with huge times doesn't overflow
    let r = rich();
    let mut d = r.doc;
    let mut c = media_clip(&r.video, 0.0, 1.0);
    c.start = Time::LIMIT;
    apply(&mut d, EditOp::AddClip { track: None, clip: c });
    check(d.project(), "huge");
}

/// Bug: NaN/infinity (e.g. 1e300 via MCP → f32 = inf) made the project file unreadable.
#[test]
fn non_finite_numbers_are_rejected() {
    let r = rich();
    let mut d = r.doc;
    let snap = d.project().clone();
    let tf = Transform { x: f32::INFINITY, ..Default::default() };
    let mut k = Keyframes::default();
    k.set(Prop::Opacity, Time::ZERO, f32::NAN);
    let mut fx = ClipFx::default();
    fx.adjust.brightness = f32::NEG_INFINITY;
    for op in [
        EditOp::SetTransform { clip: r.v1, transform: tf },
        EditOp::SetKeyframes { clip: r.v1, keyframes: k },
        EditOp::SetFx { clip: r.v1, fx },
        EditOp::SetSpeed { clip: r.v1, speed: f32::NAN },
        EditOp::SetCrop { clip: r.v1, crop: Crop { top: f32::NAN, ..Default::default() } },
        EditOp::Batch { label: String::new(), ops: vec![EditOp::SetOpacity { clip: r.v1, opacity: f32::NAN }] },
    ] {
        let name = format!("{op:?}");
        assert_eq!(d.apply(op, OpSource::Ui), Err(CoreError::InvalidValue), "{name}");
        assert_eq!(d.project(), &snap);
    }
    // via JSON (MCP, plugins): 1e300 does not fit in f32
    let op: EditOp = serde_json::from_str(&format!(
        r#"{{"op":"set_transform","clip":"{}","transform":{{"x":1e300,"y":0,"scale":1,"rotation_deg":0}}}}"#,
        r.v1
    ))
    .unwrap();
    assert!(d.apply(op, OpSource::Mcp("x".into())).is_err());
    assert_eq!(roundtrip(d.project()), *d.project());
}

/// Crop: limits (at least 2 % left), saved without field when empty, old projects load.
#[test]
fn crop_is_clamped_and_only_saved_when_set() {
    let r = rich();
    let mut d = r.doc;
    // without crop the field is not in the file; files without the field load with an empty crop
    assert!(!d.project().to_json().contains("\"crop\""));
    let old = Project::from_json(&d.project().to_json()).unwrap();
    assert!(old.find_clip(r.v1).unwrap().1.crop.is_none());
    let get = |d: &Document| d.project().find_clip(r.v1).unwrap().1.crop;
    apply(&mut d, EditOp::SetCrop { clip: r.v1, crop: Crop { left: -1.0, right: 0.3, top: 2.0, bottom: 0.0 } });
    let c = get(&d);
    assert_eq!((c.left, c.right, c.bottom), (0.0, 0.3, 0.0));
    assert!((c.top - 0.98).abs() < 1e-6, "{c:?}");
    apply(&mut d, EditOp::SetCrop { clip: r.v1, crop: Crop { left: 0.9, right: 0.9, top: 0.49, bottom: 0.49 } });
    let c = get(&d);
    assert!((c.left + c.right - 0.98).abs() < 1e-6 && (c.left - c.right).abs() < 1e-6, "{c:?}");
    assert_eq!((c.top, c.bottom), (0.49, 0.49));
    // with crop: saved and loaded losslessly; clips from outside are clamped too
    assert!(d.project().to_json().contains("\"crop\""));
    assert_eq!(roundtrip(d.project()), *d.project());
    let mut c = text_clip(20.0, 1.0);
    c.crop.left = 5.0;
    let id = clip_out(apply(&mut d, EditOp::AddClip { track: None, clip: c }));
    assert!((d.project().find_clip(id).unwrap().1.crop.left - 0.98).abs() < 1e-6);
}

#[test]
fn one_tick_clips_and_degenerate_values() {
    let mut d = Document::new(Project::new("T"));
    let mut c = text_clip(0.0, 1.0);
    c.duration = Time(1);
    let id = clip_out(apply(&mut d, EditOp::AddClip { track: None, clip: c }));
    // not splittable, not trimmable to 0
    assert_eq!(d.apply(EditOp::SplitClip { clip: id, at: Time(0) }, OpSource::Ui), Err(CoreError::InvalidSplit));
    assert_eq!(d.apply(EditOp::SplitClip { clip: id, at: Time(1) }, OpSource::Ui), Err(CoreError::InvalidSplit));
    assert_eq!(
        d.apply(EditOp::TrimClip { clip: id, edge: Edge::End, to: Time(0) }, OpSource::Ui),
        Err(CoreError::InvalidDuration)
    );
    assert_eq!(
        d.apply(EditOp::TrimClip { clip: id, edge: Edge::Start, to: Time(1) }, OpSource::Ui),
        Err(CoreError::InvalidDuration)
    );
    // speed extremes are clamped, length stays positive
    for sp in [0.0, -3.0, 1e9, f32::MIN_POSITIVE] {
        apply(&mut d, EditOp::SetSpeed { clip: id, speed: sp });
        let c = d.project().find_clip(id).unwrap().1;
        assert!((0.1..=100.0).contains(&c.speed) && c.duration > Time::ZERO, "{sp}");
    }
    // opacity/volume are clamped, fades to half the length
    apply(&mut d, EditOp::SetOpacity { clip: id, opacity: 7.0 });
    apply(&mut d, EditOp::SetVolume { clip: id, volume_db: -1e9 });
    apply(&mut d, EditOp::SetFades { clip: id, fade_in: s(-1.0), fade_out: s(99.0) });
    let c = d.project().find_clip(id).unwrap().1;
    assert_eq!((c.opacity, c.volume_db), (1.0, -60.0));
    assert_eq!(c.fade_in, Time::ZERO);
    assert!(c.fade_out <= Time(c.duration.0 / 2));
    // transition duration clamped
    apply(&mut d, EditOp::SetTransition { clip: id, transition: Some(TransitionRef { id: String::new(), duration: Time(-5) }) });
    assert_eq!(d.project().find_clip(id).unwrap().1.transition_out.as_ref().unwrap().duration, s(0.1));
    // ripple range reversed or before 0
    let tr = track_of(d.project(), id);
    for (a, b) in [(2.0, 1.0), (1.0, 1.0), (-1.0, 1.0)] {
        assert!(d.apply(EditOp::RippleRange { track: tr, from: s(a), to: s(b), all_tracks: false }, OpSource::Ui).is_err());
    }
    check(d.project(), "extremes");
}

#[test]
fn names_empty_long_emoji_and_rtl_survive_save() {
    let r = rich();
    let mut d = r.doc;
    let long: String = "Ä🎬".repeat(50_000);
    let tr = track_of(d.project(), r.a1);
    for name in ["", "   ", long.as_str(), "مرحبا بالعالم", "\u{202E}gnp.exe", "a\nb\tc\"\\"] {
        apply(&mut d, EditOp::Rename { name: name.into() });
        assert_eq!(d.project().name, name);
        apply(&mut d, EditOp::RenameTrack { track: tr, name: Some(name.into()) });
        let t = d.project().timeline.tracks.iter().find(|t| t.id == tr).unwrap();
        assert_eq!(t.name.as_deref(), Some(name.trim()).filter(|n| !n.is_empty()));
        let style = TextStyle { text: name.into(), ..Default::default() };
        apply(&mut d, EditOp::SetText { clip: r.t1, style });
        assert_eq!(d.project().find_clip(r.t1).unwrap().1.label(d.project()), name);
        assert_eq!(roundtrip(d.project()), *d.project());
    }
}

#[test]
fn ten_thousand_clips_stay_consistent() {
    let mut p = Project::new("Groß");
    let m = media(MediaKind::Video, 3600.0);
    p.media.push(m.clone());
    let mut main = Track::new(TrackKind::Video);
    main.main = true;
    let mut text = Track::new(TrackKind::Text);
    for i in 0..10_000 {
        let mut c = media_clip(&m, i as f64, 1.0);
        c.source_in = s((i % 3000) as f64);
        main.clips.push(c);
        text.clips.push(text_clip(i as f64 * 2.0, 1.5));
    }
    p.timeline.tracks = vec![main, text];
    let mut d = Document::new(p);
    let mid = d.project().timeline.tracks[0].clips[5000].id;
    apply(&mut d, EditOp::SplitClip { clip: mid, at: s(5000.5) });
    apply(&mut d, EditOp::RemoveClip { clip: mid });
    let tr = d.project().timeline.tracks[1].id;
    apply(&mut d, EditOp::RippleRange { track: tr, from: s(100.0), to: s(200.0), all_tracks: true });
    apply(&mut d, EditOp::CloseGaps { track: None });
    check(d.project(), "10,000 clips");
    assert_eq!(roundtrip(d.project()), *d.project());
    assert_eq!(d.step(-4), 4);
    assert_eq!(d.project().timeline.tracks[0].clips.len(), 10_000);
}

#[test]
fn ops_on_project_with_zero_length_clip_and_missing_media_do_not_panic() {
    // such things only come from broken/foreign files, but must not crash
    let mut p = Project::new("Kaputt");
    let mut main = Track::new(TrackKind::Video);
    main.main = true;
    let ghost = media(MediaKind::Video, 5.0); // not in the project
    let mut zero = media_clip(&ghost, 0.0, 1.0);
    zero.duration = Time::ZERO;
    let normal = media_clip(&ghost, 0.0, 2.0);
    let (z, n) = (zero.id, normal.id);
    main.clips = vec![zero, normal];
    p.timeline.tracks.push(main);
    let mut d = Document::new(p);
    let c = d.project().find_clip(z).unwrap().1.clone();
    assert_eq!(c.label(d.project()), "Medium fehlt");
    let _ = c.source_time(s(1.0));
    let _ = c.eval_at(Time::ZERO, 1920.0, 1080.0);
    let _ = c.beat_times();
    for op in [
        EditOp::SplitClip { clip: z, at: Time::ZERO },
        EditOp::TrimClip { clip: z, edge: Edge::End, to: s(1.0) },
        EditOp::TrimClip { clip: n, edge: Edge::Start, to: s(0.5) },
        EditOp::SetSpeed { clip: z, speed: 2.0 },
        EditOp::SetFades { clip: z, fade_in: s(1.0), fade_out: s(1.0) },
        EditOp::SplitClip { clip: n, at: s(1.0) },
        EditOp::CloseGaps { track: None },
        EditOp::MoveClip { clip: n, track: None, start: s(3.0) },
    ] {
        let _ = d.apply(op, OpSource::Ui);
    }
    let _ = roundtrip(d.project());
}

/// Collapsed tracks are stored in the project (formerly only window state: open again after reopening);
/// projects without the field load as "not collapsed", and non-collapsed ones don't write it.
#[test]
fn collapsed_track_survives_save_and_old_projects_load() {
    let mut t = Track::new(TrackKind::Video);
    let open = serde_json::to_string(&t).unwrap();
    assert!(!open.contains("collapsed"), "{open}");
    assert!(!serde_json::from_str::<Track>(&open).unwrap().collapsed);
    assert!(!open.contains("height"), "{open}");
    assert_eq!(serde_json::from_str::<Track>(&open).unwrap().height, 1.0);
    t.collapsed = true;
    t.height = 2.2;
    let back: Track = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
    assert!(back.collapsed && back.height == 2.2);
}

/// Nonsensical heights (from outside via MCP) are clamped instead of yielding an invisible track
#[test]
fn track_height_is_limited() {
    let mut d = Document::new(Project::new("x"));
    let Ok(OpOutput::Track(t)) = d.apply(EditOp::AddTrack { kind: TrackKind::Video }, OpSource::Ui) else { panic!() };
    assert!(d.apply(EditOp::SetTrackView { track: t, collapsed: false, height: f32::NAN }, OpSource::Ui).is_err());
    for (given, want) in [(0.0, 0.5), (99.0, 3.0), (1.5, 1.5)] {
        d.apply(EditOp::SetTrackView { track: t, collapsed: false, height: given }, OpSource::Ui).unwrap();
        assert_eq!(d.project().timeline.tracks.iter().find(|x| x.id == t).unwrap().height, want);
    }
}
