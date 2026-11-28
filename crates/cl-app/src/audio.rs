//! Audio playback: FFmpeg mixes the timeline in windows from the playhead, cpal plays them.
//! The played samples are the clock (audio clock) the picture follows.
//!
//! The whole project is never mixed, only a window of [`WINDOW_SECS`] (only the clips
//! in it, so few FFmpeg inputs; the first is shorter so sound starts quickly). Before a window ends, a second
//! process computes the next one; both are joined at the sample (each window delivers exactly
//! its number of sample frames; settling of resampler/limiter lies before the window start
//! and is discarded).
//!
//! On playback the picture starts at once on system time ([`AudioPlayer::start_synced`] +
//! [`AudioPlayer::anchor`]): when sound arrives, the output skips the samples already past
//! and comes in with a short fade exactly where the picture is.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::Read;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cl_core::{Project, Time};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Length of an audio window (seconds)
pub const WINDOW_SECS: u64 = 20;
/// Length of the first window: fewer inputs → first sound sooner (~25–80 ms per input)
pub const FIRST_SECS: u64 = 6;
/// The next window is started this many seconds before the window end
pub const LEAD_SECS: u64 = 5;
/// Settling time before the window start (computed and discarded)
pub const PRE_MS: u64 = 200;
/// Fade in/out when sound starts and stops (against clicks)
pub const FADE_MS: u64 = 15;
/// Crossfade from the running mix to a rebuilt one (audible change during playback)
pub const XFADE_MS: u64 = 30;
/// Extra mix behind a window at preview speed ≠ 1 (see [`open_window`])
const TEMPO_TAIL_MS: u64 = 150;

pub struct AudioPlayer {
    /// Control of the output thread (true = play, false = pause); dropping it ends the thread
    ctl: std::sync::mpsc::Sender<bool>,
    /// Output could not be opened (then sound never comes, the picture runs on system time)
    failed: Arc<AtomicBool>,
    shared: Arc<Shared>,
    /// Peak level left/right since the last query (f32 bits)
    peaks: Arc<[AtomicU32; 2]>,
    /// Sample rate of the output device
    rate: u32,
    /// Sample frames per timeline second (`rate` / preview speed): the unit of windows and clock
    tl_rate: u32,
    pub start: Time,
    /// Newest audible state waiting for its rebuild ([`Self::remix`])
    queued: Option<Project>,
    /// Start of the rebuild in flight (perf log)
    remix_t0: Option<Instant>,
}

/// State shared by output (cpal), feed (windows) and UI
struct Shared {
    out: Mutex<Out>,
    /// consumed frames (skipped ones too) = clock
    played: AtomicU64,
    /// Time (ns since `base`) and frames of the last output callback (for a smooth clock)
    cb_ns: AtomicU64,
    cb_frames: AtomicU64,
    running: AtomicBool,
    paused: AtomicBool,
    base: Instant,
    stop: AtomicBool,
    /// All started FFmpeg processes (killed on stop so blocked readers get free)
    kids: Mutex<Vec<Arc<Mutex<Child>>>>,
    first_sound: OnceLock<Instant>,
    /// Sample rate of the output device and preview speed (pitch kept): what FFmpeg mixes with
    dev_rate: u32,
    tempo: f64,
}

/// Chosen output device (name); `None` = system default
static DEVICE: Mutex<Option<String>> = Mutex::new(None);

/// Playback volume (slider in the player): only how loud Cutline plays – not project,
/// export or level meter. Survives a restart in `last-used.json`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Volume {
    /// 0–100
    pub percent: f32,
    pub muted: bool,
}

impl Default for Volume {
    fn default() -> Self {
        Self { percent: 100.0, muted: false }
    }
}

impl Volume {
    /// Gain for the slider: cubic (feels even), 100 % = unchanged, never louder
    pub fn gain(self) -> f32 {
        if self.muted {
            return 0.0;
        }
        // max/min instead of clamp: NaN from a broken file becomes 0
        (self.percent / 100.0).max(0.0).min(1.0).powi(3)
    }

    /// One 5 % step (mouse wheel, actions) on the grid of 5; louder lifts „muted“
    pub fn step(self, up: bool) -> Self {
        let p = ((self.percent / 5.0).round() + if up { 1.0 } else { -1.0 }) * 5.0;
        Self { percent: p.clamp(0.0, 100.0), muted: self.muted && !up }
    }
}

/// Target gain of the output (f32 bits, 1.0); the output glides there in [`GAIN_RAMP_MS`]
static GAIN: AtomicU32 = AtomicU32::new(0x3F80_0000);
/// Ramp when the volume changes (against clicks)
const GAIN_RAMP_MS: f32 = 5.0;

pub fn set_volume(v: Volume) {
    GAIN.store(v.gain().to_bits(), Ordering::Relaxed);
}

fn playback_gain() -> f32 {
    f32::from_bits(GAIN.load(Ordering::Relaxed))
}

pub fn set_output_device(name: Option<String>) {
    *DEVICE.lock().unwrap() = name;
    *OUTPUT.lock().unwrap() = None;
}

/// Last used output device with format (name, device, format): enumerating devices and
/// `default_output_config` cost noticeable UI-thread time per sound start. Forgotten on
/// device choice and when the output could not be opened (device unplugged).
type Output = (String, (cpal::Device, cpal::SupportedStreamConfig));
static OUTPUT: Mutex<Option<Output>> = Mutex::new(None);

/// Value for `name` from `slot`, otherwise fetched with `fetch` and remembered.
fn remembered<T: Clone, E>(slot: &mut Option<(String, T)>, name: &str, fetch: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    if let Some((_, v)) = slot.as_ref().filter(|(n, _)| n == name) {
        return Ok(v.clone());
    }
    let v = fetch()?;
    *slot = Some((name.to_string(), v.clone()));
    Ok(v)
}

/// Names of all output devices (for the settings)
pub fn output_devices() -> Vec<String> {
    cpal::default_host()
        .output_devices()
        .map(|d| d.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

// ---------- Window plan (pure computation) ----------

/// One audio window: FFmpeg mixes `mix_from..mix_to`, of which `skip` frames are discarded and
/// exactly `frames` frames kept (anything missing is filled with silence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowPlan {
    pub mix_from: Time,
    pub mix_to: Time,
    pub skip: u64,
    /// first frame of the window, counted from the start of playback
    pub first: u64,
    pub frames: u64,
}

/// Sample frames from `from` to `end` (rounded up: the clock surely reaches the end)
pub fn total_frames(from: Time, end: Time, rate: u32) -> u64 {
    let us = (end - from).0.max(0) as u128;
    (us * rate as u128).div_ceil(1_000_000) as u64
}

/// Window `k` of a playback from `from` to `end`; `None` past the end.
/// Window starts lie on whole seconds after `from`, the settling on whole frames before:
/// so each window computes on the same sample grid as a continuous mix.
pub fn window_plan(from: Time, end: Time, rate: u32, k: u64) -> Option<WindowPlan> {
    let total = total_frames(from, end, rate);
    // Start (seconds from `from`) and length: first FIRST_SECS, then WINDOW_SECS each
    let (start_s, len_s) = match k {
        0 => (0, FIRST_SECS),
        _ => ((k - 1).checked_mul(WINDOW_SECS)?.checked_add(FIRST_SECS)?, WINDOW_SECS),
    };
    let first = start_s.checked_mul(rate as u64).filter(|f| *f < total)?;
    let frames = (len_s * rate as u64).min(total - first);
    // `first < total`: the start lies in the project, no overflow
    let start_us = from.0 + start_s as i64 * 1_000_000;
    let pre = rate as u64 * PRE_MS / 1000;
    let pre_us = (pre as u128 * 1_000_000 / rate.max(1) as u128) as i64;
    let (mix_from, skip) = if start_us >= pre_us {
        (start_us - pre_us, pre)
    } else {
        (0, (start_us.max(0) as u128 * rate as u128 + 500_000) as u64 / 1_000_000)
    };
    let len_us = (frames as u128 * 1_000_000).div_ceil(rate.max(1) as u128) as i64;
    Some(WindowPlan {
        mix_from: Time(mix_from),
        mix_to: Time(start_us + len_us),
        skip,
        first,
        frames,
    })
}

/// Prepare the next window? (`moved` = frames of the running window already taken)
pub fn prepare_next(moved: u64, frames: u64, rate: u32) -> bool {
    moved + LEAD_SECS * rate as u64 >= frames
}

// ---------- Output (pure state machine, used by the cpal callback) ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Sound not yet released (picture buffering): silence, consume nothing
    Waiting,
    /// Clock runs since the anchor: skip past samples, then come in
    Anchored(Instant),
    /// play at once as soon as sound is there (scrubbing, preview)
    Free,
    Running,
}

