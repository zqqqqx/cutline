//! Bewegung verfolgen: Ein Bildausschnitt wird per Vorlagenvergleich (Summe der absoluten
//! Differenzen, Grob-/Feinsuche) von Bild zu Bild wiedergefunden. Die Vorlage passt sich
//! langsam an (Licht, leichte Drehung). Für Zensur-Flächen ([`track_box`]) wird zusätzlich die
//! Größe nachgeführt, in beide Richtungen verfolgt und ein verlorenes Motiv erkannt.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Bereich im Bild, normiert 0…1 (Mitte, Breite, Höhe).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxN {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
}

const WORK_W: u32 = 320;
pub const FPS: f64 = 15.0;

/// Arbeitsgröße (Breite auf `WORK_W`, gerade)
fn work_size(src_w: u32, src_h: u32) -> (u32, u32) {
    let ww = WORK_W.min(src_w.max(2)) & !1;
    let wh = (((ww as f64 * src_h as f64 / src_w.max(1) as f64).round() as u32).max(2)) & !1;
    (ww, wh)
}

#[derive(Clone)]
struct Tracker {
    w: i32,
    h: i32,
    /// Raster der Vorlage
    tw: i32,
    th: i32,
    /// Mitte und Größe des Rahmens in Arbeitspixeln
    cx: f32,
    cy: f32,
    bw: f32,
    bh: f32,
    tpl: Vec<f32>,
    search: i32,
    /// Größe nachführen und Verlust erkennen (Zensur)
    boxed: bool,
    /// mittlere Abweichung der bisherigen Treffer
    avg: f32,
}

impl Tracker {
    fn new(w: i32, h: i32, init: BoxN, boxed: bool) -> Self {
        let tw = ((init.w * w as f32) as i32).clamp(8, w / 2);
        let th = ((init.h * h as f32) as i32).clamp(8, h / 2);
        let (bw, bh) = if boxed {
            ((init.w * w as f32).clamp(8.0, w as f32), (init.h * h as f32).clamp(8.0, h as f32))
        } else {
            (tw as f32, th as f32)
        };
        Self {
            w,
            h,
            tw,
            th,
            cx: init.cx * w as f32,
            cy: init.cy * h as f32,
            bw,
            bh,
            tpl: Vec::new(),
            search: (w.max(h) / 10).max(12),
            boxed,
            avg: 0.0,
        }
    }

    /// Rasterpunkt (gx, gy) des Rahmens mit Mitte (x, y) und Größe (bw, bh)
    #[allow(clippy::too_many_arguments)]
    fn at(&self, f: &[u8], x: f32, y: f32, bw: f32, bh: f32, gx: i32, gy: i32) -> f32 {
        let px = (x - bw / 2.0 + (gx as f32 + 0.5) * bw / self.tw as f32).floor() as i32;
        let py = (y - bh / 2.0 + (gy as f32 + 0.5) * bh / self.th as f32).floor() as i32;
        f[(py.clamp(0, self.h - 1) * self.w + px.clamp(0, self.w - 1)) as usize] as f32
    }

    fn patch(&self, f: &[u8], x: f32, y: f32, bw: f32, bh: f32) -> Vec<f32> {
        (0..self.th)
            .flat_map(|gy| (0..self.tw).map(move |gx| (gx, gy)))
            .map(|(gx, gy)| self.at(f, x, y, bw, bh, gx, gy))
            .collect()
    }

    /// Mittlere absolute Abweichung zur Vorlage (jeder `step`-te Rasterpunkt)
    fn sad(&self, f: &[u8], x: f32, y: f32, bw: f32, bh: f32, step: i32) -> f32 {
        let (mut s, mut n) = (0.0, 0.0);
        let mut gy = 0;
        while gy < self.th {
            let mut gx = 0;
            while gx < self.tw {
                s += (self.at(f, x, y, bw, bh, gx, gy) - self.tpl[(gy * self.tw + gx) as usize]).abs();
                n += 1.0;
                gx += step;
            }
            gy += step;
        }
        s / n
    }

