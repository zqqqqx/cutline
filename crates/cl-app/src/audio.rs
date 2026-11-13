//! Ton-Wiedergabe: FFmpeg mischt die Zeitleiste ab dem Playhead, cpal spielt sie ab.
//! Die abgespielten Samples sind die Uhr (Audio-Clock), nach der sich das Bild richtet.

use std::collections::VecDeque;
use std::io::Read;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use cl_core::{Project, Time};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub struct AudioPlayer {
    _stream: cpal::Stream,
    child: Child,
    _workdir: tempfile::TempDir,
    reader: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    played_frames: Arc<AtomicU64>,
    /// Spitzenpegel links/rechts seit der letzten Abfrage (f32-Bits)
    peaks: Arc<[std::sync::atomic::AtomicU32; 2]>,
    rate: u32,
    pub start: Time,
}

impl AudioPlayer {
    pub fn start(project: &Project, from: Time) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("Kein Audio-Ausgabegerät")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let rate = config.sample_rate().0;
        let channels = config.channels() as usize;

        let (mut child, workdir) = cl_export::spawn_audio_preview(project, from, rate).map_err(|e| e.to_string())?;
        let mut stdout = child.stdout.take().ok_or("kein stdout")?;
        let buffer: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::with_capacity(rate as usize * 4)));
        let stop = Arc::new(AtomicBool::new(false));
        let max_buffered = rate as usize * 2 * 2; // 2 s Stereo

        let reader = {
            let (buffer, stop) = (buffer.clone(), stop.clone());
            std::thread::Builder::new()
                .name("cl-audio-reader".into())
                .spawn(move || {
                    let mut raw = vec![0u8; 16 * 1024];
                    let mut carry: Vec<u8> = Vec::new();
                    while !stop.load(Ordering::Relaxed) {
                        while buffer.lock().unwrap().len() > max_buffered && !stop.load(Ordering::Relaxed) {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        match stdout.read(&mut raw) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                carry.extend_from_slice(&raw[..n]);
                                let whole = carry.len() / 4 * 4;
                                let mut b = buffer.lock().unwrap();
                                b.extend(carry[..whole].chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])));
                                drop(b);
                                carry.drain(..whole);
                            }
                        }
                    }
                })
                .map_err(|e| e.to_string())?
        };

        let played_frames = Arc::new(AtomicU64::new(0));
        let peaks: Arc<[std::sync::atomic::AtomicU32; 2]> = Arc::new([std::sync::atomic::AtomicU32::new(0), std::sync::atomic::AtomicU32::new(0)]);
        let stream = {
            let (buffer, played, pk) = (buffer.clone(), played_frames.clone(), peaks.clone());
            let err = |e| eprintln!("Audio: {e}");
            let cfg: cpal::StreamConfig = config.clone().into();
            macro_rules! build {
                ($t:ty, $conv:expr) => {
                    device.build_output_stream(
                        &cfg,
                        move |data: &mut [$t], _| {
                            let mut b = buffer.lock().unwrap();
                            let mut frames = 0u64;
                            let (mut pl, mut pr) = (0.0f32, 0.0f32);
                            for frame in data.chunks_mut(channels) {
                                let (l, r) = match (b.pop_front(), b.pop_front()) {
                                    (Some(l), Some(r)) => {
                                        frames += 1;
                                        (l, r)
                                    }
                                    _ => (0.0, 0.0),
                                };
                                pl = pl.max(l.abs());
                                pr = pr.max(r.abs());
                                for (i, s) in frame.iter_mut().enumerate() {
                                    let v = match i {
                                        0 => l,
                                        1 => r,
                                        _ => 0.0,
                                    };
                                    *s = $conv(v);
                                }
                            }
                            played.fetch_add(frames, Ordering::Relaxed);
                            pk[0].fetch_max(pl.to_bits(), Ordering::Relaxed);
                            pk[1].fetch_max(pr.to_bits(), Ordering::Relaxed);
                        },
                        err,
                        None,
                    )
                };
            }
            match config.sample_format() {
                cpal::SampleFormat::F32 => build!(f32, |v: f32| v),
                cpal::SampleFormat::I16 => build!(i16, |v: f32| (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16),
                cpal::SampleFormat::U16 => build!(u16, |v: f32| ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16),
                f => return Err(format!("Sample-Format {f:?} nicht unterstützt")),
            }
            .map_err(|e| e.to_string())?
        };
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self { _stream: stream, child, _workdir: workdir, reader: Some(reader), stop, played_frames, peaks, rate, start: from })
    }

    /// Aktuelle Wiedergabeposition nach Audio-Clock.
    pub fn position(&self) -> Time {
        self.start + Time::from_secs_f64(self.played_frames.load(Ordering::Relaxed) as f64 / self.rate as f64)
    }

    /// Spitzenpegel (0…1+) links/rechts seit dem letzten Aufruf
    pub fn take_peaks(&self) -> (f32, f32) {
        (f32::from_bits(self.peaks[0].swap(0, Ordering::Relaxed)), f32::from_bits(self.peaks[1].swap(0, Ordering::Relaxed)))
    }

    /// Hat schon Ton ausgegeben (vorher steht das Bild, damit Bild und Ton synchron starten).
    pub fn started(&self) -> bool {
        self.played_frames.load(Ordering::Relaxed) > 0
    }

}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}
