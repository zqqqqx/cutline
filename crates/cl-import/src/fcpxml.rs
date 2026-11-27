//! FCPXML (Final Cut Pro 10.4+, DaVinci Resolve): assets, main track (`spine`) and
//! connected clips (`lane`). Times are fractions like `3003/30000s`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cl_core::{Clip, ClipContent, MediaKind, MediaRef, Project, Time, Track, TrackKind};

use crate::{Report, Result};

/// `1001/30000s`, `5s`, `0s` → seconds
pub fn parse_time(s: &str) -> f64 {
    let s = s.trim().trim_end_matches('s');
    let v = match s.split_once('/') {
        Some((n, d)) => n.parse::<f64>().unwrap_or(0.0) / d.parse::<f64>().unwrap_or(1.0).max(1e-9),
        None => s.parse().unwrap_or(0.0),
    };
    // "NaN", "inf", "1e999" from broken files
    if v.is_finite() { v } else { 0.0 }
}

fn hex_byte(h: &[u8]) -> Option<u8> {
    u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()
}

fn url_to_path(src: &str, dir: &Path) -> PathBuf {
    let s = src
        .strip_prefix("file://localhost")
        .or_else(|| src.strip_prefix("file://"))
        .unwrap_or(src);
    // decode %20 etc.
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            // decode to bytes: a &str slice in the middle of "€" would crash
            if let Some(v) = hex_byte(&b[i + 1..i + 3]) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    let p = String::from_utf8_lossy(&out).into_owned();
    // Windows: /C:/… → C:/…
    let p = if p.len() > 3 && p.as_bytes()[0] == b'/' && p.as_bytes()[2] == b':' {
        p[1..].to_string()
    } else {
        p
    };
    // relative references (e.g. in .fcpxmld packages) apply from the file's folder
    let p = PathBuf::from(p);
    if p.is_relative() && !p.has_root() {
        dir.join(p)
    } else {
        p
    }
}

pub fn import(path: &Path) -> Result<(Project, Report)> {
    import_with(path, &crate::Progress::default())
}

/// Like [`import`], with progress and cancel.
pub fn import_with(path: &Path, progress: &crate::Progress) -> Result<(Project, Report)> {
    progress.set(crate::Step::Read, 0, 1, &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "FCPXML".into());
    convert_with(&text, &name, path.parent().unwrap_or(Path::new("")), progress)
}

/// `dir`: folder of the FCPXML file (for relative media paths).
pub fn convert(text: &str, name: &str, dir: &Path) -> Result<(Project, Report)> {
    convert_with(text, name, dir, &crate::Progress::default())
}

