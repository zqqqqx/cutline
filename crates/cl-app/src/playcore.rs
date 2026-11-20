//! Player core: the playback flow as pure logic – without egui, without processes and without
//! its own clock (time comes in as input `now` in seconds). The app is only the driver: it
//! reports events (Play, Pause, jump, edit, audio state), asks per layer whether a
//! frame stream should be opened ([`PlayCore::want`]), attaches opened streams ([`PlayCore::attach_alias`]),
//! fetches due frames ([`PlayCore::pull`]) and carries out the audio commands ([`PlayCore::cmds`]).
//!
//! States and transitions:
//! - `Stand` --Play--> `Puffern` (open streams, prepare sound, clock stands)
//! - `Puffern` --all streams at the playhead have a frame, otherwise after 0.5 s, and the first sound is
//!   ready (at most 10 s)--> `Spielen` (clock runs, sound comes in at the clock)
//! - `Spielen` --Pause (sound running, speed 1)--> `Pause` (streams and sound stay open)
//! - `Pause` --Play at the same place without change--> `Spielen` (without new streams)
//! - `Pause` --playhead moved or project changed--> `Stand` (streams freed)
//! - `Spielen`/`Puffern` --jump--> `Puffern` from the new place
//! - J/K/L (`shuttle`) --> `Spielen` with speed ≠ 1 (single frames, no sound)
//! - Stop, project end, project change --> `Stand`
//!
//! In sync instead of behind: if a stream lies more than two frame intervals behind the clock for
//! longer than [`CATCH_UP_AFTER`], it is reopened from "clock + startup time" (the old one keeps
//! showing until then); the automatic quality ([`AutoQuality`]) then lowers the decode size.

use std::collections::HashMap;

use cl_core::Time;

/// Layer: clip or derived version (effect element) as a number, like `ClipId`.
pub type Lid = u128;

/// Frame source of a stream (FFmpeg in the app, simulated in the tests).
pub trait Source {
    type Frame;
    /// Next finished frame, `Ok(None)` = none yet, `Err` = stream ended (source or error)
    fn try_next(&mut self) -> Result<Option<Self::Frame>, ()>;
}

impl Source for cl_media::FrameStream {
    type Frame = cl_media::RgbaFrame;
    fn try_next(&mut self) -> Result<Option<Self::Frame>, ()> {
        cl_media::FrameStream::try_next(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// stopped; frames come as single frames
    Stand,
    /// Play pressed: streams start up, the clock still stands
    Puffern,
    Spielen,
    /// paused with open streams and sound (resume without new buffering)
    Pause,
}

/// Sound as seen by the core (input per tick).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioIn {
    /// no sound (muted, speed ≠ 1, cannot start)
    None,
    /// prepared, not yet audible
    Pending,
    /// prepared, but no sample there yet (first audio window starting up)
    Loading,
    /// audible at this place (audio clock)
    Running(Time),
    /// Output could not be opened
    Failed,
    /// Sound ended or aborted
    Finished,
}

/// Audio commands to the driver.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    /// Prepare sound from `at`; `anchor`: come in at the running clock at once, otherwise only with
    /// [`Cmd::AnchorAudio`]
    StartAudio { at: Time, anchor: bool },
    /// Clock runs now: sound comes in at the clock's place
    AnchorAudio,
    StopAudio,
    PauseAudio,
    ResumeAudio,
}

/// A layer for which a stream should run during playback.
#[derive(Clone, Copy, Debug)]
pub struct Want {
    pub id: Lid,
    /// changes with everything that changes the picture (effects, timing, size, frame rate)
    pub key: u64,
    /// Timeline place where a stream opened ahead begins (clip/element start); for a replacement
    /// during playback the place it is needed from at the earliest (source switch, see `player::want_start`)
    pub start: Time,
    /// End of the clip (a stream shortly before the end is not reopened)
    pub end: Time,
    /// begins only shortly: only open ahead, fetch no frames yet
    pub prefetch: bool,
    /// same picture from the same source (file, source time per timeline place, effects, size),
    /// independent of the clip ID: after splitting, the new clip takes over the running stream
    /// (0 = none)
    pub alias: u64,
}

/// Job "open stream from `at`" (answer to [`PlayCore::want`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Open {
    pub id: Lid,
    pub key: u64,
    pub at: Time,
    pub gen: u64,
    /// running number: only the most recently requested stream of a layer is attached
    pub seq: u64,
}

/// Input per tick.
#[derive(Clone, Copy, Debug)]
pub struct Tick {
    pub now: f64,
    /// Playhead of the UI (if it differs from the core during playback, there was a jump)
    pub playhead: Time,
    /// Edit state of the project (pause holds only until the next change)
    pub revision: u64,
    pub audio: AudioIn,
    /// Project duration
    pub end: Time,
    pub fps: f64,
    pub loop_range: Option<(Time, Time)>,
    /// Recording runs: do not stop at the project end
    pub recording: bool,
    /// highest level of the automatic quality (0 = off)
    pub max_quality_level: usize,
}

