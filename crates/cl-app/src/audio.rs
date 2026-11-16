//! Ton-Wiedergabe: FFmpeg mischt die Zeitleiste in Fenstern ab dem Playhead, cpal spielt sie ab.
//! Die abgespielten Samples sind die Uhr (Audio-Clock), nach der sich das Bild richtet.
//!
//! Gemischt wird nie das ganze Projekt, sondern ein Fenster von [`WINDOW_SECS`] (nur die Clips
//! darin, also wenige FFmpeg-Eingänge; das erste ist kürzer, damit der Ton schnell einsetzt). Bevor ein Fenster zu Ende geht, rechnet ein zweiter
//! Prozess das nächste; beide werden am Sample aneinandergehängt (jedes Fenster liefert genau
//! seine Zahl Sample-Frames, Einschwingen von Resampler/Begrenzer liegt vor dem Fensteranfang
//! und wird verworfen).
//!
//! Beim Abspielen startet das Bild sofort nach Systemzeit ([`AudioPlayer::start_synced`] +
//! [`AudioPlayer::anchor`]): kommt der Ton, überspringt die Ausgabe die schon vergangenen Samples
//! und setzt mit kurzer Einblendung genau an der Stelle ein, an der das Bild gerade ist.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::Read;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cl_core::{Project, Time};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Länge eines Ton-Fensters (Sekunden)
pub const WINDOW_SECS: u64 = 20;
/// Länge des ersten Fensters: weniger Eingänge → erster Ton schneller (je Eingang ~25–80 ms)
pub const FIRST_SECS: u64 = 6;
/// So viele Sekunden vor dem Fensterende wird das nächste Fenster gestartet
pub const LEAD_SECS: u64 = 5;
/// Einschwingzeit vor dem Fensteranfang (wird gerechnet und verworfen)
pub const PRE_MS: u64 = 200;
/// Ein-/Ausblenden beim Einsetzen und Abbrechen des Tons (gegen Knacken)
pub const FADE_MS: u64 = 15;

pub struct AudioPlayer {
    /// Steuerung des Ausgabe-Threads (true = abspielen, false = anhalten); fallen lassen beendet ihn
    ctl: std::sync::mpsc::Sender<bool>,
    /// Ausgabe ließ sich nicht öffnen (dann kommt nie Ton, das Bild läuft nach Systemzeit)
    failed: Arc<AtomicBool>,
    shared: Arc<Shared>,
    /// Spitzenpegel links/rechts seit der letzten Abfrage (f32-Bits)
    peaks: Arc<[AtomicU32; 2]>,
    rate: u32,
    pub start: Time,
}

/// Zustand zwischen Ausgabe (cpal), Zuführung (Fenster) und Oberfläche
struct Shared {
    out: Mutex<Out>,
    /// verbrauchte Frames (auch übersprungene) = Uhr
    played: AtomicU64,
    /// Zeitpunkt (ns seit `base`) und Frames des letzten Ausgabe-Rückrufs (für eine gleichmäßige Uhr)
    cb_ns: AtomicU64,
    cb_frames: AtomicU64,
    running: AtomicBool,
    paused: AtomicBool,
    base: Instant,
    stop: AtomicBool,
    /// Alle gestarteten FFmpeg-Prozesse (beim Beenden töten, damit blockierte Leser freikommen)
    kids: Mutex<Vec<Arc<Mutex<Child>>>>,
    first_sound: OnceLock<Instant>,
}

/// Gewähltes Ausgabegerät (Name); `None` = Systemstandard
static DEVICE: Mutex<Option<String>> = Mutex::new(None);

pub fn set_output_device(name: Option<String>) {
    *DEVICE.lock().unwrap() = name;
    *OUTPUT.lock().unwrap() = None;
}

/// Zuletzt genutztes Ausgabegerät mit Format (Name, Gerät, Format): Aufzählen der Geräte und
/// `default_output_config` kosten je Ton-Start spürbar Zeit im UI-Thread. Vergessen bei
/// Gerätewahl und wenn sich die Ausgabe nicht öffnen ließ (Gerät abgezogen).
type Output = (String, (cpal::Device, cpal::SupportedStreamConfig));
static OUTPUT: Mutex<Option<Output>> = Mutex::new(None);

