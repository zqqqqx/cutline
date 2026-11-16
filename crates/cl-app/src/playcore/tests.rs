//! Tests des Player-Kerns ohne Fenster: simulierter Decoder (Anlaufzeit, Zeit je Bild, liefert nie,
//! Ende, Öffnen scheitert) und simulierter Ton (Anlaufzeit, Fehler, Abbruch).

use super::*;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug)]
struct Dec {
    /// Öffnen bis erstes Bild (s)
    delay: f64,
    /// Decodierzeit je Bild (s) bei voller Qualität
    per_frame: f64,
    never: bool,
    /// Strom endet nach so vielen Bildern
    len: Option<u64>,
    fail_open: bool,
}

impl Default for Dec {
    fn default() -> Self {
        Self { delay: 0.05, per_frame: 0.004, never: false, len: None, fail_open: false }
    }
}

/// Simulierter Strom: Bild n ist ab `opened + delay + n·per_frame` fertig; Bildwert = Zeitleistenzeit.
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

/// Simulierter Ton: setzt `latency` nach dem Start an der Stelle der Uhr ein.
struct Aud {
    st: AState,
    latency: f64,
    fail: bool,
    /// Ton bricht zu dieser Wandzeit ab
    cut_at: Option<f64>,
    starts: u32,
}

