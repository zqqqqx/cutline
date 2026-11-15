//! Gemeinsame Helfer der Kern-Tests: Testprojekt, Zufallszahlen, Invarianten.
#![allow(dead_code)]

use cl_core::*;
use std::collections::HashSet;
use std::path::PathBuf;

pub fn s(x: f64) -> Time {
    Time::from_secs_f64(x)
}

/// Kleiner linearer Kongruenzgenerator (feste Seeds, keine Abhängigkeit).
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
    /// Zufallszahl in [a, b)
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
        video_codec: None,
        audio_codec: None,
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
        o => panic!("kein Clip: {o:?}"),
    }
}

/// Projekt mit allen Clip-Arten: Hauptspur (2 Videos + Bild), Ton, Text, Effekt, Filter, Marker.
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

/// Invarianten eines gültigen Projekts. `ctx` erscheint in der Fehlermeldung.
pub fn check(p: &Project, ctx: &str) {
    let mut clip_ids = HashSet::new();
    let mut track_ids = HashSet::new();
    assert!(p.timeline.tracks.iter().filter(|t| t.main).count() <= 1, "{ctx}: mehrere Hauptspuren");
    for t in &p.timeline.tracks {
        assert!(track_ids.insert(t.id), "{ctx}: Spur-ID doppelt");
        if t.main {
            assert_eq!(t.kind, TrackKind::Video, "{ctx}: Hauptspur ist keine Bildspur");
        }
        for c in &t.clips {
            assert!(clip_ids.insert(c.id), "{ctx}: Clip-ID {} doppelt", c.id);
            assert_eq!(c.kind(), t.kind, "{ctx}: Clip-Art passt nicht zur Spur");
            assert!(c.start >= Time::ZERO, "{ctx}: negativer Start {:?}", c.start);
            assert!(c.duration > Time::ZERO, "{ctx}: Länge {:?}", c.duration);
            assert!(c.source_in >= Time::ZERO, "{ctx}: negativer Quellbeginn {:?}", c.source_in);
            for v in [c.speed, c.opacity, c.volume_db, c.transform.x, c.transform.y, c.transform.scale] {
                assert!(v.is_finite(), "{ctx}: Zahl nicht endlich");
            }
            assert!((0.0..=1.0).contains(&c.opacity), "{ctx}: Deckkraft {}", c.opacity);
            if let ClipContent::Media { media, .. } = &c.content {
                assert!(p.media(*media).is_some(), "{ctx}: Clip ohne Medium");
            }
        }
        for w in t.clips.windows(2) {
            assert!(
                w[0].end() <= w[1].start,
                "{ctx}: Überlappung auf {:?}-Spur (main={}): {:?}..{:?} / {:?}..{:?}",
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

pub fn roundtrip(p: &Project) -> Project {
    Project::from_json(&p.to_json()).expect("gespeichertes Projekt lädt wieder")
}