/// Measurements since the last [`PlayCore::take_stats`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub opens: u32,
    /// shown stream frames
    pub frames: u32,
    /// fetched but skipped stream frames (catching up)
    pub dropped: u32,
    /// streams reopened on purpose (lag) or stalled streams
    pub catch_ups: u32,
    pub stalls: u32,
    /// largest gap between two frames of the main layer (s)
    pub max_gap: f64,
    /// largest lag of a stream behind the clock (s)
    pub max_lag: f64,
}

impl Stats {
    /// Measurements of two sections together (sums or maxima)
    pub fn add(&mut self, o: &Stats) {
        self.opens += o.opens;
        self.frames += o.frames;
        self.dropped += o.dropped;
        self.catch_ups += o.catch_ups;
        self.stalls += o.stalls;
        self.max_gap = self.max_gap.max(o.max_gap);
        self.max_lag = self.max_lag.max(o.max_lag);
    }
}

/// Lag from which to catch up on purpose: two frame intervals …
const LAG_FRAMES: f64 = 2.0;
/// … longer than this time: then one quality level smaller (stream too slow)
pub const CATCH_UP_AFTER: f64 = 0.2;
/// Reopening costs the seek from the keyframe (seconds with long GOPs): only when the
/// quality is already down and the lag still reaches this size …
const REOPEN_LAG: f64 = 1.0;
/// … and at most once in this time per layer (also with stalled streams: no loop)
const REOPEN_BLOCK: f64 = 5.0;
/// A stream stalls: this long entirely without a frame (before the first frame: seek in the source)
const STALL: f64 = 2.0;
const STALL_FIRST: f64 = 6.0;
/// Buffering ends after this time at the latest (then the clock runs, the picture catches up)
const WARM_MAX: f64 = 0.5;
/// If the first sound is still coming (cold disk), buffering waits for it up to this safety limit
const AUDIO_WAIT_MAX: f64 = 10.0;
/// Audible changes during playback: renew the sound only this long after the last change
pub const AUDIO_DEBOUNCE: f64 = 0.15;
/// Opening failed: do not retry for this long
const RETRY_OPEN: f64 = 1.0;
/// A replacement stream with a frame takes over at the latest this long after attaching (even with lag)
const REPLACE_AFTER: f64 = 1.0;
/// Changed key during playback (dragging a slider: a new one per UI frame): replace only once it
/// stays this long. Before, each frame started an FFmpeg from the keyframe, discarded as soon as the
/// next value came – that took computing time from the running stream (click test K5: picture stood 1.8 s).
const KEY_SETTLE: f64 = 0.15;
/// Single frame during playback (reverse, speed curve, animated images): a running job is
/// not replaced just because the clock is further – its frame is shown even if it is a few frames
/// old (otherwise every job starves). Only renewed after this long as a safeguard.
const STILL_PATIENCE: f64 = 10.0;

struct Slot<S: Source> {
    src: S,
    key: u64,
    alias: u64,
    /// Timeline place of the first frame
    t0: Time,
    fps: f64,
    taken: u64,
    ended: bool,
    waiting_since: Option<f64>,
    opened: f64,
    first: Option<f64>,
    behind_since: Option<f64>,
    /// Frames in the current measurement window
    win_frames: u64,
    /// Opened ahead: first frame fetched before its turn, and when it was there (real startup)
    ahead: Option<S::Frame>,
    ready: Option<f64>,
}

impl<S: Source> Slot<S> {
    fn new(src: S, key: u64, t0: Time, fps: f64, now: f64) -> Self {
        Self { src, key, alias: 0, t0, fps: fps.max(1.0), taken: 0, ended: false, waiting_since: None, opened: now, first: None, behind_since: None, win_frames: 0, ahead: None, ready: None }
    }

    /// Fetch all due frames, return the newest (the rest counts as skipped).
    /// `early`: fetch the first frame before the start already (a visible clip in the transition before its
    /// start shows its first frame until then); otherwise nothing is due before `t0`.
    fn drain(&mut self, pos: Time, now: f64, st: &mut Stats, early: bool) -> Option<S::Frame> {
        if pos < self.t0 && !(early && self.taken == 0) {
            // opened ahead: not its turn yet – take the first frame already, so `lead` is the startup, not the wait
            if self.taken == 0 {
                if let Ok(Some(f)) = self.src.try_next() {
                    (self.taken, self.ahead, self.ready) = (1, Some(f), Some(now));
                }
            }
            return None;
        }
        let target = ((pos - self.t0).as_secs_f64().max(0.0) * self.fps + 1e-6).floor() as u64;
        let mut latest = self.ahead.take();
        let mut n = latest.is_some() as u64;
        while self.taken <= target {
            match self.src.try_next() {
                Ok(Some(f)) => {
                    latest = Some(f);
                    self.taken += 1;
                    n += 1;
                }
                Ok(None) => break,
                Err(()) => {
                    self.ended = true;
                    break;
                }
            }
        }
        st.dropped += n.saturating_sub(1) as u32;
        self.win_frames += n;
        if n > 0 {
            self.first.get_or_insert(now);
        }
        if latest.is_some() || self.taken > target {
            self.waiting_since = None;
        } else {
            self.waiting_since.get_or_insert(now);
        }
        latest
    }

