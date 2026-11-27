//! Broken, incomplete and foreign import files: no crash, understandable error.

use cl_core::{ClipContent, Rational, Time, TrackKind};
use cl_import::{capcut, fcpxml};
use serde_json::json;
use std::path::{Path, PathBuf};

fn make_video(dir: &Path, name: &str) -> Option<PathBuf> {
    if !cl_media::test_tools() {
        return None;
    }
    let v = dir.join(name);
    let ok = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=160x90:r=25:d=2", "-threads", "2", "-pix_fmt", "yuv420p"])
        .arg(&v)
        .status()
        .unwrap()
        .success();
    ok.then_some(v)
}

fn text_draft(extra: serde_json::Value) -> serde_json::Value {
    let content = json!({"text": "Hallo 🎬 مرحبا", "styles": [{"size": 15.0}]}).to_string();
    let mut d = json!({
        "canvas_config": {"width": 1920, "height": 1080},
        "materials": {"texts": [{"id": "T1", "content": content}]},
        "tracks": [{"type": "text", "segments": [{"material_id": "T1", "target_timerange": {"start": 0, "duration": 1000000}}]}]
    });
    for (k, v) in extra.as_object().unwrap() {
        d[k] = v.clone();
    }
    d
}

#[test]
fn capcut_garbage_structures_give_errors() {
    let dir = Path::new(".");
    for d in [json!(null), json!([]), json!("x"), json!(5), json!({}), json!({"tracks": 5, "materials": []}),
        json!({"tracks": [{"type": "video", "segments": [{"material_id": 5}]}]}),
        json!({"tracks": [{"type": "text", "segments": "x"}], "materials": {"texts": "x"}})] {
        let e = capcut::convert(&d, dir, "x").unwrap_err();
        assert!(e.contains("No transferable clips"), "{d}: {e}");
    }
}

#[test]
fn capcut_odd_values_are_tamed() {
    let dir = Path::new(".");
    // bug: frame rate 0/absurd became 1 or 4 billion fps
    for (fps, want) in [(json!(0), Rational::new(30, 1)), (json!(-25), Rational::new(30, 1)), (json!(1e12), Rational::new(30, 1)),
        (json!("30"), Rational::new(30, 1)), (json!(29.97), Rational::new(30000, 1001)), (json!(23.976), Rational::new(24000, 1001)), (json!(25), Rational::new(25, 1))] {
        let (p, _) = capcut::convert(&text_draft(json!({"fps": fps})), dir, "x").unwrap();
        assert_eq!(p.settings.fps, want, "{fps}");
    }
    // huge/negative canvas
    for (w, h) in [(json!(0), json!(0)), (json!(-5), json!(1e12)), (json!("x"), json!(null))] {
        let (p, _) = capcut::convert(&text_draft(json!({"canvas_config": {"width": w, "height": h}})), dir, "x").unwrap();
        assert!(p.settings.width >= 2 && p.settings.height >= 2);
    }
    // bug: negative times were adopted (clip before 0)
    let mut d = text_draft(json!({}));
    d["tracks"][0]["segments"][0]["target_timerange"] = json!({"start": -5000000, "duration": -3});
    d["tracks"][0]["segments"][0]["speed"] = json!(1e9);
    d["tracks"][0]["segments"][0]["volume"] = json!(-1);
    d["tracks"][0]["segments"][0]["clip"] = json!({"alpha": "x", "scale": {"x": -2.0}, "transform": {"x": "y"}});
    let (p, rep) = capcut::convert(&d, dir, "x").unwrap();
    let c = &p.timeline.tracks[0].clips[0];
    assert_eq!(c.start, Time::ZERO);
    assert!(c.duration > Time::ZERO);
    assert_eq!((c.speed, c.volume_db), (cl_core::MAX_SPEED, -60.0));
    let ClipContent::Text(st) = &c.content else { panic!() };
    assert_eq!(st.text, "Hallo 🎬 مرحبا");
    assert!((8.0..=600.0).contains(&st.size));
    assert_eq!(rep.texts, 1);
    // missing media are reported, texts stay
    let mut d = text_draft(json!({}));
    d["materials"]["videos"] = json!([{"id": "V1", "path": "C:/does/not/exist/Ä 🎬.mp4"}, {"id": "V2", "path": ""}]);
    d["tracks"].as_array_mut().unwrap().push(json!({"type": "video", "segments": [
        {"material_id": "V1", "target_timerange": {"start": 0, "duration": 1}},
        {"material_id": "V2", "target_timerange": {"start": 0, "duration": 1}},
        {"material_id": "missing", "target_timerange": {"start": 0, "duration": 1}}]}));
    let (_, rep) = capcut::convert(&d, dir, "x").unwrap();
    assert_eq!(rep.missing, vec![PathBuf::from("C:/does/not/exist/Ä 🎬.mp4")]);
    assert!(rep.skipped.iter().any(|(w, _)| w == "Online media"));
}