impl Aud {
    fn new() -> Self {
        Self { st: AState::Off, latency: 0.1, fail: false, cut_at: None, starts: 0 }
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
    /// Rückwärts/Tempo-Kurve: nur Einzelbilder
    still_only: bool,
    /// schon so lange vor `start` sichtbar (Übergang vom Vorgänger)
    lead_in: f64,
    /// Quell-Fingerabdruck (gleiche Quelle nach Teilen), 0 = keiner
    alias: u64,
}

fn clip(id: Lid, start: f64, end: f64) -> Clip {
    Clip { id, start, end, ver: 0, dec: Dec::default(), still_only: false, lead_in: 0.0, alias: 0 }
}

/// Treiber wie in der App, aber mit simulierten Strömen, Einzelbildern und Ton.
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
    /// Einzelbilder in Arbeit: (Ebene, Schlüssel, fertig um)
    stills: Vec<(Lid, u64, f64)>,
    still_time: f64,
    /// geöffnete, noch nicht angehängte Ströme (Starter-Thread): (fertig um, Auftrag, Decoder)
    opening: Vec<(f64, Open, Dec, u64, u64)>,
    /// je Ebene zuletzt gezeigtes Strombild (Zeitleistenzeit)
    last_frame: HashMap<Lid, f64>,
    /// (Wandzeit, Stelle, Ebene, Bildzeit) aller gezeigten Strombilder
    shown: Vec<(f64, f64, Lid, f64)>,
    stills_shown: u32,
    ever_shown: bool,
    jumps: u64,
    last_pos: Time,
    opens: u32,
    level: usize,
    level_changes: u32,
    attached: Vec<(Open, bool)>,
    /// Wandzeit des Anhängens je (Ebene, Schlüssel)
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
        // Uhr läuft nie rückwärts außer bei einem Sprung (oder rückwärts abspielen)
        if self.core.playing() && self.core.jumps() == self.jumps && self.core.rate() > 0.0 {
            assert!(pos >= self.last_pos, "Uhr zurück ohne Sprung: {:?} → {:?}", self.last_pos, pos);
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
        // Starter-Thread: fertige Ströme anhängen (alte Projekte werden abgelehnt)
        let (ready, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.opening).into_iter().partition(|o| o.0 <= now);
        self.opening = rest;
        for (_, o, d, level, alias) in ready {
            let per_frame = d.per_frame / 2f64.powi(level as i32 * 2);
            let sim = Sim { clock: self.clock.clone(), opened: now, d, per_frame, n: 0, t0: o.at.as_secs_f64(), fps: self.fps };
            let accepted = self.core.attach_alias(o, self.fps, sim, now, alias);
            assert!(!accepted || o.gen == self.core.gen(), "Strom eines alten Projekts angehängt");
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
                let w = Want { id: c.id, key: self.key(c), start: Time::from_secs_f64(c.start), end: Time::from_secs_f64(c.end), prefetch: !active, alias: c.alias };
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
                        // aus der Zukunft höchstens das erste Bild eines Clips, der im Übergang schon sichtbar ist
                        assert!(f <= p.max(c.start) + 1e-6, "Bild aus der Zukunft: {f} bei {p}");
                        // ein Ersatz, der nach REPLACE_AFTER mit Rückstand übernimmt, darf älter sein
                        let late = had.is_some_and(|k| replacement(&self.core).is_none() && self.attached_at.get(&(c.id, k)).is_some_and(|t| now - t > REPLACE_AFTER));
                        if let Some(&last) = self.last_frame.get(&c.id).filter(|_| !late) {
                            assert!(f >= last - 1.01 / self.fps, "älteres Bild gezeigt: {f} nach {last}");
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
                    self.stills.push((c.id, key, now + self.still_time));
                }
            }
        }
        let (done, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.stills).into_iter().partition(|s| s.2 <= now);
        self.stills = rest;
        for (id, key, _) in done {
            if self.core.still_done(id, key) {
                self.stills_shown += 1;
                self.ever_shown = true;
            }
        }
        self.core.visible(&vis);
        self.check();
    }

    fn check(&self) {
        // je Ebene höchstens ein laufender Strom und ein Ersatz, der ihn gleich ablöst
        let s = self.core.streams();
        for (id, _, _) in &s {
            let same: Vec<_> = s.iter().filter(|x| x.0 == *id).collect();
            assert!(same.len() <= 2, "mehr als zwei Ströme für {id}");
            assert!(same.iter().filter(|x| !x.2).count() == 1, "zwei laufende Ströme für {id}");
        }
        if self.ever_shown {
            assert!(self.core.held().is_some(), "kein anzeigbares Bild mehr");
        }
    }

    fn run(&mut self, secs: f64, dt: f64) {
        let n = (secs / dt).round() as usize;
        for _ in 0..n {
            self.step(dt);
        }
    }

    /// Größte Lücke zwischen zwei gezeigten Bildern der Ebene `id` im Zeitraum (Wandzeit)
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
    // Ton vorbereitet, aber noch nicht eingesetzt
    assert!(matches!(w.aud.st, AState::Prep { anchor: None, .. }));
    w.run(0.2, DT);
    assert_eq!(w.core.mode(), Mode::Spielen);
    assert!(!w.core.waiting());
    let t_play = w.shown.first().unwrap().0;
    assert!(t_play < 0.1, "erstes Bild nach {t_play} s");
    w.run(3.0, DT);
    // Uhr = Ton-Uhr, läuft in Echtzeit; 30 Bilder/s ohne Lücke über 2 Intervalle
    assert!(matches!(w.aud.input(w.now()), AudioIn::Running(_)));
    let ran = w.core.pos().as_secs_f64();
    assert!((ran - 3.0).abs() < 0.15, "Uhr {ran}");
    assert!(w.gap(1, 0.5, 3.0) <= 2.0 / 30.0 + 0.01);
    assert!(w.frames(1.0, 3.0) >= 58);
    let st = w.core.take_stats();
    assert_eq!(st.catch_ups, 0);
    assert_eq!(st.opens, 1);
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
    assert_eq!(w.core.mode(), Mode::Spielen, "kein Hänger: nach 0,5 s läuft die Uhr");
    w.run(2.0, DT);
    assert!(w.core.pos().as_secs_f64() > 1.5);
}

#[test]
fn never_delivering_stream_holds_last_frame_and_is_reopened() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    assert!(w.ever_shown);
    // ab jetzt liefert der Decoder nichts mehr
    w.clips[0].dec.never = true;
    w.clips[0].ver = 1; // neuer Schlüssel → Ersatzstrom, der nie liefert
    w.run(3.0, DT);
    assert!(w.core.held().is_some(), "gehaltenes Bild statt Schwarz");
    assert_eq!(w.core.mode(), Mode::Spielen);
    // alter Strom zeigt weiter, bis der Ersatz liefert: Bilder laufen weiter
    assert!(w.frames(1.5, 4.0) > 50);
}

#[test]
fn stalled_stream_is_reopened_at_current_position() {
    let mut c = clip(1, 0.0, 30.0);
    c.dec.len = Some(40); // Strom endet vorzeitig (FFmpeg abgebrochen)
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
    w.playhead = Time::from_secs_f64(12.0); // Klick in die Zeitleiste
    w.step(DT);
    assert_eq!(w.core.mode(), Mode::Puffern);
    assert_eq!(w.aud.starts, starts + 1, "Ton ab der neuen Stelle");
    w.run(0.5, DT);
    assert_eq!(w.core.mode(), Mode::Spielen);
    let (_, p, _, f) = *w.shown.last().unwrap();
    assert!(p >= 12.0 && f >= 12.0 && p - f < 0.1, "Bild {f} bei {p}");
}

