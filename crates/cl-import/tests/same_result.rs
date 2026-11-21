//! The import in steps (background, progress, parallel probing) yields the same project
//! as the earlier one-piece path: comparison with stored results
//! (`tests/snapshots`, rewrite with `CUTLINE_SNAPSHOT_WRITE=1`).

use std::path::{Path, PathBuf};

use cl_import::capcut;
use serde_json::{json, Value};

fn ffmpeg(args: &[&str], out: &Path) -> bool {
    cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .is_ok_and(|s| s.success())
}

/// Tiny media: video with audio, image, audio (in the draft folder and next to it)
fn media(dir: &Path) -> Option<[PathBuf; 3]> {
    if !cl_media::test_tools() {
        return None;
    }
    let v = dir.join("clip.mp4");
    let b = dir.join("bild.png");
    let rec = dir.join("audio_record");
    std::fs::create_dir_all(&rec).unwrap();
    let a = rec.join("vo.wav");
    let ok = ffmpeg(&["-f", "lavfi", "-i", "testsrc2=s=160x90:r=25:d=2", "-f", "lavfi", "-i", "sine=d=2", "-shortest", "-pix_fmt", "yuv420p", "-threads", "2"], &v)
        && ffmpeg(&["-f", "lavfi", "-i", "testsrc2=s=64x36:r=1:d=1", "-frames:v", "1"], &b)
        && ffmpeg(&["-f", "lavfi", "-i", "sine=f=300:d=3"], &a);
    ok.then_some([v, b, a])
}

fn seg(mid: &str, start: i64, dur: i64, extra: Value) -> Value {
    let mut s = json!({"material_id": mid, "target_timerange": {"start": start, "duration": dur}, "source_timerange": {"start": start / 3, "duration": dur}});
    for (k, v) in extra.as_object().unwrap() {
        s[k] = v.clone();
    }
    s
}

/// Draft with everything the importer knows: tracks of all kinds, layers, texts with font,
/// outline and alignment, animations, stickers, effects, filters, online material, missing
/// files, media used twice, overlapping audio tracks.
fn fixture(dir: &Path, [v, b, _]: &[PathBuf; 3]) -> Value {
    let font = dir.join("Poppins-ExtraBold.ttf");
    std::fs::write(&font, include_bytes!("../../cl-app/assets/fonts/Poppins-ExtraBold.ttf")).unwrap();
    let text = |t: &str, size: f64| {
        json!({"text": t, "styles": [{"size": size, "bold": true, "fill": {"content": {"solid": {"color": [1.0, 0.5, 0.0]}}},
            "strokes": [{"width": 0.08, "content": {"solid": {"color": [0.0, 0.0, 1.0]}}}], "font": {"path": "C:/x/SystemFont/en.ttf"}}]})
        .to_string()
    };
    let s = 1_000_000;
    let clip = json!({"clip": {"alpha": 0.7, "scale": {"x": 1.3, "y": 1.3}, "rotation": 15.0, "transform": {"x": -0.25, "y": 0.4}, "flip": {"horizontal": false, "vertical": true}}});
    let mut main = Vec::new();
    for i in 0..12 {
        let m = ["V1", "B1", "V1", "VX"][i % 4];
        let extra = if i % 3 == 0 { json!({"extra_material_refs": ["AN"], "speed": 1.5, "volume": 0.25}) } else { clip.clone() };
        // gap after clip 5 (magnet off)
        main.push(seg(m, i as i64 * s + if i > 5 { s / 2 } else { 0 }, s, extra));
    }
    json!({
        "canvas_config": {"width": 1280, "height": 720},
        "fps": 25.0,
        "materials": {
            "videos": [
                {"id": "V1", "type": "video", "path": v.to_string_lossy()},
                {"id": "B1", "type": "photo", "path": b.to_string_lossy()},
                {"id": "VX", "type": "video", "path": dir.join("fehlt.mp4").to_string_lossy()},
                {"id": "VO", "type": "video", "path": ""}
            ],
            "audios": [
                {"id": "A1", "type": "record", "path": "##_draftpath_placeholder_ABC_##/audio_record/vo.wav"},
                {"id": "A2", "type": "music", "path": ""},
                {"id": "AX", "type": "music", "path": dir.join("fehlt.wav").to_string_lossy()}
            ],
            "texts": [
                {"id": "T1", "content": text("Eins", 12.0), "alignment": 0, "has_shadow": true, "font_path": font.to_string_lossy()},
                {"id": "T2", "content": text("Zwei", 6.0), "alignment": 2},
                {"id": "T3", "base_content": "Drei", "font_size": 9.0}
            ],
            "stickers": [{"id": "S1"}],
            "video_effects": [{"id": "E1"}],
            "material_animations": [{"id": "AN", "animations": [{"type": "in", "duration": 200000}, {"type": "out", "duration": 2000000}, {"type": "loop", "duration": 1}]}],
            "speeds": [{"id": "SP"}],
            "beats": [{"id": "BT"}]
        },
        "tracks": [
            {"type": "audio", "segments": [seg("A1", 0, 2 * s, json!({"volume": 0.0})), seg("AX", 3 * s, s, json!({}))]},
            {"type": "video", "segments": main},
            {"type": "text", "segments": [seg("T1", 0, s, json!({"render_index": 14002})), seg("T3", 2 * s, s, clip.clone())]},
            {"type": "video", "segments": [seg("V1", s, 2 * s, json!({"render_index": 3, "reverse": true, "visible": false})), seg("VO", 4 * s, s, json!({}))]},
            {"type": "audio", "segments": [seg("A1", 5 * s, s, json!({})), seg("A2", 0, s, json!({}))]},
            {"type": "audio", "segments": [seg("A1", s, s, json!({}))]},
            {"type": "text", "segments": [seg("T2", 0, s, json!({"render_index": 14001}))]},
            {"type": "sticker", "segments": [seg("S1", 0, s, json!({"render_index": 2}))]},
            {"type": "effect", "segments": [seg("E1", 0, s, json!({})), seg("E1", s, s, json!({}))]},
            {"type": "filter", "segments": [seg("E1", 0, s, json!({}))]},
            {"type": "adjust", "segments": [seg("E1", 0, s, json!({}))]},
            {"type": "video", "segments": [seg("SP", 0, s, json!({})), seg("BT", 0, s, json!({})), seg("unbekannt", 0, s, json!({}))]}
        ]
    })
}

