//! Player: composes all visible layers at the playhead (video/image/sticker with
//! effect chain as a texture per clip, text with outline/box/shadow), evaluates keyframes
//! and animations and cross-fades transitions. Editing directly in the picture via
//! `player_edit`.

use std::hash::{Hash, Hasher};

use cl_core::*;
use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText};

use crate::app::{App, DragAsset};
use crate::player_edit::Region;
use crate::playcore::{Mode, Want};
use crate::preview::{to_color_image, Done, Job, OpenJob};
use crate::theme::{self, icon};

/// Measurement point (`CUTLINE_PERF`): time of the player area per UI frame (sum and maximum, µs)
pub static PERF_PLAYER_SUM_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PERF_PLAYER_MAX_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Player-core layer for a clip (or a derived version)
fn lid(id: ClipId) -> crate::playcore::Lid {
    id.0.as_u128()
}

/// A visible layer at time t.
#[derive(Clone)]
pub(crate) struct Layer {
    clip: Clip,
    /// Time within the clip (can lie outside during transitions)
    t_rel: Time,
    /// Additional opacity (transitions)
    alpha: f32,
    /// Transition from the previous clip: (its ID, progress 0…1)
    from: Option<(ClipId, f32)>,
}

/// Preview of blended transitions: last frames of the clips involved (CPU), the blended frame
/// and which transitions were too slow for playback.
#[derive(Default)]
pub struct ShaderPreview {
    cpu: std::collections::HashMap<ClipId, (u64, std::sync::Arc<cl_media::RgbaFrame>)>,
    /// blends in the background (blending costs 10–150 ms per frame on the CPU)
    mixer: Option<Mixer>,
    /// requested blend (at most one) and the finished one not yet taken over
    pending: Option<u64>,
    ready: Option<MixDone>,
    /// besides the clips with a transition: keep these frames too (versions with effect elements)
    wanted: Vec<ClipId>,
    tex: Option<egui::TextureHandle>,
    /// blended versions with the effect elements on top: (texture, mask, strength)
    fx: Vec<(egui::TextureHandle, Option<Mask>, f32)>,
    /// last blended: all inputs (frames, position, area, progress)
    last: Option<u64>,
    /// currently shown instead of A and B: (A, B)
    current: Option<(ClipId, ClipId)>,
    /// too slow during playback: cross-fade these transitions until paused
    slow: Vec<(ClipId, ClipId)>,
    /// Number of too slow frames in the transition `slow_pair`
    slow_hits: u8,
    slow_pair: Option<(ClipId, ClipId)>,
    /// Transition that `tex` belongs to (held while a frame is missing)
    shown: Option<(ClipId, ClipId)>,
    /// Playback ran in the last frame (detect start: prepare shaders)
    was_playing: bool,
    /// Shaders already prepared once (on the first frame after the start)
    warmed: bool,
}

/// What the preview shows in a transition it blends.
#[derive(Debug, PartialEq)]
enum Step {
    /// all frames there: blend
    Mix,
    /// a frame is still missing: keep the last blended frame of this transition
    Hold,
    /// cross-fade (no blended frame yet or too slow during playback)
    Skip,
}

impl ShaderPreview {
    fn step(&self, pair: (ClipId, ClipId), have_frames: bool) -> Step {
        if self.slow.contains(&pair) {
            Step::Skip
        } else if have_frames {
            Step::Mix
        } else if self.shown == Some(pair) && self.tex.is_some() {
            Step::Hold
        } else {
            Step::Skip
        }
    }

    /// Duration of a blended frame during playback: two frames longer than a frame interval → this
    /// transition is cross-faded until paused (no back and forth).
    fn note_time(&mut self, pair: (ClipId, ClipId), secs: f64, interval: f64) {
        if self.slow_pair != Some(pair) {
            (self.slow_pair, self.slow_hits) = (Some(pair), 0);
        }
        if secs > interval {
            self.slow_hits += 1;
            if self.slow_hits >= 2 && !self.slow.contains(&pair) {
                self.slow.push(pair);
            }
        }
    }
}

/// Blend job for the background thread: frame pairs (clips, then versions with effect elements)
/// with position, area and progress.
struct MixJob {
    id: u64,
    pair: (ClipId, ClipId),
    plan: cl_fx::catalog::TransitionPlan,
    frames: Vec<(std::sync::Arc<cl_media::RgbaFrame>, std::sync::Arc<cl_media::RgbaFrame>)>,
    /// per frame pair from the second on: mask and strength of the effect element
    sets: Vec<(Option<Mask>, f32)>,
    ev: (Evaluated, Evaluated),
    size: (u32, u32),
    proj: (f32, f32),
    p: f32,
}

/// Finished blend: blended frame (unmasked elements included), then per masked element
/// its blended version. Empty if blending failed.
struct MixDone {
    id: u64,
    pair: (ClipId, ClipId),
    images: Vec<egui::ColorImage>,
    masked: Vec<(Option<Mask>, f32)>,
    secs: f64,
}

fn run_mix(j: MixJob) -> MixDone {
    let t0 = std::time::Instant::now();
    let (cw, ch) = j.size;
    let mixed: Option<Vec<Vec<u8>>> = j
        .frames
        .iter()
        .map(|(fa, fb)| mix_layers(&j.plan, (fa, &j.ev.0), (fb, &j.ev.1), (cw, ch), j.proj, j.p))
        .collect();
    let (mut images, mut masked) = (Vec::new(), Vec::new());
    if let Some(mixed) = mixed {
        // include unmasked elements in the blended frame, masked ones are drawn in the shape by `draw_layers`
        let mut it = mixed.into_iter();
        let mut base = it.next().unwrap_or_default();
        let mut rest = Vec::new();
        for (img, x) in it.zip(&j.sets) {
            if x.0.is_none() {
                fold_fx(&mut base, &img, x.1);
            } else {
                rest.push(img);
                masked.push(*x);
            }
        }
        images = std::iter::once(base)
            .chain(rest)
            .map(|img| egui::ColorImage::from_rgba_unmultiplied([cw as usize, ch as usize], &img))
            .collect();
    }
    MixDone { id: j.id, pair: j.pair, images, masked, secs: t0.elapsed().as_secs_f64() }
}

/// Background thread for transition blends (the result appears one frame later).
struct Mixer {
    tx: std::sync::mpsc::Sender<MixJob>,
    rx: std::sync::mpsc::Receiver<MixDone>,
}

