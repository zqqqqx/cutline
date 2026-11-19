//! Zensur-Flächen: vereinfachte Bahn überdeckt den verfolgten Rahmen in jedem Bild, auch nach
//! Teilen, Trimmen, Tempo und Rückwärts; alte Projekte bleiben unverändert.

mod common;
use cl_core::*;
use common::*;

/// Verfolgte Rahmen alle 1/15 s über `dur` Sekunden nach der Bahn `f(t) -> (cx, cy, w, h)`.
fn samples(dur: f64, f: impl Fn(f64) -> (f32, f32, f32, f32)) -> Vec<CensorKey> {
    (0..=(dur * 15.0) as usize)
        .map(|i| {
            let t = i as f64 / 15.0;
            let (cx, cy, w, h) = f(t);
            CensorKey { t: s(t), cx, cy, w, h }
        })
        .collect()
}

/// Wahrer Rahmen zwischen den Abtastzeiten: linear (so liefert ihn die Verfolgung)
fn truth(boxes: &[CensorKey], t: Time) -> CensorKey {
    Censor { path: boxes.to_vec(), ..Default::default() }.at(t)
}

#[test]
fn simplified_path_covers_every_frame() {
    let paths: Vec<(&str, Vec<CensorKey>)> = vec![
        ("gerade", samples(4.0, |t| (0.1 + 0.2 * t as f32, 0.5, 0.1, 0.12))),
        ("kurve", samples(4.0, |t| (0.5 + 0.3 * (t as f32 * 2.0).cos(), 0.5 + 0.3 * (t as f32 * 3.0).sin(), 0.1, 0.1))),
        ("stillstand", samples(6.0, |_| (0.4, 0.6, 0.2, 0.25))),
        ("sprung", samples(3.0, |t| if t < 1.5 { (0.2, 0.3, 0.1, 0.1) } else { (0.8, 0.7, 0.1, 0.1) })),
        // verloren: die Verfolgung hält die letzte Lage
        ("verlust", samples(4.0, |t| { let t = t.min(1.2) as f32; (0.3 + 0.2 * t, 0.5, 0.1, 0.1) })),
        ("größer", samples(3.0, |t| (0.5, 0.5, 0.05 + 0.1 * t as f32, 0.06 + 0.08 * t as f32))),
    ];
    for (name, boxes) in paths {
        let keys = censor_simplify(&boxes);
        let cs = Censor { path: keys.clone(), ..Default::default() };
        // jedes Bild bei 60 Bildern/s
        let end = boxes.last().unwrap().t;
        let mut t = Time::ZERO;
        while t <= end {
            let inner = truth(&boxes, t);
            assert!(cs.at(t).covers(&inner), "{name} bei {:.3} s: {:?} deckt {:?} nicht", t.as_secs_f64(), cs.at(t), inner);
            t = t + s(1.0 / 60.0);
        }
        // mit Rand: an den Abtastzeiten mindestens CENSOR_KEEP
        for b in &boxes {
            assert!(cs.at(b.t).covers(&b.grown(CENSOR_KEEP)), "{name}: Rand fehlt bei {:?}", b.t);
        }
        assert!(keys.len() < boxes.len(), "{name}: nicht vereinfacht");
        eprintln!("{name}: {} Rahmen → {} Stützpunkte", boxes.len(), keys.len());
        if name == "stillstand" {
            assert!(keys.len() <= 4, "{name}: {}", keys.len());
        }
        if name == "gerade" {
            assert!(keys.len() <= 3, "{name}: {}", keys.len());
        }
    }
    assert!(censor_simplify(&[]).is_empty());
}

/// Objekt in der Quelle (10 s Video): wandert und wird größer.
fn object(src: Time) -> CensorKey {
    let t = src.as_secs_f64() as f32;
    CensorKey { t: src, cx: 0.1 + 0.08 * t, cy: 0.3 + 0.03 * t, w: 0.1 + 0.005 * t, h: 0.12 }
}

/// Liegt die Fläche (Bahn in Clipzeit wie für die Filterkette) in jedem Bild des Clips auf dem Objekt?
fn assert_on_object(c: &Clip, what: &str) {
    let cs = &c.censor_in_clip_time(None)[0];
    let mut t = c.start;
    while t < c.end() {
        let rel = t - c.start;
        let obj = object(c.source_time(t));
        assert!(cs.at(rel).covers(&obj), "{what} bei {:.3} s: {:?} deckt {:?} nicht", t.as_secs_f64(), cs.at(rel), obj);
        t = t + s(1.0 / 30.0);
    }
    // Einzelbild wie die Vorschau
    let mid = Time(c.duration.0 / 2);
    let one = &c.censor_in_clip_time(Some(mid))[0];
    assert_eq!(one.path.len(), 1);
    assert!(one.path[0].covers(&object(c.source_time(c.start + mid))), "{what}: Einzelbild");
}

#[test]
fn censor_stays_on_object_after_split_trim_speed_and_reverse() {
    let r = rich();
    let mut d = r.doc;
    let boxes: Vec<CensorKey> = (0..=150).map(|i| object(s(i as f64 / 15.0))).collect();
    let mut c = d.project().find_clip(r.v2).unwrap().1.clone();
    c.fx.censor = vec![Censor { path: censor_simplify(&boxes), ..Default::default() }];
    apply(&mut d, EditOp::ReplaceClip { clip: c });
    let get = |d: &Document, id| d.project().find_clip(id).unwrap().1.clone();
    assert_on_object(&get(&d, r.v2), "unverändert");
    let right = match apply(&mut d, EditOp::SplitClip { clip: r.v2, at: s(6.0) }) {
        OpOutput::Clip(id) => id,
        o => panic!("{o:?}"),
    };
    assert_on_object(&get(&d, r.v2), "geteilt links");
    assert_on_object(&get(&d, right), "geteilt rechts");
    apply(&mut d, EditOp::TrimClip { clip: right, edge: Edge::Start, to: s(6.7) });
    assert_on_object(&get(&d, right), "vorn getrimmt");
    apply(&mut d, EditOp::SetSpeed { clip: right, speed: 2.0 });
    assert_on_object(&get(&d, right), "Tempo 2");
    apply(&mut d, EditOp::SetSpeed { clip: r.v2, speed: 0.5 });
    assert_on_object(&get(&d, r.v2), "Tempo 0,5");
    let mut c = get(&d, r.v2);
    c.fx.reverse = true;
    assert_on_object(&c, "rückwärts");
    c.fx.reverse = false;
    c.fx.speed_curve = Some("montage".into());
    assert_on_object(&c, "Tempo-Kurve");
    // gespeichert und verlustfrei geladen
    assert!(d.project().to_json().contains("\"censor\""));
    assert_eq!(roundtrip(d.project()), *d.project());
}

/// Ohne Zensur steht das Feld nicht in der Datei; alte Dateien laden ohne Zensur.
#[test]
fn projects_without_censor_are_unchanged() {
    let r = rich();
    let json = r.doc.project().to_json();
    assert!(!json.contains("censor"));
    let p = Project::from_json(&json).unwrap();
    assert!(p.find_clip(r.v1).unwrap().1.fx.censor.is_empty());
    assert_eq!(p.to_json(), json);
}
