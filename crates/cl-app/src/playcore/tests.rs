//! Tests of the player core without a window: simulated decoder (startup time, time per frame, never delivers,
//! end, opening fails) and simulated sound (startup time, error, abort).

use super::*;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug)]
struct Dec {
    /// Open to first frame (s)
    delay: f64,
    /// Decode time per frame (s) at full quality
    per_frame: f64,
    never: bool,
    /// Stream ends after this many frames
    len: Option<u64>,
    fail_open: bool,
}

impl Default for Dec {
    fn default() -> Self {
        Self { delay: 0.05, per_frame: 0.004, never: false, len: None, fail_open: false }
    }
}

/// Simulated stream: frame n is ready from `opened + delay + n·per_frame`; frame value = timeline time.
struct Sim {
    clock: Rc<Cell<f64>>,
    opened: f64,
    d: Dec,
    per_frame: f64,
    n: u64,
    t0: f64,
    fps: f64,
}

impl Source for Sim {
    type Frame = f64;
    fn try_next(&mut self) -> Result<Option<f64>, ()> {
        if self.d.len.is_some_and(|l| self.n >= l) {
            return Err(());
        }
        if self.d.never {
            return Ok(None);
        }
        let ready = self.opened + self.d.delay + self.n as f64 * self.per_frame;
        if self.clock.get() + 1e-9 >= ready {
            self.n += 1;
            Ok(Some(self.t0 + (self.n - 1) as f64 / self.fps))
        } else {
            Ok(None)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum AState {
    Off,
    Prep { at: Time, ready: f64, anchor: Option<f64> },
    Run { at: Time, anchor: f64, paused_at: Option<f64>, paused: f64 },
    Failed,
    Done,
}

/// Simulated sound: comes in `latency` after the start at the clock's place.
struct Aud {
    st: AState,
    latency: f64,
    fail: bool,
    /// Sound aborts at this wall time
    cut_at: Option<f64>,
    starts: u32,
    remixes: u32,
}

impl Aud {
    fn new() -> Self {
        Self { st: AState::Off, latency: 0.1, fail: false, cut_at: None, starts: 0, remixes: 0 }
    }
    fn input(&mut self, now: f64) -> AudioIn {
        if let AState::Prep { at, ready, anchor: Some(a) } = self.st {
            if now >= ready {
                self.st = AState::Run { at, anchor: a, paused_at: None, paused: 0.0 };
            }
        }
        if matches!(self.st, AState::Run { .. }) && self.cut_at.is_some_and(|c| now >= c) {
            self.st = AState::Done;
        }
        match self.st {
            AState::Off => AudioIn::None,
            AState::Prep { ready, .. } if now < ready => AudioIn::Loading,
            AState::Prep { .. } => AudioIn::Pending,
            AState::Run { at, anchor, paused_at, paused } => {
                let now = paused_at.unwrap_or(now);
                AudioIn::Running(at + Time::from_secs_f64(now - anchor - paused))
            }
            AState::Failed => AudioIn::Failed,
            AState::Done => AudioIn::Finished,
        }
    }
    fn apply(&mut self, c: Cmd, now: f64) {
        match c {
            Cmd::StartAudio { at, anchor } => {
                self.starts += 1;
                self.st = if self.fail { AState::Failed } else { AState::Prep { at, ready: now + self.latency, anchor: anchor.then_some(now) } };
            }
            Cmd::AnchorAudio => {
                if let AState::Prep { anchor, .. } = &mut self.st {
                    *anchor = Some(now);
                }
            }
            Cmd::StopAudio => self.st = AState::Off,
            // the running sound keeps playing (and keeps the clock) while the new mix is made
            Cmd::RemixAudio => self.remixes += 1,
            Cmd::PauseAudio => {
                if let AState::Run { paused_at, .. } = &mut self.st {
                    *paused_at = Some(now);
                }
            }
            Cmd::ResumeAudio => {
                if let AState::Run { paused_at, paused, .. } = &mut self.st {
                    if let Some(p) = paused_at.take() {
                        *paused += now - p;
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Clip {
    id: Lid,
    start: f64,
    end: f64,
    ver: u64,
    dec: Dec,
    /// Reverse/speed curve: only single frames
    still_only: bool,
    /// visible this long before `start` already (transition from the predecessor)
    lead_in: f64,
    /// Source fingerprint (same source after splitting), 0 = none
    alias: u64,
    /// While active: place from which a new stream is needed (`Want::start`, source switch)
    want_start: Option<f64>,
}

fn clip(id: Lid, start: f64, end: f64) -> Clip {
    Clip { id, start, end, ver: 0, dec: Dec::default(), still_only: false, lead_in: 0.0, alias: 0, want_start: None }
}

/// Driver as in the app, but with simulated streams, single frames and sound.
struct World {
    core: PlayCore<Sim>,
    clock: Rc<Cell<f64>>,
    clips: Vec<Clip>,
    aud: Aud,
    fps: f64,
    end: f64,
    rev: u64,
    playhead: Time,
    loop_range: Option<(Time, Time)>,
    max_q: usize,
    /// Single frames in progress like the dispatcher: (layer, key, ready at, source time); per layer one
    /// running and at most the newest waiting (`still_wait`: key, source time)
    stills: Vec<(Lid, u64, f64, f64)>,
    still_wait: HashMap<Lid, (u64, f64)>,
    /// single-frame decodes started and killed
    still_starts: u32,
    still_kills: u32,
    still_time: f64,
    /// opened, not yet attached streams (starter thread): (ready at, job, decoder)
    opening: Vec<(f64, Open, Dec, u64, u64)>,
    /// last shown stream frame per layer (timeline time)
    last_frame: HashMap<Lid, f64>,
    /// (wall time, place, layer, frame time) of all shown stream frames
    shown: Vec<(f64, f64, Lid, f64)>,
    stills_shown: u32,
    ever_shown: bool,
    jumps: u64,
    last_pos: Time,
    opens: u32,
    level: usize,
    level_changes: u32,
    attached: Vec<(Open, bool)>,
    /// Wall time of attaching per (layer, key)
    attached_at: HashMap<(Lid, u64), f64>,
}

impl World {
    fn new(clips: Vec<Clip>) -> Self {
        let end = clips.iter().map(|c| c.end).fold(0.0, f64::max);
        Self {
            core: PlayCore::default(),
            clock: Rc::new(Cell::new(0.0)),
            clips,
            aud: Aud::new(),
            fps: 30.0,
            end,
            rev: 0,
            playhead: Time::ZERO,
            loop_range: None,
            max_q: 0,
            stills: Vec::new(),
            still_wait: HashMap::new(),
            still_starts: 0,
            still_kills: 0,
            still_time: 0.08,
            opening: Vec::new(),
            last_frame: HashMap::new(),
            shown: Vec::new(),
            stills_shown: 0,
            ever_shown: false,
            jumps: 0,
            last_pos: Time::ZERO,
            opens: 0,
            level: 0,
            level_changes: 0,
            attached: Vec::new(),
            attached_at: HashMap::new(),
        }
    }
    fn now(&self) -> f64 {
        self.clock.get()
    }
    fn cmds(&mut self) {
        let now = self.now();
        for c in self.core.cmds() {
            self.aud.apply(c, now);
        }
        self.playhead = self.core.pos();
    }
    fn play(&mut self) {
        let now = self.now();
        self.core.play(self.playhead, self.rev, now);
        self.cmds();
    }
    fn pause(&mut self) {
        let running = matches!(self.aud.input(self.now()), AudioIn::Running(_));
        self.core.pause(self.rev, running, true);
        self.cmds();
    }
    fn key(&self, c: &Clip) -> u64 {
        (c.id as u64) << 20 | c.ver << 4 | self.core.quality.level as u64
    }

    fn step(&mut self, dt: f64) {
        let now = self.now() + dt;
        self.clock.set(now);
        let t = Tick {
            now,
            playhead: self.playhead,
            revision: self.rev,
            audio: self.aud.input(now),
            end: Time::from_secs_f64(self.end),
            fps: self.fps,
            loop_range: self.loop_range,
            recording: false,
            max_quality_level: self.max_q,
        };
        let pos = self.core.tick(&t);
        self.cmds();
        // the clock never runs backwards except on a jump (or reverse playback)
        if self.core.playing() && self.core.jumps() == self.jumps && self.core.rate() > 0.0 {
            assert!(pos >= self.last_pos, "clock back without a jump: {:?} → {:?}", self.last_pos, pos);
        }
        if self.core.jumps() != self.jumps {
            self.jumps = self.core.jumps();
            self.last_frame.clear();
        }
        self.last_pos = pos;
        if self.core.quality.level != self.level {
            self.level = self.core.quality.level;
            self.level_changes += 1;
        }
        // starter thread: attach finished streams (old projects are rejected)
        let (ready, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.opening).into_iter().partition(|o| o.0 <= now);
        self.opening = rest;
        for (_, o, d, level, alias) in ready {
            let per_frame = d.per_frame / 2f64.powi(level as i32 * 2);
            let sim = Sim { clock: self.clock.clone(), opened: now, d, per_frame, n: 0, t0: o.at.as_secs_f64(), fps: self.fps };
            let accepted = self.core.attach_alias(o, self.fps, sim, now, alias);
            assert!(!accepted || o.gen == self.core.gen(), "stream of an old project attached");
            if accepted {
                self.attached_at.insert((o.id, o.key), now);
            }
            self.attached.push((o, accepted));
        }
        let p = pos.as_secs_f64();
        let mut vis = Vec::new();
        let clips = self.clips.clone();
        for (i, c) in clips.iter().enumerate() {
            let active = c.start - c.lead_in <= p && p < c.end;
            let soon = self.core.streaming() && c.start > p && c.start - p <= 1.5;
            if !active && !soon {
                continue;
            }
            vis.push(c.id);
            if self.core.streaming() && !c.still_only {
                let start = c.want_start.filter(|_| active).unwrap_or(c.start);
                let w = Want { id: c.id, key: self.key(c), start: Time::from_secs_f64(start), end: Time::from_secs_f64(c.end), prefetch: !active, alias: c.alias };
                if let Some(o) = self.core.want(&w, now) {
                    self.opens += 1;
                    if c.dec.fail_open {
                        self.core.open_failed(o, now);
                    } else {
                        self.opening.push((now + 0.004, o, c.dec, self.core.quality.level as u64, c.alias));
                    }
                }
                if active {
                    let replacement = |core: &PlayCore<Sim>| core.streams().into_iter().find(|x| x.0 == c.id && x.2).map(|x| x.1);
                    let had = replacement(&self.core);
                    if let Some(f) = self.core.pull(c.id, now, i == 0) {
                        // from the future at most the first frame of a clip already visible in the transition
                        assert!(f <= p.max(c.start) + 1e-6, "frame from the future: {f} at {p}");
                        // a replacement that takes over with lag after REPLACE_AFTER or after the end of the old
                        // stream (`len`) may be older
                        let late = had.is_some_and(|k| {
                            replacement(&self.core).is_none() && (c.dec.len.is_some() || self.attached_at.get(&(c.id, k)).is_some_and(|t| now - t > REPLACE_AFTER))
                        });
                        if let Some(&last) = self.last_frame.get(&c.id).filter(|_| !late) {
                            assert!(f >= last - 1.01 / self.fps, "older frame shown: {f} after {last}");
                        }
                        self.last_frame.insert(c.id, f);
                        self.shown.push((now, p, c.id, f));
                        self.ever_shown = true;
                    }
                }
            } else if active {
                let frame = ((p - c.start) * self.fps) as u64;
                let key = self.key(c) ^ frame << 32;
                if self.core.still(c.id, key, now) {
                    self.offer_still(c.id, key, p - c.start, now);
                }
            }
        }
        let (done, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.stills).into_iter().partition(|s| s.2 <= now);
        self.stills = rest;
        for (id, key, _, _) in done {
            // a newer job waited: the finished frame comes as neighbour, the waiting one starts
            let next = self.still_wait.remove(&id);
            let shown = if next.is_some() { self.core.still_near(id, key) } else { self.core.still_done(id, key) };
            if shown {
                self.stills_shown += 1;
                self.ever_shown = true;
            }
            if let Some((k, at)) = next {
                self.still_starts += 1;
                self.stills.push((id, k, now + self.still_time, at));
            }
        }
        self.core.visible(&vis);
        self.check();
    }

    /// New single-frame job, decided like the dispatcher (`preview::next_still`)
    fn offer_still(&mut self, id: Lid, key: u64, at: f64, now: f64) {
        use crate::preview::{next_still, Next};
        let (path, t) = (std::path::Path::new("x.mp4"), Time::from_secs_f64);
        let run = self.stills.iter().find(|s| s.0 == id).map(|s| (s.1, s.3));
        let prev = self.still_wait.get(&id).map(|w| w.1).or(run.map(|r| r.1)).unwrap_or(at);
        match next_still(run.map(|r| (r.0, path, t(r.1))), t(prev), key, path, t(at)) {
            Next::Same => {
                self.still_wait.remove(&id);
            }
            Next::Wait => {
                self.still_wait.insert(id, (key, at));
            }
            n => {
                self.still_kills += (n == Next::Replace) as u32;
                self.stills.retain(|s| s.0 != id);
                self.still_wait.remove(&id);
                self.still_starts += 1;
                self.stills.push((id, key, now + self.still_time, at));
            }
        }
    }

    fn check(&self) {
        // per layer at most one running stream and one replacement about to take over
        let s = self.core.streams();
        for (id, _, _) in &s {
            let same: Vec<_> = s.iter().filter(|x| x.0 == *id).collect();
            assert!(same.len() <= 2, "more than two streams for {id}");
            assert!(same.iter().filter(|x| !x.2).count() == 1, "two running streams for {id}");
        }
        if self.ever_shown {
            assert!(self.core.held().is_some(), "no displayable frame any more");
        }
    }

    fn run(&mut self, secs: f64, dt: f64) {
        let n = (secs / dt).round() as usize;
        for _ in 0..n {
            self.step(dt);
        }
    }

    /// Largest gap between two shown frames of layer `id` in the period (wall time)
    fn gap(&self, id: Lid, from: f64, to: f64) -> f64 {
        let t: Vec<f64> = self.shown.iter().filter(|s| s.2 == id && s.0 >= from && s.0 <= to).map(|s| s.0).collect();
        t.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max)
    }
    fn frames(&self, from: f64, to: f64) -> usize {
        self.shown.iter().filter(|s| s.0 >= from && s.0 <= to).count()
    }
}

const DT: f64 = 1.0 / 120.0;

#[test]
fn play_from_stand_buffers_then_runs_with_audio_clock() {
    let mut w = World::new(vec![clip(1, 0.0, 10.0)]);
    w.step(DT);
    assert_eq!(w.core.mode(), Mode::Stand);
    w.play();
    assert_eq!(w.core.mode(), Mode::Puffern);
    assert!(w.core.waiting());
    // sound prepared but not yet come in
    assert!(matches!(w.aud.st, AState::Prep { anchor: None, .. }));
    w.run(0.2, DT);
    assert_eq!(w.core.mode(), Mode::Spielen);
    assert!(!w.core.waiting());
    let t_play = w.shown.first().unwrap().0;
    assert!(t_play < 0.1, "first frame after {t_play} s");
    w.run(3.0, DT);
    // clock = audio clock, runs in real time; 30 fps without a gap over 2 intervals
    assert!(matches!(w.aud.input(w.now()), AudioIn::Running(_)));
    let ran = w.core.pos().as_secs_f64();
    assert!((ran - 3.0).abs() < 0.15, "clock {ran}");
    assert!(w.gap(1, 0.5, 3.0) <= 2.0 / 30.0 + 0.01);
    assert!(w.frames(1.0, 3.0) >= 58);
    let st = w.core.take_stats();
    assert_eq!(st.catch_ups, 0);
    assert_eq!(st.opens, 1);
}

/// Cold disk: the first audio window takes 3 s. The clock waits for it instead of running seconds on
/// system time (the sound then chased it – or never caught up).
#[test]
fn clock_waits_for_late_first_audio() {
    let mut w = World::new(vec![clip(1, 0.0, 10.0)]);
    w.aud.latency = 3.0;
    w.play();
    w.run(2.9, DT);
    assert_eq!(w.core.mode(), Mode::Puffern, "clock runs without sound");
    assert_eq!(w.core.pos(), Time::ZERO);
    w.run(0.2, DT);
    assert_eq!(w.core.mode(), Mode::Spielen);
    assert!(matches!(w.aud.input(w.now()), AudioIn::Running(_)), "sound comes in with the clock");
}

/// If no sound comes at all, the clock still runs after the safety limit (no hang).
#[test]
fn clock_starts_without_audio_after_wait_limit() {
    let mut w = World::new(vec![clip(1, 0.0, 60.0)]);
    w.aud.latency = 1e9;
    w.play();
    w.run(AUDIO_WAIT_MAX + 0.1, DT);
    assert_eq!(w.core.mode(), Mode::Spielen);
}

#[test]
fn warmup_ends_after_deadline_if_decoder_never_delivers() {
    let mut c = clip(1, 0.0, 10.0);
    c.dec.never = true;
    let mut w = World::new(vec![c]);
    w.play();
    w.run(0.4, DT);
    assert_eq!(w.core.mode(), Mode::Puffern);
    w.run(0.2, DT);
    assert_eq!(w.core.mode(), Mode::Spielen, "no hang: the clock runs after 0.5 s");
    w.run(2.0, DT);
    assert!(w.core.pos().as_secs_f64() > 1.5);
}

#[test]
fn never_delivering_stream_holds_last_frame_and_is_reopened() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    assert!(w.ever_shown);
    // from now on the decoder delivers nothing
    w.clips[0].dec.never = true;
    w.clips[0].ver = 1; // new key → replacement stream that never delivers
    w.run(3.0, DT);
    assert!(w.core.held().is_some(), "held frame instead of black");
    assert_eq!(w.core.mode(), Mode::Spielen);
    // old stream keeps showing until the replacement delivers: frames keep running
    assert!(w.frames(1.5, 4.0) > 50);
}

#[test]
fn stalled_stream_is_reopened_at_current_position() {
    let mut c = clip(1, 0.0, 30.0);
    c.dec.len = Some(40); // stream ends prematurely (FFmpeg aborted)
    let mut w = World::new(vec![c]);
    w.play();
    w.run(3.0, DT);
    let st = w.core.take_stats();
    assert!(st.stalls >= 1, "{st:?}");
    assert!(w.opens >= 2);
    assert_eq!(w.core.mode(), Mode::Spielen);
}

#[test]
fn click_during_play_rebuffers_at_new_position() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    let starts = w.aud.starts;
    w.playhead = Time::from_secs_f64(12.0); // click in the timeline
    w.step(DT);
    assert_eq!(w.core.mode(), Mode::Puffern);
    assert_eq!(w.aud.starts, starts + 1, "sound from the new place");
    w.run(0.5, DT);
    assert_eq!(w.core.mode(), Mode::Spielen);
    let (_, p, _, f) = *w.shown.last().unwrap();
    assert!(p >= 12.0 && f >= 12.0 && p - f < 0.1, "frame {f} at {p}");
}

/// Click test K5: dragging a slider during Play – a new key per UI frame, per frame a new
/// stream that was discarded right away (the running one lacked computing time, picture stood 1.8 s).
/// Now the old stream keeps running with the old value; it is reopened once the value stays.
#[test]
fn dragging_a_slider_during_play_does_not_open_a_stream_per_frame() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    let (opens, t) = (w.opens, w.now());
    for _ in 0..90 {
        w.clips[0].ver += 1; // a new value per UI frame (for 1.5 s)
        w.step(1.0 / 60.0);
    }
    assert!(w.opens - opens <= 1, "{} streams while dragging", w.opens - opens);
    assert!(w.gap(1, t, w.now()) <= 2.0 / 30.0 + 0.01, "picture keeps running with the old value");
    // value stays: the replacement comes and takes over
    w.run(1.0, DT);
    let new = w.key(&w.clips[0]);
    assert!(w.core.streams().iter().all(|x| x.1 == new), "{:?}", w.core.streams());
    assert!(w.opens - opens <= 2);
}

#[test]
fn visible_edit_keeps_picture_running_and_switches_stream() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.clips[0].dec.delay = 0.3; // new stream takes long
    w.play();
    w.run(1.0, DT);
    w.clips[0].ver = 1; // effect changed
    w.rev += 1;
    w.core.edit(false, w.now());
    let t = w.now();
    w.run(1.5, DT);
    assert!(w.gap(1, t, t + 1.5) <= 2.0 / 30.0 + 0.01, "picture stood while editing");
    let s = w.core.streams();
    assert_eq!(s.len(), 1, "old stream freed after the change: {s:?}");
    assert_eq!(s[0].1, w.key(&w.clips[0]));
}

/// Review finding: a replacement slower than real time (heavy effect added during Play)
/// never took over – the change stayed invisible until the clip end.
#[test]
fn slow_replacement_takes_over_after_a_second() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    w.clips[0].ver = 1;
    w.clips[0].dec.per_frame = 1.0 / 24.0; // 0.8× real time at 30 fps
    w.rev += 1;
    w.core.edit(false, w.now());
    w.run(2.5, DT);
    let new = w.key(&w.clips[0]);
    let s = w.core.streams();
    assert!(s.iter().any(|x| !x.2 && x.1 == new), "change visible: {s:?}");
    assert!(s.iter().all(|x| x.1 == new), "old stream freed: {s:?}");
}

/// Review finding: a replacement that ends without a frame (effect graph fails) stayed
/// forever and was never retried.
#[test]
fn replacement_ending_without_a_frame_is_retried() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    w.clips[0].ver = 1;
    w.clips[0].dec.len = Some(0); // FFmpeg ends at once
    w.rev += 1;
    let (opens, t) = (w.opens, w.now());
    w.run(2.5, DT);
    assert!(w.opens >= opens + 2, "retried: {}", w.opens - opens);
    assert!(w.opens <= opens + 4, "not in every frame: {}", w.opens - opens);
    assert!(w.gap(1, t, w.now()) <= 2.0 / 30.0 + 0.01, "old stream keeps showing");
    w.clips[0].dec.len = None; // works again
    w.run(1.5, DT);
    let new = w.key(&w.clips[0]);
    assert!(w.core.streams().iter().all(|x| x.1 == new), "{:?}", w.core.streams());
}

#[test]
fn audible_edits_restart_audio_once_after_debounce() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    let starts = w.aud.starts;
    // dragging the volume slider: 10 changes 50 ms apart
    for _ in 0..10 {
        w.core.edit(true, w.now());
        w.run(0.05, DT);
    }
    assert_eq!(w.aud.remixes, 0, "sound not renewed at every step");
    w.run(0.2, DT);
    assert_eq!(w.aud.remixes, 1);
    w.run(0.5, DT);
    assert_eq!(w.aud.starts, starts, "running sound is rebuilt, not stopped and started anew");
    assert!(matches!(w.aud.input(w.now()), AudioIn::Running(_)));
    assert!(w.gap(1, 1.0, w.now()) <= 2.0 / 30.0 + 0.01, "picture kept running");
}

/// An audible edit while the sound runs must not stop it: every tick from the edit on the sound
/// stays audible (the old one plays until the rebuilt one takes over) and the clock keeps following it.
#[test]
fn audible_edit_keeps_running_sound_without_silence() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.aud.latency = 0.4; // FFmpeg start of the new mix
    w.play();
    w.run(1.0, DT);
    assert!(matches!(w.aud.input(w.now()), AudioIn::Running(_)));
    w.core.edit(true, w.now());
    let mut silent = 0;
    for _ in 0..60 {
        w.step(DT);
        if !matches!(w.aud.input(w.now()), AudioIn::Running(_)) {
            silent += 1;
        }
    }
    assert_eq!(silent, 0, "ticks without sound after an audible edit");
    assert_eq!(w.aud.remixes, 1);
    // the clock still is the sound's clock
    let AudioIn::Running(p) = w.aud.input(w.now()) else { unreachable!() };
    assert_eq!(w.core.pos(), p);
}