impl Mixer {
    fn spawn(ctx: egui::Context) -> Self {
        let (tx, jobs) = std::sync::mpsc::channel::<MixJob>();
        let (done, rx) = std::sync::mpsc::channel();
        let _ = std::thread::Builder::new().name("cl-mix".into()).spawn(move || {
            while let Ok(j) = jobs.recv() {
                if done.send(run_mix(j)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        Self { tx, rx }
    }
}

/// Longest side of the blend area during playback: this keeps a blended frame well below a frame
/// interval even on the CPU, also with an effect element on top (measurement: test `mix_time`).
const MIX_PLAY: f32 = 960.0;

/// Blend area in the project aspect ratio `pw`×`ph`, as large as the display `px` (at most
/// 1920, during playback at most [`MIX_PLAY`] on the long side).
fn mix_size(pw: f32, ph: f32, px: egui::Vec2, playing: bool) -> (u32, u32) {
    let max = if playing { MIX_PLAY } else { 1920.0 };
    let k = (px.x / pw).min(px.y / ph).min(max / pw.max(ph));
    (((pw * k).round() as u32).max(16), ((ph * k).round() as u32).max(16))
}

/// Prepare the project's shader transitions in the background (open GPU, compile pipelines)
/// so the first blended frame during playback does not hang.
fn warm_shaders(project: &Project) {
    let mut files: Vec<std::path::PathBuf> = project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter_map(|c| cl_fx::catalog::transition_plan(&c.transition_out.as_ref()?.id).shader.map(|s| s.0))
        .collect();
    files.sort();
    files.dedup();
    if files.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let Some(g) = cl_gpu::gpu() else { return };
        for f in files {
            if let Ok(src) = std::fs::read_to_string(&f) {
                let _ = g.prepare(&src);
            }
        }
    });
}

/// Blend the unmasked version `fx` of an effect/filter element with strength `k` into the blended frame `base`:
/// like the export, which applies the element to the finished transition (laid on top,
/// semi-transparent parts of the transition, e.g. reflections, showed through unfiltered).
fn fold_fx(base: &mut [u8], fx: &[u8], k: f32) {
    for (b, f) in base.chunks_exact_mut(4).zip(fx.chunks_exact(4)) {
        for c in 0..4 {
            b[c] = (b[c] as f32 + (f[c] as f32 - b[c] as f32) * k).round() as u8;
        }
    }
}

/// Draw version `fx` (effect/filter element) over the frame `base` at the corners `corners`,
/// with mask only in the shape (inverted: everywhere except the shape), only in the visible part `shown`
/// (crop; otherwise equal to `corners`).
#[allow(clippy::too_many_arguments)]
fn draw_fx(content: &egui::Painter, base: egui::TextureId, fx: egui::TextureId, corners: [egui::Pos2; 4], shown: [egui::Pos2; 4], mask: &Option<Mask>, alpha: f32, canvas: Rect) {
    let Some(mk) = mask else {
        crate::fx_mask::draw_textured(content, fx, corners, &shown, alpha);
        return;
    };
    let (polys, inverted) = crate::fx_mask::polygons(mk, canvas);
    let polys = polys.iter().map(|p| crate::fx_mask::clip_convex(p, &shown));
    if inverted {
        // everything except the shape: effect fully, original in the shape on top
        crate::fx_mask::draw_textured(content, fx, corners, &shown, alpha);
        for poly in polys {
            crate::fx_mask::draw_textured(content, base, corners, &poly, alpha);
        }
    } else {
        for poly in polys {
            crate::fx_mask::draw_textured(content, fx, corners, &poly, alpha);
        }
    }
}

/// Progress of a transition of length `d` around the cut `cut` (as in the export:
/// window `cut − d/2 … cut + d/2`), `None` outside.
pub(crate) fn transition_progress(ts: f64, cut: f64, d: f64) -> Option<f32> {
    if d <= 0.0 || ts < cut - d / 2.0 || ts >= cut + d / 2.0 {
        return None;
    }
    Some(((ts - (cut - d / 2.0)) / d).clamp(0.0, 1.0) as f32)
}

/// Finished layer for the transition preview: the clip frame (effects, filters and mask already
/// included) with position, size, rotation and opacity from `ev` placed on an empty area
/// `cw`×`ch` (project `pw`×`ph`) – like the export, which also blends transitions between finished
/// full-frame layers. Same geometry as `draw_layers`.
// shortcut: nearest pixel instead of filtering; enough because the clip frame already comes at display size
pub(crate) fn place_on_canvas(src: &cl_media::RgbaFrame, ev: &Evaluated, cw: u32, ch: u32, pw: f32, ph: f32) -> Vec<u8> {
    let mut out = vec![0u8; (cw * ch * 4) as usize];
    let (sw, sh) = (src.width as f32, src.height as f32);
    let fit = (cw as f32 / sw).min(ch as f32 / sh) * ev.scale;
    if src.width == 0 || src.height == 0 || fit <= 0.0 || src.data.len() < (src.width * src.height * 4) as usize {
        return out;
    }
    let (kx, ky) = (cw as f32 / pw, ch as f32 / ph);
    let (cx, cy) = (cw as f32 / 2.0 + ev.x * kx, ch as f32 / 2.0 + ev.y * ky);
    let (s, c) = (-ev.rotation_deg.to_radians()).sin_cos();
    let alpha = ev.opacity.clamp(0.0, 1.0);
    // crop: only this part of the clip frame, at its place
    let k = ev.crop;
    let (u0, u1, v0, v1) = (k.left * sw, (1.0 - k.right) * sw, k.top * sh, (1.0 - k.bottom) * sh);
    // rows spread over several cores: during playback every millisecond counts
    cl_fx::xfade::rows(&mut out, cw as i32, &|y, line| {
        let (dx, dy) = (0.5 - cx, y as f32 + 0.5 - cy);
        let (mut u, mut v) = ((dx * c - dy * s) / fit + sw / 2.0, (dx * s + dy * c) / fit + sh / 2.0);
        for o in line.chunks_exact_mut(4) {
            if u >= u0 && v >= v0 && u < u1 && v < v1 {
                let i = ((v as u32 * src.width + u as u32) * 4) as usize;
                // like the overlay on the transparent area in the export: colour outside the mask
                // (and at opacity < 1) turns to black instead of showing through when blending
                let a = (src.data[i + 3] as f32 * alpha).round() as u32;
                for k in 0..3 {
                    o[k] = ((src.data[i + k] as u32 * a + 127) / 255) as u8;
                }
                o[3] = a as u8;
            }
            u += c / fit;
            v += s / fit;
        }
    });
    out
}

/// Does the preview blend the transition itself? Otherwise it cross-fades (`fade` is exactly that; other
/// `xfade` kinds without a replica in `cl_fx::xfade` too).
pub(crate) fn mixes(plan: &cl_fx::catalog::TransitionPlan) -> bool {
    plan.shader.is_some() || (plan.xfade != "fade" && cl_fx::xfade::supported(plan.xfade))
}

/// Blend a transition as in the export between finished layers: place clip frames `fa`/`fb` (effects and
/// mask included) with position `ea`/`eb` on an area `cw`×`ch` (project `pw`×`ph`),
/// then shader (cl-gpu) or `cl_fx::xfade` at progress `p`. RGBA, alpha stays alpha.
/// Without GPU (or on a shader error) like the export with the package's `xfade` fallback.
pub(crate) fn mix_layers(
    plan: &cl_fx::catalog::TransitionPlan,
    (fa, ea): (&cl_media::RgbaFrame, &Evaluated),
    (fb, eb): (&cl_media::RgbaFrame, &Evaluated),
    (cw, ch): (u32, u32),
    (pw, ph): (f32, f32),
    p: f32,
) -> Option<Vec<u8>> {
    let a = place_on_canvas(fa, ea, cw, ch, pw, ph);
    let b = place_on_canvas(fb, eb, cw, ch, pw, ph);
    let shader = plan.shader.as_ref().and_then(|(file, params)| {
        cl_gpu::render(
            &std::fs::read_to_string(file).ok()?,
            &crate::preview::shader_params(params),
            cl_gpu::Frame { rgba: &a, width: cw, height: ch },
            cl_gpu::Frame { rgba: &b, width: cw, height: ch },
            (cw, ch),
            p,
        )
        .ok()
    });
    shader.or_else(|| cl_fx::xfade::mix(plan.xfade, &a, &b, cw, ch, p))
}

/// Lengths of the transitions at the start and end of `c` (clip `i` of the track, possibly changed
/// while editing): only to directly adjoining, active neighbours, length as in the export.
pub(crate) fn transition_spans(clips: &[Clip], i: usize, c: &Clip) -> (Option<f64>, Option<f64>) {
    let next = clips.get(i + 1).filter(|n| (n.start.0 - c.end().0).abs() < 2000 && !n.disabled);
    let tr_out = next.filter(|_| c.transition_out.is_some()).map(|n| cl_fx::transition_secs(c, n));
    let prev = i.checked_sub(1).and_then(|k| clips.get(k));
    let tr_in = prev
        .filter(|p| (c.start.0 - p.end().0).abs() < 2000 && !p.disabled && p.transition_out.is_some())
        .map(|p| cl_fx::transition_secs(p, c));
    (tr_in, tr_out)
}

/// Decode size of a clip for the preview (`canvas_px` = display area in pixels).
fn decode_size(c: &Clip, m: &MediaRef, canvas_px: egui::Vec2, div: f32) -> (u32, u32) {
    // Enlarged clips (e.g. landscape gameplay in portrait at 200 %) need
    // correspondingly more pixels, otherwise a small frame is scaled up and blurry.
    let zoom = c
        .keyframes
        .get(Prop::Scale)
        .iter()
        .map(|k| k.v)
        .fold(c.transform.scale, f32::max)
        .clamp(0.25, 6.0);
    let cap = |v: f32, src: u32| (v as u32).clamp(64, 3840).min(src.max(64));
    let max_w = cap(canvas_px.x / div * zoom, m.width);
    let max_h = cap(canvas_px.y / div * zoom, m.height);
    cl_media::fit_size(m.width, m.height, max_w, max_h)
}

/// Source time of a clip's single frame at clip time `t_rel`.
fn still_src(c: &Clip, m: &MediaRef, t_rel: Time, fps: Rational) -> Time {
    let ClipContent::Media { kind: MediaKind::Video, .. } = &c.content else {
        // Animated image: the same single frame as in the export (loop or hold last frame)
        // shortcut: during playback one single-frame job per frame instead of a stream, extend if it stutters
        return cl_media::animation(&m.path).map_or(Time::ZERO, |a| a.seek_time(t_rel));
    };
    let mut end = c.source_in + c.source_span();
    if m.duration > Time::ZERO {
        end = Time(end.0.min(m.duration.0));
    }
    still_seek(c.source_time(c.start + t_rel), c.source_in, end, m.fps.unwrap_or(fps))
}

/// Source time for a single preview frame: rounded to the frame grid, at most one and a half
/// frames before the end of the clip or file. FFmpeg delivers the first frame from `-ss`; a time in the
/// last frame (or rounded to the end) gave none at all – „Datei kann nicht gelesen werden: “.
fn still_seek(t: Time, source_in: Time, end: Time, fps: Rational) -> Time {
    let last = end - Time::from_secs_f64(1.5 / fps.as_f64().max(1.0));
    Time(t.snap_to_frame(fps).0.min(last.0).max(source_in.0))
}

/// All visible layers from bottom to top (incl. transitions); `live` replaces the clip
/// with the same ID (edit in progress).
fn layers_in(project: &Project, live: Option<&Clip>, t: Time) -> Vec<Layer> {
    let mut out = Vec::new();
    for tr in project
        .timeline
        .tracks
        .iter()
        .filter(|tr| !tr.hidden && tr.kind != TrackKind::Audio)
    {
        for (i, c0) in tr.clips.iter().enumerate() {
            // check the time window first, then clone
            let c = live.filter(|l| l.id == c0.id).unwrap_or(c0);
            if c.disabled {
                continue;
            }
            let (tr_in, tr_out) = transition_spans(&tr.clips, i, &c);
            let prev = if i > 0 { tr.clips.get(i - 1) } else { None };
            let ts = t.as_secs_f64();
            let (s, e) = (c.start.as_secs_f64(), c.end().as_secs_f64());
            let lo = s - tr_in.map(|d| d / 2.0).unwrap_or(0.0);
            let hi = e + tr_out.map(|d| d / 2.0).unwrap_or(0.0);
            if ts < lo || ts >= hi {
                continue;
            }
            let mut alpha = 1.0;
            let mut from = None;
            if let Some(d) = tr_in {
                // fade in over the predecessor
                let p = transition_progress(ts, s, d).unwrap_or(1.0);
                alpha = p;
                from = prev.map(|pc| (pc.id, p));
            }
            let t_rel = Time::from_secs_f64((ts - s).clamp(0.0, (e - s - 1e-3).max(0.0)));
            out.push(Layer {
                clip: c.clone(),
                t_rel,
                alpha,
                from,
            });
        }
    }
    out
}

/// A layer for which the preview needs a frame: clip (or its version with an
/// effect element), medium, position in time, possibly start place (open ahead) and – for versions
/// with element – (base clip, mask, strength).
pub(crate) struct Need {
    pub id: ClipId,
    pub clip: Clip,
    pub media: MediaRef,
    pub layer: Layer,
    pub prefetch: Option<Time>,
    pub overlay: Option<(ClipId, Option<Mask>, f32)>,
}

/// What the preview needs at `now` (shared by app and measurement test): all visible media
/// layers, active or about-to-start effect elements as their own versions and during playback
/// clips that begin within 1.5 s (open streams ahead so nothing hitches at cuts).
/// `adjust` changes the clip before the request (eyedropper: without chroma key).
pub(crate) fn needs(project: &Project, layers: &[Layer], now: Time, playing: bool, adjust: impl Fn(&mut Clip)) -> Vec<Need> {
    const AHEAD: f64 = 1.5;
    let mut out = Vec::new();
    for l in layers {
        let mut c = l.clip.clone();
        adjust(&mut c);
        let ClipContent::Media { media, .. } = &c.content else {
            continue;
        };
        let Some(m) = project.media(*media) else {
            continue;
        };
        out.push(Need { id: c.id, clip: c.clone(), media: m.clone(), layer: l.clone(), prefetch: None, overlay: None });
        for e in fx_elements_over(project, c.id) {
            let active = e.contains(now);
            let soon = playing && e.start > now && (e.start - now).as_secs_f64() <= AHEAD && e.start < c.end();
            if !active && !soon {
                continue;
            }
            let Some((x, k)) = fx_version(&c, e, now) else { continue };
            let id = c.id.derived(e.id.0.as_u128());
            let (layer, prefetch, overlay) = if active {
                (l.clone(), None, Some((c.id, e.fx.mask, k)))
            } else {
                (Layer { clip: x.clone(), t_rel: e.start - c.start, alpha: l.alpha, from: None }, Some(e.start), None)
            };
            out.push(Need { id, clip: x, media: m.clone(), layer, prefetch, overlay });
        }
    }
    if playing {
        let upcoming = project
            .timeline
            .tracks
            .iter()
            .filter(|tr| !tr.hidden && matches!(tr.kind, TrackKind::Video | TrackKind::Sticker))
            .flat_map(|tr| tr.clips.iter())
            .filter(|c| !c.disabled && c.start > now && (c.start - now).as_secs_f64() <= AHEAD);
        for c in upcoming {
            let ClipContent::Media { media, .. } = &c.content else {
                continue;
            };
            let Some(m) = project.media(*media) else {
                continue;
            };
            let layer = Layer { clip: c.clone(), t_rel: Time::ZERO, alpha: 1.0, from: None };
            out.push(Need { id: c.id, clip: c.clone(), media: m.clone(), layer, prefetch: Some(c.start), overlay: None });
        }
    }
    out
}

/// Can the clip run as a stream? (Reverse and speed curves: only single frames)
pub(crate) fn streamable(c: &Clip) -> bool {
    matches!(&c.content, ClipContent::Media { kind: MediaKind::Video, .. }) && !c.fx.reverse && c.fx.speed_curve.is_none()
}

/// Preroll when stopped: the playhead must rest on a heavy layer this long before its stream
/// is already opened (Play then shows frames at once; clicking and scrubbing open nothing)
const PREROLL_REST: f64 = 0.3;
/// Generation of a preroll job at the starter (the core's counts from 0)
const PREROLL_GEN: u64 = u64::MAX;

/// What Play will request for a layer: layer, stream key (size, quality, effects,
/// source), source fingerprint, place and edit state
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PrerollSpec {
    pub id: u128,
    pub key: u64,
    pub alias: u64,
    pub at: Time,
    pub revision: u64,
}

/// Stream opened ahead while stopped (see [`App::update_preroll`])
pub(crate) struct Preroll {
    spec: PrerollSpec,
    /// since when the playhead rests here (core time)
    since: f64,
    sent: bool,
    /// opened: frame rate and stream
    stream: Option<(f64, cl_media::FrameStream)>,
}

/// May the stream `p` opened ahead answer the core's request `o` after Play? Only for
/// exactly the same layer with the same key and fingerprint, at the same place and in the
/// same edit state – otherwise Play opens as before.
pub(crate) fn preroll_fits(p: &PrerollSpec, o: &crate::playcore::Open, alias: u64, revision: u64) -> bool {
    p.id == o.id && p.key == o.key && p.alias == alias && p.at == o.at && p.revision == revision
}

/// Source frame rate for the decoder's reverse window (`preview::Job::Frame::window`): only when
/// playing forwards a video running in reverse from the original without effect graph (the
/// graph depends on the clip time), at most 1080p. Stopped, paused, stepping and scrubbing:
/// `None` as before. Above 1080p FFmpeg decodes hardly faster than real time (measured 4K:
/// ~35 frames/s wall time): the window would never get ahead of the clock and be slower than single frames.
pub(crate) fn reverse_window(c: &Clip, m: &MediaRef, original: bool, no_graph: bool, playing_forward: bool, fps: Rational) -> Option<Rational> {
    let video = matches!(&c.content, ClipContent::Media { kind: MediaKind::Video, .. });
    let hd = m.width.max(m.height) <= 1920 && m.width.min(m.height) <= 1080;
    (video && hd && c.fx.reverse && original && no_graph && playing_forward).then(|| m.fps.unwrap_or(fps))
}

/// Fingerprint of a clip's effects (part of the frame and stream keys)
pub(crate) fn fx_hash_of(c: &Clip) -> u64 {
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&c.fx).unwrap_or_default().hash(&mut hs);
    hs.finish()
}

/// Stream of a layer: key and decode size
pub(crate) fn stream_spec(id: ClipId, c: &Clip, m: &MediaRef, canvas_px: egui::Vec2, div: f32, fps: Rational, fxh: u64) -> (u64, u32, u32) {
    let (w, h) = decode_size(c, m, canvas_px, div);
    (App::frame_key(id, c, Time::ZERO, w, h, Time::ZERO, false, fxh, fps), w, h)
}

/// Job to open the stream from timeline place `o.at` (source time and effect graph with it)
/// Source fingerprint of a stream: file, source time per timeline place (stays the same when
/// splitting), speed, effects, size, frame rate – without clip ID (see `playcore::Want::alias`)
pub(crate) fn stream_alias(c: &Clip, m: &MediaRef, (w, h): (u32, u32), fps: Rational, fxh: u64) -> u64 {
    let offset = ((c.source_in.0 as f64 - c.start.0 as f64 * c.speed.max(0.01) as f64) / 1000.0).round() as i64;
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    (m.id, offset, c.speed.to_bits(), fxh, w, h, fps.num, fps.den).hash(&mut hs);
    hs.finish() | 1
}

/// Place a layer's stream is needed from (`Want::start`): opened ahead from the clip/element start.
/// After a switch from proxy parts to the original (`Pick::Original`) from where the parts end:
/// the parts stream runs on until there (`SWITCH_MARGIN`), and the original (4K, long GOP) starts far
/// slower than the parts stream's lead – opened at clock + that lead it fell 1.1–1.7 s behind the sound.
pub(crate) fn want_start(c: &Clip, pick: crate::proxy::Pick, playhead: Time, prefetch: Option<Time>) -> Time {
    match (prefetch, pick) {
        (Some(t), _) => t,
        (None, crate::proxy::Pick::Original { since_ms }) => {
            // streams run forwards at constant speed (`streamable`); the margin is SWITCH_MARGIN·speed source seconds
            let rel = (since_ms as f64 / 1000.0 - c.source_in.as_secs_f64()) / c.speed.max(0.01) as f64;
            c.start + Time::from_secs_f64(rel.max(0.0) + crate::proxy::parts::SWITCH_MARGIN)
        }
        _ => playhead,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn open_job(o: crate::playcore::Open, c: &Clip, m: &MediaRef, path: std::path::PathBuf, (w, h): (u32, u32), fps: Rational, alias: u64) -> OpenJob {
    let lut = c.fx.lut.as_deref().filter(|p| std::path::Path::new(p).is_file()).map(cl_media::escape_path);
    let t_rel = Time((o.at - c.start).0.clamp(0, (c.duration.0 - 1000).max(0)));
    OpenJob {
        open: o,
        alias,
        source_at: c.source_time(c.start + t_rel),
        graph: cl_fx::preview_graph(c, w, h, t_rel.as_secs_f64(), Some(fps), lut),
        hw: crate::preview::stream_decode(m, &path),
        path,
        w,
        h,
        fps,
    }
}

/// Effect/filter elements on tracks above the clip `clip` (enabled, visible). Without clones:
/// the callers filter by time (otherwise each UI frame copied all elements of the project).
fn fx_elements_over(project: &Project, clip: ClipId) -> Vec<&Clip> {
    let tracks = &project.timeline.tracks;
    let Some(idx) = tracks
        .iter()
        .position(|tr| tr.clips.iter().any(|c| c.id == clip))
    else {
        return Vec::new();
    };
    tracks[idx + 1..]
        .iter()
        .filter(|tr| !tr.hidden && matches!(tr.kind, TrackKind::Effect | TrackKind::Filter))
        .flat_map(|tr| tr.clips.iter())
        .filter(|c| !c.disabled)
        .collect()
}

/// Version of `c` with the effect/filter element `e` (at time `now`) and its strength.
/// The preview lays it over the clip frame (the export applies the element to everything below).
fn fx_version(c: &Clip, e: &Clip, now: Time) -> Option<(Clip, f32)> {
    let (mut r, filter) = match &e.content {
        ClipContent::Effect(r) => (r.clone(), false),
        ClipContent::Filter(r) => (r.clone(), true),
        _ => return None,
    };
    let animated = !e.keyframes.get(Prop::Strength).is_empty();
    if animated {
        r.intensity = 1.0;
    }
    let mut x = c.clone();
    if filter {
        x.fx.filter = Some(r);
    } else if !x.fx.effects.iter().any(|f| f.id == r.id) {
        x.fx.effects.push(r);
    }
    Some((x, if animated { e.strength_at(now - e.start) } else { 1.0 }))
}

/// Transitions the preview blends at time `t`: (layer A, layer B, plan, progress), bottom
/// first. All others it cross-fades (B with growing opacity over A). If an effect
/// or filter element lies on top, cross-fades are blended too: the export applies the element to
/// the finished transition, the preview lays its version of A and B blended on top.
fn mix_pairs(project: &Project, layers: &[Layer], t: Time) -> Vec<(usize, usize, cl_fx::catalog::TransitionPlan, f32)> {
    layers
        .iter()
        .enumerate()
        .filter_map(|(bi, b)| {
            let (a_id, p) = b.from?;
            let ai = layers.iter().position(|l| l.clip.id == a_id && l.alpha >= 0.999)?;
            let plan = cl_fx::catalog::transition_plan(&layers[ai].clip.transition_out.as_ref()?.id);
            let fx_over = || fx_elements_over(project, a_id).iter().any(|e| e.contains(t));
            (mixes(&plan) || fx_over()).then_some((ai, bi, plan, p))
        })
        .collect()
}

#[cfg(test)]
mod play_measure;

#[cfg(test)]
mod presence_tests {
    use super::*;

    /// Follow-up test K7: a 4K clip at speed 2 over a second 4K clip stayed a still image
    /// during playback while its proxy was built in parts. Here with the real app (source choice,
    /// starter, core) without a window: the upper layer must fetch frames continuously.
    /// `CUTLINE_QA_TEMPO` (2, 0.5, rev, kurve, scale), `CUTLINE_QA_PARTS`: build this many proxy parts of the
    /// upper source beforehand (the rest is missing, as during the build).
    /// `cargo test --release -p cl-app --bin cutline tempo_in_app -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn tempo_in_app() {
        let (a, b) = (std::path::Path::new("F:/cutline-testmedien/4k60_h264_80mbit.mp4"), std::path::Path::new("F:/cutline-testmedien/4k60_hevc10_50mbit.mp4"));
        if !cl_media::ffmpeg_available() || !a.is_file() || !b.is_file() {
            return;
        }
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        std::env::set_var("CUTLINE_NO_UPDATE", "1");
        std::env::set_var("CUTLINE_NO_ONBOARDING", "1");
        let ctx = egui::Context::default();
        let mut app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        let mode = std::env::var("CUTLINE_QA_TEMPO").unwrap_or_else(|_| "2".into());
        let (ma, mb) = (cl_media::probe(a).unwrap(), cl_media::probe(b).unwrap());
        let mut p = Project::new("tempo");
        p.settings.fps = Rational::new(60, 1);
        let mut v1 = cl_core::Track::new(TrackKind::Video);
        v1.main = true;
        v1.clips.push(Clip::new(ClipContent::Media { media: ma.id, kind: MediaKind::Video }, Time::ZERO, ma.duration));
        let mut v2 = cl_core::Track::new(TrackKind::Video);
        let mut c = Clip::new(ClipContent::Media { media: mb.id, kind: MediaKind::Video }, Time::ZERO, mb.duration);
        match mode.as_str() {
            "rev" => c.fx.reverse = true,
            "kurve" => c.fx.speed_curve = Some("montage".into()),
            "scale" => {
                for (i, v) in [0.5f32, 1.5, 0.5, 1.5].iter().enumerate() {
                    c.keyframes.set(Prop::Scale, Time::from_secs_f64(20.0 + i as f64 * 3.0), *v);
                }
            }
            t => {
                c.speed = t.parse().unwrap_or(2.0);
                c.duration = Time::from_secs_f64(mb.duration.as_secs_f64() / c.speed as f64);
            }
        }
        let top = c.id;
        v2.clips.push(c);
        p.timeline.tracks = vec![v1, v2];
        p.media = vec![ma, mb.clone()];
        app.project = p;
        // proxy in parts (as in a running build): `CUTLINE_QA_PARTS` parts of 10 s finished
        if let Some(n) = std::env::var("CUTLINE_QA_PARTS").ok().and_then(|s| s.parse().ok()) {
            let want = (app.settings.proxy_h, app.project.settings.fps);
            let t = std::time::Instant::now();
            crate::proxy::test_build_parts(&mb, home.path().join("p").join("b-540.mp4"), want, n);
            eprintln!("{n} Stücke in {:.0} s", t.elapsed().as_secs_f64());
        }
        for at in [20.0, 33.0, 72.0] {
            app.playhead = Time::from_secs_f64(at);
            app.core.stop();
            app.core.play(app.playhead, app.revision, app.core_now());
            let t0 = std::time::Instant::now();
            // stream frames of the upper layer (across stream changes) and from when
            let (mut total, mut last_taken, mut first) = (0u64, 0u64, None);
            // single frames (reverse, speed curve): each new frame has a new key
            let (mut stills, mut last_key) = (0u32, None);
            while t0.elapsed().as_secs_f64() < 8.0 {
                app.receive_decoded(&ctx);
                let t = crate::playcore::Tick { now: app.core_now(), playhead: app.playhead, revision: app.revision, audio: crate::playcore::AudioIn::None, end: app.project.duration(), fps: 60.0, loop_range: None, recording: false, max_quality_level: 3 };
                app.playhead = app.core.tick(&t);
                let _ = app.core.cmds();
                app.playing = app.core.playing();
                let layers = app.layers_at(app.playhead);
                app.update_layers(&ctx, &layers, egui::vec2(1280.0, 720.0));
                let k = app.frames.get(&top).map(|f| f.0);
                if k.is_some() && k != last_key {
                    stills += 1;
                    last_key = k;
                }
                let taken = app.core.taken(lid(top));
                total += if taken >= last_taken { taken - last_taken } else { taken };
                last_taken = taken;
                if first.is_none() && total > 0 {
                    first = Some((t0.elapsed().as_secs_f64(), total));
                }
                if std::env::var_os("CUTLINE_QA_DEBUG").is_some() {
                    let el = t0.elapsed().as_secs_f64();
                    if (el * 4.0).fract() < 0.03 {
                        eprintln!("{el:.2} s pos {:.2}: oben {} Bilder, Wahl {:?}, Ströme {:?} {}", app.playhead.as_secs_f64(), app.core.taken(lid(top)), app.proxy_picks.get(&lid(top)), app.core.streams().iter().map(|x| (x.0 & 0xffff, x.1 & 0xffff, x.2)).collect::<Vec<_>>(), app.core.sizes());
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let n = total;
            eprintln!("Tempo {mode} ab {at}: obere Ebene {n} Strombilder (erstes {first:?}), {stills} Einzelbilder, Ströme {:?}", app.core.streams());
            if streamable(app.project.find_clip(top).unwrap().1) {
                let (t1, n1) = first.expect("upper layer without frame");
                assert!((n - n1) as f64 >= (8.0 - t1 - 1.0) * 20.0, "upper layer stands still: {n1} → {n}");
            } else {
                assert!(stills >= 4, "upper layer stands still: {stills} single frames in 8 s");
            }
        }
    }

    /// Review finding: during playback each UI frame checked the file per layer
    /// (`is_local`, blocking on the NAS). Now the background state `presence`
    /// decides – here "present" for a file that does not exist: stream and single frame
    /// are requested anyway, so nobody asks the file system. If it is missing per
    /// background check, nothing is requested (no open jobs).
    #[test]
    fn requests_follow_the_background_presence_check() {
        let _g = crate::paths::HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("CUTLINE_HOME", home.path());
        std::env::set_var("CUTLINE_NO_UPDATE", "1");
        std::env::set_var("CUTLINE_NO_ONBOARDING", "1");
        let ctx = egui::Context::default();
        let mut app = App::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        let path = home.path().join("gibt-es-nicht.mp4");
        let m = MediaRef {
            id: cl_core::MediaId::new(),
            path: path.clone(),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(10.0),
            width: 64,
            height: 36,
            fps: Some(Rational::new(30, 1)),
            has_audio: false,
            audio_streams: 0,
            video_codec: Some("h264".into()),
            audio_codec: None,
        };
        let c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(10.0));
        let l = Layer { clip: c.clone(), t_rel: Time::ZERO, alpha: 1.0, from: None };
        let canvas = egui::vec2(64.0, 36.0);
        let ask = |app: &mut App| app.request_frame(&ctx, c.id, &c, &m, &l, canvas, None);
        // stopped: single frame only if present
        app.presence.set_all([(path.clone(), false)].into(), 1);
        ask(&mut app);
        assert!(!app.core.has_requests(), "missing file: no job");
        app.presence.set_all([(path.clone(), true)].into(), 1);
        ask(&mut app);
        assert!(app.core.has_requests(), "present per check: requested");
        // playback: stream is requested (the starter reports even if opening fails)
        app.core.play(Time::ZERO, 0, 0.0);
        ask(&mut app);
        assert!(app.starter.rx.recv_timeout(std::time::Duration::from_secs(20)).is_ok(), "stream requested");
        app.core.invalidate();
        app.presence.set_all([(path, false)].into(), 1);
        ask(&mut app);
        assert!(app.starter.rx.recv_timeout(std::time::Duration::from_millis(500)).is_err(), "missing: no stream");
    }

    /// Stream from proxy parts: the job points to the part list, and the time in it is shifted by
    /// the start of the first part. The source choice is in the stream key and in the
    /// source fingerprint (a change reopens, a donor stream of another choice is not taken over).
    #[test]
    fn stream_from_parts_opens_the_list_at_the_shifted_time() {
        let (mut app, home, _g) = crate::saving::tests::test_app();
        let ctx = egui::Context::default();
        let (starter, jobs) = crate::preview::Starter::capture();
        app.starter = starter;
        app.settings.use_proxies = true;
        let m = MediaRef {
            id: cl_core::MediaId::new(),
            path: home.path().join("q.mp4"),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(100.0),
            width: 3840,
            height: 2160,
            fps: Some(Rational::new(30, 1)),
            has_audio: false,
            audio_streams: 0,
            video_codec: Some("h264".into()),
            audio_codec: None,
        };
        app.project.media.push(m.clone());
        let c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(100.0));
        let l = Layer { clip: c.clone(), t_rel: Time::from_secs_f64(12.0), alpha: 1.0, from: None };
        let want = (app.settings.proxy_h, app.project.settings.fps);
        let out = home.path().join("q-540.mp4");
        // parts 1–3 (10–40 s) finished
        let ready: Vec<bool> = (0..10).map(|k| (1..4).contains(&k)).collect();
        crate::proxy::set_test_entry(&m.path, out.clone(), want, ready.clone(), false);
        app.playhead = Time::from_secs_f64(12.0);
        app.core.play(app.playhead, 0, 0.0);
        let ask = |app: &mut App| app.request_frame(&ctx, c.id, &c, &m, &l, egui::vec2(64.0, 36.0), None);
        ask(&mut app);
        let parts = jobs.try_recv().expect("stream from parts requested");
        assert_eq!(parts.path, home.path().join("q-540.teile").join("0001-0004.ffconcat"));
        assert!((parts.source_at.as_secs_f64() - 2.0).abs() < 1e-3, "source time 12 s − start of part 1 (10 s): {:?}", parts.source_at);
        ask(&mut app);
        assert!(jobs.try_recv().is_err(), "same choice: no new stream");
        // fully joined: other choice → other key, new stream from the whole file
        crate::proxy::set_test_entry(&m.path, out.clone(), want, ready, true);
        ask(&mut app);
        let full = jobs.try_recv().expect("change opens a replacement stream");
        // file from the inventory, without looking on disk (in the test it is not there at all)
        assert_eq!(full.path, out);
        assert!((full.source_at.as_secs_f64() - 12.0).abs() < 1e-3, "{:?}", full.source_at);
        assert_ne!(full.open.key, parts.open.key);
        assert_ne!(full.alias, parts.alias, "source choice in the fingerprint");
    }

    /// PL_gross: clip at 0.66 s from source 53.74 s, only proxy part 5 (50–60 s) finished. 3 s before
    /// the parts end the choice switches to the original; its stream is needed only from the parts end
    /// (timeline 6.92 s), not from clock + startup of the parts stream.
    #[test]
    fn switch_to_the_original_is_needed_from_the_parts_end() {
        use crate::proxy::parts::{choose_stream, Parting, PART_SECS};
        use crate::proxy::Pick;
        let mut c = Clip::new(ClipContent::Media { media: cl_core::MediaId::new(), kind: MediaKind::Video }, Time(658_843), Time(10_423_999));
        c.source_in = Time(53_740_753);
        let pt = Parting::new(300.0, Rational::new(60, 1), PART_SECS).unwrap();
        let ready: Vec<bool> = (0..pt.count).map(|k| k == 5).collect();
        let (end, margin) = ((c.source_in + c.source_span()).as_secs_f64(), crate::proxy::parts::SWITCH_MARGIN);
        let at = |t: f64| c.source_time(Time::from_secs_f64(t)).as_secs_f64();
        let parts = choose_stream(None, &ready, &pt, at(1.7), end, margin);
        assert_eq!(parts, Pick::Parts { first: 5, end: 6 });
        assert_eq!(want_start(&c, parts, Time::from_secs_f64(1.7), None), Time::from_secs_f64(1.7));
        let orig = choose_stream(Some(parts), &ready, &pt, at(3.93), end, margin);
        assert!(matches!(orig, Pick::Original { .. }), "{orig:?}");
        let parts_end = c.start.as_secs_f64() + 60.0 - c.source_in.as_secs_f64();
        let s = want_start(&c, orig, Time::from_secs_f64(3.93), None).as_secs_f64();
        assert!((s - parts_end).abs() < 0.02, "needed from {s:.3} s, parts end at {parts_end:.3} s");
        assert_eq!(want_start(&c, orig, Time::from_secs_f64(3.93), Some(Time::from_secs_f64(9.0))), Time::from_secs_f64(9.0), "opened ahead");
    }
}

#[cfg(test)]
mod shader_tests {
    use super::*;

    #[test]
    fn short_clip_between_two_long_transitions_matches_export() {
        // 0.2 s clip with two 0.5 s transitions: both limited to 0.2 s, at the neighbour too
        let tr = |s: f64, d: f64| {
            let mut c = Clip::new(ClipContent::Text(Default::default()), Time::from_secs_f64(s), Time::from_secs_f64(d));
            c.transition_out = Some(cl_core::TransitionRef { id: "fade".into(), duration: Time::from_secs_f64(0.5) });
            c
        };
        let clips = [tr(0.0, 2.0), tr(2.0, 0.2), tr(2.2, 2.0)];
        let span = |i: usize| transition_spans(&clips, i, &clips[i]);
        assert_eq!(span(0), (None, Some(0.2)));
        assert_eq!(span(1), (Some(0.2), Some(0.2)));
        assert_eq!(span(2), (Some(0.2), None));
        // half transitions together reach at most across the whole length of the short clip
        let (a, b) = span(1);
        assert!(a.unwrap() / 2.0 + b.unwrap() / 2.0 <= 0.2 + 1e-9);
    }

    /// Reverse window only when playing forwards a video running in reverse from the
    /// original without effect graph; pause/stepping/scrubbing, proxy and effects as before.
    #[test]
    fn reverse_window_only_while_playing_a_plain_reversed_original() {
        let m = MediaRef {
            id: cl_core::MediaId::new(),
            path: "a.mp4".into(),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(10.0),
            width: 64,
            height: 36,
            fps: Some(Rational::new(30, 1)),
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
        };
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(10.0));
        let p = Rational::new(60, 1);
        assert_eq!(reverse_window(&c, &m, true, true, true, p), None, "clip running forwards");
        c.fx.reverse = true;
        assert_eq!(reverse_window(&c, &m, true, true, true, p), Some(Rational::new(30, 1)), "source rate");
        assert_eq!(reverse_window(&c, &m, true, true, false, p), None, "stopped, paused, scrubbing");
        assert_eq!(reverse_window(&c, &m, false, true, true, p), None, "Proxy");
        assert_eq!(reverse_window(&c, &m, true, false, true, p), None, "effect graph");
        let m2 = MediaRef { fps: None, ..m.clone() };
        assert_eq!(reverse_window(&c, &m2, true, true, true, p), Some(p), "without source rate: project rate");
        let uhd = MediaRef { width: 3840, height: 2160, ..m.clone() };
        assert_eq!(reverse_window(&c, &uhd, true, true, true, p), None, "above 1080p: single frames");
        let img = Clip { content: ClipContent::Media { media: m.id, kind: MediaKind::Image }, ..c };
        assert_eq!(reverse_window(&img, &m, true, true, true, p), None, "image");
    }

    /// Preroll: Play takes the stream opened ahead only for exactly the same request
    #[test]
    fn preroll_only_fits_the_same_request() {
        let spec = PrerollSpec { id: 7, key: 42, alias: 9, at: Time::from_secs_f64(5.0), revision: 3 };
        let o = crate::playcore::Open { id: 7, key: 42, at: Time::from_secs_f64(5.0), gen: 0, seq: 1 };
        assert!(preroll_fits(&spec, &o, 9, 3));
        assert!(!preroll_fits(&spec, &crate::playcore::Open { at: Time::from_secs_f64(5.5), ..o }, 9, 3), "other place");
        assert!(!preroll_fits(&spec, &o, 9, 4), "edited");
        assert!(!preroll_fits(&spec, &crate::playcore::Open { key: 43, ..o }, 9, 3), "quality, size, effects, source");
        assert!(!preroll_fits(&spec, &crate::playcore::Open { id: 8, ..o }, 9, 3), "other layer");
        assert!(!preroll_fits(&spec, &o, 10, 3), "other source");
    }

    #[test]
    fn still_seek_never_lands_behind_the_last_frame() {
        let fps = Rational { num: 25, den: 1 };
        let end = Time::from_millis(6400);
        // before: 6.38 s (end − 20 ms) rounded to the grid = 6.40 s → no frame
        let t = still_seek(Time::from_millis(6390), Time::ZERO, end, fps);
        assert!(t > Time::from_millis(6320) && t <= Time::from_millis(6360), "{t:?}");
        // in the middle of the clip unchanged on the grid
        assert_eq!(still_seek(Time::from_millis(1013), Time::ZERO, end, fps), Time::from_millis(1000));
        // never before the source start
        assert_eq!(still_seek(Time::ZERO, Time::from_millis(6390), end, fps), Time::from_millis(6390));
    }

    #[test]
    fn progress_is_centered_on_the_cut() {
        assert_eq!(transition_progress(0.69, 1.0, 0.6), None);
        assert_eq!(transition_progress(0.7, 1.0, 0.6), Some(0.0));
        assert!((transition_progress(1.0, 1.0, 0.6).unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(transition_progress(1.3, 1.0, 0.6), None);
        assert_eq!(transition_progress(1.0, 1.0, 0.0), None);
    }

    #[test]
    fn transition_layers_are_placed_like_on_screen() {
        let id = Evaluated { x: 0.0, y: 0.0, scale: 1.0, rotation_deg: 0.0, opacity: 1.0, volume_db: 0.0, crop: Crop::default() };
        let frame = |w: u32, h: u32, a: u8| cl_media::RgbaFrame { width: w, height: h, data: [255, 0, 0, a].repeat((w * h) as usize) };
        let alpha = |img: &[u8], cw: u32, x: u32, y: u32| img[((y * cw + x) * 4 + 3) as usize];
        // unchanged: fills the area
        let full = place_on_canvas(&frame(16, 9, 255), &id, 32, 18, 1920.0, 1080.0);
        assert!(full.chunks_exact(4).all(|p| p == [255, 0, 0, 255]));
        // landscape in a portrait project, enlarged to frame height (K2_short): fills too
        let cover = place_on_canvas(&frame(16, 9, 255), &Evaluated { scale: 3.1605, ..id }, 18, 32, 1080.0, 1920.0);
        assert!(cover.chunks_exact(4).all(|p| p[3] == 255));
        // without enlargement: only a strip in the middle, empty above and below
        let strip = place_on_canvas(&frame(16, 9, 255), &id, 18, 32, 1080.0, 1920.0);
        assert_eq!((alpha(&strip, 18, 9, 16), alpha(&strip, 18, 9, 1)), (255, 0));
        // shrunk and moved right: empty on the left, picture on the right
        let moved = place_on_canvas(&frame(16, 9, 255), &Evaluated { scale: 0.5, x: 480.0, ..id }, 32, 18, 1920.0, 1080.0);
        assert_eq!((alpha(&moved, 32, 4, 9), alpha(&moved, 32, 24, 9)), (0, 255));
        // opacity and mask (alpha in the clip frame) are kept
        let faded = place_on_canvas(&frame(16, 9, 255), &Evaluated { opacity: 0.5, ..id }, 32, 18, 1920.0, 1080.0);
        assert_eq!(alpha(&faded, 32, 16, 9), 128);
        // outside the mask the colour is black too (as in the export): otherwise when blending
        // (pixelate in N_tr) the picture showed through without mask
        let masked = place_on_canvas(&frame(16, 9, 0), &id, 32, 18, 1920.0, 1080.0);
        assert!(masked.iter().all(|&v| v == 0));
        let half = place_on_canvas(&frame(16, 9, 128), &id, 32, 18, 1920.0, 1080.0);
        assert_eq!(&half[..4], &[128, 0, 0, 128]);
        // rotated 90°: the landscape stands upright, empty left and right
        let turned = place_on_canvas(&frame(16, 9, 255), &Evaluated { rotation_deg: 90.0, ..id }, 32, 18, 1920.0, 1080.0);
        assert_eq!((alpha(&turned, 32, 16, 1), alpha(&turned, 32, 2, 9)), (255, 0));
    }
}

/// Preview still without a window: the same steps as the player when stopped (layers,
/// decode size, single frame with effect chain, effect elements, blending of transitions), only
/// the composing of the layers on the CPU instead of in egui. Text is left out.
#[cfg(test)]
pub(crate) mod still {
    use super::*;

    fn over(dst: &mut [u8], src: &[u8], k: f32) {
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            let a = s[3] as f32 / 255.0 * k;
            for c in 0..3 {
                d[c] = (s[c] as f32 * a + d[c] as f32 * (1.0 - a)).round() as u8;
            }
        }
    }

    fn decode(project: &Project, c: &Clip, t_rel: Time, cw: u32, ch: u32) -> Option<cl_media::RgbaFrame> {
        let ClipContent::Media { media, .. } = &c.content else { return None };
        let m = project.media(*media)?;
        let (w, h) = decode_size(c, m, egui::vec2(cw as f32, ch as f32), 1.0);
        let lut = c.fx.lut.as_deref().filter(|p| std::path::Path::new(p).is_file()).map(cl_media::escape_path);
        let graph = cl_fx::preview_graph(c, w, h, t_rel.as_secs_f64(), None, lut);
        let src = still_src(c, m, t_rel, project.settings.fps);
        cl_media::decode_frame_fx(&m.path, src, w, h, graph.as_deref()).ok()
    }

    /// RGBA `cw`×`ch` of the project at time `t` on black.
    pub(crate) fn render(project: &Project, t: Time, cw: u32, ch: u32) -> Vec<u8> {
        let (pw, ph) = (project.settings.width.max(2) as f32, project.settings.height.max(2) as f32);
        let layers = layers_in(project, None, t);
        let pairs = mix_pairs(project, &layers, t);
        let mut out = [0u8, 0, 0, 255].repeat((cw * ch) as usize);
        for (i, l) in layers.iter().enumerate() {
            if pairs.iter().any(|p| p.1 == i) || !matches!(l.clip.content, ClipContent::Media { .. }) {
                continue;
            }
            let pair = pairs.iter().find(|p| p.0 == i);
            // the clip itself, then the versions with the effect elements on top (with their strength)
            let mut versions = vec![(None, 1.0)];
            for e in fx_elements_over(project, l.clip.id).into_iter().filter(|e| e.contains(t)) {
                versions.push((Some(e.clone()), 0.0));
            }
            // blended frame with the unmasked elements included (as in the player); otherwise on top
            let mut base: Option<Vec<u8>> = None;
            for (e, _) in versions {
                let with = |c: &Clip| match &e {
                    Some(e) => fx_version(c, e, t),
                    None => Some((c.clone(), 1.0)),
                };
                let Some((ca, k)) = with(&l.clip) else { continue };
                let Some(fa) = decode(project, &ca, l.t_rel, cw, ch) else { continue };
                let ea = l.clip.eval_at(l.t_rel, pw, ph);
                let img = match pair {
                    Some(&(_, bi, ref plan, p)) => {
                        let b = &layers[bi];
                        let Some((cb, _)) = with(&b.clip) else { continue };
                        let Some(fb) = decode(project, &cb, b.t_rel, cw, ch) else { continue };
                        let eb = b.clip.eval_at(b.t_rel, pw, ph);
                        let Some(m) = mix_layers(plan, (&fa, &ea), (&fb, &eb), (cw, ch), (pw, ph), p) else { continue };
                        m
                    }
                    None => place_on_canvas(&fa, &Evaluated { opacity: ea.opacity * l.alpha, ..ea }, cw, ch, pw, ph),
                };
                match (&mut base, pair, e.as_ref().map(|e| e.fx.mask.is_none())) {
                    (None, Some(_), None) => base = Some(img),
                    (Some(b), Some(_), Some(true)) => fold_fx(b, &img, k),
                    // shortcut: masked elements laid on top without mask (the player draws the shape)
                    _ => over(&mut out, &img, k),
                }
            }
            if let Some(b) = base {
                over(&mut out, &b, 1.0);
            }
        }
        out
    }

    /// Reproduce by hand: `CUTLINE_STILL="<project>;<sec>[,<sec>…];<output folder>"`.
    #[test]
    fn still_from_env() {
        let Ok(v) = std::env::var("CUTLINE_STILL") else { return };
        let parts: Vec<&str> = v.split(';').collect();
        let project = Project::from_json(&std::fs::read_to_string(parts[0]).unwrap()).unwrap();
        let reg = cl_plugin::Registry::scan([std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins"))]);
        cl_fx::catalog::load_packs(crate::contents::catalog_packs(&reg, "de"));
        let (cw, ch) = (640, 360);
        for t in parts[1].split(',') {
            let img = render(&project, Time::from_secs_f64(t.parse().unwrap()), cw, ch);
            let out = std::path::Path::new(parts[2]).join(format!("still_{t}.png"));
            image::save_buffer(&out, &img, cw, ch, image::ExtendedColorType::Rgba8).unwrap();
        }
    }
}

/// Preview transitions against the export (N_tr: cube, pixelate with mask, circle open).
#[cfg(test)]
mod transition_tests {
    use super::*;
    use std::path::Path;

    fn load_shipped_packs() {
        let reg = cl_plugin::Registry::scan([Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins"))]);
        cl_fx::catalog::load_packs(crate::contents::catalog_packs(&reg, "de"));
    }

    fn media(dir: &Path, name: &str, src: &str) -> MediaRef {
        let path = dir.join(name);
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-threads", "2", "-f", "lavfi", "-i"])
            .arg(format!("{src}=size=320x180:rate=30:duration=2"))
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(st.success());
        cl_media::probe(&path).unwrap()
    }

    /// 320×180, 30 fps: A (0–1.5 s) → B (1.5–3 s), transition `tr` over 0.6 s (window 1.2–1.8 s);
    /// `mask_b`: B with circle mask; `filter`: filter element over both.
    fn project(dir: &Path, tr: &str, mask_b: bool, filter: Option<&str>) -> Project {
        let a = media(dir, "a.mp4", "testsrc2");
        let b = media(dir, "b.mp4", "smptebars");
        let mut p = Project::new("Übergang");
        (p.settings.width, p.settings.height) = (320, 180);
        p.settings.fps = Rational { num: 30, den: 1 };
        let mut t = Track::new(TrackKind::Video);
        for (i, m) in [&a, &b].into_iter().enumerate() {
            let mut c = Clip::new(
                ClipContent::Media { media: m.id, kind: MediaKind::Video },
                Time::from_secs_f64(1.5 * i as f64),
                Time::from_secs_f64(1.5),
            );
            if i == 0 {
                c.transition_out = Some(TransitionRef { id: tr.into(), duration: Time::from_secs_f64(0.6) });
            } else if mask_b {
                c.fx.mask = Some(Mask::new(MaskShape::Circle));
            }
            t.clips.push(c);
        }
        p.media = vec![a, b];
        p.timeline.tracks.push(t);
        if let Some(f) = filter {
            let mut ft = Track::new(TrackKind::Filter);
            ft.clips.push(Clip::new(ClipContent::Filter(FxRef::new(f)), Time::from_secs_f64(1.0), Time::from_secs_f64(1.0)));
            p.timeline.tracks.push(ft);
        }
        p
    }

    /// Frame `n` of the export (frame-exact)
    fn export_frame(p: &Project, dir: &Path, n: usize) -> Vec<u8> {
        let out = dir.join("out.mp4");
        let job = cl_export::build(p, &cl_export::ExportSettings::from_project(p), &|_| None, &out).unwrap();
        cl_export::run(&job, std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), |_| {}).unwrap();
        let o = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-threads", "2", "-i"])
            .arg(&out)
            .args(["-vf", &format!("select=eq(n\\,{n})"), "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .output()
            .unwrap();
        assert_eq!(o.stdout.len(), 320 * 180 * 4);
        o.stdout
    }

    /// Mean deviation of the colours in 16×16 blocks (encoding and shifted pixel blocks
    /// average out, shape and brightness do not)
    fn block_diff(a: &[u8], b: &[u8]) -> f64 {
        let (w, h, s) = (320usize, 180usize, 16usize);
        let mean = |img: &[u8], bx: usize, by: usize, c: usize| {
            let mut sum = 0u32;
            for y in by * s..((by + 1) * s).min(h) {
                for x in bx * s..(bx + 1) * s {
                    sum += img[(y * w + x) * 4 + c] as u32;
                }
            }
            sum as f64 / (s * s.min(h - by * s)) as f64
        };
        let (mut d, mut n) = (0.0, 0.0);
        for by in 0..h.div_ceil(s) {
            for bx in 0..w / s {
                for c in 0..3 {
                    d += (mean(a, bx, by, c) - mean(b, bx, by, c)).abs();
                    n += 1.0;
                }
            }
        }
        d / n
    }

    fn still_vs_export(tr: &str, mask_b: bool, filter: Option<&str>, t: f64) -> f64 {
        let dir = tempfile::tempdir().unwrap();
        let p = project(dir.path(), tr, mask_b, filter);
        let ex = export_frame(&p, dir.path(), (t * 30.0).round() as usize);
        let pv = still::render(&p, Time::from_secs_f64(t), 320, 180);
        if let Ok(d) = std::env::var("CUTLINE_STILL_DEBUG") {
            let (d, tr) = (Path::new(&d), tr.replace('/', "_"));
            image::save_buffer(d.join(format!("t_{tr}_pv.png")), &pv, 320, 180, image::ExtendedColorType::Rgba8).unwrap();
            image::save_buffer(d.join(format!("t_{tr}_ex.png")), &ex, 320, 180, image::ExtendedColorType::Rgba8).unwrap();
        }
        let d = block_diff(&pv, &ex);
        eprintln!("{tr} bei {t} s: Abweichung Vorschau/Export {d:.2}");
        d
    }

    /// Pixelate with masked clip B (N_tr 32:13): outside the mask A turns to black as in the export,
    /// instead of B showing through without mask.
    #[test]
    fn masked_clip_pixelize_matches_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let d = still_vs_export("pixelize", true, None, 1.6);
        assert!(d < 8.0, "preview differs from the export: {d:.1}");
    }

    /// Crop (moved, shrunk, rotated) alone and in a blended transition with
    /// a different crop on B: preview like export.
    #[test]
    fn crop_matches_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = project(dir.path(), "circleopen", false, None);
        let clips = &mut p.timeline.tracks[0].clips;
        clips[0].crop = Crop { left: 0.3, right: 0.0, top: 0.2, bottom: 0.1 };
        clips[0].transform = Transform { x: 30.0, y: -10.0, scale: 0.7, rotation_deg: 20.0 };
        clips[1].crop = Crop { left: 0.0, right: 0.25, top: 0.0, bottom: 0.3 };
        for t in [0.5f64, 1.5, 2.5] {
            let ex = export_frame(&p, dir.path(), (t * 30.0).round() as usize);
            let pv = still::render(&p, Time::from_secs_f64(t), 320, 180);
            let d = block_diff(&pv, &ex);
            assert!(d < 8.0, "crop at {t} s: preview differs from the export: {d:.1}");
            // without crop in the preview it would be clearly different (checks that the test bites)
            let mut q = p.clone();
            q.timeline.tracks[0].clips.iter_mut().for_each(|c| c.crop = Crop::default());
            let d0 = block_diff(&still::render(&q, Time::from_secs_f64(t), 320, 180), &ex);
            assert!(d0 > 2.0 * d.max(2.0), "at {t} s: without crop {d0:.1}, with {d:.1}");
        }
    }

    /// Split screen from the layout action (crop + transform of two tracks): preview like export.
    #[test]
    fn splitscreen_matches_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = project(dir.path(), "fade", false, None);
        // B on its own track above A, both from 0
        let mut b = p.timeline.tracks[0].clips.pop().unwrap();
        p.timeline.tracks[0].clips[0].transition_out = None;
        b.start = Time::ZERO;
        let mut t = Track::new(TrackKind::Video);
        t.clips.push(b);
        p.timeline.tracks.push(t);
        let before = p.clone();
        let sizes: Vec<(f32, f32)> = p.media.iter().map(|m| (m.width as f32, m.height as f32)).collect();
        let geo = crate::arrange::arrange(320.0, 180.0, &sizes, crate::arrange::Arrange::Side);
        for (t, (tf, k)) in p.timeline.tracks.iter_mut().zip(geo) {
            (t.clips[0].transform, t.clips[0].crop) = (tf, k);
        }
        let ex = export_frame(&p, dir.path(), 15);
        let at = Time::from_secs_f64(0.5);
        let d = block_diff(&still::render(&p, at, 320, 180), &ex);
        assert!(d < 8.0, "split screen: preview differs from the export: {d:.1}");
        let d0 = block_diff(&still::render(&before, at, 320, 180), &ex);
        assert!(d0 > 2.0 * d.max(2.0), "without layout {d0:.1}, with {d:.1}");
    }

    /// Circle open under a filter element (N_tr 26:50): blended like the export (element on
    /// the finished transition), not cross-faded.
    #[test]
    fn circleopen_under_filter_matches_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let d = still_vs_export("circleopen", false, Some("noir"), 1.5);
        assert!(d < 8.0, "preview differs from the export: {d:.1}");
    }

    /// Cube (package shader) after a masked clip under a filter element (N_tr 37:90).
    #[test]
    fn shader_transition_under_filter_matches_export() {
        if !cl_media::ffmpeg_available() || cl_gpu::gpu().is_none() {
            eprintln!("kein FFmpeg oder keine GPU, übersprungen");
            return;
        }
        load_shipped_packs();
        let d = still_vs_export("cutline.uebergaenge/wuerfel", false, Some("noir"), 1.5);
        assert!(d < 8.0, "preview differs from the export: {d:.1}");
    }

    /// Decision "blend or cross-fade": an effect/filter element over the transition is
    /// no longer a reason to cross-fade (before: cube and circle open were missing in N_tr).
    #[test]
    fn elements_above_do_not_force_a_crossfade() {
        let clip = |s: f64, tr: Option<&str>| {
            let mut c = Clip::new(
                ClipContent::Media { media: MediaId::new(), kind: MediaKind::Video },
                Time::from_secs_f64(s),
                Time::from_secs_f64(1.5),
            );
            c.transition_out = tr.map(|id| TransitionRef { id: id.into(), duration: Time::from_secs_f64(0.6) });
            c
        };
        let make = |tr: &str, filter: bool| {
            let mut p = Project::new("x");
            let mut t = Track::new(TrackKind::Video);
            t.clips = vec![clip(0.0, Some(tr)), clip(1.5, None)];
            p.timeline.tracks.push(t);
            if filter {
                let mut ft = Track::new(TrackKind::Filter);
                ft.clips.push(Clip::new(ClipContent::Filter(FxRef::new("noir")), Time::from_secs_f64(1.0), Time::from_secs_f64(1.0)));
                p.timeline.tracks.push(ft);
            }
            p
        };
        let at = Time::from_secs_f64(1.5);
        let pairs = |p: &Project| mix_pairs(p, &layers_in(p, None, at), at).into_iter().map(|x| (x.0, x.1, x.3)).collect::<Vec<_>>();
        for tr in ["circleopen", "pixelize", "fade"] {
            assert_eq!(pairs(&make(tr, true)), vec![(0, 1, 0.5)], "{tr} with filter on top");
        }
        assert_eq!(pairs(&make("circleopen", false)), vec![(0, 1, 0.5)]);
        // plain cross-fade without element: stays the (exact) cross-fade of the display
        assert!(pairs(&make("fade", false)).is_empty());
        // nothing outside the window
        let p = make("circleopen", true);
        assert!(mix_pairs(&p, &layers_in(&p, None, Time::from_secs_f64(1.1)), Time::from_secs_f64(1.1)).is_empty());
    }

    /// If a frame is missing when advancing, the last blended frame of the same transition stays;
    /// too slow during playback holds per transition until paused, a fast frame does
    /// not switch back.
    #[test]
    fn missing_frames_hold_and_slow_sticks_per_transition() {
        let ctx = egui::Context::default();
        let (a, b, c) = (ClipId::new(), ClipId::new(), ClipId::new());
        let mut sp = ShaderPreview::default();
        assert_eq!(sp.step((a, b), false), Step::Skip);
        assert_eq!(sp.step((a, b), true), Step::Mix);
        sp.tex = Some(ctx.load_texture("t", egui::ColorImage::new([1, 1], Color32::BLACK), Default::default()));
        sp.shown = Some((a, b));
        assert_eq!(sp.step((a, b), false), Step::Hold);
        assert_eq!(sp.step((b, c), false), Step::Skip, "other transition: not the old frame");
        let iv = 1.0 / 30.0;
        // a first slow frame (e.g. pipeline) alone does not switch
        sp.note_time((a, b), 0.2, iv);
        assert_eq!(sp.step((a, b), true), Step::Mix);
        sp.note_time((a, b), 0.01, iv);
        sp.note_time((a, b), 0.2, iv);
        assert_eq!(sp.step((a, b), true), Step::Skip);
        sp.note_time((a, b), 0.01, iv);
        assert_eq!(sp.step((a, b), true), Step::Skip, "stays until paused");
        // the next transition starts afresh
        sp.note_time((b, c), 0.2, iv);
        assert_eq!(sp.step((b, c), true), Step::Mix);
    }

    #[test]
    fn background_mix_matches_direct_mix_with_elements() {
        // same computation as before in the UI thread: blended frame, unmasked element
        // included, masked one as its own frame
        let ev = Evaluated { x: 0.0, y: 0.0, scale: 1.0, rotation_deg: 0.0, opacity: 1.0, volume_db: 0.0, crop: Crop::default() };
        let f = |v: u8| std::sync::Arc::new(cl_media::RgbaFrame { width: 32, height: 18, data: [v, 255 - v, 90, 255].repeat(32 * 18) });
        let plan = cl_fx::catalog::transition_plan("circleopen");
        let mask = Mask::new(MaskShape::Circle);
        let job = MixJob {
            id: 7,
            pair: (ClipId::new(), ClipId::new()),
            plan: plan.clone(),
            frames: vec![(f(10), f(200)), (f(60), f(120)), (f(250), f(5))],
            sets: vec![(None, 0.5), (Some(mask), 0.8)],
            ev: (ev, ev),
            size: (32, 18),
            proj: (1920.0, 1080.0),
            p: 0.4,
        };
        let d = run_mix(job);
        assert_eq!((d.id, d.images.len(), d.masked.len()), (7, 2, 1));
        let mix = |a: u8, b: u8| mix_layers(&plan, (&f(a), &ev), (&f(b), &ev), (32, 18), (1920.0, 1080.0), 0.4).unwrap();
        let mut base = mix(10, 200);
        fold_fx(&mut base, &mix(60, 120), 0.5);
        assert_eq!(d.images[0], egui::ColorImage::from_rgba_unmultiplied([32, 18], &base));
        assert_eq!(d.images[1], egui::ColorImage::from_rgba_unmultiplied([32, 18], &mix(250, 5)));
        assert_eq!(d.masked[0], (Some(mask), 0.8));
    }

    #[test]
    fn mix_area_is_limited_while_playing() {
        let px = egui::vec2(2560.0, 1440.0);
        assert_eq!(mix_size(1920.0, 1080.0, px, false), (1920, 1080));
        assert_eq!(mix_size(1920.0, 1080.0, px, true), (960, 540));
        assert_eq!(mix_size(1080.0, 1920.0, egui::vec2(400.0, 700.0), true), (394, 700));
    }

    /// Time per blended frame (1080p project): `cargo test --release -p cl-app mix_time -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn mix_time() {
        load_shipped_packs();
        let ev = Evaluated { x: 0.0, y: 0.0, scale: 1.0, rotation_deg: 0.0, opacity: 1.0, volume_db: 0.0, crop: Crop::default() };
        for (cw, ch) in [(1920, 1080), (1600, 900), (1280, 720), (960, 540)] {
            let f = |v: u8| cl_media::RgbaFrame { width: cw, height: ch, data: [v, 255 - v, 90, 255].repeat((cw * ch) as usize) };
            let (fa, fb) = (f(30), f(200));
            let t0 = std::time::Instant::now();
            for _ in 0..5 {
                place_on_canvas(&fa, &ev, cw, ch, 1920.0, 1080.0);
            }
            eprintln!("{cw}x{ch} Ebene setzen: {:.1} ms", t0.elapsed().as_secs_f64() * 1e3 / 5.0);
            for id in ["circleopen", "pixelize", "cutline.uebergaenge/wuerfel"] {
                let plan = cl_fx::catalog::transition_plan(id);
                let t0 = std::time::Instant::now();
                mix_layers(&plan, (&fa, &ev), (&fb, &ev), (cw, ch), (1920.0, 1080.0), 0.4);
                let first = t0.elapsed();
                let t0 = std::time::Instant::now();
                for i in 0..5 {
                    mix_layers(&plan, (&fa, &ev), (&fb, &ev), (cw, ch), (1920.0, 1080.0), 0.5 + i as f32 * 0.01);
                }
                eprintln!("{cw}×{ch} {id}: erstes {:.1} ms, danach {:.1} ms", first.as_secs_f64() * 1e3, t0.elapsed().as_secs_f64() * 1e3 / 5.0);
            }
        }
    }

    /// Position of the yellow area (middle, left and right edge) relative to the text width in
    /// the preview: the same drawing as in the player, evaluated on the glyphs
    fn karaoke_preview(app: &App, st: &TextStyle, dur: Time, t: f64) -> (f32, f32, f32) {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        let painter = egui::Painter::new(ctx.clone(), egui::LayerId::background(), Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 180.0)));
        app.draw_text(&painter, st, pos2(160.0, 90.0), 1.0, 0.0, 1.0, 320.0, st.karaoke_lit(dur, Time::from_secs_f64(t)));
        let out = ctx.end_pass();
        let hi = st.karaoke.as_ref().unwrap().color;
        let hi = Color32::from_rgba_unmultiplied(hi[0], hi[1], hi[2], hi[3]);
        let (mut all, mut lit) = ((f32::MAX, f32::MIN), (f32::MAX, f32::MIN));
        for s in &out.shapes {
            let egui::Shape::Text(ts) = &s.shape else { continue };
            for g in ts.galley.rows.iter().flat_map(|r| r.glyphs.iter()).filter(|g| !g.chr.is_whitespace()) {
                let (x0, x1) = (ts.pos.x + g.pos.x, ts.pos.x + g.pos.x + g.advance_width);
                all = (all.0.min(x0), all.1.max(x1));
                if ts.galley.job.sections[g.section_index as usize].format.color == hi {
                    lit = (lit.0.min(x0), lit.1.max(x1));
                }
            }
        }
        let fr = |x: f32| (x - all.0) / (all.1 - all.0);
        (fr((lit.0 + lit.1) / 2.0), fr(lit.0), fr(lit.1))
    }

    /// The same in the export frame: yellow columns against all text columns
    fn karaoke_export(img: &[u8]) -> (f32, f32, f32) {
        let (mut all, mut lit) = ((usize::MAX, 0), (usize::MAX, 0));
        for (i, c) in img.chunks_exact(4).enumerate() {
            let x = i % 320;
            let yellow = c[0] > 170 && c[1] > 140 && c[2] < 100;
            if yellow || c.iter().take(3).all(|&v| v > 170) {
                all = (all.0.min(x), all.1.max(x + 1));
            }
            if yellow {
                lit = (lit.0.min(x), lit.1.max(x + 1));
            }
        }
        assert!(lit.0 < lit.1, "no yellow word in the export");
        let fr = |x: usize| (x as f32 - all.0 as f32) / (all.1 - all.0) as f32;
        ((fr(lit.0) + fr(lit.1)) / 2.0, fr(lit.0), fr(lit.1))
    }

    /// Middle of each line (y, project pixels) in the preview: rows of the main text's galley
    fn preview_lines(app: &App, st: &TextStyle) -> Vec<f32> {
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        let painter = egui::Painter::new(ctx.clone(), egui::LayerId::background(), Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 360.0)));
        app.draw_text(&painter, st, pos2(320.0, 180.0), 1.0, 0.0, 1.0, 640.0, None);
        let out = ctx.end_pass();
        let ts = out.shapes.iter().rev().find_map(|s| match &s.shape {
            egui::Shape::Text(ts) => Some(ts.clone()),
            _ => None,
        });
        let ts = ts.unwrap();
        ts.galley.rows.iter().filter(|r| !r.glyphs.is_empty()).map(|r| ts.pos.y + r.rect.center().y).collect()
    }

    /// Middle of each line in the export frame: bands of bright pixels (capitals without descenders,
    /// so the middle of the cap height is measured, not that of the line box)
    fn export_lines(img: &[u8]) -> Vec<f32> {
        let ink: Vec<bool> = (0..360).map(|y| (0..640).any(|x| img[(y * 640 + x) * 4..][..3].iter().all(|&v| v > 160))).collect();
        let mut v = Vec::new();
        let mut start = None;
        for (y, &on) in ink.iter().chain([false].iter()).enumerate() {
            match (on, start) {
                (true, None) => start = Some(y),
                (false, Some(s)) => {
                    v.push((s + y) as f32 / 2.0);
                    start = None;
                }
                _ => {}
            }
        }
        v
    }

    /// Line position and spacing: two-line, three-line and automatically wrapped. The preview sets
    /// with egui (own font), the export with `drawtext`; compared are line count, spacing
    /// of the lines and the middle of the block.
    #[test]
    fn text_lines_match_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let (app, _home, _g) = crate::saving::tests::test_app();
        let dir = tempfile::tempdir().unwrap();
        for text in ["AAA BBB\nCCC DDD", "EINS\nZWEI\nDREI", "HHHH HHHH HHHH HHHH HHHH HHHH HHHH HHHH HHHH HHHH HHHH"] {
            let st = TextStyle { text: text.into(), size: 44.0, shadow: false, ..Default::default() };
            let mut p = Project::new("Zeilen");
            (p.settings.width, p.settings.height) = (640, 360);
            p.settings.fps = Rational { num: 30, den: 1 };
            let mut t = Track::new(TrackKind::Text);
            t.clips.push(Clip::new(ClipContent::Text(st.clone()), Time::ZERO, Time::from_secs_f64(1.0)));
            p.timeline.tracks.push(t);
            let out = dir.path().join("z.mp4");
            let job = cl_export::build(&p, &cl_export::ExportSettings::from_project(&p), &|_| None, &out).unwrap();
            cl_export::run(&job, std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), |_| {}).unwrap();
            let o = cl_media::ffmpeg_command()
                .unwrap()
                .args(["-v", "error", "-i"])
                .arg(&out)
                .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
                .output()
                .unwrap();
            let (pv, ex) = (preview_lines(&app, &st), export_lines(&o.stdout));
            let pitch = |v: &[f32]| (v[v.len() - 1] - v[0]) / (v.len() - 1) as f32;
            let mid = |v: &[f32]| (v[0] + v[v.len() - 1]) / 2.0;
            eprintln!("{text:?}: Vorschau {pv:.1?}, Export {ex:.1?}");
            assert_eq!(pv.len(), ex.len(), "{text:?}: line count");
            assert!(pv.len() >= 2, "{text:?}");
            let (dp, dm) = ((pitch(&pv) - pitch(&ex)).abs(), (mid(&pv) - mid(&ex)).abs());
            eprintln!("{text:?}: Abweichung Zeilenabstand {dp:.1} px, Blockmitte {dm:.1} px (Größe 44)");
            assert!(dp < 2.0 && dm < 5.0, "{text:?}: spacing {dp:.1}, middle {dm:.1}");
        }
    }

    /// Karaoke at three points in time (and „Füllen“): in preview and export the same word is yellow.
    /// The preview uses egui's font, so the position relative to the text width is
    /// compared.
    #[test]
    fn karaoke_word_matches_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let (app, _home, _g) = crate::saving::tests::test_app();
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::new("Karaoke");
        (p.settings.width, p.settings.height) = (320, 180);
        p.settings.fps = Rational { num: 30, den: 1 };
        let words = [0.0, 1.0, 2.0].map(Time::from_secs_f64).to_vec();
        let mut st = TextStyle {
            text: "AAA BBB CCC".into(),
            size: 36.0,
            shadow: false,
            karaoke: Some(Karaoke { color: [255, 221, 0, 255], fill: false, words }),
            ..Default::default()
        };
        let dur = Time::from_secs_f64(3.0);
        let mut t = Track::new(TrackKind::Text);
        t.clips.push(Clip::new(ClipContent::Text(st.clone()), Time::ZERO, dur));
        p.timeline.tracks.push(t);
        for (t, word) in [(0.5f64, 0usize), (1.5, 1), (2.5, 2)] {
            let pv = karaoke_preview(&app, &st, dur, t);
            let ex = karaoke_export(&export_frame(&p, dir.path(), (t * 30.0).round() as usize));
            eprintln!("Karaoke bei {t} s: Vorschau {pv:.2?}, Export {ex:.2?}");
            assert_eq!(((pv.0 * 3.0) as usize, (ex.0 * 3.0) as usize), (word, word), "at {t} s");
            assert!((pv.0 - ex.0).abs() < 0.06, "at {t} s: preview {pv:?}, export {ex:?}");
            // only the one word: nothing colours into the neighbours
            assert!(ex.2 - ex.1 < 0.36 && pv.2 - pv.1 < 0.36, "at {t} s: {pv:?} {ex:?}");
        }
        // „Füllen“: at 1.5 s the first two words are yellow
        st.karaoke.as_mut().unwrap().fill = true;
        p.timeline.tracks[0].clips[0].content = ClipContent::Text(st.clone());
        let pv = karaoke_preview(&app, &st, dur, 1.5);
        let ex = karaoke_export(&export_frame(&p, dir.path(), 45));
        assert!(pv.1 < 0.05 && ex.1 < 0.05 && (0.55..0.72).contains(&pv.2) && (0.55..0.72).contains(&ex.2), "{pv:?} {ex:?}");
    }

    /// Fine checkerboard (2 px) over 2 s, losslessly encoded
    fn checker(dir: &Path) -> MediaRef {
        let path = dir.join("muster.mp4");
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-threads", "2", "-f", "lavfi", "-i", "color=c=black:s=320x180:r=30:d=2"])
            .args(["-vf", "format=gray,geq=lum='255*mod(floor(X/2)+floor(Y/2),2)'", "-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv444p"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(st.success());
        cl_media::probe(&path).unwrap()
    }

    /// One clip (0–2 s) with the checkerboard; censoring moves from left (x 0.3) to right (0.7)
    fn censor_project(dir: &Path, blur: bool) -> Project {
        let m = checker(dir);
        let mut p = Project::new("Zensur");
        (p.settings.width, p.settings.height) = (320, 180);
        p.settings.fps = Rational { num: 30, den: 1 };
        let mut t = Track::new(TrackKind::Video);
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(2.0));
        let key = |t: f64, cx: f32| CensorKey { t: Time::from_secs_f64(t), cx, cy: 0.5, w: 0.3, h: 0.5 };
        c.fx.censor = vec![Censor { blur, path: vec![key(0.0, 0.3), key(2.0, 0.7)], ..Default::default() }];
        t.clips.push(c);
        p.media = vec![m];
        p.timeline.tracks.push(t);
        p
    }

    /// Mean difference of neighbouring pixels (red) in the area – high for the fine pattern
    fn detail(img: &[u8], x0: usize, x1: usize, y0: usize, y1: usize) -> f64 {
        let at = |x: usize, y: usize| img[(y * 320 + x) * 4] as f64;
        let (mut d, mut n) = (0.0, 0.0);
        for y in y0..y1 {
            for x in x0..x1 {
                d += (at(x, y) - at(x + 1, y)).abs() + (at(x, y) - at(x, y + 1)).abs();
                n += 1.0;
            }
        }
        d / n
    }

    /// Fineness per 16×16 block: mean deviation of two frames
    fn detail_diff(a: &[u8], b: &[u8]) -> f64 {
        let (mut d, mut n) = (0.0, 0.0);
        for by in 0..11 {
            for bx in 0..19 {
                let r = (bx * 16, bx * 16 + 16, by * 16, by * 16 + 16);
                d += (detail(a, r.0, r.1, r.2, r.3) - detail(b, r.0, r.1, r.2, r.3)).abs();
                n += 1.0;
            }
        }
        d / n
    }

    /// Export: under the area the fine pattern is gone (pixelate and blur, weakest
    /// level), next to it the picture is unchanged, and the area lies left at 0.2 s, right at 1.8 s.
    #[test]
    fn censor_destroys_the_pattern_only_inside_and_moves() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut plain = censor_project(dir.path(), false);
        plain.timeline.tracks[0].clips[0].fx.censor.clear();
        let orig = [export_frame(&plain, dir.path(), 6), export_frame(&plain, dir.path(), 54)];
        // area at frame 6: x 61–157, at frame 54: x 163–259; y 45–135
        let (left, right) = ((66, 152), (168, 254));
        assert!(detail(&orig[0], left.0, left.1, 50, 130) > 60.0, "pattern arrives in the export");
        for blur in [false, true] {
            let p = censor_project(dir.path(), blur);
            for (n, inside, outside, o) in [(6, left, right, &orig[0]), (54, right, left, &orig[1])] {
                let f = export_frame(&p, dir.path(), n);
                let (di, dout) = (detail(&f, inside.0, inside.1, 50, 130), detail(&f, outside.0, outside.1, 50, 130));
                eprintln!("blur={blur} Bild {n}: Feinheit innen {di:.1}, an der anderen Stelle {dout:.1}");
                assert!(di < 6.0, "blur={blur} frame {n}: pattern still there under the area ({di:.1})");
                assert!(dout > 60.0, "blur={blur} frame {n}: pattern next to the area missing ({dout:.1})");
                // far enough from the area: as without censoring
                let far = if n == 6 { 175..319 } else { 0..145 };
                let diff: f64 = (0..180).flat_map(|y| far.clone().map(move |x| (x, y))).map(|(x, y)| (f[(y * 320 + x) * 4] as f64 - o[(y * 320 + x) * 4] as f64).abs()).sum::<f64>()
                    / (180 * far.len()) as f64;
                assert!(diff < 3.0, "blur={blur} frame {n}: changed outside ({diff:.1})");
            }
        }
    }

    /// Preview against export at two points in time (the area moves in between).
    #[test]
    fn censor_preview_matches_export() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = censor_project(dir.path(), false);
        let mut q = p.clone();
        q.timeline.tracks[0].clips[0].fx.censor.clear();
        for (t, n) in [(0.2, 6), (1.8, 54)] {
            let ex = export_frame(&p, dir.path(), n);
            let pv = still::render(&p, Time::from_secs_f64(t), 320, 180);
            let (d, c) = (detail_diff(&pv, &ex), block_diff(&pv, &ex));
            let d0 = detail_diff(&still::render(&q, Time::from_secs_f64(t), 320, 180), &ex);
            eprintln!("Zensur bei {t} s: Feinheit Vorschau/Export {d:.2} (ohne Zensur {d0:.2}), Farbe {c:.2}");
            assert!(d < 6.0 && c < 8.0, "at {t} s: preview differs from the export ({d:.1}, {c:.1})");
            assert!(d0 > 3.0 * d.max(2.0), "at {t} s: test does not bite ({d0:.1} / {d:.1})");
        }
    }

    /// With crop, scaling and shift of the clip the area stays on the same
    /// place of the source frame – in preview and export.
    #[test]
    fn censor_follows_crop_scale_and_position() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = censor_project(dir.path(), false);
        let c = &mut p.timeline.tracks[0].clips[0];
        c.crop = Crop { left: 0.05, right: 0.05, top: 0.1, bottom: 0.0 };
        c.transform = Transform { x: 40.0, y: 0.0, scale: 0.6, rotation_deg: 0.0 };
        // frame 6: area u 0.19–0.49, v 0.25–0.75 → x 160+40+(u−0.5)·192 = 144–201, y 90+(v−0.5)·108 = 63–117
        let ex = export_frame(&p, dir.path(), 6);
        let pv = still::render(&p, Time::from_secs_f64(0.2), 320, 180);
        for (what, img) in [("Export", &ex), ("Vorschau", &pv)] {
            let (di, dout) = (detail(img, 148, 197, 67, 113), detail(img, 212, 250, 67, 113));
            eprintln!("{what}: Feinheit auf der Fläche {di:.1}, daneben {dout:.1}");
            assert!(di < 6.0, "{what}: area not on the subject ({di:.1})");
            assert!(dout > 15.0, "{what}: censored next to it ({dout:.1})");
        }
        // fineness next to it differs (scaling of the pattern), the colours do not
        let d = block_diff(&pv, &ex);
        assert!(d < 8.0, "preview differs from the export ({d:.1})");
    }
}