    /// Lag of the last fetched frame behind the clock (s)
    fn lag(&self, pos: Time) -> Option<f64> {
        (self.taken > 0 && pos >= self.t0).then(|| pos.as_secs_f64() - (self.t0.as_secs_f64() + (self.taken - 1) as f64 / self.fps))
    }

    /// Stream ended or entirely without a frame since [`STALL`] (before the first frame the seek in the
    /// source is still running: more generous). A slow stream that still delivers does not stall.
    fn stalled(&self, now: f64) -> bool {
        let limit = if self.taken == 0 { STALL_FIRST } else { STALL };
        self.ended || self.waiting_since.is_some_and(|w| now - w > limit)
    }

    /// Startup time of this stream (open to first frame), for catching up
    fn lead(&self) -> f64 {
        self.ready.or(self.first).map_or(0.3, |f| f - self.opened).clamp(0.05, 3.0)
    }
}

pub struct PlayCore<S: Source> {
    mode: Mode,
    pos: Time,
    rate: f64,
    /// (wall time, place): system clock as long as no sound runs
    clock: Option<(f64, Time)>,
    warm_since: f64,
    paused: Option<(Time, u64)>,
    gen: u64,
    cur: HashMap<Lid, Slot<S>>,
    /// Replacement with a new key (edit, quality, catching up): shows only once it delivers
    next: HashMap<Lid, Slot<S>>,
    /// Opening requested: (key, as replacement, running number)
    opening: HashMap<Lid, (u64, bool, u64)>,
    seq: u64,
    /// reopen on purpose from this place
    reopen: HashMap<Lid, Time>,
    failed: HashMap<Lid, (u64, f64)>,
    /// last reopened (stall, catching up): blocking period [`REOPEN_BLOCK`]
    reopened: HashMap<Lid, f64>,
    /// new key per layer since (for [`KEY_SETTLE`])
    settle: HashMap<Lid, (u64, f64)>,
    /// layers wanted in this UI frame (a stream is only taken over from a layer no longer wanted)
    wanted: Vec<Lid>,
    /// highest level of the automatic quality (from the tick)
    max_level: usize,
    /// Single frames: requested (key, since) and done (shown or failed)
    requested: HashMap<Lid, (u64, f64)>,
    shown: HashMap<Lid, u64>,
    /// layer last supplied with a frame (there is a displayable frame)
    held: Option<Lid>,
    audio_due: Option<f64>,
    last_primary: Option<f64>,
    /// Jumps (Play, jump, loop, stop): the clock may only go back with these
    jumps: u64,
    win_start: f64,
    win_catch_up: bool,
    pub quality: AutoQuality,
    /// catch up on purpose (can be turned off only for comparison measurements)
    pub catch_up: bool,
    stats: Stats,
    cmds: Vec<Cmd>,
}

impl<S: Source> Default for PlayCore<S> {
    fn default() -> Self {
        Self {
            mode: Mode::Stand,
            pos: Time::ZERO,
            rate: 1.0,
            clock: None,
            warm_since: 0.0,
            paused: None,
            gen: 0,
            cur: HashMap::new(),
            next: HashMap::new(),
            opening: HashMap::new(),
            seq: 0,
            reopen: HashMap::new(),
            failed: HashMap::new(),
            reopened: HashMap::new(),
            settle: HashMap::new(),
            wanted: Vec::new(),
            max_level: 0,
            requested: HashMap::new(),
            shown: HashMap::new(),
            held: None,
            audio_due: None,
            last_primary: None,
            jumps: 0,
            win_start: 0.0,
            win_catch_up: false,
            quality: AutoQuality::default(),
            catch_up: true,
            stats: Stats::default(),
            cmds: Vec::new(),
        }
    }
}

/// Last frame before `end` (project end) at `fps`
pub fn last_frame(end: Time, fps: f64) -> Time {
    Time((end - Time::from_secs_f64(1.0 / fps.max(1.0))).0.max(0))
}

impl<S: Source> PlayCore<S> {
    pub fn mode(&self) -> Mode {
        self.mode
    }
    pub fn pos(&self) -> Time {
        self.pos
    }
    pub fn rate(&self) -> f64 {
        self.rate
    }
    #[cfg(test)]
    pub fn gen(&self) -> u64 {
        self.gen
    }
    #[cfg(test)]
    pub fn jumps(&self) -> u64 {
        self.jumps
    }
    /// Clock runs or is about to run (buffering)
    pub fn playing(&self) -> bool {
        matches!(self.mode, Mode::Puffern | Mode::Spielen)
    }
    /// Frames come from streams (otherwise single frames)
    pub fn streaming(&self) -> bool {
        self.playing() && (self.rate - 1.0).abs() < 1e-6
    }
    #[cfg(test)]
    /// Layer with the last delivered frame (`None`: none ever came)
    pub fn held(&self) -> Option<Lid> {
        self.held
    }
    /// Collect audio commands
    pub fn cmds(&mut self) -> Vec<Cmd> {
        std::mem::take(&mut self.cmds)
    }
    pub fn take_stats(&mut self) -> Stats {
        std::mem::take(&mut self.stats)
    }
    /// Measurements since the last `take_stats`, without resetting them
    pub fn stats(&self) -> Stats {
        self.stats
    }
    #[cfg(test)]
    /// Open streams: (layer, key, is replacement)
    pub fn streams(&self) -> Vec<(Lid, u64, bool)> {
        self.cur.iter().map(|(id, s)| (*id, s.key, false)).chain(self.next.iter().map(|(id, s)| (*id, s.key, true))).collect()
    }

