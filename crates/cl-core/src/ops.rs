use crate::fx::*;
use crate::model::*;
use crate::time::Time;
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Start,
    End,
}

/// Eine atomare, serialisierbare Änderung. UI, Plugins und MCP benutzen dieselben Ops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditOp {
    ImportMedia { media: MediaRef },
    AddTrack { kind: TrackKind },
    SetTrackFlags { track: TrackId, locked: bool, hidden: bool, muted: bool },
    /// `track = None`: passende Spur automatisch wählen (Video → Hauptspur).
    AddClip { track: Option<TrackId>, clip: Clip },
    RemoveClip { clip: ClipId },
    SplitClip { clip: ClipId, at: Time },
    TrimClip { clip: ClipId, edge: Edge, to: Time },
    MoveClip { clip: ClipId, track: Option<TrackId>, start: Time },
    SetText { clip: ClipId, style: TextStyle },
    SetTransform { clip: ClipId, transform: Transform },
    SetOpacity { clip: ClipId, opacity: f32 },
    SetVolume { clip: ClipId, volume_db: f32 },
    SetSettings { settings: ProjectSettings },
    AddProjectFont { path: PathBuf },
    Rename { name: String },
    /// Geschwindigkeit ändern; der genutzte Quellbereich bleibt, die Dauer passt sich an.
    SetSpeed { clip: ClipId, speed: f32 },
    SetFx { clip: ClipId, fx: ClipFx },
    SetKeyframes { clip: ClipId, keyframes: Keyframes },
    SetAnimations { clip: ClipId, anim: Animations },
    SetTransition { clip: ClipId, transition: Option<TransitionRef> },
    SetClipDisabled { clip: ClipId, disabled: bool },
    SetFades { clip: ClipId, fade_in: Time, fade_out: Time },
    /// Eigenschaften eines Clips ersetzen (Inhalt, Transform, Effekte, Keyframes …).
    /// Lage und Länge (Start, Dauer, Quellbeginn, Tempo) bleiben unverändert.
    ReplaceClip { clip: Clip },
    /// Neue leere Spur an Index (0 = unten) einfügen.
    InsertTrack { index: usize, kind: TrackKind },
    /// Spur samt Clips entfernen (nicht die Hauptspur).
    RemoveTrack { track: TrackId },
    /// Clip auf eine neue Spur an `index` verschieben (Ziehen ins Leere).
    MoveClipToNewTrack { clip: ClipId, index: usize, start: Time },
    /// Lücken schließen: eine Spur oder (None) alle Spuren.
    CloseGaps { track: Option<TrackId> },
    /// Lage und Quellbereich direkt setzen (Rollen, Slippen, Zahleneingabe), ohne Ripple
    SetClipTiming { clip: ClipId, start: Time, duration: Time, source_in: Time },
    /// Datei eines Mediums austauschen (verschoben/umbenannt)
    RelinkMedia { media: MediaId, path: PathBuf },
    /// Alle Marker ersetzen (setzen, verschieben, bearbeiten, löschen)
    SetMarkers { markers: Vec<crate::Marker> },
    /// Spur umbenennen (`None` = Standardname)
    RenameTrack { track: TrackId, name: Option<String> },
    /// Mehrere Ops als ein Undo-Schritt (Mehrfachauswahl).
    Batch { label: String, ops: Vec<EditOp> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum OpOutput {
    None,
    Clip(ClipId),
    Track(TrackId),
    Media(MediaId),
}

impl EditOp {
    /// Bezeichnung für den Undo-Verlauf.
    pub fn label(&self) -> &'static str {
        match self {
            EditOp::ImportMedia { .. } => "Medium importieren",
            EditOp::AddTrack { .. } => "Spur hinzufügen",
            EditOp::SetTrackFlags { .. } => "Spur-Eigenschaften",
            EditOp::AddClip { .. } => "Clip hinzufügen",
            EditOp::RemoveClip { .. } => "Clip löschen",
            EditOp::SplitClip { .. } => "Teilen",
            EditOp::TrimClip { .. } => "Trimmen",
            EditOp::MoveClip { .. } => "Clip verschieben",
            EditOp::SetText { .. } => "Text bearbeiten",
            EditOp::SetTransform { .. } => "Transformieren",
            EditOp::SetOpacity { .. } => "Deckkraft",
            EditOp::SetVolume { .. } => "Lautstärke",
            EditOp::SetSettings { .. } => "Projekteinstellungen",
            EditOp::AddProjectFont { .. } => "Projektschrift hinzufügen",
            EditOp::Rename { .. } => "Projekt umbenennen",
            EditOp::SetSpeed { .. } => "Geschwindigkeit",
            EditOp::SetFx { .. } => "Effekte/Anpassen",
            EditOp::SetKeyframes { .. } => "Keyframe",
            EditOp::SetAnimations { .. } => "Animation",
            EditOp::SetTransition { .. } => "Übergang",
            EditOp::SetClipDisabled { .. } => "Clip deaktivieren",
            EditOp::SetFades { .. } => "Ein-/Ausblenden",
            EditOp::ReplaceClip { .. } => "Clip bearbeiten",
            EditOp::InsertTrack { .. } => "Spur hinzufügen",
            EditOp::RemoveTrack { .. } => "Spur löschen",
            EditOp::MoveClipToNewTrack { .. } => "Auf neue Spur verschieben",
            EditOp::CloseGaps { .. } => "Lücken schließen",
            EditOp::SetMarkers { .. } => "Marker",
            EditOp::RelinkMedia { .. } => "Medium neu verknüpfen",
            EditOp::SetClipTiming { .. } => "Clip-Zeit",
            EditOp::RenameTrack { .. } => "Spur umbenennen",
            EditOp::Batch { ops, .. } => match ops.first() {
                Some(EditOp::RemoveClip { .. }) => "Clips löschen",
                Some(EditOp::MoveClip { .. }) => "Clips verschieben",
                Some(EditOp::AddClip { .. }) => "Clips einfügen",
                _ => "Mehrere Änderungen",
            },
        }
    }

    pub(crate) fn apply(self, p: &mut Project) -> Result<OpOutput> {
        match self {
            EditOp::ImportMedia { media } => {
                let id = media.id;
                if p.media(id).is_none() {
                    p.media.push(media);
                }
                Ok(OpOutput::Media(id))
            }
            EditOp::AddTrack { kind } => {
                let t = Track::new(kind);
                let id = t.id;
                p.timeline.tracks.push(t);
                Ok(OpOutput::Track(id))
            }
            EditOp::SetTrackFlags { track, locked, hidden, muted } => {
                let t = track_mut(p, track)?;
                t.locked = locked;
                t.hidden = hidden;
                t.muted = muted;
                Ok(OpOutput::None)
            }
            EditOp::AddClip { track, clip } => add_clip(p, track, clip),
            EditOp::SetClipTiming { clip, start, duration, source_in } => {
                if duration.0 <= 0 || start.0 < 0 || source_in.0 < 0 {
                    return Err(CoreError::InvalidDuration);
                }
                let c = clip_mut(p, clip)?;
                c.start = start;
                c.duration = duration;
                c.source_in = source_in;
                let magnet = p.settings.main_track_magnet;
                for t in &mut p.timeline.tracks {
                    t.sort();
                    // Hauptspur-Magnet: Lücken sofort schließen
                    if magnet && t.main && t.clips.iter().any(|c| c.id == clip) {
                        compact(t);
                    }
                }
                Ok(OpOutput::None)
            }
            EditOp::RelinkMedia { media, path } => {
                let m = p.media.iter_mut().find(|m| m.id == media).ok_or(CoreError::MediaNotFound(media))?;
                m.path = path;
                Ok(OpOutput::None)
            }
            EditOp::SetMarkers { mut markers } => {
                markers.sort_by_key(|m| m.at);
                p.timeline.markers = markers;
                Ok(OpOutput::None)
            }
            EditOp::RenameTrack { track, name } => {
                track_mut(p, track)?.name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
                Ok(OpOutput::None)
            }
            EditOp::RemoveClip { clip } => {
                let ti = clip_track_index(p, clip)?;
                ensure_unlocked(&p.timeline.tracks[ti])?;
                p.timeline.tracks[ti].clips.retain(|c| c.id != clip);
                after_change(p, ti);
                prune_if_empty(p, ti);
                Ok(OpOutput::None)
            }
            EditOp::SplitClip { clip, at } => {
                let ti = clip_track_index(p, clip)?;
                let track = &mut p.timeline.tracks[ti];
                ensure_unlocked(track)?;
                let c = track.clips.iter_mut().find(|c| c.id == clip).expect("geprüft");
                if at <= c.start || at >= c.end() {
                    return Err(CoreError::InvalidSplit);
                }
                let mut right = c.clone();
                right.id = ClipId::new();
                right.start = at;
                right.duration = c.end() - at;
                let rel = at - c.start;
                right.source_in = c.source_in + Time((rel.0 as f64 * c.speed.max(0.01) as f64).round() as i64);
                right.keyframes.shift(rel);
                // Ausblend-Animation und Übergang gehören zum rechten Teil, Einblenden zum linken
                right.anim.intro = None;
                c.anim.outro = None;
                c.transition_out = None;
                c.duration = rel;
                let rid = right.id;
                track.clips.push(right);
                track.sort();
                Ok(OpOutput::Clip(rid))
            }
            EditOp::TrimClip { clip, edge, to } => {
                let ti = clip_track_index(p, clip)?;
                let (src_len, is_audio) = {
                    let c = p.timeline.tracks[ti].clips.iter().find(|c| c.id == clip).expect("geprüft");
                    (source_length(p, c), c.kind() == TrackKind::Audio)
                };
                let track = &mut p.timeline.tracks[ti];
                ensure_unlocked(track)?;
                let c = track.clips.iter_mut().find(|c| c.id == clip).expect("geprüft");
                let speed = c.speed.max(0.01) as f64;
                let to_src = |d: Time| Time((d.0 as f64 * speed).round() as i64);
                let to_tl = |d: Time| Time((d.0 as f64 / speed).round() as i64);
                match edge {
                    Edge::Start => {
                        let mut to = Time(to.0.max(0));
                        if src_len.is_some() {
                            // Nicht vor den Quellanfang: still begrenzen
                            let earliest = c.start - to_tl(c.source_in);
                            if to < earliest {
                                to = earliest;
                            }
                        }
                        let delta = to - c.start;
                        let new_dur = c.duration - delta;
                        if new_dur <= Time::ZERO {
                            return Err(CoreError::InvalidDuration);
                        }
                        if src_len.is_some() {
                            c.source_in = Time((c.source_in + to_src(delta)).0.max(0));
                        }
                        c.keyframes.shift(delta);
                        c.start = to;
                        c.duration = new_dur;
                    }
                    Edge::End => {
                        let mut new_dur = to - c.start;
                        // Video: still auf das Quellende begrenzen. Audio darf länger
                        // werden; der Rest wird mit Stille aufgefüllt.
                        if let (Some(len), false) = (src_len, is_audio) {
                            let max = to_tl(len - c.source_in);
                            if new_dur > max {
                                new_dur = max;
                            }
                        }
                        if new_dur <= Time::ZERO {
                            return Err(CoreError::InvalidDuration);
                        }
                        c.duration = new_dur;
                    }
                }
                after_change(p, ti);
                Ok(OpOutput::None)
            }
            EditOp::MoveClip { clip, track, start } => {
                let ti = clip_track_index(p, clip)?;
                ensure_unlocked(&p.timeline.tracks[ti])?;
                let src_track = p.timeline.tracks[ti].id;
                let pos = p.timeline.tracks[ti].clips.iter().position(|c| c.id == clip).expect("geprüft");
                let mut c = p.timeline.tracks[ti].clips.remove(pos);
                c.start = Time(start.0.max(0));
                after_change(p, ti);
                let target = track.unwrap_or(src_track);
                let out = add_clip(p, Some(target), c)?;
                if target != src_track {
                    if let Some(ti) = p.timeline.tracks.iter().position(|t| t.id == src_track) {
                        prune_if_empty(p, ti);
                    }
                }
                Ok(out)
            }
            EditOp::SetSpeed { clip, speed } => {
                let ti = clip_track_index(p, clip)?;
                ensure_unlocked(&p.timeline.tracks[ti])?;
                let c = p.timeline.tracks[ti].clips.iter_mut().find(|c| c.id == clip).expect("geprüft");
                let speed = speed.clamp(0.1, 100.0);
                let span = c.source_span();
                let old = c.duration;
                c.speed = speed;
                c.duration = Time(((span.0 as f64 / speed as f64).round() as i64).max(1000));
                // Keyframes proportional mitziehen
                let f = c.duration.0 as f64 / old.0.max(1) as f64;
                for list in c.keyframes.0.values_mut() {
                    for k in list.iter_mut() {
                        k.t = Time((k.t.0 as f64 * f).round() as i64);
                    }
                }
                after_change(p, ti);
                resolve_overlaps(p, ti, clip);
                Ok(OpOutput::None)
            }
            EditOp::SetFx { clip, fx } => {
                clip_mut(p, clip)?.fx = fx;
                Ok(OpOutput::None)
            }
            EditOp::SetKeyframes { clip, keyframes } => {
                clip_mut(p, clip)?.keyframes = keyframes;
                Ok(OpOutput::None)
            }
            EditOp::SetAnimations { clip, anim } => {
                clip_mut(p, clip)?.anim = anim;
                Ok(OpOutput::None)
            }
            EditOp::SetTransition { clip, transition } => {
                let c = clip_mut(p, clip)?;
                c.transition_out = transition.map(|mut t| {
                    t.duration = Time(t.duration.0.clamp(100_000, 5_000_000));
                    t
                });
                Ok(OpOutput::None)
            }
            EditOp::SetClipDisabled { clip, disabled } => {
                clip_mut(p, clip)?.disabled = disabled;
                Ok(OpOutput::None)
            }
            EditOp::SetFades { clip, fade_in, fade_out } => {
                let c = clip_mut(p, clip)?;
                let half = Time(c.duration.0 / 2);
                c.fade_in = Time(fade_in.0.clamp(0, half.0));
                c.fade_out = Time(fade_out.0.clamp(0, half.0));
                Ok(OpOutput::None)
            }
            EditOp::ReplaceClip { clip } => {
                let c = clip_mut(p, clip.id)?;
                if c.kind() != clip.kind() {
                    return Err(CoreError::KindMismatch { clip: clip.kind(), track: c.kind() });
                }
                let (start, duration, source_in, speed) = (c.start, c.duration, c.source_in, c.speed);
                *c = clip;
                c.start = start;
                c.duration = duration;
                c.source_in = source_in;
                c.speed = speed;
                c.opacity = c.opacity.clamp(0.0, 1.0);
                c.volume_db = c.volume_db.clamp(-60.0, 20.0);
                Ok(OpOutput::None)
            }
            EditOp::InsertTrack { index, kind } => {
                let t = Track::new(kind);
                let id = t.id;
                let main_idx = p.timeline.tracks.iter().position(|t| t.main);
                let mut index = index.min(p.timeline.tracks.len());
                // Nichts unter die Hauptspur außer Audio (Audio liegt in der Anzeige darunter)
                if let (Some(mi), false) = (main_idx, kind == TrackKind::Audio) {
                    index = index.max(mi + 1);
                }
                p.timeline.tracks.insert(index, t);
                Ok(OpOutput::Track(id))
            }
            EditOp::RemoveTrack { track } => {
                let ti = p.timeline.tracks.iter().position(|t| t.id == track).ok_or(CoreError::TrackNotFound(track))?;
                let t = &p.timeline.tracks[ti];
                ensure_unlocked(t)?;
                if t.main {
                    // Hauptspur bleibt bestehen, wird nur geleert
                    p.timeline.tracks[ti].clips.clear();
                } else {
                    p.timeline.tracks.remove(ti);
                }
                Ok(OpOutput::None)
            }
            EditOp::MoveClipToNewTrack { clip, index, start } => {
                let ti = clip_track_index(p, clip)?;
                ensure_unlocked(&p.timeline.tracks[ti])?;
                let src_track = p.timeline.tracks[ti].id;
                let pos = p.timeline.tracks[ti].clips.iter().position(|c| c.id == clip).expect("geprüft");
                let mut c = p.timeline.tracks[ti].clips.remove(pos);
                c.start = Time(start.0.max(0));
                after_change(p, ti);
                let mut t = Track::new(c.kind());
                let tid = t.id;
                t.clips.push(c);
                let index = index.min(p.timeline.tracks.len());
                p.timeline.tracks.insert(index, t);
                if let Some(ti) = p.timeline.tracks.iter().position(|t| t.id == src_track) {
                    prune_if_empty(p, ti);
                }
                Ok(OpOutput::Track(tid))
            }
            EditOp::CloseGaps { track } => {
                let ids: Vec<TrackId> = match track {
                    Some(t) => vec![t],
                    None => p.timeline.tracks.iter().map(|t| t.id).collect(),
                };
                for id in ids {
                    let t = track_mut(p, id)?;
                    if t.locked {
                        continue;
                    }
                    t.sort();
                    compact(t);
                }
                Ok(OpOutput::None)
            }
            EditOp::Batch { ops, .. } => {
                let mut last = OpOutput::None;
                for op in ops {
                    let out = op.apply(p)?;
                    if out != OpOutput::None {
                        last = out;
                    }
                }
                Ok(last)
            }
            EditOp::SetText { clip, style } => {
                let c = clip_mut(p, clip)?;
                match &mut c.content {
                    ClipContent::Text(s) => {
                        *s = style;
                        Ok(OpOutput::None)
                    }
                    _ => Err(CoreError::KindMismatch { clip: c.kind(), track: TrackKind::Text }),
                }
            }
            EditOp::SetTransform { clip, transform } => {
                clip_mut(p, clip)?.transform = transform;
                Ok(OpOutput::None)
            }
            EditOp::SetOpacity { clip, opacity } => {
                clip_mut(p, clip)?.opacity = opacity.clamp(0.0, 1.0);
                Ok(OpOutput::None)
            }
            EditOp::SetVolume { clip, volume_db } => {
                clip_mut(p, clip)?.volume_db = volume_db.clamp(-60.0, 20.0);
                Ok(OpOutput::None)
            }
            EditOp::SetSettings { settings } => {
                let magnet_on = settings.main_track_magnet && !p.settings.main_track_magnet;
                p.settings = settings;
                if magnet_on {
                    if let Some(ti) = p.timeline.tracks.iter().position(|t| t.main) {
                        p.timeline.tracks[ti].sort();
                        compact(&mut p.timeline.tracks[ti]);
                    }
                }
                Ok(OpOutput::None)
            }
            EditOp::AddProjectFont { path } => {
                if !p.project_fonts.contains(&path) {
                    p.project_fonts.push(path);
                }
                Ok(OpOutput::None)
            }
            EditOp::Rename { name } => {
                p.name = name;
                Ok(OpOutput::None)
            }
        }
    }
}