impl App {
    /// Take over finished decoder results.
    pub fn receive_decoded(&mut self, ctx: &egui::Context) {
        while let Ok(d) = self.decoder.rx.try_recv() {
            match d {
                Done::Frame { clip, key, frame, exact } => {
                    if if exact { self.core.still_done(lid(clip), key) } else { self.core.still_near(lid(clip), key) } {
                        self.set_frame(ctx, clip, key, &frame);
                    }
                }
                Done::FrameFailed { clip, key, error } => {
                    log::warn!("Decoder: {error}");
                    self.core.still_failed(lid(clip), key);
                }
                Done::Thumb { media, frame } => {
                    let tex = ctx.load_texture(
                        format!("thumb-{media}"),
                        to_color_image(&frame),
                        egui::TextureOptions::LINEAR,
                    );
                    self.thumbs.insert(media, tex);
                }
                Done::Wave { media, stream, peaks } => {
                    self.waves.insert((media, stream), peaks);
                }
                Done::SoundMeta { key, media, bars } => {
                    if self.sound_pending.as_deref() == Some(key.as_str()) {
                        self.sound_pending = None;
                        self.preview_media(key.clone(), media.clone());
                    }
                    crate::assets::put_capped(
                        &mut self.sound_meta,
                        &mut self.sound_meta_req,
                        key,
                        (media, bars),
                        crate::assets::SOUND_META_CAP,
                    );
                }
                Done::FxThumb { key, frame } => {
                    let tex = ctx.load_texture(
                        format!("fx-{key}"),
                        to_color_image(&frame),
                        egui::TextureOptions::LINEAR,
                    );
                    self.fx_thumbs.insert(key, tex);
                }
                Done::ProjectThumb { out } => self.home.thumb_ready(&out),
                Done::Skipped(key) => {
                    self.fx_thumbs_requested.remove(&key);
                }
                Done::Error(e) => log::warn!("Decoder: {e}"),
            }
        }
    }