    /// Size of all lists of the core (endurance measurement)
    #[cfg(test)]
    pub fn sizes(&self) -> String {
        format!(
            "cur {} next {} opening {} reopen {} failed {} reopened {} wanted {} requested {} shown {} cmds {}",
            self.cur.len(),
            self.next.len(),
            self.opening.len(),
            self.reopen.len(),
            self.failed.len(),
            self.reopened.len(),
            self.wanted.len(),
            self.requested.len(),
            self.shown.len(),
            self.cmds.len()
        )
    }

    /// Frames fetched so far from the layer's running stream (test: does the picture keep running?)
    #[cfg(test)]
    pub fn taken(&self, id: Lid) -> u64 {
        self.cur.get(&id).map_or(0, |s| s.taken)
    }

    /// Open streams (running and replacement)
    #[cfg(test)]
    pub fn stream_count(&self) -> usize {
        self.cur.len() + self.next.len()
    }

    /// Is the player waiting for a frame (wait line)? During playback: only while buffering. When
    /// stopped: no matching frame has been delivered for a requested single frame yet.
    pub fn waiting(&self) -> bool {
        match self.mode {
            Mode::Puffern => true,
            Mode::Spielen => false,
            Mode::Stand | Mode::Pause => self.requested.iter().any(|(id, (k, _))| self.shown.get(id) != Some(k)),
        }
    }

    /// Picture-sound offset (ms): how far the shown stream frame lies behind the clock (largest value)
    pub fn av_offset_ms(&self) -> Option<f64> {
        self.cur.values().filter_map(|s| s.lag(self.pos)).map(|l| l * 1000.0).reduce(f64::max)
    }

    fn clear_streams(&mut self) {
        // jump: running single frames belong to the old place
        self.requested.clear();
        self.reopened.clear();
        self.settle.clear();
        self.cur.clear();
        self.next.clear();
        self.opening.clear();
        self.reopen.clear();
        self.failed.clear();
    }

    fn start_audio(&mut self, at: Time, anchor: bool) {
        self.cmds.push(Cmd::StopAudio);
        if (self.rate - 1.0).abs() < 1e-6 {
            self.cmds.push(Cmd::StartAudio { at, anchor });
        }
    }

    /// Play: when stopped, buffer from `at`; when paused at the same place (`at`, `revision`)
    /// continue without new streams.
    pub fn play(&mut self, at: Time, revision: u64, now: f64) {
        if self.mode == Mode::Pause && self.paused == Some((at, revision)) {
            self.paused = None;
            // the place stands still in the pause: streams continue without a shift
            for s in self.cur.values_mut() {
                s.waiting_since = None;
                s.behind_since = None;
                s.win_frames = 0;
            }
            // reset the measurement window of the automatic quality: in the pause the UI hardly
            // draws (no tick), otherwise the whole pause would count as a window without frames
            self.win_start = now;
            self.win_catch_up = false;
            self.mode = Mode::Spielen;
            self.clock = Some((now, at));
            self.last_primary = None;
            self.cmds.push(Cmd::ResumeAudio);
            return;
        }
        self.reset(at);
        self.mode = Mode::Puffern;
        self.warm_since = now;
        self.win_start = now;
        self.start_audio(at, false);
    }

    /// Free everything, stand at `at`
    fn reset(&mut self, at: Time) {
        self.clear_streams();
        self.mode = Mode::Stand;
        self.rate = 1.0;
        self.pos = at;
        self.clock = None;
        self.paused = None;
        self.audio_due = None;
        self.last_primary = None;
        self.jumps += 1;
    }

    /// Would Play continue now without new buffering (paused at the same place, nothing changed)?
    pub fn can_resume(&self, at: Time, revision: u64) -> bool {
        self.mode == Mode::Pause && self.paused == Some((at, revision))
    }

    /// Are there open single-frame jobs (loading indicator when opening a project)?
    pub fn has_requests(&self) -> bool {
        self.requested.iter().any(|(id, (k, _))| self.shown.get(id) != Some(k))
    }

    /// Pause. Only with sound running at speed 1 (`can_hold`, e.g. not with multicam) do
    /// streams and sound stay open, otherwise it stops.
    pub fn pause(&mut self, revision: u64, audio_running: bool, can_hold: bool) {
        if self.mode != Mode::Spielen || !audio_running || !can_hold || (self.rate - 1.0).abs() > 1e-6 {
            self.stop();
            return;
        }
        self.mode = Mode::Pause;
        self.paused = Some((self.pos, revision));
        self.next.clear();
        self.opening.clear();
        self.reopen.clear();
        self.clock = None;
        self.audio_due = None;
        self.cmds.push(Cmd::PauseAudio);
    }

