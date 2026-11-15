//! Vorschau: Hintergrund-Decoder (Einzelbilder je Clip mit Effektkette, Thumbnails,
//! Waveforms, Effekt-Vorschaubilder, Projekt-Titelbilder) und Wiedergabe-Ströme.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use cl_core::{ClipId, MediaId, Rational, Time};
use cl_media::{FrameStream, KillSwitch, RgbaFrame};
use eframe::egui;

/// Auflösung der Waveforms (Werte pro Sekunde)
pub const WAVE_PER_SEC: u32 = 50;

pub enum Job {
    /// Bild eines Clips (mit optionalem Filtergraphen `[0:v]` → `[out]`)
    Frame {
        clip: ClipId,
        key: u64,
        path: PathBuf,
        at: Time,
        w: u32,
        h: u32,
        graph: Option<String>,
    },
    Thumb {
        media: MediaId,
        path: PathBuf,
        at: Time,
    },
    Wave {
        media: MediaId,
        path: PathBuf,
    },
    /// Kleines Bild einer Datei bei `at` (Bibliothek, Hover-Vorschau); Ergebnis als `FxThumb`
    FileThumb {
        key: String,
        path: PathBuf,
        at: Time,
    },
    /// `n` Bilder gleichmäßig über ein Video (Hover-Vorschau im Medien-Tab), als ein Streifen
    MediaStrip {
        key: String,
        path: PathBuf,
        dur: Time,
        n: u32,
    },
    /// Sound aus der Bibliothek vorbereiten: ggf. erzeugen (`sfx`), prüfen, Wellenform
    SoundMeta {
        key: String,
        path: PathBuf,
        sfx: Option<&'static str>,
    },
    /// Vorschaubild aus einem Graphen ohne Eingänge (Effekt-/Filter-/Übergangskacheln)
    /// Kachel für Effekte/Filter/Übergänge. `frames` > 1: Filmstreifen (Bilder untereinander)
    FxThumb {
        key: String,
        graph: String,
        /// Shader-Übergang: Bilder mit cl-gpu mischen; ohne GPU oder bei Fehlern gilt `graph`
        shader: Option<ShaderTile>,
        w: u32,
        h: u32,
        frames: u32,
        cache: PathBuf,
    },
    /// Titelbild eines Projekts für die Startseite
    ProjectThumb {
        out: PathBuf,
        path: PathBuf,
        at: Time,
    },
}

/// Kachel eines Shader-Übergangs: `pair_graph` liefert A und B nebeneinander (Breite 2·w).
pub struct ShaderTile {
    pub file: PathBuf,
    pub params: Vec<f32>,
    pub pair_graph: String,
}

/// Balken der kleinen Sound-Wellenform (Zeilen und Kacheln)
pub const SOUND_BARS: usize = 24;

/// `n` Balken über die ganze Länge (Spitzenwert je Abschnitt), auf den lautesten normiert, 0…1.
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
        assert!(b.iter().any(|v| *v == 1.0), "lautester Balken voll");
        // Stille, leer und sehr kurz: feste Zahl, nichts außerhalb 0…1
        assert!(sound_bars(&[0.0; 100], 24).iter().all(|v| *v == 0.0));
        assert_eq!(sound_bars(&[], 24), vec![0.0; 24]);
        let short = sound_bars(&[0.2, 0.8, 2.0], 24);
        assert_eq!(short.len(), 24);
        assert!(short.iter().all(|v| (0.0..=1.0).contains(v)) && short.contains(&1.0));
    }
}

/// Filmstreifen eines Shader-Übergangs: `frames` Bilder bei Fortschritt k/frames (wie der
/// `xfade`-Streifen), A und B aus dem Bildpaar `pair` (2·w × h).
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

