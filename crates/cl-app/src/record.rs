//! Voiceover-Aufnahme: Mikrofon → WAV (32-bit float, Gerätesamplerate), mit Pegelanzeige.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use cl_core::Time;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Schreibt Samples in eine WAV-Datei (verlustfrei, float).
pub struct WavSink {
    writer: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
}

impl WavSink {
    pub fn create(path: &Path, rate: u32, channels: u16) -> Result<Self, String> {
        let spec = hound::WavSpec { channels, sample_rate: rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        Ok(Self { writer: hound::WavWriter::create(path, spec).map_err(|e| e.to_string())? })
    }
    pub fn push(&mut self, samples: &[f32]) -> Result<(), String> {
        for s in samples {
            self.writer.write_sample(*s).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn finish(self) -> Result<(), String> {
        self.writer.finalize().map_err(|e| e.to_string())
    }
}

pub fn input_devices() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

pub fn default_input() -> Option<String> {
    cpal::default_host().default_input_device().and_then(|d| d.name().ok())
}

pub struct Recorder {
    stream: Option<cpal::Stream>,
    tx: Option<SyncSender<Vec<f32>>>,
    writer: Option<JoinHandle<Result<(), String>>>,
    level: Arc<AtomicU32>,
    /// Spitzenpegel, 50 Werte pro Sekunde (für die Live-Waveform in der Zeitleiste)
    pub peaks: Arc<Mutex<Vec<f32>>>,
    pub path: PathBuf,
    /// Position auf der Zeitleiste, an der die Aufnahme beginnt
    pub start_at: Time,
    pub started: std::time::Instant,
}

impl Recorder {
    pub fn start(device: Option<&str>, path: PathBuf, start_at: Time, gain: f32) -> Result<Self, String> {
        let host = cpal::default_host();
        let dev = match device {
            Some(n) => host.input_devices().map_err(|e| e.to_string())?.find(|d| d.name().ok().as_deref() == Some(n)),
            None => host.default_input_device(),
        }
        .ok_or("Kein Mikrofon gefunden")?;
        let config = dev.default_input_config().map_err(|e| e.to_string())?;
        let (rate, channels) = (config.sample_rate().0, config.channels());
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut sink = WavSink::create(&path, rate, channels)?;
        let (tx, rx) = sync_channel::<Vec<f32>>(64);
        let writer = std::thread::Builder::new()
            .name("cl-voiceover-writer".into())
            .spawn(move || {
                while let Ok(buf) = rx.recv() {
                    sink.push(&buf)?;
                }
                sink.finish()
            })
            .map_err(|e| e.to_string())?;
        let level = Arc::new(AtomicU32::new(0));
        let peaks = Arc::new(Mutex::new(Vec::<f32>::new()));
        let bucket = ((rate as usize * channels as usize) / crate::preview::WAVE_PER_SEC as usize).max(1);
        let cfg: cpal::StreamConfig = config.clone().into();
        let err = |e| eprintln!("Mikrofon: {e}");
        macro_rules! build {
            ($t:ty, $conv:expr) => {{
                let (tx, level, peaks) = (tx.clone(), level.clone(), peaks.clone());
                let (mut acc_n, mut acc_max) = (0usize, 0.0f32);
                dev.build_input_stream(
                    &cfg,
                    move |data: &[$t], _| {
                        let buf: Vec<f32> = data.iter().map(|s| ($conv(*s) * gain).clamp(-1.0, 1.0)).collect();
                        let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                        level.store(peak.to_bits(), Ordering::Relaxed);
                        // Waveform-Werte im Raster der Zeitleiste sammeln
                        let mut new = Vec::new();
                        for s in &buf {
                            acc_max = acc_max.max(s.abs());
                            acc_n += 1;
                            if acc_n >= bucket {
                                new.push(acc_max);
                                acc_n = 0;
                                acc_max = 0.0;
                            }
                        }
                        if !new.is_empty() {
                            if let Ok(mut p) = peaks.lock() {
                                p.extend(new);
                            }
                        }
                        let _ = tx.try_send(buf);
                    },
                    err,
                    None,
                )
            }};
        }
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build!(f32, |s: f32| s),
            cpal::SampleFormat::I16 => build!(i16, |s: i16| s as f32 / i16::MAX as f32),
            cpal::SampleFormat::U16 => build!(u16, |s: u16| s as f32 / u16::MAX as f32 * 2.0 - 1.0),
            f => return Err(format!("Sample-Format {f:?} nicht unterstützt")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self { stream: Some(stream), tx: Some(tx), writer: Some(writer), level, peaks, path, start_at, started: std::time::Instant::now() })
    }

    /// Spitzenpegel der letzten Puffer (0..1).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Kopie der bisher gesammelten Waveform-Werte.
    pub fn peaks(&self) -> Vec<f32> {
        self.peaks.lock().map(|p| p.clone()).unwrap_or_default()
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// Stoppt und schließt die Datei sauber ab.
    pub fn stop(mut self) -> Result<PathBuf, String> {
        drop(self.stream.take()); // Callback endet, Sender im Callback wird verworfen
        drop(self.tx.take());
        if let Some(w) = self.writer.take() {
            w.join().map_err(|_| "Schreib-Thread abgestürzt".to_string())??;
        }
        Ok(self.path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_sink_writes_readable_float_wav() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("vo.wav");
        let mut s = WavSink::create(&p, 48000, 1).unwrap();
        let tone: Vec<f32> = (0..48000).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 0.5).collect();
        s.push(&tone).unwrap();
        s.finish().unwrap();
        let r = hound::WavReader::open(&p).unwrap();
        assert_eq!(r.spec().sample_rate, 48000);
        assert_eq!(r.duration(), 48000);
        if cl_media::ffmpeg_available() {
            let m = cl_media::probe(&p).unwrap();
            assert_eq!(m.kind, cl_core::MediaKind::Audio);
            assert!((m.duration.as_secs_f64() - 1.0).abs() < 0.01);
        }
    }
}