    pub fn stop(&mut self) {
        let was = self.mode != Mode::Stand || !self.cur.is_empty();
        self.reset(self.pos);
        if was {
            self.cmds.push(Cmd::StopAudio);
        }
    }

    /// J/K/L: at speed `rate` from `at`, without buffering (speed ≠ 1: single frames, no sound)
    pub fn shuttle(&mut self, rate: f64, at: Time, now: f64) {
        self.reset(at);
        self.rate = rate;
        self.mode = Mode::Spielen;
        self.clock = Some((now, at));
        self.start_audio(at, true);
    }

    /// Jump to `at` (during playback: buffer again from there)
    pub fn seek(&mut self, at: Time, now: f64) {
        match self.mode {
            Mode::Stand => self.pos = at,
            Mode::Pause => self.stop(),
            Mode::Puffern | Mode::Spielen => {
                let rate = self.rate;
                if (rate - 1.0).abs() > 1e-6 {
                    self.shuttle(rate, at, now);
                } else {
                    self.play(at, 0, now);
                }
            }
        }
        self.pos = at;
    }

    /// Edit during playback. Visible changes need nothing (new keys →
    /// replacement streams); audible ones renew the sound only [`AUDIO_DEBOUNCE`] after the last one (dragging
    /// a slider does not restart it at every step), the picture keeps running.
    pub fn edit(&mut self, audible: bool, now: f64) {
        if audible && self.playing() {
            self.audio_due = Some(now + AUDIO_DEBOUNCE);
        }
    }

    /// Renew sound at once from the current place (solo, output device)
    pub fn restart_audio(&mut self, now: f64) {
        if !self.playing() {
            return;
        }
        self.audio_due = None;
        let anchor = self.mode == Mode::Spielen;
        self.start_audio(self.pos, anchor);
        if anchor {
            self.clock = Some((now, self.pos));
        }
    }

    /// Project change: free everything, jobs of the old project expire
    pub fn new_project(&mut self) {
        self.stop();
        self.gen += 1;
        self.requested.clear();
        self.shown.clear();
        self.held = None;
        self.pos = Time::ZERO;
    }

    /// Preview renewed (quality, proxy): streams and single-frame jobs anew, shown frames stay
    pub fn invalidate(&mut self) {
        self.clear_streams();
        self.requested.clear();
    }

    /// Project changed: request single frames again (streams change via their key)
    pub fn forget_stills(&mut self) {
        self.requested.clear();
    }

    /// Tick: advance the clock, end buffering, process audio state, end/loop.
    pub fn tick(&mut self, t: &Tick) -> Time {
        let now = t.now;
        match self.mode {
            Mode::Stand => self.pos = t.playhead,
            Mode::Pause => {
                if self.paused.is_some_and(|(at, rev)| at != t.playhead || rev != t.revision) {
                    self.stop();
                }
                self.pos = t.playhead;
            }
            Mode::Puffern | Mode::Spielen if t.playhead != self.pos => self.seek(t.playhead, now),
            Mode::Puffern => self.warm(t),
            Mode::Spielen => self.run(t),
        }
        self.max_level = t.max_quality_level;
        self.measure(now, t.max_quality_level);
        self.pos
    }

    fn warm(&mut self, t: &Tick) {
        let pos = self.pos;
        let (mut any, mut ready) = (false, true);
        for s in self.cur.values().filter(|s| s.t0 <= pos) {
            any = true;
            ready &= s.taken > 0;
        }
        let waited = t.now - self.warm_since;
        // first sound still on its way: do not start on system time, or the sound would chase the clock
        let audio_late = t.audio == AudioIn::Loading && waited < AUDIO_WAIT_MAX;
        if !audio_late && ((ready && (any || waited > 0.05)) || waited > WARM_MAX) {
            self.mode = Mode::Spielen;
            for s in self.cur.values_mut().filter(|s| s.t0 <= pos) {
                // first frame = now (do not skip frames that became due during buffering)
                s.t0 = pos - Time::from_secs_f64(s.taken.saturating_sub(1) as f64 / s.fps);
            }
            self.clock = Some((t.now, pos));
            if t.audio == AudioIn::None {
                self.start_audio(pos, true);
            } else {
                self.cmds.push(Cmd::AnchorAudio);
            }
        }
    }

