//! Export test with real project files.
//!
//! Every `*.cutline` in `tests/projects/` is loaded, its media are replaced by generated
//! placeholders of the same kind, size and length, then the project is exported (scaled down)
//! and the result checked. New test project = just put a saved
//! project file into the folder.
//!
//! `CUTLINE_EXPORT_PROJECT=<file.cutline>` instead exports a project with its
//! real media at full size and prints the time (measuring the export speed).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use cl_core::{ClipContent, MediaKind, Project};
use cl_export::{build, run, ExportSettings};

fn no_fonts(_: Option<&str>) -> Option<(PathBuf, u32)> {
    None
}

fn ffmpeg(args: &[&str]) {
    let out = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y", "-nostdin"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "ffmpeg {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Replaces all media with generated placeholders in `dir`.
fn stand_in_media(p: &mut Project, dir: &Path) {
    let used_end = |id| {
        p.timeline
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .filter(|c| matches!(&c.content, ClipContent::Media { media, .. } if *media == id))
            .map(|c| c.source_in.as_secs_f64() + c.duration.as_secs_f64() * (c.speed as f64).max(1.0) * 1.5)
            .fold(0.0, f64::max)
    };
    let ends: Vec<f64> = p.media.iter().map(|m| used_end(m.id)).collect();
    for (i, (m, end)) in p.media.iter_mut().zip(ends).enumerate() {
        let secs = format!("{:.3}", m.duration.as_secs_f64().min(end + 1.0).max(0.5));
        let (w, h) = (m.width.max(16) & !1, m.height.max(16) & !1);
        let size = format!("testsrc2=size={w}x{h}:rate=30");
        let path = match m.kind {
            MediaKind::Video => {
                let f = dir.join(format!("{i}.mp4"));
                let mut a = vec!["-f", "lavfi", "-i", &size];
                if m.has_audio {
                    a.extend(["-f", "lavfi", "-i", "sine=f=330:sample_rate=48000"]);
                }
                a.extend(["-t", &secs, "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"]);
                let fs = f.to_string_lossy().into_owned();
                a.push(&fs);
                ffmpeg(&a);
                f
            }
            MediaKind::Image => {
                let f = dir.join(format!("{i}.png"));
                ffmpeg(&["-f", "lavfi", "-i", &size, "-frames:v", "1", &f.to_string_lossy()]);
                f
            }
            _ => {
                let f = dir.join(format!("{i}.wav"));
                ffmpeg(&["-f", "lavfi", "-i", "sine=f=220:sample_rate=48000", "-t", &secs, &f.to_string_lossy()]);
                f
            }
        };
        m.path = path;
    }
}

/// Grayscale image (16×16) at position `at` from a video file.
fn gray_frame(f: &Path, at: f64) -> Vec<u8> {
    let out = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-nostdin", "-ss", &format!("{at:.3}"), "-i"])
        .arg(f)
        .args(["-frames:v", "1", "-vf", "scale=16:16", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
        .output()
        .unwrap();
    out.stdout
}

fn export(p: &Project, s: &ExportSettings, out: &Path) -> f64 {
    let t = Instant::now();
    let job = build(p, s, &no_fonts, out).unwrap_or_else(|e| panic!("build {}: {e}", p.name));
    run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_or_else(|e| panic!("export {}: {e}", p.name));
    t.elapsed().as_secs_f64()
}

#[test]
fn export_test_projects() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    if let Some(f) = std::env::var_os("CUTLINE_EXPORT_PROJECT") {
        let p = Project::from_json(&std::fs::read_to_string(&f).unwrap()).unwrap();
        let s = ExportSettings::from_project(&p);
        let dir = tempfile::tempdir().unwrap();
        let secs = export(&p, &s, &dir.path().join("out.mp4"));
        println!(
            "{}: {:.1} s Video in {secs:.1} s exportiert ({:.2}× Echtzeit)",
            p.name,
            p.duration().as_secs_f64(),
            p.duration().as_secs_f64() / secs
        );
        return;
    }
    // CUTLINE_TEST_PROJECTS: another folder with test projects (e.g. for narrowing down)
    let root = std::env::var_os("CUTLINE_TEST_PROJECTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("projects"));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "cutline"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no test projects in {}", root.display());
    for f in files {
        let mut p = Project::from_json(&std::fs::read_to_string(&f).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let dir = tempfile::tempdir().unwrap();
        stand_in_media(&mut p, dir.path());
        let mut s = ExportSettings::from_project(&p);
        // scaled down: checks the filter graph, not the image quality
        s.width = (s.width / 4).max(16) & !1;
        s.height = (s.height / 4).max(16) & !1;
        let out = dir.path().join("out.mp4");
        let secs = export(&p, &s, &out);
        let m = cl_media::probe(&out).unwrap();
        let want = p.duration().as_secs_f64();
        assert!(
            (m.duration.as_secs_f64() - want).abs() < 0.2,
            "{}: length {} instead of {want:.2} s",
            f.display(),
            m.duration
        );
        assert_eq!((m.width, m.height), (s.width, s.height), "{}", f.display());
        let mid = gray_frame(&out, want / 2.0);
        assert_eq!(mid.len(), 256, "{}: no frame in the middle", f.display());
        assert!(
            mid.iter().max() != mid.iter().min(),
            "{}: frame in the middle is a single color",
            f.display()
        );
        eprintln!("{}: ok ({secs:.1} s)", f.display());
    }
}