/// Edit while no sound plays (here: output failed): nothing to keep, it starts anew (as before).
#[test]
fn audible_edit_without_running_sound_starts_it_anew() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.aud.fail = true;
    w.play();
    w.run(1.0, DT);
    let starts = w.aud.starts;
    w.core.edit(true, w.now());
    w.run(0.3, DT);
    assert_eq!((w.aud.starts, w.aud.remixes), (starts + 1, 0));
}

#[test]
fn cuts_between_many_short_clips_have_no_gap() {
    let clips: Vec<Clip> = (0..30).map(|i| clip(i + 1, i as f64 * 0.5, (i + 1) as f64 * 0.5)).collect();
    let mut w = World::new(clips);
    w.play();
    w.run(14.0, DT);
    let t: Vec<f64> = w.shown.iter().filter(|s| s.0 > 0.3).map(|s| s.0).collect();
    let gap = t.windows(2).map(|x| x[1] - x[0]).fold(0.0, f64::max);
    assert!(gap <= 2.0 / 30.0 + 0.01, "gap {gap} s at the cut");
    // one stream per clip (opened ahead), no restarts
    assert!(w.opens <= 31, "{} streams", w.opens);
}

#[test]
fn several_tracks_all_deliver() {
    let mut w = World::new(vec![clip(1, 0.0, 10.0), clip(2, 0.0, 10.0), clip(3, 2.0, 6.0)]);
    w.play();
    w.run(5.0, DT);
    for id in [1, 2, 3] {
        let from = if id == 3 { 2.2 } else { 0.5 };
        assert!(w.gap(id, from, 5.0) <= 2.0 / 30.0 + 0.01, "layer {id}");
    }
}