    fn run(&mut self, t: &Tick) {
        let now = t.now;
        if self.audio_due.is_some_and(|d| now >= d) {
            self.audio_due = None;
            self.start_audio(self.pos, true);
            self.clock = Some((now, self.pos));
        } else {
            match t.audio {
                AudioIn::Failed | AudioIn::Finished => {
                    // without sound continue on system time
                    self.cmds.push(Cmd::StopAudio);
                    self.clock = Some((now, self.pos));
                }
                // audio clock; never back (handover from system time is without a jump)
                AudioIn::Running(p) => {
                    self.pos = p.max(self.pos);
                    self.clock = Some((now, self.pos));
                }
                AudioIn::None | AudioIn::Pending | AudioIn::Loading => {}
            }
        }
        if !matches!(t.audio, AudioIn::Running(_)) {
            let (t0, p0) = *self.clock.get_or_insert((now, self.pos));
            let s = p0.as_secs_f64() + (now - t0) * self.rate;
            if s <= 0.0 && self.rate < 0.0 {
                self.reset(Time::ZERO);
                self.cmds.push(Cmd::StopAudio);
                return;
            }
            let next = Time::from_secs_f64(s.max(0.0));
            // forward never back (rounding at the handover sound → system time)
            self.pos = if self.rate > 0.0 { next.max(self.pos) } else { next };
        }
        if self.rate > 0.0 {
            if let Some((a, b)) = t.loop_range.filter(|(a, b)| a < b) {
                if self.pos >= b {
                    self.clear_streams();
                    self.pos = a;
                    self.jumps += 1;
                    self.last_primary = None;
                    self.clock = Some((now, a));
                    self.start_audio(a, true);
                    return;
                }
            }
        }
        if self.pos >= t.end && !t.recording {
            self.reset(last_frame(t.end, t.fps));
            self.cmds.push(Cmd::StopAudio);
        }
    }

    /// Measurement window (~1 s) for the automatic quality: share of delivered frames per stream
    /// When stopped back to full quality; while buffering and paused the level stays (otherwise
    /// resuming would need new streams), measured only while playing.
    fn measure(&mut self, now: f64, max: usize) {
        if self.mode != Mode::Spielen {
            if self.mode == Mode::Stand || max == 0 {
                self.quality.decide(now, None, max, false);
            }
            self.win_start = now;
            self.win_catch_up = false;
            for s in self.cur.values_mut() {
                s.win_frames = 0;
            }
            return;
        }
        if now - self.win_start < 1.0 {
            return;
        }
        let (pos, ws) = (self.pos, self.win_start);
        let mut ratio: Option<f64> = None;
        for s in self.cur.values_mut() {
            // streams opened ahead (not their turn yet) and not yet started do not count
            let Some(first) = s.first.filter(|_| s.t0 <= pos) else {
                s.win_frames = 0;
                continue;
            };
            let active = now - first.max(ws);
            if active >= 0.5 && !s.ended {
                let r = s.win_frames as f64 / (active * s.fps);
                ratio = Some(ratio.map_or(r, |x: f64| x.min(r)));
            }
            s.win_frames = 0;
        }
        if self.win_catch_up {
            ratio = Some(ratio.map_or(0.5, |r| r.min(0.5)));
        }
        self.quality.decide(now, ratio, max, true);
        self.win_start = now;
        self.win_catch_up = false;
    }

    /// Should a stream be opened for layer `w`? `Some(open)`: open from `open.at` and report back with
    /// [`Self::attach_alias`] (or [`Self::open_failed`]). Only during playback at speed 1.
    pub fn want(&mut self, w: &Want, now: f64) -> Option<Open> {
        if !self.streaming() {
            return None;
        }
        let id = w.id;
        if self.failed.get(&id).is_some_and(|(k, t)| *k == w.key && now - t < RETRY_OPEN) {
            return None;
        }
        self.wanted.push(id);
        // stalled or prematurely ended stream: reopen at the current place (the last frame
        // stays until the new one delivers), with a blocking period against a restart loop
        if !w.prefetch
            && self.mode == Mode::Spielen
            && (w.end - self.pos).as_secs_f64() > 0.25
            && self.reopened.get(&id).is_none_or(|t| now - t >= REOPEN_BLOCK)
            && self.cur.get(&id).is_some_and(|s| s.key == w.key && s.t0 <= self.pos && s.stalled(now))
        {
            self.cur.remove(&id);
            self.next.remove(&id);
            self.reopened.insert(id, now);
            self.stats.stalls += 1;
        }
        // after splitting etc.: take over a running stream of the same source from a layer no longer wanted
        if w.alias != 0 && !w.prefetch && !self.cur.contains_key(&id) && !self.opening.contains_key(&id) {
            let donor = self.cur.iter().find(|(o, s)| s.alias == w.alias && !self.wanted.contains(o) && s.t0 <= self.pos).map(|(o, _)| *o);
            if let Some(o) = donor {
                if let Some(mut s) = self.cur.remove(&o) {
                    s.key = w.key;
                    self.next.remove(&o);
                    self.cur.insert(id, s);
                    if let Some(k) = self.shown.remove(&o) {
                        self.shown.insert(id, k);
                    }
                }
            }
        }
        let cur = self.cur.get(&id).is_some_and(|s| s.key == w.key);
        let next = self.next.get(&id).is_some_and(|s| s.key == w.key);
        let opening = self.opening.get(&id).is_some_and(|(k, _, _)| *k == w.key);
        if next || opening {
            return None;
        }
        // replacement for a running stream: only once the new key stays for a moment
        if !cur && !w.prefetch && self.mode == Mode::Spielen && self.cur.contains_key(&id) {
            let since = match self.settle.get(&id) {
                Some((k, t)) if *k == w.key => *t,
                _ => {
                    self.settle.insert(id, (w.key, now));
                    now
                }
            };
            if now - since < KEY_SETTLE {
                return None;
            }
        }
        self.seq += 1;
        if cur {
            let at = self.reopen.remove(&id)?;
            self.opening.insert(id, (w.key, true, self.seq));
            return Some(Open { id, key: w.key, at, gen: self.gen, seq: self.seq });
        }
        self.reopen.remove(&id);
        let replace = self.cur.contains_key(&id) && !w.prefetch;
        self.opening.insert(id, (w.key, replace, self.seq));
        let at = match self.cur.get(&id) {
            _ if w.prefetch => w.start,
            // replacement during playback: open where the clock stands when it starts up – the old
            // stream's startup says nothing about a new source, so not before the place it is needed from
            Some(s) if self.mode == Mode::Spielen => (self.pos + Time::from_secs_f64(s.lead())).max(w.start),
            _ => self.pos,
        };
        Some(Open { id, key: w.key, at, gen: self.gen, seq: self.seq })
    }

