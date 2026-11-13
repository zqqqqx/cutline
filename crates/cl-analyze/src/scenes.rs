//! Szenenwechsel erkennen (FFmpeg-Szenenwert je Bild).

use std::path::Path;
use std::process::Stdio;

/// Zeiten (s, relativ zu `start`) mit Szenenwechsel. `sensitivity` 0…1 (höher = mehr Schnitte).
pub fn detect(path: &Path, start: f64, dur: f64, sensitivity: f32) -> Result<Vec<f64>, String> {
    let thr = (0.55 - 0.45 * sensitivity.clamp(0.0, 1.0)).max(0.05);
    let out = cl_media::ffmpeg_command()
        .map_err(|e| e.to_string())?
        .args([
            "-hide_banner",
            "-nostdin",
            "-ss",
            &format!("{start:.3}"),
            "-t",
            &format!("{dur:.3}"),
        ])
        .arg("-i")
        .arg(path)
        .args([
            "-an",
            "-vf",
            &format!("scale=320:-2,select='gt(scene\\,{thr:.3})',showinfo"),
            "-f",
            "null",
            "-",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let log = String::from_utf8_lossy(&out.stderr);
    let mut times: Vec<f64> = log
        .lines()
        .filter(|l| l.contains("showinfo") && l.contains("pts_time:"))
        .filter_map(|l| {
            l.split("pts_time:")
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse::<f64>()
                .ok()
        })
        .filter(|t| *t > 0.2 && *t < dur - 0.2)
        .collect();
    times.dedup_by(|a, b| (*a - *b).abs() < 0.3);
    Ok(times)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_hard_cuts() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("cuts.mp4");
        // drei Szenen: rot (0–1 s), Testbild (1–2 s), blau (2–3 s)
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=red:s=320x180:r=25:d=1",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=320x180:r=25:d=1",
                "-f",
                "lavfi",
                "-i",
                "color=blue:s=320x180:r=25:d=1",
            ])
            .args([
                "-filter_complex",
                "[0][1][2]concat=n=3:v=1[v]",
                "-map",
                "[v]",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&v)
            .status()
            .unwrap();
        assert!(st.success());
        let t = detect(&v, 0.0, 3.0, 0.5).unwrap();
        assert_eq!(t.len(), 2, "{t:?}");
        assert!(
            (t[0] - 1.0).abs() < 0.08 && (t[1] - 2.0).abs() < 0.08,
            "{t:?}"
        );
    }
}