fn convert_with(text: &str, name: &str, dir: &Path, progress: &crate::Progress) -> Result<(Project, Report)> {
    let doc = roxmltree::Document::parse(text).map_err(|e| format!("FCPXML not readable: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "fcpxml" {
        return Err("Not an FCPXML file".into());
    }
    let mut rep = Report::default();
    let mut p = Project::new(name);
    // formats (size, frame rate)
    let mut formats: HashMap<String, (u32, u32, f64)> = HashMap::new();
    for f in root.descendants().filter(|n| n.has_tag_name("format")) {
        let id = f.attribute("id").unwrap_or("").to_string();
        let w = f
            .attribute("width")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1920);
        let h = f
            .attribute("height")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1080);
        let fd = f
            .attribute("frameDuration")
            .map(parse_time)
            .unwrap_or(1.0 / 30.0);
        formats.insert(id, (w, h, fd));
    }
    // assets
    let mut assets: HashMap<String, Option<MediaRef>> = HashMap::new();
    let n = root.descendants().filter(|n| n.has_tag_name("asset")).count();
    progress.set(crate::Step::Probe, 0, n, "");
    for a in root.descendants().filter(|n| n.has_tag_name("asset")) {
        progress.check()?;
        let id = a.attribute("id").unwrap_or("").to_string();
        let src = a.attribute("src").map(String::from).or_else(|| {
            a.children()
                .find(|c| c.has_tag_name("media-rep"))
                .and_then(|m| m.attribute("src"))
                .map(String::from)
        });
        let m = src.map(|s| url_to_path(&s, dir)).and_then(|path| {
            if !path.is_file() {
                rep.missing.push(path.clone());
                let dur = a.attribute("duration").map(parse_time);
                let kind = match (a.attribute("hasVideo"), dur) {
                    (Some("0"), _) => MediaKind::Audio,
                    // still images have duration 0s
                    (_, Some(d)) if d <= 0.0 => MediaKind::Image,
                    _ => MediaKind::Video,
                };
                let dur = dur.unwrap_or(0.0).max(0.0);
                let (w, h, _) = a.attribute("format").and_then(|f| formats.get(f)).copied().unwrap_or((0, 0, 0.0));
                let m = crate::placeholder(path, kind, Time::from_secs_f64(dur), w, h);
                p.media.push(m.clone());
                return Some(m);
            }
            let m = cl_media::probe(&path).ok()?;
            p.media.push(m.clone());
            Some(m)
        });
        progress.tick(&id);
        assets.insert(id, m);
    }
    let seq = root
        .descendants()
        .find(|n| n.has_tag_name("sequence"))
        .ok_or("No sequence found")?;
    if let Some((w, h, fd)) = seq.attribute("format").and_then(|f| formats.get(f)) {
        p.settings.width = *w & !1;
        p.settings.height = *h & !1;
        // bug: frameDuration 0 gave 4 billion fps; unusable values → default
        if let Some(fps) = crate::fps_from(1.0 / fd) {
            p.settings.fps = fps;
        }
        p.settings.aspect = cl_core::AspectPreset::Custom;
    }
    let spine = seq
        .descendants()
        .find(|n| n.has_tag_name("spine"))
        .ok_or("No main track (spine) found")?;
    // lane → track
    let mut lanes: std::collections::BTreeMap<i32, Track> = Default::default();
    let mut main = Track::new(TrackKind::Video);
    main.main = true;
    let clip_tags = ["asset-clip", "clip", "video", "audio", "ref-clip"];
    for item in spine.children().filter(|n| n.is_element()) {
        let tag = item.tag_name().name();
        let offset = item.attribute("offset").map(parse_time).unwrap_or(0.0);
        let start_in = item.attribute("start").map(parse_time).unwrap_or(0.0);
        if tag == "gap" || clip_tags.contains(&tag) {
            if clip_tags.contains(&tag) {
                if let Some(c) = make_clip(item, offset, &assets, &mut rep) {
                    main.clips.push(c);
                }
            }
            // connected clips: time relative to the parent clip
            for child in item
                .children()
                .filter(|n| n.is_element() && clip_tags.contains(&n.tag_name().name()))
            {
                let lane: i32 = child
                    .attribute("lane")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1);
                let off = child.attribute("offset").map(parse_time).unwrap_or(0.0);
                let abs = offset + (off - start_in);
                if let Some(c) = make_clip(child, abs, &assets, &mut rep) {
                    let kind = if matches!(
                        c.content,
                        ClipContent::Media {
                            kind: MediaKind::Audio,
                            ..
                        }
                    ) {
                        TrackKind::Audio
                    } else {
                        TrackKind::Video
                    };
                    lanes
                        .entry(lane)
                        .or_insert_with(|| Track::new(kind))
                        .clips
                        .push(c);
                }
            }
        } else if tag == "title" {
            rep.skip("Title");
        } else if tag == "transition" {
            rep.skip("Transitions");
        }
    }
    p.timeline.tracks.push(main);
    // positive lanes above, negative below the main track (for us: audio at the end)
    let mut rest: Vec<Track> = lanes.into_values().collect();
    rest.sort_by_key(|t| t.kind == TrackKind::Audio);
    p.timeline.tracks.extend(rest);
    for t in &mut p.timeline.tracks {
        t.clips.sort_by_key(|c| c.start);
    }
    p.timeline.tracks.retain(|t| !t.clips.is_empty() || t.main);
    if rep.clips == 0 {
        return Err("No transferable clips found in the FCPXML".into());
    }
    Ok((p, rep))
}

