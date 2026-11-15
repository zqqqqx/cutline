//! Import in Schritten: Fortschritt, Abbruch in jedem Schritt, gesammelte Fehler,
//! verschachtelte Entwürfe, sehr große Entwürfe.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use cl_core::{ClipContent, Time, TrackKind};
use cl_import::{capcut, Progress, Status, Step};
use serde_json::{json, Value};

const S: i64 = 1_000_000;

fn video(dir: &Path, name: &str) -> Option<PathBuf> {
    if !cl_media::ffmpeg_available() {
        return None;
    }
    let v = dir.join(name);
    cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=64x36:r=25:d=3", "-pix_fmt", "yuv420p", "-threads", "1"])
        .arg(&v)
        .status()
        .unwrap()
        .success()
        .then_some(v)
}

fn seg(mid: &str, start: i64, dur: i64, src: i64) -> Value {
    json!({"material_id": mid, "target_timerange": {"start": start, "duration": dur}, "source_timerange": {"start": src, "duration": dur}})
}

fn text_mat(id: &str, t: &str) -> Value {
    json!({"id": id, "content": json!({"text": t, "styles": [{"size": 10.0}]}).to_string()})
}

/// Entwurf mit Medien (vorhanden, fehlend, unlesbar), Texten und Effekten
fn draft(dir: &Path, v: &Path) -> Value {
    let junk = dir.join("kaputt.mp4");
    std::fs::write(&junk, b"kein video").unwrap();
    let mut vids = vec![json!({"id": "V", "path": v.to_string_lossy()}), json!({"id": "J", "path": junk.to_string_lossy()})];
    let mut segs = vec![seg("V", 0, S, 0), seg("J", S, S, 0)];
    for i in 0..20 {
        let id = format!("X{i}");
        vids.push(json!({"id": id, "path": dir.join(format!("fehlt{i}.mp4")).to_string_lossy()}));
        segs.push(seg(&id, (2 + i) * S, S, 0));
    }
    json!({
        "materials": {"videos": vids, "texts": [text_mat("T", "Hallo")], "video_effects": [{"id": "E"}]},
        "tracks": [
            {"type": "video", "segments": segs},
            {"type": "text", "segments": [seg("T", 0, S, 0)]},
            {"type": "effect", "segments": [seg("E", 0, S, 0)]}
        ]
    })
}

/// Fortschritt mit Aufzeichnung; `cancel_at`: in diesem Schritt abbrechen
fn recorder(cancel_at: Option<Step>) -> (Arc<Progress>, Arc<Mutex<Vec<Status>>>) {
    let log: Arc<Mutex<Vec<Status>>> = Arc::default();
    let slot: Arc<OnceLock<std::sync::Weak<Progress>>> = Arc::default();
    let (l, sl) = (log.clone(), slot.clone());
    let p = Arc::new(Progress::new(move || {
        let Some(p) = sl.get().and_then(|w| w.upgrade()) else { return };
        let st = p.status();
        if Some(st.step) == cancel_at {
            p.cancel();
        }
        l.lock().unwrap().push(st);
    }));
    slot.set(Arc::downgrade(&p)).unwrap();
    (p, log)
}

#[test]
fn every_step_reports_monotonic_progress_and_errors_are_collected() {
    let dir = tempfile::tempdir().unwrap();
    let Some(v) = video(dir.path(), "v.mp4") else { return };
    let (p, log) = recorder(None);
    let (proj, rep) = capcut::convert_with(&draft(dir.path(), &v), dir.path(), "x", &p).unwrap();
    let log = log.lock().unwrap();
    for s in [Step::Find, Step::Probe, Step::Timeline] {
        assert!(log.iter().any(|x| x.step == s && x.done == x.total), "{s:?} meldet kein Ende: {log:?}");
    }
    let f: Vec<f32> = log.iter().map(|s| s.fraction()).collect();
    assert!(f.windows(2).all(|w| w[0] <= w[1] + 1e-6), "{f:?}");
    assert!(f.iter().all(|x| (0.0..=1.0).contains(x)));
    // eine kaputte und 20 fehlende Dateien halten nichts auf
    assert_eq!(rep.failed, vec![dir.path().join("kaputt.mp4")]);
    assert_eq!(rep.missing.len(), 20);
    assert_eq!((rep.clips, rep.texts), (1, 1));
    assert_eq!(proj.media.len(), 1);
}

