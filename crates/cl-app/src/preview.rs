//! Vorschau: Hintergrund-Decoder (Einzelbilder je Clip mit Effektkette, Thumbnails,
//! Waveforms, Effekt-Vorschaubilder, Projekt-Titelbilder) und Wiedergabe-Ströme.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use cl_core::{ClipId, MediaId, Rational, Time};
use cl_media::{FrameStream, RgbaFrame};
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
}

impl FrameCache {
    fn new(budget: usize) -> Self {
        Self {
            map: Default::default(),
            order: Default::default(),
            bytes: 0,
            budget,
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

/// Zwei Worker-Threads: einer für Bilder des Players (neuester Auftrag je Clip gewinnt),
/// einer für alles im Hintergrund (Thumbnails, Waveforms, Kacheln).
pub struct Decoder {
    tx: Sender<Job>,
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
                // 40 Balken über die ganze Länge, auf den lautesten normiert
                let n = 40;
                let max = peaks.iter().copied().fold(1e-4, f32::max);
                let bars = (0..n)
                    .map(|i| {
                        let a = i * peaks.len() / n;
                        let b = ((i + 1) * peaks.len() / n).max(a + 1).min(peaks.len());
                        peaks.get(a..b).map(|s| s.iter().copied().fold(0.0, f32::max)).unwrap_or(0.0) / max
                    })
                    .collect();
                Ok(Done::SoundMeta { key, media, bars })
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
                    Err(e) => eprintln!("Shader-Kachel {key}: {e}"),
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
        let (tx, jobs) = channel::<Job>();
        {
            let (done_tx, ctx) = (done_tx.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("cl-decoder".into())
                .spawn(move || {
                    let mut cache = FrameCache::new(384 << 20);
                    while let Ok(first) = jobs.recv() {
                        let mut pending = vec![first];
                        pending.extend(jobs.try_iter());
                        // je Clip nur den neuesten Auftrag
                        let mut keep: Vec<Job> = Vec::new();
                        for j in pending.into_iter().rev() {
                            let dup = match &j {
                                Job::Frame { clip, .. } => keep
                                    .iter()
                                    .any(|k| matches!(k, Job::Frame { clip: c, .. } if c == clip)),
                                _ => false,
                            };
                            if !dup {
                                keep.push(j);
                            }
                        }
                        // Bereits berechnete Bilder sofort aus dem Speicher, der Rest parallel
                        let mut todo = Vec::new();
                        for job in keep.into_iter().rev() {
                            match job {
                                Job::Frame { clip, key, .. } if cache.get(key).is_some() => {
                                    let frame = cache.get(key).unwrap().clone();
                                    if done_tx.send(Done::Frame { clip, key, frame }).is_err() {
                                        return;
                                    }
                                    ctx.request_repaint();
                                }
                                j => todo.push(j),
                            }
                        }
                        let results: Vec<Option<Done>> = std::thread::scope(|sc| {
                            let hs: Vec<_> = todo
                                .into_iter()
                                .map(|job| sc.spawn(move || run(job)))
                                .collect();
                            hs.into_iter().map(|h| h.join().ok().flatten()).collect()
                        });
                        for d in results.into_iter().flatten() {
                            if let Done::Frame { key, frame, .. } = &d {
                                cache.put(*key, frame.clone());
                            }
                            if done_tx.send(d).is_err() {
                                return;
                            }
                            ctx.request_repaint();
                        }
                    }
                })
                .expect("Decoder-Thread");
        }
        let strips_wanted: Arc<Mutex<HashSet<String>>> = Default::default();
        let generation = Arc::new(AtomicU64::new(0));
        let (tx_fx, jobs_fx) = channel::<(u64, Job)>();
        {
            let (done_tx, ctx) = (done_tx.clone(), ctx.clone());
            let (gen, wanted) = (generation.clone(), strips_wanted.clone());
            std::thread::Builder::new()
                .name("cl-decoder-fx".into())
                .spawn(move || {
                    while let Ok(first) = jobs_fx.recv() {
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
                let _ = self.tx.send(job);
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
    ) -> Option<Self> {
        FrameStream::open_fx(path, source_at, w, h, fps, graph)
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
}
