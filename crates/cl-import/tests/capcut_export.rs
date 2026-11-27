//! Measurement (manual): import a large CapCut draft and export it entirely (sections).
//! Output numbers only (no names/paths). Draft: `CUTLINE_CAPCUT_DRAFT` (folder with
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

/// One frame (rgb24) or 0.2 s of audio (f32, stereo) of a file from `t`.
fn grab(f: &Path, t: f64, video: bool) -> Vec<u8> {
    let mut c = cl_media::ffmpeg_command().unwrap();
    c.args(["-v", "error", "-nostdin", "-ss", &format!("{t:.4}"), "-i"])
        .arg(f);
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
        .args([
            "-v",
            "error",
            "-count_packets",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_read_packets",
            "-of",
            "csv=p=0",
        ])
        .arg(f)
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout)
        .trim()
        .trim_end_matches(',')
        .parse()
        .unwrap_or(0)
}

/// Ignored: imports and exports a large real CapCut draft (needs FFmpeg and a draft in CUTLINE_CAPCUT_DRAFT).
/// `CUTLINE_CAPCUT_DRAFT=<folder> cargo test --release -p cl-import --test capcut_export -- --ignored --nocapture`
#[test]
#[ignore]
fn capcut_big_export() {
    let dir = std::env::var_os("CUTLINE_CAPCUT_DRAFT").map(PathBuf::from).expect("set CUTLINE_CAPCUT_DRAFT=<draft folder>");
    let (p, rep) = cl_import::capcut::import(&dir).unwrap();
    println!(
        "Import: {} clips, {} media, {} missing",
        rep.clips,
        p.media.len(),
        rep.missing.len()
    );
    // compute small (measuring the sections, not the encoder)
    let w: u32 = std::env::var("CUTLINE_W")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(320);
    let h = (w as u64 * p.settings.height as u64 / p.settings.width.max(1) as u64) as u32 & !1;
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("large.mp4");
    let mut s = ExportSettings::from_project(&p);
    (s.width, s.height) = (w, h);
    s.quality = cl_export::Quality::Lower;
    let only_audio = std::env::var_os("CUTLINE_AUDIO_ONLY").is_some();
    let t0 = Instant::now();
    let job = build(&p, &s, &no_fonts, &out).unwrap();
    let longest = job
        .section_args()
        .iter()
        .map(|a| a.iter().map(|x| x.len() + 3).sum::<usize>())
        .max()
        .unwrap_or(0);
    let most = job
        .section_args()
        .iter()
        .map(|a| a.iter().filter(|x| *x == "-i").count())
        .max()
        .unwrap_or(0);
    println!("build {:.1} s: {} sections, longest command line {longest} characters, at most {most} inputs", t0.elapsed().as_secs_f64(), job.sections());
    let work = job.workdir.path().to_path_buf();
    let secs = p.duration().as_secs_f64();
    let (mut vd, mut ad) = (0.0f64, 0.0f64);
    if !only_audio {
        let t0 = Instant::now();
        let (mut first, mut peak) = (None, 0u64);
        let mut last = 0.0f32;
        run(&job, Arc::new(AtomicBool::new(false)), |x| {
            if x > 0.0 && first.is_none() {
                first = Some(t0.elapsed());
            }
            if x - last > 0.02 {
                last = x;
                let used: u64 = std::fs::read_dir(&work)
                    .map(|d| {
                        d.flatten()
                            .filter_map(|e| e.metadata().ok())
                            .map(|m| m.len())
                            .sum()
                    })
                    .unwrap_or(0);
                peak = peak.max(used);
                println!("{:5.1} s  {:3.0} %", t0.elapsed().as_secs_f64(), x * 100.0);
            }
        })
        .unwrap();
        let n = frames(&out);
        let want = (secs * s.fps.as_f64()).ceil() as usize;
        println!(
        "export {:.0} s, first progress {:.1} s, intermediate files at most {} MB, frames {n} (expected {want}), result {} MB",
        t0.elapsed().as_secs_f64(),
        first.unwrap_or_default().as_secs_f64(),
        peak >> 20,
        std::fs::metadata(&out).unwrap().len() >> 20
    );
        assert!(
            (n as i64 - want as i64).abs() <= 1,
            "frame count {n} instead of {want}"
        );
        // 20 samples: short range in the single pass against the section export
        for k in 0..20 {
            // to 50 ms (whole frames and whole samples), otherwise re-measuring alone shifts by samples
        let t = Time::from_secs_f64((secs * (k as f64 + 0.5) / 20.0 * 20.0).round() / 20.0);
            let mut r = s.clone();
            r.range = Some((t, t + Time::from_secs_f64(0.5)));
            let o = out_dir.path().join(format!("probe{k}.mp4"));
            let j = build(&p, &r, &no_fonts, &o).unwrap();
            assert_eq!(j.sections(), 0);
            run(&j, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
            let (a, b) = (grab(&o, 0.1, true), grab(&out, t.as_secs_f64() + 0.1, true));
            let d = a
                .iter()
                .zip(&b)
                .map(|(x, y)| (*x as f64 - *y as f64).abs())
                .sum::<f64>()
                / a.len().max(1) as f64;
            vd = vd.max(d);
        }
    }
    // compare audio without encoding (AAC frames would lie differently with short samples): WAV whole and per sample
    let mut wav = s.clone();
    wav.format = cl_export::Format::Wav;
    let whole = out_dir.path().join("whole.wav");
    let t0 = Instant::now();
    let j = build(&p, &wav, &no_fonts, &whole).unwrap();
    run(&j, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
    println!(
        "audio only (WAV): {} sections, {:.0} s",
        j.sections(),
        t0.elapsed().as_secs_f64()
    );
    let f32s = |v: Vec<u8>| {
        v.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect::<Vec<f32>>()
    };
    for k in 0..20 {
        // to 50 ms (whole frames and whole samples), otherwise re-measuring alone shifts by samples
        let t = Time::from_secs_f64((secs * (k as f64 + 0.5) / 20.0 * 20.0).round() / 20.0);
        // 3 s lead: the limiter there already has the same history as in the whole
        let pre = 3.0f64.min(t.as_secs_f64());
        let mut r = wav.clone();
        r.range = Some((t - Time::from_secs_f64(pre), t + Time::from_secs_f64(0.5)));
        let o = out_dir.path().join(format!("probe{k}.wav"));
        let j = build(&p, &r, &no_fonts, &o).unwrap();
        run(&j, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
        let (x, y) = (
            f32s(grab(&o, pre + 0.1, false)),
            f32s(grab(&whole, t.as_secs_f64() + 0.1, false)),
        );
        let e = x
            .iter()
            .zip(&y)
            .map(|(a, b)| (a - b).abs() as f64)
            .fold(0.0, f64::max);
        let peak = y.iter().map(|v| v.abs() as f64).fold(0.0, f64::max);
        // offset in samples at which both fit best (stereo interleaved: left only)
        let (l, r): (Vec<f32>, Vec<f32>) = (x.iter().step_by(2).copied().collect(), y.iter().step_by(2).copied().collect());
        let lag = (-2000i64..=2000)
            .min_by_key(|&d| {
                let err: f64 = (2000..l.len().saturating_sub(2000)).map(|i| (l[i] - r[(i as i64 + d) as usize]).abs() as f64).sum();
                (err * 1e6) as i64
            })
            .unwrap_or(0);
        let el: f64 = l.iter().map(|v| v.abs() as f64).sum::<f64>() / l.len().max(1) as f64;
        let er: f64 = r.iter().map(|v| v.abs() as f64).sum::<f64>() / r.len().max(1) as f64;
        println!("sample {k} at {:.3} s: deviation {e:.5}, peak {peak:.3}, offset {lag} samples, level {el:.4}/{er:.4}", t.as_secs_f64());
        ad = ad.max(e);
    }
    println!(
        "samples: picture mean deviation at most {vd:.2} (0–255), audio at most {ad:.4}"
    );
    assert!(ad < 1e-3, "audio deviates: {ad}");
}
