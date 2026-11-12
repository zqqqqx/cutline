//! Bewegung verfolgen: Ein Bildausschnitt wird per Vorlagenvergleich (Summe der absoluten
//! Differenzen, Grob-/Feinsuche) von Bild zu Bild wiedergefunden. Die Vorlage passt sich
//! langsam an (Licht, leichte Drehung).

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

/// Ergebnis: (Zeit relativ zu `start`, Mitte x, Mitte y) normiert.
pub fn track(path: &Path, src_w: u32, src_h: u32, start: f64, dur: f64, init: BoxN, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Vec<(f64, f32, f32)>, String> {
    let ww = WORK_W.min(src_w.max(2)) & !1;
    let wh = (((ww as f64 * src_h as f64 / src_w.max(1) as f64).round() as u32).max(2)) & !1;
    let (w, h) = (ww as i32, wh as i32);
    let tw = ((init.w * ww as f32) as i32).clamp(8, w / 2);
    let th = ((init.h * wh as f32) as i32).clamp(8, h / 2);
    let mut cx = init.cx * ww as f32;
    let mut cy = init.cy * wh as f32;
    let mut tpl: Vec<f32> = Vec::new();
    let mut out = Vec::new();
    let total = (dur * FPS).ceil().max(1.0);
    let search = (w.max(h) / 10).max(12);
    crate::gray_frames(path, ww, wh, FPS, start, dur, |i, f| {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let px = |x: i32, y: i32| f[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize] as f32;
        let patch = |x0: i32, y0: i32| -> Vec<f32> { (0..th).flat_map(|y| (0..tw).map(move |x| (x, y))).map(|(x, y)| px(x0 + x, y0 + y)).collect() };
        if i == 0 {
            tpl = patch(cx as i32 - tw / 2, cy as i32 - th / 2);
        } else {
            let sad = |x0: i32, y0: i32, step: i32| -> f32 {
                let mut s = 0.0;
                let mut yy = 0;
                while yy < th {
                    let mut xx = 0;
                    while xx < tw {
                        s += (px(x0 + xx, y0 + yy) - tpl[(yy * tw + xx) as usize]).abs();
                        xx += step;
                    }
                    yy += step;
                }
                s
            };
            let (bx, by) = (cx as i32 - tw / 2, cy as i32 - th / 2);
            // grob (Schritt 2), dann fein um das Optimum
            let mut best = (bx, by, f32::MAX);
            let mut dy = -search;
            while dy <= search {
                let mut dx = -search;
                while dx <= search {
                    let s = sad(bx + dx, by + dy, 2);
                    if s < best.2 {
                        best = (bx + dx, by + dy, s);
                    }
                    dx += 2;
                }
                dy += 2;
            }
            let (gx, gy) = (best.0, best.1);
            best.2 = f32::MAX;
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let s = sad(gx + dx, gy + dy, 1);
                    if s < best.2 {
                        best = (gx + dx, gy + dy, s);
                    }
                }
            }
            cx = (best.0 + tw / 2).clamp(0, w) as f32;
            cy = (best.1 + th / 2).clamp(0, h) as f32;
            let now = patch(best.0, best.1);
            for (t, n) in tpl.iter_mut().zip(now) {
                *t = *t * 0.85 + n * 0.15;
            }
        }
        out.push((i as f64 / FPS, cx / ww as f32, cy / wh as f32));
        progress((i as f64 / total).min(1.0) as f32);
        true
    })?;
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
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=0x203040:s=320x180:r=25:d=2", "-f", "lavfi", "-i", "testsrc2=s=40x40:r=25:d=2"])
            .args(["-filter_complex", "[0][1]overlay=x='40+100*t':y=70", "-pix_fmt", "yuv420p"])
            .arg(&v)
            .status()
            .unwrap();
        assert!(st.success());
        let init = BoxN { cx: 60.0 / 320.0, cy: 90.0 / 180.0, w: 40.0 / 320.0, h: 40.0 / 180.0 };
        let path = track(&v, 320, 180, 0.0, 2.0, init, &|_| {}, &AtomicBool::new(false)).unwrap();
        assert!(path.len() >= 28);
        for (t, x, y) in &path {
            let ex = (60.0 + 100.0 * t) / 320.0;
            assert!((*x as f64 - ex).abs() < 0.03, "t={t} x={x} erwartet {ex}");
            assert!((*y - 0.5).abs() < 0.04, "y={y}");
        }
    }
}
