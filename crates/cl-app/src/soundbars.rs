//! Waveforms of the library sounds: computed once in the background, remembered on disk and
//! loaded at startup – the tiles show them without waiting.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use serde::{Deserialize, Serialize};

use crate::preview::{sound_bars, SOUND_BARS};

/// Waveform and length of a sound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bars {
    /// Size and modification time of the file: if they change, it is recomputed
    stamp: u64,
    pub secs: f32,
    /// Bar heights 0…255
    pub bars: Vec<u8>,
}

struct Job {
    key: String,
    path: PathBuf,
    sfx: Option<&'static str>,
}

#[derive(Default)]
struct Shared {
    map: Mutex<HashMap<String, Bars>>,
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
    /// The owner is gone: workers finish their job and end
    stop: std::sync::atomic::AtomicBool,
    /// Save one after another: several workers share one file (and `write_atomic` one
    /// temp file per process); the state is only taken under the lock, the last one wins
    save: Mutex<()>,
}

pub struct SoundBars {
    shared: Arc<Shared>,
    workers: Vec<std::thread::JoinHandle<()>>,
    /// already queued (once per session and key)
    asked: HashSet<String>,
}

fn stamp(p: &Path) -> Option<u64> {
    let m = std::fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(m.len() ^ (t << 20))
}

/// Needs computing? Only without an entry or if the file has changed.
fn stale(cached: Option<&Bars>, now: Option<u64>) -> bool {
    match (cached, now) {
        (Some(b), Some(s)) => b.stamp != s,
        _ => true,
    }
}

fn quantize(peaks: &[f32], per_sec: u32, stamp: u64) -> Bars {
    Bars {
        stamp,
        secs: peaks.len() as f32 / per_sec.max(1) as f32,
        bars: sound_bars(peaks, SOUND_BARS).iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect(),
    }
}

fn read(file: &Path) -> HashMap<String, Bars> {
    std::fs::read(file).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
}

fn store(file: &Path, map: &HashMap<String, Bars>) {
    if let Ok(d) = serde_json::to_vec(map) {
        let _ = crate::paths::write_atomic(file, d);
    }
}

/// Save the current state (callable from any worker).
fn save(sh: &Shared, file: &Path) {
    let _one = sh.save.lock().unwrap_or_else(|e| e.into_inner());
    let snap = sh.map.lock().unwrap_or_else(|e| e.into_inner()).clone();
    store(file, &snap);
}

const PER_SEC: u32 = 40;
/// Decode at most this much of a file up front (hours-long files: time and memory)
const HEAD_SECS: f64 = 600.0;

/// Waveform of the first `limit` seconds; the length is right beyond that too (ffprobe).
// shortcut: for longer files the tile only shows the start as a waveform, which is enough for sound tiles
fn bars_for(path: &Path, st: u64, limit: f64) -> Option<Bars> {
    let peaks = cl_media::waveform_head(path, PER_SEC, Some(limit)).ok()?;
    let mut b = quantize(&peaks, PER_SEC, st);
    if peaks.len() as f64 >= limit * PER_SEC as f64 - 1.0 {
        if let Ok(m) = cl_media::probe(path) {
            b.secs = m.duration.as_secs_f64() as f32;
        }
    }
    Some(b)
}