#[test]
fn project_end_stops_on_last_frame() {
    let mut w = World::new(vec![clip(1, 0.0, 2.0)]);
    w.play();
    w.run(3.0, DT);
    assert_eq!(w.core.mode(), Mode::Stand);
    assert_eq!(w.playhead, last_frame(Time::from_secs_f64(2.0), 30.0));
    assert!(w.core.streams().is_empty());
    assert_eq!(w.aud.st, AState::Off);
}

#[test]
fn loop_range_jumps_back_and_keeps_playing() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.loop_range = Some((Time::from_secs_f64(1.0), Time::from_secs_f64(2.0)));
    w.playhead = Time::from_secs_f64(1.0);
    w.play();
    w.run(3.5, DT);
    assert_eq!(w.core.mode(), Mode::Spielen);
    let p = w.core.pos().as_secs_f64();
    assert!((1.0..2.0).contains(&p), "{p}");
    assert!(w.core.jumps() >= 3);
}

/// Follow-up test K7: in the pause the UI hardly draws (no tick); on resume the whole
/// pause counted as a measurement window without frames – the automatic quality dropped a level and
/// opened replacement streams. After the pause: no lag, no level, no new stream.
#[test]
fn resume_after_a_long_pause_without_ticks_keeps_quality_and_streams() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.max_q = 3;
    w.play();
    w.run(2.0, DT);
    w.pause();
    w.step(DT);
    let opens = w.opens;
    w.clock.set(w.now() + 3.0); // 3 s pause without a single UI frame
    w.play();
    w.run(3.0, DT);
    assert_eq!(w.core.quality.level, 0, "level stays");
    assert_eq!(w.opens, opens, "no new stream");
    let st = w.core.take_stats();
    assert_eq!(st.catch_ups, 0);
}