#[test]
fn visible_edit_keeps_picture_running_and_switches_stream() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.clips[0].dec.delay = 0.3; // neuer Strom braucht lange
    w.play();
    w.run(1.0, DT);
    w.clips[0].ver = 1; // Effekt geändert
    w.rev += 1;
    w.core.edit(false, w.now());
    let t = w.now();
    w.run(1.5, DT);
    assert!(w.gap(1, t, t + 1.5) <= 2.0 / 30.0 + 0.01, "Bild stand beim Bearbeiten");
    let s = w.core.streams();
    assert_eq!(s.len(), 1, "alter Strom nach dem Wechsel frei: {s:?}");
    assert_eq!(s[0].1, w.key(&w.clips[0]));
}

/// Befund aus dem Review: ein Ersatz langsamer als Echtzeit (schwerer Effekt während Play
/// hinzugefügt) übernahm nie – die Änderung blieb bis zum Clipende unsichtbar.
#[test]
fn slow_replacement_takes_over_after_a_second() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    w.clips[0].ver = 1;
    w.clips[0].dec.per_frame = 1.0 / 24.0; // 0,8× Echtzeit bei 30 Bildern/s
    w.rev += 1;
    w.core.edit(false, w.now());
    w.run(2.5, DT);
    let new = w.key(&w.clips[0]);
    let s = w.core.streams();
    assert!(s.iter().any(|x| !x.2 && x.1 == new), "Änderung sichtbar: {s:?}");
    assert!(s.iter().all(|x| x.1 == new), "alter Strom frei: {s:?}");
}

/// Befund aus dem Review: ein Ersatz, der ohne Bild endet (Effekt-Graph geht nicht), blieb für
/// immer stehen und wurde nie neu versucht.
#[test]
fn replacement_ending_without_a_frame_is_retried() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    w.clips[0].ver = 1;
    w.clips[0].dec.len = Some(0); // FFmpeg endet sofort
    w.rev += 1;
    let (opens, t) = (w.opens, w.now());
    w.run(2.5, DT);
    assert!(w.opens >= opens + 2, "neu versucht: {}", w.opens - opens);
    assert!(w.opens <= opens + 4, "nicht in jedem Bild: {}", w.opens - opens);
    assert!(w.gap(1, t, w.now()) <= 2.0 / 30.0 + 0.01, "alter Strom zeigt weiter");
    w.clips[0].dec.len = None; // geht wieder
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
    // Lautstärke-Regler ziehen: 10 Änderungen im Abstand von 50 ms
    for _ in 0..10 {
        w.core.edit(true, w.now());
        w.run(0.05, DT);
    }
    assert_eq!(w.aud.starts, starts, "Ton nicht bei jedem Schritt neu");
    w.run(0.2, DT);
    assert_eq!(w.aud.starts, starts + 1);
    w.run(0.5, DT);
    assert!(matches!(w.aud.input(w.now()), AudioIn::Running(_)));
    assert!(w.gap(1, 1.0, w.now()) <= 2.0 / 30.0 + 0.01, "Bild lief weiter");
}

/// Messlauf PL_gross 0:40: Kurz vor einem Schnitt änderte sich der Schlüssel des vorab
/// geöffneten Stroms (Qualitätsstufe). Der fertige Vorab-Strom wurde verworfen und der neue
/// (4K, Hardware-Start ~1 s) war am Schnitt nicht da – 1 s Stillstand. Jetzt bleibt der alte, bis
/// der neue liefert.
#[test]
fn prefetched_stream_stays_until_its_replacement_delivers() {
    let mut b = clip(2, 3.0, 10.0);
    b.dec.delay = 0.9; // Strom braucht 0,9 s bis zum ersten Bild
    let mut w = World::new(vec![clip(1, 0.0, 3.0), b]);
    w.play();
    w.run(2.6, DT); // Vorab-Strom ab 1,5 s geöffnet, seit 2,4 s bereit
    w.clips[1].ver = 1; // neuer Schlüssel 0,4 s vor dem Schnitt
    w.run(1.5, DT);
    let t: Vec<f64> = w.shown.iter().filter(|s| s.0 > 2.5 && s.0 < 4.1).map(|s| s.0).collect();
    let gap = t.windows(2).map(|x| x[1] - x[0]).fold(0.0, f64::max);
    assert!(gap <= 2.0 / 30.0 + 0.01, "Lücke {gap:.2} s am Schnitt");
    w.run(1.0, DT);
    let new = w.key(&w.clips[1]);
    assert!(w.core.streams().iter().filter(|x| x.0 == 2).all(|x| x.1 == new), "neuer Strom übernimmt");
}