    /// Attach an opened stream. Stale ones (other project, key no longer wanted,
    /// no longer playing) are discarded (ended when dropped).
    /// `alias`: source fingerprint of the stream (see [`Want::alias`]).
    pub fn attach_alias(&mut self, o: Open, fps: f64, src: S, now: f64, alias: u64) -> bool {
        if o.gen != self.gen || !self.streaming() {
            return false;
        }
        let Some((key, replace, _)) = self.opening.get(&o.id).copied().filter(|(k, _, q)| *k == o.key && *q == o.seq) else {
            return false;
        };
        self.opening.remove(&o.id);
        let mut slot = Slot::new(src, key, o.at, fps, now);
        slot.alias = alias;
        if replace && self.cur.contains_key(&o.id) {
            self.next.insert(o.id, slot);
        } else {
            self.cur.insert(o.id, slot);
        }
        self.stats.opens += 1;
        true
    }

    /// Opening failed: retry only after [`RETRY_OPEN`]
    pub fn open_failed(&mut self, o: Open, now: f64) {
        if self.opening.get(&o.id).is_some_and(|(k, _, q)| *k == o.key && *q == o.seq) {
            self.opening.remove(&o.id);
            self.failed.insert(o.id, (o.key, now));
        }
    }

    /// Due frame of the layer (the newest up to the clock; older ones are skipped). A replacement
    /// stream takes over with its first due frame. `primary`: main layer (gap measurement).
    pub fn pull(&mut self, id: Lid, now: f64, primary: bool) -> Option<S::Frame> {
        let pos = self.pos;
        let mut got = None;
        if let Some(n) = self.next.get_mut(&id) {
            if let Some(f) = n.drain(pos, now, &mut self.stats, false) {
                // take over only when the replacement is not behind the previous stream (never show an
                // older frame); until then it catches up and its frames expire. If it is
                // slower than real time (heavy effect), it never catches up: after REPLACE_AFTER
                // it takes over anyway, then catching up and automatic quality take effect.
                let behind = |s: &Slot<S>| s.lag(pos).unwrap_or(f64::MAX);
                let old = self.cur.get(&id).filter(|c| !c.stalled(now) && !c.ended);
                if now - n.opened > REPLACE_AFTER || old.is_none_or(|c| behind(n) <= behind(c) + 1e-9) {
                    if let Some(n) = self.next.remove(&id) {
                        self.cur.insert(id, n);
                    }
                    got = Some(f);
                } else {
                    self.stats.dropped += 1;
                }
            } else if n.stalled(now) {
                // replacement ends or stalls without a frame: discard, retry after RETRY_OPEN
                let key = n.key;
                self.next.remove(&id);
                self.failed.insert(id, (key, now));
            }
        }
        let s = self.cur.get_mut(&id)?;
        if got.is_none() {
            got = s.drain(pos, now, &mut self.stats, true);
        }
        // lag over two frame intervals longer than CATCH_UP_AFTER: stream too slow → one
        // quality level smaller (due frames are skipped up to the clock anyway). Reopen from
        // clock + startup time only when the quality is already down and the lag grows –
        // a restart seeks from the keyframe and costs time itself.
        if let Some(lag) = s.lag(pos).filter(|_| self.mode == Mode::Spielen) {
            self.stats.max_lag = self.stats.max_lag.max(lag);
            if lag > LAG_FRAMES / s.fps {
                let since = *s.behind_since.get_or_insert(now);
                if now - since >= CATCH_UP_AFTER {
                    self.win_catch_up = true;
                }
                let bottom = self.quality.level >= self.max_level;
                if self.catch_up
                    && bottom
                    && lag >= REOPEN_LAG
                    && self.reopened.get(&id).is_none_or(|t| now - t >= REOPEN_BLOCK)
                    && !self.next.contains_key(&id)
                    && !self.opening.contains_key(&id)
                {
                    s.behind_since = None;
                    let at = pos + Time::from_secs_f64(s.lead());
                    self.reopen.insert(id, at);
                    self.reopened.insert(id, now);
                    self.stats.catch_ups += 1;
                }
            } else {
                s.behind_since = None;
            }
        }
        if got.is_some() {
            let key = self.cur.get(&id).map_or(0, |s| s.key);
            self.shown.insert(id, key);
            self.held = Some(id);
            self.stats.frames += 1;
            if primary && self.mode == Mode::Spielen {
                if let Some(last) = self.last_primary {
                    self.stats.max_gap = self.stats.max_gap.max(now - last);
                }
                self.last_primary = Some(now);
            }
        }
        got
    }