    fn set_frame(&mut self, ctx: &egui::Context, clip: ClipId, key: u64, f: &cl_media::RgbaFrame) {
        if matches!(self.pick, Some(crate::pro::Pick::Chroma(c)) if c == clip) {
            self.frame_cpu = Some((clip, f.clone()));
        }
        if self.in_shader_transition(clip) || self.shader_preview.wanted.contains(&clip) {
            let cpu = &mut self.shader_preview.cpu;
            if cpu.len() >= 16 && !cpu.contains_key(&clip) {
                cpu.clear();
            }
            cpu.insert(clip, (key, std::sync::Arc::new(f.clone())));
        }
        let img = to_color_image(f);
        match self.frames.get_mut(&clip) {
            Some((k, t)) if t.size() == [f.width as usize, f.height as usize] => {
                t.set(img, egui::TextureOptions::LINEAR);
                *k = key;
            }
            _ => {
                let t =
                    ctx.load_texture(format!("frame-{clip}"), img, egui::TextureOptions::LINEAR);
                self.frames.insert(clip, (key, t));
            }
        }
    }

    /// Does the clip have a shader or blended `xfade` transition (not just a cross-fade) at the
    /// start or end? (Only then keep CPU frames.)
    fn in_shader_transition(&self, clip: ClipId) -> bool {
        let shader = |c: &Clip| {
            c.transition_out.as_ref().is_some_and(|t| {
                mixes(&cl_fx::catalog::transition_plan(&t.id))
            })
        };
        self.project.timeline.tracks.iter().any(|tr| {
            tr.clips.iter().enumerate().any(|(i, c)| {
                c.id == clip && (shader(c) || i.checked_sub(1).and_then(|p| tr.clips.get(p)).is_some_and(shader))
            })
        })
    }

