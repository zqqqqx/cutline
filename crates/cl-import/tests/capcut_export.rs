//! Messung (von Hand): großen CapCut-Entwurf importieren und ganz exportieren (Abschnitte).
//! Ausgabe nur Zahlen (keine Namen/Pfade). Entwurf: `CUTLINE_CAPCUT_DRAFT` (Ordner mit
//! `draft_content.json`).
//!
//! `CUTLINE_CAPCUT_DRAFT=… cargo test --release -p cl-import --test capcut_export -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use cl_core::*;
use cl_export::{build, run, ExportSettings};

fn no_fonts(_: Option<&str>) -> Option<(PathBuf, u32)> {
    None
}

/// Ein Bild (rgb24) bzw. 0,2 s Ton (f32, Stereo) einer Datei ab `t`.
fn grab(f: &Path, t: f64, video: bool) -> Vec<u8> {
    let mut c = cl_media::ffmpeg_command().unwrap();
    c.args(["-v", "error", "-nostdin", "-ss", &format!("{t:.4}"), "-i"]).arg(f);
    if video {
        c.args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]);
    } else {
        c.args(["-map", "0:a", "-t", "0.2", "-f", "f32le", "-"]);
    }
    c.output().unwrap().stdout
}

fn frames(f: &Path) -> usize {
    let o = cl_media::ffprobe_command()
        .unwrap()
        .args(["-v", "error", "-count_packets", "-select_streams", "v:0", "-show_entries", "stream=nb_read_packets", "-of", "csv=p=0"])
        .arg(f)
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).trim().trim_end_matches(',').parse().unwrap_or(0)
}

#[test]
#[ignore]
fn capcut_big_export() {
    let Some(dir) = std::env::var_os("CUTLINE_CAPCUT_DRAFT").map(PathBuf::from) else { return };
    let (p, rep) = cl_import::capcut::import(&dir).unwrap();
    println!("Import: {} Clips, {} Medien, {} fehlend", rep.clips, p.media.len(), rep.missing.len());
    // klein rechnen (Messung der Abschnitte, nicht des Encoders)
    let w: u32 = std::env::var("CUTLINE_W").ok().and_then(|v| v.parse().ok()).unwrap_or(320);
    let h = (w as u64 * p.settings.height as u64 / p.settings.width.max(1) as u64) as u32 & !1;
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("gross.mp4");
    let mut s = ExportSettings::from_project(&p);
    (s.width, s.height) = (w, h);
    s.quality = cl_export::Quality::Lower;
    let t0 = Instant::now();
    let job = build(&p, &s, &no_fonts, &out).unwrap();
    let longest = job.section_args().iter().map(|a| a.iter().map(|x| x.len() + 3).sum::<usize>()).max().unwrap_or(0);
    let most = job.section_args().iter().map(|a| a.iter().filter(|x| *x == "-i").count()).max().unwrap_or(0);
    println!("build {:.1} s: {} Abschnitte, längste Befehlszeile {longest} Zeichen, höchstens {most} Eingänge", t0.elapsed().as_secs_f64(), job.sections());
    let work = job.workdir.path().to_path_buf();
    let t0 = Instant::now();
    let (mut first, mut peak) = (None, 0u64);
    let mut last = 0.0f32;
    run(&job, Arc::new(AtomicBool::new(false)), |x| {
        if x > 0.0 && first.is_none() {
            first = Some(t0.elapsed());
        }
        if x - last > 0.02 {
            last = x;
            let used: u64 = std::fs::read_dir(&work).map(|d| d.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0);
            peak = peak.max(used);
            println!("{:5.1} s  {:3.0} %", t0.elapsed().as_secs_f64(), x * 100.0);
        }
    })
    .unwrap();
    let secs = p.duration().as_secs_f64();
    let n = frames(&out);
    let want = (secs * s.fps.as_f64()).ceil() as usize;
    println!(
        "Export {:.0} s, erster Fortschritt {:.1} s, Zwischendateien höchstens {} MB, Bilder {n} (Soll {want}), Ergebnis {} MB",
        t0.elapsed().as_secs_f64(),
        first.unwrap_or_default().as_secs_f64(),
        peak >> 20,
        std::fs::metadata(&out).unwrap().len() >> 20
    );
    assert!((n as i64 - want as i64).abs() <= 1, "Bildzahl {n} statt {want}");
    // 20 Stichproben: kurzer Bereich im Durchgang gegen den Abschnitts-Export
    let (mut vd, mut ad) = (0.0f64, 0.0f64);
    for k in 0..20 {
        let t = Time::from_secs_f64(secs * (k as f64 + 0.5) / 20.0).snap_to_frame(s.fps);
        let mut r = s.clone();
        r.range = Some((t, t + Time::from_secs_f64(0.5)));
        let o = out_dir.path().join(format!("probe{k}.mp4"));
        let j = build(&p, &r, &no_fonts, &o).unwrap();
        assert_eq!(j.sections(), 0);
        run(&j, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let (a, b) = (grab(&o, 0.1, true), grab(&out, t.as_secs_f64() + 0.1, true));
        let d = a.iter().zip(&b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len().max(1) as f64;
        vd = vd.max(d);
        let f32s = |v: Vec<u8>| v.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect::<Vec<f32>>();
        let (x, y) = (f32s(grab(&o, 0.1, false)), f32s(grab(&out, t.as_secs_f64() + 0.1, false)));
        let e = x.iter().zip(&y).map(|(a, b)| (a - b).abs() as f64).fold(0.0, f64::max);
        ad = ad.max(e);
    }
    println!("Stichproben: Bild mittlere Abweichung höchstens {vd:.2} (0–255), Ton höchstens {ad:.4}");
}