#[test]
fn cancel_in_any_step_stops_with_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let Some(v) = video(dir.path(), "v.mp4") else { return };
    let d = draft(dir.path(), &v);
    std::fs::write(dir.path().join("draft_content.json"), d.to_string()).unwrap();
    for step in [Step::Read, Step::Find, Step::Probe, Step::Timeline] {
        let (p, _) = recorder(Some(step));
        let r = cl_import::import_with(dir.path(), &p);
        assert_eq!(r.unwrap_err(), cl_import::CANCELLED, "{step:?}");
    }
    // schon vor dem Start abgebrochen
    let p = Progress::default();
    p.cancel();
    assert_eq!(capcut::convert_with(&d, dir.path(), "x", &p).unwrap_err(), cl_import::CANCELLED);
}

#[test]
fn probe_all_keeps_order_and_limits_workers() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let paths: Vec<PathBuf> = (0..40).map(|i| PathBuf::from(format!("{i}"))).collect();
    let (now, peak) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let p = Progress::default();
    let out = cl_import::probe_all(&paths, 3, &p, |path| {
        let n = now.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(n, Ordering::SeqCst);
        std::thread::yield_now();
        now.fetch_sub(1, Ordering::SeqCst);
        if path == Path::new("7") {
            Err("kaputt".into())
        } else {
            Err(path.to_string_lossy().into_owned())
        }
    })
    .unwrap();
    assert!(peak.load(Ordering::SeqCst) <= 3);
    assert_eq!(p.status().done, 40);
    for (i, r) in out.iter().enumerate() {
        let want = if i == 7 { "kaputt".to_string() } else { i.to_string() };
        assert_eq!(r.as_ref().unwrap_err(), &want);
    }
    // abgebrochen: Fehler statt halber Liste
    let p = Progress::default();
    p.cancel();
    assert!(cl_import::probe_all(&paths, 3, &p, |_| Err(String::new())).is_err());
}

#[test]
fn nested_drafts_are_resolved_level_by_level() {
    // innen: Text 0–4 s; außen: zusammengesetzter Clip zeigt innen 1–3 s bei 10 s
    let inner = json!({
        "materials": {"texts": [text_mat("IT", "innen")]},
        "tracks": [{"type": "text", "segments": [seg("IT", 0, 4 * S, 0)]}]
    });
    // zweite Stufe: Entwurf im Entwurf
    let outer_inner = json!({
        "materials": {"drafts": [{"id": "D2", "type": "combination", "draft": inner}]},
        "tracks": [{"type": "video", "segments": [seg("D2", 0, 4 * S, 0)]}]
    });
    let mut fast = seg("D1", 20 * S, S, 0);
    fast["speed"] = json!(2.0);
    let d = json!({
        "materials": {
            "texts": [text_mat("T", "außen")],
            "drafts": [{"id": "D1", "type": "combination", "draft": outer_inner}, {"id": "D0", "type": "combination"}]
        },
        "tracks": [
            {"type": "text", "segments": [seg("T", 0, S, 0)]},
            {"type": "video", "segments": [seg("D1", 10 * S, 2 * S, S), fast, seg("D0", 30 * S, S, 0)]}
        ]
    });
    let (p, rep) = capcut::convert(&d, Path::new("."), "x").unwrap();
    assert_eq!(rep.nested, 2, "{rep:?}");
    assert_eq!(rep.texts, 2);
    let n = |w: &str| rep.skipped.iter().find(|(k, _)| k == w).map_or(0, |x| x.1);
    assert_eq!(n("Zusammengesetzte Clips"), 1, "ohne inneren Entwurf");
    assert_eq!(n("Zusammengesetzte Clips mit Tempo"), 1);
    let inner: Vec<_> = p
        .timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Text)
        .flat_map(|t| &t.clips)
        .filter(|c| matches!(&c.content, ClipContent::Text(s) if s.text == "innen"))
        .collect();
    assert_eq!(inner.len(), 1);
    assert_eq!((inner[0].start, inner[0].duration), (Time(10 * S), Time(2 * S)));
}