fn source_length(p: &Project, c: &Clip) -> Option<Time> {
    match &c.content {
        ClipContent::Media { media, kind: MediaKind::Video | MediaKind::Audio } => {
            p.media(*media).map(|m| m.duration).filter(|d| *d > Time::ZERO)
        }
        _ => None,
    }
}

fn ensure_unlocked(t: &Track) -> Result<()> {
    if t.locked {
        Err(CoreError::TrackLocked)
    } else {
        Ok(())
    }
}

fn track_mut(p: &mut Project, id: TrackId) -> Result<&mut Track> {
    p.timeline.tracks.iter_mut().find(|t| t.id == id).ok_or(CoreError::TrackNotFound(id))
}

fn clip_track_index(p: &Project, id: ClipId) -> Result<usize> {
    p.timeline
        .tracks
        .iter()
        .position(|t| t.clips.iter().any(|c| c.id == id))
        .ok_or(CoreError::ClipNotFound(id))
}

fn clip_mut(p: &mut Project, id: ClipId) -> Result<&mut Clip> {
    let ti = clip_track_index(p, id)?;
    ensure_unlocked(&p.timeline.tracks[ti])?;
    Ok(p.timeline.tracks[ti].clips.iter_mut().find(|c| c.id == id).expect("geprüft"))
}

