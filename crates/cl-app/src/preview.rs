//! Vorschau: Hintergrund-Decoder (Einzelbilder je Clip mit Effektkette, Thumbnails,
//! Waveforms, Effekt-Vorschaubilder, Projekt-Titelbilder) und Wiedergabe-Ströme.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc::{channel, Receiver, Sender};

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
    /// Vorschaubild aus einem Graphen ohne Eingänge (Effekt-/Filter-/Übergangskacheln)
    FxThumb {
        key: String,
        graph: String,
        w: u32,
        h: u32,
        cache: PathBuf,
    },
    /// Titelbild eines Projekts für die Startseite
    ProjectThumb {
        out: PathBuf,
        path: PathBuf,
        at: Time,
    },
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
    FxThumb {
        key: String,
        frame: RgbaFrame,
    },
    ProjectThumb {
        out: PathBuf,
    },
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
    tx_bg: Sender<Job>,
    pub rx: Receiver<Done>,
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
        Job::FxThumb {
            key,
            graph,
            w,
            h,
            cache,
        } => {
            // Zwischenspeicher: rohe RGBA-Bytes
            if let Ok(data) = std::fs::read(&cache) {
                if data.len() == (w * h * 4) as usize {
                    return Some(Done::FxThumb {
                        key,
                        frame: RgbaFrame {
                            width: w,
                            height: h,
                            data,
                        },
                    });
                }
            }
            let out = cl_media::ffmpeg_command()
                .ok()?
                .args([
                    "-v",
                    "error",
                    "-nostdin",
                    "-filter_complex",
                    &graph,
                    "-map",
                    "[out]",
                    "-frames:v",
                    "1",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgba",
                    "-",
                ])
                .stdin(Stdio::null())
                .output()
                .ok()?;
            let need = (w * h * 4) as usize;
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
            let _ = std::fs::write(&cache, &data);
            Ok(Done::FxThumb {
                key,
                frame: RgbaFrame {
                    width: w,
                    height: h,
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
        // Hintergrund
        let (tx_bg, jobs_bg) = channel::<Job>();
        std::thread::Builder::new()
            .name("cl-decoder-bg".into())
            .spawn(move || {
                while let Ok(job) = jobs_bg.recv() {
                    if let Some(d) = run(job) {
                        if done_tx.send(d).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                }
            })
            .expect("Decoder-Thread");
        Self { tx, tx_bg, rx }
    }

    pub fn send(&self, job: Job) {
        let _ = match job {
            Job::Frame { .. } => self.tx.send(job),
            _ => self.tx_bg.send(job),
        };
    }
}

/// Laufender Wiedergabe-Strom für einen Clip.
pub struct Playback {
    pub stream: FrameStream,
    /// Zeitleisten-Position, an der der Strom begann.
    pub t0: Time,
    pub taken: u64,
    pub key: u64,
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
                _ => break,
            }
        }
        latest
    }
}

pub fn to_color_image(f: &RgbaFrame) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.data)
}

/// Beispielbild für Effekt-Kacheln (Farbverlauf mit „Sonne“ und Horizont).
pub fn sample_source(w: u32, h: u32, warm: bool) -> String {
    let (c0, c1) = if warm {
        ("0xFF9E5E", "0x3B3F9C")
    } else {
        ("0x52D1C6", "0x1E2A6B")
    };
    format!(
        "gradients=s={w}x{h}:c0={c0}:c1={c1}:x0=0:y0=0:x1={w}:y1={h}:speed=0.0001:d=2,drawbox=x={}:y={}:w={}:h={}:color=0xFFF2C0@1:t=fill,drawbox=x=0:y={}:w={w}:h={}:color=0x101225@0.85:t=fill",
        w * 6 / 10,
        h / 5,
        w / 6,
        w / 6,
        h * 7 / 10,
        h - h * 7 / 10
    )
}