fn make_clip(
    n: roxmltree::Node,
    at: f64,
    assets: &HashMap<String, Option<MediaRef>>,
    rep: &mut Report,
) -> Option<Clip> {
    let r = n
        .attribute("ref")
        .or_else(|| n.descendants().find_map(|d| d.attribute("ref")))?;
    let m = assets.get(r)?.as_ref()?;
    let dur = n
        .attribute("duration")
        .map(parse_time)
        .unwrap_or(m.duration.as_secs_f64());
    let start = n.attribute("start").map(parse_time).unwrap_or(0.0);
    let mut c = Clip::new(
        ClipContent::Media {
            media: m.id,
            kind: m.kind,
        },
        Time::from_secs_f64(at.max(0.0)),
        Time::from_secs_f64(dur.max(0.04)),
    );
    c.source_in = Time::from_secs_f64(start);
    rep.clips += 1;
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding_survives_multibyte() {
        let d = Path::new("");
        assert_eq!(url_to_path("file:///a%1€b", d), PathBuf::from("/a%1€b"));
        assert_eq!(url_to_path("file:///a%20b", d), PathBuf::from("/a b"));
    }

    /// Missing file: the clip stays with a placeholder media (relinkable), `missing` names it.
    #[test]
    fn missing_asset_keeps_clip() {
        let xml = r#"<?xml version="1.0"?><fcpxml version="1.9"><resources><format id="r1" frameDuration="1/25s" width="1280" height="720"/>
            <asset id="r2" src="file:///does/not/exist.mov" duration="6s" hasVideo="1" hasAudio="1" format="r1"/></resources>
            <library><event><project name="P"><sequence format="r1"><spine>
              <asset-clip ref="r2" offset="0s" start="1s" duration="2s"/>
            </spine></sequence></project></event></library></fcpxml>"#;
        let (p, rep) = convert(xml, "P", Path::new("")).unwrap();
        assert_eq!(rep.missing, [PathBuf::from("/does/not/exist.mov")]);
        assert_eq!((p.media.len(), rep.clips, p.timeline.tracks[0].clips.len()), (1, 1, 1));
        let m = &p.media[0];
        assert_eq!((m.kind, m.width, m.height, m.duration), (MediaKind::Video, 1280, 720, Time::from_secs_f64(6.0)));
    }

    #[test]
    fn parses_times_and_spine() {
        // single time values: `fcpxml_times_and_formats` in tests/robust.rs
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("a b.mp4");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=320x180:r=25:d=6",
                "-pix_fmt",
                "yuv420p"
            ])
            .arg(&v)
            .status()
            .unwrap()
            .success());
        let url = format!(
            "file:///{}",
            v.to_string_lossy().replace('\\', "/").replace(' ', "%20")
        );
        let xml = format!(
            r#"<?xml version="1.0"?><fcpxml version="1.9"><resources><format id="r1" frameDuration="1/25s" width="1280" height="720"/><asset id="r2" src="{url}" duration="6s"/></resources>
            <library><event><project name="P"><sequence format="r1"><spine>
              <asset-clip ref="r2" offset="0s" start="1s" duration="2s"><asset-clip ref="r2" lane="1" offset="1.5s" start="0s" duration="1s"/></asset-clip>
              <gap offset="2s" duration="1s"/>
              <asset-clip ref="r2" offset="3s" start="0s" duration="2s"/>
              <transition offset="2.5s" duration="1s"/>
            </spine></sequence></project></event></library></fcpxml>"#
        );
        let (p, rep) = convert(&xml, "P", Path::new("")).unwrap();
        assert_eq!((p.settings.width, p.settings.height), (1280, 720));
        assert_eq!(rep.clips, 3);
        assert_eq!(p.timeline.tracks[0].clips.len(), 2);
        assert_eq!(
            p.timeline.tracks[0].clips[1].start,
            Time::from_secs_f64(3.0)
        );
        // connected clip: 0 + (1.5 - 1) = 0.5 s
        assert_eq!(
            p.timeline.tracks[1].clips[0].start,
            Time::from_secs_f64(0.5)
        );
        assert!(rep.skipped.iter().any(|(w, _)| w == "Transitions"));
    }
}