#[test]
fn pause_and_resume_at_same_place_opens_no_new_streams() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.5, DT);
    w.pause();
    assert_eq!(w.core.mode(), Mode::Pause);
    let at = w.core.pos();
    let opens = w.opens;
    w.run(1.0, DT);
    assert_eq!(w.core.pos(), at, "clock stands in the pause");
    w.play();
    assert_eq!(w.core.mode(), Mode::Spielen, "straight on, without buffering");
    let t = w.now();
    w.run(1.0, DT);
    assert_eq!(w.opens, opens, "no new streams");
    assert!(w.gap(1, t, t + 1.0) <= 2.0 / 30.0 + 0.01);
    assert!((w.core.pos() - at).as_secs_f64() > 0.9);
}

#[test]
fn pause_then_move_frees_streams_and_play_rebuffers() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    w.pause();
    w.playhead = Time::from_secs_f64(5.0); // moved
    w.step(DT);
    assert_eq!(w.core.mode(), Mode::Stand);
    assert!(w.core.streams().is_empty());
    w.play();
    assert_eq!(w.core.mode(), Mode::Puffern);
    w.run(0.5, DT);
    assert!(w.shown.last().unwrap().3 >= 5.0);
}

#[test]
fn pause_after_edit_does_not_resume() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    w.pause();
    w.rev += 1;
    w.step(DT);
    assert_eq!(w.core.mode(), Mode::Stand);
}