    /// Single frame for the layer: `true` = request now. During playback (reverse, speed,
    /// animated images) a running job is not replaced while it is young – otherwise
    /// each new one aborts the previous one and no frame ever comes (standstill).
    pub fn still(&mut self, id: Lid, key: u64, now: f64) -> bool {
        match self.requested.get(&id) {
            Some((k, _)) if *k == key => false,
            Some((k, since)) if self.playing() && self.shown.get(&id) != Some(k) && now - since < STILL_PATIENCE => false,
            _ => {
                self.requested.insert(id, (key, now));
                true
            }
        }
    }

    /// Single frame delivered: show it if it is the requested one
    pub fn still_done(&mut self, id: Lid, key: u64) -> bool {
        if self.requested.get(&id).is_some_and(|(k, _)| *k == key) {
            self.shown.insert(id, key);
            self.held = Some(id);
            true
        } else {
            false
        }
    }

    /// Single frame could not be decoded: the job is finished (loading indicator ends, during
    /// playback the next one is requested right away). The same key is not retried at once
    /// (otherwise one FFmpeg per UI frame), only after a change.
    pub fn still_failed(&mut self, id: Lid, key: u64) {
        if self.requested.get(&id).is_some_and(|(k, _)| *k == key) {
            self.shown.insert(id, key);
        }
    }

    /// Only these layers are still visible or about to come: free everything else
    pub fn visible(&mut self, ids: &[Lid]) {
        self.wanted.clear();
        self.reopened.retain(|id, _| ids.contains(id));
        self.settle.retain(|id, _| ids.contains(id));
        self.cur.retain(|id, _| ids.contains(id));
        self.next.retain(|id, _| ids.contains(id));
        self.opening.retain(|id, _| ids.contains(id));
        self.reopen.retain(|id, _| ids.contains(id));
        self.requested.retain(|id, _| ids.contains(id));
        self.shown.retain(|id, _| ids.contains(id));
    }
}

/// Automatic quality during playback: level 0 = chosen quality; each level above halves
/// the decode size or, last, halves the frame rate (see [`quality_steps`]).
#[derive(Clone, Debug)]
pub struct AutoQuality {
    pub level: usize,
    good_since: Option<f64>,
    hold_until: f64,
    up_wait: f64,
    last_up: Option<f64>,
}

impl Default for AutoQuality {
    fn default() -> Self {
        Self { level: 0, good_since: None, hold_until: 0.0, up_wait: UP_WAIT, last_up: None }
    }
}

/// Below this share of the target frame rate (over ~1 s) one level smaller …
pub const TOO_SLOW: f64 = 0.9;
/// … from this share on, one level back after this long in a row
const GOOD: f64 = 0.98;
const UP_WAIT: f64 = 4.0;
/// After a change: do not decide again for this long (new streams are starting up)
const SETTLE: f64 = 1.5;

impl AutoQuality {
    /// Evaluate a measurement window: `ratio` = delivered / due frames (worst stream),
    /// `max` = highest level (0 = off). When stopped back to 0. If stepping up fails within
    /// 3 s, the next waits twice as long (at most 30 s): no flapping.
    pub fn decide(&mut self, now: f64, ratio: Option<f64>, max: usize, playing: bool) -> usize {
        if !playing || max == 0 {
            // the learned waiting time stays (the same computer is just as fast on the next Play)
            let up_wait = if max == 0 { UP_WAIT } else { self.up_wait };
            *self = Self { up_wait, ..Self::default() };
            return 0;
        }
        self.level = self.level.min(max);
        if now < self.hold_until {
            return self.level;
        }
        match ratio {
            Some(r) if r < TOO_SLOW => {
                if self.level < max {
                    if self.last_up.is_some_and(|t| now - t < 3.0) {
                        self.up_wait = (self.up_wait * 2.0).min(30.0);
                    }
                    self.level += 1;
                    self.hold_until = now + SETTLE;
                    self.last_up = None;
                }
                self.good_since = None;
            }
            Some(r) if r >= GOOD && self.level > 0 => {
                let g = *self.good_since.get_or_insert(now);
                if now - g >= self.up_wait {
                    self.level -= 1;
                    self.last_up = Some(now);
                    self.hold_until = now + SETTLE;
                    self.good_since = None;
                }
            }
            Some(_) => self.good_since = None,
            None => {}
        }
        self.level
    }
}

/// Levels of the automatic quality: (divisor of the decode size, half frame rate). The quality
/// chosen by the user `user_div` is the upper limit; nothing smaller than ¼ is computed; above
/// 31 fps the last level is ¼ with half frame rate.
pub fn quality_steps(user_div: u32, fps: f64) -> Vec<(u32, bool)> {
    let mut v = Vec::new();
    let mut d = user_div.clamp(1, 4);
    v.push((d, false));
    while d < 4 {
        d *= 2;
        v.push((d, false));
    }
    if fps > 31.0 {
        v.push((d, true));
    }
    v
}

#[cfg(test)]
mod tests;