struct Out {
    buf: VecDeque<f32>,
    phase: Phase,
    played: u64,
    fade_len: u32,
    fade_in: u32,
    /// remaining fade-out frames on abort (silence afterwards)
    closing: Option<u32>,
    /// Feed finished (no more sound to expect)
    finished: bool,
    in_underrun: bool,
    underruns: u64,
    underrun_frames: u64,
    /// current playback gain (glides to [`GAIN`])
    gain: f32,
    /// Feed generation that fills `buf` (a rebuilt mix gets the next one)
    gen: u64,
    /// Rebuilt mix being prepared beside the running one
    alt: Option<Alt>,
    /// Rest of the old mix during the crossfade, remaining and total crossfade frames
    xf_old: VecDeque<f32>,
    xf: u32,
    xf_len: u32,
}

/// Rebuilt mix (audible change during playback): its samples start at output frame `at`.
struct Alt {
    gen: u64,
    at: u64,
    buf: VecDeque<f32>,
}

impl Out {
    fn new(phase: Phase, rate: u32) -> Self {
        Self {
            gen: 0,
            alt: None,
            xf_old: VecDeque::new(),
            xf: 0,
            xf_len: (rate as u64 * XFADE_MS / 1000).max(1) as u32,
            buf: VecDeque::with_capacity(rate as usize * 4),
            phase,
            played: 0,
            fade_len: (rate as u64 * FADE_MS / 1000).max(1) as u32,
            fade_in: 0,
            closing: None,
            finished: false,
            in_underrun: false,
            underruns: 0,
            underrun_frames: 0,
            gain: playback_gain(),
        }
    }

    /// Output `n` stereo frames; `target` = target frames since the anchor (only `Anchored`).
    /// Returns the number of real sound frames output.
    fn render(&mut self, n: usize, target: u64, mut put: impl FnMut(f32, f32)) -> u64 {
        match self.phase {
            Phase::Waiting => {
                (0..n).for_each(|_| put(0.0, 0.0));
                return 0;
            }
            Phase::Anchored(_) => {
                let drop = target.saturating_sub(self.played).min(self.buf.len() as u64 / 2);
                self.buf.drain(..drop as usize * 2);
                self.played += drop;
                if self.played < target || self.buf.len() < 2 {
                    (0..n).for_each(|_| put(0.0, 0.0));
                    return 0;
                }
                self.phase = Phase::Running;
                self.fade_in = self.fade_len;
            }
            Phase::Free => {
                if self.buf.len() < 2 {
                    (0..n).for_each(|_| put(0.0, 0.0));
                    return 0;
                }
                self.phase = Phase::Running;
                self.fade_in = self.fade_len;
            }
            Phase::Running => self.take_alt(n),
        }
        let mut real = 0;
        for _ in 0..n {
            let (Some(l), Some(r)) = (self.buf.pop_front(), self.buf.pop_front()) else {
                if !self.finished {
                    if !self.in_underrun {
                        self.underruns += 1;
                        self.in_underrun = true;
                    }
                    self.underrun_frames += 1;
                }
                put(0.0, 0.0);
                continue;
            };
            self.in_underrun = false;
            real += 1;
            let (mut l, mut r) = (l, r);
            if self.xf > 0 {
                // old mix fades out while the rebuilt one fades in (same frame in both)
                let k = self.xf as f32 / self.xf_len as f32;
                let (ol, or) = (self.xf_old.pop_front().unwrap_or(0.0), self.xf_old.pop_front().unwrap_or(0.0));
                l += (ol - l) * k;
                r += (or - r) * k;
                self.xf -= 1;
                if self.xf == 0 {
                    self.xf_old = VecDeque::new();
                }
            }
            let mut g = 1.0;
            if self.fade_in > 0 {
                g = 1.0 - self.fade_in as f32 / self.fade_len as f32;
                self.fade_in -= 1;
            }
            if let Some(c) = &mut self.closing {
                g *= *c as f32 / self.fade_len as f32;
                *c = c.saturating_sub(1);
            }
            put(l * g, r * g);
        }
        self.played += real;
        real
    }

    /// Rebuilt mix: discard its frames the output has already passed; once it holds the frames from
    /// `played` on (at least a block and the crossfade), it becomes the playing one – no gap, no jump.
    fn take_alt(&mut self, n: usize) {
        let Some(a) = &mut self.alt else { return };
        let drop = self.played.saturating_sub(a.at).min(a.buf.len() as u64 / 2);
        a.buf.drain(..drop as usize * 2);
        a.at += drop;
        if self.closing.is_some() || a.at != self.played || a.buf.len() / 2 < n.max(self.xf_len as usize) {
            return;
        }
        let a = self.alt.take().expect("checked above");
        self.xf_old = std::mem::replace(&mut self.buf, a.buf);
        self.gen = a.gen;
        self.xf = self.xf_len;
        // the old feed may have reached the end; the new one reports its own end
        self.finished = false;
    }

    /// Buffer the feed of generation `gen` fills: the playing one, the rebuilt one, or none (outdated)
    fn slot(&mut self, gen: u64) -> Option<&mut VecDeque<f32>> {
        if gen == self.gen {
            Some(&mut self.buf)
        } else {
            self.alt.as_mut().filter(|a| a.gen == gen).map(|a| &mut a.buf)
        }
    }
}

/// Clock in frames: state before the last callback plus the time since, at most as many
/// frames as the callback output (otherwise the clock would run on during a dropout).
/// `played` already counts the last callback; without subtracting, the clock would lead it by one buffer.
pub fn clock_frames(played: u64, since_cb: Duration, cb_frames: u64, rate: u32) -> u64 {
    played.saturating_sub(cb_frames) + ((since_cb.as_secs_f64() * rate as f64) as u64).min(cb_frames)
}

// ---------- Feed: window by window ----------

/// A running window: FFmpeg + reader thread that puts exactly `frames` frames into `q`.
struct Win {
    child: Arc<Mutex<Child>>,
    q: Arc<Mutex<VecDeque<f32>>>,
    done: Arc<AtomicBool>,
    ready: Arc<OnceLock<Instant>>,
    plan: WindowPlan,
    spawned: Instant,
}

impl Drop for Win {
    fn drop(&mut self) {
        let mut c = self.child.lock().unwrap();
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// Starts the FFmpeg of a window. Called from the thread that starts the sound (UI
/// or the scrub thread that keeps the sound until the end), and from `cl-audio-feed`, which lives
/// until the sound ends – important on Linux: `PR_SET_PDEATHSIG` ends the child with the
/// *starting thread* (see `cl_media::proc`), not only with the process.
/// `rate`: sample frames per timeline second (see `AudioPlayer::tl_rate`).
fn open_window(project: &Project, plan: WindowPlan, rate: u32, shared: &Arc<Shared>) -> Result<Win, String> {
    // at another speed `atempo` settles a few ms: mix a little further, the surplus is discarded
    let tail = if (shared.tempo - 1.0).abs() > 1e-6 { Time(TEMPO_TAIL_MS as i64 * 1000) } else { Time::ZERO };
    let (mut child, workdir) = cl_export::spawn_audio_window_tempo(project, plan.mix_from, plan.mix_to + tail, shared.dev_rate, shared.tempo)
        .map_err(|e| e.to_string())?;
    cl_media::raise_priority(&child);
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let child = Arc::new(Mutex::new(child));
    {
        let mut kids = shared.kids.lock().unwrap();
        kids.retain(|k| k.lock().map(|mut c| c.try_wait().ok().flatten().is_none()).unwrap_or(false));
        kids.push(child.clone());
    }
    let q = Arc::new(Mutex::new(VecDeque::with_capacity(rate as usize * 2)));
    let done = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(OnceLock::new());
    let cap = rate as usize * 2; // 1 s stereo per window ahead
    let (q2, done2, ready2, sh) = (q.clone(), done.clone(), ready.clone(), shared.clone());
    std::thread::Builder::new()
        .name("cl-audio-window".into())
        .spawn(move || {
            let _workdir = workdir; // graph file lives until FFmpeg is done
            let mut skip = plan.skip * 8; // Bytes
            let mut want = plan.frames * 2; // Samples
            let mut raw = vec![0u8; 16 * 1024];
            let mut carry: Vec<u8> = Vec::new();
            while want > 0 && !sh.stop.load(Ordering::Relaxed) {
                while q2.lock().unwrap().len() > cap && !sh.stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let n = match stdout.read(&mut raw) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let mut data = &raw[..n];
                let s = (skip as usize).min(data.len());
                skip -= s as u64;
                data = &data[s..];
                carry.extend_from_slice(data);
                let whole = (carry.len() / 4).min(want as usize);
                if whole > 0 {
                    ready2.get_or_init(Instant::now);
                    let mut b = q2.lock().unwrap();
                    b.extend(carry[..whole * 4].chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])));
                    drop(b);
                    carry.drain(..whole * 4);
                    want -= whole as u64;
                }
            }
            // FFmpeg delivered too little (rounding at the project end, errors): fill with silence
            // so the next window joins at the right place
            if want > 0 && !sh.stop.load(Ordering::Relaxed) {
                if crate::app::perf() {
                    crate::applog::perf_line!("perf: audio window from frame {} padded with {} frames of silence", plan.first, want / 2);
                }
                ready2.get_or_init(Instant::now);
                q2.lock().unwrap().extend(std::iter::repeat_n(0.0, want as usize));
            }
            done2.store(true, Ordering::Release);
        })
        .map_err(|e| e.to_string())?;
    Ok(Win { child, q, done, ready, plan, spawned: Instant::now() })
}

