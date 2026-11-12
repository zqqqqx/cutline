//! Auto-Reframe: Wo ist das Wichtige im Bild? Schwerpunkt aus Bewegung (Bilddifferenz)
//! und Kanten, zeitlich geglättet. Daraus setzt die App Keyframes für die Position.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub const FPS: f64 = 5.0;

/// (Zeit relativ zu `start`, Schwerpunkt x, y) normiert 0…1.
pub fn focus_path(path: &Path, src_w: u32, src_h: u32, start: f64, dur: f64, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Vec<(f64, f32, f32)>, String> {
    let w = 160u32;
    let h = (((w as f64 * src_h as f64 / src_w.max(1) as f64).round() as u32).max(2)) & !1;
    let (wi, hi) = (w as usize, h as usize);
    let mut prev: Option<Vec<u8>> = None;
    let mut raw = Vec::new();
    let total = (dur * FPS).ceil().max(1.0);
    crate::gray_frames(path, w, h, FPS, start, dur, |i, f| {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let (mut sx, mut sy, mut sw) = (0f64, 0f64, 0f64);
        for y in 1..hi - 1 {
            for x in 1..wi - 1 {
                let p = |dx: isize, dy: isize| f[((y as isize + dy) as usize) * wi + (x as isize + dx) as usize] as f64;
                let grad = (p(1, 0) - p(-1, 0)).abs() + (p(0, 1) - p(0, -1)).abs();
                let motion = prev.as_ref().map(|pv| (f[y * wi + x] as f64 - pv[y * wi + x] as f64).abs()).unwrap_or(0.0);
                // Bewegung zählt stärker; Mitte leicht bevorzugt
                let cw = 1.0 - 0.3 * (((x as f64 / wi as f64) - 0.5).abs() * 2.0);
                let v = (grad * 0.25 + motion * 2.0).powf(1.5) * cw;
                sx += v * x as f64;
                sy += v * y as f64;
                sw += v;
            }
        }
        let (cx, cy) = if sw > 1e-6 { (sx / sw / wi as f64, sy / sw / hi as f64) } else { (0.5, 0.5) };
        raw.push((i as f64 / FPS, cx as f32, cy as f32));
        prev = Some(f.to_vec());
        progress((i as f64 / total).min(1.0) as f32);
        true
    })?;
    // Glätten (±0,6 s)
    let k = (FPS * 0.6) as usize;
    let smooth = (0..raw.len())
        .map(|i| {
            let s = &raw[i.saturating_sub(k)..(i + k + 1).min(raw.len())];
            let n = s.len() as f32;
            (raw[i].0, s.iter().map(|v| v.1).sum::<f32>() / n, s.iter().map(|v| v.2).sum::<f32>() / n)
        })
        .collect();
    Ok(smooth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_moving_subject() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("subj.mp4");
        // ruhiger Hintergrund, Objekt bewegt sich von links nach rechts
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=0x305070:s=640x360:r=25:d=4", "-f", "lavfi", "-i", "testsrc2=s=80x80:r=25:d=4"])
            .args(["-filter_complex", "[0][1]overlay=x='40+120*t':y=140", "-pix_fmt", "yuv420p"])
            .arg(&v)
            .status()
            .unwrap();
        assert!(st.success());
        let p = focus_path(&v, 640, 360, 0.0, 4.0, &|_| {}, &AtomicBool::new(false)).unwrap();
        assert!(p.len() >= 18);
        let first = p[3].1;
        let last = p[p.len() - 4].1;
        assert!(first < 0.35 && last > 0.6, "{first} → {last}");
        assert!(p.windows(2).all(|w| w[1].1 >= w[0].1 - 0.03), "monoton nach rechts");
    }
}