/// Wert für `name` aus `slot`, sonst mit `fetch` holen und merken.
fn remembered<T: Clone, E>(slot: &mut Option<(String, T)>, name: &str, fetch: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    if let Some((_, v)) = slot.as_ref().filter(|(n, _)| n == name) {
        return Ok(v.clone());
    }
    let v = fetch()?;
    *slot = Some((name.to_string(), v.clone()));
    Ok(v)
}

/// Namen aller Ausgabegeräte (für die Einstellungen)
pub fn output_devices() -> Vec<String> {
    cpal::default_host()
        .output_devices()
        .map(|d| d.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

// ---------- Fenster-Plan (reine Rechnung) ----------

/// Ein Ton-Fenster: FFmpeg mischt `mix_from..mix_to`, davon werden `skip` Frames verworfen und
/// genau `frames` Frames übernommen (fehlt etwas, wird mit Stille aufgefüllt).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowPlan {
    pub mix_from: Time,
    pub mix_to: Time,
    pub skip: u64,
    /// erster Frame des Fensters, gezählt ab dem Start der Wiedergabe
    pub first: u64,
    pub frames: u64,
}

/// Sample-Frames von `from` bis `end` (aufgerundet: die Uhr erreicht das Ende sicher)
pub fn total_frames(from: Time, end: Time, rate: u32) -> u64 {
    let us = (end - from).0.max(0) as u128;
    (us * rate as u128).div_ceil(1_000_000) as u64
}

/// Fenster `k` einer Wiedergabe ab `from` bis `end`; `None` hinter dem Ende.
/// Fensteranfänge liegen auf ganzen Sekunden nach `from`, das Einschwingen auf ganzen Frames davor:
/// so rechnet jedes Fenster auf demselben Sample-Raster wie ein durchgehender Mix.
pub fn window_plan(from: Time, end: Time, rate: u32, k: u64) -> Option<WindowPlan> {
    let total = total_frames(from, end, rate);
    // Anfang (Sekunden ab `from`) und Länge: erst FIRST_SECS, dann je WINDOW_SECS
    let (start_s, len_s) = match k {
        0 => (0, FIRST_SECS),
        _ => ((k - 1).checked_mul(WINDOW_SECS)?.checked_add(FIRST_SECS)?, WINDOW_SECS),
    };
    let first = start_s.checked_mul(rate as u64).filter(|f| *f < total)?;
    let frames = (len_s * rate as u64).min(total - first);
    // `first < total`: der Anfang liegt im Projekt, kein Überlauf
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

/// Nächstes Fenster vorbereiten? (`moved` = schon übernommene Frames des laufenden Fensters)
pub fn prepare_next(moved: u64, frames: u64, rate: u32) -> bool {
    moved + LEAD_SECS * rate as u64 >= frames
}

// ---------- Ausgabe (reine Zustandsmaschine, vom cpal-Rückruf benutzt) ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Ton noch nicht freigegeben (Bild puffert): Stille, nichts verbrauchen
    Waiting,
    /// Uhr läuft seit dem Anker: vergangene Samples überspringen, dann einsetzen
    Anchored(Instant),
    /// sofort spielen, sobald Ton da ist (Scrubben, Vorhören)
    Free,
    Running,
}

struct Out {
    buf: VecDeque<f32>,
    phase: Phase,
    played: u64,
    fade_len: u32,
    fade_in: u32,
    /// verbleibende Ausblend-Frames beim Abbrechen (danach Stille)
    closing: Option<u32>,
    /// Zuführung fertig (kein Ton mehr zu erwarten)
    finished: bool,
    in_underrun: bool,
    underruns: u64,
    underrun_frames: u64,
}

impl Out {
    fn new(phase: Phase, rate: u32) -> Self {
        Self {
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
        }
    }

    /// `n` Stereo-Frames ausgeben; `target` = Soll-Frames seit dem Anker (nur `Anchored`).
    /// Gibt die Zahl der ausgegebenen echten Ton-Frames zurück.
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
            Phase::Running => {}
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
}

/// Uhr in Frames: Stand vor dem letzten Rückruf plus die seither vergangene Zeit, höchstens so viele
/// Frames, wie der Rückruf ausgegeben hat (sonst liefe die Uhr in einem Aussetzer weiter).
/// `played` zählt den letzten Rückruf schon mit; ohne Abzug liefe die Uhr ihm um einen Puffer voraus.
pub fn clock_frames(played: u64, since_cb: Duration, cb_frames: u64, rate: u32) -> u64 {
    played.saturating_sub(cb_frames) + ((since_cb.as_secs_f64() * rate as f64) as u64).min(cb_frames)
}

// ---------- Zuführung: Fenster für Fenster ----------

/// Ein laufendes Fenster: FFmpeg + Lese-Thread, der genau `frames` Frames in `q` legt.
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

/// Startet das FFmpeg eines Fensters. Aufgerufen aus dem Thread, der den Ton startet (Oberfläche
/// bzw. der Scrub-Thread, der den Ton bis zum Ende hält), und aus `cl-audio-feed`, der bis zum
/// Ende des Tons lebt – wichtig unter Linux: `PR_SET_PDEATHSIG` beendet das Kind mit dem
/// *startenden Thread* (siehe `cl_media::proc`), nicht erst mit dem Prozess.
fn open_window(project: &Project, plan: WindowPlan, rate: u32, shared: &Arc<Shared>) -> Result<Win, String> {
    let (mut child, workdir) = cl_export::spawn_audio_window(project, plan.mix_from, plan.mix_to, rate)
        .map_err(|e| e.to_string())?;
    cl_media::raise_priority(&child);
    let mut stdout = child.stdout.take().ok_or("kein stdout")?;
    let child = Arc::new(Mutex::new(child));
    {
        let mut kids = shared.kids.lock().unwrap();
        kids.retain(|k| k.lock().map(|mut c| c.try_wait().ok().flatten().is_none()).unwrap_or(false));
        kids.push(child.clone());
    }
    let q = Arc::new(Mutex::new(VecDeque::with_capacity(rate as usize * 2)));
    let done = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(OnceLock::new());
    let cap = rate as usize * 2; // 1 s Stereo je Fenster im Voraus
    let (q2, done2, ready2, sh) = (q.clone(), done.clone(), ready.clone(), shared.clone());
    std::thread::Builder::new()
        .name("cl-audio-window".into())
        .spawn(move || {
            let _workdir = workdir; // Graph-Datei lebt, bis FFmpeg fertig ist
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
            // FFmpeg lieferte zu wenig (Rundung am Projektende, Fehler): mit Stille auffüllen,
            // damit das nächste Fenster an der richtigen Stelle anschließt
            if want > 0 && !sh.stop.load(Ordering::Relaxed) {
                if crate::app::perf() {
                    crate::applog::perf_line!("perf: Ton-Fenster ab Frame {} mit {} Frames Stille aufgefüllt", plan.first, want / 2);
                }
                ready2.get_or_init(Instant::now);
                q2.lock().unwrap().extend(std::iter::repeat_n(0.0, want as usize));
            }
            done2.store(true, Ordering::Release);
        })
        .map_err(|e| e.to_string())?;
    Ok(Win { child, q, done, ready, plan, spawned: Instant::now() })
}

/// Holt Fenster für Fenster in den Ausgabepuffer (höchstens `max_buffered` Samples voraus).
fn feed(project: Project, from: Time, rate: u32, shared: Arc<Shared>, mut cur: Win) {
    let end = project.duration();
    let max_buffered = rate as usize * 2; // 1 s Stereo
    let mut next: Option<Win> = None;
    let mut moved = 0u64;
    let mut k = 0u64;
    while !shared.stop.load(Ordering::Relaxed) {
        if next.is_none() && prepare_next(moved, cur.plan.frames, rate) {
            if let Some(plan) = window_plan(from, end, rate, k + 1) {
                match open_window(&project, plan, rate, &shared) {
                    Ok(w) => next = Some(w),
                    Err(e) => {
                        log::warn!("Ton: {e}");
                        break;
                    }
                }
            }
        }
        let room = max_buffered.saturating_sub(shared.out.lock().unwrap().buf.len()) / 2 * 2;
        let mut got = 0;
        if room > 0 {
            let mut q = cur.q.lock().unwrap();
            got = room.min(q.len());
            if got > 0 {
                let mut o = shared.out.lock().unwrap();
                o.buf.extend(q.drain(..got));
            }
        }
        moved += got as u64 / 2;
        let exhausted = cur.done.load(Ordering::Acquire) && cur.q.lock().unwrap().is_empty();
        if exhausted && moved >= cur.plan.frames {
            let Some(n) = next.take() else { break };
            if crate::app::perf() {
                // Vorlauf: wie lange das nächste Fenster schon Ton bereithielt; Versatz: Abweichung
                // des Fensteranfangs vom durchgehenden Sample-Zähler (muss 0 sein)
                let lead = n.ready.get().map(|r| r.elapsed().as_secs_f64() * 1000.0);
                crate::applog::perf_line!(
                    "perf: Ton-Fensterwechsel bei Frame {}: Vorlauf {} ms (Start nach {:.0} ms), Versatz {} Samples",
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
    shared.out.lock().unwrap().finished = true;
}

/// Ein Ausgabe-Block von `n` Frames zur Zeit `now`: Uhr fortschreiben, Ton (oder Stille) an `put`
/// geben, Pegel und ersten Ton merken. Gemeinsam für das Gerät und die Ausgabe ohne Gerät.
fn render_block(sh: &Shared, pk: &[AtomicU32; 2], rate: u32, n: usize, now: Instant, mut put: impl FnMut(f32, f32)) {
    let mut o = sh.out.lock().unwrap();
    let target = match o.phase {
        Phase::Anchored(at) => (now.saturating_duration_since(at).as_secs_f64() * rate as f64) as u64,
        _ => 0,
    };
    let (mut pl, mut pr) = (0.0f32, 0.0f32);
    let real = o.render(n, target, |l, r| {
        pl = pl.max(l.abs());
        pr = pr.max(r.abs());
        put(l, r);
    });
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
    /// Spielt ab `from`, sobald Ton da ist (Scrubben, Vorhören).
    pub fn start(project: &Project, from: Time) -> Result<Self, String> {
        Self::open(Cow::Borrowed(project), from, false, false)
    }

    /// Für die Wiedergabe mit Bild: Ton wird vorbereitet, setzt aber erst nach [`Self::anchor`] ein –
    /// dann an der Stelle, an der die Uhr seit dem Anker steht. Ein eigener Klon (z. B. mit Solo)
    /// geht ohne weitere Kopie an die Zuführung.
    pub fn start_synced(project: Cow<'_, Project>, from: Time) -> Result<Self, String> {
        Self::open(project, from, true, false)
    }

    /// Wie [`Self::start_synced`], aber ohne Gerät: ein Thread verbraucht die Samples im Takt der
    /// Systemzeit (Mess-Test ohne Fenster und ohne hörbaren Ton; Uhr, Aussetzer und Fenster wie echt).
    #[cfg(test)]
    pub fn start_null(project: &Project, from: Time) -> Result<Self, String> {
        Self::open(Cow::Borrowed(project), from, true, true)
    }

    fn open(project: Cow<'_, Project>, from: Time, synced: bool, null: bool) -> Result<Self, String> {
        let (device, config) = if null {
            (None, None)
        } else {
            let host = cpal::default_host();
            // gewähltes Gerät (aus dem Speicher, sonst aufzählen); ist es nicht (mehr) da, der Systemstandard
            let wanted = DEVICE.lock().unwrap().clone();
            let mut out = OUTPUT.lock().unwrap();
            let hit = wanted.as_ref().and_then(|w| out.as_ref().filter(|(n, _)| n == w).map(|(_, v)| v.clone()));
            let (device, config) = match hit {
                Some(v) => v,
                None => {
                    let device = wanted
                        .and_then(|w| host.output_devices().ok()?.find(|d| d.name().ok().as_deref() == Some(w.as_str())))
                        .or_else(|| host.default_output_device())
                        .ok_or("Kein Audio-Ausgabegerät")?;
                    let name = device.name().unwrap_or_default();
                    remembered(&mut out, &name, || device.default_output_config().map(|c| (device.clone(), c)).map_err(|e| e.to_string()))?
                }
            };
            (Some(device), Some(config))
        };
        let rate = config.as_ref().map_or(48_000, |c| c.sample_rate().0);
        let channels = config.as_ref().map_or(2, |c| c.channels() as usize);

        let end = project.duration();
        let plan = window_plan(from, end, rate, 0).ok_or_else(|| cl_export::ExportError::Empty.to_string())?;
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
        });
        let first = open_window(&project, plan, rate, &shared)?;
        {
            let (p, sh) = (project.into_owned(), shared.clone());
            std::thread::Builder::new()
                .name("cl-audio-feed".into())
                .spawn(move || feed(p, from, rate, sh, first))
                .map_err(|e| e.to_string())?;
        }

        let peaks: Arc<[AtomicU32; 2]> = Arc::new([AtomicU32::new(0), AtomicU32::new(0)]);
        // Das Öffnen der Ausgabe (WASAPI) dauert mehrere hundert Millisekunden: im eigenen Thread,
        // damit die Oberfläche beim Abspielen nicht steht. Der cpal-Strom ist nicht Send und lebt
        // deshalb ganz dort; er endet, sobald `ctl` fallen gelassen wird.
        let (ctl, ctl_rx) = std::sync::mpsc::channel::<bool>();
        let failed = Arc::new(AtomicBool::new(false));
        let (sh, pk, fail) = (shared.clone(), peaks.clone(), failed.clone());
        let (Some(device), Some(config)) = (device, config) else {
            // ohne Gerät: alle 10 ms die seither fälligen Frames verbrauchen
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
                            render_block(&sh, &pk, rate, n, now, |_, _| {});
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            return Ok(Self { ctl, failed, shared, peaks, rate, start: from });
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
                            render_block(&sh, &pk, rate, n, Instant::now(), |l, r| {
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
                f => return Err(format!("Sample-Format {f:?} nicht unterstützt")),
            }
            .map_err(|e| e.to_string())?;
            stream.play().map_err(|e| e.to_string())?;
            while let Ok(play) = ctl_rx.recv() {
                let _ = if play { stream.play().is_ok() } else { stream.pause().is_ok() };
            }
            // Abbruch: kurz ausblenden statt hart abzuschneiden (kein Knacken)
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
                    log::warn!("Ton: {e}");
                    fail.store(true, Ordering::Relaxed);
                    // Gerät abgezogen o. Ä.: beim nächsten Start neu suchen
                    *OUTPUT.lock().unwrap() = None;
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { ctl, failed, shared, peaks, rate, start: from })
    }

    /// Uhr freigeben: ab `at` läuft die Zeit; der Ton setzt an der dann aktuellen Stelle ein.
    pub fn anchor(&self, at: Instant) {
        let mut o = self.shared.out.lock().unwrap();
        if o.phase == Phase::Waiting {
            o.phase = Phase::Anchored(at);
        }
    }

    /// Ausgabe anhalten bzw. fortsetzen (Puffer und Uhr bleiben erhalten → sofort weiter)
    pub fn pause(&self) -> bool {
        self.shared.paused.store(true, Ordering::Relaxed);
        !self.failed() && self.ctl.send(false).is_ok()
    }
    pub fn resume(&self) -> bool {
        self.shared.paused.store(false, Ordering::Relaxed);
        !self.failed() && self.ctl.send(true).is_ok()
    }

    /// Ausgabe ließ sich nicht öffnen (es kommt kein Ton mehr)
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Aktuelle Wiedergabeposition nach Audio-Clock (gleichmäßig zwischen den Ausgabe-Rückrufen).
    pub fn position(&self) -> Time {
        let sh = &self.shared;
        let played = sh.played.load(Ordering::Relaxed);
        let frames = if sh.running.load(Ordering::Acquire) {
            let since = sh.base.elapsed().saturating_sub(Duration::from_nanos(sh.cb_ns.load(Ordering::Relaxed)));
            clock_frames(played, since, sh.cb_frames.load(Ordering::Relaxed), self.rate)
        } else {
            played
        };
        self.start + Time::from_secs_f64(frames as f64 / self.rate as f64)
    }

    /// Spitzenpegel (0…1+) links/rechts seit dem letzten Aufruf
    pub fn take_peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.peaks[0].swap(0, Ordering::Relaxed)),
            f32::from_bits(self.peaks[1].swap(0, Ordering::Relaxed)),
        )
    }

    /// Gibt schon Ton aus (vorher läuft das Bild nach Systemzeit bzw. steht beim Vorhören).
    pub fn started(&self) -> bool {
        self.shared.running.load(Ordering::Acquire)
    }

    /// Ton ist zu Ende (Projektende oder FFmpeg brach ab): die Uhr bleibt stehen.
    pub fn finished(&self) -> bool {
        let o = self.shared.out.lock().unwrap();
        o.finished && o.buf.is_empty()
    }

    /// Zeitpunkt des ersten hörbaren Samples
    pub fn first_sound(&self) -> Option<Instant> {
        self.shared.first_sound.get().copied()
    }

    /// Aussetzer (Puffer leer, obwohl noch Ton kommt) seit dem letzten Aufruf: (Anzahl, Dauer)
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
        // Ausgabe-Thread endet mit `ctl` (nach dem Ausblenden), Lese-Threads nach dem Töten
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000;

    /// Befund aus dem Review: jeder Ton-Start zählte die Geräte auf und holte das Format neu.
    /// Jetzt nur beim ersten Mal je Gerät (und nach Wechsel bzw. Vergessen).
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
        assert_eq!(get(&mut slot, "A", 1), Ok(48_000), "aus dem Speicher");
        assert_eq!(get(&mut slot, "B", 44_100), Ok(44_100), "anderes Gerät: neu");
        slot = None; // Gerät gewechselt oder Ausgabe scheiterte
        assert_eq!(get(&mut slot, "B", 96_000), Ok(96_000));
        assert_eq!(calls, 3);
        // Fehler werden nicht gemerkt
        let mut s2: Option<(String, u32)> = None;
        assert_eq!(remembered(&mut s2, "C", || Err::<u32, _>("weg")), Err("weg"));
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
                    assert_eq!(w.first, next, "lückenlos: {rate} {k}");
                    assert!(w.frames > 0 && w.frames <= WINDOW_SECS * rate as u64);
                    assert!(k > 0 || w.frames <= FIRST_SECS * rate as u64, "erstes Fenster kurz");
                    // Fensteranfang liegt auf dem Raster der Wiedergabe: Frame `first` ab `from`
                    let start = w.mix_from.0 as i128 * rate as i128 + w.skip as i128 * 1_000_000;
                    let want = from.0 as i128 * rate as i128 + w.first as i128 * 1_000_000;
                    assert!((start - want).abs() <= 1_000_000, "Raster {rate} {k}: {start} {want}");
                    if from.0 > PRE_MS as i64 * 1000 || k > 0 {
                        assert_eq!(start, want, "genau auf dem Raster");
                        assert_eq!(w.skip, rate as u64 * PRE_MS / 1000, "Einschwingen");
                    }
                    assert!(w.mix_to >= from + Time((w.first + w.frames) as i64 * S / rate as i64));
                    next += w.frames;
                    k += 1;
                }
                assert_eq!(next, total_frames(from, end, rate), "alles abgedeckt");
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
        // letztes Fenster endet am Projektende: 90+6+5×20 = 196 … 200
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
        // kurzes (erstes oder letztes) Fenster: sofort
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
        let mut o = Out::new(Phase::Waiting, 1000); // 1000 Hz: 15 Frames Einblenden
        feed_ramp(&mut o, 0, 100);
        // vor dem Anker: Stille, nichts verbraucht (Bild puffert noch)
        assert!(take(&mut o, 10, 0).iter().all(|s| *s == (0.0, 0.0)));
        assert_eq!(o.played, 0);
        o.phase = Phase::Anchored(Instant::now());
        // Uhr steht schon bei 40 Frames: die ersten 40 werden übersprungen, Ton setzt bei 40 ein
        let v = take(&mut o, 30, 40);
        assert_eq!(o.phase, Phase::Running);
        assert_eq!(o.played, 70, "Uhr = übersprungene + gespielte Frames, kein Sprung");
        assert_eq!(v[0], (0.0, -0.0), "Einblenden beginnt bei 0");
        assert!((v[1].0 - 41.0 / 15.0).abs() < 1e-4, "Rampe: {:?}", v[1]);
        assert_eq!(v[20], (60.0, -60.0), "nach dem Einblenden unverändert");
    }

    #[test]
    fn anchored_waits_while_audio_is_behind_the_clock() {
        let mut o = Out::new(Phase::Waiting, 1000);
        o.phase = Phase::Anchored(Instant::now());
        feed_ramp(&mut o, 0, 20);
        // Uhr bei 50, erst 20 Frames da: alle verwerfen, weiter Stille
        assert!(take(&mut o, 10, 50).iter().all(|s| *s == (0.0, 0.0)));
        assert_eq!(o.played, 20);
        assert!(matches!(o.phase, Phase::Anchored(_)));
        feed_ramp(&mut o, 20, 100);
        let v = take(&mut o, 40, 60);
        assert_eq!(o.played, 100);
        assert_eq!(v[30], (90.0, -90.0), "setzt bei Frame 60 ein");
        assert_eq!(o.underruns, 0, "kein Aussetzer vor dem Einsetzen");
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
        assert_eq!(o.underruns, 2, "Ende ist kein Aussetzer");
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

    #[test]
    fn clock_is_smooth_between_callbacks_and_stops_in_a_gap() {
        let r = 48_000;
        // `played` = 1480 nach einem Rückruf über 480 Frames
        assert_eq!(clock_frames(1480, Duration::ZERO, 480, r), 1000);
        assert_eq!(clock_frames(1480, Duration::from_millis(5), 480, r), 1240);
        // höchstens bis zum Ende dessen, was ausgegeben wurde (auch in der Pause)
        assert_eq!(clock_frames(1480, Duration::from_millis(50), 480, r), 1480);
        assert_eq!(clock_frames(1000, Duration::from_millis(50), 0, r), 1000);
        // nächster Rückruf schließt nahtlos an
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
        })
    }

    /// Wiedergabe-Ton wie in der App (Fenster für Fenster über die Zuführung, ohne Ausgabegerät)
    fn windowed(p: &Project, from: Time, rate: u32) -> Vec<f32> {
        let shared = test_shared(rate);
        let first = open_window(p, window_plan(from, p.duration(), rate, 0).unwrap(), rate, &shared).unwrap();
        let sh = shared.clone();
        let pp = p.clone();
        let t = std::thread::spawn(move || feed(pp, from, rate, sh, first));
        let mut all = Vec::new();
        loop {
            let mut o = shared.out.lock().unwrap();
            all.extend(o.buf.drain(..));
            if o.finished && o.buf.is_empty() {
                break;
            }
            drop(o);
            std::thread::sleep(Duration::from_millis(2));
        }
        t.join().unwrap();
        all
    }

    /// Bisheriger Weg: ein Mix vom Startpunkt bis zum Projektende
    fn whole(p: &Project, from: Time, rate: u32) -> Vec<f32> {
        let (mut child, _wd) = cl_export::spawn_audio_preview(p, from, rate).unwrap();
        let mut buf = Vec::new();
        child.stdout.as_mut().unwrap().read_to_end(&mut buf).unwrap();
        child.wait().unwrap();
        buf.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    }

    /// Ganze Kette mit echtem FFmpeg (ohne Ausgabegerät): Fenster für Fenster in den Puffer.
    #[test]
    fn feeder_delivers_exact_frame_count_across_windows() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = test_project(dir.path(), 26.0);
        let (rate, from) = (8000, Time(3 * S));
        assert_eq!(windowed(&p, from, rate).len() as u64, 2 * total_frames(from, p.duration(), rate));
    }

    /// Projekt mit allem, was den Ton formt: Schnitte derselben Datei, Blenden, Lautstärke-Keyframes,
    /// Tempo ≠ 1, rückwärts, ein Clip über die Fenstergrenze (bei Start 1,5 s liegt sie bei 21,5 s).
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
        // Tonhöhe ändert sich laufend: jeder Versatz um ein Sample fiele auf
        let a = tone("a.wav", "aevalsrc=0.5*sin(2*PI*(200+20*t)*t):s=48000:d=40");
        let b = tone("b.wav", "sine=f=523:d=40:sample_rate=44100");
        let mut p = Project::new("fenster");
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
        // über beide Fenstergrenzen, mit Lautstärke-Keyframes und Ausblenden
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

    /// Fenster-Mix über zwei Fenster = bisheriger Gesamt-Mix desselben Abschnitts.
    #[test]
    fn windowed_mix_matches_whole_mix() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let p = rich_project(dir.path());
        assert!(p.duration() > Time((FIRST_SECS + WINDOW_SECS + 2) as i64 * S), "drei Fenster");
        for (rate, from, tol) in [(48_000, Time(1_500_000), 1e-6f32), (44_100, Time(1_500_000), 2e-3), (48_000, Time(100_000), 1e-6)] {
            let w = windowed(&p, from, rate);
            let full = whole(&p, from, rate);
            let n = total_frames(from, p.duration(), rate) as usize * 2;
            assert_eq!(w.len(), n, "{rate}: genaue Länge");
            assert!(full.len() + 2 * rate as usize / 100 >= n, "{rate}: {} {n}", full.len());
            let cmp = full.len().min(n);
            let (i, d) = (0..cmp).map(|i| (i, (w[i] - full[i]).abs())).fold((0, 0.0f32), |a, b| if b.1 > a.1 { b } else { a });
            let shifted = (0..cmp - 2).map(|i| (w[i] - full[i + 2]).abs()).fold(0.0f32, f32::max);
            assert!(shifted > 0.1, "Test erkennt einen Versatz um ein Sample: {shifted}");
            assert!(d <= tol, "{rate} ab {from:?}: größte Abweichung {d} bei {:.4} s", i as f64 / 2.0 / rate as f64);
            // an den Nähten kein Sprung: der Schritt dort ist nicht größer als sonst
            for s in [FIRST_SECS, FIRST_SECS + WINDOW_SECS] {
                let seam = s as usize * rate as usize * 2;
                let step = |i: usize| (w[i] - w[i - 2]).abs();
                let usual = (seam - 200..seam - 2).map(step).fold(0.0f32, f32::max);
                assert!(step(seam) <= usual * 1.5 + 1e-4, "{rate}: Naht {} / sonst {usual}", step(seam));
            }
        }
    }

    /// Zeit bis zum ersten Ton-Sample an der Startstelle (Aufruf + FFmpeg + Einschwingen)
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

    /// Messung von Hand: `CUTLINE_QA_DIR=<Ordner mit PL_schnitt.cutline> cargo test --release -p cl-app
    /// -- --ignored first_audio_sample --nocapture`. Bisher (ganzer Mix bis Projektende) gegen Fenster.
    #[test]
    #[ignore]
    fn first_audio_sample_pl_schnitt() {
        let Some(dir) = std::env::var_os("CUTLINE_QA_DIR") else { return };
        let mut ok = false;
        for name in ["PL_schnitt.cutline", "PL_gross.cutline"] {
            let Ok(json) = std::fs::read_to_string(std::path::Path::new(&dir).join(name)) else { continue };
            let p = Project::from_json(&json).unwrap();
            if p.media.iter().any(|m| m.is_local() && !m.path.exists()) {
                eprintln!("{name}: Medien fehlen, übersprungen");
                continue;
            }
            ok = true;
            let rate = 48_000;
            for secs in [3.0, 37.0, 128.0, 335.0, 571.0] {
                let from = Time::from_secs_f64(secs);
                let plan = window_plan(from, p.duration(), rate, 0).unwrap();
                let n_old = cl_export::audio_clips_in(&p, from, p.duration()).len();
                let n_new = cl_export::audio_clips_in(&p, plan.mix_from, plan.mix_to).len();
                // je zweimal, der zweite Wert zählt (Dateien im Cache wie in der App)
                let mut old = 0.0;
                let mut new = 0.0;
                for _ in 0..2 {
                    old = first_sample_ms(|| cl_export::spawn_audio_preview(&p, from, rate).unwrap(), 0);
                    new = first_sample_ms(|| cl_export::spawn_audio_window(&p, plan.mix_from, plan.mix_to, rate).unwrap(), plan.skip);
                }
                eprintln!("{name} ab {secs:>5.0} s: bisher {old:>6.0} ms ({n_old} Eingänge) · Fenster {new:>5.0} ms ({n_new} Eingänge)");
            }
        }
        if !ok {
            eprintln!("keine Testprojekte in CUTLINE_QA_DIR");
        }
    }

    /// Messung von Hand mit dem echten Ausgabegerät (spielt nur Stille): Start → erstes Sample am
    /// Gerät (Öffnen der Ausgabe + FFmpeg + Zuführung), synchron gestartet wie beim Abspielen.
    /// `cargo test --release -p cl-app -- --ignored output_start_latency --nocapture`
    #[test]
    #[ignore]
    fn output_start_latency() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut p = test_project(dir.path(), 30.0);
        p.timeline.tracks[0].muted = true; // Stille aus eigener Quelle
        for i in 0..5 {
            let t0 = Instant::now();
            let a = AudioPlayer::start_synced(Cow::Borrowed(&p), Time(S)).unwrap();
            let opened = t0.elapsed();
            a.anchor(t0);
            while a.first_sound().is_none() && t0.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(1));
            }
            let first = a.first_sound().map(|t| t.duration_since(t0));
            // Uhr nach der Übergabe = Zeit seit dem Anker (kein Sprung)
            let diff = a.position().as_secs_f64() - 1.0 - t0.elapsed().as_secs_f64();
            eprintln!("Lauf {i}: Aufruf {:.0} ms, erstes Sample {:?} ms, Uhr − Systemzeit {:+.1} ms", opened.as_secs_f64() * 1e3, first.map(|d| (d.as_secs_f64() * 1e3).round()), diff * 1e3);
            assert!(diff.abs() < 0.005, "Übergabe ohne Sprung: {diff}");
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
