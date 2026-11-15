//! Wellenformen der Bibliotheks-Sounds: einmal im Hintergrund berechnet, auf Platte gemerkt und beim
//! Start geladen – die Kacheln zeigen sie ohne Wartezeit.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use serde::{Deserialize, Serialize};

use crate::preview::{sound_bars, SOUND_BARS};

/// Wellenform und Länge eines Sounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bars {
    /// Größe und Änderungszeit der Datei: ändert sie sich, wird neu gerechnet
    stamp: u64,
    pub secs: f32,
    /// Balkenhöhen 0…255
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
    /// Speichern nacheinander: mehrere Rechner teilen sich eine Datei (und `write_atomic` eine
    /// Zwischendatei je Prozess); der Stand wird erst unter der Sperre genommen, der letzte gewinnt
    save: Mutex<()>,
}

pub struct SoundBars {
    shared: Arc<Shared>,
    /// schon eingereiht (einmal je Sitzung und Schlüssel)
    asked: HashSet<String>,
}

fn stamp(p: &Path) -> Option<u64> {
    let m = std::fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(m.len() ^ (t << 20))
}

/// Muss gerechnet werden? Nur ohne Eintrag oder wenn sich die Datei geändert hat.
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

/// Aktuellen Stand sichern (von jedem Rechner aus aufrufbar).
fn save(sh: &Shared, file: &Path) {
    let _one = sh.save.lock().unwrap_or_else(|e| e.into_inner());
    let snap = sh.map.lock().unwrap_or_else(|e| e.into_inner()).clone();
    store(file, &snap);
}

const PER_SEC: u32 = 40;
/// Vorab höchstens so viel einer Datei decodieren (stundenlange Dateien: Zeit und Speicher)
const HEAD_SECS: f64 = 600.0;

/// Wellenform der ersten `limit` Sekunden; die Länge stimmt auch darüber hinaus (ffprobe).
// shortcut: bei längeren Dateien zeigt die Kachel nur den Anfang als Wellenform, für Sound-Kacheln genügt das
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
                if let Some(j) = q.pop_front() {
                    break j;
                }
                // Warteschlange leer: Ergebnis sichern, dann schlafen
                if std::mem::take(&mut changed) {
                    drop(q);
                    save(sh, file);
                    q = sh.queue.lock().unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                q = sh.wake.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        // Datei erst prüfen, dann sperren: die Oberfläche liest dieselbe Tabelle
        let fresh = |sh: &Shared| {
            let now = stamp(&job.path);
            !stale(sh.map.lock().unwrap_or_else(|e| e.into_inner()).get(&job.key), now)
        };
        if fresh(sh) {
            continue;
        }
        // eingebaute Sounds werden erst erzeugt
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
    /// Lädt die gemerkten Werte und startet die Rechner (höchstens 4, halbe Kernzahl).
    pub fn load(file: PathBuf) -> Self {
        let shared = Arc::new(Shared { map: Mutex::new(read(&file)), ..Default::default() });
        let n = std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 4);
        for _ in 0..n {
            let (sh, file) = (shared.clone(), file.clone());
            std::thread::spawn(move || work(&sh, &file));
        }
        Self { shared, asked: HashSet::new() }
    }

    pub fn get(&self, key: &str) -> Option<Bars> {
        self.shared.map.lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned()
    }

    /// Einmal je Sitzung prüfen bzw. rechnen lassen (im Hintergrund; gemerkte Werte bleiben sichtbar).
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // nach dem Laden sofort da, ohne zu rechnen
        let sb = SoundBars::load(file.clone());
        assert_eq!(sb.get("sfx:a"), Some(b.clone()));
        assert_eq!(sb.get("sfx:b"), None);
        // unveränderte Datei: nichts zu tun; geänderte oder fehlende: neu rechnen
        assert!(!stale(Some(&b), Some(st)));
        assert!(stale(Some(&b), Some(st + 1)) && stale(None, Some(st)) && stale(Some(&b), None));
        // kaputte Datei: leer statt Fehler
        std::fs::write(&file, b"{kaputt").unwrap();
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
        // der zuletzt gesicherte Stand enthält alles
        assert_eq!(read(&file).len(), 200);
    }

    #[test]
    fn long_files_are_read_only_at_the_start_but_keep_their_length() {
        if !cl_media::ffmpeg_available() {
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
        // kurze Datei: ganz gelesen, Länge aus den Werten
        let b = bars_for(&snd, 1, 60.0).unwrap();
        assert!((b.secs - 12.0).abs() < 0.2, "{}", b.secs);
        assert!(cl_media::waveform_head(&snd, PER_SEC, Some(4.0)).unwrap().len() <= 4 * PER_SEC as usize + 1);
    }
}