#[test]
fn cuts_between_many_short_clips_have_no_gap() {
    let clips: Vec<Clip> = (0..30).map(|i| clip(i + 1, i as f64 * 0.5, (i + 1) as f64 * 0.5)).collect();
    let mut w = World::new(clips);
    w.play();
    w.run(14.0, DT);
    let t: Vec<f64> = w.shown.iter().filter(|s| s.0 > 0.3).map(|s| s.0).collect();
    let gap = t.windows(2).map(|x| x[1] - x[0]).fold(0.0, f64::max);
    assert!(gap <= 2.0 / 30.0 + 0.01, "Lücke {gap} s am Schnitt");
    // je Clip ein Strom (vorab geöffnet), keine Neustarts
    assert!(w.opens <= 31, "{} Ströme", w.opens);
}

#[test]
fn several_tracks_all_deliver() {
    let mut w = World::new(vec![clip(1, 0.0, 10.0), clip(2, 0.0, 10.0), clip(3, 2.0, 6.0)]);
    w.play();
    w.run(5.0, DT);
    for id in [1, 2, 3] {
        let from = if id == 3 { 2.2 } else { 0.5 };
        assert!(w.gap(id, from, 5.0) <= 2.0 / 30.0 + 0.01, "Ebene {id}");
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
    assert_eq!(w.core.pos(), at, "Uhr steht in der Pause");
    w.play();
    assert_eq!(w.core.mode(), Mode::Spielen, "direkt weiter, ohne Puffern");
    let t = w.now();
    w.run(1.0, DT);
    assert_eq!(w.opens, opens, "keine neuen Ströme");
    assert!(w.gap(1, t, t + 1.0) <= 2.0 / 30.0 + 0.01);
    assert!((w.core.pos() - at).as_secs_f64() > 0.9);
}

#[test]
fn pause_then_move_frees_streams_and_play_rebuffers() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.play();
    w.run(1.0, DT);
    w.pause();
    w.playhead = Time::from_secs_f64(5.0); // verschoben
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
    w.clips[0].ver = 7; // Ersatz wird gerade geöffnet …
    w.step(DT);
    assert!(!w.opening.is_empty());
    let gen = w.core.gen();
    w.core.new_project(); // … da wechselt das Projekt
    w.cmds();
    assert_eq!(w.core.gen(), gen + 1);
    assert_eq!(w.core.mode(), Mode::Stand);
    assert!(w.core.held().is_none());
    w.ever_shown = false;
    w.step(DT); // hängt den alten an: wird abgelehnt (Assertion im Treiber)
    assert!(w.core.streams().is_empty());
    assert_eq!(w.aud.st, AState::Off);
}

#[test]
fn audio_failure_lets_picture_run_on_system_clock() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    w.aud.cut_at = Some(1.0); // Ton bricht nach 1 s ab
    w.play();
    w.run(3.0, DT);
    assert_eq!(w.core.mode(), Mode::Spielen);
    let p = w.core.pos().as_secs_f64();
    assert!((p - 3.0).abs() < 0.15, "Uhr {p}");
    assert!(w.gap(1, 0.5, 3.0) <= 2.0 / 30.0 + 0.01);
    assert_eq!(w.aud.st, AState::Off, "Ton freigegeben");
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
    assert!(w.core.streams().is_empty(), "keine Ströme bei Tempo ≠ 1");
    // Einzelbilder kommen laufend (laufender Auftrag wird nicht ständig abgebrochen)
    assert!(w.stills_shown >= 8, "{} Einzelbilder", w.stills_shown);
    w.core.shuttle(-2.0, w.playhead, w.now());
    w.cmds();
    w.run(2.5, DT);
    assert_eq!(w.core.mode(), Mode::Stand);
    assert_eq!(w.core.pos(), Time::ZERO);
}

