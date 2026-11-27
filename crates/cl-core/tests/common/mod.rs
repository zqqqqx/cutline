//! Shared helpers of the core tests: test project, random numbers, invariants.
#![allow(dead_code)]

use cl_core::*;
use std::collections::HashSet;
use std::path::PathBuf;

pub fn s(x: f64) -> Time {
    Time::from_secs_f64(x)
}

/// Small linear congruential generator (fixed seeds, no dependency).
pub struct Lcg(pub u64);
impl Lcg {
    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    pub fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
    /// Random number in [a, b)
    pub fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (self.next() % 1_000_000) as f64 / 1_000_000.0 * (b - a)
    }
}

pub fn media(kind: MediaKind, dur: f64) -> MediaRef {
    MediaRef {
        id: MediaId::new(),
        path: PathBuf::from(format!("{kind:?}.mp4")),
        kind,
        duration: s(dur),
        width: 1920,
        height: 1080,
        fps: Some(Rational::new(30, 1)),
        has_audio: kind != MediaKind::Image,
        audio_streams: (kind != MediaKind::Image) as u32,
        video_codec: None,
        audio_codec: None,
        has_alpha: false,
    }
}

pub fn media_clip(m: &MediaRef, start: f64, dur: f64) -> Clip {
    Clip::new(ClipContent::Media { media: m.id, kind: m.kind }, s(start), s(dur))
}

pub fn text_clip(start: f64, dur: f64) -> Clip {
    Clip::new(ClipContent::Text(TextStyle::default()), s(start), s(dur))
}

pub fn apply(d: &mut Document, op: EditOp) -> OpOutput {
    let name = format!("{op:?}");
    d.apply(op, OpSource::Ui).unwrap_or_else(|e| panic!("{e}: {name}"))
}

pub fn clip_out(o: OpOutput) -> ClipId {
    match o {
        OpOutput::Clip(c) => c,
        o => panic!("no clip: {o:?}"),
    }
}

/// Project with all clip kinds: main track (2 videos + image), audio, text, effect, filter, markers.
pub struct Rich {
    pub doc: Document,
    pub video: MediaRef,
    pub audio: MediaRef,
    pub image: MediaRef,
    pub v1: ClipId,
    pub v2: ClipId,
    pub img: ClipId,
    pub a1: ClipId,
    pub t1: ClipId,
    pub t2: ClipId,
    pub fx: ClipId,
}

pub fn rich() -> Rich {
    let mut doc = Document::new(Project::new("Reich"));
    doc.max_history = usize::MAX;
    let (video, audio, image) = (
        media(MediaKind::Video, 10.0),
        media(MediaKind::Audio, 8.0),
        media(MediaKind::Image, 0.0),
    );
    for m in [&video, &audio, &image] {
        apply(&mut doc, EditOp::ImportMedia { media: m.clone() });
    }
    let add = |doc: &mut Document, c: Clip| clip_out(apply(doc, EditOp::AddClip { track: None, clip: c }));
    let v1 = add(&mut doc, media_clip(&video, 0.0, 4.0));
    let mut c2 = media_clip(&video, 4.0, 5.0);
    c2.source_in = s(2.0);
    let v2 = add(&mut doc, c2);
    let img = add(&mut doc, media_clip(&image, 9.0, 3.0));
    let a1 = add(&mut doc, media_clip(&audio, 1.0, 6.0));
    let t1 = add(&mut doc, text_clip(0.5, 2.0));
    let t2 = add(&mut doc, text_clip(4.0, 3.0));
    let fx = add(
        &mut doc,
        Clip::new(ClipContent::Effect(FxRef::new("blur")), s(2.0), s(2.0)),
    );
    add(&mut doc, Clip::new(ClipContent::Filter(FxRef::new("warm")), s(0.0), s(5.0)));
    let markers = [1.0, 6.0, 11.0]
        .iter()
        .enumerate()
        .map(|(i, t)| Marker {
            id: i as u64,
            at: s(*t),
            duration: Time::ZERO,
            name: format!("M{i}"),
            note: String::new(),
            color: i as u8,
        })
        .collect();
    apply(&mut doc, EditOp::SetMarkers { markers });
    Rich { doc, video, audio, image, v1, v2, img, a1, t1, t2, fx }
}