fn work(sh: &Shared, file: &Path) {
    let mut changed = false;
    loop {
        let job = {
            let mut q = sh.queue.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if sh.stop.load(std::sync::atomic::Ordering::Relaxed) {
                    drop(q);
                    if changed {
                        save(sh, file);
                    }
                    return;
                }
                if let Some(j) = q.pop_front() {
                    break j;
                }
                // Queue empty: save the result, then sleep
                if std::mem::take(&mut changed) {
                    drop(q);
                    save(sh, file);
                    q = sh.queue.lock().unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                q = sh.wake.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        // Check the file first, then lock: the UI reads the same table
        let fresh = |sh: &Shared| {
            let now = stamp(&job.path);
            !stale(sh.map.lock().unwrap_or_else(|e| e.into_inner()).get(&job.key), now)
        };
        if fresh(sh) {
            continue;
        }
        // built-in sounds are created first
        if let Some(s) = job.sfx.and_then(cl_fx::sfx::sfx) {
            if cl_fx::sfx::render(s, &job.path).is_err() {
                continue;
            }
            if fresh(sh) {
                continue;
            }
        }
        let Some(b) = stamp(&job.path).and_then(|st| bars_for(&job.path, st, HEAD_SECS)) else {
            continue;
        };
        sh.map.lock().unwrap_or_else(|e| e.into_inner()).insert(job.key, b);
        changed = true;
    }
}

impl SoundBars {
    /// Loads the remembered values and starts the workers (at most 4, half the core count).
    pub fn load(file: PathBuf) -> Self {
        let shared = Arc::new(Shared { map: Mutex::new(read(&file)), ..Default::default() });
        let n = std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 4);
        let workers = (0..n)
            .map(|_| {
                let (sh, file) = (shared.clone(), file.clone());
                std::thread::spawn(move || work(&sh, &file))
            })
            .collect();
        Self { shared, workers, asked: HashSet::new() }
    }

    pub fn get(&self, key: &str) -> Option<Bars> {
        self.shared.map.lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned()
    }

    /// Check or compute once per session (in the background; remembered values stay visible).
    pub fn ensure(&mut self, key: &str, path: &Path, sfx: Option<&'static str>) {
        if self.asked.contains(key) {
            return;
        }
        self.asked.insert(key.to_string());
        let job = Job { key: key.to_string(), path: path.to_path_buf(), sfx };
        self.shared.queue.lock().unwrap_or_else(|e| e.into_inner()).push_back(job);
        self.shared.wake.notify_one();
    }
}

/// The workers end with their owner. Bug: they went on rendering the built-in sounds into the data
/// folder after the app was gone; in test runs that folder was a deleted temp folder, which they
/// created again (thousands of left-over folders, about 100 GB).
impl Drop for SoundBars {
    fn drop(&mut self) {
        self.shared.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.shared.queue.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.shared.wake.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_ends_the_workers_and_nothing_is_written_afterwards() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().to_path_buf();
        let mut sb = SoundBars::load(dir.join("sound-bars.json"));
        for s in cl_fx::sfx::SFX.iter() {
            sb.ensure(&format!("builtin:sfx:{}", s.id), &dir.join("Sfx").join(format!("{}.wav", s.id)), Some(s.id));
        }
        // the owner goes while most sounds are still waiting
        drop(sb);
        drop(d);
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert!(!dir.exists(), "a worker wrote into the folder after its owner was gone");
    }

    #[test]
    fn stored_bars_are_there_at_once_and_only_changed_files_are_redone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bars.json");
        let snd = dir.path().join("a.wav");
        std::fs::write(&snd, b"RIFF").unwrap();
        let st = stamp(&snd).unwrap();
        let b = quantize(&[0.0, 0.5, 1.0, 2.0], 2, st);
        assert_eq!((b.bars.len(), b.secs), (SOUND_BARS, 2.0));
        assert!(b.bars.contains(&255));
        store(&file, &HashMap::from([("sfx:a".to_string(), b.clone())]));
        // available right after loading, without computing
        let sb = SoundBars::load(file.clone());
        assert_eq!(sb.get("sfx:a"), Some(b.clone()));
        assert_eq!(sb.get("sfx:b"), None);
        // unchanged file: nothing to do; changed or missing: recompute
        assert!(!stale(Some(&b), Some(st)));
        assert!(stale(Some(&b), Some(st + 1)) && stale(None, Some(st)) && stale(Some(&b), None));
        // broken file: empty instead of an error
        std::fs::write(&file, b"{broken").unwrap();
        assert!(read(&file).is_empty());
    }

    #[test]
    fn parallel_savers_leave_the_newest_complete_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bars.json");
        let sh = Shared::default();
        let b = quantize(&[0.5; 8], 4, 1);
        std::thread::scope(|s| {
            for t in 0..8 {
                let (sh, file, b) = (&sh, &file, &b);
                s.spawn(move || {
                    for i in 0..25 {
                        sh.map.lock().unwrap().insert(format!("{t}-{i}"), b.clone());
                        save(sh, file);
                    }
                });
            }
        });
        // the last saved state contains everything
        assert_eq!(read(&file).len(), 200);
    }

    #[test]
    fn long_files_are_read_only_at_the_start_but_keep_their_length() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let snd = dir.path().join("lang.wav");
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=12", "-ac", "1"])
            .arg(&snd)
            .status()
            .unwrap();
        assert!(st.success());
        let b = bars_for(&snd, 1, 4.0).unwrap();
        assert!((b.secs - 12.0).abs() < 0.2, "{}", b.secs);
        // short file: read fully, length from the values
        let b = bars_for(&snd, 1, 60.0).unwrap();
        assert!((b.secs - 12.0).abs() < 0.2, "{}", b.secs);
        assert!(cl_media::waveform_head(&snd, PER_SEC, Some(4.0)).unwrap().len() <= 4 * PER_SEC as usize + 1);
    }
}