#[test]
fn reverse_clip_during_play_shows_stills_and_never_goes_black() {
    let mut c = clip(2, 1.0, 4.0);
    c.still_only = true; // Rückwärts-Clip
    let mut w = World::new(vec![clip(1, 0.0, 1.0), c, clip(3, 4.0, 6.0)]);
    w.still_time = 0.15; // langsames Einzelbild
    w.play();
    w.run(5.5, DT);
    assert!(w.stills_shown >= 10, "{} Einzelbilder im Rückwärts-Clip", w.stills_shown);
    assert!(w.core.held().is_some());
}

#[test]
fn slow_stream_catches_up_instead_of_drifting() {
    let mut c = clip(1, 0.0, 60.0);
    c.dec.per_frame = 1.0 / 40.0; // 40 statt 60 Bilder/s
    c.dec.delay = 0.15;
    let mut w = World::new(vec![c]);
    w.fps = 60.0;
    w.play();
    w.run(10.0, DT);
    let st = w.core.take_stats();
    assert!(st.catch_ups >= 1, "{st:?}");
    // ohne automatische Qualität: Rückstand bleibt begrenzt (ohne Aufholen wüchse er auf über 3 s),
    // neu geöffnet wird höchstens alle 5 s
    assert!(st.max_lag < 2.0, "Rückstand {} s", st.max_lag);
    assert!(st.catch_ups <= 2, "{st:?}");
}

#[test]
fn auto_quality_restores_frame_rate_of_slow_stream() {
    let mut c = clip(1, 0.0, 60.0);
    c.dec.per_frame = 1.0 / 40.0; // voll: 40 Bilder/s; ½: 160 Bilder/s
    let mut w = World::new(vec![c]);
    w.fps = 60.0;
    w.max_q = 3;
    w.play();
    w.run(3.0, DT);
    assert!(w.core.quality.level >= 1, "nach 1–2 s eine Stufe kleiner");
    w.run(27.0, DT);
    // Versuche, wieder voll zu rechnen, werden seltener (4, 8, 16 s): kaum Verlust, kein Flattern
    let n = w.frames(2.0, 30.0) as f64 / 28.0;
    assert!(n >= 55.0, "{n} Bilder/s mit automatischer Qualität");
    assert!(w.level_changes <= 6, "{} Wechsel", w.level_changes);
    // im Stand zurück auf voll
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
    // zu langsam → eine Stufe, dann Ruhe während der Umstellung
    assert_eq!(q.decide(1.0, Some(0.8), 3, true), 1);
    assert_eq!(q.decide(2.0, Some(0.5), 3, true), 1);
    assert_eq!(q.decide(3.0, Some(0.5), 3, true), 2);
    // nie über die höchste Stufe
    assert_eq!(q.decide(5.0, Some(0.5), 2, true), 2);
    // reicht es wieder: erst nach 4 s am Stück zurück
    for t in 6..10 {
        assert_eq!(q.decide(t as f64, Some(1.0), 2, true), 2, "t={t}");
    }
    assert_eq!(q.decide(10.0, Some(1.0), 2, true), 1);
    // Hochschalten scheitert gleich wieder → nächster Versuch erst nach 8 s
    assert_eq!(q.decide(12.0, Some(0.7), 2, true), 2);
    for t in 14..21 {
        assert_eq!(q.decide(t as f64, Some(1.0), 2, true), 2, "t={t}");
    }
    assert_eq!(q.decide(22.0, Some(1.0), 2, true), 1);
    // zwischen den Schwellen: bleibt, Gut-Zeit beginnt neu
    assert_eq!(q.decide(24.0, Some(0.95), 2, true), 1);
    // Stand: sofort voll
    assert_eq!(q.decide(25.0, None, 2, false), 0);
    assert_eq!(q.level, 0);
    // aus: immer 0
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
    assert!((2..=4).contains(&w.opens), "{} Versuche", w.opens);
    assert_eq!(w.core.mode(), Mode::Spielen);
}