#[test]
fn capcut_files_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    assert!(capcut::import(&dir.path().join("missing")).is_err());
    assert!(capcut::import(dir.path()).unwrap_err().contains("draft_content.json"), "folder without draft");
    let f = dir.path().join("draft_content.json");
    for junk in [&b""[..], b"\xff\xfe\x00", b"{\"tracks\": [", b"verschl\xc3\xbcsselt"] {
        std::fs::write(&f, junk).unwrap();
        assert!(capcut::import(dir.path()).is_err());
    }
    // name from draft_meta_info.json (even if it is broken)
    std::fs::write(&f, text_draft(json!({})).to_string()).unwrap();
    std::fs::write(dir.path().join("draft_meta_info.json"), "{broken").unwrap();
    let (p, _) = cl_import::import(&f).unwrap();
    assert_eq!(p.name, dir.path().file_name().unwrap().to_string_lossy());
    std::fs::write(dir.path().join("draft_meta_info.json"), json!({"draft_name": "My Ä 🎬"}).to_string()).unwrap();
    assert_eq!(cl_import::import(dir.path()).unwrap().0.name, "My Ä 🎬");
    assert!(cl_import::import(&dir.path().join("x.mov")).unwrap_err().contains("Unknown"));
    // material in the draft folder with umlauts via the placeholder
    let Some(v) = make_video(dir.path(), "Ä ö 🎬.mp4") else { return };
    let mut d = text_draft(json!({}));
    d["materials"]["videos"] = json!([{"id": "V1", "path": "##_draftpath_placeholder_X_##\\Ä ö 🎬.mp4"}]);
    d["tracks"].as_array_mut().unwrap().push(json!({"type": "video", "segments": [{"material_id": "V1", "target_timerange": {"start": 0, "duration": 1000000}}]}));
    let (p, rep) = capcut::convert(&d, dir.path(), "x").unwrap();
    assert!(rep.missing.is_empty(), "{:?}", rep.missing);
    assert_eq!(p.media[0].path, v);
    assert!(p.timeline.tracks.iter().any(|t| t.main && t.kind == TrackKind::Video));
}

#[test]
fn fcpxml_garbage_gives_errors() {
    let d = Path::new("");
    for (xml, msg) in [
        ("", "not readable"),
        ("<fcpxml", "not readable"),
        ("<x/>", "Not an FCPXML"),
        ("<fcpxml/>", "No sequence"),
        ("<fcpxml><sequence/></fcpxml>", "spine"),
        ("<fcpxml><sequence><spine><asset-clip ref='r9' duration='NaN'/></spine></sequence></fcpxml>", "No transferable"),
    ] {
        let e = fcpxml::convert(xml, "x", d).unwrap_err();
        assert!(e.contains(msg), "{xml}: {e}");
    }
    // missing file: the clip stays (placeholder for relinking)
    let (p, rep) = fcpxml::convert("<fcpxml><resources><asset id='r1' src='file:///C:/missing/Ä%20b.mov'/></resources><sequence><spine><asset-clip ref='r1'/></spine></sequence></fcpxml>", "x", d).unwrap();
    assert_eq!((rep.missing.len(), p.timeline.tracks[0].clips.len()), (1, 1));
}

#[test]
fn fcpxml_times_and_formats() {
    for (s, v) in [("3003/30000s", 0.1001), ("5s", 5.0), ("0s", 0.0), ("", 0.0), ("abc", 0.0), ("-5s", -5.0), ("1/0s", 1e9), ("10/s", 10.0)] {
        assert!((fcpxml::parse_time(s) - v).abs() < 1e-6, "{s}");
    }
    // bug: NaN/inf from the file became times at the i64 end
    for s in ["NaNs", "infs", "1e999s", "inf/1s", "1/infs"] {
        assert!(fcpxml::parse_time(s).is_finite(), "{s}");
    }
    let dir = tempfile::tempdir().unwrap();
    let Some(v) = make_video(dir.path(), "Ä b.mp4") else { return };
    let xml = |fd: &str, src: &str| {
        format!(r#"<fcpxml version="1.9"><resources><format id="f" frameDuration="{fd}" width="99999999999" height="-3"/><asset id="a" src="{src}"/></resources>
        <library><event><project><sequence format="f"><spine><asset-clip ref="a" offset="infs" start="NaNs" duration="1s"/></spine></sequence></project></event></library></fcpxml>"#)
    };
    let url = format!("file:///{}", v.to_string_lossy().replace('\\', "/").replace(' ', "%20"));
    // bug: frameDuration 0 gave 4 billion fps
    for (fd, want) in [("0s", Rational::new(30, 1)), ("1/0s", Rational::new(30, 1)), ("-1/25s", Rational::new(30, 1)), ("1001/24000s", Rational::new(24000, 1001)), ("1/50s", Rational::new(50, 1))] {
        let (p, _) = fcpxml::convert(&xml(fd, &url), "x", Path::new("")).unwrap();
        assert_eq!(p.settings.fps, want, "{fd}");
        let c = &p.timeline.tracks[0].clips[0];
        assert!(c.start == Time::ZERO && c.source_in == Time::ZERO, "{c:?}");
    }
    // bug: relative media paths were looked up from the working directory instead of from the file
    let f = dir.path().join("Project.fcpxml");
    std::fs::write(&f, xml("1/25s", "%C3%84%20b.mp4")).unwrap();
    let (p, rep) = fcpxml::import(&f).unwrap();
    assert!(rep.missing.is_empty(), "{:?}", rep.missing);
    assert_eq!(p.media[0].path, v);
}
