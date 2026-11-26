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

/// An atomic, serializable change. UI, plugins and MCP use the same ops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditOp {
    ImportMedia {
        media: MediaRef,
    },
    AddTrack {
        kind: TrackKind,
    },
    SetTrackFlags {
        track: TrackId,
        locked: bool,
        hidden: bool,
        muted: bool,
    },
    /// Display of the track in the timeline: collapsed, height factor (saved with the project)
    SetTrackView {
        track: TrackId,
        collapsed: bool,
        height: f32,
    },
    /// `track = None`: choose a fitting track automatically (video → main track).
    AddClip {
        track: Option<TrackId>,
        clip: Clip,
    },
    RemoveClip {
        clip: ClipId,
    },
    SplitClip {
        clip: ClipId,
        at: Time,
    },
    TrimClip {
        clip: ClipId,
        edge: Edge,
        to: Time,
    },
    MoveClip {
        clip: ClipId,
        track: Option<TrackId>,
        start: Time,
    },
    SetText {
        clip: ClipId,
        style: TextStyle,
    },
    SetTransform {
        clip: ClipId,
        transform: Transform,
    },
    /// Set the crop (clamped, see `Crop::clamped`).
    SetCrop {
        clip: ClipId,
        crop: Crop,
    },
    SetOpacity {
        clip: ClipId,
        opacity: f32,
    },
    SetVolume {
        clip: ClipId,
        volume_db: f32,
    },
    SetSettings {
        settings: ProjectSettings,
    },
    AddProjectFont {
        path: PathBuf,
    },
    Rename {
        name: String,
    },
    /// Change speed; the used source range stays, the duration adapts.
    SetSpeed {
        clip: ClipId,
        speed: f32,
    },
    SetFx {
        clip: ClipId,
        fx: ClipFx,
    },
    SetKeyframes {
        clip: ClipId,
        keyframes: Keyframes,
    },
    SetAnimations {
        clip: ClipId,
        anim: Animations,
    },
    SetTransition {
        clip: ClipId,
        transition: Option<TransitionRef>,
    },
    SetClipDisabled {
        clip: ClipId,
        disabled: bool,
    },
    SetFades {
        clip: ClipId,
        fade_in: Time,
        fade_out: Time,
    },
    /// Replace a clip's properties (content, transform, effects, keyframes …).
    /// Position and length (start, duration, source in, speed) stay unchanged.
    ReplaceClip {
        clip: Clip,
    },
    /// Insert a new empty track at index (0 = bottom).
    InsertTrack {
        index: usize,
        kind: TrackKind,
    },
    /// Remove a track with its clips (not the main track).
    RemoveTrack {
        track: TrackId,
    },
    /// Move a track one place up/down in the display (within visual or audio tracks,
    /// the main track stays at the bottom). At the very top/bottom: no change.
    MoveTrack {
        track: TrackId,
        up: bool,
    },
    /// Move a clip to a new track at `index` (drag into empty space).
    MoveClipToNewTrack {
        clip: ClipId,
        index: usize,
        start: Time,
    },
    /// Close gaps: one track or (None) all tracks.
    CloseGaps {
        track: Option<TrackId>,
    },
    /// Set position and source range directly (roll, slip, numeric entry), without ripple
    SetClipTiming {
        clip: ClipId,
        start: Time,
        duration: Time,
        source_in: Time,
    },
    /// Replace the file of a media item (moved/renamed)
    RelinkMedia {
        media: MediaId,
        path: PathBuf,
    },
    /// Replace all markers (set, move, edit, delete)
    SetMarkers {
        markers: Vec<crate::Marker>,
    },
    /// Rename a track (`None` = default name)
    RenameTrack {
        track: TrackId,
        name: Option<String>,
    },
    /// Remove a time range and pull everything after it left (ripple). With
    /// `all_tracks` on all unlocked tracks (clips at the edge are split), otherwise
    /// only on `track`. Markers after it move along, markers inside the range fall to `from`.
    RippleRange {
        track: TrackId,
        from: Time,
        to: Time,
        all_tracks: bool,
    },
    /// Several ops as one undo step (multi-selection).
    Batch {
        label: String,
        ops: Vec<EditOp>,
    },
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
    /// Label for the undo history (a given batch name takes precedence).
    pub fn label(&self) -> String {
        let l: &str = match self {
            EditOp::ImportMedia { .. } => "Medium importieren",
            EditOp::AddTrack { .. } => "Spur hinzufügen",
            EditOp::SetTrackFlags { .. } => "Spur-Eigenschaften",
            EditOp::SetTrackView { .. } => "Spuransicht",
            EditOp::AddClip { .. } => "Clip hinzufügen",
            EditOp::RemoveClip { .. } => "Clip löschen",
            EditOp::SplitClip { .. } => "Teilen",
            EditOp::TrimClip { .. } => "Trimmen",
            EditOp::MoveClip { .. } => "Clip verschieben",
            EditOp::SetText { .. } => "Text bearbeiten",
            EditOp::SetTransform { .. } => "Transformieren",
            EditOp::SetCrop { .. } => "Zuschnitt",
            EditOp::SetOpacity { .. } => "Deckkraft",
            EditOp::SetVolume { .. } => "Lautstärke",
            EditOp::SetSettings { .. } => "Projekteinstellungen",
            EditOp::AddProjectFont { .. } => "Projektschrift hinzufügen",
            EditOp::Rename { .. } => "Projekt umbenennen",
            EditOp::SetSpeed { .. } => "Tempo",
            EditOp::SetFx { .. } => "Effekte/Anpassen",
            EditOp::SetKeyframes { .. } => "Keyframe",
            EditOp::SetAnimations { .. } => "Animation",
            EditOp::SetTransition { .. } => "Übergang",
            EditOp::SetClipDisabled { .. } => "Clip deaktivieren",
            EditOp::SetFades { .. } => "Ein-/Ausblenden",
            EditOp::ReplaceClip { .. } => "Clip bearbeiten",
            EditOp::InsertTrack { .. } => "Spur hinzufügen",
            EditOp::RemoveTrack { .. } => "Spur löschen",
            EditOp::MoveTrack { .. } => "Spur verschieben",
            EditOp::MoveClipToNewTrack { .. } => "Auf neue Spur verschieben",
            EditOp::CloseGaps { .. } => "Lücken schließen",
            EditOp::SetMarkers { .. } => "Marker",
            EditOp::RelinkMedia { .. } => "Medium neu verknüpfen",
            EditOp::SetClipTiming { .. } => "Clip-Zeit",
            EditOp::RenameTrack { .. } => "Spur umbenennen",
            EditOp::RippleRange { .. } => "Bereich entfernen",
            EditOp::Batch { label, .. } if !label.is_empty() => label,
            EditOp::Batch { ops, .. } => match ops.first() {
                Some(EditOp::RemoveClip { .. }) => "Clips löschen",
                Some(EditOp::MoveClip { .. }) => "Clips verschieben",
                Some(EditOp::AddClip { .. }) => "Clips einfügen",
                Some(EditOp::RippleRange { .. }) => "Bereich entfernen",
                _ => "Mehrere Änderungen",
            },
        };
        l.to_string()
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
            EditOp::SetTrackFlags {
                track,
                locked,
                hidden,
                muted,
            } => {
                let t = track_mut(p, track)?;
                t.locked = locked;
                t.hidden = hidden;
                t.muted = muted;
                Ok(OpOutput::None)
            }
            EditOp::SetTrackView { track, collapsed, height } => {
                let t = track_mut(p, track)?;
                t.collapsed = collapsed;
                // values from outside (MCP) don't make an invisible or huge track
                t.height = height.clamp(0.5, 3.0);
                Ok(OpOutput::None)
            }
            EditOp::AddClip { track, clip } => add_clip(p, track, clip, None, None),
            EditOp::SetClipTiming {
                clip,
                start,
                duration,
                source_in,
            } => {
                if duration.0 <= 0
                    || start.0 < 0
                    || source_in.0 < 0
                    || start > Time::LIMIT
                    || duration > Time::LIMIT
                {
                    return Err(CoreError::InvalidDuration);
                }
                let len = p.find_clip(clip).and_then(|(_, c)| source_length(p, c));
                let c = clip_mut(p, clip)?;
                c.start = start;
                c.duration = duration;
                c.source_in = source_in;
                clamp_to_source(c, len);
                c.clamp_fades();
                let (start, duration) = (c.start, c.duration);
                let magnet = p.settings.main_track_magnet;
                // without magnet not into the neighbour (like trimming; rolling shortens first)
                let t = &p.timeline.tracks[clip_track_index(p, clip)?];
                if !(t.main && magnet) && !t.is_free(start, duration, Some(clip)) {
                    return Err(CoreError::InvalidDuration);
                }
                for t in &mut p.timeline.tracks {
                    t.sort();
                    // main track magnet: close gaps immediately
                    if magnet && t.main && t.clips.iter().any(|c| c.id == clip) {
                        compact(t);
                    }
                }
                Ok(OpOutput::None)
            }
            EditOp::RelinkMedia { media, path } => {
                let m = p
                    .media
                    .iter_mut()
                    .find(|m| m.id == media)
                    .ok_or(CoreError::MediaNotFound(media))?;
                m.path = path;
                Ok(OpOutput::None)
            }
            EditOp::SetMarkers { mut markers } => {
                markers.sort_by_key(|m| m.at);
                p.timeline.markers = markers;
                Ok(OpOutput::None)
            }
            EditOp::RenameTrack { track, name } => {
                track_mut(p, track)?.name =
                    name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
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
                let c = track
                    .clips
                    .iter_mut()
                    .find(|c| c.id == clip)
                    .expect("geprüft");
                if at <= c.start || at >= c.end() {
                    return Err(CoreError::InvalidSplit);
                }
                // Karaoke: both parts show the whole line, the highlight continues across the
                // cut
                c.retime_words(|t| t);
                let mut right = c.clone();
                right.id = ClipId::new();
                right.start = at;
                right.duration = c.end() - at;
                let rel = at - c.start;
                let rel_src = Time((rel.0 as f64 * c.speed.max(0.01) as f64).round() as i64);
                if c.fx.speed_curve.is_some() {
                    // shortcut: a curve becomes constant speed per part on split, upgrade when curves should be splittable
                    let (s0, s1, s_at) = (c.source_in, c.source_in + c.source_span(), c.source_time(at));
                    // forward: left [s0, s_at), right [s_at, s1); reversed the other way round
                    let (l_src, r_src) = if c.fx.reverse { ((s_at, s1), (s0, s_at)) } else { ((s0, s_at), (s_at, s1)) };
                    let tempo = |src: (Time, Time), d: Time| {
                        ((src.1 - src.0).0 as f64 / d.0.max(1) as f64).clamp(MIN_SPEED as f64, MAX_SPEED as f64) as f32
                    };
                    c.fx.speed_curve = None;
                    right.fx.speed_curve = None;
                    c.speed = tempo(l_src, rel);
                    right.speed = tempo(r_src, right.duration);
                    c.source_in = l_src.0;
                    right.source_in = r_src.0;
                } else if c.fx.reverse {
                    // reversed: the left part shows the end of the source range
                    right.source_in = c.source_in;
                    c.source_in = c.source_in + c.source_span() - rel_src;
                } else {
                    right.source_in = c.source_in + rel_src;
                }
                right.keyframes.shift(rel);
                right.retime_words(|t| t - rel);
                // out animation and transition belong to the right part, fade-in to the left
                right.anim.intro = None;
                c.anim.outro = None;
                c.transition_out = None;
                // fades only at the outer edges, otherwise a fade longer than half
                right.fade_in = Time::ZERO;
                c.fade_out = Time::ZERO;
                c.duration = rel;
                let rid = right.id;
                track.clips.push(right);
                track.sort();
                Ok(OpOutput::Clip(rid))
            }
            EditOp::TrimClip { clip, edge, to } => {
                let ti = clip_track_index(p, clip)?;
                let (src_len, is_audio, has_source, prev_end, next_start) = {
                    let t = &p.timeline.tracks[ti];
                    let c = t.clips.iter().find(|c| c.id == clip).expect("geprüft");
                    // without magnet don't extend into the neighbouring clip (overlap otherwise)
                    let free = !(t.main && p.settings.main_track_magnet);
                    let others = t.clips.iter().filter(|o| free && o.id != clip);
                    let prev_end = others.clone().filter(|o| o.start < c.start).map(|o| o.end()).max();
                    let next_start = others.filter(|o| o.start >= c.start).map(|o| o.start).min();
                    // by content instead of known length (duration 0 = unknown)
                    let has_source = matches!(
                        c.content,
                        ClipContent::Media { kind: MediaKind::Video | MediaKind::Audio, .. }
                    );
                    (source_length(p, c), c.kind() == TrackKind::Audio, has_source, prev_end, next_start)
                };
                let track = &mut p.timeline.tracks[ti];
                ensure_unlocked(track)?;
                let c = track
                    .clips
                    .iter_mut()
                    .find(|c| c.id == clip)
                    .expect("geprüft");
                let speed = c.speed.max(0.01) as f64;
                let to_src = |d: Time| Time((d.0 as f64 * speed).round() as i64);
                let to_tl = |d: Time| Time((d.0 as f64 / speed).round() as i64);
                // reversed, the source runs from the end of the range to `source_in`:
                // the clip start then hangs on the range end, the clip end on `source_in`.
                let rev = c.fx.reverse;
                match edge {
                    Edge::Start => {
                        let mut to = Time(to.0.clamp(0, Time::LIMIT.0));
                        if let Some(len) = src_len {
                            // not past the source: clamp silently
                            let room = if rev {
                                Time((len - c.source_in - c.source_span()).0.max(0))
                            } else {
                                c.source_in
                            };
                            let earliest = c.start - to_tl(room);
                            if to < earliest {
                                to = earliest;
                            }
                        }
                        if let Some(e) = prev_end.filter(|e| to < *e) {
                            to = e;
                        }
                        let delta = to - c.start;
                        let new_dur = c.duration - delta;
                        if new_dur <= Time::ZERO {
                            return Err(CoreError::InvalidDuration);
                        }
                        if has_source && !rev {
                            c.source_in = Time((c.source_in + to_src(delta)).0.max(0));
                        }
                        c.keyframes.shift(delta);
                        c.retime_words(|t| t - delta);
                        c.start = to;
                        c.duration = new_dur;
                    }
                    Edge::End => {
                        let mut new_dur = Time(to.0.min(Time::LIMIT.0)) - c.start;
                        // video: silently clamp to the source end. Audio may get
                        // longer; the rest is filled with silence.
                        if rev && src_len.is_some() {
                            let max = c.duration + to_tl(c.source_in);
                            if new_dur > max {
                                new_dur = max;
                            }
                        } else if let (Some(len), false) = (src_len, is_audio) {
                            let max = to_tl(len - c.source_in);
                            if new_dur > max {
                                new_dur = max;
                            }
                        }
                        if let Some(n) = next_start {
                            new_dur = new_dur.min(n - c.start);
                        }
                        if new_dur <= Time::ZERO {
                            return Err(CoreError::InvalidDuration);
                        }
                        let old_span = c.source_span();
                        c.duration = new_dur;
                        if rev && src_len.is_some() {
                            c.source_in = Time((c.source_in + old_span - c.source_span()).0.max(0));
                        }
                    }
                }
                c.clamp_fades();
                after_change(p, ti);
                Ok(OpOutput::None)
            }
            EditOp::MoveClip { clip, track, start } => {
                let ti = clip_track_index(p, clip)?;
                ensure_unlocked(&p.timeline.tracks[ti])?;
                let src_track = p.timeline.tracks[ti].id;
                let pos = p.timeline.tracks[ti]
                    .clips
                    .iter()
                    .position(|c| c.id == clip)
                    .expect("geprüft");
                let mut c = p.timeline.tracks[ti].clips.remove(pos);
                c.start = Time(start.0.max(0));
                after_change(p, ti);
                let target = track.unwrap_or(src_track);
                let out = add_clip(p, Some(target), c, Some(src_track), None)?;
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
                let c = p.timeline.tracks[ti]
                    .clips
                    .iter_mut()
                    .find(|c| c.id == clip)
                    .expect("geprüft");
                let speed = speed.clamp(MIN_SPEED, MAX_SPEED);
                let span = c.source_span();
                let old = c.duration;
                c.speed = speed;
                let new = Time(((span.0 as f64 / speed as f64).round() as i64).clamp(1000, Time::LIMIT.0));
                // pull keyframes and word times along proportionally
                let f = new.0 as f64 / old.0.max(1) as f64;
                c.retime_words(|t| Time((t.0 as f64 * f).round() as i64));
                c.duration = new;
                for list in c.keyframes.0.values_mut() {
                    for k in list.iter_mut() {
                        k.t = Time((k.t.0 as f64 * f).round() as i64);
                    }
                }
                c.clamp_fades();
                after_change(p, ti);
                resolve_overlaps(p, ti);
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
            EditOp::SetFades {
                clip,
                fade_in,
                fade_out,
            } => {
                let c = clip_mut(p, clip)?;
                c.fade_in = fade_in;
                c.fade_out = fade_out;
                c.clamp_fades();
                Ok(OpOutput::None)
            }
            EditOp::ReplaceClip { clip } => {
                if let ClipContent::Media { media, .. } = &clip.content {
                    if p.media(*media).is_none() {
                        return Err(CoreError::MediaNotFound(*media));
                    }
                }
                let c = clip_mut(p, clip.id)?;
                if c.kind() != clip.kind() {
                    return Err(CoreError::KindMismatch {
                        clip: clip.kind(),
                        track: c.kind(),
                    });
                }
                let (start, duration, source_in, speed) =
                    (c.start, c.duration, c.source_in, c.speed);
                *c = clip;
                c.start = start;
                c.duration = duration;
                c.source_in = source_in;
                c.speed = speed;
                c.opacity = c.opacity.clamp(0.0, 1.0);
                c.volume_db = c.volume_db.clamp(-60.0, 20.0);
                c.crop = c.crop.clamped();
                Ok(OpOutput::None)
            }
            EditOp::InsertTrack { index, kind } => {
                let t = Track::new(kind);
                let id = t.id;
                let main_idx = p.timeline.tracks.iter().position(|t| t.main);
                let mut index = index.min(p.timeline.tracks.len());
                // nothing below the main track except audio (audio lies below it in the display)
                if let (Some(mi), false) = (main_idx, kind == TrackKind::Audio) {
                    index = index.max(mi + 1);
                }
                p.timeline.tracks.insert(index, t);
                Ok(OpOutput::Track(id))
            }
            EditOp::MoveTrack { track, up } => {
                let tracks = &mut p.timeline.tracks;
                let i = tracks
                    .iter()
                    .position(|t| t.id == track)
                    .ok_or(CoreError::TrackNotFound(track))?;
                let audio = tracks[i].kind == TrackKind::Audio;
                // display: visual tracks with a higher index lie on top, audio tracks with a lower one
                let peer = |t: &Track| (t.kind == TrackKind::Audio) == audio && !t.main;
                if peer(&tracks[i]) {
                    let j = if up != audio {
                        (i + 1..tracks.len()).find(|&j| peer(&tracks[j]))
                    } else {
                        (0..i).rev().find(|&j| peer(&tracks[j]))
                    };
                    if let Some(j) = j {
                        tracks.swap(i, j);
                    }
                }
                Ok(OpOutput::None)
            }
            EditOp::RemoveTrack { track } => {
                let ti = p
                    .timeline
                    .tracks
                    .iter()
                    .position(|t| t.id == track)
                    .ok_or(CoreError::TrackNotFound(track))?;
                let t = &p.timeline.tracks[ti];
                ensure_unlocked(t)?;
                if t.main {
                    // main track stays, is only emptied
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
                let pos = p.timeline.tracks[ti]
                    .clips
                    .iter()
                    .position(|c| c.id == clip)
                    .expect("geprüft");
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
            EditOp::RippleRange {
                track,
                from,
                to,
                all_tracks,
            } => {
                if to <= from || from.0 < 0 {
                    return Err(CoreError::InvalidDuration);
                }
                ensure_unlocked(track_mut(p, track)?)?;
                // markers follow the main track (or everything); cuts only on secondary tracks leave them
                let move_markers = all_tracks || track_mut(p, track)?.main;
                let ids: Vec<TrackId> = if all_tracks {
                    p.timeline.tracks.iter().filter(|t| !t.locked).map(|t| t.id).collect()
                } else {
                    vec![track]
                };
                let d = to - from;
                for id in ids {
                    // split clips at the range bounds, then everything lies fully inside or outside
                    for at in [from, to] {
                        let hit = track_mut(p, id)?
                            .clips
                            .iter()
                            .find(|c| c.start < at && at < c.end())
                            .map(|c| c.id);
                        if let Some(clip) = hit {
                            EditOp::SplitClip { clip, at }.apply(p)?;
                        }
                    }
                    let ti = p.timeline.tracks.iter().position(|t| t.id == id).expect("geprüft");
                    let t = &mut p.timeline.tracks[ti];
                    let had = !t.clips.is_empty();
                    t.clips.retain(|c| c.start < from || c.start >= to);
                    for c in t.clips.iter_mut().filter(|c| c.start >= to) {
                        c.start = c.start - d;
                    }
                    after_change(p, ti);
                    if had {
                        prune_if_empty(p, ti);
                    }
                }
                for m in p.timeline.markers.iter_mut().filter(|_| move_markers) {
                    if m.at >= to {
                        m.at = m.at - d;
                    } else if m.at > from {
                        m.at = from;
                    }
                }
                Ok(OpOutput::None)
            }
            EditOp::Batch { ops, .. } => {
                let mut last = OpOutput::None;
                // ids of all clips while AddClips follow each other: thousands of captions stay linear
                let mut ids: Option<std::collections::HashSet<ClipId>> = None;
                for op in ops {
                    let out = match op {
                        EditOp::AddClip { track, clip } => {
                            let ids = ids.get_or_insert_with(|| {
                                p.timeline.tracks.iter().flat_map(|t| &t.clips).filter(|_| visit()).map(|c| c.id).collect()
                            });
                            add_clip(p, track, clip, None, Some(ids))?
                        }
                        op => {
                            ids = None;
                            op.apply(p)?
                        }
                    };
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
                    _ => Err(CoreError::KindMismatch {
                        clip: c.kind(),
                        track: TrackKind::Text,
                    }),
                }
            }
            EditOp::SetTransform { clip, transform } => {
                clip_mut(p, clip)?.transform = transform;
                Ok(OpOutput::None)
            }
            EditOp::SetCrop { clip, crop } => {
                clip_mut(p, clip)?.crop = crop.clamped();
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
                if !settings.valid() {
                    return Err(CoreError::InvalidValue);
                }
                let magnet_on = settings.main_track_magnet && !p.settings.main_track_magnet;
                p.settings = settings;
                if magnet_on {
                    if let Some(ti) = p.timeline.tracks.iter().position(|t| t.main && !t.locked) {
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

/// Gaps between clips (from 0) as (from, to), clips in any order.
pub fn gaps<'a>(clips: impl IntoIterator<Item = &'a Clip>) -> Vec<(Time, Time)> {
    let mut iv: Vec<(Time, Time)> = clips.into_iter().map(|c| (c.start, c.end())).collect();
    iv.sort();
    let mut out = Vec::new();
    let mut cursor = Time::ZERO;
    for (a, b) in iv {
        if a > cursor {
            out.push((cursor, a));
        }
        cursor = cursor.max(b);
    }
    out
}

/// Ripple ops for several ranges (track, from, to): overlapping ones are merged
/// (across tracks with `all_tracks`) and removed back to front so earlier
/// ranges keep their position.
pub fn ripple_ops(ranges: Vec<(TrackId, Time, Time)>, all_tracks: bool) -> Vec<EditOp> {
    let mut r: Vec<(TrackId, Time, Time)> = ranges.into_iter().filter(|r| r.2 > r.1).collect();
    r.sort_by_key(|r| r.1);
    let mut merged: Vec<(TrackId, Time, Time)> = Vec::new();
    for (t, a, b) in r {
        match merged
            .iter_mut()
            .rev()
            .find(|m| all_tracks || m.0 == t)
            .filter(|m| a <= m.2)
        {
            Some(m) => m.2 = m.2.max(b),
            None => merged.push((t, a, b)),
        }
    }
    merged
        .into_iter()
        .rev()
        .map(|(track, from, to)| EditOp::RippleRange {
            track,
            from,
            to,
            all_tracks,
        })
        .collect()
}

fn source_length(p: &Project, c: &Clip) -> Option<Time> {
    match &c.content {
        ClipContent::Media {
            media,
            kind: MediaKind::Video | MediaKind::Audio,
        } => p
            .media(*media)
            .map(|m| m.duration)
            .filter(|d| *d > Time::ZERO),
        _ => None,
    }
}

/// Video/audio: duration at most up to the source end from `source_in` (speed respected).
fn clamp_to_source(c: &mut Clip, len: Option<Time>) {
    if let Some(len) = len {
        let max = Time(((len - c.source_in).0 as f64 / c.speed.max(0.01) as f64).floor() as i64);
        if max > Time::ZERO && c.duration > max {
            c.duration = max;
        }
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
    p.timeline
        .tracks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or(CoreError::TrackNotFound(id))
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
    Ok(p.timeline.tracks[ti]
        .clips
        .iter_mut()
        .find(|c| c.id == id)
        .expect("geprüft"))
}

/// Push the main track together without gaps (ripple, main track magnet).
/// The order in the vector is kept (it is the cut order).
fn compact(t: &mut Track) {
    let mut cursor = Time::ZERO;
    for c in &mut t.clips {
        c.start = cursor;
        cursor = c.end();
    }
}

/// Remove an empty secondary track (tracks vanish when the last clip goes).
fn prune_if_empty(p: &mut Project, ti: usize) {
    if ti < p.timeline.tracks.len() {
        let t = &p.timeline.tracks[ti];
        if !t.main && t.clips.is_empty() {
            p.timeline.tracks.remove(ti);
        }
    }
}

/// After a length change on secondary tracks: push following clips that now
/// overlap to the back.
fn resolve_overlaps(p: &mut Project, ti: usize) {
    let magnet = p.settings.main_track_magnet;
    let t = &mut p.timeline.tracks[ti];
    // main track with magnet is already pushed together without gaps; without magnet like secondary tracks
    if t.main && magnet {
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

/// `Track::is_free` in O(log n); relies on the order every op keeps (sorted by start, no overlaps).
fn free_sorted(t: &Track, start: Time, dur: Time) -> bool {
    let end = start + dur;
    let i = t.clips.partition_point(|c| visit() && c.start < end);
    // of the clips starting before `end`, the last one ends latest
    i == 0 || t.clips[i - 1].end() <= start
}

/// `home`: track the clip already was on (move) – the kind check does not apply there.
/// `ids`: all clip ids of the project (batch of AddClips), kept up to date here.
fn add_clip(
    p: &mut Project,
    track: Option<TrackId>,
    mut clip: Clip,
    home: Option<TrackId>,
    mut ids: Option<&mut std::collections::HashSet<ClipId>>,
) -> Result<OpOutput> {
    clip.crop = clip.crop.clamped();
    // values from outside (MCP, plugins): nothing before 0, nothing beyond Time::LIMIT
    if !clip.sanitize_times() || clip.duration > Time::LIMIT || clip.start > Time::LIMIT {
        return Err(CoreError::InvalidDuration);
    }
    let len = source_length(p, &clip);
    clamp_to_source(&mut clip, len);
    // a duplicate id (e.g. the same op twice) would otherwise give two clips one ID
    let taken = match &ids {
        Some(ids) => ids.contains(&clip.id),
        None => p.find_clip(clip.id).is_some(),
    };
    if taken {
        clip.id = ClipId::new();
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
            let ti = p
                .timeline
                .tracks
                .iter()
                .position(|t| t.id == tid)
                .ok_or(CoreError::TrackNotFound(tid))?;
            let t = &p.timeline.tracks[ti];
            if !t.kind.accepts(kind) && home != Some(tid) {
                return Err(CoreError::KindMismatch {
                    clip: kind,
                    track: t.kind,
                });
            }
            ensure_unlocked(t)?;
            let magnet_main = t.main && p.settings.main_track_magnet;
            if magnet_main || free_sorted(t, clip.start, clip.duration) {
                ti
            } else {
                // occupied: create a new track above.
                insert_track_above(p, ti, kind)
            }
        }
        None if kind == TrackKind::Video => match p.timeline.tracks.iter().position(|t| t.main) {
            Some(ti) => {
                ensure_unlocked(&p.timeline.tracks[ti])?;
                let magnet = p.settings.main_track_magnet;
                if magnet || free_sorted(&p.timeline.tracks[ti], clip.start, clip.duration) {
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
        None => {
            match p.timeline.tracks.iter().position(|t| {
                t.kind == kind && !t.locked && free_sorted(t, clip.start, clip.duration)
            }) {
                Some(ti) => ti,
                None => {
                    p.timeline.tracks.push(Track::new(kind));
                    p.timeline.tracks.len() - 1
                }
            }
        }
    };

    let magnet_main = p.timeline.tracks[ti].main && p.settings.main_track_magnet;
    let t = &mut p.timeline.tracks[ti];
    if magnet_main {
        // insert before the first clip whose middle lies after the drop position
        let at = clip.start.0;
        let idx = t
            .clips
            .iter()
            .take_while(|c| (c.start.0 as i128 + c.end().0 as i128) / 2 < at as i128)
            .count();
        t.clips.insert(idx, clip);
        compact(t);
    } else {
        // behind clips with the same start, like push + stable sort
        let i = t.clips.partition_point(|c| visit() && c.start <= clip.start);
        t.clips.insert(i, clip);
    }
    if let Some(ids) = ids.as_mut() {
        ids.insert(id);
    }
    Ok(OpOutput::Clip(id))
}

fn insert_track_above(p: &mut Project, ti: usize, kind: TrackKind) -> usize {
    p.timeline.tracks.insert(ti + 1, Track::new(kind));
    ti + 1
}