#[test]
fn stills_in_stand_replace_running_requests_and_drive_the_wait_line() {
    let mut core: PlayCore<Sim> = PlayCore::default();
    assert!(!core.waiting());
    assert!(core.still(1, 10, 0.0));
    assert!(core.waiting(), "angefordert, noch nichts da");
    assert!(!core.still(1, 10, 0.01), "derselbe Auftrag läuft schon");
    // Scrubben: der neue ersetzt den alten sofort
    assert!(core.still(1, 11, 0.02));
    assert!(!core.still_done(1, 10), "veraltetes Bild wird nicht gezeigt");
    assert!(core.waiting());
    assert!(core.still_done(1, 11));
    assert!(!core.waiting());
    assert_eq!(core.held(), Some(1));
    // Projekt geändert: neu anfordern, Bild bleibt stehen
    core.forget_stills();
    assert!(core.still(1, 11, 0.1));
    core.visible(&[]);
    assert!(!core.waiting());
    assert_eq!(core.held(), Some(1));
}

/// Befund aus dem Review: ein gescheitertes Einzelbild blieb als Auftrag stehen – Ladelinie für
/// immer, beim Abspielen stand das Bild bis `STILL_PATIENCE`.
#[test]
fn failed_still_ends_the_request_and_the_next_is_requested() {
    let mut core: PlayCore<Sim> = PlayCore::default();
    assert!(core.still(1, 10, 0.0));
    assert!(core.has_requests());
    core.still_failed(1, 9); // veraltet: ändert nichts
    assert!(core.has_requests());
    core.still_failed(1, 10);
    assert!(!core.has_requests(), "Ladeanzeige endet");
    assert!(!core.waiting());
    assert!(!core.still(1, 10, 0.1), "derselbe nicht in jedem UI-Bild neu");
    assert!(core.still(1, 11, 0.2), "das nächste wird angefordert");
    // beim Abspielen (Rückwärts-Clip): das nächste sofort, nicht erst nach STILL_PATIENCE
    core.shuttle(2.0, Time::ZERO, 1.0);
    assert!(core.still(1, 20, 1.0));
    assert!(!core.still(1, 21, 1.1), "laufender Auftrag wird nicht ersetzt");
    core.still_failed(1, 20);
    assert!(core.still(1, 21, 1.2));
    // ein gelieferter Auftrag zählt ebenfalls nicht mehr als offen
    assert!(core.still_done(1, 21));
    assert!(!core.has_requests());
}

#[test]
fn play_during_buffering_seek_and_stop_from_every_state() {
    let mut w = World::new(vec![clip(1, 0.0, 30.0)]);
    // Puffern → Stopp
    w.play();
    w.core.stop();
    w.cmds();
    assert_eq!(w.core.mode(), Mode::Stand);
    // Pause → Stopp
    w.play();
    w.run(1.0, DT);
    w.pause();
    w.core.stop();
    w.cmds();
    assert_eq!(w.core.mode(), Mode::Stand);
    assert!(w.core.streams().is_empty());
    // Sprung im Stand bewegt nur die Stelle
    w.core.seek(Time::from_secs_f64(3.0), w.now());
    assert_eq!(w.core.mode(), Mode::Stand);
    assert_eq!(w.core.pos(), Time::from_secs_f64(3.0));
    // Sprung in der Pause beendet sie
    w.playhead = w.core.pos();
    w.play();
    w.run(0.5, DT);
    w.pause();
    w.core.seek(Time::from_secs_f64(8.0), w.now());
    assert_eq!(w.core.mode(), Mode::Stand);
    // Ton sofort neu (Solo, Gerät) nur beim Abspielen
    let s = w.aud.starts;
    w.core.restart_audio(w.now());
    w.cmds();
    assert_eq!(w.aud.starts, s);
}

/// Zufallstest: 10 000 Ereignisfolgen; die Invarianten prüft der Treiber in jedem Takt
/// (`World::step`/`check`): nie zwei Ströme mit demselben Schlüssel je Ebene, kein Strom eines
/// alten Projekts, Uhr nur bei Sprüngen zurück, kein Bild aus der Zukunft oder älter als das
/// gezeigte, immer ein anzeigbares Bild, sobald eins da war.
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
    w.step(0.001); // Öffnen ab 0 beauftragt, Starter noch nicht fertig
    assert_eq!(w.opening.len(), 1);
    w.playhead = Time::from_secs_f64(9.0); // Sprung, bevor der Strom da ist
    w.run(0.3, DT);
    let stale: Vec<_> = w.attached.iter().filter(|(o, _)| o.at < Time::from_secs_f64(9.0)).collect();
    assert!(stale.iter().all(|(_, ok)| !ok), "Strom von vor dem Sprung angehängt: {stale:?}");
    assert!(w.shown.iter().all(|s| s.3 >= 9.0));
}

