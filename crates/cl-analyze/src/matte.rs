//! Hintergrund entfernen: RobustVideoMatting (rekurrent, zeitlich stabil) erzeugt aus
//! einem Video ein Graustufen-Maskenvideo (FFV1, verlustfrei, jedes Bild ein Schlüsselbild).
//! Vorschau und Export legen es per `alphamerge` als Alphakanal auf den Clip.

use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

use ort::value::Tensor;

use crate::{session, Models};

/// Verarbeitungsgröße (längste Kante)
const WORK: u32 = 640;

/// Maskenvideo für die ganze Datei `src` nach `out` schreiben.
pub fn matte(models: &Models, src: &Path, out: &Path, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<(), String> {
    let m = cl_media::probe(src).map_err(|e| e.to_string())?;
    if m.width == 0 {
        return Err("Kein Bild in dieser Datei".into());
    }
    let lim = WORK.min(m.width.max(m.height));
    let (w, h) = cl_media::fit_size(m.width, m.height, lim, lim);
    let fps = m.fps.map(|f| f.as_f64()).filter(|f| *f > 0.0).unwrap_or(30.0);
    let fps_s = m.fps.map(|f| format!("{}/{}", f.num, f.den)).unwrap_or_else(|| "30".into());
    let total = (m.duration.as_secs_f64() * fps).ceil().max(1.0);
    let mut sess = session(models, &models.rvm().ok_or("Freistell-Modell fehlt (Einstellungen → Modelle)")?)?;

    let mut dec = cl_media::ffmpeg_command()
        .map_err(|e| e.to_string())?
        .args(["-v", "error", "-nostdin"])
        .arg("-i")
        .arg(src)
        .args(["-an", "-vf", &format!("fps={fps_s},scale={w}:{h}:flags=bilinear,format=rgb24"), "-f", "rawvideo", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = out.with_extension("part.mkv");
    let mut enc = cl_media::ffmpeg_command()
        .map_err(|e| e.to_string())?
        .args(["-v", "error", "-nostdin", "-y", "-f", "rawvideo", "-pix_fmt", "gray", "-s", &format!("{w}x{h}"), "-r", &fps_s, "-i", "-", "-c:v", "ffv1", "-g", "1"])
        .arg(&tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut reader = dec.stdout.take().expect("piped");
    let mut writer = enc.stdin.take().expect("piped");

    let ratio = (512.0 / w.max(h) as f32).min(1.0);
    let (wu, hu) = (w as usize, h as usize);
    let mut rgb = vec![0u8; wu * hu * 3];
    let mut states: Vec<(Vec<usize>, Vec<f32>)> = (0..4).map(|_| (vec![1, 1, 1, 1], vec![0.0])).collect();
    let mut n = 0usize;
    let e = |e: ort::Error| e.to_string();
    let result = (|| -> Result<(), String> {
        while reader.read_exact(&mut rgb).is_ok() {
            if cancel.load(Ordering::Relaxed) {
                return Err("Abgebrochen".into());
            }
            // HWC u8 → CHW f32 0..1
            let mut chw = vec![0f32; 3 * wu * hu];
            for (i, px) in rgb.chunks_exact(3).enumerate() {
                for c in 0..3 {
                    chw[c * wu * hu + i] = px[c] as f32 / 255.0;
                }
            }
            let mut inputs = ort::inputs![
                "src" => Tensor::from_array(([1usize, 3, hu, wu], chw)).map_err(e)?,
                "downsample_ratio" => Tensor::from_array(([1usize], vec![ratio])).map_err(e)?,
            ];
            for (k, (shape, data)) in states.iter().enumerate() {
                inputs.push((format!("r{}i", k + 1).into(), Tensor::from_array((shape.clone(), data.clone())).map_err(e)?.into()));
            }
            let res = sess.run(inputs).map_err(e)?;
            let (_, pha) = res["pha"].try_extract_tensor::<f32>().map_err(e)?;
            let alpha: Vec<u8> = pha.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
            for k in 0..4 {
                let (s, d) = res[format!("r{}o", k + 1).as_str()].try_extract_tensor::<f32>().map_err(e)?;
                states[k] = (s.iter().map(|x| *x as usize).collect(), d.to_vec());
            }
            writer.write_all(&alpha).map_err(|e| e.to_string())?;
            n += 1;
            progress((n as f64 / total).min(0.99) as f32);
        }
        Ok(())
    })();
    drop(writer);
    let _ = dec.kill();
    let _ = dec.wait();
    let ok = enc.wait().map(|s| s.success()).unwrap_or(false);
    result?;
    if !ok || n == 0 {
        let _ = std::fs::remove_file(&tmp);
        return Err("Maskenvideo konnte nicht geschrieben werden".into());
    }
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    progress(1.0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn writes_matte_video_with_all_frames() {
        let Some(models) = testutil::models() else { return };
        if models.rvm().is_none() || models.ort_dll().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("v.mp4");
        // „Person“ als heller Kreis vor bewegtem Hintergrund
        assert!(cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=480x270:r=25:d=1.2", "-pix_fmt", "yuv420p"])
            .arg(&src)
            .status()
            .unwrap()
            .success());
        let out = dir.path().join("m.mkv");
        let t0 = std::time::Instant::now();
        matte(&models, &src, &out, &|_| {}, &AtomicBool::new(false)).unwrap();
        println!("RVM: 30 Bilder in {:.2} s", t0.elapsed().as_secs_f64());
        let m = cl_media::probe(&out).unwrap();
        assert!((m.duration.as_secs_f64() - 1.2).abs() < 0.1, "{}", m.duration);
        assert_eq!((m.width, m.height), (480, 270));
    }
}