/// Replace ids (random per run) by their order, the test folder by `<dir>`.
fn normalize(v: &mut Value, dir: &str, ids: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            let uuid = s.len() == 36 && s.chars().filter(|c| *c == '-').count() == 4;
            if uuid {
                let n = ids.iter().position(|i| i == s).unwrap_or_else(|| {
                    ids.push(s.clone());
                    ids.len() - 1
                });
                *s = format!("#{n}");
            } else {
                *s = s.replace(dir, "<dir>").replace('\\', "/");
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| normalize(x, dir, ids)),
        Value::Object(o) => o.values_mut().for_each(|x| normalize(x, dir, ids)),
        _ => {}
    }
}

fn snapshot(name: &str, dir: &Path, p: &cl_core::Project, rep: &cl_import::Report) {
    let mut v = json!({
        "project": serde_json::from_str::<Value>(&p.to_json()).unwrap(),
        "report": {"clips": rep.clips, "texts": rep.texts, "skipped": rep.skipped, "missing": rep.missing, "fonts": rep.fonts}
    });
    normalize(&mut v, &dir.to_string_lossy(), &mut Vec::new());
    // paths in short form (Windows: paths without `\\?\`, `/`) so the file is the same everywhere
    let got = serde_json::to_string_pretty(&v).unwrap();
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(format!("{name}.json"));
    if std::env::var_os("CUTLINE_SNAPSHOT_WRITE").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&file).unwrap().replace("\r\n", "\n");
    assert_eq!(got, want, "{name}: Ergebnis weicht vom gespeicherten ab");
}

#[test]
fn staged_import_gives_the_same_project() {
    let dir = tempfile::tempdir().unwrap();
    let Some(m) = media(dir.path()) else { return };
    let d = fixture(dir.path(), &m);
    let (p, rep) = capcut::convert(&d, dir.path(), "Fixture").unwrap();
    snapshot("capcut_fixture", dir.path(), &p, &rep);
    // via file and folder (name from draft_meta_info.json)
    std::fs::write(dir.path().join("draft_content.json"), d.to_string()).unwrap();
    std::fs::write(dir.path().join("draft_meta_info.json"), json!({"draft_name": "Aus Datei"}).to_string()).unwrap();
    let (p, rep) = cl_import::import(dir.path()).unwrap();
    snapshot("capcut_dir", dir.path(), &p, &rep);
}