#[test]
fn pause_without_audio_stops() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.aud.fail = true;
    w.play();
    w.run(1.0, DT);
    w.pause();
    assert_eq!(w.core.mode(), Mode::Stand);
}

#[test]
fn project_change_mid_play_drops_everything_of_the_old_project() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    w.clips[0].ver = 7; // replacement is being opened (after the key stayed for a moment) …
    for _ in 0..40 {
        if !w.opening.is_empty() {
            break;
        }
        w.step(DT);
    }
    assert!(!w.opening.is_empty());
    let gen = w.core.gen();
    w.core.new_project(); // … then the project changes
    w.cmds();
    assert_eq!(w.core.gen(), gen + 1);
    assert_eq!(w.core.mode(), Mode::Stand);
    assert!(w.core.held().is_none());
    w.ever_shown = false;
    w.step(DT); // attaches the old one: is rejected (assertion in the driver)
    assert!(w.core.streams().is_empty());
    assert_eq!(w.aud.st, AState::Off);
}

#[test]
fn audio_failure_lets_picture_run_on_system_clock() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.aud.cut_at = Some(1.0); // sound aborts after 1 s
    w.play();
    w.run(3.0, DT);
    assert_eq!(w.core.mode(), Mode::Spielen);
    let p = w.core.pos().as_secs_f64();
    assert!((p - 3.0).abs() < 0.15, "clock {p}");
    assert!(w.gap(1, 0.5, 3.0) <= 2.0 / 30.0 + 0.01);
    assert_eq!(w.aud.st, AState::Off, "sound released");
}

#[test]
fn audio_start_failure_is_not_a_hang() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.aud.fail = true;
    w.play();
    w.run(2.0, DT);
    assert!((w.core.pos().as_secs_f64() - 1.9).abs() < 0.2);
}

#[test]
fn shuttle_runs_on_stills_at_rate_and_stops_at_zero_backwards() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.playhead = Time::from_secs_f64(2.0);
    w.core.shuttle(2.0, w.playhead, w.now());
    w.cmds();
    assert!(!w.core.streaming());
    w.run(1.0, DT);
    assert!((w.core.pos().as_secs_f64() - 4.0).abs() < 0.05);
    assert!(w.core.streams().is_empty(), "no streams at speed ≠ 1");
    // single frames keep coming (the running job is not constantly aborted)
    assert!(w.stills_shown >= 8, "{} single frames", w.stills_shown);
    w.core.shuttle(-2.0, w.playhead, w.now());
    w.cmds();
    w.run(2.5, DT);
    assert_eq!(w.core.mode(), Mode::Stand);
    assert_eq!(w.core.pos(), Time::ZERO);
}

#[test]
fn reverse_clip_during_play_shows_stills_and_never_goes_black() {
    let mut c = clip(2, 1.0, 4.0);
    c.still_only = true; // reverse clip
    let mut w = World::new(vec![clip(1, 0.0, 1.0), c, clip(3, 4.0, 6.0)]);
    w.still_time = 0.15; // slow single frame
    w.play();
    w.run(5.5, DT);
    assert!(w.stills_shown >= 10, "{} single frames in the reverse clip", w.stills_shown);
    assert!(w.core.held().is_some());
}

#[test]
fn slow_stream_catches_up_instead_of_drifting() {
    let mut c = clip(1, 0.0, 60.0);
    c.dec.per_frame = 1.0 / 40.0; // 40 instead of 60 fps
    c.dec.delay = 0.15;
    let mut w = World::new(vec![c]);
    w.fps = 60.0;
    w.play();
    w.run(10.0, DT);
    let st = w.core.take_stats();
    assert!(st.catch_ups >= 1, "{st:?}");
    // without automatic quality: lag stays bounded (without catching up it would grow beyond 3 s),
    // reopened at most every 5 s
    assert!(st.max_lag < 2.0, "lag {} s", st.max_lag);
    assert!(st.catch_ups <= 2, "{st:?}");
}

#[test]
fn auto_quality_restores_frame_rate_of_slow_stream() {
    let mut c = clip(1, 0.0, 60.0);
    c.dec.per_frame = 1.0 / 40.0; // full: 40 fps; ½: 160 fps
    let mut w = World::new(vec![c]);
    w.fps = 60.0;
    w.max_q = 3;
    w.play();
    w.run(3.0, DT);
    assert!(w.core.quality.level >= 1, "one level smaller after 1–2 s");
    w.run(27.0, DT);
    // attempts to compute at full size again become rarer (4, 8, 16 s): hardly any loss, no flapping
    let n = w.frames(2.0, 30.0) as f64 / 28.0;
    assert!(n >= 55.0, "{n} fps with automatic quality");
    assert!(w.level_changes <= 6, "{} changes", w.level_changes);
    // when stopped back to full
    w.core.stop();
    w.cmds();
    w.step(DT);
    assert_eq!(w.core.quality.level, 0);
}