    /// If the playhead is in a transition the preview blends (`mix_pairs`), blend A and B like
    /// the export (`mix_layers`), plus per effect element on top its versions of A
    /// and B. If a frame is missing, the last blended frame of this transition stays (the missing
    /// one requests a redraw when it arrives); if there was none yet or the transition is
    /// too slow during playback: cross-fade.
    fn shader_mix(&mut self, ctx: &egui::Context, layers: &[Layer], canvas_px: egui::Vec2) {
        let sp = &mut self.shader_preview;
        sp.current = None;
        let mixer = sp.mixer.get_or_insert_with(|| Mixer::spawn(ctx.clone()));
        while let Ok(d) = mixer.rx.try_recv() {
            sp.pending = None;
            sp.ready = Some(d);
        }
        if !sp.warmed || (self.playing && !sp.was_playing) {
            warm_shaders(&self.project);
            sp.warmed = true;
        }
        sp.was_playing = self.playing;
        if !self.playing {
            (sp.slow, sp.slow_pair) = (Vec::new(), None);
        }
        let s = &self.project.settings;
        let (pw, ph) = (s.width.max(2) as f32, s.height.max(2) as f32);
        let t = self.playhead;
        let div = self.play_quality().0 as f32;
        let mut wanted = Vec::new();
        for (ai, bi, plan, p) in mix_pairs(&self.project, layers, t) {
            let (a, b) = (&layers[ai], &layers[bi]);
            let pair = (a.clip.id, b.clip.id);
            // frame pairs: the clips themselves, then their versions with the effect elements on top
            let mut sets = vec![(pair.0, pair.1, None, 1.0)];
            for e in fx_elements_over(&self.project, pair.0).iter().filter(|e| e.contains(t)) {
                let Some((_, k)) = fx_version(&a.clip, e, t) else { continue };
                let d = e.id.0.as_u128();
                sets.push((pair.0.derived(d), pair.1.derived(d), e.fx.mask, k));
            }
            wanted.extend(sets.iter().flat_map(|x| [x.0, x.1]));
            let sp = &mut self.shader_preview;
            let frames: Option<Vec<_>> = sets.iter().map(|x| Some((sp.cpu.get(&x.0)?.clone(), sp.cpu.get(&x.1)?.clone()))).collect();
            match sp.step(pair, frames.is_some()) {
                Step::Skip => continue,
                Step::Hold => {
                    sp.current = Some(pair);
                    break;
                }
                Step::Mix => {}
            }
            let frames = frames.unwrap_or_default();
            let (ea, eb) = (a.clip.eval_at(a.t_rel, pw, ph), b.clip.eval_at(b.t_rel, pw, ph));
            let (cw, ch) = mix_size(pw, ph, canvas_px / div, self.playing);
            // other frames, other transition, other position or area: blend anew
            let id = {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (plan.xfade, &plan.shader.as_ref().map(|s| &s.0), p.to_bits(), cw, ch).hash(&mut h);
                for e in [&ea, &eb] {
                    let k = e.crop;
                    [e.x, e.y, e.scale, e.rotation_deg, e.opacity, k.left, k.right, k.top, k.bottom].map(f32::to_bits).hash(&mut h);
                }
                for ((ka, _), (kb, _)) in &frames {
                    (ka, kb).hash(&mut h);
                }
                h.finish()
            };
            // take over the finished blend of this transition
            if let Some(d) = sp.ready.take_if(|d| d.pair == pair) {
                let fps = self.project.settings.fps;
                let interval = fps.den.max(1) as f64 / fps.num.max(1) as f64;
                if self.core.mode() == Mode::Spielen {
                    sp.note_time(pair, d.secs, interval);
                }
                if crate::app::perf() {
                    crate::applog::perf_line!("perf: Übergang gemischt ({}×{}, {} Bild(er)) in {:.1} ms (im Hintergrund)", cw, ch, sets.len(), d.secs * 1e3);
                }
                sp.last = Some(d.id);
                if !d.images.is_empty() {
                    // reuse existing textures: blended frame, then per effect element
                    let mut old = sp.tex.take().into_iter().chain(sp.fx.drain(..).map(|f| f.0));
                    let mut texs: Vec<egui::TextureHandle> = d
                        .images
                        .into_iter()
                        .enumerate()
                        .map(|(i, ci)| match old.next() {
                            Some(mut t) => {
                                t.set(ci, egui::TextureOptions::LINEAR);
                                t
                            }
                            None => ctx.load_texture(format!("shader-mix-{i}"), ci, egui::TextureOptions::LINEAR),
                        })
                        .collect();
                    drop(old);
                    let rest = texs.split_off(1);
                    sp.tex = texs.pop();
                    sp.fx = rest.into_iter().zip(d.masked).map(|(t, (m, k))| (t, m, k)).collect();
                    sp.shown = Some(pair);
                }
            }
            // other frames, other transition, other position or area: blend anew (in the background)
            if sp.last != Some(id) && sp.pending.is_none() {
                let job = MixJob {
                    id,
                    pair,
                    plan: plan.clone(),
                    frames: frames.iter().map(|((_, a), (_, b))| (a.clone(), b.clone())).collect(),
                    sets: sets[1..].iter().map(|x| (x.2, x.3)).collect(),
                    ev: (ea, eb),
                    size: (cw, ch),
                    proj: (pw, ph),
                    p,
                };
                if let Some(m) = &sp.mixer {
                    if m.tx.send(job).is_ok() {
                        sp.pending = Some(id);
                    }
                }
            }
            // no blended frame of this transition yet: cross-fade until then
            if sp.shown != Some(pair) || sp.tex.is_none() {
                continue;
            }
            sp.current = Some(pair);
            break;
        }
        self.shader_preview.ready = None;
        self.shader_preview.wanted = wanted;
    }