/// Parameter aus dem Katalog in das feste Feld von cl-gpu (Rest 0).
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
    },
    Thumb {
        media: MediaId,
        frame: RgbaFrame,
    },
    Wave {
        media: MediaId,
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
    /// Streifen-Auftrag übersprungen (Kachel nicht mehr sichtbar) → wieder anforderbar
    Skipped(String),
    Error(String),
}

/// Zuletzt berechnete Player-Bilder (Schlüssel enthält Clip, Quellzeit, Größe und Effekte),
/// damit erneutes Scrubben und Rückgängig ohne FFmpeg-Aufruf auskommen.
struct FrameCache {
    map: std::collections::HashMap<u64, RgbaFrame>,
    order: std::collections::VecDeque<u64>,
    bytes: usize,
    budget: usize,
    /// Projektgeneration der Bilder: nach einem Projektwechsel wird alles freigegeben
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
    /// Bilder eines verlassenen Projekts freigeben (sonst blieben bis zu `budget` Bytes belegt)
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

/// Drei Worker-Threads: einer verteilt die Bilder des Players (jeder Auftrag eigener Prozess,
/// veraltete werden abgebrochen), einer für alles im Hintergrund (Thumbnails, Waveforms), einer
/// für Kacheln. Hintergrund und Kacheln warten, solange Player-Bilder laufen.
pub struct Decoder {
    tx: Sender<Msg>,
    tx_bg: Sender<(u64, Job)>,
    /// Kacheln (Effekte, Filter, Übergänge): eigener Thread, bis zu vier FFmpeg parallel,
    /// damit das Vorladen Thumbnails und Waveforms der Medien nicht aufhält
    tx_fx: Sender<(u64, Job)>,
    pub rx: Receiver<Done>,
    /// Streifen-Schlüssel, die gerade sichtbar/gehovert sind (Medien-Tab setzt sie je Frame)
    pub strips_wanted: Arc<Mutex<HashSet<String>>>,
    /// Projektgeneration: Aufträge eines verlassenen Projekts verwirft der Worker
    pub generation: Arc<AtomicU64>,
}

/// Nachricht an den Verteiler der Player-Bilder: neuer Auftrag oder fertiges Ergebnis.
enum Msg {
    Job(Job),
    Done {
        clip: ClipId,
        key: u64,
        res: cl_media::Result<(RgbaFrame, cl_media::DecodeInfo)>,
        ms: f64,
        proxy: bool,
    },
}

/// Laufende Player-Bilder: höchstens ein Auftrag je Clip. Ein neuer Auftrag für denselben Clip
/// macht den alten zum veralteten (sein FFmpeg wird beendet, sein Ergebnis nicht gezeigt).
#[derive(Default)]
struct InFlight(HashMap<ClipId, (u64, Arc<KillSwitch>)>);

impl InFlight {
    /// `None` = derselbe Auftrag läuft schon; sonst der Schalter des neuen und ggf. des veralteten.
    fn start(&mut self, clip: ClipId, key: u64) -> Option<(Arc<KillSwitch>, Option<Arc<KillSwitch>>)> {
        if self.0.get(&clip).is_some_and(|(k, _)| *k == key) {
            return None;
        }
        let ks = Arc::new(KillSwitch::default());
        let old = self.0.insert(clip, (key, ks.clone())).map(|(_, k)| k);
        Some((ks, old))
    }
    /// Bild kam aus dem Speicher: laufender Auftrag des Clips ist veraltet.
    fn cancel(&mut self, clip: ClipId) -> Option<Arc<KillSwitch>> {
        self.0.remove(&clip).map(|(_, k)| k)
    }
    /// Ergebnis da: noch der aktuelle Auftrag des Clips?
    fn finish(&mut self, clip: ClipId, key: u64) -> bool {
        let cur = self.0.get(&clip).is_some_and(|(k, _)| *k == key);
        if cur {
            self.0.remove(&clip);
        }
        cur
    }
    fn len(&self) -> usize {
        self.0.len()
    }
}

/// Darf ein Hintergrund-Auftrag starten? Erst wenn kein Player-Bild läuft – höchstens 1,5 s
/// gewartet, damit Thumbnails beim Dauer-Scrubben nicht verhungern.
fn bg_may_start(player_busy: usize, waited: std::time::Duration) -> bool {
    player_busy == 0 || waited >= std::time::Duration::from_millis(1500)
}

fn yield_to_player(busy: &AtomicUsize) {
    let t = std::time::Instant::now();
    while !bg_may_start(busy.load(Ordering::Relaxed), t.elapsed()) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Verteiler der Player-Bilder (läuft im Thread `cl-decoder`).
fn frame_dispatcher(
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    done_tx: Sender<Done>,
    ctx: egui::Context,
    gen: Arc<AtomicU64>,
    busy: Arc<AtomicUsize>,
) {
    let mut cache = FrameCache::new(384 << 20);
    let mut inflight = InFlight::default();
    let proxies = crate::proxy::dir();
    loop {
        // auch ohne neue Aufträge nachsehen, ob das Projekt gewechselt wurde
        cache.set_generation(gen.load(Ordering::Relaxed));
        let msg = match rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(m) => m,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let out = match msg {
            Msg::Job(Job::Frame { clip, key, path, at, w, h, graph }) => {
                crate::proxy::note_activity();
                if let Some(frame) = cache.get(key).cloned() {
                    if let Some(k) = inflight.cancel(clip) {
                        k.kill();
                        perf_discarded();
                    }
                    Some(Done::Frame { clip, key, frame })
                } else {
                    if let Some((ks, old)) = inflight.start(clip, key) {
                        if let Some(o) = old {
                            o.kill();
                            perf_discarded();
                        }
                        let tx = tx.clone();
                        let proxy = path.starts_with(&proxies);
                        std::thread::spawn(move || {
                            let t = std::time::Instant::now();
                            let res = cl_media::decode_frame_opts(&path, at, w, h, graph.as_deref(), false, Some(&ks));
                            let ms = t.elapsed().as_secs_f64() * 1e3;
                            let _ = tx.send(Msg::Done { clip, key, res, ms, proxy });
                        });
                    }
                    None
                }
            }
            Msg::Job(j) => run(j),
            Msg::Done { clip, key, res, ms, proxy } => {
                crate::proxy::note_activity();
                let current = inflight.finish(clip, key);
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
                        current.then_some(Done::Frame { clip, key, frame })
                    }
                    Err(cl_media::MediaError::Cancelled) => None,
                    Err(e) => current.then(|| Done::Error(e.to_string())),
                }
            }
        };
        busy.store(inflight.len(), Ordering::Relaxed);
        if let Some(d) = out {
            if done_tx.send(d).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    }
}

fn perf_discarded() {
    if crate::app::perf() {
        crate::applog::perf_line!("perf: einzelbild verworfen (veraltet)");
    }
}

/// Auftrag noch nötig? `None` = verwerfen, `Some(Some(d))` = statt Ausführung melden.
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
        } => cl_media::decode_frame_fx(&path, at, w, h, graph.as_deref())
            .map(|frame| Done::Frame { clip, key, frame }),
        Job::Thumb { media, path, at } => {
            let (w, h) = cl_media::probe(&path)
                .map(|m| cl_media::fit_size(m.width, m.height, 192, 108))
                .unwrap_or((192, 108));
            cl_media::decode_frame(&path, at, w, h).map(|frame| Done::Thumb { media, frame })
        }
        Job::Wave { media, path } => {
            cl_media::waveform(&path, WAVE_PER_SEC).map(|peaks| Done::Wave { media, peaks })
        }
        Job::FileThumb { key, path, at } => {
            let (w, h) = cl_media::probe(&path)
                .map(|m| cl_media::fit_size(m.width.max(1), m.height.max(1), 192, 108))
                .unwrap_or((192, 108));
            cl_media::decode_frame(&path, at, w, h).map(|frame| Done::FxThumb { key, frame })
        }
        Job::MediaStrip { key, path, dur, n } => {
            let (w, h) = cl_media::probe(&path)
                .map(|m| cl_media::fit_size(m.width.max(1), m.height.max(1), 160, 90))
                .unwrap_or((160, 90));
            let mut data = Vec::with_capacity((w * h * 4 * n) as usize);
            let mut last: Option<Vec<u8>> = None;
            for i in 0..n {
                let at = Time::from_secs_f64(dur.as_secs_f64() * (i as f64 + 0.5) / n as f64);
                // fehlt ein Bild (z. B. kurz vor dem Ende), das vorige wiederholen
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
            // Zwischenspeicher: rohe RGBA-Bytes
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
                // neu schreiben, wenn eine ältere App-Version ein anderes Beispielbild abgelegt hat
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
            // Shader-Übergang mit cl-gpu; ohne GPU oder bei Fehlern die Kachel des Rückfalls
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
            // Rückfall eines Shader-Übergangs nicht festschreiben: beim nächsten Start neu versuchen
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
        let (done_tx, rx) = channel::<Done>();
        // Player-Bilder
        let (tx, jobs) = channel::<Msg>();
        let generation = Arc::new(AtomicU64::new(0));
        let busy = Arc::new(AtomicUsize::new(0));
        {
            let (done_tx, ctx, gen, busy, tx) = (done_tx.clone(), ctx.clone(), generation.clone(), busy.clone(), tx.clone());
            std::thread::Builder::new()
                .name("cl-decoder".into())
                .spawn(move || frame_dispatcher(jobs, tx, done_tx, ctx, gen, busy))
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
        // Hintergrund
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

    /// Projektwechsel: noch wartende Aufträge des alten Projekts verfallen
    pub fn next_generation(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

/// Laufender Wiedergabe-Strom für einen Clip.
pub struct Playback {
    pub stream: FrameStream,
    /// Zeitleisten-Position, an der der Strom begann.
    pub t0: Time,
    pub taken: u64,
    pub key: u64,
    /// Seit wann wird abgefragt, ohne dass ein fälliges Bild kommt
    pub waiting_since: Option<std::time::Instant>,
    /// FFmpeg hat den Strom beendet (Ende der Quelle oder Fehler)
    pub ended: bool,
}

impl Playback {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        path: &std::path::Path,
        source_at: Time,
        t0: Time,
        w: u32,
        h: u32,
        fps: Rational,
        graph: Option<&str>,
        key: u64,
        hw: bool,
    ) -> Option<Self> {
        FrameStream::open_opts(path, source_at, w, h, fps, graph, hw)
            .ok()
            .map(|stream| Self {
                stream,
                t0,
                taken: 0,
                key,
                waiting_since: None,
                ended: false,
            })
    }

    /// Holt alle Frames bis zur aktuellen Position und gibt den neuesten zurück.
    pub fn catch_up(&mut self, playhead: Time) -> Option<RgbaFrame> {
        let fps = self.stream.fps.as_f64();
        let target = ((playhead - self.t0).as_secs_f64() * fps).floor().max(0.0) as u64;
        let mut latest = None;
        while self.taken <= target {
            match self.stream.try_next() {
                Ok(Some(f)) => {
                    latest = Some(f);
                    self.taken += 1;
                }
                Ok(None) => break,
                Err(()) => {
                    self.ended = true;
                    break;
                }
            }
        }
        // Nur zählen, solange wirklich abgefragt wird: ein vorab geöffneter, noch nicht
        // gebrauchter Strom (z. B. während eines Übergangs) hängt nicht.
        if latest.is_some() || self.taken > target {
            self.waiting_since = None;
        } else {
            self.waiting_since.get_or_insert_with(std::time::Instant::now);
        }
        latest
    }

    /// Hängt der Strom (beendet oder seit über einer Sekunde abgefragt, ohne fälliges Bild)?
    /// Vor dem ersten Bild läuft noch die Suche in der Quelle: großzügiger warten, sonst
    /// würde ein langsamer Start immer wieder neu geöffnet und das Video liefe nie los.
    pub fn stalled(&self) -> bool {
        let limit = if self.taken == 0 { 6.0 } else { 1.0 };
        self.ended || self.waiting_since.is_some_and(|w| w.elapsed().as_secs_f64() > limit)
    }
}

/// Hardware-Decodierung für einen Abspiel-Strom? Nur für Originale, deren Software-Decodierung
/// bremst: über 1080p oder HEVC/AV1/VP9 ab 1080p (gemessen: 4K-HEVC-10-Bit 3,6 → 0,9 Kerne,
/// 4K-H.264 2,3 → 0,6). 1080p-H.264 und Proxys sind in Software schnell genug, und das Öffnen
/// eines Hardware-Decoders kostet 200–300 ms. Einzelbilder bleiben deshalb ganz in Software
/// (Klick → Bild mit D3D11VA 0,2–0,4 s langsamer).
pub fn stream_hw(m: &cl_core::MediaRef, path: &std::path::Path) -> bool {
    let codec = m.video_codec.as_deref().unwrap_or("").to_ascii_lowercase();
    path == m.path
        && (m.width > 1920
            || m.height > 1080
            || (m.width.min(m.height) >= 1080 && ["hevc", "av1", "vp9"].iter().any(|c| codec.contains(c))))
}

pub fn to_color_image(f: &RgbaFrame) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.data)
}

/// Beispielbild für Effekt-/Übergangskacheln (CC0-Bilder von Wikimedia Commons:
/// „Sunset timelapse on the beach in Libreville, Gabon“ und „Mavora Lake South“).
/// Läuft im Ordner der Kachel-Zwischenspeicher, siehe `Job::FxThumb`.
pub fn sample_source(w: u32, h: u32, warm: bool) -> String {
    let f = if warm { "fx-sample-a.jpg" } else { "fx-sample-b.jpg" };
    format!("movie={f},loop=loop=-1:size=1:start=0,setpts=N/25/TB,scale={w}:{h},setsar=1")
}

const SAMPLES: [(&str, &[u8]); 2] = [
    ("fx-sample-a.jpg", include_bytes!("../assets/fx-sample-a.jpg")),
    ("fx-sample-b.jpg", include_bytes!("../assets/fx-sample-b.jpg")),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Kachel eines Shader-Übergangs: Bildpaar wird richtig geteilt, Bild 0 = A, Streifen hat
    /// `frames` Bilder; ohne GPU ein Fehler (dann gilt die Kachel des Rückfalls).
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
        // links rot, rechts blau
        let pair: Vec<u8> = (0..h)
            .flat_map(|_| (0..w * 2).flat_map(|x| if x < w { [255, 0, 0, 255] } else { [0, 0, 255, 255] }))
            .collect();
        let r = shader_strip(&s, &pair, w, h, 4);
        if cl_gpu::gpu().is_none() {
            assert!(r.is_err());
            return;
        }
        let r = r.unwrap();
        assert_eq!(r.len(), (w * h * 4 * 4) as usize);
        assert!(r[..(w * h * 4) as usize].chunks(4).all(|p| p == [255, 0, 0, 255]));
        // Bild 2 von 4: halb gemischt
        let mid = &r[(w * h * 4 * 2) as usize..][..4];
        assert!(mid[0].abs_diff(128) <= 1 && mid[2].abs_diff(128) <= 1, "{mid:?}");
        assert!(shader_strip(&s, &pair[..8], w, h, 4).is_err());
    }

    #[test]
    fn newer_job_makes_older_one_stale() {
        let (a, b) = (ClipId::new(), ClipId::new());
        let mut f = InFlight::default();
        let (k1, old) = f.start(a, 1).unwrap();
        assert!(old.is_none());
        assert!(f.start(a, 1).is_none(), "gleicher Auftrag läuft schon");
        // Folgeklick: alter Auftrag wird abgebrochen, neuer startet sofort
        let (_, old) = f.start(a, 2).unwrap();
        assert!(Arc::ptr_eq(&old.unwrap(), &k1));
        f.start(b, 7).unwrap();
        assert_eq!(f.len(), 2);
        assert!(!f.finish(a, 1), "veraltetes Ergebnis wird nicht gezeigt");
        assert!(f.finish(a, 2));
        assert!(!f.finish(a, 2));
        // Bild aus dem Speicher: laufender Auftrag des Clips verfällt
        assert!(f.cancel(b).is_some() && f.cancel(b).is_none());
        assert_eq!(f.len(), 0);
    }

    #[test]
    fn background_waits_for_player_frames_but_not_forever() {
        let ms = std::time::Duration::from_millis;
        assert!(bg_may_start(0, ms(0)));
        assert!(!bg_may_start(2, ms(100)));
        assert!(bg_may_start(2, ms(1500)), "kein Verhungern beim Dauer-Scrubben");
    }

    #[test]
    fn hardware_streams_only_for_heavy_originals() {
        let m = |w: u32, h: u32, codec: &str| cl_core::MediaRef {
            id: cl_core::MediaId::new(),
            path: PathBuf::from("o.mp4"),
            kind: cl_core::MediaKind::Video,
            duration: Time::from_secs_f64(1.0),
            width: w,
            height: h,
            fps: Some(Rational::new(60, 1)),
            has_audio: false,
            video_codec: Some(codec.into()),
            audio_codec: None,
        };
        let orig = std::path::Path::new("o.mp4");
        assert!(stream_hw(&m(3840, 2160, "h264"), orig));
        assert!(stream_hw(&m(1920, 1080, "hevc"), orig));
        assert!(stream_hw(&m(1080, 1920, "hevc"), orig), "Hochkant");
        assert!(!stream_hw(&m(1920, 1080, "h264"), orig));
        assert!(!stream_hw(&m(1280, 720, "hevc"), orig));
        assert!(!stream_hw(&m(3840, 2160, "hevc"), std::path::Path::new("proxy.mp4")), "Proxy");
    }

    /// Wartet auf das Bild mit Schlüssel `key`; zählt andere gelieferte Player-Bilder.
    fn wait_frame(d: &Decoder, key: u64, other: &mut usize) -> std::time::Instant {
        loop {
            match d.rx.recv_timeout(std::time::Duration::from_secs(20)).expect("kein Bild") {
                Done::Frame { key: k, .. } if k == key => return std::time::Instant::now(),
                Done::Frame { .. } => *other += 1,
                Done::Error(e) => panic!("{e}"),
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
        Job::Frame { clip, key, path: path.to_path_buf(), at: Time::from_secs_f64(at), w: 696, h: 392, graph: None }
    }

    /// Mess-Test (von Hand): Scrubben – 20 Aufträge im Abstand von 15 ms an verschiedenen
    /// Stellen; gemessen wird die Zeit vom letzten Auftrag bis zu seinem Bild.
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

    /// Mess-Test (von Hand): Einzelbild ohne und während eines Proxy-Baus (der Bau hält an,
    /// sobald der Player Bilder anfordert).
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
                // Abstand wie beim Klicken
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
        // Obergrenze: älteste fallen heraus
        assert!(c.bytes <= 1000 && c.get(0).is_none() && c.get(4).is_some());
        c.set_generation(0);
        assert!(c.get(4).is_some());
        // Projektwechsel: alles frei
        c.set_generation(1);
        assert_eq!((c.bytes, c.map.len(), c.order.len()), (0, 0, 0));
        assert_eq!(c.budget, 1000);
    }
}