#[test]
fn auto_quality_off_keeps_level() {
    let mut c = clip(1, 0.0, 60.0);
    c.dec.per_frame = 1.0 / 40.0;
    let mut w = World::new(vec![c]);
    w.fps = 60.0;
    w.max_q = 0;
    w.play();
    w.run(4.0, DT);
    assert_eq!(w.core.quality.level, 0);
}

#[test]
fn quality_decisions_have_hysteresis() {
    let mut q = AutoQuality::default();
    // too slow → one level, then quiet during the switch
    assert_eq!(q.decide(1.0, Some(0.8), 3, true), 1);
    assert_eq!(q.decide(2.0, Some(0.5), 3, true), 1);
    assert_eq!(q.decide(3.0, Some(0.5), 3, true), 2);
    // never above the highest level
    assert_eq!(q.decide(5.0, Some(0.5), 2, true), 2);
    // enough again: back only after 4 s in a row
    for t in 6..10 {
        assert_eq!(q.decide(t as f64, Some(1.0), 2, true), 2, "t={t}");
    }
    assert_eq!(q.decide(10.0, Some(1.0), 2, true), 1);
    // stepping up fails again at once → next attempt only after 8 s
    assert_eq!(q.decide(12.0, Some(0.7), 2, true), 2);
    for t in 14..21 {
        assert_eq!(q.decide(t as f64, Some(1.0), 2, true), 2, "t={t}");
    }
    assert_eq!(q.decide(22.0, Some(1.0), 2, true), 1);
    // between the thresholds: stays, good time restarts
    assert_eq!(q.decide(24.0, Some(0.95), 2, true), 1);
    // stopped: full at once
    assert_eq!(q.decide(25.0, None, 2, false), 0);
    assert_eq!(q.level, 0);
    // off: always 0
    assert_eq!(q.decide(26.0, Some(0.1), 0, true), 0);
}

#[test]
fn quality_steps_respect_user_choice_as_upper_bound() {
    assert_eq!(quality_steps(1, 30.0), vec![(1, false), (2, false), (4, false)]);
    assert_eq!(quality_steps(1, 60.0), vec![(1, false), (2, false), (4, false), (4, true)]);
    assert_eq!(quality_steps(2, 25.0), vec![(2, false), (4, false)]);
    assert_eq!(quality_steps(4, 30.0), vec![(4, false)]);
    assert_eq!(quality_steps(4, 59.94), vec![(4, false), (4, true)]);
}

#[test]
fn open_failure_retries_after_a_while() {
    let mut c = clip(1, 0.0, 30.0);
    c.dec.fail_open = true;
    let mut w = World::new(vec![c]);
    w.play();
    w.run(2.5, DT);
    assert!((2..=4).contains(&w.opens), "{} attempts", w.opens);
    assert_eq!(w.core.mode(), Mode::Spielen);
}

/// Stand (scrubbing), changed deliberately with "newest wins": the core still requests every new
/// position (the dispatcher lets the running decode finish and keeps only the newest waiting), and an
/// overtaken frame is shown as neighbour instead of being thrown away. The request stays open until
/// its exact frame comes: the wait line does not flicker, a neighbour never covers the exact frame.
#[test]
fn stills_in_stand_show_overtaken_frames_as_neighbours_until_the_exact_one() {
    let mut core: PlayCore<Sim> = PlayCore::default();
    assert!(!core.waiting());
    assert!(core.still(1, 10, 0.0));
    assert!(core.waiting(), "requested, nothing there yet");
    assert!(!core.still(1, 10, 0.01), "the same job is already running");
    // scrubbing: the newer one is requested at once
    assert!(core.still(1, 11, 0.02));
    assert!(!core.still_done(1, 10), "overtaken frame is not the exact one");
    assert!(core.still_near(1, 10), "but it bridges as neighbour");
    assert_eq!(core.held(), Some(1));
    assert!(core.waiting(), "still waiting for the exact frame: line stays on");
    assert!(!core.still(1, 11, 0.1), "no repeat while the exact frame is young");
    assert!(core.still(1, 11, 0.6), "overdue: asked again");
    assert!(!core.still(1, 11, 0.7));
    assert!(core.still_done(1, 11));
    assert!(!core.waiting());
    assert!(!core.still_near(1, 10), "a late neighbour never covers the exact frame");
    assert!(!core.still(1, 11, 5.0), "exact frame stays, no repeat");
    // project changed: request again, picture stays
    core.forget_stills();
    assert!(core.still(1, 11, 0.1));
    core.visible(&[]);
    assert!(!core.waiting());
    assert_eq!(core.held(), Some(1));
}

/// World with a slow single-frame decoder (80 ms): drag the playhead one frame per 16-ms UI tick.
/// Before, every new position killed the running decode and nothing came until the pointer rested.
fn drag(w: &mut World, from: f64, step: f64, ticks: usize) -> (u32, u32) {
    let (shown, mut gaps) = (w.stills_shown, 0);
    for i in 0..ticks {
        w.playhead = Time::from_secs_f64(from + i as f64 * step);
        w.step(0.016);
        gaps += (i > 0 && !w.core.waiting()) as u32;
    }
    (w.stills_shown - shown, gaps)
}

#[test]
fn scrubbing_finishes_running_stills_and_ends_on_the_exact_frame() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    let (shown, line_off) = drag(&mut w, 2.0, 1.0 / 30.0, 60);
    // ~0.96 s at 80 ms per decode: about 12 decodes finish, at most one waits at any time
    assert!(shown >= 8, "{shown} frames while dragging");
    assert!(w.still_starts <= 14, "{} decodes started for 60 positions", w.still_starts);
    assert_eq!(w.still_kills, 0, "dragging kills nothing");
    assert_eq!(line_off, 0, "wait line stays on while dragging (no flicker)");
    assert!(w.stills.len() <= 1 && w.still_wait.len() <= 1);
    // rest: the exact frame of the last position comes without any further input
    let starts = w.still_starts;
    w.run(0.3, 0.016);
    assert!(!w.core.waiting(), "exact frame at rest");
    assert!(w.still_starts <= starts + 1, "at most the waiting one started");
    assert!(w.stills.is_empty() && w.still_wait.is_empty());
}

#[test]
fn far_jump_in_stand_kills_the_running_still() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.playhead = Time::from_secs_f64(1.0);
    w.step(0.016);
    w.playhead = Time::from_secs_f64(20.0);
    w.step(0.016);
    assert_eq!((w.still_starts, w.still_kills), (2, 1));
    w.run(0.2, 0.016);
    assert!(!w.core.waiting());
    // a short click back: the running decode is close, it is allowed to finish
    w.playhead = Time::from_secs_f64(19.5);
    w.step(0.016);
    w.playhead = Time::from_secs_f64(19.8);
    w.step(0.016);
    assert_eq!(w.still_kills, 1);
    w.run(0.3, 0.016);
    assert!(!w.core.waiting());
}