    /// Signal "waiting for frame" for the wait line (see [`crate::playcore::PlayCore::waiting`]).
    pub(crate) fn waiting_for_frame(&self) -> bool {
        self.core.waiting()
    }

    /// Preview quality now: (divisor of the decode size, half frame rate). During playback the
    /// level of the automatic quality, when stopped the chosen one.
    pub(crate) fn play_quality(&self) -> (u32, bool) {
        let steps = self.quality_steps();
        if self.core.playing() {
            steps[self.core.quality.level.min(steps.len() - 1)]
        } else {
            steps[0]
        }
    }

    /// Fingerprint of a clip's effects: computed once per edit state (before, each
    /// UI frame serialized the effects of each layer to JSON). `cacheable` = unchanged
    /// from the project (not being dragged, no eyedropper).
    fn fx_hash(&mut self, id: ClipId, c: &Clip, cacheable: bool) -> u64 {
        let calc = || fx_hash_of(c);
        if !cacheable {
            return calc();
        }
        if self.fx_hashes.0 != self.revision {
            self.fx_hashes = (self.revision, Default::default());
        }
        *self.fx_hashes.1.entry(id).or_insert_with(calc)
    }

    fn layers_at(&self, t: Time) -> Vec<Layer> {
        layers_in(&self.project, self.live.as_ref(), t)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn frame_key(
        id: ClipId,
        c: &Clip,
        src: Time,
        w: u32,
        h: u32,
        t_rel: Time,
        animated_fx: bool,
        fx: u64,
        fps: Rational,
    ) -> u64 {
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        (id, src, w, h, fps.num, fps.den, fx).hash(&mut hs);
        // timing is part of the key: moved/trimmed clips need a new stream
        (c.start, c.duration, c.source_in, c.speed.to_bits()).hash(&mut hs);
        if animated_fx {
            t_rel.hash(&mut hs);
        }
        hs.finish() | 1
    }

    /// Request frames for all visible media layers or fetch them from streams.
    ///
    /// Smooth playback: a clip's stream runs without interruption. Effect/filter elements
    /// on top run as their own version of the clip (own stream), which is laid on top when drawing with its
    /// strength and possibly only inside the mask. Streams for clips and
    /// elements about to begin are opened ahead so nothing hitches at cuts.
    fn update_layers(&mut self, ctx: &egui::Context, layers: &[Layer], canvas_px: egui::Vec2) {
        // During playback the decode size stays fixed (switching to the large player or
        // resizing the window does not restart the streams); when stopped it follows the view.
        let canvas_px = if self.playing {
            *self.play_canvas_px.get_or_insert(canvas_px)
        } else {
            self.play_canvas_px = None;
            canvas_px
        };
        // attach streams opened in the background (the core discards stale ones)
        let now = self.core_now();
        while let Ok((o, fps, alias, s)) = self.starter.rx.try_recv() {
            match s {
                // preroll: keep only if it is still wanted (otherwise it ends when discarded)
                _ if o.gen == PREROLL_GEN => {
                    if let (Some(p), Some(s)) = (self.preroll.as_mut().filter(|p| p.spec.id == o.id && p.spec.key == o.key && p.spec.at == o.at), s) {
                        p.stream = Some((fps, s));
                    }
                }
                Some(s) => {
                    self.core.attach_alias(o, fps, s, now, alias);
                }
                None => self.core.open_failed(o, now),
            }
        }
        // eyedropper: show the frame without chroma key so the colour stays visible
        let pick = match self.pick {
            Some(crate::pro::Pick::Chroma(id)) => Some(id),
            _ => None,
        };
        let needs = needs(&self.project, layers, self.playhead, self.playing, |c| {
            if pick == Some(c.id) {
                c.fx.chroma = None;
            }
        });
        let mut visible = Vec::with_capacity(needs.len());
        let mut overlays = Vec::new();
        for n in &needs {
            visible.push(n.id);
            if let Some((base, mk, k)) = n.overlay {
                overlays.push((base, n.id, mk, k));
            }
            self.request_frame(ctx, n.id, &n.clip, &n.media, &n.layer, canvas_px, n.prefetch);
        }
        self.fx_overlays = overlays;
        self.update_preroll(ctx, &needs, canvas_px);
        // frames of clips no longer under the playhead are discarded by `receive_decoded`
        // (when scrubbing across many tracks dozens came at once otherwise); if the playhead
        // returns, the frame comes again from the decoder's cache. Likewise the
        // textures: otherwise a large project held one per clip after scrubbing (1–8 MB each at preview size).
        self.core.visible(&visible.iter().map(|id| lid(*id)).collect::<Vec<_>>());
        self.frames.retain(|id, _| visible.contains(id));
    }

    /// Preroll when stopped: if the playhead rests [`PREROLL_REST`] on a heavy layer (original above
    /// 1080p or high frame rate, see `preview::stream_hw`) without proxy, its stream is already
    /// opened, exactly as Play will request it; Play attaches it instead of seeking anew from the keyframe
    /// (4K: ~1 s). At most one (one hardware decoder); every change of place,
    /// edit, project, quality or preview size changes the wish and discards it, as do
    /// playback, pause and a running export.
    fn update_preroll(&mut self, ctx: &egui::Context, needs: &[Need], canvas_px: egui::Vec2) {
        let wish = if self.core.mode() == Mode::Stand && !self.playing { self.preroll_wish(needs, canvas_px) } else { None };
        let Some((spec, i, (w, h), sfps)) = wish else {
            self.preroll = None;
            return;
        };
        let now = self.core_now();
        match &mut self.preroll {
            Some(p) if p.spec == spec => {
                if p.sent {
                    return;
                }
                if now - p.since < PREROLL_REST {
                    // a quiet UI does not draw: check once more after the rest time
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(PREROLL_REST - (now - p.since) + 0.01));
                    return;
                }
                if self.export.current().is_some() {
                    return;
                }
                p.sent = true;
                let n = &needs[i];
                let o = crate::playcore::Open { id: spec.id, key: spec.key, at: spec.at, gen: PREROLL_GEN, seq: 0 };
                self.starter.send(open_job(o, &n.clip, &n.media, n.media.path.clone(), (w, h), sfps, spec.alias));
            }
            _ => {
                self.preroll = Some(Preroll { spec, since: now, sent: false, stream: None });
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(PREROLL_REST + 0.01));
            }
        }
    }

    /// The layer for the preroll and what Play will request for it (as in `request_frame`):
    /// (wish, index in `needs`, decode size, stream frame rate)
    fn preroll_wish(&mut self, needs: &[Need], canvas_px: egui::Vec2) -> Option<(PrerollSpec, usize, (u32, u32), Rational)> {
        let i = needs.iter().position(|n| {
            n.prefetch.is_none() && n.overlay.is_none() && streamable(&n.clip) && crate::preview::stream_hw(&n.media, &n.media.path) && self.presence.get(&n.media.path)
        })?;
        let (id, c, m) = (needs[i].id, &needs[i].clip, &needs[i].media);
        let fps = self.project.settings.fps;
        if self.settings.use_proxies {
            // no finished proxy (per search) and no parts: Play would otherwise take the
            if self.proxies.cached(m.id, (self.settings.proxy_h, fps)) != Some(None) {
                return None;
            }
            // choice of source as on Play, without changing the remembered choice
            let prev = self.proxy_picks.get(&lid(id)).copied();
            let pick = self.stream_pick(lid(id), c, m, self.playhead);
            match prev {
                Some(p) => self.proxy_picks.insert(lid(id), p),
                None => self.proxy_picks.remove(&lid(id)),
            };
            if pick != crate::proxy::Pick::Whole {
                return None;
            }
        }
        let (div, half) = self.play_quality();
        let sfps = if half { Rational::new(fps.num, fps.den * 2) } else { fps };
        let cacheable = self.live.as_ref().is_none_or(|lv| lv.id != needs[i].layer.clip.id)
            && !matches!(self.pick, Some(crate::pro::Pick::Chroma(p)) if p == needs[i].layer.clip.id);
        let fxh = self.fx_hash(id, c, cacheable);
        let (key, w, h) = stream_spec(id, c, m, canvas_px, div as f32, sfps, fxh);
        let alias = stream_alias(c, m, (w, h), sfps, fxh);
        Some((PrerollSpec { id: lid(id), key, alias, at: self.playhead, revision: self.revision }, i, (w, h), sfps))
    }

    /// Request a frame for the layer (stream during playback, otherwise single frame).
    #[allow(clippy::too_many_arguments)]
    fn request_frame(
        &mut self,
        ctx: &egui::Context,
        id: ClipId,
        c: &Clip,
        m: &MediaRef,
        l: &Layer,
        canvas_px: egui::Vec2,
        prefetch_at: Option<Time>,
    ) {
        let (div, half) = self.play_quality();
        let div = div as f32;
        let fps = self.project.settings.fps;
        // automatic quality, last level: preview with half frame rate
        let stream_fps = if half { Rational::new(fps.num, fps.den * 2) } else { fps };
        if !matches!(&c.content, ClipContent::Media { .. }) {
            return;
        }
        let cacheable = self.live.as_ref().is_none_or(|lv| lv.id != l.clip.id)
            && !matches!(self.pick, Some(crate::pro::Pick::Chroma(p)) if p == l.clip.id);
        let fxh = self.fx_hash(id, c, cacheable);
        let now = self.core_now();
        // also during transitions: the stream opened ahead begins exactly at the clip start
        if self.core.streaming() && streamable(c) {
            let (key, w, h) = stream_spec(id, c, m, canvas_px, div, stream_fps, fxh);
            // source (original/proxy parts) in the key: a change opens a replacement stream
            let pick = self.stream_pick(lid(id), c, m, prefetch_at.unwrap_or(self.playhead));
            let key = key ^ pick.tag();
            // in the fingerprint too: after splitting no donor stream over other parts
            let alias = stream_alias(c, m, (w, h), stream_fps, fxh) ^ pick.tag();
            let want = Want {
                id: lid(id),
                key,
                start: want_start(c, pick, self.playhead, prefetch_at),
                end: c.end(),
                prefetch: prefetch_at.is_some(),
                alias,
            };
            // present per background check (no file access per layer and UI frame: on
            // a sleeping NAS it blocked the UI thread)
            if self.presence.get(&m.path) {
                // The core says whether and from where: first stream, replacement after a change (the old one
                // keeps showing until the new one delivers), catching up, stalled stream
                if let Some(o) = self.core.want(&want, now) {
                    let rev = self.revision;
                    if let Some((sfps, s)) = self.preroll.take_if(|p| preroll_fits(&p.spec, &o, alias, rev)).and_then(|p| p.stream) {
                        self.core.attach_alias(o, sfps, s, now, alias);
                    } else {
                        let (path, offset) = self.stream_source(m, h, pick);
                        let mut job = open_job(o, c, m, path, (w, h), stream_fps, alias);
                        job.source_at = Time((job.source_at - offset).0.max(0));
                        self.starter.send(job);
                    }
                }
            }
            if prefetch_at.is_some() {
                return;
            }
            let primary = id == l.clip.id;
            if let Some(f) = self.core.pull(lid(id), now, primary) {
                if primary && self.core.mode() == Mode::Spielen {
                    self.perf_frame_shown();
                }
                self.set_frame(ctx, id, key, &f);
            }
            return;
        }
        if prefetch_at.is_some() {
            return;
        }
        let (w, h) = decode_size(c, m, canvas_px, div);
        let src = still_src(c, m, l.t_rel, fps);
        let animated_fx = c.fx.effects.iter().any(|e| {
            matches!(
                e.id.as_str(),
                "shake"
                    | "zoom-pulse"
                    | "glitch"
                    | "rainbow"
                    | "strobe"
                    | "flash"
                    | "old-film"
                    | "warm-leak"
            )
        });
        let key = Self::frame_key(id, c, src, w, h, l.t_rel, animated_fx, fxh, fps);
        // missing file (background check): do not request at all, otherwise the job would stay open
        if self.presence.get(&m.path) && self.core.still(lid(id), key, now) {
            // check the LUT file only when requesting, not per UI frame
            let lut = c.fx.lut.as_deref().filter(|p| std::path::Path::new(p).is_file()).map(cl_media::escape_path);
            let graph = cl_fx::preview_graph(c, w, h, l.t_rel.as_secs_f64(), None, lut);
            let (path, at) = self.still_source(m, h, src);
            let window = reverse_window(c, m, path == m.path, graph.is_none(), self.core.playing() && self.core.rate() > 0.0, fps);
            self.decoder.send(Job::Frame {
                clip: id,
                key,
                path,
                at,
                w,
                h,
                graph,
                window,
            });
        }
    }

    pub fn player(&mut self, ui: &mut egui::Ui) {
        let t_player = std::time::Instant::now();
        let s = self.project.settings.clone();
        let (pw, ph) = (s.width.max(2) as f32, s.height.max(2) as f32);
        let controls_h = 34.0;
        let avail = ui.available_size() - vec2(0.0, controls_h);
        let fit_scale = ((avail.x - 16.0) / pw).min((avail.y - 16.0) / ph).max(0.01);
        let (area, area_resp) = ui.allocate_exact_size(avail, egui::Sense::click_and_drag());
        self.player_zoom_input(ui, &area_resp, area, fit_scale, pw, ph);
        let scale = fit_scale * self.player_zoom;
        let canvas_size = vec2(pw * scale, ph * scale);
        let canvas = Rect::from_center_size(area.center() + self.player_pan, canvas_size);
        let ppp = ui.ctx().pixels_per_point();
        let layers = self.layers_at(self.playhead);
        self.update_layers(ui.ctx(), &layers, canvas_size * ppp);
        self.shader_mix(ui.ctx(), &layers, canvas_size * ppp);

        let painter = ui.painter_at(area);
        painter.rect_filled(canvas, 0.0, Color32::BLACK);
        let content = painter.with_clip_rect(canvas.intersect(area));
        let regions = self.draw_layers(&content, canvas, scale, &layers);
        // waiting for a frame: the last one stays, below it a thin running line. If nothing comes after
        // 8 s (file unreadable, removed medium), the line ends.
        let wait_id = egui::Id::new("player-wait");
        let waiting = self.waiting_for_frame();
        let now = ui.input(|i| i.time);
        let since = waiting.then(|| ui.data(|d| d.get_temp::<f64>(wait_id)).unwrap_or(now));
        ui.data_mut(|d| match since {
            Some(s) => d.insert_temp(wait_id, s),
            None => d.remove::<f64>(wait_id),
        });
        if theme::loading(ui.ctx(), wait_id, since.is_some_and(|s| now - s < 8.0)) {
            let vis = canvas.intersect(area);
            crate::widgets::busy_line(&painter, Rect::from_min_max(pos2(vis.left(), vis.bottom() - 2.0), vis.max));
        }
        if crate::app::perf() {
            let us = t_player.elapsed().as_micros() as u64;
            PERF_PLAYER_SUM_US.fetch_add(us, std::sync::atomic::Ordering::Relaxed);
            PERF_PLAYER_MAX_US.fetch_max(us, std::sync::atomic::Ordering::Relaxed);
        }
        // drop from the asset area directly in the player
        if let Some(payload) = area_resp.dnd_release_payload::<DragAsset>() {
            let on = area_resp
                .hover_pos()
                .and_then(|p| regions.iter().rev().find(|r| r.contains(p)).map(|r| r.clip));
            self.drop_asset(&payload, self.playhead, None, on);
        } else if area_resp.dnd_hover_payload::<DragAsset>().is_some() {
            painter.rect_stroke(
                canvas,
                0.0,
                egui::Stroke::new(2.0_f32, theme::ACCENT()),
                egui::StrokeKind::Inside,
            );
        }
        if area_resp.clicked() || area_resp.drag_started() {
            crate::app::release_text_focus(ui.ctx());
        }
        if !self.player_pick(ui, &area_resp, &regions) && !self.censor_gizmo(ui, &area_resp, &regions) {
            self.player_interact(ui, &area_resp, canvas, scale, &regions);
        }
        // platform controls (top ~10 %, bottom ~20 %, right ~12 %): preview only
        if self.safe_area && ph > pw {
            let (w, h) = (canvas.width(), canvas.height());
            let zones = [
                Rect::from_min_size(canvas.min, vec2(w, h * 0.10)),
                Rect::from_min_max(canvas.min + vec2(0.0, h * 0.80), canvas.max),
                Rect::from_min_max(canvas.min + vec2(w * 0.88, h * 0.10), canvas.max - vec2(0.0, h * 0.20)),
            ];
            let line = egui::Stroke::new(1.0_f32, Color32::from_white_alpha(70));
            for z in zones {
                content.rect_filled(z, 0.0, Color32::from_black_alpha(70));
                // hatching
                let hatch = content.with_clip_rect(z.intersect(canvas.intersect(area)));
                let mut x = z.left() - z.height();
                while x < z.right() {
                    hatch.line_segment([pos2(x, z.bottom()), pos2(x + z.height(), z.top())], egui::Stroke::new(1.0_f32, Color32::from_white_alpha(18)));
                    x += 12.0;
                }
                content.rect_stroke(z, 0.0, line, egui::StrokeKind::Inside);
            }
        }
        area_resp.context_menu(|ui| self.player_menu(ui));
        if self.voiceover.recorder.is_some() {
            painter.circle_filled(canvas.left_top() + vec2(14.0, 14.0), 5.0, theme::DANGER);
            painter.text(
                canvas.left_top() + vec2(24.0, 14.0),
                egui::Align2::LEFT_CENTER,
                crate::tr("REC"),
                egui::FontId::proportional(theme::FS_UI),
                Color32::WHITE,
            );
        }
        if self.project.duration() == Time::ZERO {
            painter.text(
                canvas.center() - vec2(0.0, 10.0),
                egui::Align2::CENTER_CENTER,
                icon::FILM_SLATE,
                egui::FontId::proportional(26.0), // icon glyph in the empty state
                theme::TEXT_FAINT(),
            );
            painter.text(
                canvas.center() + vec2(0.0, 18.0),
                egui::Align2::CENTER_CENTER,
                crate::tr(self.t("drop-hint")),
                egui::FontId::proportional(theme::FS_TEXT),
                theme::TEXT_DIM(),
            );
        }
        self.player_controls(ui, controls_h);
    }

    /// Draw all layers in `canvas` (player and external preview window).
    fn draw_layers(
        &self,
        content: &egui::Painter,
        canvas: Rect,
        scale: f32,
        layers: &[Layer],
    ) -> Vec<Region> {
        let s = &self.project.settings;
        let (pw, ph) = (s.width.max(2) as f32, s.height.max(2) as f32);
        let mut regions: Vec<Region> = Vec::new();
        // lowest shown frame: stays while the next clip has no frame yet
        // (only in the same project: another never shows the previous one's frame)
        let held_id = egui::Id::new("player-held-frame");
        let held_for = egui::Id::new(&self.project_path).value();
        let mut bottom = true;
        for l in layers {
            let c = &l.clip;
            let mut ev = c.eval_at(l.t_rel, pw, ph);
            if let Some((id, lt)) = self.gizmo.live {
                if id == c.id {
                    ev.x = lt.x;
                    ev.y = lt.y;
                    ev.scale = lt.scale;
                    ev.rotation_deg = lt.rotation_deg;
                }
            }
            if let Some((_, k)) = self.gizmo.live_crop.filter(|l| l.0 == c.id) {
                ev.crop = k;
            }
            let tr = Transform {
                x: ev.x,
                y: ev.y,
                scale: ev.scale,
                rotation_deg: ev.rotation_deg,
            };
            let alpha = (ev.opacity * l.alpha).clamp(0.0, 1.0);
            let center = canvas.center() + vec2(ev.x, ev.y) * scale;
            let angle = ev.rotation_deg.to_radians();
            match &c.content {
                ClipContent::Media { .. } => {
                    // shader transition: the blended frame (finished layers on the whole area) instead of A,
                    // B is dropped
                    let mixed = match (self.shader_preview.current, &self.shader_preview.tex) {
                        (Some((_, b)), _) if b == c.id => continue,
                        (Some((a, _)), Some(t)) if a == c.id => Some(t),
                        _ => None,
                    };
                    if let Some(t) = mixed {
                        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                        content.image(t.id(), canvas, uv, Color32::WHITE);
                        // effect elements on top: their blended version (as the export on the
                        // finished transition)
                        let corners = [canvas.left_top(), canvas.right_top(), canvas.right_bottom(), canvas.left_bottom()];
                        for (fx_tex, mk, k) in &self.shader_preview.fx {
                            draw_fx(content, t.id(), fx_tex.id(), corners, corners, mk, *k, canvas);
                        }
                    }
                    let Some(tex) = self.frames.get(&c.id).map(|f| &f.1) else {
                        if std::mem::take(&mut bottom) && mixed.is_none() {
                            let held = content.ctx().data(|d| d.get_temp::<(u64, egui::TextureHandle)>(held_id));
                            if let Some((_, h)) = held.filter(|h| h.0 == held_for) {
                                let hs = h.size_vec2();
                                let fit = (canvas.width() / hs.x).min(canvas.height() / hs.y);
                                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                                content.image(h.id(), Rect::from_center_size(canvas.center(), hs * fit), uv, Color32::WHITE);
                            }
                        }
                        continue;
                    };
                    if std::mem::take(&mut bottom) {
                        content.ctx().data_mut(|d| d.insert_temp(held_id, (held_for, tex.clone())));
                    }
                    let ts = tex.size_vec2();
                    let fit = (canvas.width() / ts.x).min(canvas.height() / ts.y) * ev.scale;
                    let region = Region {
                        clip: c.id,
                        center,
                        size: ts * fit,
                        angle,
                        tr,
                        crop: ev.crop,
                    };
                    if mixed.is_some() {
                        regions.push(region);
                        continue;
                    }
                    // crop as a section of the texture; the rest stays in its place
                    let k = ev.crop;
                    let uvs = [(k.left, k.top), (1.0 - k.right, k.top), (1.0 - k.right, 1.0 - k.bottom), (k.left, 1.0 - k.bottom)];
                    let shown = uvs.map(|(u, v)| region.corner(2.0 * u - 1.0, 2.0 * v - 1.0));
                    let mut mesh = egui::Mesh::with_texture(tex.id());
                    let col = Color32::from_white_alpha((alpha * 255.0) as u8);
                    for (pos, (u, v)) in shown.into_iter().zip(uvs) {
                        mesh.vertices.push(egui::epaint::Vertex { pos, uv: pos2(u, v), color: col });
                    }
                    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
                    content.add(egui::Shape::mesh(mesh));
                    // masked effect/filter elements on top: second version only inside the mask
                    let corners = [
                        region.corner(-1.0, -1.0),
                        region.corner(1.0, -1.0),
                        region.corner(1.0, 1.0),
                        region.corner(-1.0, 1.0),
                    ];
                    for (base, did, mk, k) in &self.fx_overlays {
                        if *base != c.id || *k <= 0.001 {
                            continue;
                        }
                        let Some((_, fx_tex)) = self.frames.get(did) else {
                            continue;
                        };
                        draw_fx(content, tex.id(), fx_tex.id(), corners, shown, mk, alpha * k, canvas);
                    }
                    regions.push(region);
                }
                ClipContent::Effect(_) | ClipContent::Filter(_) => {
                    // show the mask of the selected element as an outline
                    if self.selected == Some(c.id) {
                        if let Some(mk) = &c.fx.mask {
                            let (polys, _) = crate::fx_mask::polygons(mk, canvas);
                            for poly in polys {
                                content.add(egui::Shape::closed_line(
                                    poly,
                                    egui::Stroke::new(1.5_f32, theme::ACCENT()),
                                ));
                            }
                        }
                    }
                }
                ClipContent::Text(st) => {
                    let size = self.draw_text(
                        content,
                        st,
                        center,
                        ev.scale * scale,
                        angle,
                        alpha,
                        canvas.width(),
                        st.karaoke_lit(c.duration, l.t_rel),
                    );
                    regions.push(Region {
                        clip: c.id,
                        center,
                        size,
                        angle,
                        tr,
                        crop: Crop::default(),
                    });
                }
            }
        }
        regions
    }

    /// Preview in its own window (e.g. on the second screen, F11 = full screen).
    pub fn external_preview(&mut self, ctx: &egui::Context) {
        let init = egui::Id::new("ext-preview-init");
        if !self.ext_preview {
            ctx.data_mut(|d| d.remove::<u8>(init));
            return;
        }
        let vid = egui::ViewportId::from_hash_of("cutline-preview");
        // in the first frames after opening explicitly bring it to the front: a setting in the
        // builder has no effect on Windows, the window would lie behind the main window
        let n = ctx.data(|d| d.get_temp::<u8>(init)).unwrap_or(0);
        if n < 4 {
            ctx.data_mut(|d| d.insert_temp(init, n + 1));
            ctx.send_viewport_cmd_to(vid, egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            ctx.send_viewport_cmd_to(vid, egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
        let mut builder = egui::ViewportBuilder::default()
            .with_title(crate::tr("Cutline – Vorschau"))
            .with_inner_size([960.0, 540.0])
            .with_min_inner_size([320.0, 180.0]);
        // open on the main window's screen first; drag it to the second from there
        // only on opening: a permanently set position pins the window
        if let Some(r) = ctx.input(|i| i.viewport().outer_rect).filter(|_| n < 4) {
            builder = builder.with_position(r.min + vec2(80.0, 80.0));
        }
        let mut ext_full = self.ext_full;
        let mut toggle_full = false;
        ctx.show_viewport_immediate(vid, builder, |ctx, _| {
            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(Color32::BLACK))
                .show(ctx, |ui| {
                    let s = self.project.settings.clone();
                    let (pw, ph) = (s.width.max(2) as f32, s.height.max(2) as f32);
                    let area = ui.max_rect();
                    let scale = (area.width() / pw).min(area.height() / ph);
                    let canvas =
                        Rect::from_center_size(area.center(), vec2(pw * scale, ph * scale));
                    let layers = self.layers_at(self.playhead);
                    let painter = ui.painter_at(area);
                    self.draw_layers(&painter.with_clip_rect(canvas), canvas, scale, &layers);
                    let r = ui.interact(area, ui.id().with("ext"), egui::Sense::click());
                    if r.double_clicked() {
                        toggle_full = true;
                    }
                });
            if ctx.input(|i| i.key_pressed(egui::Key::F11) || (ext_full && i.key_pressed(egui::Key::Escape))) {
                toggle_full = true;
            }
            // full screen as a borderless, maximized window: real full screen of a second
            // viewport makes egui/winit hang on Windows.
            if toggle_full {
                let full = !ext_full;
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(!full));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(full));
                ext_full = full;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                self.toggle_play();
            }
            if ctx.input(|i| i.viewport().close_requested()) {
                self.ext_preview = false;
                ext_full = false;
            }
        });
        self.ext_full = ext_full;
        if self.playing {
            ctx.request_repaint_of(vid);
        }
    }

    /// Text as in the export: box, outline, shadow, alignment, line spacing; `lit`: words
    /// in the karaoke colour. Returns the size of the text block (screen).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &self,
        p: &egui::Painter,
        st: &TextStyle,
        center: egui::Pos2,
        k: f32,
        angle: f32,
        alpha: f32,
        canvas_w: f32,
        lit: Option<std::ops::Range<usize>>,
    ) -> egui::Vec2 {
        let txt = if st.uppercase {
            st.text.to_uppercase()
        } else {
            st.text.clone()
        };
        let px = (st.size * k).max(2.0);
        let font = egui::FontId::new(px, self.font_family(st.font_family.as_deref()));
        let wrap = if st.auto_wrap {
            canvas_w * 0.9
        } else {
            f32::INFINITY
        };
        let a = |c: [u8; 4]| {
            Color32::from_rgba_unmultiplied(c[0], c[1], c[2], (c[3] as f32 * alpha) as u8)
        };
        // karaoke: byte range of the highlighted words and their colour
        let hi = st.karaoke.as_ref().zip(lit).and_then(|(kar, lit)| {
            let w = cl_core::word_ranges(&txt);
            Some((w.get(lit.start)?.start..w.get(lit.end - 1)?.end, a(kar.color)))
        });
        let job = |col: Color32, hi: Option<(std::ops::Range<usize>, Color32)>| {
            let mut j = egui::text::LayoutJob::default();
            let fmt = |color| egui::TextFormat {
                font_id: font.clone(),
                color,
                line_height: Some(px * 1.25 * st.line_spacing.max(0.5)),
                italics: st.italic,
                ..Default::default()
            };
            match hi {
                Some((r, hc)) => {
                    j.append(&txt[..r.start], 0.0, fmt(col));
                    j.append(&txt[r.clone()], 0.0, fmt(hc));
                    j.append(&txt[r.end..], 0.0, fmt(col));
                }
                None => j.append(&txt, 0.0, fmt(col)),
            }
            j.wrap.max_width = wrap;
            j.halign = match st.align {
                x if x < 0 => egui::Align::LEFT,
                x if x > 0 => egui::Align::RIGHT,
                _ => egui::Align::Center,
            };
            j
        };
        let galley = p.layout_job(job(a(st.color), hi.clone()));
        let size = galley.size();
        let rot = |v: egui::Vec2| {
            let (s, c) = angle.sin_cos();
            vec2(v.x * c - v.y * s, v.x * s + v.y * c)
        };
        // anchor point for TextShape: top left of the galley (with centre alignment the
        // galley lies around x = 0)
        let x_off = match st.align {
            x if x < 0 => -size.x / 2.0,
            x if x > 0 => size.x / 2.0,
            _ => 0.0,
        };
        let anchor = center + rot(vec2(x_off, -size.y / 2.0));
        if let Some(bg) = st.background {
            let pad = (px * 0.25).max(3.0);
            let pts: Vec<egui::Pos2> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .iter()
                .map(|(sx, sy)| {
                    center
                        + rot(vec2(
                            sx * (size.x / 2.0 + pad),
                            sy * (size.y / 2.0 + pad * 0.6),
                        ))
                })
                .collect();
            p.add(egui::Shape::convex_polygon(pts, a(bg), egui::Stroke::NONE));
        }
        if st.shadow {
            let sg = p.layout_job(job(Color32::from_black_alpha((150.0 * alpha) as u8), None));
            let off = rot(vec2(1.0, 1.0) * (px / 22.0).clamp(1.0, 4.0));
            p.add(egui::epaint::TextShape::new(anchor + off, sg, Color32::BLACK).with_angle(angle));
        }
        if let Some((w, col)) = st.outline {
            let og = p.layout_job(job(a(col), None));
            let r = (w * k).clamp(0.6, 12.0);
            for i in 0..12 {
                let ang = i as f32 / 12.0 * std::f32::consts::TAU;
                p.add(
                    egui::epaint::TextShape::new(
                        anchor + rot(vec2(ang.cos(), ang.sin()) * r),
                        og.clone(),
                        Color32::BLACK,
                    )
                    .with_angle(angle),
                );
            }
        }
        p.add(egui::epaint::TextShape::new(anchor, galley, a(st.color)).with_angle(angle));
        if st.bold {
            // imitate bold when the font has no bold face of its own
            let g2 = p.layout_job(job(a(st.color), hi));
            p.add(
                egui::epaint::TextShape::new(anchor + rot(vec2(px * 0.03, 0.0)), g2, a(st.color))
                    .with_angle(angle),
            );
        }
        size
    }

    /// Control bar: time on the left, Play in the middle, chips on the right.
    /// Level meter (peak with decay and hold line), red from −1 dBFS
    fn level_meter(&mut self, ui: &mut egui::Ui) {
        let (l, r) = self
            .audio
            .as_ref()
            .map(|a| a.take_peaks())
            .unwrap_or((0.0, 0.0));
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        for (i, v) in [l, r].into_iter().enumerate() {
            let cur = &mut self.meter[i];
            cur.0 = v.max(cur.0 - dt * 1.2);
            if v >= cur.1 {
                cur.1 = v;
                cur.2 = 1.0;
            } else {
                cur.2 -= dt;
                if cur.2 <= 0.0 {
                    cur.1 = (cur.1 - dt * 0.6).max(cur.0);
                }
            }
        }
        let (rect, resp) = ui.allocate_exact_size(vec2(14.0, 22.0), egui::Sense::hover());
        let db = |v: f32| if v <= 1e-5 { -60.0 } else { 20.0 * v.log10() };
        let y_of = |v: f32| rect.bottom() - rect.height() * ((db(v) + 60.0) / 60.0).clamp(0.0, 1.0);
        let p = ui.painter();
        for i in 0..2 {
            let x0 = rect.left() + i as f32 * 7.0;
            let bar = Rect::from_min_max(pos2(x0, rect.top()), pos2(x0 + 5.0, rect.bottom()));
            p.rect_filled(bar, 1.0, theme::BG_HOVER());
            let (v, hold, _) = self.meter[i];
            let col = if db(v) > -1.0 {
                theme::DANGER
            } else if db(v) > -9.0 {
                theme::WARN
            } else {
                theme::SUCCESS
            };
            p.rect_filled(
                Rect::from_min_max(pos2(bar.left(), y_of(v)), bar.max),
                1.0,
                col,
            );
            if hold > 1e-4 {
                let y = y_of(hold);
                p.line_segment(
                    [pos2(bar.left(), y), pos2(bar.right(), y)],
                    egui::Stroke::new(
                        1.0_f32,
                        if db(hold) > -1.0 {
                            theme::DANGER
                        } else {
                            theme::TEXT_DIM()
                        },
                    ),
                );
            }
        }
        resp.on_hover_text(crate::tr(format!(
            "Pegel L {:.0} dB · R {:.0} dB",
            db(self.meter[0].1),
            db(self.meter[1].1)
        )));
        if self.playing {
            ui.ctx().request_repaint_after(self.play_repaint_delay());
        }
    }

    /// Set the playback volume; `save` only once the change is complete
    pub fn set_volume(&mut self, v: crate::audio::Volume, save: bool) {
        self.volume = v;
        crate::audio::set_volume(v);
        if save {
            self.save_last_used();
        }
    }

    pub fn step_volume(&mut self, up: bool) {
        self.set_volume(self.volume.step(up), true);
    }

    /// Speaker next to the level meter: click = mute/unmute, mouse wheel = 5 % steps, on
    /// hover a slider above it (own layer, takes no room from the bar).
    fn volume_button(&mut self, ui: &mut egui::Ui) {
        let v = self.volume;
        let glyph = if v.muted || v.percent <= 0.0 {
            icon::SPEAKER_SLASH
        } else if v.percent < 50.0 {
            icon::SPEAKER_LOW
        } else {
            icon::SPEAKER_HIGH
        };
        let tip = if v.muted {
            "Lautstärke der Wiedergabe: stumm".to_string()
        } else {
            format!("Lautstärke der Wiedergabe: {:.0} %", v.percent)
        };
        let r = theme::icon_button(ui, glyph, crate::keymap::with_key(&tip, &self.key_of("playback_mute")), true, false);
        if r.clicked() {
            self.set_volume(crate::audio::Volume { muted: !v.muted, ..v }, true);
        }
        let pop = r.id.with("volume");
        if r.hovered() {
            // shortcut: one step per wheel event (touchpads deliver many small ones), enough for mouse wheels
            let steps: Vec<bool> = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::MouseWheel { delta, .. } if delta.y != 0.0 => Some(delta.y > 0.0),
                        _ => None,
                    })
                    .collect()
            });
            for up in steps {
                self.step_volume(up);
            }
        }
        // open while the mouse is on the icon or the slider (or drags it)
        let last: Option<Rect> = ui.data(|d| d.get_temp(pop));
        let keep = last.is_some_and(|rect| {
            let near = rect.expand(8.0);
            ui.input(|i| {
                i.pointer.hover_pos().is_some_and(|p| near.contains(p))
                    || (i.pointer.primary_down() && i.pointer.press_origin().is_some_and(|p| near.contains(p)))
            })
        });
        if !(r.hovered() || keep) {
            ui.data_mut(|d| d.remove::<Rect>(pop));
            return;
        }
        let area = egui::Area::new(pop)
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(r.rect.left_top() - vec2(0.0, 2.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(150.0);
                    let mut p = self.volume.percent;
                    let e = crate::widgets::Slider::new(&mut p, 0.0..=100.0).suffix("%").default(100.0).show(ui);
                    if e.changed {
                        self.set_volume(crate::audio::Volume { percent: p, muted: false }, false);
                    }
                    if e.commit {
                        self.save_last_used();
                    }
                });
            });
        ui.data_mut(|d| d.insert_temp(pop, area.response.rect));
    }

    fn player_controls(&mut self, ui: &mut egui::Ui, h: f32) {
        let fps = self.project.settings.fps;
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), egui::Sense::hover());
        let left = Rect::from_min_max(bar.min, pos2(bar.center().x - 24.0, bar.max.y));
        let right = Rect::from_min_max(pos2(bar.center().x + 24.0, bar.min.y), bar.max);
        ui.allocate_new_ui(
            egui::UiBuilder::new()
                .max_rect(left)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.add_space(4.0);
                self.level_meter(ui);
                self.volume_button(ui);
                ui.label(
                    RichText::new(crate::tr(self.playhead.timecode(fps)))
                        .color(theme::TEXT())
                        .monospace(),
                );
                let total = format!("/ {}", self.project.duration().timecode(fps));
                let need = ui
                    .painter()
                    .layout_no_wrap(
                        total.clone(),
                        egui::TextStyle::Monospace.resolve(ui.style()),
                        theme::TEXT_DIM(),
                    )
                    .size()
                    .x
                    + 8.0;
                if ui.available_width() > need {
                    ui.label(
                        RichText::new(crate::tr(total))
                            .color(theme::TEXT_DIM())
                            .monospace(),
                    );
                }
            },
        );
        let pr = ui.put(
            Rect::from_center_size(bar.center(), vec2(36.0, 28.0)),
            egui::Button::new("").frame(false),
        );
        // icon changes smoothly: Play fades out, Pause in (and vice versa)
        let k = theme::fade(ui.ctx(), pr.id.with("glyph"), self.playing, theme::MOTION_FAST);
        for (glyph, a) in [(icon::PLAY, 1.0 - k), (icon::PAUSE, k)] {
            if a > 0.0 {
                // icon glyph: large play button
                ui.painter().text(pr.rect.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(19.0), theme::TEXT().gamma_multiply(a));
            }
        }
        if pr
            .on_hover_text(crate::tr(format!(
                "{}  (Leertaste)",
                if self.playing {
                    self.t("pause")
                } else {
                    self.t("play")
                }
            )))
            .clicked()
        {
            self.toggle_play();
        }
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
            let compact = ui.available_width() < 200.0;
            let (g, tip) = if self.player_max { (icon::CORNERS_IN, "Normale Ansicht (F)") } else { (icon::CORNERS_OUT, "Großer Player (F)") };
            if theme::icon_button(ui, g, tip, true, self.player_max).clicked() {
                self.player_max = !self.player_max;
            }
            if theme::icon_button(ui, icon::MONITOR_PLAY, "Vorschau in eigenem Fenster (zweiter Bildschirm, Doppelklick oder F11 = Vollbild)", true, self.ext_preview).clicked() {
                self.ext_preview = !self.ext_preview;
            }
            self.player_zoom_chip(ui, compact);
            // chip short (½, ¼), tooltip with the same words as everywhere: Voll, Halb, Viertel
            let (q, word) = match self.settings.preview_div {
                1 => ("Voll", "Voll"),
                2 => ("½", "Halb"),
                _ => ("¼", "Viertel"),
            };
            let qlabel = if compact { icon::MONITOR.to_string() } else { format!("{} {}", icon::MONITOR, crate::tr(q)) };
            if theme::chip(ui, &qlabel).on_hover_text(crate::tr(format!("Vorschauqualität: {}", crate::tr(word)))).clicked() {
                self.settings.preview_div = match self.settings.preview_div {
                    1 => 2,
                    2 => 4,
                    _ => 1,
                };
                self.invalidate_preview();
            }
            // automatic quality during playback: small indicator next to the chip, fades in and out
            let auto_id = egui::Id::new("player-auto-quality");
            let k = theme::fade(ui.ctx(), auto_id, self.core.playing() && self.core.quality.level > 0, theme::MOTION);
            if self.core.quality.level > 0 {
                let (div, half) = self.play_quality();
                let fps = self.project.settings.fps.as_f64();
                let txt = format!(
                    "{}{}",
                    match div {
                        2 => "½",
                        4 => "¼",
                        _ => "",
                    },
                    if half { format!(" {:.0} fps", fps / 2.0) } else { String::new() }
                );
                ui.data_mut(|d| d.insert_temp(auto_id, txt));
            }
            if k > 0.01 {
                let txt = ui.data(|d| d.get_temp::<String>(auto_id)).unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(txt.trim()).size(theme::FS_HINT).color(theme::ACCENT().gamma_multiply(k))))
                    .on_hover_text(crate::tr("Vorschauqualität beim Abspielen automatisch verringert"));
            }
            let aspect = AspectPreset::PRESETS.iter().find(|(_, p)| *p == self.project.settings.aspect).map(|(n, _)| *n).unwrap_or("Eigenes");
            let alabel = if compact { icon::FRAME_CORNERS.to_string() } else { format!("{} {aspect}", icon::FRAME_CORNERS) };
            let resp = theme::chip(ui, &alabel).on_hover_text(crate::tr(format!("{}: {aspect}", self.t("aspect"))));
            let popup = ui.make_persistent_id("aspect-popup");
            if resp.clicked() {
                ui.memory_mut(|m| m.toggle_popup(popup));
            }
            egui::popup_above_or_below_widget(ui, popup, &resp, egui::AboveOrBelow::Above, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                ui.set_min_width(120.0);
                for (n, p) in AspectPreset::PRESETS {
                    if ui.selectable_label(self.project.settings.aspect == p, crate::tr(n)).clicked() {
                        self.set_aspect(p);
                    }
                }
            });
        });
    }

    /// Right click in the player: the same commands as in the player bar and the quick actions.
    fn player_menu(&mut self, ui: &mut egui::Ui) {
        use crate::timeline::menu_item;
        if menu_item(ui, icon::IMAGE, "Standbild speichern …", "") {
            let ctx = ui.ctx().clone();
            self.tool_thumbnail(&ctx);
        }
        ui.separator();
        if menu_item(ui, icon::MAGNIFYING_GLASS, "Ganzes Bild", "Alt+Umschalt+Z") {
            self.player_zoom = 1.0;
            self.player_pan = egui::Vec2::ZERO;
        }
        let ext = if self.ext_preview { icon::CHECK } else { icon::MONITOR_PLAY };
        if menu_item(ui, ext, "Vorschau in eigenem Fenster", "") {
            self.ext_preview = !self.ext_preview;
        }
        let max = if self.player_max { icon::CHECK } else { icon::CORNERS_OUT };
        if menu_item(ui, max, "Großer Player", &self.key_of("max")) {
            self.player_max = !self.player_max;
        }
        let sa = if self.safe_area { icon::CHECK } else { icon::DEVICE_MOBILE };
        if menu_item(ui, sa, "Safe-Area", "") {
            self.safe_area = !self.safe_area;
            self.save_last_used();
        }
    }

    pub fn set_aspect(&mut self, p: AspectPreset) {
        let mut s = self.project.settings.clone();
        if p == AspectPreset::Original {
            if let Some(m) = self.project.media.iter().find(|m| m.width > 0) {
                s.width = m.width;
                s.height = m.height;
            }
            s.aspect = p;
        } else {
            s.apply_aspect(p);
        }
        if s.platform
            .as_deref()
            .and_then(crate::export_ui::platform)
            .is_some_and(|pl| !pl.fits(&s))
        {
            s.platform = None;
        }
        if s != self.project.settings {
            // format changes and there are frames: first ask how they should be fitted again
            if (s.width, s.height) != (self.project.settings.width, self.project.settings.height) && self.has_visual_clips() {
                self.aspect_fit = Some(s);
            } else {
                self.apply(EditOp::SetSettings { settings: s });
            }
        }
    }
}
