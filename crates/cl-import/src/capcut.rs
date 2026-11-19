//! CapCut-Desktop-Entwürfe (`draft_content.json`, Zeiten in Mikrosekunden).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cl_core::{
    AspectPreset, Clip, ClipContent, MediaKind, MediaRef, Project, TextStyle, Time,
    Track, TrackId, TrackKind,
};
use serde_json::Value;

use crate::{Progress, Report, Result, Step};

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
        v.push(
            l.join("CapCut")
                .join("User Data")
                .join("Projects")
                .join("com.lveditor.draft"),
        );
        v.push(
            l.join("JianyingPro")
                .join("User Data")
                .join("Projects")
                .join("com.lveditor.draft"),
        );
    }
    if let Some(h) = std::env::var_os("HOME") {
        v.push(
            PathBuf::from(h)
                .join("Movies")
                .join("CapCut")
                .join("User Data")
                .join("Projects")
                .join("com.lveditor.draft"),
        );
    }
    v.into_iter().filter(|p| p.is_dir()).collect()
}

/// Alle lokalen Entwürfe, neueste zuerst.
pub fn list_drafts() -> Vec<Draft> {
    let mut out = Vec::new();
    for root in draft_roots() {
        let Ok(rd) = std::fs::read_dir(&root) else {
            continue;
        };
        for e in rd.flatten() {
            let dir = e.path();
            let json = dir.join("draft_content.json");
            if !json.is_file() {
                continue;
            }
            let meta: Option<Value> = std::fs::read_to_string(dir.join("draft_meta_info.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok());
            let name = meta
                .as_ref()
                .and_then(|m| {
                    m.get("draft_name")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .unwrap_or_else(|| {
                    dir.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
            let cover = Some(dir.join("draft_cover.jpg")).filter(|p| p.is_file());
            out.push(Draft {
                name,
                modified: json.metadata().and_then(|m| m.modified()).ok(),
                dir,
                cover,
            });
        }
    }
    out.sort_by_key(|d| std::cmp::Reverse(d.modified));
    out
}

fn us(v: Option<&Value>, key: &str) -> i64 {
    v.and_then(|r| r.get(key))
        .and_then(|x| {
            x.as_i64()
                .or_else(|| x.as_f64().map(|f| f as i64))
                .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(0)
        // kaputte Entwürfe: nichts jenseits von Time::LIMIT (wie im Kern)
        .clamp(-Time::LIMIT.0, Time::LIMIT.0)
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
    import_with(path, &Progress::default())
}

/// Wie [`import`], mit Fortschritt und Abbruch (Hintergrund-Thread).
pub fn import_with(path: &Path, progress: &Progress) -> Result<(Project, Report)> {
    let (dir, json) = if path.is_dir() {
        (path.to_path_buf(), path.join("draft_content.json"))
    } else {
        (
            path.parent().unwrap_or(Path::new(".")).to_path_buf(),
            path.to_path_buf(),
        )
    };
    progress.set(Step::Read, 0, 1, &file_label(&json));
    let text = std::fs::read_to_string(&json).map_err(|e| format!("{}: {e}", json.display()))?;
    progress.check()?;
    let d: Value = serde_json::from_str(&text).map_err(|_| {
        "Der Entwurf ist verschlüsselt oder beschädigt und kann nicht gelesen werden".to_string()
    })?;
    drop(text);
    let name = std::fs::read_to_string(dir.join("draft_meta_info.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|m| {
            m.get("draft_name")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .or_else(|| {
            d.get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from)
        })
        .unwrap_or_else(|| {
            dir.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "CapCut-Projekt".into())
        });
    progress.set(Step::Read, 1, 1, "");
    convert_with(&d, &dir, &name, progress)
}

fn file_label(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Verschachtelte Entwürfe (zusammengesetzte Clips) höchstens so tief auflösen
const MAX_DEPTH: usize = 8;

/// Höchstens so viele Segmente (alle Stufen zusammen); echte Entwürfe haben wenige Tausend
const MAX_SEGMENTS: usize = 1_000_000;

/// Materialien eines Entwurfs nach ID: (Art, Material)
fn materials(d: &Value) -> HashMap<&str, (&str, &Value)> {
    let mut by_id = HashMap::new();
    if let Some(obj) = d.get("materials").and_then(|m| m.as_object()) {
        for (kind, list) in obj {
            for m in list.as_array().into_iter().flatten() {
                if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                    by_id.insert(id, (kind.as_str(), m));
                }
            }
        }
    }
    by_id
}

fn segments(t: &Value) -> impl Iterator<Item = &Value> {
    t.get("segments").and_then(|s| s.as_array()).into_iter().flatten()
}

/// Verschachtelter Entwurf eines Materials (CapCut „zusammengesetzter Clip“)
fn sub_draft(m: &Value) -> Option<&Value> {
    m.get("draft").filter(|v| v.is_object())
}

/// Medien- und Schriftdateien, die der Import braucht (in der Reihenfolge der Spuren, auch in
/// verschachtelten Entwürfen) – zum Prüfen vorab. `seen`: bisher besuchte Segmente; jedes Segment
/// auf einen inneren Entwurf löst ihn neu auf, so wächst der Aufwand mit der Tiefe exponentiell.
/// Die Zeitleiste geht danach genau dieselben Segmente durch, die Grenze gilt also für beide.
fn collect_files(d: &Value, dir: &Path, depth: usize, media: &mut Vec<PathBuf>, fonts: &mut Vec<PathBuf>, seen: &mut usize, progress: &Progress) -> Result<()> {
    let by_id = materials(d);
    for t in d.get("tracks").and_then(|t| t.as_array()).into_iter().flatten() {
        if !matches!(t.get("type").and_then(|v| v.as_str()), Some("video" | "sticker" | "audio" | "text")) {
            continue;
        }
        for s in segments(t) {
            *seen += 1;
            if *seen > MAX_SEGMENTS {
                return Err("Entwurf zu groß".into());
            }
            if *seen % 256 == 0 {
                progress.check()?;
            }
            let mid = s.get("material_id").and_then(|v| v.as_str()).unwrap_or("");
            let Some((kind, m)) = by_id.get(mid) else { continue };
            match *kind {
                "videos" | "audios" => {
                    let raw = m.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    if !raw.is_empty() {
                        media.push(resolve(raw, dir));
                    }
                }
                "texts" => fonts.extend(text_font(m, dir)),
                "drafts" if depth < MAX_DEPTH => {
                    if let Some(sub) = sub_draft(m) {
                        collect_files(sub, dir, depth + 1, media, fonts, seen, progress)?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Doppelte entfernen, Reihenfolge behalten
fn unique(v: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    v.into_iter().filter(|p| seen.insert(p.clone())).collect()
}

fn text_content(m: &Value) -> Value {
    m.get("content")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

fn first_style(content: &Value) -> Value {
    content
        .get("styles")
        .and_then(|s| s.as_array())
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or_default()
}

/// Schriftdatei eines Textes: eigene Schrift des Textes, sonst die des Stils (CapCut-Standard)
fn text_font(m: &Value, dir: &Path) -> Option<PathBuf> {
    let style0 = first_style(&text_content(m));
    let style_font = style0.pointer("/font/path").and_then(|v| v.as_str());
    let mat_font = m.get("font_path").and_then(|v| v.as_str());
    let is_default = |p: &str| p.contains("SystemFont");
    let font = match (mat_font, style_font) {
        (Some(a), _) if !a.is_empty() && !is_default(a) => Some(a),
        (_, Some(b)) if !b.is_empty() => Some(b),
        (Some(a), _) if !a.is_empty() => Some(a),
        _ => None,
    };
    font.map(|f| resolve(f, dir))
}

/// Zustand beim Aufbau der Zeitleiste: vorab geprüfte Dateien, übernommene Medien, Bericht.
struct Build<'a> {
    w: u32,
    h: u32,
    exists: HashMap<PathBuf, bool>,
    probed: HashMap<PathBuf, Result<MediaRef>>,
    fonts: HashMap<PathBuf, Option<String>>,
    /// Medien nur einmal übernehmen
    used: HashMap<PathBuf, Option<MediaRef>>,
    media: Vec<MediaRef>,
    rep: Report,
    progress: &'a Progress,
    /// Segmente seit der letzten Abbruch-Prüfung
    since_check: usize,
}

impl Build<'_> {
    /// `mkind`, `mat`: Materialart und Material (Angaben für den Platzhalter einer fehlenden Datei)
    fn media(&mut self, path: PathBuf, mkind: &str, mat: &Value) -> Option<MediaRef> {
        if let Some(m) = self.used.get(&path) {
            return m.clone();
        }
        let m = if !self.exists.get(&path).copied().unwrap_or(false) {
            self.rep.missing.push(path.clone());
            let kind = match (mkind, mat.get("type").and_then(|v| v.as_str())) {
                ("audios", _) => MediaKind::Audio,
                (_, Some("photo")) => MediaKind::Image,
                _ => MediaKind::Video,
            };
            let dim = |k: &str| mat.get(k).and_then(|v| v.as_u64()).unwrap_or(0).min(u32::MAX as u64) as u32;
            let ph = crate::placeholder(path.clone(), kind, Time(us(Some(mat), "duration").max(0)), dim("width"), dim("height"));
            self.media.push(ph.clone());
            Some(ph)
        } else {
            match self.probed.remove(&path) {
                Some(Ok(m)) => {
                    self.media.push(m.clone());
                    Some(m)
                }
                _ => {
                    self.rep.failed.push(path.clone());
                    None
                }
            }
        };
        self.used.insert(path, m.clone());
        m
    }

    /// Spuren eines Entwurfs (Ebene nach CapCut-`render_index`); `depth` > 0: verschachtelt.
    fn tracks(&mut self, d: &Value, dir: &Path, depth: usize) -> Result<Vec<(Track, i64)>> {
        let by_id = materials(d);
        let tracks: Vec<&Value> = d.get("tracks").and_then(|t| t.as_array()).into_iter().flatten().collect();
        let mut out = Vec::new();
        let mut nested: Vec<(&Value, &Value, i64)> = Vec::new();
        let mut first_video = depth == 0;
        let (w, h) = (self.w, self.h);
        for (ti, t) in tracks.iter().enumerate() {
            if depth == 0 {
                self.progress.check()?;
                self.progress.set(Step::Timeline, ti, tracks.len(), &format!("{} / {}", ti + 1, tracks.len()));
            }
            let ty = t.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let kind = match ty {
                "video" | "sticker" => TrackKind::Video,
                "audio" => TrackKind::Audio,
                "text" => TrackKind::Text,
                "effect" => {
                    for _ in segments(t) {
                        self.rep.skip("Effekte");
                    }
                    continue;
                }
                "filter" | "adjust" => {
                    for _ in segments(t) {
                        self.rep.skip("Filter");
                    }
                    continue;
                }
                _ => continue,
            };
            let mut track = Track::new(kind);
            if kind == TrackKind::Video && ty == "video" && first_video {
                track.main = true;
                first_video = false;
                // „Originalton der Hauptspur aus“
                track.muted = d.pointer("/config/video_mute").and_then(|v| v.as_bool()).unwrap_or(false);
            }
            // Stummschalter am Spurkopf (Bit 0); CapCut setzt dabei die Clips auf 0 und merkt sich
            // die Lautstärke in `last_nonzero_volume` – in Cutline bleibt die Spur stumm (ein Klick hebt es auf)
            track.muted |= t.get("attribute").and_then(|v| v.as_i64()).unwrap_or(0) & 1 == 1;
            for s in segments(t) {
                self.since_check += 1;
                if self.since_check >= 256 {
                    self.since_check = 0;
                    self.progress.check()?;
                }
                let mid = s.get("material_id").and_then(|v| v.as_str()).unwrap_or("");
                let Some(&(mkind, m)) = by_id.get(mid) else {
                    continue;
                };
                let target = s.get("target_timerange");
                let source = s.get("source_timerange");
                // kaputte Entwürfe: nichts vor 0
                let start = Time(us(target, "start").max(0));
                let duration = Time(us(target, "duration").max(1));
                let mut clip = match mkind {
                    "videos" | "audios" => {
                        let raw = m.get("path").and_then(|v| v.as_str()).unwrap_or("");
                        if raw.is_empty() {
                            self.rep.skip(if mkind == "videos" {
                                "Online-Material"
                            } else {
                                "Online-Musik / Sprachausgabe"
                            });
                            continue;
                        }
                        let Some(media) = self.media(resolve(raw, dir), mkind, m) else {
                            continue;
                        };
                        let kind = match media.kind {
                            MediaKind::Image => MediaKind::Image,
                            _ if mkind == "audios" => MediaKind::Audio,
                            k => k,
                        };
                        let mut c = Clip::new(
                            ClipContent::Media {
                                media: media.id,
                                kind,
                            },
                            start,
                            duration,
                        );
                        c.source_in = Time(us(source, "start").max(0));
                        self.rep.clips += 1;
                        c
                    }
                    "texts" => {
                        let st = self.text_style(m, dir, w, h);
                        self.rep.texts += 1;
                        Clip::new(ClipContent::Text(st), start, duration)
                    }
                    "stickers" => {
                        self.rep.skip("Sticker");
                        continue;
                    }
                    "drafts" => {
                        match sub_draft(m).filter(|_| depth < MAX_DEPTH) {
                            Some(sub) => nested.push((sub, s, layer_of(t, kind))),
                            None => self.rep.skip("Zusammengesetzte Clips"),
                        }
                        continue;
                    }
                    other => {
                        self.rep.skip(match other {
                            "speeds" | "canvases" | "sound_channel_mappings" => continue,
                            _ => "Sonstiges",
                        });
                        continue;
                    }
                };
                // gemeinsame Eigenschaften
                let speed = s.get("speed").and_then(|v| v.as_f64()).unwrap_or(1.0);
                clip.speed = speed.clamp(0.1, 10.0) as f32;
                let mut vol = s.get("volume").and_then(|v| v.as_f64()).unwrap_or(1.0);
                if vol <= 0.0001 && track.muted {
                    vol = s.get("last_nonzero_volume").and_then(|v| v.as_f64()).unwrap_or(vol);
                }
                // Ton vom Video getrennt: er liegt als eigener Ton-Clip vor (wie „Ton extrahieren“)
                if mkind == "videos" && m.get("has_sound_separated").and_then(|v| v.as_bool()).unwrap_or(false) {
                    vol = 0.0;
                }
                // Grenzen wie beim Bearbeiten (ReplaceClip, Ziehen im Player); unsinnige Werte
                // aus kaputten Entwürfen wären als f32 sonst unendlich
                let fin = |v: f64, d: f32| Some(v as f32).filter(|v| v.is_finite()).unwrap_or(d);
                clip.volume_db = if vol <= 0.0001 {
                    -60.0
                } else {
                    ((20.0 * vol.log10()) as f32).clamp(-60.0, 20.0)
                };
                clip.disabled = !s.get("visible").and_then(|v| v.as_bool()).unwrap_or(true);
                clip.fx.reverse = s.get("reverse").and_then(|v| v.as_bool()).unwrap_or(false);
                if let Some(c) = s.get("clip").filter(|c| !c.is_null()) {
                    clip.opacity = (f(c, &["alpha"]).unwrap_or(1.0) as f32).clamp(0.0, 1.0);
                    clip.transform.scale = (f(c, &["scale", "x"]).unwrap_or(1.0) as f32).clamp(0.02, 20.0);
                    clip.transform.rotation_deg = fin(f(c, &["rotation"]).unwrap_or(0.0), 0.0);
                    // CapCut: Anteil der halben Bildbreite/-höhe, y nach oben
                    clip.transform.x = fin(f(c, &["transform", "x"]).unwrap_or(0.0) * w as f64 / 2.0, 0.0);
                    clip.transform.y = fin(-f(c, &["transform", "y"]).unwrap_or(0.0) * h as f64 / 2.0, 0.0);
                    clip.fx.flip_h = c
                        .pointer("/flip/horizontal")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    clip.fx.flip_v = c
                        .pointer("/flip/vertical")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                }
                // Ein-/Ausblend-Animationen: CapCuts eigene Animationen gibt es hier nicht, deshalb
                // wird jede als weiche Blende mit gleicher Dauer übernommen
                for r in s.get("extra_material_refs").and_then(|v| v.as_array()).into_iter().flatten() {
                    let Some(&(k, am)) = r.as_str().and_then(|id| by_id.get(id)) else { continue };
                    let us_of = |key: &str| Time(am.get(key).and_then(|v| v.as_i64()).unwrap_or(0).clamp(0, duration.0));
                    match k {
                        // Tonspur der Datei, 1-basiert (1 = Standard); ohne Verweis die erste
                        "audio_track_indexes" => {
                            let n = am.get("audio_track_index").and_then(|v| v.as_i64()).unwrap_or(1);
                            clip.audio_stream = (n > 1).then(|| (n - 1) as u32);
                            continue;
                        }
                        "audio_fades" => {
                            clip.fade_in = us_of("fade_in_duration");
                            clip.fade_out = us_of("fade_out_duration");
                            continue;
                        }
                        "material_animations" => {}
                        _ => continue,
                    }
                    for a in am.get("animations").and_then(|v| v.as_array()).into_iter().flatten() {
                        let anim = cl_core::AnimRef {
                            id: "fade".into(),
                            duration: Time(a.get("duration").and_then(|v| v.as_i64()).unwrap_or(500_000).clamp(100_000, duration.0.max(100_000))),
                        };
                        match a.get("type").and_then(|v| v.as_str()) {
                            Some("in") => clip.anim.intro = Some(anim),
                            Some("out") => clip.anim.outro = Some(anim),
                            _ => continue,
                        }
                        self.rep.skip("Animation (als Blende übernommen)");
                    }
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
            out.push((track, layer_of(t, kind)));
        }
        // Zusammengesetzte Clips: Spuren des inneren Entwurfs an die Stelle des Clips setzen,
        // auf dessen Ausschnitt gekürzt (Stufe für Stufe, höchstens `MAX_DEPTH` tief)
        for (sub, s, layer) in nested {
            let speed = s.get("speed").and_then(|v| v.as_f64()).unwrap_or(1.0);
            if (speed - 1.0).abs() > 1e-6 {
                self.rep.skip("Zusammengesetzte Clips mit Tempo");
                continue;
            }
            let start = Time(us(s.get("target_timerange"), "start").max(0));
            let dur = Time(us(s.get("target_timerange"), "duration").max(1));
            let from = Time(us(s.get("source_timerange"), "start").max(0));
            for (t, ri) in self.tracks(sub, dir, depth + 1)? {
                let mut t = place(t, start, dur, from);
                t.main = false;
                if !t.clips.is_empty() {
                    let ri = if t.kind == TrackKind::Text { ri } else { layer };
                    out.push((t, ri));
                }
            }
            self.rep.nested += 1;
        }
        Ok(out)
    }

    fn text_style(&mut self, m: &Value, dir: &Path, w: u32, h: u32) -> TextStyle {
        let content = text_content(m);
        let text = content
            .get("text")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                m.get("base_content")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .unwrap_or_default();
        let style0 = first_style(&content);
        let rgb = style0
            .pointer("/fill/content/solid/color")
            .and_then(|c| c.as_array())
            .map(|a| {
                a.iter()
                    .map(|x| (x.as_f64().unwrap_or(1.0) * 255.0).round() as u8)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![255, 255, 255]);
        let size = style0
            .get("size")
            .and_then(|v| v.as_f64())
            .or_else(|| m.get("font_size").and_then(|v| v.as_f64()))
            .unwrap_or(15.0);
        let mut st = TextStyle {
            text,
            ..Default::default()
        };
        st.color = [
            rgb.first().copied().unwrap_or(255),
            rgb.get(1).copied().unwrap_or(255),
            rgb.get(2).copied().unwrap_or(255),
            255,
        ];
        // CapCut-Größe 15 entspricht etwa 72 px bei 1080p
        // 11,5: an CapCut 9.2 nachgemessen (Projekt mit Größe 15 und 8,1 bei 1080p)
        st.size = (size as f32 * 11.5 * h.min(w) as f32 / 1080.0).clamp(8.0, 600.0);
        st.bold = style0
            .get("bold")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        st.italic = style0
            .get("italic")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        st.align = match m.get("alignment").and_then(|v| v.as_i64()) {
            Some(0) => -1,
            Some(2) => 1,
            _ => 0,
        };
        if let Some(fp) = text_font(m, dir) {
            if let Some(fam) = self.fonts.get(&fp).cloned().flatten() {
                st.font_family = Some(fam);
                if !self.rep.fonts.contains(&fp) {
                    self.rep.fonts.push(fp);
                }
            }
        }
        if let Some(sw) = style0.pointer("/strokes/0/width").and_then(|v| v.as_f64()) {
            let c = style0
                .pointer("/strokes/0/content/solid/color")
                .and_then(|c| c.as_array())
                .map(|a| {
                    a.iter()
                        .map(|x| (x.as_f64().unwrap_or(0.0) * 255.0).round() as u8)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec![0, 0, 0]);
            st.outline = Some((
                // höchstens so stark wie der Regler im Inspektor
                ((sw * 60.0) as f32).clamp(0.0, 20.0),
                [
                    c.first().copied().unwrap_or(0),
                    c.get(1).copied().unwrap_or(0),
                    c.get(2).copied().unwrap_or(0),
                    255,
                ],
            ));
        }
        // Schatten nur, wenn er in CapCut an ist (Cutline-Standard wäre „an“)
        st.shadow = m.get("has_shadow").and_then(|v| v.as_bool()).unwrap_or(false);
        st
    }
}

/// Ebene einer Spur: höchster `render_index` ihrer Segmente
fn layer_of(t: &Value, kind: TrackKind) -> i64 {
    segments(t)
        .filter_map(|s| s.get("render_index").and_then(|v| v.as_i64()))
        .max()
        .unwrap_or(if kind == TrackKind::Text { 14000 } else { 0 })
}

/// Clips einer inneren Spur auf die Zeitleiste außen setzen: innere Zeit `from` liegt bei
/// `start`, sichtbar ist nur `[start, start + dur)`; angeschnittene Clips werden gekürzt.
pub fn place(mut t: Track, start: Time, dur: Time, from: Time) -> Track {
    let end = start + dur;
    t.clips = std::mem::take(&mut t.clips)
        .into_iter()
        .filter_map(|mut c| {
            let s = c.start + start - from;
            let e = c.end() + start - from;
            if e <= start || s >= end {
                return None;
            }
            let cut = if s < start { start - s } else { Time::ZERO };
            c.source_in = c.source_in + Time((cut.0 as f64 * c.speed as f64).round() as i64);
            c.start = s + cut;
            c.duration = e.min(end) - c.start;
            c.id = cl_core::ClipId::new();
            Some(c)
        })
        .collect();
    t
}

/// Umwandlung der JSON-Struktur (getrennt für Tests).
pub fn convert(d: &Value, dir: &Path, name: &str) -> Result<(Project, Report)> {
    convert_with(d, dir, name, &Progress::default())
}

/// Wie [`convert`], in Schritten mit Fortschritt und Abbruch: Medien finden, parallel prüfen,
/// Schriften lesen, Zeitleiste aufbauen.
pub fn convert_with(d: &Value, dir: &Path, name: &str, progress: &Progress) -> Result<(Project, Report)> {
    let mut p = Project::new(name);
    // Grenzen wie `ProjectSettings::valid` (16…16384 px, über 0 bis 240 fps)
    let w = f(d, &["canvas_config", "width"]).unwrap_or(1920.0).clamp(16.0, 16384.0) as u32 & !1;
    let h = f(d, &["canvas_config", "height"])
        .unwrap_or(1080.0)
        .clamp(16.0, 16384.0) as u32
        & !1;
    p.settings.width = w;
    p.settings.height = h;
    // gekürztes Seitenverhältnis (1920×1080 → 16:9) statt „Eigenes“
    let g = {
        let (mut a, mut b) = (w.max(1), h.max(1));
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    p.settings.aspect = AspectPreset::Ratio(w / g, h / g);
    if let Some(fps) = f(d, &["fps"]).filter(|f| *f <= 240.0).and_then(crate::fps_from) {
        p.settings.fps = fps;
    }

    // Medien finden
    let (mut media, mut fonts) = (Vec::new(), Vec::new());
    collect_files(d, dir, 0, &mut media, &mut fonts, &mut 0, progress)?;
    let (media, fonts) = (unique(media), unique(fonts));
    progress.set(Step::Find, 0, media.len(), "");
    let mut exists = HashMap::new();
    for m in &media {
        progress.check()?;
        exists.insert(m.clone(), m.is_file());
        progress.tick(&file_label(m));
    }
    // Medien prüfen (ffprobe parallel)
    let found: Vec<PathBuf> = media.iter().filter(|m| exists[*m]).cloned().collect();
    let probed = crate::probe_all(&found, crate::probe_workers(), progress, |p| {
        cl_media::probe(p).map_err(|e| e.to_string())
    })?;
    // Schriften
    progress.set(Step::Fonts, 0, fonts.len(), "");
    let mut families = HashMap::new();
    for fp in fonts {
        progress.check()?;
        let fam = fp
            .is_file()
            .then(|| std::fs::read(&fp).ok())
            .flatten()
            .and_then(|data| cl_fonts::inspect(&data).ok())
            .and_then(|faces| faces.first().map(|f| f.family.clone()));
        progress.tick(&file_label(&fp));
        families.insert(fp, fam);
    }
    // Zeitleiste
    let mut b = Build {
        w,
        h,
        exists,
        probed: found.into_iter().zip(probed).collect(),
        fonts: families,
        used: HashMap::new(),
        media: Vec::new(),
        rep: Report::default(),
        progress,
        since_check: 0,
    };
    let tracks = b.tracks(d, dir, 0)?;
    progress.check()?;
    progress.set(Step::Timeline, 1, 1, "");
    let mut rep = b.rep;
    p.media = b.media;
    // CapCut stapelt nach `render_index` (Text ab 14000, Overlays 1, 2, …), nicht nach der
    // Reihenfolge der Spuren in der Datei
    let mut layer: HashMap<TrackId, i64> = HashMap::new();
    for (t, ri) in tracks {
        layer.insert(t.id, ri);
        if !t.clips.is_empty() {
            if t.muted && matches!(t.kind, TrackKind::Video | TrackKind::Audio) {
                rep.skip("Spur stumm übernommen");
            }
            p.timeline.tracks.push(t);
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
        let fits = |m: &Track, t: &Track| {
            m.kind == t.kind
                && m.muted == t.muted
                && !m.main
                && t.clips.iter().all(|c| {
                    m.clips
                        .iter()
                        .all(|o| c.end() <= o.start || c.start >= o.end())
                })
        };
        match merged.iter_mut().find(|m| fits(m, &t)) {
            Some(m) => {
                let ri = layer.get(&t.id).copied().unwrap_or(0);
                let e = layer.entry(m.id).or_insert(0);
                *e = (*e).max(ri);
                m.clips.extend(t.clips);
                m.clips.sort_by_key(|c| c.start);
            }
            None => merged.push(t),
        }
    }
    p.timeline.tracks = merged;
    if !p.timeline.tracks.iter().any(|t| t.main) {
        if let Some(t) = p
            .timeline
            .tracks
            .iter_mut()
            .find(|t| t.kind == TrackKind::Video)
        {
            t.main = true;
        }
    }
    // Hauptspur nach unten, darüber nach CapCut-Ebene, Audio ans Ende
    // (Cutline zeichnet Bildspuren von unten nach oben)
    p.timeline.tracks.sort_by_key(|t| match (t.main, t.kind) {
        (true, _) => (0, 0),
        (_, TrackKind::Audio) => (2, 0),
        _ => (1, layer.get(&t.id).copied().unwrap_or(0)),
    });
    if rep.clips + rep.texts == 0 {
        return Err("Im Entwurf wurden keine übertragbaren Clips gefunden".into());
    }
    Ok((p, rep))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fehler: Unsinnige Zahlen aus kaputten Entwürfen (riesig, negativ, 0) landeten im Projekt
    /// (unendlich als f32, Zeiten jenseits von `Time::LIMIT`, Bildgröße 4 Mrd. px).
    /// Nicht endliche Werte lässt schon serde_json nicht durch.
    #[test]
    fn absurd_numbers_are_clamped_where_read() {
        assert!(serde_json::from_str::<Value>("1e999").is_err());
        let content = serde_json::json!({"text": "x", "styles": [{"size": 1e300, "strokes": [{"width": 1e300}]}]}).to_string();
        let clip = |v: f64| serde_json::json!({"alpha": v, "scale": {"x": v}, "rotation": v, "transform": {"x": v, "y": -v}});
        let seg = |start: f64, dur: f64, speed: f64, vol: f64, c: Value| {
            serde_json::json!({"material_id": "T", "target_timerange": {"start": start, "duration": dur}, "speed": speed, "volume": vol, "clip": c})
        };
        let draft = serde_json::json!({
            "canvas_config": {"width": 1e300, "height": 0},
            "fps": 500.0,
            "materials": {"texts": [{"id": "T", "content": content}]},
            "tracks": [
                {"type": "text", "segments": [seg(1e300, 1e300, 0.0, 1e300, clip(1e300)), seg(-5.0, -7.0, -1.0, -1.0, clip(-1e300))]},
                {"type": "text", "segments": [seg(0.0, 1e6, 1.0, 1.0, clip(0.0))]}
            ]
        });
        let (p, _) = convert(&draft, Path::new("."), "T").unwrap();
        assert!(p.settings.valid(), "{:?}", p.settings);
        for c in p.timeline.tracks.iter().flat_map(|t| &t.clips) {
            assert!(c.start >= Time::ZERO && c.duration > Time::ZERO && c.end() <= Time::LIMIT + Time::LIMIT, "{c:?}");
            assert!(c.source_in >= Time::ZERO && c.source_in <= Time::LIMIT);
            let t = &c.transform;
            for v in [c.speed, c.opacity, c.volume_db, t.x, t.y, t.scale, t.rotation_deg] {
                assert!(v.is_finite(), "{c:?}");
            }
            assert!((0.0..=1.0).contains(&c.opacity) && (-60.0..=20.0).contains(&c.volume_db) && t.scale > 0.0, "{c:?}");
            if let ClipContent::Text(st) = &c.content {
                assert!(st.size.is_finite() && st.outline.is_none_or(|o| o.0.is_finite() && o.0 <= 20.0), "{st:?}");
            }
        }
        let back = Project::from_json(&p.to_json()).expect("Projekt lädt wieder");
        assert_eq!(back, p);
    }

    #[test]
    fn layers_follow_render_index_and_fonts_are_taken_over() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("clip.mp4");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=2", "-pix_fmt", "yuv420p"])
            .arg(&v)
            .status()
            .unwrap()
            .success());
        // Schriftdatei aus den mitgelieferten Cutline-Schriften
        let font = dir.path().join("Poppins-ExtraBold.ttf");
        std::fs::write(&font, include_bytes!("../../cl-app/assets/fonts/Poppins-ExtraBold.ttf")).unwrap();
        let content = serde_json::json!({"text": "Oben", "styles": [{"size": 15.0, "font": {"path": "C:/x/SystemFont/en.ttf"}}]}).to_string();
        let seg = |mid: &str, ri: i64| serde_json::json!({"material_id": mid, "render_index": ri, "target_timerange": {"start": 0, "duration": 1000000}, "source_timerange": {"start": 0, "duration": 1000000}});
        let draft = serde_json::json!({
            "canvas_config": {"width": 1920, "height": 1080},
            "fps": 30.0,
            "materials": {
                "videos": [{"id": "V1", "type": "video", "path": v.to_string_lossy()}, {"id": "V2", "type": "video", "path": v.to_string_lossy()}],
                "texts": [{"id": "T1", "content": content, "font_path": font.to_string_lossy()}]
            },
            "tracks": [
                {"type": "video", "segments": [seg("V1", 0)]},
                {"type": "text", "segments": [seg("T1", 14000)]},
                {"type": "video", "segments": [seg("V2", 1)]}
            ]
        });
        let (p, rep) = convert(&draft, dir.path(), "Ebenen").unwrap();
        let kinds: Vec<TrackKind> = p.timeline.tracks.iter().map(|t| t.kind).collect();
        // Hauptspur, darüber das Overlay-Video, ganz oben der Text
        assert_eq!(kinds, [TrackKind::Video, TrackKind::Video, TrackKind::Text]);
        assert!(p.timeline.tracks[0].main);
        let ClipContent::Text(st) = &p.timeline.tracks[2].clips[0].content else { panic!() };
        assert_eq!(st.font_family.as_deref(), Some("Poppins"));
        assert_eq!(rep.fonts, vec![font]);
    }

    #[test]
    fn text_size_shadow_animation_and_empty_stroke_color() {
        // leere Konturfarbe darf nicht abstürzen
        let content = serde_json::json!({"text": "Hi", "styles": [{"size": 15.0, "strokes": [{"width": 0.1, "content": {"solid": {"color": []}}}]}]}).to_string();
        let draft = serde_json::json!({
            "canvas_config": {"width": 1920, "height": 1080},
            "materials": {
                "texts": [{"id": "T1", "content": content, "has_shadow": false}],
                "material_animations": [{"id": "A1", "animations": [{"type": "in", "duration": 300000}]}]
            },
            "tracks": [{"type": "text", "segments": [{"material_id": "T1", "extra_material_refs": ["A1"], "target_timerange": {"start": 0, "duration": 1000000}}]}]
        });
        let (p, _) = convert(&draft, Path::new("."), "Text").unwrap();
        let c = &p.timeline.tracks[0].clips[0];
        let ClipContent::Text(st) = &c.content else { panic!() };
        assert!((st.size - 172.5).abs() < 1e-3, "{}", st.size);
        assert!(!st.shadow);
        assert_eq!(st.outline.unwrap().1, [0, 0, 0, 255]);
        assert_eq!(c.anim.intro.as_ref().unwrap().id, "fade");
    }

    /// Fehlende Datei (Platte nicht angesteckt): Clip und Medium bleiben, das Medium ist ein
    /// Platzhalter mit den Angaben des Materials, `missing` nennt die Datei.
    #[test]
    fn missing_file_keeps_clip_with_placeholder_media() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("fehlt.jpg");
        let seg = |mid: &str, at: i64| serde_json::json!({"material_id": mid, "target_timerange": {"start": at, "duration": 1_000_000}});
        let draft = serde_json::json!({
            "materials": {
                "videos": [{"id": "B", "type": "photo", "path": gone.to_string_lossy(), "width": 640, "height": 480, "duration": 5_000_000}],
                "audios": [{"id": "A", "type": "music", "path": dir.path().join("fehlt.wav").to_string_lossy(), "duration": 9_000_000}]
            },
            "tracks": [
                {"type": "video", "segments": [seg("B", 0), seg("B", 1_000_000)]},
                {"type": "audio", "segments": [seg("A", 0)]}
            ]
        });
        let (p, rep) = convert(&draft, dir.path(), "Fehlt").unwrap();
        assert_eq!(rep.missing.len(), 2);
        assert_eq!(rep.clips, 3);
        assert_eq!(p.media.len(), 2, "jede Datei einmal");
        let b = p.media.iter().find(|m| m.path == gone).unwrap();
        assert_eq!((b.kind, b.width, b.height, b.duration), (MediaKind::Image, 640, 480, Time(5_000_000)));
        assert_eq!(p.media.iter().find(|m| m.path != gone).unwrap().kind, MediaKind::Audio);
        assert_eq!(p.timeline.tracks.iter().map(|t| t.clips.len()).sum::<usize>(), 3);
    }

    /// 8 Stufen mit je 30 Segmenten auf denselben inneren Entwurf (30^8 Durchläufe, wenige KB):
    /// endet schnell mit Fehler statt Speicher und Zeit aufzubrauchen.
    #[test]
    fn nested_draft_bomb_stops_early() {
        let segs = |mid: &str| serde_json::json!([{"type": "video", "segments": (0..30).map(|_| serde_json::json!({"material_id": mid, "target_timerange": {"start": 0, "duration": 1000000}})).collect::<Vec<_>>()}]);
        let mut d = serde_json::json!({"materials": {}, "tracks": segs("leer")});
        for _ in 0..MAX_DEPTH {
            d = serde_json::json!({"materials": {"drafts": [{"id": "D", "draft": d}]}, "tracks": segs("D")});
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(convert(&d, Path::new("."), "Bombe").map(|_| ()));
        });
        // ohne Grenze liefe das tagelang: dann scheitert der Test hier statt zu hängen
        let r = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("Import endet nicht");
        assert_eq!(r.unwrap_err(), "Entwurf zu groß");
    }

    /// Echten Entwurf ansehen: CAPCUT_DRAFT=<Ordner> cargo test -- --ignored real_draft --nocapture
    #[test]
    #[ignore]
    fn real_draft() {
        let Some(d) = std::env::var_os("CAPCUT_DRAFT") else { return };
        let (p, rep) = import(std::path::Path::new(&d)).unwrap();
        for t in &p.timeline.tracks {
            let fonts: Vec<String> = t.clips.iter().filter_map(|c| match &c.content {
                ClipContent::Text(s) => s.font_family.clone(),
                _ => None,
            }).collect();
            println!("{:?} main={} clips={} fonts={fonts:?}", t.kind, t.main, t.clips.len());
        }
        println!("fonts: {:?}", rep.fonts);
        println!("clips={} texts={} skipped={:?} missing={:?}", rep.clips, rep.texts, rep.skipped, rep.missing);
        if let Some(out) = std::env::var_os("CAPCUT_OUT") {
            std::fs::write(out, p.to_json()).unwrap();
        }
    }

    #[test]
    fn converts_draft() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("clip.mp4");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=320x180:r=30:d=4",
                "-pix_fmt",
                "yuv420p"
            ])
            .arg(&v)
            .status()
            .unwrap()
            .success());
        let rec = dir.path().join("audio_record");
        std::fs::create_dir_all(&rec).unwrap();
        let a = rec.join("vo.wav");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=d=2"])
            .arg(&a)
            .status()
            .unwrap()
            .success());
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
        assert!(
            rep.skipped.iter().any(|(w, _)| w == "Sticker")
                && rep.skipped.iter().any(|(w, _)| w == "Effekte")
        );
        let main = &p.timeline.tracks[0];
        assert!(main.main);
        let c = &main.clips[0];
        assert_eq!(c.source_in, Time(500000));
        assert!((c.volume_db + 6.02).abs() < 0.1);
        assert!((c.transform.x - 270.0).abs() < 0.1 && (c.transform.y + 480.0).abs() < 0.1);
        assert!(c.fx.flip_h && (c.opacity - 0.8).abs() < 1e-3);
        let text = p
            .timeline
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Text)
            .unwrap();
        let ClipContent::Text(st) = &text.clips[0].content else {
            panic!()
        };
        assert_eq!(st.text, "Hallo Welt");
        assert_eq!(st.color, [255, 0, 0, 255]);
        assert_eq!(st.align, 0);
        assert_eq!(p.timeline.tracks.last().unwrap().kind, TrackKind::Audio);
    }

    /// Stummschaltung (Spur-Bit, `video_mute`, `last_nonzero_volume`), getrennter Originalton,
    /// Tonspur der Datei und Tonblenden – alle Kombinationen.
    #[test]
    fn mute_separated_sound_stream_and_fades() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("zwei.mp4");
        let ok = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=160x90:r=30:d=4", "-f", "lavfi", "-i", "sine=f=300:d=4"])
            .args(["-f", "lavfi", "-i", "sine=f=900:d=4", "-map", "0", "-map", "1", "-map", "2", "-c:a", "aac", "-pix_fmt", "yuv420p", "-shortest"])
            .arg(&v)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let seg = |mid: &str, start: i64, vol: f64, lnz: Option<f64>, refs: &[&str]| {
            let mut s = serde_json::json!({"material_id": mid, "target_timerange": {"start": start * 1_000_000, "duration": 1_000_000},
                "source_timerange": {"start": 0, "duration": 1_000_000}, "speed": 1.0, "volume": vol, "extra_material_refs": refs});
            if let Some(l) = lnz {
                s["last_nonzero_volume"] = l.into();
            }
            s
        };
        let path = v.to_string_lossy();
        let draft = |video_mute: bool, main_attr: i64| {
            serde_json::json!({
                "config": {"video_mute": video_mute},
                "materials": {
                    "videos": [{"id": "V", "type": "video", "path": path}, {"id": "VS", "type": "video", "path": path, "has_sound_separated": true}],
                    "audios": [{"id": "O", "type": "video_original_sound", "video_id": "VS", "path": path}],
                    "audio_track_indexes": [{"id": "I2", "audio_track_index": 2, "default_audio_track_index": 1}, {"id": "I1", "audio_track_index": 1, "default_audio_track_index": 1}],
                    "audio_fades": [{"id": "F", "fade_in_duration": 0, "fade_out_duration": 600_000}]
                },
                "tracks": [
                    {"type": "video", "attribute": main_attr, "segments": [seg("V", 0, 0.0, Some(0.8), &[]), seg("V", 1, 1.0, None, &[]), seg("VS", 2, 1.0, None, &["I2"])]},
                    {"type": "video", "attribute": 1, "segments": [seg("V", 0, 0.0, None, &[])]},
                    {"type": "audio", "attribute": 0, "segments": [seg("O", 2, 1.0, None, &["I2", "F"]), seg("V", 4, 0.0, Some(1.0), &[]), seg("V", 5, 0.5, None, &["I1"])]},
                    {"type": "audio", "attribute": 1, "segments": [seg("O", 8, 0.0, Some(1.0), &[])]},
                    {"type": "audio", "attribute": 0, "segments": [seg("O", 9, 0.0, None, &[])]}
                ]
            })
        };
        let vol = |c: &Clip| (c.volume_db * 10.0).round() / 10.0;
        // Hauptspur über das Spur-Bit stumm: Lautstärke aus `last_nonzero_volume`, Spur stumm
        let (p, rep) = convert(&draft(false, 1), dir.path(), "T").unwrap();
        assert_eq!(p.media[0].audio_streams, 2);
        let tr = &p.timeline.tracks;
        let main = tr.iter().find(|t| t.main).unwrap();
        assert!(main.muted);
        assert_eq!(main.clips.iter().map(vol).collect::<Vec<_>>(), [-1.9, 0.0, -60.0], "0,8 übernommen; getrennter Ton stumm wie „Ton extrahieren“");
        assert_eq!(main.clips[2].audio_stream, Some(1));
        // zweite Bildspur: stumm, Lautstärke 0 ohne `last_nonzero_volume` bleibt 0
        let v2 = tr.iter().find(|t| t.kind == TrackKind::Video && !t.main).unwrap();
        assert!(v2.muted && vol(&v2.clips[0]) == -60.0);
        // Tonspuren: stumme und hörbare werden nicht zusammengelegt
        let audio: Vec<&Track> = tr.iter().filter(|t| t.kind == TrackKind::Audio).collect();
        let loud = audio.iter().find(|t| !t.muted && t.clips.len() == 4).unwrap();
        let c = &loud.clips;
        assert_eq!((c[0].audio_stream, c[0].fade_in, c[0].fade_out), (Some(1), Time::ZERO, Time(600_000)));
        assert_eq!(vol(&c[1]), -60.0, "Clip-Lautstärke 0 auf hörbarer Spur bleibt 0");
        assert_eq!((vol(&c[2]), c[2].audio_stream), (-6.0, None), "Index 1 = erste Spur");
        assert_eq!(vol(&c[3]), -60.0);
        let muted = audio.iter().find(|t| t.muted).unwrap();
        assert_eq!(vol(&muted.clips[0]), 0.0);
        assert_eq!(rep.skipped.iter().find(|(w, _)| w == "Spur stumm übernommen").map(|x| x.1), Some(3));
        // `video_mute` schaltet die Hauptspur stumm, auch ohne Spur-Bit; ohne beides ist sie hörbar
        let (p, _) = convert(&draft(true, 0), dir.path(), "T").unwrap();
        let main = p.timeline.tracks.iter().find(|t| t.main).unwrap();
        assert!(main.muted && vol(&main.clips[0]) == -1.9);
        let (p, _) = convert(&draft(false, 0), dir.path(), "T").unwrap();
        let main = p.timeline.tracks.iter().find(|t| t.main).unwrap();
        assert!(!main.muted && vol(&main.clips[0]) == -60.0);
    }
}