/// Review finding: a failed single frame stayed as a job – loading line
/// forever, during playback the picture stood until `STILL_PATIENCE`.
#[test]
fn failed_still_ends_the_request_and_the_next_is_requested() {
    let mut core: PlayCore<Sim> = PlayCore::default();
    assert!(core.still(1, 10, 0.0));
    assert!(core.has_requests());
    core.still_failed(1, 9); // stale: changes nothing
    assert!(core.has_requests());
    core.still_failed(1, 10);
    assert!(!core.has_requests(), "loading indicator ends");
    assert!(!core.waiting());
    assert!(!core.still(1, 10, 0.1), "the same one not renewed every UI frame");
    assert!(core.still(1, 11, 0.2), "the next one is requested");
    // during playback (reverse clip): the next one at once, not only after STILL_PATIENCE
    core.shuttle(2.0, Time::ZERO, 1.0);
    assert!(core.still(1, 20, 1.0));
    assert!(!core.still(1, 21, 1.1), "running job is not replaced");
    core.still_failed(1, 20);
    assert!(core.still(1, 21, 1.2));
    // a delivered job no longer counts as open either
    assert!(core.still_done(1, 21));
    assert!(!core.has_requests());
}

/// CI finding: the reverse window answers a job with a held neighbour while playing. Shown, but
/// in the pause the same key is asked again (now without window: the exact frame) – before,
/// the neighbour stayed as the paused picture.
#[test]
fn neighbour_frame_is_asked_again_when_playback_ends() {
    let mut core: PlayCore<Sim> = PlayCore::default();
    core.shuttle(1.0, Time::ZERO, 0.0);
    assert!(core.still(1, 10, 0.0));
    assert!(core.still_near(1, 10), "shown during playback");
    assert!(!core.has_requests());
    assert!(!core.still(1, 10, 0.1), "not asked again while playing");
    core.pause(0, true, true);
    assert_eq!(core.mode(), Mode::Pause);
    assert!(core.still(1, 10, 0.2), "asked again in the pause");
    assert!(core.still_done(1, 10));
    assert!(!core.still(1, 10, 0.3), "the exact frame stays");
    // an exact frame from playback stays in the pause
    core.shuttle(1.0, Time::ZERO, 1.0);
    assert!(core.still(1, 11, 1.0));
    assert!(core.still_done(1, 11));
    core.pause(0, true, true);
    assert!(!core.still(1, 11, 1.1));
}

#[test]
fn play_during_buffering_seek_and_stop_from_every_state() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    // buffering → stop
    w.play();
    w.core.stop();
    w.cmds();
    assert_eq!(w.core.mode(), Mode::Stand);
    // pause → stop
    w.play();
    w.run(1.0, DT);
    w.pause();
    w.core.stop();
    w.cmds();
    assert_eq!(w.core.mode(), Mode::Stand);
    assert!(w.core.streams().is_empty());
    // jump when stopped moves only the place
    w.core.seek(Time::from_secs_f64(3.0), w.now());
    assert_eq!(w.core.mode(), Mode::Stand);
    assert_eq!(w.core.pos(), Time::from_secs_f64(3.0));
    // jump in the pause ends it
    w.playhead = w.core.pos();
    w.play();
    w.run(0.5, DT);
    w.pause();
    w.core.seek(Time::from_secs_f64(8.0), w.now());
    assert_eq!(w.core.mode(), Mode::Stand);
    // renew sound at once (solo, device) only during playback
    let s = w.aud.starts;
    w.core.restart_audio(w.now());
    w.cmds();
    assert_eq!(w.aud.starts, s);
}

/// Random test: 10 000 event sequences; the driver checks the invariants every tick
/// (`World::step`/`check`): never two streams with the same key per layer, no stream of an
/// old project, clock back only on jumps, no frame from the future or older than the
/// shown one, always a displayable frame once there was one.
#[test]
fn random_event_sequences_keep_invariants() {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut rnd = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n.max(1)
    };
    for _ in 0..10_000 {
        let n = 1 + rnd(5);
        let clips: Vec<Clip> = (0..n)
            .map(|i| {
                let start = rnd(40) as f64 * 0.25;
                let mut c = clip(i as Lid + 1, start, start + 0.5 + rnd(20) as f64 * 0.25);
                c.dec.delay = rnd(40) as f64 * 0.01;
                c.dec.per_frame = [0.002, 0.02, 0.03, 0.05][rnd(4) as usize];
                c.dec.never = rnd(15) == 0;
                c.dec.len = (rnd(10) == 0).then(|| rnd(60));
                c.dec.fail_open = rnd(20) == 0;
                c.still_only = rnd(8) == 0;
                c
            })
            .collect();
        let mut w = World::new(clips);
        w.fps = [24.0, 30.0, 60.0][rnd(3) as usize];
        w.max_q = rnd(4) as usize;
        w.aud.fail = rnd(10) == 0;
        w.aud.cut_at = (rnd(10) == 0).then(|| rnd(30) as f64 * 0.1);
        if rnd(5) == 0 {
            w.loop_range = Some((Time::from_secs_f64(rnd(10) as f64 * 0.5), Time::from_secs_f64(5.0 + rnd(10) as f64 * 0.5)));
        }
        for _ in 0..20 {
            match rnd(12) {
                0 | 1 | 2 => w.play(),
                3 => w.pause(),
                4 => {
                    w.core.stop();
                    w.cmds();
                }
                5 => w.playhead = Time::from_secs_f64(rnd(80) as f64 * 0.25),
                6 => {
                    let i = rnd(w.clips.len() as u64) as usize;
                    w.clips[i].ver += 1;
                    w.rev += 1;
                    w.core.edit(rnd(2) == 0, w.now());
                }
                7 => {
                    let r = [-2.0, -1.0, 2.0, 4.0][rnd(4) as usize];
                    w.core.shuttle(r, w.playhead, w.now());
                    w.cmds();
                }
                8 => {
                    w.core.new_project();
                    w.cmds();
                    w.ever_shown = false;
                }
                9 => {
                    w.core.restart_audio(w.now());
                    w.cmds();
                }
                10 => w.core.invalidate(),
                _ => {}
            }
            let steps = 1 + rnd(30);
            let dt = [DT, 1.0 / 60.0, 0.033][rnd(3) as usize];
            for _ in 0..steps {
                w.step(dt);
            }
        }
    }
}