#[test]
fn place_trims_to_the_window() {
    use cl_core::{Clip, Track};
    let mut t = Track::new(TrackKind::Video);
    let st = || ClipContent::Text(Default::default());
    t.clips.push(Clip::new(st(), Time(0), Time(4 * S)));
    t.clips.push(Clip::new(st(), Time(5 * S), Time(S)));
    let mut c = Clip::new(st(), Time(2 * S), Time(2 * S));
    c.speed = 2.0;
    t.clips.push(c);
    // innen 1–3 s bei außen 10 s
    let out = capcut::place(t, Time(10 * S), Time(2 * S), Time(S));
    let v: Vec<_> = out.clips.iter().map(|c| (c.start.0 / 1000, c.duration.0 / 1000, c.source_in.0 / 1000)).collect();
    assert_eq!(v, [(10_000, 2_000, 1_000), (11_000, 1_000, 0)]);
}

#[test]
fn very_large_draft_is_converted_quickly() {
    let mut texts = Vec::new();
    let mut tracks = Vec::new();
    for t in 0..12 {
        let mut segs = Vec::new();
        for i in 0..250 {
            let id = format!("T{t}-{i}");
            texts.push(text_mat(&id, &id));
            segs.push(seg(&id, i * S, S, 0));
        }
        tracks.push(json!({"type": "text", "segments": segs}));
    }
    let d = json!({"materials": {"texts": texts}, "tracks": tracks});
    let t = std::time::Instant::now();
    let (p, rep) = capcut::convert(&d, Path::new("."), "x").unwrap();
    assert_eq!(rep.texts, 3000);
    assert_eq!(p.timeline.tracks.iter().map(|t| t.clips.len()).sum::<usize>(), 3000);
    assert!(t.elapsed().as_secs_f64() < 10.0, "{:?}", t.elapsed());
}

/// Mess-Test: großer Testentwurf (F:/cutline-testmedien/import, `mkdraft.py`) mit Zeiten je Schritt.
/// `cargo test --release -p cl-import --test staged -- --ignored --nocapture`
#[test]
#[ignore]
fn measure_big_draft_steps() {
    let dir = std::env::var_os("CUTLINE_IMPORT_MEDIA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("F:/cutline-testmedien/import"))
        .join("grosser_entwurf");
    if !dir.is_dir() {
        return;
    }
    let last: Arc<Mutex<(Step, std::time::Instant, f64)>> = Arc::new(Mutex::new((Step::Read, std::time::Instant::now(), 0.0)));
    let t0 = std::time::Instant::now();
    let slot: Arc<OnceLock<std::sync::Weak<Progress>>> = Arc::default();
    let (l, sl) = (last.clone(), slot.clone());
    let p = Arc::new(Progress::new(move || {
        let Some(p) = sl.get().and_then(|w| w.upgrade()) else { return };
        let st = p.status();
        let mut l = l.lock().unwrap();
        // längste Zeit ohne Fortschrittsmeldung
        l.2 = l.2.max(l.1.elapsed().as_secs_f64());
        if st.step != l.0 {
            println!("{:?} fertig nach {:.2} s", l.0, t0.elapsed().as_secs_f64());
            l.0 = st.step;
        }
        l.1 = std::time::Instant::now();
    }));
    slot.set(Arc::downgrade(&p)).unwrap();
    let (proj, rep) = cl_import::import_with(&dir, &p).unwrap();
    println!(
        "gesamt {:.1} s, längste Zeit ohne Meldung {:.2} s; {} Medien, {} Clips, {} Texte, fehlend {}, verschachtelt {}, übersprungen {:?}",
        t0.elapsed().as_secs_f64(),
        last.lock().unwrap().2,
        proj.media.len(),
        rep.clips,
        rep.texts,
        rep.missing.len(),
        rep.nested,
        rep.skipped
    );
}
