//! CapCut-Desktop-Entwürfe (`draft_content.json`, Zeiten in Mikrosekunden).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cl_core::{AspectPreset, Clip, ClipContent, MediaKind, MediaRef, Project, Rational, TextStyle, Time, Track, TrackKind};
use serde_json::Value;

use crate::{Report, Result};

/// Ein lokaler Entwurf für die Auswahlliste.
#[derive(Debug, Clone)]
pub struct Draft {
    pub name: String,
    pub dir: PathBuf,
    pub modified: Option<SystemTime>,
    pub cover: Option<PathBuf>,
}

/// Bekannte Ablageorte der Entwürfe (Windows/macOS).
pub fn draft_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        let l = PathBuf::from(l);
        v.push(l.join("CapCut").join("User Data").join("Projects").join("com.lveditor.draft"));
        v.push(l.join("JianyingPro").join("User Data").join("Projects").join("com.lveditor.draft"));
    }
    if let Some(h) = std::env::var_os("HOME") {
        v.push(PathBuf::from(h).join("Movies").join("CapCut").join("User Data").join("Projects").join("com.lveditor.draft"));
    }
    v.into_iter().filter(|p| p.is_dir()).collect()
}

/// Alle lokalen Entwürfe, neueste zuerst.
pub fn list_drafts() -> Vec<Draft> {
    let mut out = Vec::new();
    for root in draft_roots() {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let dir = e.path();
            let json = dir.join("draft_content.json");
            if !json.is_file() {
                continue;
            }
            let meta: Option<Value> = std::fs::read_to_string(dir.join("draft_meta_info.json")).ok().and_then(|t| serde_json::from_str(&t).ok());
            let name = meta
                .as_ref()
                .and_then(|m| m.get("draft_name").and_then(|v| v.as_str()).map(String::from))
                .unwrap_or_else(|| dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
            let cover = Some(dir.join("draft_cover.jpg")).filter(|p| p.is_file());
            out.push(Draft { name, modified: json.metadata().and_then(|m| m.modified()).ok(), dir, cover });
        }
    }
    out.sort_by_key(|d| std::cmp::Reverse(d.modified));
    out
}

fn us(v: Option<&Value>, key: &str) -> i64 {
    v.and_then(|r| r.get(key)).and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)).or_else(|| x.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0)
}

fn f(v: &Value, path: &[&str]) -> Option<f64> {
    let mut cur = v;
    for p in path {
        cur = cur.get(*p)?;
    }
    cur.as_f64()
}

/// Pfad eines Materials auflösen (Platzhalter für den Entwurfsordner ersetzen).
fn resolve(path: &str, draft_dir: &Path) -> PathBuf {
    if let Some(rest) = path.strip_prefix("##_draftpath_placeholder_") {
        if let Some((_, tail)) = rest.split_once("_##") {
            return draft_dir.join(tail.trim_start_matches(['/', '\\']));
        }
    }
    PathBuf::from(path)
}

/// Entwurf importieren: Ordner oder `draft_content.json`.
pub fn import(path: &Path) -> Result<(Project, Report)> {
    let (dir, json) = if path.is_dir() { (path.to_path_buf(), path.join("draft_content.json")) } else { (path.parent().unwrap_or(Path::new(".")).to_path_buf(), path.to_path_buf()) };
    let text = std::fs::read_to_string(&json).map_err(|e| format!("{}: {e}", json.display()))?;
    let d: Value = serde_json::from_str(&text).map_err(|_| "Der Entwurf ist verschlüsselt oder beschädigt und kann nicht gelesen werden".to_string())?;
    let name = std::fs::read_to_string(dir.join("draft_meta_info.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|m| m.get("draft_name").and_then(|v| v.as_str()).map(String::from))
        .or_else(|| d.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from))
        .unwrap_or_else(|| dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "CapCut-Projekt".into()));
    convert(&d, &dir, &name)
}

