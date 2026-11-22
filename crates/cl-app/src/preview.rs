//! Preview: background decoder (single frames per clip with effect chain, thumbnails,
//! waveforms, effect preview images, project title images) and start of playback streams.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use cl_core::{ClipId, MediaId, Rational, Time};
use cl_media::{FrameStream, KillSwitch, RgbaFrame};
use eframe::egui;

/// Waveform resolution (values per second)
pub const WAVE_PER_SEC: u32 = 50;

pub enum Job {
    /// Frame of a clip (with optional filter graph `[0:v]` → `[out]`)
    Frame {
        clip: ClipId,
        key: u64,
        path: PathBuf,
        at: Time,
        w: u32,
        h: u32,
        graph: Option<String>,
        /// Source frame rate if the frame should come from a reverse window (see
        /// `player::reverse_window`); otherwise one FFmpeg per frame
        window: Option<Rational>,
    },
    Thumb {
        media: MediaId,
        path: PathBuf,
        at: Time,
        /// Display size of the medium (`MediaRef`, rotation applied); saves a probe per image
        size: (u32, u32),
    },
    Wave {
        media: MediaId,
        path: PathBuf,
        /// Audio track of the file (0 = first)
        stream: u32,
    },
    /// Small image of a file at `at` (library, hover preview); result as `FxThumb`
    FileThumb {
        key: String,
        path: PathBuf,
        at: Time,
    },
    /// `n` frames evenly across a video (hover preview in the media tab), as one strip
    MediaStrip {
        key: String,
        path: PathBuf,
        dur: Time,
        n: u32,
        /// Display size of the medium (`MediaRef`, rotation applied)
        size: (u32, u32),
    },
    /// Prepare a sound from the library: generate if needed (`sfx`), check, waveform
    SoundMeta {
        key: String,
        path: PathBuf,
        sfx: Option<&'static str>,
    },
    /// Preview image from a graph without inputs (effect/filter/transition tiles)
    /// Tile for effects/filters/transitions. `frames` > 1: film strip (frames stacked)
    FxThumb {
        key: String,
        graph: String,
        /// Shader transition: blend frames with cl-gpu; without GPU or on errors `graph` applies
        shader: Option<ShaderTile>,
        w: u32,
        h: u32,
        frames: u32,
        cache: PathBuf,
    },
    /// Title image of a project for the start page
    ProjectThumb {
        out: PathBuf,
        path: PathBuf,
        at: Time,
    },
}

/// Tile of a shader transition: `pair_graph` delivers A and B side by side (width 2·w).
pub struct ShaderTile {
    pub file: PathBuf,
    pub params: Vec<f32>,
    pub pair_graph: String,
}

/// Bars of the small sound waveform (rows and tiles)
pub const SOUND_BARS: usize = 24;

