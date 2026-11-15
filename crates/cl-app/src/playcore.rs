//! Player-Kern: der Ablauf der Wiedergabe als reine Logik – ohne egui, ohne Prozesse und ohne
//! eigene Uhr (die Zeit kommt als Eingabe `now` in Sekunden). Die App ist nur der Treiber: sie
//! meldet Ereignisse (Play, Pause, Sprung, Bearbeitung, Ton-Zustand), fragt je Ebene, ob ein
//! Bildstrom zu öffnen ist ([`PlayCore::want`]), hängt geöffnete Ströme an ([`PlayCore::attach`]),
//! holt fällige Bilder ([`PlayCore::pull`]) und führt die Ton-Aufträge aus ([`PlayCore::cmds`]).
//!
//! Zustände und Übergänge:
//! - `Stand` --Play--> `Puffern` (Ströme öffnen, Ton vorbereiten, Uhr steht)
//! - `Puffern` --alle Ströme am Playhead haben ein Bild, sonst nach 1 s--> `Spielen` (Uhr läuft,
//!   Ton setzt an der Uhr ein)
//! - `Spielen` --Pause (Ton läuft, Tempo 1)--> `Pause` (Ströme und Ton bleiben offen)
//! - `Pause` --Play an derselben Stelle ohne Änderung--> `Spielen` (ohne neue Ströme)
//! - `Pause` --Playhead verschoben oder Projekt geändert--> `Stand` (Ströme frei)
//! - `Spielen`/`Puffern` --Sprung--> `Puffern` ab der neuen Stelle
//! - J/K/L (`shuttle`) --> `Spielen` mit Tempo ≠ 1 (Einzelbilder, kein Ton)
//! - Stopp, Projektende, Projektwechsel --> `Stand`
//!
//! Synchron statt hinterher: Liegt ein Strom länger als [`CATCH_UP_AFTER`] mehr als zwei
//! Bildintervalle hinter der Uhr, wird er ab „Uhr + Anlaufzeit“ neu geöffnet (der alte zeigt bis
//! dahin weiter); die automatische Qualität ([`AutoQuality`]) senkt dann die Decodiergröße.

use std::collections::HashMap;

use cl_core::Time;

/// Ebene: Clip oder abgeleitete Fassung (Effekt-Element) als Zahl, wie `ClipId`.
pub type Lid = u128;

/// Bildquelle eines Stroms (FFmpeg in der App, simuliert in den Tests).
pub trait Source {
    type Frame;
    /// Nächstes fertiges Bild, `Ok(None)` = noch keins, `Err` = Strom zu Ende (Quelle oder Fehler)
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
    /// steht; Bilder kommen als Einzelbilder
    Stand,
    /// Play gedrückt: Ströme laufen an, die Uhr steht noch
    Puffern,
    Spielen,
    /// angehalten mit offenen Strömen und Ton (Fortsetzen ohne neues Puffern)
    Pause,
}

/// Ton aus Sicht des Kerns (Eingabe je Takt).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioIn {
    /// kein Ton (stumm, Tempo ≠ 1, nicht startbar)
    None,
    /// vorbereitet, noch nicht hörbar
    Pending,
    /// hörbar an dieser Stelle (Ton-Uhr)
    Running(Time),
    /// Ausgabe ließ sich nicht öffnen
    Failed,
    /// Ton zu Ende bzw. abgebrochen
    Finished,
}

/// Ton-Aufträge an den Treiber.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    /// Ton ab `at` vorbereiten; `anchor`: sofort an der laufenden Uhr einsetzen, sonst erst mit
    /// [`Cmd::AnchorAudio`]
    StartAudio { at: Time, anchor: bool },
    /// Uhr läuft jetzt: Ton setzt an der Stelle der Uhr ein
    AnchorAudio,
    StopAudio,
    PauseAudio,
    ResumeAudio,
}

/// Eine Ebene, für die beim Abspielen ein Strom laufen soll.
#[derive(Clone, Copy, Debug)]
pub struct Want {
    pub id: Lid,
    /// ändert sich mit allem, was das Bild ändert (Effekte, Timing, Größe, Bildrate)
    pub key: u64,
    /// Zeitleistenstelle, ab der ein vorab geöffneter Strom beginnt (Clip-/Element-Anfang)
    pub start: Time,
    /// Ende des Clips (ein Strom kurz vor dem Ende wird nicht mehr neu geöffnet)
    pub end: Time,
    /// beginnt erst gleich: nur vorab öffnen, noch keine Bilder holen
    pub prefetch: bool,
}