/// Umwandlung der JSON-Struktur (getrennt für Tests).
pub fn convert(d: &Value, dir: &Path, name: &str) -> Result<(Project, Report)> {
    let mut rep = Report::default();
    let mut p = Project::new(name);
    let w = f(d, &["canvas_config", "width"]).unwrap_or(1920.0).max(2.0) as u32 & !1;
    let h = f(d, &["canvas_config", "height"]).unwrap_or(1080.0).max(2.0) as u32 & !1;
    p.settings.width = w;
    p.settings.height = h;
    p.settings.aspect = AspectPreset::Custom;
    let fps = f(d, &["fps"]).unwrap_or(30.0);
    p.settings.fps = if (fps - 29.97).abs() < 0.01 { Rational::new(30000, 1001) } else { Rational::new(fps.round().max(1.0) as u32, 1) };

    // Materialien nach ID
    let mats = d.get("materials").cloned().unwrap_or_default();
    let mut by_id: HashMap<String, (String, Value)> = HashMap::new();
    if let Some(obj) = mats.as_object() {
        for (kind, list) in obj {
            if let Some(arr) = list.as_array() {
                for m in arr {
                    if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                        by_id.insert(id.to_string(), (kind.clone(), m.clone()));
                    }
                }
            }
        }
    }
    // Medien nur einmal prüfen
    let mut media_cache: HashMap<PathBuf, Option<MediaRef>> = HashMap::new();
    let mut probe = |path: PathBuf, rep: &mut Report, p: &mut Project| -> Option<MediaRef> {
        media_cache
            .entry(path.clone())
            .or_insert_with(|| {
                if !path.is_file() {
                    rep.missing.push(path.clone());
                    return None;
                }
                let m = cl_media::probe(&path).ok()?;
                p.media.push(m.clone());
                Some(m)
            })
            .clone()
    };

    let tracks = d.get("tracks").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    let mut first_video = true;
    for t in &tracks {
        let ty = t.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let kind = match ty {
            "video" | "sticker" => TrackKind::Video,
            "audio" => TrackKind::Audio,
            "text" => TrackKind::Text,
            "effect" => {
                for _ in t.get("segments").and_then(|s| s.as_array()).into_iter().flatten() {
                    rep.skip("Effekte");
                }
                continue;
            }
            "filter" | "adjust" => {
                for _ in t.get("segments").and_then(|s| s.as_array()).into_iter().flatten() {
                    rep.skip("Filter");
                }
                continue;
            }
            _ => continue,
        };
        let mut track = Track::new(kind);
        if kind == TrackKind::Video && ty == "video" && first_video {
            track.main = true;
            first_video = false;
        }
        for s in t.get("segments").and_then(|s| s.as_array()).into_iter().flatten() {
            let mid = s.get("material_id").and_then(|v| v.as_str()).unwrap_or("");
            let Some((mkind, m)) = by_id.get(mid) else { continue };
            let target = s.get("target_timerange");
            let source = s.get("source_timerange");
            let start = Time(us(target, "start"));
            let duration = Time(us(target, "duration").max(1));
            let mut clip = match mkind.as_str() {
                "videos" | "audios" => {
                    let raw = m.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    if raw.is_empty() {
                        rep.skip(if mkind == "videos" { "Online-Material" } else { "Online-Musik / Sprachausgabe" });
                        continue;
                    }
                    let Some(media) = probe(resolve(raw, dir), &mut rep, &mut p) else { continue };
                    let kind = match media.kind {
                        MediaKind::Image => MediaKind::Image,
                        k if mkind == "audios" => {
                            let _ = k;
                            MediaKind::Audio
                        }
                        k => k,
                    };
                    let mut c = Clip::new(ClipContent::Media { media: media.id, kind }, start, duration);
                    c.source_in = Time(us(source, "start"));
                    rep.clips += 1;
                    c
                }
                "texts" => {
                    let content: Value = m.get("content").and_then(|v| v.as_str()).and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
                    let text = content.get("text").and_then(|v| v.as_str()).map(String::from).or_else(|| m.get("base_content").and_then(|v| v.as_str()).map(String::from)).unwrap_or_default();
                    let style0 = content.get("styles").and_then(|s| s.as_array()).and_then(|a| a.first()).cloned().unwrap_or_default();
                    let rgb = style0.pointer("/fill/content/solid/color").and_then(|c| c.as_array()).map(|a| a.iter().map(|x| (x.as_f64().unwrap_or(1.0) * 255.0).round() as u8).collect::<Vec<_>>()).unwrap_or_else(|| vec![255, 255, 255]);
                    let size = style0.get("size").and_then(|v| v.as_f64()).or_else(|| m.get("font_size").and_then(|v| v.as_f64())).unwrap_or(15.0);
                    let mut st = TextStyle { text, ..Default::default() };
                    st.color = [rgb.first().copied().unwrap_or(255), rgb.get(1).copied().unwrap_or(255), rgb.get(2).copied().unwrap_or(255), 255];
                    // CapCut-Größe 15 entspricht etwa 72 px bei 1080p
                    st.size = (size as f32 * 4.8 * h.min(w) as f32 / 1080.0).clamp(8.0, 600.0);
                    st.bold = style0.get("bold").and_then(|v| v.as_bool()).unwrap_or(false);
                    st.italic = style0.get("italic").and_then(|v| v.as_bool()).unwrap_or(false);
                    st.align = match m.get("alignment").and_then(|v| v.as_i64()) {
                        Some(0) => -1,
                        Some(2) => 1,
                        _ => 0,
                    };
                    if let Some(sw) = style0.pointer("/strokes/0/width").and_then(|v| v.as_f64()) {
                        let c = style0.pointer("/strokes/0/content/solid/color").and_then(|c| c.as_array()).map(|a| a.iter().map(|x| (x.as_f64().unwrap_or(0.0) * 255.0).round() as u8).collect::<Vec<_>>()).unwrap_or_else(|| vec![0, 0, 0]);
                        st.outline = Some(((sw * 60.0) as f32, [c[0], c.get(1).copied().unwrap_or(0), c.get(2).copied().unwrap_or(0), 255]));
                    }
                    rep.texts += 1;
                    Clip::new(ClipContent::Text(st), start, duration)
                }
                "stickers" => {
                    rep.skip("Sticker");
                    continue;
                }
                other => {
                    rep.skip(match other {
                        "speeds" | "canvases" | "sound_channel_mappings" => continue,
                        _ => "Sonstiges",
                    });
                    continue;
                }
            };
            // gemeinsame Eigenschaften
            let speed = s.get("speed").and_then(|v| v.as_f64()).unwrap_or(1.0);
            clip.speed = speed.clamp(0.1, 10.0) as f32;
            let vol = s.get("volume").and_then(|v| v.as_f64()).unwrap_or(1.0);
            clip.volume_db = if vol <= 0.0001 { -60.0 } else { (20.0 * vol.log10()) as f32 };
            clip.disabled = !s.get("visible").and_then(|v| v.as_bool()).unwrap_or(true);
            clip.fx.reverse = s.get("reverse").and_then(|v| v.as_bool()).unwrap_or(false);
            if let Some(c) = s.get("clip").filter(|c| !c.is_null()) {
                clip.opacity = f(c, &["alpha"]).unwrap_or(1.0) as f32;
                clip.transform.scale = f(c, &["scale", "x"]).unwrap_or(1.0) as f32;
                clip.transform.rotation_deg = f(c, &["rotation"]).unwrap_or(0.0) as f32;
                // CapCut: Anteil der halben Bildbreite/-höhe, y nach oben
                clip.transform.x = (f(c, &["transform", "x"]).unwrap_or(0.0) * w as f64 / 2.0) as f32;
                clip.transform.y = (-f(c, &["transform", "y"]).unwrap_or(0.0) * h as f64 / 2.0) as f32;
                clip.fx.flip_h = c.pointer("/flip/horizontal").and_then(|v| v.as_bool()).unwrap_or(false);
                clip.fx.flip_v = c.pointer("/flip/vertical").and_then(|v| v.as_bool()).unwrap_or(false);
            }
            // Bei Text ist die Größe schon in der Schrift enthalten
            if matches!(clip.content, ClipContent::Text(_)) {
                if let ClipContent::Text(st) = &mut clip.content {
                    st.size = (st.size * clip.transform.scale).clamp(8.0, 600.0);
                }
                clip.transform.scale = 1.0;
            }
            track.clips.push(clip);
        }
        track.clips.sort_by_key(|c| c.start);
        if !track.clips.is_empty() {
            p.timeline.tracks.push(track);
        }
    }
    // Spuren gleicher Art ohne Überschneidung zusammenlegen (CapCut legt z. B. jede
    // Sprachaufnahme auf eine eigene Spur)
    let mut merged: Vec<Track> = Vec::new();
    for t in std::mem::take(&mut p.timeline.tracks) {
        if t.main || t.kind == TrackKind::Video {
            merged.push(t);
            continue;
        }
        let fits = |m: &Track, t: &Track| m.kind == t.kind && !m.main && t.clips.iter().all(|c| m.clips.iter().all(|o| c.end() <= o.start || c.start >= o.end()));
        match merged.iter_mut().find(|m| fits(m, &t)) {
            Some(m) => {
                m.clips.extend(t.clips);
                m.clips.sort_by_key(|c| c.start);
            }
            None => merged.push(t),
        }
    }
    p.timeline.tracks = merged;
    if !p.timeline.tracks.iter().any(|t| t.main) {
        if let Some(t) = p.timeline.tracks.iter_mut().find(|t| t.kind == TrackKind::Video) {
            t.main = true;
        }
    }
    // Hauptspur nach unten, Audio ans Ende (Cutline zeichnet von unten nach oben)
    p.timeline.tracks.sort_by_key(|t| match (t.main, t.kind) {
        (true, _) => 0,
        (_, TrackKind::Audio) => 2,
        _ => 1,
    });
    if rep.clips + rep.texts == 0 {
        return Err("Im Entwurf wurden keine übertragbaren Clips gefunden".into());
    }
    Ok((p, rep))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_draft() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("clip.mp4");
        assert!(cl_media::ffmpeg_command().unwrap().args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=4", "-pix_fmt", "yuv420p"]).arg(&v).status().unwrap().success());
        let rec = dir.path().join("audio_record");
        std::fs::create_dir_all(&rec).unwrap();
        let a = rec.join("vo.wav");
        assert!(cl_media::ffmpeg_command().unwrap().args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=d=2"]).arg(&a).status().unwrap().success());
        let content = serde_json::json!({"text": "Hallo Welt", "styles": [{"fill": {"content": {"solid": {"color": [1.0, 0.0, 0.0]}}}, "size": 15.0}]}).to_string();
        let draft = serde_json::json!({
            "canvas_config": {"width": 1080, "height": 1920},
            "fps": 30.0,
            "materials": {
                "videos": [{"id": "V1", "type": "video", "path": v.to_string_lossy()}],
                "audios": [{"id": "A1", "type": "record", "path": "##_draftpath_placeholder_ABC_##/audio_record/vo.wav"}],
                "texts": [{"id": "T1", "content": content, "alignment": 1}],
                "stickers": [{"id": "S1"}]
            },
            "tracks": [
                {"type": "video", "segments": [{"material_id": "V1", "target_timerange": {"start": 0, "duration": 3000000}, "source_timerange": {"start": 500000, "duration": 3000000}, "speed": 1.0, "volume": 0.5, "visible": true, "clip": {"alpha": 0.8, "scale": {"x": 1.2, "y": 1.2}, "rotation": 10.0, "transform": {"x": 0.5, "y": 0.5}, "flip": {"horizontal": true, "vertical": false}}}]},
                {"type": "audio", "segments": [{"material_id": "A1", "target_timerange": {"start": 1000000, "duration": 2000000}, "source_timerange": {"start": 0, "duration": 2000000}, "speed": 1.0, "volume": 1.0, "clip": null}]},
                {"type": "text", "segments": [{"material_id": "T1", "target_timerange": {"start": 0, "duration": 2000000}, "speed": 1.0, "volume": 1.0, "clip": {"alpha": 1.0, "scale": {"x": 1.0, "y": 1.0}, "rotation": 0.0, "transform": {"x": 0.0, "y": -0.8}, "flip": {"horizontal": false, "vertical": false}}}]},
                {"type": "sticker", "segments": [{"material_id": "S1", "target_timerange": {"start": 0, "duration": 1000000}}]},
                {"type": "effect", "segments": [{"material_id": "E1", "target_timerange": {"start": 0, "duration": 1000000}}]}
            ]
        });
        let (p, rep) = convert(&draft, dir.path(), "Test").unwrap();
        assert_eq!((p.settings.width, p.settings.height), (1080, 1920));
        assert_eq!(rep.clips, 2);
        assert_eq!(rep.texts, 1);
        assert!(rep.missing.is_empty(), "{:?}", rep.missing);
        assert!(rep.skipped.iter().any(|(w, _)| w == "Sticker") && rep.skipped.iter().any(|(w, _)| w == "Effekte"));
        let main = &p.timeline.tracks[0];
        assert!(main.main);
        let c = &main.clips[0];
        assert_eq!(c.source_in, Time(500000));
        assert!((c.volume_db + 6.02).abs() < 0.1);
        assert!((c.transform.x - 270.0).abs() < 0.1 && (c.transform.y + 480.0).abs() < 0.1);
        assert!(c.fx.flip_h && (c.opacity - 0.8).abs() < 1e-3);
        let text = p.timeline.tracks.iter().find(|t| t.kind == TrackKind::Text).unwrap();
        let ClipContent::Text(st) = &text.clips[0].content else { panic!() };
        assert_eq!(st.text, "Hallo Welt");
        assert_eq!(st.color, [255, 0, 0, 255]);
        assert_eq!(st.align, 0);
        assert_eq!(p.timeline.tracks.last().unwrap().kind, TrackKind::Audio);
    }
}