/// Fetches window after window into the output buffer of generation `gen` (at most `max_buffered`
/// samples ahead); ends when a rebuilt mix has replaced it.
fn feed(project: Project, from: Time, rate: u32, shared: Arc<Shared>, mut cur: Win, gen: u64) {
    let end = project.duration();
    let max_buffered = rate as usize * 2; // 1 s stereo
    let mut next: Option<Win> = None;
    let mut moved = 0u64;
    let mut k = 0u64;
    while !shared.stop.load(Ordering::Relaxed) {
        if next.is_none() && prepare_next(moved, cur.plan.frames, rate) {
            if let Some(plan) = window_plan(from, end, rate, k + 1) {
                match open_window(&project, plan, rate, &shared) {
                    Ok(w) => next = Some(w),
                    Err(e) => {
                        log::warn!("Audio: {e}");
                        break;
                    }
                }
            }
        }
        let Some(len) = shared.out.lock().unwrap().slot(gen).map(|b| b.len()) else { break };
        let room = max_buffered.saturating_sub(len) / 2 * 2;
        let mut got = 0;
        if room > 0 {
            let mut q = cur.q.lock().unwrap();
            got = room.min(q.len());
            if got > 0 {
                let mut o = shared.out.lock().unwrap();
                let Some(b) = o.slot(gen) else { break };
                b.extend(q.drain(..got));
            }
        }
        moved += got as u64 / 2;
        let exhausted = cur.done.load(Ordering::Acquire) && cur.q.lock().unwrap().is_empty();
        if exhausted && moved >= cur.plan.frames {
            let Some(n) = next.take() else {
                // a rebuilt mix that is not playing yet must not report the end of the running one
                if shared.out.lock().unwrap().gen == gen {
                    break;
                }
                std::thread::sleep(Duration::from_millis(4));
                continue;
            };
            if crate::app::perf() {
                // Lead: how long the next window already held sound; offset: deviation
                // of the window start from the continuous sample counter (must be 0)
                let lead = n.ready.get().map(|r| r.elapsed().as_secs_f64() * 1000.0);
                crate::applog::perf_line!(
                    "perf: audio window change at frame {}: lead {} ms (start after {:.0} ms), offset {} samples",
                    n.plan.first,
                    lead.map_or("–".into(), |l| format!("{l:.0}")),
                    n.ready.get().map_or(f64::NAN, |r| r.duration_since(n.spawned).as_secs_f64() * 1000.0),
                    (cur.plan.first + moved) as i64 - n.plan.first as i64
                );
            }
            cur = n;
            moved = 0;
            k += 1;
            continue;
        }
        if got == 0 {
            std::thread::sleep(Duration::from_millis(4));
        }
    }
    let mut o = shared.out.lock().unwrap();
    if o.gen == gen {
        o.finished = true;
    } else if o.alt.as_ref().is_some_and(|a| a.gen == gen) {
        // rebuild failed: the running mix stays, a waiting one may start
        o.alt = None;
    }
}

/// One output block of `n` frames at time `now`: advance the clock, pass sound (or silence) to `put`,
/// remember levels and first sound. Shared by the device and the output without device.
/// `gain` = playback volume; applied only after the levels (they show the project level).
fn render_block(sh: &Shared, pk: &[AtomicU32; 2], rate: u32, n: usize, now: Instant, gain: f32, mut put: impl FnMut(f32, f32)) {
    let mut o = sh.out.lock().unwrap();
    let target = match o.phase {
        Phase::Anchored(at) => (now.saturating_duration_since(at).as_secs_f64() * rate as f64) as u64,
        _ => 0,
    };
    let (mut pl, mut pr) = (0.0f32, 0.0f32);
    let step = 1000.0 / (rate.max(1) as f32 * GAIN_RAMP_MS);
    let mut g = o.gain;
    let real = o.render(n, target, |l, r| {
        pl = pl.max(l.abs());
        pr = pr.max(r.abs());
        g += (gain - g).clamp(-step, step);
        put(l * g, r * g);
    });
    o.gain = g;
    let running = o.phase == Phase::Running;
    sh.played.store(o.played, Ordering::Relaxed);
    drop(o);
    if running && real > 0 {
        sh.first_sound.get_or_init(|| now);
    }
    sh.cb_ns.store(now.duration_since(sh.base).as_nanos() as u64, Ordering::Relaxed);
    sh.cb_frames.store(real, Ordering::Relaxed);
    sh.running.store(running, Ordering::Release);
    pk[0].fetch_max(pl.to_bits(), Ordering::Relaxed);
    pk[1].fetch_max(pr.to_bits(), Ordering::Relaxed);
}

impl AudioPlayer {
    /// Plays from `from` as soon as sound is there (scrubbing, preview).
    pub fn start(project: &Project, from: Time) -> Result<Self, String> {
        Self::open(Cow::Borrowed(project), from, false, false, 1.0)
    }

    /// For playback with picture: sound is prepared but only comes in after [`Self::anchor`] –
    /// then at the place where the clock stands since the anchor. An own clone (e.g. with solo)
    /// goes to the feed without another copy.
    /// `tempo`: preview speed (1 = normal); the sound keeps its pitch.
    pub fn start_synced(project: Cow<'_, Project>, from: Time, tempo: f64) -> Result<Self, String> {
        Self::open(project, from, true, false, tempo)
    }

    /// Like [`Self::start_synced`], but without device: a thread consumes the samples at the pace of
    /// system time (measurement test without window or audible sound; clock, dropouts and windows as real).
    #[cfg(test)]
    pub fn start_null(project: &Project, from: Time) -> Result<Self, String> {
        Self::open(Cow::Borrowed(project), from, true, true, 1.0)
    }