/// Auftrag „Strom ab `at` öffnen“ (Antwort auf [`PlayCore::want`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Open {
    pub id: Lid,
    pub key: u64,
    pub at: Time,
    pub gen: u64,
    /// laufende Nummer: nur der zuletzt beauftragte Strom einer Ebene wird angehängt
    pub seq: u64,
}

/// Eingabe je Takt.
#[derive(Clone, Copy, Debug)]
pub struct Tick {
    pub now: f64,
    /// Playhead der Oberfläche (weicht er beim Abspielen vom Kern ab, wurde gesprungen)
    pub playhead: Time,
    /// Bearbeitungsstand des Projekts (Pause gilt nur bis zur nächsten Änderung)
    pub revision: u64,
    pub audio: AudioIn,
    /// Projektdauer
    pub end: Time,
    pub fps: f64,
    pub loop_range: Option<(Time, Time)>,
    /// Aufnahme läuft: am Projektende nicht anhalten
    pub recording: bool,
    /// höchste Stufe der automatischen Qualität (0 = aus)
    pub max_quality_level: usize,
}

/// Messwerte seit dem letzten [`PlayCore::take_stats`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub opens: u32,
    /// gezeigte Strombilder
    pub frames: u32,
    /// geholte, aber übersprungene Strombilder (Aufholen)
    pub dropped: u32,
    /// gezielt neu geöffnete Ströme (Rückstand) bzw. hängende Ströme
    pub catch_ups: u32,
    pub stalls: u32,
    /// größte Lücke zwischen zwei Bildern der Hauptebene (s)
    pub max_gap: f64,
    /// größter Rückstand eines Stroms hinter der Uhr (s)
    pub max_lag: f64,
}

/// Rückstand, ab dem gezielt aufgeholt wird: zwei Bildintervalle …
const LAG_FRAMES: f64 = 2.0;
/// … länger als diese Zeit (so bleibt der Rückstand samt Neustart unter 0,5 s)
pub const CATCH_UP_AFTER: f64 = 0.2;
/// Puffern endet spätestens nach dieser Zeit (dann läuft die Uhr mit gehaltenem Bild)
const WARM_MAX: f64 = 1.0;
/// Hörbare Änderungen beim Abspielen: Ton erst so lange nach der letzten Änderung neu
pub const AUDIO_DEBOUNCE: f64 = 0.15;
/// Öffnen schlug fehl: so lange nicht erneut versuchen
const RETRY_OPEN: f64 = 1.0;
/// Einzelbild beim Abspielen (Rückwärts, Tempo-Kurve): laufenden Auftrag so lange nicht ersetzen
const STILL_PATIENCE: f64 = 0.6;

struct Slot<S> {
    src: S,
    key: u64,
    /// Zeitleistenstelle des ersten Bildes
    t0: Time,
    fps: f64,
    taken: u64,
    ended: bool,
    waiting_since: Option<f64>,
    opened: f64,
    first: Option<f64>,
    behind_since: Option<f64>,
    /// Bilder im laufenden Messfenster
    win_frames: u64,
}

impl<S: Source> Slot<S> {
    fn new(src: S, key: u64, t0: Time, fps: f64, now: f64) -> Self {
        Self { src, key, t0, fps: fps.max(1.0), taken: 0, ended: false, waiting_since: None, opened: now, first: None, behind_since: None, win_frames: 0 }
    }