    fn step(&mut self, f: &[u8]) {
        let (bw, bh) = (self.bw, self.bh);
        if self.tpl.is_empty() {
            self.tpl = self.patch(f, self.cx, self.cy, bw, bh);
            return;
        }
        // grob (Schritt 2), dann fein um das Optimum
        let mut best = (self.cx, self.cy, f32::MAX);
        let mut dy = -self.search;
        while dy <= self.search {
            let mut dx = -self.search;
            while dx <= self.search {
                let (x, y) = (self.cx + dx as f32, self.cy + dy as f32);
                let s = self.sad(f, x, y, bw, bh, 2);
                if s < best.2 {
                    best = (x, y, s);
                }
                dx += 2;
            }
            dy += 2;
        }
        let (gx, gy) = (best.0, best.1);
        best.2 = f32::MAX;
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (x, y) = (gx + dx as f32, gy + dy as f32);
                let s = self.sad(f, x, y, bw, bh, 1);
                if s < best.2 {
                    best = (x, y, s);
                }
            }
        }
        let (mut nbw, mut nbh) = (bw, bh);
        if self.boxed {
            // viel schlechter als bisher: Motiv verloren (verdeckt, aus dem Bild) → stehen bleiben
            if self.avg > 0.0 && best.2 > (2.0 * self.avg).max(25.0) {
                return;
            }
            self.avg = if self.avg == 0.0 { best.2.max(1.0) } else { self.avg * 0.8 + best.2 * 0.2 };
            for k in [0.95f32, 1.05] {
                let (sw, sh) = ((bw * k).clamp(8.0, self.w as f32), (bh * k).clamp(8.0, self.h as f32));
                let s = self.sad(f, best.0, best.1, sw, sh, 1);
                if s < best.2 * 0.98 {
                    best.2 = s;
                    (nbw, nbh) = (sw, sh);
                }
            }
        }
        self.cx = best.0.clamp(0.0, self.w as f32);
        self.cy = best.1.clamp(0.0, self.h as f32);
        (self.bw, self.bh) = (nbw, nbh);
        let now = self.patch(f, self.cx, self.cy, nbw, nbh);
        for (t, n) in self.tpl.iter_mut().zip(now) {
            *t = *t * 0.85 + n * 0.15;
        }
    }

    fn boxn(&self) -> BoxN {
        BoxN {
            cx: self.cx / self.w as f32,
            cy: self.cy / self.h as f32,
            w: self.bw / self.w as f32,
            h: self.bh / self.h as f32,
        }
    }
}