    fn open(project: Cow<'_, Project>, from: Time, synced: bool, null: bool, tempo: f64) -> Result<Self, String> {
        let (device, config) = if null {
            (None, None)
        } else {
            let host = cpal::default_host();
            // chosen device (from memory, otherwise enumerate); if it is not there (any more), the system default
            let wanted = DEVICE.lock().unwrap().clone();
            let mut out = OUTPUT.lock().unwrap();
            let hit = wanted.as_ref().and_then(|w| out.as_ref().filter(|(n, _)| n == w).map(|(_, v)| v.clone()));
            let (device, config) = match hit {
                Some(v) => v,
                None => {
                    let device = wanted
                        .and_then(|w| host.output_devices().ok()?.find(|d| d.name().ok().as_deref() == Some(w.as_str())))
                        .or_else(|| host.default_output_device())
                        .ok_or("No audio output device")?;
                    let name = device.name().unwrap_or_default();
                    remembered(&mut out, &name, || device.default_output_config().map(|c| (device.clone(), c)).map_err(|e| e.to_string()))?
                }
            };
            (Some(device), Some(config))
        };
        let rate = config.as_ref().map_or(48_000, |c| c.sample_rate().0);
        let channels = config.as_ref().map_or(2, |c| c.channels() as usize);

        let tempo = tempo.clamp(0.25, 4.0);
        // shortcut: rounded to whole frames (exact for the offered speeds at 44.1 and 48 kHz), upgrade if odd device rates drift audibly
        let tl_rate = (rate as f64 / tempo).round() as u32;
        let end = project.duration();
        let plan = window_plan(from, end, tl_rate, 0).ok_or_else(|| cl_export::ExportError::Empty.to_string())?;
        let shared = Arc::new(Shared {
            out: Mutex::new(Out::new(if synced { Phase::Waiting } else { Phase::Free }, rate)),
            played: AtomicU64::new(0),
            cb_ns: AtomicU64::new(0),
            cb_frames: AtomicU64::new(0),
            running: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            base: Instant::now(),
            stop: AtomicBool::new(false),
            kids: Mutex::new(Vec::new()),
            first_sound: OnceLock::new(),
            dev_rate: rate,
            tempo,
        });
        let first = open_window(&project, plan, tl_rate, &shared)?;
        {
            let (p, sh) = (project.into_owned(), shared.clone());
            std::thread::Builder::new()
                .name("cl-audio-feed".into())
                .spawn(move || feed(p, from, tl_rate, sh, first, 0))
                .map_err(|e| e.to_string())?;
        }

        let peaks: Arc<[AtomicU32; 2]> = Arc::new([AtomicU32::new(0), AtomicU32::new(0)]);
        // Opening the output (WASAPI) takes several hundred milliseconds: in its own thread
        // so the UI does not freeze on play. The cpal stream is not Send and therefore
        // lives entirely there; it ends as soon as `ctl` is dropped.
        let (ctl, ctl_rx) = std::sync::mpsc::channel::<bool>();
        let failed = Arc::new(AtomicBool::new(false));
        let (sh, pk, fail) = (shared.clone(), peaks.clone(), failed.clone());
        let (Some(device), Some(config)) = (device, config) else {
            // without device: every 10 ms consume the frames due since
            std::thread::Builder::new()
                .name("cl-audio-null".into())
                .spawn(move || {
                    let (mut last, mut playing) = (Instant::now(), true);
                    loop {
                        match ctl_rx.try_recv() {
                            Ok(p) => playing = p,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        }
                        std::thread::sleep(Duration::from_millis(10));
                        let now = Instant::now();
                        let n = (now.duration_since(last).as_secs_f64() * rate as f64) as usize;
                        last += Duration::from_secs_f64(n as f64 / rate as f64);
                        if playing && n > 0 {
                            render_block(&sh, &pk, rate, n, now, playback_gain(), |_, _| {});
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            return Ok(Self { ctl, failed, shared, peaks, rate, tl_rate, start: from, queued: None, remix_t0: None });
        };
        let out = move || -> Result<(), String> {
            let err = |e| log::warn!("Audio: {e}");
            let cfg: cpal::StreamConfig = config.clone().into();
            macro_rules! build {
                ($t:ty, $conv:expr) => {{
                    let sh = sh.clone();
                    device.build_output_stream(
                        &cfg,
                        move |data: &mut [$t], _| {
                            let n = data.len() / channels.max(1);
                            let mut frames = data.chunks_mut(channels.max(1));
                            render_block(&sh, &pk, rate, n, Instant::now(), playback_gain(), |l, r| {
                                if let Some(frame) = frames.next() {
                                    for (i, s) in frame.iter_mut().enumerate() {
                                        *s = $conv(match i {
                                            0 => l,
                                            1 => r,
                                            _ => 0.0,
                                        });
                                    }
                                }
                            });
                        },
                        err,
                        None,
                    )
                }};
            }
            let stream = match config.sample_format() {
                cpal::SampleFormat::F32 => build!(f32, |v: f32| v),
                cpal::SampleFormat::I16 => {
                    build!(i16, |v: f32| (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                }
                cpal::SampleFormat::U16 => build!(u16, |v: f32| ((v.clamp(-1.0, 1.0) * 0.5 + 0.5)
                    * u16::MAX as f32)
                    as u16),
                f => return Err(format!("Sample format {f:?} not supported")),
            }
            .map_err(|e| e.to_string())?;
            stream.play().map_err(|e| e.to_string())?;
            while let Ok(play) = ctl_rx.recv() {
                let _ = if play { stream.play().is_ok() } else { stream.pause().is_ok() };
            }
            // Abort: fade out briefly instead of cutting hard (no click)
            {
                let mut o = sh.out.lock().unwrap();
                if o.phase == Phase::Running && !sh.paused.load(Ordering::Relaxed) {
                    o.closing = Some(o.fade_len);
                } else {
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_millis(FADE_MS * 2 + 10));
            Ok(())
        };
        std::thread::Builder::new()
            .name("cl-audio-out".into())
            .spawn(move || {
                if let Err(e) = out() {
                    log::warn!("Audio: {e}");
                    fail.store(true, Ordering::Relaxed);
                    // Device unplugged or similar: search again on the next start
                    *OUTPUT.lock().unwrap() = None;
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { ctl, failed, shared, peaks, rate, tl_rate, start: from, queued: None, remix_t0: None })
    }

    /// Release the clock: time runs from `at`; sound comes in at the then current place.
    pub fn anchor(&self, at: Instant) {
        let mut o = self.shared.out.lock().unwrap();
        if o.phase == Phase::Waiting {
            o.phase = Phase::Anchored(at);
        }
    }

    /// Audible change during playback: mix `project` anew beside the running sound; it takes over
    /// with a crossfade at the frame the output has reached then (no gap, clock goes on). At most one
    /// rebuild in flight; meanwhile only the newest state waits ([`Self::poll`]).
    pub fn remix(&mut self, project: Project) {
        self.queued = Some(project);
        self.poll();
    }

    /// Start the waiting rebuild once none is in flight (called every tick).
    pub fn poll(&mut self) {
        if self.remixing() {
            return;
        }
        if let Some(t0) = self.remix_t0.take() {
            if crate::app::perf() {
                crate::applog::perf_line!("perf: sound remixed, taken over after {:.0} ms (without gap)", t0.elapsed().as_secs_f64() * 1000.0);
            }
        }
        let Some(p) = self.queued.take() else { return };
        let at = self.shared.out.lock().unwrap().played;
        // `at` frames after `start` (sub-microsecond rounding, far below one sample)
        let from = self.start + Time((at as u128 * 1_000_000 / self.tl_rate.max(1) as u128) as i64);
        let Some(plan) = window_plan(from, p.duration(), self.tl_rate, 0) else { return };
        let first = match open_window(&p, plan, self.tl_rate, &self.shared) {
            Ok(w) => w,
            Err(e) => return log::warn!("Audio: {e}"),
        };
        let gen = {
            let mut o = self.shared.out.lock().unwrap();
            let gen = o.gen + 1;
            o.alt = Some(Alt { gen, at, buf: VecDeque::new() });
            gen
        };
        let (sh, rate) = (self.shared.clone(), self.tl_rate);
        let spawned = std::thread::Builder::new().name("cl-audio-feed".into()).spawn(move || feed(p, from, rate, sh, first, gen));
        match spawned {
            Ok(_) => self.remix_t0 = Some(Instant::now()),
            Err(e) => {
                self.shared.out.lock().unwrap().alt = None;
                log::warn!("Audio: {e}");
            }
        }
    }

    /// A rebuilt mix is on its way and not playing yet
    pub fn remixing(&self) -> bool {
        self.shared.out.lock().unwrap().alt.is_some()
    }

    /// Pause or resume output (buffer and clock are kept → continue at once)
    pub fn pause(&self) -> bool {
        self.shared.paused.store(true, Ordering::Relaxed);
        !self.failed() && self.ctl.send(false).is_ok()
    }
    pub fn resume(&self) -> bool {
        self.shared.paused.store(false, Ordering::Relaxed);
        !self.failed() && self.ctl.send(true).is_ok()
    }

    /// Output could not be opened (no more sound will come)
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Current playback position by audio clock (smooth between output callbacks).
    pub fn position(&self) -> Time {
        let sh = &self.shared;
        let played = sh.played.load(Ordering::Relaxed);
        let frames = if sh.running.load(Ordering::Acquire) {
            let since = sh.base.elapsed().saturating_sub(Duration::from_nanos(sh.cb_ns.load(Ordering::Relaxed)));
            clock_frames(played, since, sh.cb_frames.load(Ordering::Relaxed), self.rate)
        } else {
            played
        };
        self.start + Time::from_secs_f64(frames as f64 / self.tl_rate as f64)
    }

    /// Peak level (0…1+) left/right since the last call
    pub fn take_peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.peaks[0].swap(0, Ordering::Relaxed)),
            f32::from_bits(self.peaks[1].swap(0, Ordering::Relaxed)),
        )
    }

    /// Already outputs sound (before that the picture runs on system time or stands still in preview).
    pub fn started(&self) -> bool {
        self.shared.running.load(Ordering::Acquire)
    }

    /// Sound has ended (project end or FFmpeg aborted): the clock stops.
    pub fn finished(&self) -> bool {
        let o = self.shared.out.lock().unwrap();
        o.finished && o.buf.is_empty()
    }

    /// Sound is ready (first window delivered samples or the feed is finished)
    pub fn ready(&self) -> bool {
        let o = self.shared.out.lock().unwrap();
        o.buf.len() >= 2 || o.finished
    }

    /// Time of the first audible sample
    pub fn first_sound(&self) -> Option<Instant> {
        self.shared.first_sound.get().copied()
    }

    /// Dropouts (buffer empty while sound is still coming) since the last call: (count, duration)
    pub fn take_underruns(&self) -> (u64, Duration) {
        let mut o = self.shared.out.lock().unwrap();
        let r = (o.underruns, Duration::from_secs_f64(o.underrun_frames as f64 / self.rate as f64));
        o.underruns = 0;
        o.underrun_frames = 0;
        r
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for k in self.shared.kids.lock().unwrap().iter() {
            if let Ok(mut c) = k.lock() {
                let _ = c.kill();
            }
        }
        // Output thread ends with `ctl` (after the fade-out), reader threads after the kill
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000;

    /// Review finding: every sound start enumerated the devices and fetched the format again.
    /// Now only the first time per device (and after a change or forgetting).
    #[test]
    fn output_format_is_fetched_once_per_device() {
        let mut slot = None;
        let mut calls = 0;
        let mut get = |slot: &mut Option<(String, u32)>, name: &str, v: u32| {
            remembered(slot, name, || {
                calls += 1;
                Ok::<_, ()>(v)
            })
        };
        assert_eq!(get(&mut slot, "A", 48_000), Ok(48_000));
        assert_eq!(get(&mut slot, "A", 1), Ok(48_000), "from memory");
        assert_eq!(get(&mut slot, "B", 44_100), Ok(44_100), "other device: new");
        slot = None; // device changed or output failed
        assert_eq!(get(&mut slot, "B", 96_000), Ok(96_000));
        assert_eq!(calls, 3);
        // errors are not remembered
        let mut s2: Option<(String, u32)> = None;
        assert_eq!(remembered(&mut s2, "C", || Err::<u32, _>("gone")), Err("gone"));
        assert!(s2.is_none());
    }

    #[test]
    fn windows_tile_the_range_without_gap_or_overlap() {
        for rate in [44_100u32, 48_000, 96_000, 22_050] {
            for (from, end) in [(0, 65 * S), (1_234_567, 61 * S + 7), (19 * S, 20 * S), (0, 3), (0, 40 * S)] {
                let (from, end) = (Time(from), Time(end));
                let mut next = 0;
                let mut k = 0;
                while let Some(w) = window_plan(from, end, rate, k) {
                    assert_eq!(w.first, next, "gapless: {rate} {k}");
                    assert!(w.frames > 0 && w.frames <= WINDOW_SECS * rate as u64);
                    assert!(k > 0 || w.frames <= FIRST_SECS * rate as u64, "first window short");
                    // window start lies on the playback grid: frame `first` from `from`
                    let start = w.mix_from.0 as i128 * rate as i128 + w.skip as i128 * 1_000_000;
                    let want = from.0 as i128 * rate as i128 + w.first as i128 * 1_000_000;
                    assert!((start - want).abs() <= 1_000_000, "grid {rate} {k}: {start} {want}");
                    if from.0 > PRE_MS as i64 * 1000 || k > 0 {
                        assert_eq!(start, want, "exactly on the grid");
                        assert_eq!(w.skip, rate as u64 * PRE_MS / 1000, "settling");
                    }
                    assert!(w.mix_to >= from + Time((w.first + w.frames) as i64 * S / rate as i64));
                    next += w.frames;
                    k += 1;
                }
                assert_eq!(next, total_frames(from, end, rate), "all covered");
            }
        }
        assert_eq!(window_plan(Time(5), Time(5), 48_000, 0), None);
    }

    #[test]
    fn window_bounds_for_typical_start() {
        let w0 = window_plan(Time(90 * S), Time(200 * S), 48_000, 0).unwrap();
        assert_eq!((w0.first, w0.frames), (0, 6 * 48_000));
        assert_eq!((w0.mix_from, w0.mix_to), (Time(90 * S - 200_000), Time(96 * S)));
        let w = window_plan(Time(90 * S), Time(200 * S), 48_000, 1).unwrap();
        assert_eq!(w.first, 6 * 48_000);
        assert_eq!(w.mix_from, Time(96 * S - 200_000));
        assert_eq!(w.skip, 9600);
        assert_eq!(w.frames, 20 * 48_000);
        assert_eq!(w.mix_to, Time(116 * S));
        // last window ends at the project end: 90+6+5×20 = 196 … 200
        let last = window_plan(Time(90 * S), Time(200 * S), 48_000, 6).unwrap();
        assert_eq!(last.frames, 4 * 48_000);
        assert_eq!(last.mix_to, Time(200 * S));
        assert_eq!(window_plan(Time(90 * S), Time(200 * S), 48_000, 7), None);
        assert_eq!(window_plan(Time(0), Time(200 * S), 48_000, u64::MAX), None);
    }

    #[test]
    fn next_window_is_prepared_lead_seconds_before_the_end() {
        let r = 48_000;
        let f = WINDOW_SECS * r as u64;
        assert!(!prepare_next(0, f, r));
        assert!(!prepare_next(f - LEAD_SECS * r as u64 - 1, f, r));
        assert!(prepare_next(f - LEAD_SECS * r as u64, f, r));
        // short (first or last) window: at once
        assert!(prepare_next(0, 3 * r as u64, r));
    }

    fn feed_ramp(o: &mut Out, from: usize, n: usize) {
        for i in from..from + n {
            o.buf.extend([i as f32, -(i as f32)]);
        }
    }

    fn take(o: &mut Out, n: usize, target: u64) -> Vec<(f32, f32)> {
        let mut v = Vec::new();
        o.render(n, target, |l, r| v.push((l, r)));
        v
    }

    #[test]
    fn anchored_output_skips_to_the_clock_and_fades_in() {
        let mut o = Out::new(Phase::Waiting, 1000); // 1000 Hz: 15 frames fade-in
        feed_ramp(&mut o, 0, 100);
        // before the anchor: silence, nothing consumed (picture still buffering)
        assert!(take(&mut o, 10, 0).iter().all(|s| *s == (0.0, 0.0)));
        assert_eq!(o.played, 0);
        o.phase = Phase::Anchored(Instant::now());
        // clock already at 40 frames: the first 40 are skipped, sound comes in at 40
        let v = take(&mut o, 30, 40);
        assert_eq!(o.phase, Phase::Running);
        assert_eq!(o.played, 70, "clock = skipped + played frames, no jump");
        assert_eq!(v[0], (0.0, -0.0), "fade-in starts at 0");
        assert!((v[1].0 - 41.0 / 15.0).abs() < 1e-4, "ramp: {:?}", v[1]);
        assert_eq!(v[20], (60.0, -60.0), "unchanged after the fade-in");
    }

    #[test]
    fn anchored_waits_while_audio_is_behind_the_clock() {
        let mut o = Out::new(Phase::Waiting, 1000);
        o.phase = Phase::Anchored(Instant::now());
        feed_ramp(&mut o, 0, 20);
        // clock at 50, only 20 frames there: discard all, keep silence
        assert!(take(&mut o, 10, 50).iter().all(|s| *s == (0.0, 0.0)));
        assert_eq!(o.played, 20);
        assert!(matches!(o.phase, Phase::Anchored(_)));
        feed_ramp(&mut o, 20, 100);
        let v = take(&mut o, 40, 60);
        assert_eq!(o.played, 100);
        assert_eq!(v[30], (90.0, -90.0), "comes in at frame 60");
        assert_eq!(o.underruns, 0, "no dropout before coming in");
    }

    #[test]
    fn underruns_are_counted_once_per_gap_but_not_at_the_end() {
        let mut o = Out::new(Phase::Free, 1000);
        feed_ramp(&mut o, 0, 20);
        take(&mut o, 30, 0);
        assert_eq!((o.underruns, o.underrun_frames), (1, 10));
        take(&mut o, 5, 0);
        assert_eq!((o.underruns, o.underrun_frames), (1, 15));
        feed_ramp(&mut o, 20, 5);
        take(&mut o, 10, 0);
        assert_eq!(o.underruns, 2);
        o.finished = true;
        take(&mut o, 10, 0);
        assert_eq!(o.underruns, 2, "the end is no dropout");
    }

    #[test]
    fn closing_fades_out_then_silence() {
        let mut o = Out::new(Phase::Running, 1000);
        feed_ramp(&mut o, 100, 40);
        o.closing = Some(o.fade_len);
        let v = take(&mut o, 30, 0);
        assert_eq!(v[0].0, 100.0);
        assert!(v[7].0 < 100.0 && v[7].0 > 0.0);
        assert!(v[15..].iter().all(|s| s.0 == 0.0));
    }

    /// Rebuilt mix during playback (audible change): the old mix plays on until the new one holds
    /// the current frame, then a crossfade – not one silent sample, the frame count runs on without a jump.
    #[test]
    fn rebuilt_mix_takes_over_without_silence_at_the_reached_frame() {
        let mut o = Out::new(Phase::Running, 1000); // 30 frames crossfade
        feed_ramp(&mut o, 1, 400); // old mix: frame i = i (never 0)
        let mut v = take(&mut o, 100, 0);
        // remix requested at frame 100; FFmpeg needs a while, the old mix keeps playing
        o.alt = Some(Alt { gen: 1, at: o.played, buf: VecDeque::new() });
        for _ in 0..6 {
            v.extend(take(&mut o, 10, 0));
        }
        assert_eq!(o.gen, 0, "no switch without new samples");
        // the new mix arrives from frame 100 on (frame i = 10000 + i), the output is at 160 by now
        let alt = o.alt.as_mut().unwrap();
        for i in 101..400 {
            alt.buf.extend([10_000.0 + i as f32, 0.0]);
        }
        for _ in 0..10 {
            v.extend(take(&mut o, 10, 0));
        }
        assert_eq!(o.gen, 1, "switched");
        assert!(o.alt.is_none() && o.slot(0).is_none(), "old feed is outdated and stops");
        assert_eq!(o.played, 260);
        assert_eq!(v.iter().filter(|s| s.0 == 0.0).count(), 0, "silent samples around the rebuild");
        // up to frame 160 the old mix, then crossfade, from 190 only the new mix – same frame numbers
        assert_eq!(v[159].0, 160.0);
        assert!(v[165].0 > 166.0 && v[165].0 < 10_166.0, "crossfade: {}", v[165].0);
        assert!(v[161..190].windows(2).all(|w| w[1].0 > w[0].0), "crossfade rises smoothly");
        assert_eq!(v[190].0, 10_191.0);
        assert_eq!(v[259].0, 10_260.0, "frame position continuous after the switch");
        assert_eq!(o.underruns, 0);
    }

    /// Stopping while a rebuild is pending: fade out to silence, the late new mix never comes in.
    #[test]
    fn stop_during_pending_rebuild_stays_silent() {
        let mut o = Out::new(Phase::Running, 1000);
        feed_ramp(&mut o, 1, 400);
        take(&mut o, 50, 0);
        o.alt = Some(Alt { gen: 1, at: o.played, buf: VecDeque::new() });
        o.closing = Some(o.fade_len);
        o.alt.as_mut().unwrap().buf.extend(std::iter::repeat_n(5.0, 600));
        let v = take(&mut o, 100, 0);
        assert_eq!(o.gen, 0, "no switch after stop");
        assert!(v[o.fade_len as usize..].iter().all(|s| s.0 == 0.0), "silence after the fade-out");
    }

    /// The alternative feed fills only its own buffer; a later rebuild makes it outdated.
    #[test]
    fn feed_slots_follow_the_generation() {
        let mut o = Out::new(Phase::Running, 1000);
        assert!(o.slot(0).is_some() && o.slot(1).is_none());
        o.alt = Some(Alt { gen: 1, at: 0, buf: VecDeque::new() });
        o.slot(1).unwrap().push_back(1.0);
        assert_eq!((o.buf.len(), o.alt.as_ref().unwrap().buf.len()), (0, 1));
        assert!(o.slot(2).is_none());
    }

    #[test]
    fn clock_is_smooth_between_callbacks_and_stops_in_a_gap() {
        let r = 48_000;
        // `played` = 1480 after a callback of 480 frames
        assert_eq!(clock_frames(1480, Duration::ZERO, 480, r), 1000);
        assert_eq!(clock_frames(1480, Duration::from_millis(5), 480, r), 1240);
        // at most up to the end of what was output (also while paused)
        assert_eq!(clock_frames(1480, Duration::from_millis(50), 480, r), 1480);
        assert_eq!(clock_frames(1000, Duration::from_millis(50), 0, r), 1000);
        // next callback joins seamlessly
        assert_eq!(clock_frames(1960, Duration::ZERO, 480, r), 1480);
    }

    fn test_shared(rate: u32) -> Arc<Shared> {
        Arc::new(Shared {
            out: Mutex::new(Out::new(Phase::Free, rate)),
            played: AtomicU64::new(0),
            cb_ns: AtomicU64::new(0),
            cb_frames: AtomicU64::new(0),
            running: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            base: Instant::now(),
            stop: AtomicBool::new(false),
            kids: Mutex::new(Vec::new()),
            first_sound: OnceLock::new(),
            dev_rate: rate,
            tempo: 1.0,
        })
    }

    #[test]
    fn volume_curve() {
        let g = |p: f32| Volume { percent: p, muted: false }.gain();
        assert_eq!(g(0.0), 0.0);
        assert_eq!(g(100.0), 1.0);
        assert!((g(50.0) - 0.125).abs() < 1e-6, "cubic");
        let mut last = -1.0;
        for p in (0..=100).step_by(5) {
            assert!(g(p as f32) > last, "monotonic at {p}");
            last = g(p as f32);
        }
        // never louder than the project, even with broken values
        for p in [150.0, f32::INFINITY, -20.0, f32::NAN] {
            assert!((0.0..=1.0).contains(&g(p)), "{p}");
        }
        assert_eq!(Volume { percent: 80.0, muted: true }.gain(), 0.0);
        let v = |percent, muted| Volume { percent, muted };
        assert_eq!(v(100.0, false).step(true), v(100.0, false));
        assert_eq!(v(37.0, true).step(true), v(40.0, false));
        assert_eq!(v(37.0, true).step(false), v(30.0, true));
        assert_eq!(v(0.0, false).step(false), v(0.0, false));
    }

    /// Playback volume acts at the output (device and no device share `render_block`),
    /// glides without a jump and leaves the level meter untouched.
    #[test]
    fn playback_volume_reaches_the_output_but_not_the_meter() {
        let rate = 1000; // 5 frames ramp
        let run = |start: f32, gain: f32| {
            let sh = test_shared(rate);
            let pk = [AtomicU32::new(0), AtomicU32::new(0)];
            {
                let mut o = sh.out.lock().unwrap();
                o.phase = Phase::Running;
                o.gain = start;
                o.buf.extend(std::iter::repeat_n(0.5, 200));
            }
            let mut v = Vec::new();
            render_block(&sh, &pk, rate, 100, Instant::now(), gain, |l, r| v.push((l, r)));
            (v, f32::from_bits(pk[0].load(Ordering::Relaxed)))
        };
        // 100 %: samples unchanged
        let (v, peak) = run(1.0, 1.0);
        assert!(v.iter().all(|s| *s == (0.5, 0.5)));
        assert_eq!(peak, 0.5);
        // muted/0 %: silence after the ramp, ramp without a jump, levels still from the project
        let (v, peak) = run(1.0, Volume { percent: 100.0, muted: true }.gain());
        assert!(v[10..].iter().all(|s| *s == (0.0, 0.0)));
        assert!(v[0].0 > 0.3 && v[0].0 < 0.5, "glides: {:?}", v[0]);
        assert!(v.windows(2).all(|w| w[0].0 - w[1].0 <= 0.1 + 1e-6), "no jump");
        assert_eq!(peak, 0.5, "level before the slider");
        // on again: glides up
        let (v, _) = run(0.0, 1.0);
        assert!(v[0].0 > 0.0 && v[0].0 < 0.2);
        assert!(v[10..].iter().all(|s| *s == (0.5, 0.5)));
    }

    /// Playback sound as in the app (window by window through the feed, without output device)
    fn windowed(p: &Project, from: Time, rate: u32) -> Vec<f32> {
        let shared = test_shared(rate);
        let first = open_window(p, window_plan(from, p.duration(), rate, 0).unwrap(), rate, &shared).unwrap();
        let sh = shared.clone();
        let pp = p.clone();
        let t = std::thread::spawn(move || feed(pp, from, rate, sh, first, 0));
        let mut all = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        loop {
            let mut o = shared.out.lock().unwrap();
            all.extend(o.buf.drain(..));
            if o.finished && o.buf.is_empty() {
                break;
            }
            drop(o);
            assert!(std::time::Instant::now() < deadline, "feed not finished after 120 s ({} samples so far)", all.len());
            std::thread::sleep(Duration::from_millis(2));
        }
        t.join().unwrap();
        all
    }

    /// Old way: one mix from the start point to the project end
    fn whole(p: &Project, from: Time, rate: u32) -> Vec<f32> {
        let (mut child, _wd) = cl_export::spawn_audio_preview(p, from, rate).unwrap();
        let mut buf = Vec::new();
        child.stdout.as_mut().unwrap().read_to_end(&mut buf).unwrap();
        child.wait().unwrap();
        buf.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    }

    /// Whole chain with real FFmpeg (without output device): window by window into the buffer.
    #[test]
    fn feeder_delivers_exact_frame_count_across_windows() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = test_project(dir.path(), 26.0);
        let (rate, from) = (8000, Time(3 * S));
        assert_eq!(windowed(&p, from, rate).len() as u64, 2 * total_frames(from, p.duration(), rate));
    }

    /// Project with everything that shapes the sound: cuts of the same file, fades, volume keyframes,
    /// speed ≠ 1, reverse, a clip across the window boundary (with start 1.5 s it lies at 21.5 s).
    fn rich_project(dir: &std::path::Path) -> Project {
        use cl_core::*;
        let tone = |name: &str, src: &str| {
            let path = dir.join(name);
            assert!(cl_media::ffmpeg_command()
                .unwrap()
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", src, "-ac", "2"])
                .arg(&path)
                .status()
                .unwrap()
                .success());
            cl_media::probe(&path).unwrap()
        };
        // pitch changes constantly: any offset by one sample would show
        let a = tone("a.wav", "aevalsrc=0.5*sin(2*PI*(200+20*t)*t):s=48000:d=40");
        let b = tone("b.wav", "sine=f=523:d=40:sample_rate=44100");
        let mut p = Project::new("window");
        p.media.extend([a.clone(), b.clone()]);
        let clip = |m: &MediaRef, start: f64, dur: f64, src: f64| {
            let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Audio }, Time::from_secs_f64(start), Time::from_secs_f64(dur));
            c.source_in = Time::from_secs_f64(src);
            c
        };
        let mut t1 = Track::new(TrackKind::Audio);
        let mut c = clip(&a, 0.0, 6.0, 1.0);
        c.fade_in = Time::from_secs_f64(0.5);
        c.fade_out = Time::from_secs_f64(1.0);
        t1.clips.push(c);
        let mut c = clip(&a, 6.0, 9.0, 20.0);
        c.speed = 1.5;
        t1.clips.push(c);
        // across both window boundaries, with volume keyframes and fade-out
        let mut c = clip(&a, 15.0, 15.0, 3.0);
        c.keyframes.set(Prop::Volume, Time::ZERO, -12.0);
        c.keyframes.set(Prop::Volume, Time::from_secs_f64(8.0), 3.0);
        c.fade_out = Time::from_secs_f64(2.0);
        t1.clips.push(c);
        let mut t2 = Track::new(TrackKind::Audio);
        let mut c = clip(&b, 4.0, 5.0, 0.0);
        c.speed = 0.75;
        c.volume_db = -6.0;
        t2.clips.push(c);
        let mut c = clip(&b, 18.0, 8.0, 2.0);
        c.fx.reverse = true;
        c.fade_in = Time::from_secs_f64(0.3);
        t2.clips.push(c);
        p.timeline.tracks.extend([t1, t2]);
        p
    }

    /// Window mix over two windows = old full mix of the same section.
    #[test]
    fn windowed_mix_matches_whole_mix() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = rich_project(dir.path());
        assert!(p.duration() > Time((FIRST_SECS + WINDOW_SECS + 2) as i64 * S), "three windows");
        for (rate, from, tol) in [(48_000, Time(1_500_000), 1e-6f32), (44_100, Time(1_500_000), 2e-3), (48_000, Time(100_000), 1e-6)] {
            let w = windowed(&p, from, rate);
            let full = whole(&p, from, rate);
            let n = total_frames(from, p.duration(), rate) as usize * 2;
            assert_eq!(w.len(), n, "{rate}: exact length");
            assert!(full.len() + 2 * rate as usize / 100 >= n, "{rate}: {} {n}", full.len());
            let cmp = full.len().min(n);
            let (i, d) = (0..cmp).map(|i| (i, (w[i] - full[i]).abs())).fold((0, 0.0f32), |a, b| if b.1 > a.1 { b } else { a });
            let shifted = (0..cmp - 2).map(|i| (w[i] - full[i + 2]).abs()).fold(0.0f32, f32::max);
            assert!(shifted > 0.1, "test detects a one-sample offset: {shifted}");
            assert!(d <= tol, "{rate} from {from:?}: largest deviation {d} at {:.4} s", i as f64 / 2.0 / rate as f64);
            // no jump at the seams: the step there is no larger than elsewhere
            for s in [FIRST_SECS, FIRST_SECS + WINDOW_SECS] {
                let seam = s as usize * rate as usize * 2;
                let step = |i: usize| (w[i] - w[i - 2]).abs();
                let usual = (seam - 200..seam - 2).map(step).fold(0.0f32, f32::max);
                assert!(step(seam) <= usual * 1.5 + 1e-4, "{rate}: seam {} / elsewhere {usual}", step(seam));
            }
        }
    }

    /// Time to the first audio sample at the start point (call + FFmpeg + settling)
    fn first_sample_ms(spawn: impl FnOnce() -> (Child, tempfile::TempDir), skip_frames: u64) -> f64 {
        let t0 = Instant::now();
        let (mut child, _wd) = spawn();
        let mut out = child.stdout.take().unwrap();
        let mut need = skip_frames as usize * 8 + 8;
        let mut buf = vec![0u8; 64 * 1024];
        while need > 0 {
            match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => need = need.saturating_sub(n),
            }
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let _ = child.kill();
        let _ = child.wait();
        ms
    }

    /// Manual measurement: `CUTLINE_QA_DIR=<folder with PL_cut.cutline> cargo test --release -p cl-app
    /// -- --ignored first_audio_sample --nocapture`. Old way (full mix to project end) against windows.
    #[test]
    #[ignore]
    fn first_audio_sample_pl_cut() {
        let Some(dir) = std::env::var_os("CUTLINE_QA_DIR") else { return };
        let mut ok = false;
        for name in ["PL_cut.cutline", "PL_large.cutline"] {
            let Ok(json) = std::fs::read_to_string(std::path::Path::new(&dir).join(name)) else { continue };
            let p = Project::from_json(&json).unwrap();
            if p.media.iter().any(|m| m.is_local() && !m.path.exists()) {
                eprintln!("{name}: media missing, skipped");
                continue;
            }
            ok = true;
            let rate = 48_000;
            for secs in [3.0, 37.0, 128.0, 335.0, 571.0] {
                let from = Time::from_secs_f64(secs);
                let plan = window_plan(from, p.duration(), rate, 0).unwrap();
                let n_old = cl_export::audible_clips(&p, from, p.duration()).count();
                let n_new = cl_export::audible_clips(&p, plan.mix_from, plan.mix_to).count();
                // twice each, the second value counts (files in cache as in the app)
                let mut old = 0.0;
                let mut new = 0.0;
                for _ in 0..2 {
                    old = first_sample_ms(|| cl_export::spawn_audio_preview(&p, from, rate).unwrap(), 0);
                    new = first_sample_ms(|| cl_export::spawn_audio_window(&p, plan.mix_from, plan.mix_to, rate).unwrap(), plan.skip);
                }
                eprintln!("{name} from {secs:>5.0} s: so far {old:>6.0} ms ({n_old} inputs) · window {new:>5.0} ms ({n_new} inputs)");
            }
        }
        if !ok {
            eprintln!("no test projects in CUTLINE_QA_DIR");
        }
    }

    /// Manual measurement with the real output device (plays only silence): start → first sample at the
    /// device (opening the output + FFmpeg + feed), started synchronized as on playback.
    /// `cargo test --release -p cl-app -- --ignored output_start_latency --nocapture`
    #[test]
    #[ignore]
    fn output_start_latency() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = test_project(dir.path(), 30.0);
        p.timeline.tracks[0].muted = true; // silence from own source
        for i in 0..5 {
            let t0 = Instant::now();
            let a = AudioPlayer::start_synced(Cow::Borrowed(&p), Time(S), 1.0).unwrap();
            let opened = t0.elapsed();
            a.anchor(t0);
            while a.first_sound().is_none() && t0.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(1));
            }
            let first = a.first_sound().map(|t| t.duration_since(t0));
            // clock after the handover = time since the anchor (no jump)
            let diff = a.position().as_secs_f64() - 1.0 - t0.elapsed().as_secs_f64();
            eprintln!("run {i}: call {:.0} ms, first sample {:?} ms, clock − system time {:+.1} ms", opened.as_secs_f64() * 1e3, first.map(|d| (d.as_secs_f64() * 1e3).round()), diff * 1e3);
            assert!(diff.abs() < 0.005, "handover without a jump: {diff}");
        }
    }