/// Hauptspur lückenlos zusammenschieben (Ripple, Hauptspur-Magnet).
/// Die Reihenfolge im Vektor bleibt erhalten (sie ist die Schnittreihenfolge).
fn compact(t: &mut Track) {
    let mut cursor = Time::ZERO;
    for c in &mut t.clips {
        c.start = cursor;
        cursor = c.end();
    }
}

/// Leere Nebenspur entfernen (Spuren verschwinden, wenn der letzte Clip geht).
fn prune_if_empty(p: &mut Project, ti: usize) {
    if ti < p.timeline.tracks.len() {
        let t = &p.timeline.tracks[ti];
        if !t.main && t.clips.is_empty() {
            p.timeline.tracks.remove(ti);
        }
    }
}

/// Nach einer Längenänderung auf Nebenspuren: nachfolgende Clips, die jetzt
/// überlappen, nach hinten schieben.
fn resolve_overlaps(p: &mut Project, ti: usize, _changed: ClipId) {
    let t = &mut p.timeline.tracks[ti];
    if t.main {
        return;
    }
    t.sort();
    let mut cursor = Time::ZERO;
    for c in &mut t.clips {
        if c.start < cursor {
            c.start = cursor;
        }
        cursor = c.end();
    }
}

fn after_change(p: &mut Project, ti: usize) {
    let magnet = p.settings.main_track_magnet;
    let t = &mut p.timeline.tracks[ti];
    if t.main && magnet {
        compact(t);
    } else {
        t.sort();
    }
}