/// `n` bars over the whole length (peak per section), normalized to the loudest, 0…1.
pub fn sound_bars(peaks: &[f32], n: usize) -> Vec<f32> {
    let max = peaks.iter().copied().fold(1e-4, f32::max);
    (0..n)
        .map(|i| {
            let a = (i * peaks.len() / n).min(peaks.len());
            let b = ((i + 1) * peaks.len() / n).max(a + 1).min(peaks.len());
            (peaks[a..b].iter().copied().fold(0.0, f32::max) / max).clamp(0.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod sound_bar_tests {
    use super::sound_bars;

    #[test]
    fn bars_are_fixed_count_and_normalized() {
        let peaks: Vec<f32> = (0..500).map(|i| (i % 50) as f32 / 100.0).collect();
        let b = sound_bars(&peaks, 24);
        assert_eq!(b.len(), 24);
        assert!(b.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(b.iter().any(|v| *v == 1.0), "loudest bar full");
        // silence, empty and very short: fixed count, nothing outside 0…1
        assert!(sound_bars(&[0.0; 100], 24).iter().all(|v| *v == 0.0));
        assert_eq!(sound_bars(&[], 24), vec![0.0; 24]);
        let short = sound_bars(&[0.2, 0.8, 2.0], 24);
        assert_eq!(short.len(), 24);
        assert!(short.iter().all(|v| (0.0..=1.0).contains(v)) && short.contains(&1.0));
    }
}

/// Film strip of a shader transition: `frames` frames at progress k/frames (like the
/// `xfade` strip), A and B from the frame pair `pair` (2·w × h).
pub fn shader_strip(s: &ShaderTile, pair: &[u8], w: u32, h: u32, frames: u32) -> Result<Vec<u8>, String> {
    let src = std::fs::read_to_string(&s.file).map_err(|e| e.to_string())?;
    let row = (w * 4) as usize;
    if pair.len() < row * 2 * h as usize {
        return Err("Bildpaar unvollständig".into());
    }
    let (mut a, mut b) = (Vec::with_capacity(row * h as usize), Vec::with_capacity(row * h as usize));
    for line in pair.chunks_exact(row * 2).take(h as usize) {
        a.extend_from_slice(&line[..row]);
        b.extend_from_slice(&line[row..]);
    }
    let prm = shader_params(&s.params);
    let fa = cl_gpu::Frame { rgba: &a, width: w, height: h };
    let fb = cl_gpu::Frame { rgba: &b, width: w, height: h };
    let mut out = Vec::with_capacity(row * (h * frames) as usize);
    for k in 0..frames {
        out.extend(cl_gpu::render(&src, &prm, fa, fb, (w, h), k as f32 / frames as f32)?);
    }
    Ok(out)
}

/// Parameters from the catalog into the fixed array of cl-gpu (rest 0).
pub fn shader_params(v: &[f32]) -> [f32; cl_gpu::PARAMS] {
    let mut prm = [0.0; cl_gpu::PARAMS];
    for (p, x) in prm.iter_mut().zip(v) {
        *p = *x;
    }
    prm
}

pub enum Done {
    Frame {
        clip: ClipId,
        key: u64,
        frame: RgbaFrame,
        /// `false`: a held neighbour of the requested frame (reverse window during playback),
        /// not the frame of `key` itself
        exact: bool,
    },
    /// Player frame could not be decoded (the job is finished with that)
    FrameFailed {
        clip: ClipId,
        key: u64,
        error: String,
    },
    Thumb {
        media: MediaId,
        frame: RgbaFrame,
    },
    Wave {
        media: MediaId,
        stream: u32,
        peaks: Vec<f32>,
    },
    SoundMeta {
        key: String,
        media: cl_core::MediaRef,
        bars: Vec<f32>,
    },
    FxThumb {
        key: String,
        frame: RgbaFrame,
    },
    ProjectThumb {
        out: PathBuf,
    },
    /// Strip job skipped (tile no longer visible) → can be requested again
    Skipped(String),
    Error(String),
}

/// Last computed player frames (key contains clip, source time, size and effects),
/// so repeated scrubbing and undo need no FFmpeg call.
struct FrameCache {
    map: std::collections::HashMap<u64, RgbaFrame>,
    order: std::collections::VecDeque<u64>,
    bytes: usize,
    budget: usize,
    /// Project generation of the frames: everything is freed after a project change
    gen: u64,
}

impl FrameCache {
    fn new(budget: usize) -> Self {
        Self {
            map: Default::default(),
            order: Default::default(),
            bytes: 0,
            budget,
            gen: 0,
        }
    }
    /// Free the frames of a left project (otherwise up to `budget` bytes stayed in use)
    fn set_generation(&mut self, gen: u64) {
        if gen != self.gen {
            *self = Self { gen, ..Self::new(self.budget) };
        }
    }
    fn get(&self, key: u64) -> Option<&RgbaFrame> {
        self.map.get(&key)
    }
    fn put(&mut self, key: u64, f: RgbaFrame) {
        if self.map.contains_key(&key) {
            return;
        }
        self.bytes += f.data.len();
        self.map.insert(key, f);
        self.order.push_back(key);
        while self.bytes > self.budget {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(o) = self.map.remove(&old) {
                self.bytes -= o.data.len();
            }
        }
    }
}

/// Three worker threads: one dispatches the player frames (each job its own process,
/// stale ones are aborted), one for everything in the background (thumbnails, waveforms), one
/// for tiles. Background and tiles wait while player frames are running.
pub struct Decoder {
    tx: Sender<Msg>,
    tx_bg: Sender<(u64, Job)>,
    /// Tiles (effects, filters, transitions): own thread, up to four FFmpeg in parallel,
    /// so preloading does not hold up thumbnails and waveforms of the media
    tx_fx: Sender<(u64, Job)>,
    pub rx: Receiver<Done>,
    /// Strip keys currently visible/hovered (the media tab sets them each frame)
    pub strips_wanted: Arc<Mutex<HashSet<String>>>,
    /// Project generation: the worker discards jobs of a left project
    pub generation: Arc<AtomicU64>,
    /// Dispatcher cache: frames (count, bytes), animations (count, bytes);
    /// read by the endurance measurement test
    #[cfg_attr(not(test), allow(dead_code))]
    pub held: Arc<[AtomicUsize; 4]>,
}

/// Message to the player frame dispatcher: new job or finished result.
enum Msg {
    Job(Job),
    Done {
        clip: ClipId,
        key: u64,
        res: cl_media::Result<(RgbaFrame, cl_media::DecodeInfo)>,
        ms: f64,
        proxy: bool,
    },
    /// Reverse window decoded (`seq`: job number, frames from `start`)
    Win {
        clip: ClipId,
        seq: u64,
        start: i64,
        res: cl_media::Result<Vec<(u32, RgbaFrame)>>,
        ms: f64,
    },
    /// Animated image fully decoded (`None`: too large or unreadable → single frames as before)
    Anim {
        key: (PathBuf, u32, u32),
        frames: Option<Arc<Vec<RgbaFrame>>>,
    },
}

/// Animated images (GIF/APNG) of the preview in memory: all frames per file and size, so
/// an animated element does not need one FFmpeg per frame during playback. Limited: at most
/// [`ANIM_ONE`] per animation and [`ANIM_ALL`] in total (then it starts over).
#[derive(Default)]
struct Anims {
    map: HashMap<(PathBuf, u32, u32), Option<Arc<Vec<RgbaFrame>>>>,
    bytes: usize,
}

const ANIM_ONE: usize = 64 << 20;
const ANIM_ALL: usize = 192 << 20;

impl Anims {
    /// Frame for `at` (seek time from `Animation::seek_time`) if the animation is in memory.
    /// `Err(true)`: never requested (decode now), `Err(false)`: already running or not possible.
    fn get(&mut self, path: &std::path::Path, w: u32, h: u32, at: Time) -> Result<RgbaFrame, bool> {
        // only look at image files (`animation` reads the whole file)
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !["gif", "png", "apng"].contains(&ext.as_str()) {
            return Err(false);
        }
        let Some(a) = cl_media::animation(path) else { return Err(false) };
        let key = (path.to_path_buf(), w, h);
        match self.map.get(&key) {
            Some(Some(frames)) => frames.get(cl_media::animation_frame(&a, at).min(frames.len().saturating_sub(1))).cloned().ok_or(false),
            Some(None) => Err(false),
            None => {
                self.map.insert(key, None);
                Err(true)
            }
        }
    }
    fn put(&mut self, key: (PathBuf, u32, u32), frames: Option<Arc<Vec<RgbaFrame>>>) {
        let n: usize = frames.as_ref().map_or(0, |f| f.iter().map(|x| x.data.len()).sum());
        if self.bytes + n > ANIM_ALL {
            self.map.retain(|_, v| v.is_none());
            self.bytes = 0;
        }
        self.bytes += n;
        self.map.insert(key, frames);
    }
}

/// Single-frame job of a clip (a `Job::Frame` without window)
struct Still {
    key: u64,
    path: PathBuf,
    at: Time,
    w: u32,
    h: u32,
    graph: Option<String>,
}

/// A running decode is killed for a new job only when the new target is this far (source seconds)
/// from it and from the previous job: a jump. While dragging, the targets move in small steps – the
/// running frame then still shows roughly the right place, and killing it starved every job.
pub(crate) const KILL_DIST: f64 = 1.0;

/// What a new single-frame job does next to the clip's running one
#[derive(Debug, PartialEq)]
pub(crate) enum Next {
    /// nothing runs: start
    Start,
    /// the same job runs already
    Same,
    /// let the running one finish, this one waits (replacing an older waiting one)
    Wait,
    /// running one is useless (other file, jump): kill it, start this one
    Replace,
}

/// `run`: (key, file, source time) of the running job; `prev`: source time of the newest job so far
pub(crate) fn next_still(run: Option<(u64, &std::path::Path, Time)>, prev: Time, key: u64, path: &std::path::Path, at: Time) -> Next {
    let Some((k, p, a)) = run else { return Next::Start };
    let far = |x: Time| (x.as_secs_f64() - at.as_secs_f64()).abs() > KILL_DIST;
    if k == key {
        Next::Same
    } else if p != path || (far(a) && far(prev)) {
        Next::Replace
    } else {
        Next::Wait
    }
}

/// Running player frames: at most one decode per clip, and while it runs at most the newest
/// further job waits ("newest wins"; older waiting ones are dropped without a process).
#[derive(Default)]
struct InFlight(HashMap<ClipId, ((u64, PathBuf, Time, Arc<KillSwitch>), Option<Still>)>);

impl InFlight {
    /// New job: `Some` = start it now (with its switch, and the switch of a killed one)
    fn offer(&mut self, clip: ClipId, s: Still) -> Option<(Still, Arc<KillSwitch>, Option<Arc<KillSwitch>>)> {
        let slot = self.0.get(&clip);
        let run = slot.as_ref().map(|((k, p, a, _), _)| (*k, p.as_path(), *a));
        let prev = slot.as_ref().map_or(s.at, |((.., a, _), w)| w.as_ref().map_or(*a, |w| w.at));
        let old = match next_still(run, prev, s.key, &s.path, s.at) {
            Next::Same => {
                if let Some(slot) = self.0.get_mut(&clip) {
                    slot.1 = None;
                }
                return None;
            }
            Next::Wait => {
                if let Some(slot) = self.0.get_mut(&clip) {
                    slot.1 = Some(s);
                }
                return None;
            }
            Next::Start => None,
            Next::Replace => self.0.remove(&clip).map(|((.., k), _)| k),
        };
        let ks = Arc::new(KillSwitch::default());
        self.0.insert(clip, ((s.key, s.path.clone(), s.at, ks.clone()), None));
        Some((s, ks, old))
    }
    /// Frame came from memory: the clip's running job is stale, a waiting one too.
    fn cancel(&mut self, clip: ClipId) -> Option<Arc<KillSwitch>> {
        self.0.remove(&clip).map(|((.., k), _)| k)
    }
    /// Result there: `None` = not the clip's running job (stale), otherwise the job waiting next.
    fn finish(&mut self, clip: ClipId, key: u64) -> Option<Option<Still>> {
        if self.0.get(&clip).is_none_or(|((k, ..), _)| *k != key) {
            return None;
        }
        self.0.remove(&clip).map(|(_, w)| w)
    }
    fn len(&self) -> usize {
        self.0.len()
    }
}

/// Decoder of single player frames (`cl_media::decode_frame_opts`; tests count with a fake)
type StillFn = Arc<dyn Fn(&std::path::Path, Time, u32, u32, Option<&str>, &KillSwitch) -> cl_media::Result<(RgbaFrame, cl_media::DecodeInfo)> + Send + Sync>;

/// May a background job start? Only when no player frame is running – waited at most 1.5 s
/// so thumbnails do not starve during continuous scrubbing.
fn bg_may_start(player_busy: usize, waited: std::time::Duration) -> bool {
    player_busy == 0 || waited >= std::time::Duration::from_millis(1500)
}

fn yield_to_player(busy: &AtomicUsize) {
    let t = std::time::Instant::now();
    while !bg_may_start(busy.load(Ordering::Relaxed), t.elapsed()) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Reverse window: all held frames of a clip together (running and preloaded
/// window) at most this large – rather a shorter window than more memory.
const WINDOW_BYTES: usize = 120 << 20;

/// (frames per window, max held frames) for frame size `w`×`h` at source rate `fps`:
/// one window at most 1 s and half the limit (room for the next one ahead, which
/// starts in time while that many frames are still left).
fn window_plan(w: u32, h: u32, fps: f64) -> (i64, i64) {
    let cap = (WINDOW_BYTES / (w as usize * h as usize * 4).max(1)).max(2) as i64;
    ((cap / 2).clamp(1, (fps.round() as i64).max(1)), cap)
}

/// Which frames to decode next: (first frame, count); only the first `n` of them
/// are kept (and the missing `idx`). If frame `idx` is missing, the range reaches up to `idx`; otherwise
/// ahead up to right before the held frames (from `lowest`; in reverse they come
/// next), as soon as `n` fits under the limit beside the `held` ones. `lead`: the clock is this many
/// frames further when the window is done – the kept ones lie there.
fn next_window(idx: i64, has_idx: bool, lowest: i64, held: i64, lead: i64, (n, cap): (i64, i64)) -> Option<(i64, i64)> {
    let end = if has_idx { lowest - 1 } else { idx };
    if end < 0 || (has_idx && held + n > cap) {
        return None;
    }
    let first = (end.min(idx - lead.max(0)) - n + 1).max(0);
    Some((first, end - first + 1))
}

/// Answer from the held window frames: the highest one in `range` (which ends at `idx` or
/// before); any other than `idx` is only a neighbour for playback and is marked as such.
fn window_answer(frames: &std::collections::BTreeMap<i64, RgbaFrame>, range: std::ops::RangeInclusive<i64>, clip: ClipId, key: u64, idx: i64) -> Option<Done> {
    frames.range(range).next_back().map(|(&i, f)| Done::Frame { clip, key, frame: f.clone(), exact: i == idx })
}

/// Reverse window of a clip in the dispatcher
struct Win {
    /// File, size, source rate: if one changes, nothing is valid any more
    spec: (PathBuf, u32, u32, Rational),
    frames: std::collections::BTreeMap<i64, RgbaFrame>,
    /// running decode: (number, first frame, keep this many from there, plus the missing
    /// frame, abort)
    busy: Option<(u64, i64, i64, Option<i64>, Arc<KillSwitch>)>,
    /// Duration of the last decode (s) and pace of requests (source frames per s, smoothed),
    /// last request: from these the lead `next_window(lead)`
    took: f64,
    speed: f64,
    prev: Option<(i64, std::time::Instant)>,
    /// waiting job: (frame number, key, source time, since)
    want: Option<(i64, u64, Time, std::time::Instant)>,
    last: std::time::Instant,
}

impl Win {
    fn kill(&mut self) {
        if let Some((.., k)) = self.busy.take() {
            k.kill();
        }
    }
    fn bytes(&self) -> usize {
        self.frames.values().map(|f| f.data.len()).sum()
    }
}

/// Dispatcher of the player frames (runs in the `cl-decoder` thread).
#[allow(clippy::too_many_arguments)]
fn frame_dispatcher(
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    done_tx: Sender<Done>,
    ctx: egui::Context,
    gen: Arc<AtomicU64>,
    busy: Arc<AtomicUsize>,
    held: Arc<[AtomicUsize; 4]>,
    decode: StillFn,
) {
    let proxies = crate::proxy::dir();
    let spawn = |s: Still, ks: Arc<KillSwitch>, clip: ClipId| {
        #[cfg(test)]
        STILL_JOBS[0].fetch_add(1, Ordering::Relaxed);
        let (tx, decode) = (tx.clone(), decode.clone());
        let proxy = s.path.starts_with(&proxies);
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let res = decode(&s.path, s.at, s.w, s.h, s.graph.as_deref(), &ks);
            let ms = t.elapsed().as_secs_f64() * 1e3;
            let _ = tx.send(Msg::Done { clip, key: s.key, res, ms, proxy });
        });
    };
    let mut cache = FrameCache::new(384 << 20);
    let mut anims = Anims::default();
    let mut inflight = InFlight::default();
    let mut windows: HashMap<ClipId, Win> = HashMap::new();
    let mut win_seq = 0u64;
    // file and size whose window did not deliver the frame (for the session: single frames)
    let mut win_failed: HashSet<(PathBuf, u32, u32)> = HashSet::new();
    loop {
        // check whether the project changed even without new jobs
        if cache.gen != gen.load(Ordering::Relaxed) {
            anims = Anims::default();
            windows.values_mut().for_each(Win::kill);
            windows.clear();
        }
        // window of a clip no longer played in reverse (pause, stand): free it;
        // a running one (the job waits for it, slow sources need seconds) stays
        windows.retain(|_, w| {
            let keep = w.busy.is_some() || w.last.elapsed() < std::time::Duration::from_secs(2);
            if !keep {
                w.kill();
            }
            keep
        });
        cache.set_generation(gen.load(Ordering::Relaxed));
        let msg = match rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(m) => m,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let out = match msg {
            Msg::Job(Job::Frame { clip, key, path, at, w, h, graph, window }) => {
                crate::proxy::note_activity();
                // reverse during playback (original, without effect graph): frames window by window from
                // one FFmpeg instead of one per frame; everything else as before
                let spec = window
                    .filter(|_| graph.is_none() && !path.starts_with(&proxies) && !win_failed.contains(&(path.clone(), w, h)))
                    .map(|fps| (path.clone(), w, h, fps));
                let win = match spec {
                    Some(spec) => {
                        let fresh = |spec: (PathBuf, u32, u32, Rational)| Win { speed: spec.3.as_f64(), spec, frames: Default::default(), busy: None, want: None, took: 0.0, prev: None, last: std::time::Instant::now() };
                        let wn = windows.entry(clip).or_insert_with(|| fresh(spec.clone()));
                        if wn.spec != spec {
                            wn.kill();
                            *wn = fresh(spec);
                        }
                        wn.last = std::time::Instant::now();
                        Some(wn)
                    }
                    None => {
                        if let Some(mut o) = windows.remove(&clip) {
                            o.kill();
                        }
                        None
                    }
                };
                if let Some(wn) = win {
                    let fps = wn.spec.3.as_f64();
                    let idx = (at.as_secs_f64() * fps).round() as i64;
                    let now = std::time::Instant::now();
                    if let Some((pi, pt)) = wn.prev {
                        let dt = now.duration_since(pt).as_secs_f64();
                        if dt > 1e-3 && pi > idx {
                            wn.speed = 0.5 * wn.speed + 0.5 * ((pi - idx) as f64 / dt).min(8.0 * fps);
                        }
                    }
                    wn.prev = Some((idx, now));
                    let plan = window_plan(w, h, fps);
                    // free frames already shown (reverse: later ones); after a jump everything
                    wn.frames.retain(|i, _| *i <= idx);
                    // the frame – or, if the window came earlier than the clock, the next held one
                    // after it: it then stands briefly, the order stays
                    let near = window_answer(&wn.frames, idx - plan.0..=idx, clip, key, idx);
                    let has = near.is_some();
                    if !has {
                        wn.frames.clear();
                    }
                    let out = if has {
                        if let Some(k) = inflight.cancel(clip) {
                            k.kill();
                            perf_discarded();
                        }
                        wn.want = None;
                        near
                    } else {
                        wn.want = Some((idx, key, at, now));
                        None
                    };
                    // if one is running that keeps frames up to here or further ahead, the job
                    // waits for it; if it lies entirely behind the clock, abort and restart
                    if !has && !wn.busy.as_ref().is_some_and(|(_, s, _, e, _)| *s <= idx || *e == Some(idx)) {
                        wn.kill();
                    }
                    if wn.busy.is_none() {
                        let lowest = wn.frames.keys().next().copied().unwrap_or(idx);
                        // lead with reserve: rather there early (frame stands briefly) than too late (restart)
                        let lead = (1.3 * wn.took * wn.speed).min(4.0 * fps) as i64;
                        if let Some((start, n)) = next_window(idx, has, lowest, wn.frames.len() as i64, lead, plan) {
                            win_seq += 1;
                            let ks = Arc::new(KillSwitch::default());
                            let (keep, end) = (plan.0, (!has).then_some(idx));
                            wn.busy = Some((win_seq, start, keep, end, ks.clone()));
                            let (tx, path, seq) = (tx.clone(), path.clone(), win_seq);
                            let from = Time::from_secs_f64(start as f64 / fps);
                            std::thread::spawn(move || {
                                let t = std::time::Instant::now();
                                let keep = |k: u32| (k as i64) < keep || end == Some(start + k as i64);
                                let res = cl_media::decode_frames(&path, from, n as u32, w, h, &keep, Some(&ks));
                                let ms = t.elapsed().as_secs_f64() * 1e3;
                                let _ = tx.send(Msg::Win { clip, seq, start, res, ms });
                            });
                        }
                    }
                    out
                } else {
                    // animated image without effects: from memory (fully decoded in the background the first time)
                    let anim = if graph.is_none() {
                        match anims.get(&path, w, h, at) {
                            Ok(f) => Some(f),
                            Err(true) => {
                                let (tx, path) = (tx.clone(), path.clone());
                                std::thread::spawn(move || {
                                    let frames = cl_media::decode_animation(&path, w, h, ANIM_ONE).map(Arc::new);
                                    let _ = tx.send(Msg::Anim { key: (path, w, h), frames });
                                });
                                None
                            }
                            Err(false) => None,
                        }
                    } else {
                        None
                    };
                    if let Some(frame) = anim.or_else(|| cache.get(key).cloned()) {
                        if let Some(k) = inflight.cancel(clip) {
                            k.kill();
                            perf_discarded();
                        }
                        Some(Done::Frame { clip, key, frame, exact: true })
                    } else {
                        if let Some((s, ks, old)) = inflight.offer(clip, Still { key, path, at, w, h, graph }) {
                            if let Some(o) = old {
                                o.kill();
                                perf_discarded();
                            }
                            spawn(s, ks, clip);
                        }
                        None
                    }
                }
            }
            Msg::Job(j) => run(j),
            Msg::Win { clip, seq, start, res, ms } => match windows.get_mut(&clip) {
                Some(wn) if wn.busy.as_ref().is_some_and(|b| b.0 == seq) => {
                    wn.busy = None;
                    wn.took = ms / 1e3;
                    if crate::app::perf() {
                        let n = res.as_ref().map_or(0, Vec::len);
                        crate::applog::perf_line!("perf: rückwärts-fenster {n} Bilder {ms:.0} ms");
                    }
                    for (k, f) in res.into_iter().flatten() {
                        wn.frames.insert(start + k as i64, f);
                    }
                    let (path, w, h, fps) = wn.spec.clone();
                    match wn.want.take() {
                        // the clock kept running while the job waited: the frame where it now roughly
                        // stands, or the next one after (it then stands briefly)
                        Some((idx, key, at, since)) => match window_answer(&wn.frames, i64::MIN..=idx.min(idx - (since.elapsed().as_secs_f64() * wn.speed) as i64 + window_plan(w, h, fps.as_f64()).0), clip, key, idx) {
                            Some(d) => Some(d),
                            // nothing read (file with varying frame rate, errors): single frames as before
                            None if wn.frames.is_empty() => {
                                win_failed.insert((path.clone(), w, h));
                                let _ = tx.send(Msg::Job(Job::Frame { clip, key, path, at, w, h, graph: None, window: None }));
                                None
                            }
                            // everything already behind the clock: request again (new window)
                            None => {
                                let _ = tx.send(Msg::Job(Job::Frame { clip, key, path, at, w, h, graph: None, window: Some(fps) }));
                                None
                            }
                        },
                        None => None,
                    }
                }
                _ => None,
            },
            Msg::Anim { key, frames } => {
                anims.put(key, frames);
                None
            }
            Msg::Done { clip, key, res, ms, proxy } => {
                crate::proxy::note_activity();
                let next = inflight.finish(clip, key);
                let current = next.is_some();
                let waiting = next.flatten();
                // a newer job waited for this one: the frame bridges as a neighbour until its exact frame comes
                let exact = waiting.is_none();
                if let Some((s, ks, _)) = waiting.and_then(|s| inflight.offer(clip, s)) {
                    spawn(s, ks, clip);
                }
                match res {
                    Ok((frame, info)) => {
                        if crate::app::perf() {
                            crate::applog::perf_line!(
                                "perf: einzelbild {ms:.0} ms {} {}{}{}",
                                if proxy { "Proxy" } else { "Original" },
                                if info.hw { "HW" } else { "SW" },
                                info.cpu.map(|c| format!(" cpu {} ms", c.as_millis())).unwrap_or_default(),
                                if current { "" } else { " (veraltet)" }
                            );
                        }
                        cache.put(key, frame.clone());
                        current.then_some(Done::Frame { clip, key, frame, exact })
                    }
                    Err(cl_media::MediaError::Cancelled) => None,
                    Err(e) => (current && exact).then(|| Done::FrameFailed { clip, key, error: e.to_string() }),
                }
            }
        };
        busy.store(inflight.len() + windows.values().filter(|w| w.busy.is_some()).count(), Ordering::Relaxed);
        let win_bytes: usize = windows.values().map(Win::bytes).sum();
        for (a, v) in held.iter().zip([cache.map.len(), cache.bytes + win_bytes, anims.map.len(), anims.bytes]) {
            a.store(v, Ordering::Relaxed);
        }
        if let Some(d) = out {
            if done_tx.send(d).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    }
}

/// Measurement only (`scrub_in_app`): single-frame decodes started and killed by the dispatcher
#[cfg(test)]
pub(crate) static STILL_JOBS: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];

fn perf_discarded() {
    #[cfg(test)]
    STILL_JOBS[1].fetch_add(1, Ordering::Relaxed);
    if crate::app::perf() {
        crate::applog::perf_line!("perf: einzelbild verworfen (veraltet)");
    }
}

/// Job still needed? `None` = discard, `Some(Some(d))` = report instead of running.
fn check(job: &Job, g: u64, gen: &AtomicU64, wanted: &Mutex<HashSet<String>>) -> Option<Option<Done>> {
    let project = matches!(job, Job::Thumb { .. } | Job::Wave { .. } | Job::MediaStrip { .. } | Job::FileThumb { .. });
    if project && g != gen.load(Ordering::Relaxed) {
        return None;
    }
    if let Job::MediaStrip { key, .. } = job {
        if !wanted.lock().unwrap_or_else(|e| e.into_inner()).contains(key) {
            return Some(Some(Done::Skipped(key.clone())));
        }
    }
    Some(None)
}

fn run(job: Job) -> Option<Done> {
    let res = match job {
        Job::Frame {
            clip,
            key,
            path,
            at,
            w,
            h,
            graph,
            ..
        } => cl_media::decode_frame_fx(&path, at, w, h, graph.as_deref())
            .map(|frame| Done::Frame { clip, key, frame, exact: true }),
        Job::Thumb { media, path, at, size } => {
            let (w, h) = cl_media::fit_size(size.0, size.1, 192, 108);
            cl_media::decode_frame(&path, at, w, h).map(|frame| Done::Thumb { media, frame })
        }
        Job::Wave { media, path, stream } => {
            cl_media::waveform_stream(&path, WAVE_PER_SEC, stream).map(|peaks| Done::Wave { media, stream, peaks })
        }
        Job::FileThumb { key, path, at } => {
            let (w, h) = cl_media::probe(&path)
                .map(|m| cl_media::fit_size(m.width.max(1), m.height.max(1), 192, 108))
                .unwrap_or((192, 108));
            cl_media::decode_frame(&path, at, w, h).map(|frame| Done::FxThumb { key, frame })
        }
        Job::MediaStrip { key, path, dur, n, size } => {
            let (w, h) = cl_media::fit_size(size.0, size.1, 160, 90);
            let mut data = Vec::with_capacity((w * h * 4 * n) as usize);
            let mut last: Option<Vec<u8>> = None;
            for i in 0..n {
                let at = Time::from_secs_f64(dur.as_secs_f64() * (i as f64 + 0.5) / n as f64);
                // if a frame is missing (e.g. shortly before the end), repeat the previous one
                let f = cl_media::decode_frame(&path, at, w, h)
                    .ok()
                    .filter(|f| f.data.len() == (w * h * 4) as usize)
                    .map(|f| f.data)
                    .or_else(|| last.clone())
                    .unwrap_or_else(|| vec![0; (w * h * 4) as usize]);
                data.extend_from_slice(&f);
                last = Some(f);
            }
            Ok(Done::FxThumb {
                key,
                frame: RgbaFrame {
                    width: w,
                    height: h * n,
                    data,
                },
            })
        }
        Job::SoundMeta { key, path, sfx } => {
            if let Some(s) = sfx.and_then(cl_fx::sfx::sfx) {
                if let Err(e) = cl_fx::sfx::render(s, &path) {
                    return Some(Done::Error(e));
                }
            }
            cl_media::probe(&path).and_then(|media| {
                let peaks = cl_media::waveform(&path, 40)?;
                Ok(Done::SoundMeta { key, media, bars: sound_bars(&peaks, SOUND_BARS) })
            })
        }
        Job::FxThumb {
            key,
            graph,
            shader,
            w,
            h,
            frames,
            cache,
        } => {
            let need = (w * h * 4 * frames) as usize;
            // cache: raw RGBA bytes
            if let Ok(data) = std::fs::read(&cache) {
                if data.len() == need {
                    return Some(Done::FxThumb {
                        key,
                        frame: RgbaFrame {
                            width: w,
                            height: h * frames,
                            data,
                        },
                    });
                }
            }
            let dir = cache.parent()?.to_path_buf();
            let _ = std::fs::create_dir_all(&dir);
            for (n, b) in SAMPLES {
                // rewrite if an older app version stored a different sample image
                if std::fs::metadata(dir.join(n)).map(|m| m.len()).ok() != Some(b.len() as u64) {
                    let _ = std::fs::write(dir.join(n), b);
                }
            }
            let ffmpeg = |graph: &str, frames: u32| {
                cl_media::ffmpeg_command()
                    .ok()?
                    .current_dir(&dir)
                    .args(["-v", "error", "-nostdin", "-filter_complex", graph, "-map", "[out]", "-frames:v"])
                    .arg(frames.to_string())
                    .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
                    .stdin(Stdio::null())
                    .output()
                    .ok()
            };
            // shader transition with cl-gpu; without GPU or on errors the fallback tile
            if let Some(s) = &shader {
                let strip = ffmpeg(&s.pair_graph, 1)
                    .ok_or_else(|| "FFmpeg".to_string())
                    .and_then(|o| shader_strip(s, &o.stdout, w, h, frames));
                match strip {
                    Ok(data) if data.len() == need => {
                        let _ = std::fs::write(&cache, &data);
                        return Some(Done::FxThumb {
                            key,
                            frame: RgbaFrame { width: w, height: h * frames, data },
                        });
                    }
                    Ok(_) => {}
                    Err(e) => log::warn!("Shader-Kachel {key}: {e}"),
                }
            }
            let out = ffmpeg(&graph, frames)?;
            if out.stdout.len() < need {
                return Some(Done::Error(format!(
                    "{key}: {}",
                    String::from_utf8_lossy(&out.stderr)
                        .lines()
                        .last()
                        .unwrap_or("")
                )));
            }
            let mut data = out.stdout;
            data.truncate(need);
            if let Some(d) = cache.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            // do not persist the fallback of a shader transition: try again on the next start
            if shader.is_none() {
                let _ = std::fs::write(&cache, &data);
            }
            Ok(Done::FxThumb {
                key,
                frame: RgbaFrame {
                    width: w,
                    height: h * frames,
                    data,
                },
            })
        }
        Job::ProjectThumb { out, path, at } => {
            if let Some(d) = out.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let ok = cl_media::ffmpeg_command()
                .ok()?
                .args(["-v", "error", "-nostdin", "-y", "-ss", &format!("{:.3}", at.as_secs_f64())])
                .arg("-i")
                .arg(&path)
                .args(["-frames:v", "1", "-vf", "scale=384:216:force_original_aspect_ratio=decrease,pad=384:216:(ow-iw)/2:(oh-ih)/2:color=0x1F1F24,format=rgba", "-f", "rawvideo", "-pix_fmt", "rgba"])
                .arg(&out)
                .stdin(Stdio::null())
                .status()
                .ok()?
                .success();
            if ok {
                Ok(Done::ProjectThumb { out })
            } else {
                return None;
            }
        }
    };
    Some(res.unwrap_or_else(|e| Done::Error(e.to_string())))
}

impl Decoder {
    pub fn new(ctx: egui::Context) -> Self {
        Self::with_still_decoder(ctx, Arc::new(|p, at, w, h, g, ks| cl_media::decode_frame_opts(p, at, w, h, g, false, Some(ks))))
    }

    fn with_still_decoder(ctx: egui::Context, decode: StillFn) -> Self {
        let (done_tx, rx) = channel::<Done>();
        // player frames
        let (tx, jobs) = channel::<Msg>();
        let generation = Arc::new(AtomicU64::new(0));
        let busy = Arc::new(AtomicUsize::new(0));
        let held: Arc<[AtomicUsize; 4]> = Default::default();
        {
            let (done_tx, ctx, gen, busy, tx, held) = (done_tx.clone(), ctx.clone(), generation.clone(), busy.clone(), tx.clone(), held.clone());
            std::thread::Builder::new()
                .name("cl-decoder".into())
                .spawn(move || frame_dispatcher(jobs, tx, done_tx, ctx, gen, busy, held, decode))
                .expect("Decoder-Thread");
        }
        let strips_wanted: Arc<Mutex<HashSet<String>>> = Default::default();
        let (tx_fx, jobs_fx) = channel::<(u64, Job)>();
        {
            let (done_tx, ctx) = (done_tx.clone(), ctx.clone());
            let (gen, wanted, busy) = (generation.clone(), strips_wanted.clone(), busy.clone());
            std::thread::Builder::new()
                .name("cl-decoder-fx".into())
                .spawn(move || {
                    while let Ok(first) = jobs_fx.recv() {
                        yield_to_player(&busy);
                        let mut batch = vec![first];
                        batch.extend(jobs_fx.try_iter().take(3));
                        let mut skipped = Vec::new();
                        let batch: Vec<Job> = batch
                            .into_iter()
                            .filter(|(g, j)| match check(j, *g, &gen, &wanted) {
                                None => false,
                                Some(Some(d)) => {
                                    skipped.push(d);
                                    false
                                }
                                Some(None) => true,
                            })
                            .map(|(_, j)| j)
                            .collect();
                        let results: Vec<Option<Done>> = std::thread::scope(|sc| {
                            let hs: Vec<_> = batch.into_iter().map(|j| sc.spawn(move || run(j))).collect();
                            hs.into_iter().map(|h| h.join().ok().flatten()).collect()
                        });
                        for d in skipped.into_iter().chain(results.into_iter().flatten()) {
                            if done_tx.send(d).is_err() {
                                return;
                            }
                        }
                        ctx.request_repaint();
                    }
                })
                .expect("Decoder-Thread");
        }
        // background
        let (tx_bg, jobs_bg) = channel::<(u64, Job)>();
        let (gen, wanted) = (generation.clone(), strips_wanted.clone());
        std::thread::Builder::new()
            .name("cl-decoder-bg".into())
            .spawn(move || {
                while let Ok((g, job)) = jobs_bg.recv() {
                    yield_to_player(&busy);
                    let d = match check(&job, g, &gen, &wanted) {
                        None => continue,
                        Some(Some(d)) => Some(d),
                        Some(None) => run(job),
                    };
                    if let Some(d) = d {
                        if done_tx.send(d).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                }
            })
            .expect("Decoder-Thread");
        Self {
            tx,
            tx_bg,
            tx_fx,
            rx,
            strips_wanted,
            generation,
            held,
        }
    }

    pub fn send(&self, job: Job) {
        let g = self.generation.load(Ordering::Relaxed);
        match job {
            Job::Frame { .. } => {
                let _ = self.tx.send(Msg::Job(job));
            }
            Job::FxThumb { .. } | Job::MediaStrip { .. } | Job::FileThumb { .. } => {
                let _ = self.tx_fx.send((g, job));
            }
            _ => {
                let _ = self.tx_bg.send((g, job));
            }
        }
    }

    /// Project change: still waiting jobs of the old project expire
    pub fn next_generation(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

/// Job for the [`Starter`]: open a stream from `source_at` (the reply carries `open` back).
pub struct OpenJob {
    pub open: crate::playcore::Open,
    /// Source fingerprint (see `playcore::Want::alias`), comes back with the stream
    pub alias: u64,
    pub path: PathBuf,
    pub source_at: Time,
    pub w: u32,
    pub h: u32,
    pub fps: Rational,
    pub graph: Option<String>,
    pub hw: cl_media::Hw,
}

/// Opens playback streams in its own long-lived thread: the process start (5–20 ms on
/// Windows) does not hold up the UI. Long-lived also because of Linux: `PR_SET_PDEATHSIG` is tied to the
/// starting thread (see `cl_media::proc`) – a stream from a short-lived thread ended with it.
pub struct Starter {
    tx: Sender<OpenJob>,
    pub rx: Receiver<(crate::playcore::Open, f64, u64, Option<FrameStream>)>,
}

impl Starter {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, jobs) = channel::<OpenJob>();
        let (done, rx) = channel();
        std::thread::Builder::new()
            .name("cl-stream-start".into())
            .spawn(move || {
                while let Ok(first) = jobs.recv() {
                    for j in newest_per_layer(std::iter::once(first).chain(jobs.try_iter()).collect()) {
                        let s = FrameStream::open_opts(&j.path, j.source_at, j.w, j.h, j.fps, j.graph.as_deref(), j.hw).ok();
                        if done.send((j.open, j.fps.as_f64(), j.alias, s)).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                }
            })
            .expect("Strom-Thread");
        Self { tx, rx }
    }
    pub fn send(&self, j: OpenJob) {
        let _ = self.tx.send(j);
    }

    /// Test: starter that opens nothing – the jobs arrive at the second value
    #[cfg(test)]
    pub fn capture() -> (Self, Receiver<OpenJob>) {
        let (tx, jobs) = channel();
        let (_, rx) = channel();
        (Self { tx, rx }, jobs)
    }
}

/// Waiting jobs: only the newest per layer (the core only attaches the most recently requested
/// number anyway). Otherwise, while dragging a slider during Play, the starter started every stale
/// stream in turn, and the right one would only come after that backlog.
fn newest_per_layer(jobs: Vec<OpenJob>) -> Vec<OpenJob> {
    let mut out: Vec<OpenJob> = Vec::with_capacity(jobs.len());
    for j in jobs {
        out.retain(|o| o.open.id != j.open.id);
        out.push(j);
    }
    out
}

/// Hardware decoding for a playback stream? Only for originals whose software decoding
/// slows down: above 1080p, HEVC/AV1/VP9 from 1080p and 1080p from 50 fps (measured: 4K HEVC 10-bit
/// 3.6 → 0.9 cores, 4K H.264 2.3 → 0.6; 1080p60 H.264 under full load 6.1 → 3.6 s CPU per 10 s video –
/// in software a single 1080p60 stream under outside load no longer kept up in real time). Proxies
/// and 1080p30 are fast enough in software, and opening a hardware decoder costs
/// 200–300 ms. Single frames stay fully in software (click → frame 0.2–0.4 s slower with D3D11VA).
/// Decoding of the stream: hardware per [`stream_hw`]; up to 1080p it starts in software (first
/// frame 0.3–0.8 s sooner, the hardware takes over after about 1 s). Larger sources go straight to
/// hardware: 4K in software alongside slowed both (measured PL_gross: 38–53 instead of 60 fps).
pub fn stream_decode(m: &cl_core::MediaRef, path: &std::path::Path) -> cl_media::Hw {
    match stream_hw(m, path) {
        false => cl_media::Hw::Off,
        true if m.width.max(m.height) <= 1920 && m.width.min(m.height) <= 1080 => cl_media::Hw::AfterSoftware,
        true => cl_media::Hw::On,
    }
}

pub fn stream_hw(m: &cl_core::MediaRef, path: &std::path::Path) -> bool {
    let codec = m.video_codec.as_deref().unwrap_or("").to_ascii_lowercase();
    let high_rate = m.fps.is_some_and(|f| f.as_f64() >= 50.0);
    // hardware decoders may hand back frames without the alpha plane: transparent videos in software
    path == m.path
        && !m.has_alpha
        && (m.width > 1920
            || m.height > 1080
            || (m.width.min(m.height) >= 1080 && (high_rate || ["hevc", "av1", "vp9"].iter().any(|c| codec.contains(c)))))
}

/// Frame for a texture. Video frames are almost always opaque: then copy without conversion
/// (opaque is premultiplied = unpremultiplied; measured 1280×720: 9 → ~1 ms per frame in the
/// UI thread), otherwise with premultiplying.
pub fn to_color_image(f: &RgbaFrame) -> egui::ColorImage {
    let size = [f.width as usize, f.height as usize];
    // check alpha word by word (fast); oddly aligned buffer: byte by byte
    let opaque = match bytemuck::try_cast_slice::<u8, u32>(&f.data) {
        Ok(w) => w.iter().all(|x| x.to_le() >> 24 == 255),
        Err(_) => f.data.chunks_exact(4).all(|p| p[3] == 255),
    };
    if opaque {
        egui::ColorImage { size, pixels: bytemuck::cast_slice::<u8, egui::Color32>(&f.data).to_vec() }
    } else {
        egui::ColorImage::from_rgba_unmultiplied(size, &f.data)
    }
}

/// Sample image for effect/transition tiles (CC0 images from Wikimedia Commons:
/// “Sunset timelapse on the beach in Libreville, Gabon” and “Mavora Lake South”).
/// Runs in the folder of the tile cache, see `Job::FxThumb`.
pub fn sample_source(w: u32, h: u32, warm: bool) -> String {
    let f = if warm { "fx-sample-a.jpg" } else { "fx-sample-b.jpg" };
    // `fps=25` tells the graph the frame rate: without it `xfade` in FFmpeg 7 (Linux distributions)
    // rejects the source – all transition tiles stayed empty there
    format!("movie={f},loop=loop=-1:size=1:start=0,setpts=N/25/TB,fps=25,scale={w}:{h},setsar=1")
}

const SAMPLES: [(&str, &[u8]); 2] = [
    ("fx-sample-a.jpg", include_bytes!("../assets/fx-sample-a.jpg")),
    ("fx-sample-b.jpg", include_bytes!("../assets/fx-sample-b.jpg")),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The sample source of the tiles states its frame rate (FFmpeg 7 on Linux needs it for
    /// `xfade`; reproduced with Debian 13, FFmpeg 7.1: without `fps` “Error configuring filter graph”).
    #[test]
    fn tile_source_declares_its_frame_rate() {
        assert!(sample_source(192, 108, true).contains(",setpts=N/25/TB,fps=25,"));
    }

    #[test]
    fn hardware_streams_for_large_or_fast_originals() {
        let m = |w, h, fps: u32, codec: &str| cl_core::MediaRef {
            path: "a.mp4".into(),
            width: w,
            height: h,
            fps: Some(Rational::new(fps, 1)),
            video_codec: Some(codec.into()),
            id: MediaId::new(),
            kind: cl_core::MediaKind::Video,
            duration: Time::ZERO,
            has_audio: false,
            audio_codec: None,
            audio_streams: 0,
            has_alpha: false,
        };
        let p = std::path::Path::new("a.mp4");
        assert!(stream_hw(&m(1920, 1080, 60, "h264"), p), "1080p60: hardware");
        assert!(!stream_hw(&m(1920, 1080, 30, "h264"), p), "1080p30 H.264: software is enough");
        assert!(stream_hw(&m(1920, 1080, 30, "hevc"), p));
        assert!(stream_hw(&m(3840, 2160, 30, "h264"), p));
        assert!(!stream_hw(&m(1280, 720, 60, "h264"), p));
        assert!(!stream_hw(&m(1920, 1080, 60, "h264"), std::path::Path::new("proxy.mp4")), "proxy: software");
        assert!(stream_hw(&m(1080, 1920, 30, "hevc"), p), "portrait");
        assert!(!stream_hw(&m(1280, 720, 60, "hevc"), p));
        assert!(!stream_hw(&m(3840, 2160, 60, "hevc"), std::path::Path::new("proxy.mp4")), "Proxy");
        // up to 1080p the stream starts in software (first frame sooner), 4K straight in hardware
        use cl_media::Hw;
        assert_eq!(stream_decode(&m(1920, 1080, 60, "h264"), p), Hw::AfterSoftware);
        assert_eq!(stream_decode(&m(1080, 1920, 30, "hevc"), p), Hw::AfterSoftware, "portrait");
        assert_eq!(stream_decode(&m(3840, 2160, 60, "hevc"), p), Hw::On);
        assert_eq!(stream_decode(&m(1920, 1080, 30, "h264"), p), Hw::Off);
    }

    #[test]
    fn color_image_fast_path_matches_unmultiplied() {
        let opaque = RgbaFrame { width: 3, height: 2, data: [10u8, 20, 30, 255].repeat(6) };
        let mut alpha = opaque.clone();
        alpha.data[7] = 128;
        for f in [opaque, alpha] {
            assert_eq!(to_color_image(&f), egui::ColorImage::from_rgba_unmultiplied([3, 2], &f.data));
        }
    }

    /// Animated images: decoded once in the background, then every frame from memory;
    /// videos and still images go the old way.
    #[test]
    fn animations_are_served_from_memory_after_one_decode() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.gif");
        let ok = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=32x32:rate=10:duration=0.5", "-loop", "0"])
            .arg(&f)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let a = cl_media::animation(&f).unwrap();
        let mut an = Anims::default();
        let at = a.seek_time(a.starts[3]);
        assert_eq!(an.get(&f, 16, 16, at).err(), Some(true), "first time: decode");
        assert_eq!(an.get(&f, 16, 16, at).err(), Some(false), "already running: not twice");
        let frames = cl_media::decode_animation(&f, 16, 16, ANIM_ONE).map(Arc::new);
        assert!(frames.is_some());
        an.put((f.clone(), 16, 16), frames.clone());
        assert_eq!(an.get(&f, 16, 16, at).unwrap().data, frames.unwrap()[3].data);
        // other size: own entry; video: never
        assert_eq!(an.get(&f, 8, 8, at).err(), Some(true));
        assert_eq!(an.get(std::path::Path::new("x.mp4"), 16, 16, at).err(), Some(false));
    }

    /// Tile of a shader transition: frame pair is split correctly, frame 0 = A, strip has
    /// `frames` frames; without GPU an error (then the fallback tile applies).
    #[test]
    fn shader_strip_splits_pair_and_starts_with_a() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.wgsl");
        std::fs::write(
            &file,
            "fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f { return mix(from_color(uv), to_color(uv), progress); }",
        )
        .unwrap();
        let s = ShaderTile { file, params: vec![], pair_graph: String::new() };
        let (w, h) = (4u32, 2u32);
        // left red, right blue
        let pair: Vec<u8> = (0..h)
            .flat_map(|_| (0..w * 2).flat_map(|x| if x < w { [255, 0, 0, 255] } else { [0, 0, 255, 255] }))
            .collect();
        let r = shader_strip(&s, &pair, w, h, 4);
        if cl_gpu::gpu().is_none() {
            assert!(r.is_err());
            eprintln!("skipped (no GPU): shader_strip_splits_pair_and_starts_with_a");
            return;
        }
        let r = r.unwrap();
        assert_eq!(r.len(), (w * h * 4 * 4) as usize);
        assert!(r[..(w * h * 4) as usize].chunks(4).all(|p| p == [255, 0, 0, 255]));
        // frame 2 of 4: half blended
        let mid = &r[(w * h * 4 * 2) as usize..][..4];
        assert!(mid[0].abs_diff(128) <= 1 && mid[2].abs_diff(128) <= 1, "{mid:?}");
        assert!(shader_strip(&s, &pair[..8], w, h, 4).is_err());
    }

    fn still(key: u64, path: &str, at: f64) -> Still {
        Still { key, path: PathBuf::from(path), at: Time::from_secs_f64(at), w: 4, h: 4, graph: None }
    }

    /// Stand: a running job finishes, only the newest further job waits; a jump or another file
    /// kills the running one.
    #[test]
    fn newest_job_waits_and_jumps_replace() {
        let (a, b) = (ClipId::new(), ClipId::new());
        let mut f = InFlight::default();
        let (_, k1, old) = f.offer(a, still(1, "a.mp4", 5.0)).unwrap();
        assert!(old.is_none());
        assert!(f.offer(a, still(1, "a.mp4", 5.0)).is_none(), "same job already running");
        // dragging: the newer ones wait, only the newest stays
        for (k, at) in [(2, 5.1), (3, 5.5), (4, 6.2)] {
            assert!(f.offer(a, still(k, "a.mp4", at)).is_none(), "job {k} waits");
        }
        assert!(!k1.is_killed());
        let next = f.finish(a, 1).expect("current job").expect("newest waits");
        assert_eq!(next.key, 4);
        assert!(f.finish(a, 1).is_none(), "stale result");
        let (_, k4, _) = f.offer(a, next).unwrap();
        // the running job came back as newest: the waiting one is dropped
        f.offer(a, still(5, "a.mp4", 6.3));
        assert!(f.offer(a, still(4, "a.mp4", 6.2)).is_none());
        assert!(f.finish(a, 4).unwrap().is_none(), "nothing waits any more");
        // jump (far from the running and from the previous job): kill and start at once
        let (_, k6, _) = f.offer(a, still(6, "a.mp4", 1.0)).unwrap();
        assert!(!k4.is_killed());
        let (s, _, old) = f.offer(a, still(7, "a.mp4", 9.0)).unwrap();
        assert_eq!(s.key, 7);
        assert!(Arc::ptr_eq(&old.unwrap(), &k6));
        // other file (proxy ↔ original): the running frame is not wanted
        assert!(f.offer(a, still(8, "proxy.mp4", 9.0)).unwrap().2.is_some());
        // a drag that wandered far from the running one in small steps: no kill
        f.offer(b, still(10, "b.mp4", 0.0)).unwrap();
        for i in 1..30 {
            assert!(f.offer(b, still(10 + i, "b.mp4", i as f64 * 0.1)).is_none(), "step {i}");
        }
        assert_eq!(f.len(), 2);
        // frame from memory: the clip's running job expires, the waiting one too
        assert!(f.cancel(b).is_some() && f.cancel(b).is_none());
        assert!(f.finish(b, 10).is_none());
        assert_eq!(f.len(), 1);
    }

    /// Dispatcher with a fake decoder (no FFmpeg, no clock): many quick jobs of one clip start at
    /// most two decodes, the overtaken ones never; the first frame comes as neighbour, then the exact
    /// one without further input; finished frames are cached; other clips run independently.
    #[test]
    fn quick_jobs_start_at_most_two_decodes_per_clip() {
        // each decode reports (source time ms, release) and waits for its release
        let (calls_tx, calls) = channel::<(u64, Sender<()>)>();
        let calls_tx = Mutex::new(calls_tx);
        let d = Decoder::with_still_decoder(
            egui::Context::default(),
            Arc::new(move |_, at, w, h, _, ks| {
                let (go, wait) = channel();
                let ms = (at.as_secs_f64() * 1000.0).round() as u64;
                calls_tx.lock().unwrap().send((ms, go)).unwrap();
                loop {
                    if ks.is_killed() {
                        return Err(cl_media::MediaError::Cancelled);
                    }
                    if wait.recv_timeout(std::time::Duration::from_millis(1)).is_ok() {
                        return Ok((RgbaFrame { width: w, height: h, data: vec![(ms / 10) as u8; (w * h * 4) as usize] }, Default::default()));
                    }
                }
            }),
        );
        let (a, b) = (ClipId::new(), ClipId::new());
        let job = |clip, key, at: f64| Job::Frame { clip, key, path: "a.mp4".into(), at: Time::from_secs_f64(at), w: 4, h: 2, graph: None, window: None };
        let call = || calls.recv_timeout(std::time::Duration::from_secs(10)).expect("decode started");
        let frame = || match d.rx.recv_timeout(std::time::Duration::from_secs(10)).expect("frame") {
            Done::Frame { clip, key, exact, frame } => (clip, key, exact, frame.data[0]),
            _ => panic!("not a frame"),
        };
        d.send(job(a, 1, 1.0));
        let (ms, go1) = call();
        assert_eq!(ms, 1000);
        for k in 2..=10 {
            d.send(job(a, k, 1.0 + k as f64 * 0.01));
        }
        // other clip: not held up
        d.send(job(b, 50, 3.0));
        let (ms, go_b) = call();
        assert_eq!(ms, 3000);
        go1.send(()).unwrap();
        assert_eq!(frame(), (a, 1, false, 100), "running frame bridges as neighbour");
        // the newest starts by itself; the overtaken 2…9 never
        let (ms, go10) = call();
        assert_eq!(ms, 1100);
        go10.send(()).unwrap();
        assert_eq!(frame(), (a, 10, true, 110), "exact frame without further input");
        go_b.send(()).unwrap();
        assert_eq!(frame(), (b, 50, true, 44));
        // cached: back to the first frame without a decode
        d.send(job(a, 1, 1.0));
        assert_eq!(frame(), (a, 1, true, 100));
        // far jump kills the running decode and starts the new one at once
        d.send(job(a, 20, 5.0));
        let (_, _go20) = call();
        d.send(job(a, 21, 9.0));
        let (ms, go21) = call();
        assert_eq!(ms, 9000);
        go21.send(()).unwrap();
        assert_eq!(frame(), (a, 21, true, 132));
        assert!(calls.try_recv().is_err(), "no further decode");
    }

    /// Dragging a slider during Play: many jobs of the same layer pile up – only the newest
    /// per layer is opened, in the order of their last job.
    #[test]
    fn starter_opens_only_the_newest_job_per_layer() {
        let job = |id, seq| OpenJob {
            open: crate::playcore::Open { id, key: seq, at: Time::ZERO, gen: 0, seq },
            alias: 0,
            path: PathBuf::from("a.mp4"),
            source_at: Time::ZERO,
            w: 16,
            h: 16,
            fps: Rational::new(30, 1),
            graph: None,
            hw: cl_media::Hw::Off,
        };
        let jobs = vec![job(1, 1), job(2, 2), job(1, 3), job(1, 4), job(3, 5), job(2, 6)];
        let left: Vec<(u128, u64)> = newest_per_layer(jobs).iter().map(|j| (j.open.id, j.open.seq)).collect();
        assert_eq!(left, vec![(1, 4), (3, 5), (2, 6)]);
        assert!(newest_per_layer(Vec::new()).is_empty());
    }

    /// Reverse window: size under the memory limit, at most 1 s; missing frame = window
    /// that ends with it; ahead the one right before, only once it fits beside the held ones.
    #[test]
    fn reverse_window_math() {
        let (n, cap) = window_plan(1280, 720, 60.0);
        assert!(cap as usize * 1280 * 720 * 4 <= WINDOW_BYTES, "{cap} frames");
        assert_eq!((n, cap), (17, 34));
        assert_eq!(window_plan(320, 180, 60.0).0, 60, "at most 1 s");
        assert_eq!(window_plan(320, 180, 23.976).0, 24);
        assert_eq!(window_plan(7680, 4320, 60.0), (1, 2), "huge: still one frame");
        let plan = (21, 32);
        assert_eq!(next_window(500, false, 500, 0, 0, plan), Some((480, 21)));
        assert_eq!(next_window(10, false, 10, 0, 0, plan), Some((0, 11)), "not before the first frame");
        // 21 held (480..=500): the next fits only when at most 11 are left
        assert_eq!(next_window(500, true, 480, 21, 0, plan), None);
        assert_eq!(next_window(490, true, 480, 11, 0, plan), Some((459, 21)));
        assert_eq!(next_window(5, true, 0, 6, 0, plan), None, "file start reached");
        assert_eq!(next_window(12, true, 4, 9, 0, plan), Some((0, 4)));
        // slow source (window takes 60 frames): decode up to `idx`, keep the first 21 from where
        // the clock will then be – the missing frame is always in the range
        assert_eq!(next_window(500, false, 500, 0, 60, plan), Some((420, 81)));
        // ahead: behind the lead, not right before the held ones (they are shown by then)
        assert_eq!(next_window(490, true, 480, 11, 60, plan), Some((410, 70)));
        assert_eq!(next_window(490, true, 480, 11, 5, plan), Some((459, 21)), "small lead");
    }

    /// Timeline thumbnails take the size from the job (`MediaRef`, rotation applied) instead of
    /// probing the file once more: a stored portrait size gives a portrait image.
    #[test]
    fn thumb_uses_the_size_from_the_job() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("t.mp4");
        let ok = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=128x72:r=10:d=1", "-pix_fmt", "yuv420p"])
            .arg(&src)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let media = MediaId::new();
        match run(Job::Thumb { media, path: src, at: Time::ZERO, size: (72, 128) }) {
            Some(Done::Thumb { frame, .. }) => assert_eq!((frame.width, frame.height), (60, 108)),
            _ => panic!("no thumbnail"),
        }
    }

    /// Reverse playback: jobs with window, frame by frame backwards – each arrives, and each
    /// is byte-identical to the single frame without window.
    #[test]
    fn reverse_window_delivers_the_same_frames() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("r.mp4");
        let ok = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=128x72:r=30:d=4", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-g", "30"])
            .arg(&src)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let d = Decoder::new(egui::Context::default());
        let fps = Rational::new(30, 1);
        let clip = ClipId::new();
        let at = |i: i64| Time::from_secs_f64(i as f64 / 30.0);
        let mut other = 0;
        for i in (40..100).rev() {
            let key = 1000 + i as u64;
            d.send(Job::Frame { clip, key, path: src.clone(), at: at(i), w: 64, h: 36, graph: None, window: Some(fps) });
            let f = loop {
                match d.rx.recv_timeout(std::time::Duration::from_secs(20)).expect("no frame") {
                    Done::Frame { key: k, frame, exact: true, .. } if k == key => break frame,
                    // slow machine: the job waited longer than the window lasts, a neighbour came
                    // (playback); asked again without window as the player does in the pause
                    Done::Frame { key: k, exact: false, .. } if k == key => {
                        d.send(Job::Frame { clip, key, path: src.clone(), at: at(i), w: 64, h: 36, graph: None, window: None })
                    }
                    Done::Error(e) | Done::FrameFailed { error: e, .. } => panic!("{e}"),
                    _ => other += 1,
                }
            };
            let one = cl_media::decode_frame(&src, at(i), 64, 36).unwrap();
            assert!(one.data == f.data, "frame {i} differs");
        }
        assert_eq!(other, 0);
    }

    /// CI finding: a job is answered from the window with a held neighbour (window came early,
    /// or the job waited longer than the clock allows) – that must never pass as the frame itself.
    #[test]
    fn window_neighbour_is_not_the_requested_frame() {
        let clip = ClipId::new();
        let frames: std::collections::BTreeMap<i64, RgbaFrame> = (60..70).map(|i| (i, RgbaFrame { width: 1, height: 1, data: vec![i as u8; 4] })).collect();
        let answer = |range, idx| match window_answer(&frames, range, clip, 7, idx) {
            Some(Done::Frame { key: 7, frame, exact, .. }) => Some((frame.data[0] as i64, exact)),
            _ => None,
        };
        assert_eq!(answer(65..=65, 65), Some((65, true)));
        assert_eq!(answer(45..=75, 75), Some((69, false)), "gap above the window");
        assert_eq!(answer(i64::MIN..=62, 70), Some((62, false)), "job waited: where the clock now is");
        assert_eq!(answer(70..=75, 75), None);
    }

    #[test]
    fn background_waits_for_player_frames_but_not_forever() {
        let ms = std::time::Duration::from_millis;
        assert!(bg_may_start(0, ms(0)));
        assert!(!bg_may_start(2, ms(100)));
        assert!(bg_may_start(2, ms(1500)), "no starving during continuous scrubbing");
    }

    /// Waits for the frame with key `key`; counts other delivered player frames.
    fn wait_frame(d: &Decoder, key: u64, other: &mut usize) -> std::time::Instant {
        loop {
            match d.rx.recv_timeout(std::time::Duration::from_secs(20)).expect("no frame") {
                Done::Frame { key: k, .. } if k == key => return std::time::Instant::now(),
                Done::Frame { .. } => *other += 1,
                Done::Error(e) | Done::FrameFailed { error: e, .. } => panic!("{e}"),
                _ => {}
            }
        }
    }

    fn test_media() -> Vec<PathBuf> {
        let home = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
        [
            home.join("Downloads/videoplayback.mp4"),
            PathBuf::from("F:/cutline-testmedien/1080p60_gop10s.mp4"),
            PathBuf::from("F:/cutline-testmedien/4k60_h264_80mbit.mp4"),
            PathBuf::from("F:/cutline-testmedien/4k60_hevc10_50mbit.mp4"),
        ]
        .into_iter()
        .filter(|p| p.is_file())
        .collect()
    }

    fn frame_job(clip: ClipId, key: u64, path: &std::path::Path, at: f64) -> Job {
        Job::Frame { clip, key, path: path.to_path_buf(), at: Time::from_secs_f64(at), w: 696, h: 392, graph: None, window: None }
    }

    /// Measurement test (manual): scrubbing – 20 jobs 15 ms apart at different
    /// places; measured is the time from the last job to its frame.
    /// `cargo test --release -p cl-app -- --ignored --nocapture measure_scrub`
    #[test]
    #[ignore]
    fn measure_scrub() {
        for path in test_media() {
            let d = Decoder::new(egui::Context::default());
            let clip = ClipId::new();
            let mut times = Vec::new();
            let mut shown_stale = 0;
            for round in 0..3u64 {
                for i in 0..20u64 {
                    let at = 30.0 + round as f64 * 40.0 + i as f64 * 0.7;
                    d.send(frame_job(clip, round * 100 + i, &path, at));
                    if i < 19 {
                        std::thread::sleep(std::time::Duration::from_millis(15));
                    }
                }
                let t = std::time::Instant::now();
                let done = wait_frame(&d, round * 100 + 19, &mut shown_stale);
                times.push(done.duration_since(t).as_millis());
            }
            println!(
                "{}: letztes Bild nach {:?} ms, veraltete gezeigt: {shown_stale}",
                path.file_name().unwrap().to_string_lossy(),
                times
            );
        }
    }

    /// Measurement test (manual): single frame without and during a proxy build (the build pauses
    /// as soon as the player requests frames).
    /// `cargo test --release -p cl-app -- --ignored --nocapture measure_still_during_proxy`
    #[test]
    #[ignore]
    fn measure_still_during_proxy() {
        let Some(path) = test_media().into_iter().next() else { return };
        let m = cl_media::probe(&path).unwrap();
        let d = Decoder::new(egui::Context::default());
        let clip = ClipId::new();
        let mut key = 0u64;
        let mut run = |label: &str| {
            let mut ms = Vec::new();
            for i in 0..10 {
                key += 1;
                let t = std::time::Instant::now();
                d.send(frame_job(clip, key, &path, 60.0 + i as f64 * 97.3 + key as f64 * 0.01));
                ms.push(wait_frame(&d, key, &mut 0).duration_since(t).as_millis());
                // spacing like clicking
                std::thread::sleep(std::time::Duration::from_millis(1200));
            }
            ms.sort();
            println!("{label}: Median {} ms, max {} ms ({ms:?})", ms[5], ms[9]);
            ms[5]
        };
        let base = run("ohne Proxy-Bau");
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("p.mp4");
        let spec = crate::proxy::proxy_spec(&m, 540, Rational::new(30, 1));
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let builder = {
            let (m, cancel) = (m.clone(), cancel.clone());
            std::thread::spawn(move || crate::proxy::build(&m, &out, &spec, &|_| {}, &cancel))
        };
        std::thread::sleep(std::time::Duration::from_secs(2));
        let during = run("während Proxy-Bau");
        cancel.store(true, Ordering::Relaxed);
        let _ = builder.join();
        println!("Verhältnis {:.2}", during as f64 / base.max(1) as f64);
    }

    #[test]
    fn frame_cache_is_bounded_and_freed_on_project_change() {
        let frame = || RgbaFrame { width: 10, height: 10, data: vec![0; 400] };
        let mut c = FrameCache::new(1000);
        for k in 0..5 {
            c.put(k, frame());
        }
        // limit: oldest drop out
        assert!(c.bytes <= 1000 && c.get(0).is_none() && c.get(4).is_some());
        c.set_generation(0);
        assert!(c.get(4).is_some());
        // project change: everything freed
        c.set_generation(1);
        assert_eq!((c.bytes, c.map.len(), c.order.len()), (0, 0, 0));
        assert_eq!(c.budget, 1000);
    }
}