    /// Preview speed: FFmpeg delivers half the frames at double speed with the pitch kept (300 Hz stay
    /// 300 Hz), and the audio clock runs twice as fast as the system time.
    #[test]
    fn tempo_halves_the_frames_keeps_the_pitch_and_scales_the_clock() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = test_project(dir.path(), 30.0);
        let (mut child, _wd) = cl_export::spawn_audio_window_tempo(&p, Time(S), Time(5 * S), 48_000, 2.0).unwrap();
        let mut buf = Vec::new();
        child.stdout.as_mut().unwrap().read_to_end(&mut buf).unwrap();
        child.wait().unwrap();
        let left: Vec<f32> = buf.chunks_exact(8).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        assert!((left.len() as f64 / 96_000.0 - 1.0).abs() < 0.02, "4 s at double speed = 2 s: {} frames", left.len());
        // middle second: 300 Hz have 600 zero crossings
        let mid = &left[24_000..72_000];
        let crossings = mid.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        assert!((560..=640).contains(&crossings), "pitch kept: {crossings} zero crossings per second");

        let a = AudioPlayer::open(Cow::Borrowed(&p), Time(S), true, true, 2.0).unwrap();
        let t0 = Instant::now();
        a.anchor(t0);
        while a.first_sound().is_none() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(a.first_sound().is_some());
        std::thread::sleep(Duration::from_millis(700));
        // comes in at the clock's place and runs on at double speed (null output: 10 ms steps)
        let diff = a.position().as_secs_f64() - 1.0 - 2.0 * t0.elapsed().as_secs_f64();
        assert!(diff.abs() < 0.08, "audio clock at double speed: {diff:+.3} s");
    }

    /// Before the anchor: ready as soon as the first window delivers samples (the clock waits for that)
    #[test]
    fn ready_once_first_window_delivers() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = test_project(dir.path(), 3.0);
        let a = AudioPlayer::start_null(&p, Time::ZERO).unwrap();
        let t0 = Instant::now();
        while !a.ready() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(a.ready());
        assert!(!a.started(), "no sound without anchor");
    }

    /// Several audible changes in a row (slider drag): one rebuild in flight at a time, states in
    /// between are skipped, the newest one is heard in the end – with real FFmpeg, without device.
    #[test]
    fn remixes_coalesce_to_the_newest_state() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = test_project(dir.path(), 120.0); // the end is never reached, even on a slow machine
        let mut a = AudioPlayer::start_null(&p, Time::ZERO).unwrap();
        a.anchor(Instant::now());
        let wait = |a: &mut AudioPlayer, what: &str, done: &mut dyn FnMut(&mut AudioPlayer) -> bool| {
            let t0 = Instant::now();
            while !done(a) {
                assert!(t0.elapsed() < Duration::from_secs(60), "timeout: {what}");
                a.poll();
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        wait(&mut a, "first sound", &mut |a| a.started());
        let state = |db: f32| {
            let mut q = p.clone();
            q.timeline.tracks[0].clips[0].volume_db = db;
            q
        };
        // hold the output (no switch can happen) while the three changes arrive, however slow the machine
        a.shared.out.lock().unwrap().phase = Phase::Waiting;
        for db in [-3.0, -9.0, -40.0] {
            a.remix(state(db));
        }
        assert!(a.remixing());
        assert_eq!(a.shared.out.lock().unwrap().alt.as_ref().unwrap().gen, 1, "only the first one started");
        assert_eq!(a.queued.as_ref().unwrap().timeline.tracks[0].clips[0].volume_db, -40.0, "newest waits, -9 dB dropped");
        a.shared.out.lock().unwrap().phase = Phase::Running;
        wait(&mut a, "both rebuilds", &mut |a| !a.remixing() && a.queued.is_none() && a.remix_t0.is_none());
        assert_eq!(a.shared.out.lock().unwrap().gen, 2, "exactly two rebuilds (first and newest)");
        // the newest state is heard: -40 dB of a sine at 1/8. Wait on conditions, not on time (a busy
        // machine may not run the output thread or the feed for a while): first the crossfade is over,
        // so later levels come from the newest mix only, then the first block with sound.
        wait(&mut a, "crossfade over", &mut |a| a.shared.out.lock().unwrap().xf == 0);
        // the output stores the level of a block only after it released the lock: once a later
        // block is out, the level of the last crossfade block is stored too and can be discarded
        let at = a.shared.out.lock().unwrap().played;
        wait(&mut a, "a block after the crossfade", &mut |a| a.shared.out.lock().unwrap().played > at);
        a.take_peaks();
        let mut l = 0.0;
        wait(&mut a, "sound of the newest mix", &mut |a| {
            l = a.take_peaks().0;
            l > 0.0
        });
        assert!(l < 0.01, "peak after the rebuild: {l}");
    }

    /// Manual measurement without device: silence around an audible change during playback, old way
    /// (stop + new player; the real device opening comes on top) against the rebuild.
    /// `cargo test --release -p cl-app -- --ignored remix_gap_measurement --nocapture`
    #[test]
    #[ignore]
    fn remix_gap_measurement() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = test_project(dir.path(), 30.0);
        let until = |what: &str, done: &mut dyn FnMut() -> bool| {
            let t0 = Instant::now();
            while !done() {
                assert!(t0.elapsed() < Duration::from_secs(20), "timeout: {what}");
                std::thread::sleep(Duration::from_millis(1));
            }
            t0.elapsed().as_secs_f64() * 1000.0
        };
        for i in 0..3 {
            let a = AudioPlayer::start_null(&p, Time::ZERO).unwrap();
            a.anchor(Instant::now());
            until("start", &mut || a.started());
            std::thread::sleep(Duration::from_millis(300));
            let at = a.position();
            drop(a);
            let b = AudioPlayer::start_null(&p, at).unwrap();
            b.anchor(Instant::now());
            let old = until("restart", &mut || b.first_sound().is_some());
            drop(b);
            let mut a = AudioPlayer::start_null(&p, Time::ZERO).unwrap();
            a.anchor(Instant::now());
            until("start", &mut || a.started());
            std::thread::sleep(Duration::from_millis(300));
            a.take_underruns();
            let mut q = p.clone();
            q.timeline.tracks[0].clips[0].volume_db = -6.0;
            a.remix(q);
            let new = until("switch", &mut || !a.remixing());
            std::thread::sleep(Duration::from_millis(100));
            let (n, d) = a.take_underruns();
            eprintln!("run {i}: old way silence {old:.0} ms · rebuild heard after {new:.0} ms, dropouts {n} ({:.0} ms)", d.as_secs_f64() * 1e3);
        }
    }

    fn test_project(dir: &std::path::Path, secs: f64) -> Project {
        use cl_core::*;
        let src = dir.join("t.wav");
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=f=300:d={secs}"), "-ac", "2"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        let m = cl_media::probe(&src).unwrap();
        let mut p = Project::new("t");
        p.media.push(m.clone());
        let mut t = Track::new(TrackKind::Audio);
        t.clips.push(Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Audio }, Time::ZERO, m.duration));
        p.timeline.tracks.push(t);
        p
    }
}