/// Ergebnis: (Zeit relativ zu `start`, Mitte x, Mitte y) normiert.
#[allow(clippy::too_many_arguments)]
pub fn track(
    path: &Path,
    src_w: u32,
    src_h: u32,
    start: f64,
    dur: f64,
    init: BoxN,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<Vec<(f64, f32, f32)>, String> {
    let (ww, wh) = work_size(src_w, src_h);
    let mut tr = Tracker::new(ww as i32, wh as i32, init, false);
    let mut out = Vec::new();
    let total = (dur * FPS).ceil().max(1.0);
    crate::gray_frames(path, ww, wh, FPS, start, dur, |i, f| {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        tr.step(f);
        out.push((i as f64 / FPS, tr.cx / ww as f32, tr.cy / wh as f32));
        progress((i as f64 / total).min(1.0) as f32);
        true
    })?;
    Ok(out)
}

/// Rahmen samt Größe verfolgen (Zensur): `init` liegt bei Quellzeit `at`; verfolgt wird vorwärts
/// bis `start + dur` und rückwärts bis `start` (in Stücken zu 4 s, rückwärts abgearbeitet).
/// Ergebnis: (Quellzeit in s, Rahmen) nach Zeit sortiert, 15 je Sekunde. Wird der Vergleich
/// plötzlich viel schlechter (Motiv verdeckt oder aus dem Bild), bleibt der Rahmen stehen.
#[allow(clippy::too_many_arguments)]
pub fn track_box(
    path: &Path,
    src_w: u32,
    src_h: u32,
    start: f64,
    dur: f64,
    at: f64,
    init: BoxN,
    progress: &dyn Fn(f32),
    cancel: &AtomicBool,
) -> Result<Vec<(f64, BoxN)>, String> {
    let (ww, wh) = work_size(src_w, src_h);
    let at = at.clamp(start, start + dur);
    let mut tr = Tracker::new(ww as i32, wh as i32, init, true);
    let mut back: Option<Tracker> = None;
    let mut out = Vec::new();
    let total = (dur * FPS).ceil().max(1.0);
    let mut done = 0.0;
    crate::gray_frames(path, ww, wh, FPS, at, (start + dur - at).max(0.04), |i, f| {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        tr.step(f);
        if i == 0 {
            back = Some(tr.clone());
        }
        out.push((at + i as f64 / FPS, tr.boxn()));
        done += 1.0;
        progress((done / total).min(1.0) as f32);
        true
    })?;
    let Some(mut tr) = back else {
        return Ok(out);
    };
    let mut end = at;
    while end > start + 1e-3 && !cancel.load(Ordering::Relaxed) {
        let cs = (end - 4.0).max(start);
        let mut frames = Vec::new();
        crate::gray_frames(path, ww, wh, FPS, cs, end - cs, |i, f| {
            let t = cs + i as f64 / FPS;
            if t < end - 1e-6 {
                frames.push((t, f.to_vec()));
            }
            !cancel.load(Ordering::Relaxed)
        })?;
        for (t, f) in frames.iter().rev() {
            tr.step(f);
            out.push((*t, tr.boxn()));
            done += 1.0;
            progress((done / total).min(1.0) as f32);
        }
        end = cs;
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_moving_box() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("move.mp4");
        // gemustertes Kästchen wandert von x=40 nach x=240 (2 s), y fest
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=0x203040:s=320x180:r=25:d=2",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=40x40:r=25:d=2",
            ])
            .args([
                "-filter_complex",
                "[0][1]overlay=x='40+100*t':y=70",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&v)
            .status()
            .unwrap();
        assert!(st.success());
        let init = BoxN {
            cx: 60.0 / 320.0,
            cy: 90.0 / 180.0,
            w: 40.0 / 320.0,
            h: 40.0 / 180.0,
        };
        let path = track(
            &v,
            320,
            180,
            0.0,
            2.0,
            init,
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(path.len() >= 28);
        for (t, x, y) in &path {
            let ex = (60.0 + 100.0 * t) / 320.0;
            assert!((*x as f64 - ex).abs() < 0.03, "t={t} x={x} erwartet {ex}");
            assert!((*y - 0.5).abs() < 0.04, "y={y}");
        }
        // Rahmen ab der Mitte (1 s): vorwärts und rückwärts, Größe bleibt
        let mid = BoxN { cx: 160.0 / 320.0, ..init };
        let boxes = track_box(&v, 320, 180, 0.0, 2.0, 1.0, mid, &|_| {}, &AtomicBool::new(false)).unwrap();
        assert!(boxes.len() >= 28 && boxes.windows(2).all(|w| w[0].0 < w[1].0), "{}", boxes.len());
        assert!(boxes[0].0 < 0.1 && boxes.last().unwrap().0 > 1.85);
        for (t, b) in &boxes {
            let ex = (60.0 + 100.0 * t) / 320.0;
            assert!((b.cx as f64 - ex).abs() < 0.03, "t={t} x={} erwartet {ex}", b.cx);
            assert!((b.cy - 0.5).abs() < 0.04 && (b.w / init.w - 1.0).abs() < 0.2, "t={t} {b:?}");
        }
    }

    /// Wachsendes Kästchen: die Größe wächst mit; verschwindet es, bleibt der Rahmen stehen.
    #[test]
    fn box_follows_size_and_holds_when_lost() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("grow.mp4");
        // 0–2 s: wächst von 40 auf 80 px (Mitte fest bei 160/90); ab 2 s verschwunden
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=0x203040:s=320x180:r=25:d=3"])
            .args(["-f", "lavfi", "-i", "testsrc2=s=80x80:r=25:d=3"])
            .args([
                "-filter_complex",
                "[1]scale=w='trunc((40+20*t)/2)*2':h='trunc((40+20*t)/2)*2':eval=frame[o];[0][o]overlay=x='160-overlay_w/2':y='90-overlay_h/2':enable='lt(t,2)'",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&v)
            .status()
            .unwrap();
        assert!(st.success());
        let init = BoxN { cx: 0.5, cy: 0.5, w: 40.0 / 320.0, h: 40.0 / 180.0 };
        let boxes = track_box(&v, 320, 180, 0.0, 3.0, 0.0, init, &|_| {}, &AtomicBool::new(false)).unwrap();
        let at = |t: f64| boxes.iter().min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs())).unwrap().1;
        let b = at(1.9);
        assert!(b.w * 320.0 > 65.0, "Größe wächst mit: {b:?}");
        let gone = at(2.8);
        assert!((gone.cx - 0.5).abs() < 0.05 && (gone.cy - 0.5).abs() < 0.05, "bleibt stehen: {gone:?}");
        assert!(gone.w * 320.0 > 60.0, "{gone:?}");
    }
}