#[test]
fn incoming_clip_in_transition_shows_its_first_frame_before_its_start() {
    let mut b = clip(2, 2.0, 6.0);
    b.lead_in = 0.25; // Übergang: B ist ab 1,75 s sichtbar, sein Strom beginnt bei 2 s
    let mut w = World::new(vec![clip(1, 0.0, 2.25), b]);
    w.play();
    w.run(1.9, DT);
    let early: Vec<_> = w.shown.iter().filter(|s| s.2 == 2 && s.1 < 2.0).collect();
    assert!(!early.is_empty(), "B zeigt im Übergang vor seinem Anfang nichts");
    assert!(early.iter().all(|s| (s.3 - 2.0).abs() < 1e-6), "vor dem Anfang nur das erste Bild");
    w.run(1.0, DT);
    assert!(w.gap(2, 2.1, 2.9) <= 2.0 / 30.0 + 0.01);
}

/// Mangel aus dem Klicktest: Einzelbilder beim Abspielen (Rückwärts, Tempo-Kurve, bewegte Bilder)
/// wurden je Bild der Oberfläche neu angefordert und der laufende Auftrag dabei beendet – dauert
/// einer länger als ein paar Bilder, kam nie eins an (Standbild bei laufendem Ton).
#[test]
fn slow_stills_during_play_are_finished_and_shown() {
    let mut c = clip(1, 0.0, 20.0);
    c.still_only = true;
    let mut w = World::new(vec![c]);
    w.still_time = 1.5; // unter Last: 1,5 s je Einzelbild
    w.play();
    w.run(8.0, DT);
    assert!(w.stills_shown >= 4, "{} Einzelbilder in 8 s", w.stills_shown);
    // Sprung: der laufende Auftrag gehört zur alten Stelle und wird ersetzt
    let before = w.stills.len();
    w.playhead = Time::from_secs_f64(15.0);
    w.run(0.1, DT);
    assert!(w.stills.len() > before || w.stills.iter().any(|s| s.2 > w.now()), "neuer Auftrag nach dem Sprung");
}

/// Mangel aus dem Klicktest: ein Strom, der nichts mehr liefert, wurde jede Sekunde neu geöffnet
/// (bei langer GOP holt er so nie auf). Jetzt: erst nach 2 s ohne Bild, dann höchstens alle 5 s.
#[test]
fn hanging_stream_is_reopened_rarely() {
    let mut c = clip(1, 0.0, 60.0);
    c.dec.len = Some(30); // liefert eine Sekunde, dann nichts mehr (FFmpeg beendet)
    c.dec.delay = 0.1;
    let mut w = World::new(vec![c]);
    w.play();
    w.run(20.0, DT);
    // erster Strom + Neustarts nach Ende, je höchstens alle 5 s
    assert!(w.opens <= 6, "{} Ströme in 20 s", w.opens);
    assert!(w.opens >= 3);
}

/// Mangel aus dem Klicktest: Teilen während Play hielt das Bild an (neuer Clip = neuer Strom ab
/// dem Keyframe). Jetzt übernimmt der rechte Teil den laufenden Strom derselben Quelle.
#[test]
fn split_during_play_takes_over_the_running_stream() {
    let mut a = clip(1, 0.0, 10.0);
    a.alias = 77;
    a.dec.delay = 1.5; // neuer Strom bräuchte 1,5 s (lange GOP)
    let mut w = World::new(vec![a]);
    w.play();
    w.run(1.0 + w.clips[0].dec.delay, DT);
    // Teilen am Playhead: links bleibt Clip 1 (bis hier), rechts neuer Clip 2 aus derselben Quelle
    let cut = w.core.pos().as_secs_f64();
    let mut b = w.clips[0].clone();
    (b.id, b.start) = (2, cut);
    w.clips[0].end = cut;
    w.clips.push(b);
    w.rev += 1;
    w.core.edit(true, w.now());
    let (opens, t) = (w.opens, w.now());
    w.run(2.0, DT);
    assert_eq!(w.opens, opens, "kein neuer Strom");
    let gap = w.shown.iter().filter(|s| s.0 >= t - 0.1).map(|s| s.0).collect::<Vec<_>>().windows(2).map(|x| x[1] - x[0]).fold(0.0, f64::max);
    assert!(gap <= 2.0 / 30.0 + 0.01, "Lücke {gap} s nach dem Teilen");
}