    /// Alle fälligen Bilder holen, das neueste zurückgeben (Rest zählt als übersprungen).
    fn drain(&mut self, pos: Time, now: f64, st: &mut Stats) -> Option<S::Frame> {
        if pos < self.t0 {
            return None; // vorab geöffnet: noch nicht dran
        }
        let target = ((pos - self.t0).as_secs_f64() * self.fps + 1e-6).floor() as u64;
        let (mut latest, mut n) = (None, 0u64);
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

    /// Rückstand des zuletzt geholten Bildes hinter der Uhr (s)
    fn lag(&self, pos: Time) -> Option<f64> {
        (self.taken > 0 && pos >= self.t0).then(|| pos.as_secs_f64() - (self.t0.as_secs_f64() + (self.taken - 1) as f64 / self.fps))
    }

    /// Strom beendet oder seit über einer Sekunde ohne fälliges Bild (vor dem ersten Bild läuft
    /// noch die Suche in der Quelle: großzügiger)
    fn stalled(&self, now: f64) -> bool {
        let limit = if self.taken == 0 { 6.0 } else { 1.0 };
        self.ended || self.waiting_since.is_some_and(|w| now - w > limit)
    }

    /// Anlaufzeit dieses Stroms (Öffnen bis erstes Bild), für das Aufholen
    fn lead(&self) -> f64 {
        self.first.map_or(0.3, |f| f - self.opened).clamp(0.05, 1.0)
    }
}

pub struct PlayCore<S: Source> {
    mode: Mode,
    pos: Time,
    rate: f64,
    /// (Wandzeit, Stelle): Systemuhr, solange kein Ton läuft
    clock: Option<(f64, Time)>,
    warm_since: f64,
    paused: Option<(Time, u64)>,
    gen: u64,
    cur: HashMap<Lid, Slot<S>>,
    /// Ersatz mit neuem Schlüssel (Bearbeitung, Qualität, Aufholen): zeigt erst, wenn er liefert
    next: HashMap<Lid, Slot<S>>,
    /// Öffnen beauftragt: (Schlüssel, als Ersatz, laufende Nummer)
    opening: HashMap<Lid, (u64, bool, u64)>,
    seq: u64,
    /// gezielt neu öffnen ab dieser Stelle
    reopen: HashMap<Lid, Time>,
    failed: HashMap<Lid, (u64, f64)>,
    /// Einzelbilder: angefordert (Schlüssel, seit) und gezeigt
    requested: HashMap<Lid, (u64, f64)>,
    shown: HashMap<Lid, u64>,
    /// zuletzt mit einem Bild versorgte Ebene (es gibt ein anzeigbares Bild)
    held: Option<Lid>,
    audio_due: Option<f64>,
    last_primary: Option<f64>,
    /// Sprünge (Play, Sprung, Schleife, Stopp): Uhr darf nur dabei zurückgehen
    jumps: u64,
    win_start: f64,
    win_catch_up: bool,
    pub quality: AutoQuality,
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
            requested: HashMap::new(),
            shown: HashMap::new(),
            held: None,
            audio_due: None,
            last_primary: None,
            jumps: 0,
            win_start: 0.0,
            win_catch_up: false,
            quality: AutoQuality::default(),
            stats: Stats::default(),
            cmds: Vec::new(),
        }
    }
}