#[test]
fn stream_opened_before_a_seek_is_not_attached() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.clock.set(w.now() + 0.001);
    w.step(0.001); // open from 0 requested, starter not done yet
    assert_eq!(w.opening.len(), 1);
    w.playhead = Time::from_secs_f64(9.0); // jump before the stream is there
    w.run(0.3, DT);
    let stale: Vec<_> = w.attached.iter().filter(|(o, _)| o.at < Time::from_secs_f64(9.0)).collect();
    assert!(stale.iter().all(|(_, ok)| !ok), "stream from before the jump attached: {stale:?}");
    assert!(w.shown.iter().all(|s| s.3 >= 9.0));
}

#[test]
fn incoming_clip_in_transition_shows_its_first_frame_before_its_start() {
    let mut b = clip(2, 2.0, 6.0);
    b.lead_in = 0.25; // transition: B is visible from 1.75 s, its stream begins at 2 s
    let mut w = World::new(vec![clip(1, 0.0, 2.25), b]);
    w.play();
    w.run(1.9, DT);
    let early: Vec<_> = w.shown.iter().filter(|s| s.2 == 2 && s.1 < 2.0).collect();
    assert!(!early.is_empty(), "B shows nothing in the transition before its start");
    assert!(early.iter().all(|s| (s.3 - 2.0).abs() < 1e-6), "only the first frame before the start");
    w.run(1.0, DT);
    assert!(w.gap(2, 2.1, 2.9) <= 2.0 / 30.0 + 0.01);
}

/// Flaw from the click test: single frames during playback (reverse, speed curve, animated images)
/// were requested anew per UI frame and the running job was ended – if one takes
/// longer than a few frames, none ever arrived (still picture with sound running).
#[test]
fn slow_stills_during_play_are_finished_and_shown() {
    let mut c = clip(1, 0.0, 20.0);
    c.still_only = true;
    let mut w = World::new(vec![c]);
    w.still_time = 1.5; // under load: 1.5 s per single frame
    w.play();
    w.run(8.0, DT);
    assert!(w.stills_shown >= 4, "{} single frames in 8 s", w.stills_shown);
    // jump: the running job belongs to the old place and is replaced
    let before = w.stills.len();
    w.playhead = Time::from_secs_f64(15.0);
    w.run(0.1, DT);
    assert!(w.stills.len() > before || w.stills.iter().any(|s| s.2 > w.now()), "new job after the jump");
}

/// Flaw from the click test: a stream that delivers nothing more was reopened every second
/// (with a long GOP it never catches up that way). Now: only after 2 s without a frame, then at most every 5 s.
#[test]
fn hanging_stream_is_reopened_rarely() {
    let mut c = clip(1, 0.0, 60.0);
    c.dec.len = Some(30); // delivers for one second, then nothing more (FFmpeg ended)
    c.dec.delay = 0.1;
    let mut w = World::new(vec![c]);
    w.play();
    w.run(20.0, DT);
    // first stream + restarts after the end, each at most every 5 s
    assert!(w.opens <= 6, "{} streams in 20 s", w.opens);
    assert!(w.opens >= 3);
}

/// Flaw from the click test: splitting during Play stopped the picture (new clip = new stream from
/// the keyframe). Now the right part takes over the running stream of the same source.
#[test]
fn split_during_play_takes_over_the_running_stream() {
    let mut a = clip(1, 0.0, 10.0);
    a.alias = 77;
    a.dec.delay = 1.5; // new stream would need 1.5 s (long GOP)
    let mut w = World::new(vec![a]);
    w.play();
    w.run(1.0 + w.clips[0].dec.delay, DT);
    // split at the playhead: clip 1 stays on the left (up to here), new clip 2 from the same source on the right
    let cut = w.core.pos().as_secs_f64();
    let mut b = w.clips[0].clone();
    (b.id, b.start) = (2, cut);
    w.clips[0].end = cut;
    w.clips.push(b);
    w.rev += 1;
    w.core.edit(true, w.now());
    let (opens, t) = (w.opens, w.now());
    w.run(2.0, DT);
    assert_eq!(w.opens, opens, "no new stream");
    let gap = w.shown.iter().filter(|s| s.0 >= t - 0.1).map(|s| s.0).collect::<Vec<_>>().windows(2).map(|x| x[1] - x[0]).fold(0.0, f64::max);
    assert!(gap <= 2.0 / 30.0 + 0.01, "gap {gap} s after splitting");
}
/// Source switch proxy parts -> original during Play (PL_gross, ~2 s after Play): the parts stream
/// runs on until the parts end, the original needs far longer to start (here 1.5 s) than the parts
/// stream's lead. Before, the replacement opened at clock + old lead, was taken over after
/// REPLACE_AFTER 1.4 s behind the sound, and the quality dropped. Now it opens where it is needed.
#[test]
fn source_switch_opens_the_replacement_where_it_is_needed() {
    let mut c = clip(1, 0.0, 20.0);
    c.dec.len = Some(7 * 30); // parts end at 7 s
    let mut w = World::new(vec![c]);
    w.max_q = 2;
    w.play();
    w.run(4.0, DT);
    // switch to the original at ~4 s, needed from the parts end
    w.clips[0].ver += 1;
    w.clips[0].dec = Dec { delay: 1.5, ..Dec::default() };
    w.clips[0].want_start = Some(7.0);
    let (t, mut lag) = (w.now(), 0.0f64);
    for _ in 0..(5.0 / DT) as usize {
        w.step(DT);
        lag = lag.max(w.core.av_offset_ms().unwrap_or(0.0));
    }
    assert!(w.core.pos().as_secs_f64() > 8.5);
    assert!(lag < 100.0, "picture {lag:.0} ms behind the sound after the switch");
    assert_eq!(w.level_changes, 0, "quality dropped");
    assert!(w.gap(1, t, w.now()) <= 2.0 / 30.0 + 0.01, "gap {} s", w.gap(1, t, w.now()));
}
/// A stream opened ahead (source switch) measures its real startup, not the wait for its turn: the next
/// replacement (quality level) opens at clock + this lead. With the wait (2.9 s in PL_gross) the slow
/// original ran on until then and fell 2.6 s behind.
#[test]
fn stream_opened_ahead_measures_its_real_startup() {
    let mut c = clip(1, 0.0, 20.0);
    c.dec.len = Some(7 * 30);
    let mut w = World::new(vec![c]);
    w.play();
    w.run(4.0, DT);
    w.clips[0].ver += 1;
    w.clips[0].dec = Dec { delay: 1.5, ..Dec::default() };
    w.clips[0].want_start = Some(7.0);
    w.run(4.0, DT);
    assert!(w.core.pos().as_secs_f64() > 7.5);
    let s = &w.core.cur[&1];
    assert_eq!(s.key, w.key(&w.clips[0]), "switched stream runs");
    assert!((s.lead() - 1.5).abs() < 0.05, "lead {:.2} s, startup 1.5 s", s.lead());
}