fn add_clip(p: &mut Project, track: Option<TrackId>, clip: Clip) -> Result<OpOutput> {
    if clip.duration <= Time::ZERO {
        return Err(CoreError::InvalidDuration);
    }
    if let ClipContent::Media { media, .. } = &clip.content {
        if p.media(*media).is_none() {
            return Err(CoreError::MediaNotFound(*media));
        }
    }
    let kind = clip.kind();
    let id = clip.id;

    let ti = match track {
        Some(tid) => {
            let ti = p.timeline.tracks.iter().position(|t| t.id == tid).ok_or(CoreError::TrackNotFound(tid))?;
            let t = &p.timeline.tracks[ti];
            if t.kind != kind {
                return Err(CoreError::KindMismatch { clip: kind, track: t.kind });
            }
            ensure_unlocked(t)?;
            let magnet_main = t.main && p.settings.main_track_magnet;
            if magnet_main || t.is_free(clip.start, clip.duration, None) {
                ti
            } else {
                // Belegt: eine neue Spur darüber anlegen.
                insert_track_above(p, ti, kind)
            }
        }
        None if kind == TrackKind::Video => match p.timeline.tracks.iter().position(|t| t.main) {
            Some(ti) => {
                let magnet = p.settings.main_track_magnet;
                if magnet || p.timeline.tracks[ti].is_free(clip.start, clip.duration, None) {
                    ti
                } else {
                    insert_track_above(p, ti, kind)
                }
            }
            None => {
                let mut t = Track::new(TrackKind::Video);
                t.main = true;
                p.timeline.tracks.insert(0, t);
                0
            }
        },
        None => match p
            .timeline
            .tracks
            .iter()
            .position(|t| t.kind == kind && !t.locked && t.is_free(clip.start, clip.duration, None))
        {
            Some(ti) => ti,
            None => {
                p.timeline.tracks.push(Track::new(kind));
                p.timeline.tracks.len() - 1
            }
        },
    };

    let magnet_main = p.timeline.tracks[ti].main && p.settings.main_track_magnet;
    let t = &mut p.timeline.tracks[ti];
    if magnet_main {
        // Einfügen vor dem ersten Clip, dessen Mitte hinter der Ablageposition liegt
        let at = clip.start.0;
        let idx = t.clips.iter().take_while(|c| (c.start.0 + c.end().0) / 2 < at).count();
        t.clips.insert(idx, clip);
        compact(t);
    } else {
        t.clips.push(clip);
        t.sort();
    }
    Ok(OpOutput::Clip(id))
}

fn insert_track_above(p: &mut Project, ti: usize, kind: TrackKind) -> usize {
    p.timeline.tracks.insert(ti + 1, Track::new(kind));
    ti + 1
}