/// Letztes Bild vor `end` (Projektende) bei `fps`
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
    /// Uhr läuft oder läuft gleich (Puffern)
    pub fn playing(&self) -> bool {
        matches!(self.mode, Mode::Puffern | Mode::Spielen)
    }
    /// Bilder kommen aus Strömen (sonst Einzelbilder)
    pub fn streaming(&self) -> bool {
        self.playing() && (self.rate - 1.0).abs() < 1e-6
    }
    #[cfg(test)]
    /// Ebene mit dem zuletzt gelieferten Bild (`None`: es kam noch nie eins)
    pub fn held(&self) -> Option<Lid> {
        self.held
    }
    /// Ton-Aufträge abholen
    pub fn cmds(&mut self) -> Vec<Cmd> {
        std::mem::take(&mut self.cmds)
    }
    pub fn take_stats(&mut self) -> Stats {
        std::mem::take(&mut self.stats)
    }
    #[cfg(test)]
    /// Offene Ströme: (Ebene, Schlüssel, ist Ersatz)
    pub fn streams(&self) -> Vec<(Lid, u64, bool)> {
        self.cur.iter().map(|(id, s)| (*id, s.key, false)).chain(self.next.iter().map(|(id, s)| (*id, s.key, true))).collect()
    }

    /// Wartet der Player auf ein Bild (Warte-Linie)? Beim Abspielen: nur während des Pufferns. Im
    /// Stand: zu einem angeforderten Einzelbild ist noch kein passendes geliefert.
    pub fn waiting(&self) -> bool {
        match self.mode {
            Mode::Puffern => true,
            Mode::Spielen => false,
            Mode::Stand | Mode::Pause => self.requested.iter().any(|(id, (k, _))| self.shown.get(id) != Some(k)),
        }
    }

    /// Bild-Ton-Versatz (ms): wie weit das gezeigte Strombild hinter der Uhr liegt (größter Wert)
    pub fn av_offset_ms(&self) -> Option<f64> {
        self.cur.values().filter_map(|s| s.lag(self.pos)).map(|l| l * 1000.0).reduce(f64::max)
    }

    fn clear_streams(&mut self) {
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

    /// Play: im Stand ab `at` puffern; in der Pause an derselben Stelle (`at`, `revision`)
    /// ohne neue Ströme weiter.
    pub fn play(&mut self, at: Time, revision: u64, now: f64) {
        if self.mode == Mode::Pause && self.paused == Some((at, revision)) {
            self.paused = None;
            // die Stelle steht in der Pause: Ströme laufen ohne Verschiebung weiter
            for s in self.cur.values_mut() {
                s.waiting_since = None;
                s.behind_since = None;
            }
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

    /// Alles frei, Stand an `at`
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

    /// Ginge Play jetzt ohne neues Puffern weiter (Pause an derselben Stelle, nichts geändert)?
    pub fn can_resume(&self, at: Time, revision: u64) -> bool {
        self.mode == Mode::Pause && self.paused == Some((at, revision))
    }

    /// Gibt es Einzelbild-Aufträge (Ladeanzeige beim Öffnen eines Projekts)?
    pub fn has_requests(&self) -> bool {
        !self.requested.is_empty()
    }

    /// Pause. Nur mit laufendem Ton bei Tempo 1 (`can_hold`, z. B. nicht bei Mehrkamera) bleiben
    /// Ströme und Ton offen, sonst wird gestoppt.
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

    /// J/K/L: mit Tempo `rate` ab `at`, ohne Puffern (Tempo ≠ 1: Einzelbilder, kein Ton)
    pub fn shuttle(&mut self, rate: f64, at: Time, now: f64) {
        self.reset(at);
        self.rate = rate;
        self.mode = Mode::Spielen;
        self.clock = Some((now, at));
        self.start_audio(at, true);
    }

    /// Sprung an `at` (beim Abspielen: ab dort neu puffern)
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

    /// Bearbeitung beim Abspielen. Sichtbare Änderungen brauchen nichts (neue Schlüssel →
    /// Ersatzströme); hörbare erneuern den Ton erst [`AUDIO_DEBOUNCE`] nach der letzten (Regler
    /// ziehen startet ihn nicht bei jedem Schritt neu), das Bild läuft weiter.
    pub fn edit(&mut self, audible: bool, now: f64) {
        if audible && self.playing() {
            self.audio_due = Some(now + AUDIO_DEBOUNCE);
        }
    }

    /// Ton sofort neu ab der aktuellen Stelle (Solo, Ausgabegerät)
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

    /// Projektwechsel: alles frei, Aufträge des alten Projekts verfallen
    pub fn new_project(&mut self) {
        self.stop();
        self.gen += 1;
        self.requested.clear();
        self.shown.clear();
        self.held = None;
        self.pos = Time::ZERO;
    }

    /// Vorschau neu (Qualität, Proxy): Ströme und Einzelbild-Aufträge neu, gezeigte Bilder bleiben
    pub fn invalidate(&mut self) {
        self.clear_streams();
        self.requested.clear();
    }

    /// Projekt geändert: Einzelbilder neu anfordern (Ströme wechseln über ihren Schlüssel)
    pub fn forget_stills(&mut self) {
        self.requested.clear();
    }

    /// Takt: Uhr fortschreiben, Puffern beenden, Ton-Zustand verarbeiten, Ende/Schleife.
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
        if (ready && (any || waited > 0.05)) || waited > WARM_MAX {
            self.mode = Mode::Spielen;
            for s in self.cur.values_mut().filter(|s| s.t0 <= pos) {
                // erstes Bild = jetzt (nicht schon während des Pufferns fällige überspringen)
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
                    // ohne Ton nach Systemzeit weiter
                    self.cmds.push(Cmd::StopAudio);
                    self.clock = Some((now, self.pos));
                }
                // Ton-Uhr; nie zurück (Übergabe von der Systemzeit ist ohne Sprung)
                AudioIn::Running(p) => {
                    self.pos = p.max(self.pos);
                    self.clock = Some((now, self.pos));
                }
                AudioIn::None | AudioIn::Pending => {}
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
            // vorwärts nie zurück (Rundung bei der Übergabe Ton → Systemzeit)
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

    /// Messfenster (~1 s) für die automatische Qualität: Anteil gelieferter Bilder je Strom
    /// Im Stand zurück auf volle Qualität; beim Puffern und in der Pause bleibt die Stufe (sonst
    /// bräuchte das Fortsetzen neue Ströme), gemessen wird nur beim Spielen.
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
            // vorab geöffnete (noch nicht dran) und noch nicht angelaufene zählen nicht
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

    /// Soll für die Ebene `w` ein Strom geöffnet werden? `Some(open)`: ab `open.at` öffnen und mit
    /// [`Self::attach`] (oder [`Self::open_failed`]) zurückmelden. Nur beim Abspielen mit Tempo 1.
    pub fn want(&mut self, w: &Want, now: f64) -> Option<Open> {
        if !self.streaming() {
            return None;
        }
        let id = w.id;
        if self.failed.get(&id).is_some_and(|(k, t)| *k == w.key && now - t < RETRY_OPEN) {
            return None;
        }
        // hängender oder vorzeitig beendeter Strom: an der aktuellen Stelle neu (das letzte Bild
        // bleibt stehen, bis der neue liefert)
        if !w.prefetch
            && self.mode == Mode::Spielen
            && (w.end - self.pos).as_secs_f64() > 0.25
            && self.cur.get(&id).is_some_and(|s| s.key == w.key && s.t0 <= self.pos && s.stalled(now))
        {
            self.cur.remove(&id);
            self.next.remove(&id);
            self.stats.stalls += 1;
        }
        let cur = self.cur.get(&id).is_some_and(|s| s.key == w.key);
        let next = self.next.get(&id).is_some_and(|s| s.key == w.key);
        let opening = self.opening.get(&id).is_some_and(|(k, _, _)| *k == w.key);
        if next || opening {
            return None;
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
            // Ersatz beim Spielen: dort öffnen, wo die Uhr steht, wenn er anläuft
            Some(s) if self.mode == Mode::Spielen => self.pos + Time::from_secs_f64(s.lead()),
            _ => self.pos,
        };
        Some(Open { id, key: w.key, at, gen: self.gen, seq: self.seq })
    }

    /// Geöffneten Strom anhängen. Veraltete (anderes Projekt, Schlüssel nicht mehr gewollt,
    /// nicht mehr beim Abspielen) werden verworfen (beendet beim Fallenlassen).
    pub fn attach(&mut self, o: Open, fps: f64, src: S, now: f64) -> bool {
        if o.gen != self.gen || !self.streaming() {
            return false;
        }
        let Some((key, replace, _)) = self.opening.get(&o.id).copied().filter(|(k, _, q)| *k == o.key && *q == o.seq) else {
            return false;
        };
        self.opening.remove(&o.id);
        let slot = Slot::new(src, key, o.at, fps, now);
        if replace && self.cur.contains_key(&o.id) {
            self.next.insert(o.id, slot);
        } else {
            self.cur.insert(o.id, slot);
        }
        self.stats.opens += 1;
        true
    }

    /// Öffnen schlug fehl: erst nach [`RETRY_OPEN`] erneut
    pub fn open_failed(&mut self, o: Open, now: f64) {
        if self.opening.get(&o.id).is_some_and(|(k, _, q)| *k == o.key && *q == o.seq) {
            self.opening.remove(&o.id);
            self.failed.insert(o.id, (o.key, now));
        }
    }

    /// Fälliges Bild der Ebene (das neueste bis zur Uhr; ältere werden übersprungen). Ein Ersatz-
    /// strom übernimmt mit seinem ersten fälligen Bild. `primary`: Hauptebene (Lücken-Messung).
    pub fn pull(&mut self, id: Lid, now: f64, primary: bool) -> Option<S::Frame> {
        let pos = self.pos;
        let mut got = None;
        if let Some(n) = self.next.get_mut(&id) {
            if let Some(f) = n.drain(pos, now, &mut self.stats) {
                // erst übernehmen, wenn der Ersatz nicht hinter dem bisherigen Strom liegt (nie ein
                // älteres Bild zeigen); bis dahin holt er auf und seine Bilder verfallen
                let behind = |s: &Slot<S>| s.lag(pos).unwrap_or(f64::MAX);
                let old = self.cur.get(&id).filter(|c| !c.stalled(now) && !c.ended);
                if old.is_none_or(|c| behind(n) <= behind(c) + 1e-9) {
                    if let Some(n) = self.next.remove(&id) {
                        self.cur.insert(id, n);
                    }
                    got = Some(f);
                } else {
                    self.stats.dropped += 1;
                }
            }
        }
        let s = self.cur.get_mut(&id)?;
        if got.is_none() {
            got = s.drain(pos, now, &mut self.stats);
        }
        // Rückstand: länger als CATCH_UP_AFTER über zwei Bildintervalle → ab Uhr + Anlaufzeit neu
        if let Some(lag) = s.lag(pos).filter(|_| self.mode == Mode::Spielen) {
            self.stats.max_lag = self.stats.max_lag.max(lag);
            if lag > LAG_FRAMES / s.fps {
                let since = *s.behind_since.get_or_insert(now);
                if now - since >= CATCH_UP_AFTER && !self.next.contains_key(&id) && !self.opening.contains_key(&id) {
                    s.behind_since = None;
                    let at = pos + Time::from_secs_f64(s.lead());
                    self.reopen.insert(id, at);
                    self.stats.catch_ups += 1;
                    self.win_catch_up = true;
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

    /// Einzelbild für die Ebene: `true` = jetzt anfordern. Beim Abspielen (Rückwärts, Tempo,
    /// bewegte Bilder) wird ein laufender Auftrag nicht ersetzt, solange er jung ist – sonst
    /// bricht jeder neue den vorigen ab und es kommt nie ein Bild (Stillstand).
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

    /// Einzelbild geliefert: zeigen, wenn es das angeforderte ist
    pub fn still_done(&mut self, id: Lid, key: u64) -> bool {
        if self.requested.get(&id).is_some_and(|(k, _)| *k == key) {
            self.shown.insert(id, key);
            self.held = Some(id);
            true
        } else {
            false
        }
    }

    /// Nur diese Ebenen sind noch sichtbar bzw. gleich dran: alles andere freigeben
    pub fn visible(&mut self, ids: &[Lid]) {
        self.cur.retain(|id, _| ids.contains(id));
        self.next.retain(|id, _| ids.contains(id));
        self.opening.retain(|id, _| ids.contains(id));
        self.reopen.retain(|id, _| ids.contains(id));
        self.requested.retain(|id, _| ids.contains(id));
        self.shown.retain(|id, _| ids.contains(id));
    }
}

/// Automatische Qualität beim Abspielen: Stufe 0 = gewählte Qualität; jede Stufe darüber halbiert
/// die Decodiergröße bzw. halbiert zuletzt die Bildrate (siehe [`quality_steps`]).
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

/// Unter diesem Anteil der Soll-Bildrate (über ~1 s) eine Stufe kleiner …
pub const TOO_SLOW: f64 = 0.9;
/// … ab diesem Anteil so lange am Stück eine Stufe zurück
const GOOD: f64 = 0.98;
const UP_WAIT: f64 = 4.0;
/// Nach einem Wechsel: so lange nicht neu entscheiden (neue Ströme laufen an)
const SETTLE: f64 = 1.5;

impl AutoQuality {
    /// Ein Messfenster auswerten: `ratio` = gelieferte / fällige Bilder (schlechtester Strom),
    /// `max` = höchste Stufe (0 = aus). Im Stand zurück auf 0. Scheitert ein Hochschalten innerhalb
    /// von 3 s, wartet das nächste doppelt so lange (höchstens 30 s): kein Flattern.
    pub fn decide(&mut self, now: f64, ratio: Option<f64>, max: usize, playing: bool) -> usize {
        if !playing || max == 0 {
            // die gelernte Wartezeit bleibt (derselbe Rechner ist beim nächsten Play gleich schnell)
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

/// Stufen der automatischen Qualität: (Teiler der Decodiergröße, halbe Bildrate). Die vom Nutzer
/// gewählte Qualität `user_div` ist die Obergrenze; kleiner als ¼ wird nicht gerechnet, bei mehr
/// als 31 fps ist die letzte Stufe ¼ mit halber Bildrate.
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