/// Invariants of a valid project. `ctx` appears in the failure message.
pub fn check(p: &Project, ctx: &str) {
    let mut clip_ids = HashSet::new();
    let mut track_ids = HashSet::new();
    assert!(p.timeline.tracks.iter().filter(|t| t.main).count() <= 1, "{ctx}: mehrere Hauptspuren");
    for t in &p.timeline.tracks {
        assert!(track_ids.insert(t.id), "{ctx}: track ID duplicated");
        if t.main {
            assert_eq!(t.kind, TrackKind::Video, "{ctx}: main track is not a visual track");
        }
        for c in &t.clips {
            assert!(clip_ids.insert(c.id), "{ctx}: Clip ID {} duplicated", c.id);
            assert_eq!(c.kind(), t.kind, "{ctx}: clip kind does not match the track");
            assert!(c.start >= Time::ZERO, "{ctx}: negativer Start {:?}", c.start);
            assert!(c.duration > Time::ZERO, "{ctx}: length {:?}", c.duration);
            assert!(c.source_in >= Time::ZERO, "{ctx}: negativer Quellbeginn {:?}", c.source_in);
            for v in [c.speed, c.opacity, c.volume_db, c.transform.x, c.transform.y, c.transform.scale] {
                assert!(v.is_finite(), "{ctx}: number not finite");
            }
            assert!((0.0..=1.0).contains(&c.opacity), "{ctx}: Deckkraft {}", c.opacity);
            let k = c.crop;
            assert!(k.left.min(k.right).min(k.top).min(k.bottom) >= 0.0, "{ctx}: Zuschnitt {k:?}");
            assert!(k.left + k.right <= 0.981 && k.top + k.bottom <= 0.981, "{ctx}: Zuschnitt {k:?}");
            if let ClipContent::Media { media, .. } = &c.content {
                assert!(p.media(*media).is_some(), "{ctx}: clip without media");
            }
        }
        for w in t.clips.windows(2) {
            assert!(
                w[0].end() <= w[1].start,
                "{ctx}: overlap on {:?} track (main={}): {:?}..{:?} / {:?}..{:?}",
                t.kind,
                t.main,
                w[0].start,
                w[0].end(),
                w[1].start,
                w[1].end()
            );
        }
    }
    assert!(p.timeline.markers.windows(2).all(|w| w[0].at <= w[1].at), "{ctx}: Marker unsortiert");
}

/// Save and load. On failure (seen about once in 2000 runs, cause unknown) it collects evidence:
/// the lines around the error, whether a second parse of the same text fails too, whether saving
/// again gives the same text, and the text itself in a file in the temp folder.
pub fn roundtrip(p: &Project) -> Project {
    let json = p.to_json();
    let e = match Project::from_json(&json) {
        Ok(q) => return q,
        Err(e) => e,
    };
    let line = e.line().max(1);
    let around: String = json
        .lines()
        .enumerate()
        .skip(line.saturating_sub(6))
        .take(11)
        .map(|(i, l)| format!("{:>8}{} {l}\n", i + 1, if i + 1 == line { ">" } else { " " }))
        .collect();
    let again = Project::from_json(&json).err().map(|e| e.to_string());
    let same_text = p.to_json() == json;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let file = std::env::temp_dir().join(format!("cutline-roundtrip-{}-{nanos}.json", std::process::id()));
    let saved = std::fs::write(&file, &json).map(|_| file.display().to_string());
    panic!(
        "saved project loads again: {e}\nlines around {line}:\n{around}second parse of the same text: {again:?}\n\
         saving again gives the same text: {same_text}\ntext saved to: {saved:?}"
    );
}
