//! Karaoke word highlight frame by frame: export frame `n` lights exactly the words the preview
//! lights at frame `n`'s time (`TextStyle::karaoke_lit` at frame time minus clip start, the frame
//! time in whole µs as the playhead stands on it). Word starts and the clip start lie off the
//! frame grid; full export, a range starting inside the clip, and sections mode.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Once};

use cl_core::*;
use cl_export::{build_limited, run, ExportSettings};

const W: u32 = 400;
const H: u32 = 100;
const WORDS: usize = 5;

fn font(_: Option<&str>) -> Option<(PathBuf, u32)> {
    Some((Path::new(env!("CARGO_MANIFEST_DIR")).join("../cl-app/assets/fonts/Anton-Regular.ttf"), 0))
}

/// Flat dark grey video (made once per test binary run: the cases run in parallel)
fn backdrop() -> MediaRef {
    static MAKE: Once = Once::new();
    let f = Path::new(env!("CARGO_TARGET_TMPDIR")).join("karaoke-frames-grey.mp4");
    MAKE.call_once(|| {
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        let o = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-nostdin", "-f", "lavfi", "-i", "color=c=0x282828:s=64x36:r=30:d=4"])
            .args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"])
            .arg(&f)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    });
    cl_media::probe(&f).unwrap()
}

fn frame_time(fps: Rational, n: i64) -> Time {
    Time::from_secs_f64(n as f64 / fps.as_f64())
}

/// `x` frames as µs (off the grid when `x` is fractional)
fn at(fps: Rational, x: f64) -> Time {
    Time((x * 1e6 / fps.as_f64()).round() as i64)
}

/// Text clip from `start` (frames) with word starts at absolute `words` (frames; `None`: exactly on
/// that frame's µs time); four grey clips under it so sections mode splits the export.
fn project(fps: Rational, start: f64, words: &[f64], fill: bool) -> Project {
    let mut p = Project::new("Karaoke");
    (p.settings.width, p.settings.height, p.settings.fps) = (W, H, fps);
    let m = backdrop();
    let mut tr = Track::new(TrackKind::Video);
    for k in 0..4 {
        let c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, frame_time(fps, 8 * k), frame_time(fps, 8 * k + 8) - frame_time(fps, 8 * k));
        tr.clips.push(c);
    }
    p.media.push(m);
    p.timeline.tracks.push(tr);
    let c0 = at(fps, start);
    // whole frame numbers: exactly the frame's µs time (as `frame_time`)
    let abs = |x: f64| if x.fract() == 0.0 { frame_time(fps, x as i64) } else { at(fps, x) };
    let st = TextStyle {
        text: vec!["HIH"; WORDS].join(" "),
        size: 40.0,
        shadow: false,
        karaoke: Some(Karaoke { color: [255, 221, 0, 255], fill, words: words.iter().map(|&w| abs(w) - c0).collect() }),
        ..Default::default()
    };
    let mut t = Track::new(TrackKind::Text);
    t.clips.push(Clip::new(ClipContent::Text(st), c0, at(fps, 29.0) - c0));
    p.timeline.tracks.push(t);
    p
}

/// Lit words of the preview at absolute frame `n` (`None`: text not shown)
fn expected(p: &Project, n: i64) -> Option<Vec<bool>> {
    let c = &p.timeline.tracks[1].clips[0];
    let ClipContent::Text(st) = &c.content else { unreachable!() };
    let t = frame_time(p.settings.fps, n);
    c.contains(t).then(|| {
        let lit = st.karaoke_lit(c.duration, t - c.start).unwrap_or(0..0);
        (0..WORDS).map(|i| lit.contains(&i)).collect()
    })
}

/// Lit words per decoded frame: the ink extent split into equal parts (equal words), a part is
/// lit when most of its ink is yellow; `None` without ink.
fn lit_words(f: &Path) -> Vec<Option<Vec<bool>>> {
    let o = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(f)
        .args(["-fps_mode", "passthrough", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    o.stdout
        .chunks((W * H * 3) as usize)
        .map(|img| {
            let px = |x: u32, y: u32| {
                let i = ((y * W + x) * 3) as usize;
                (img[i], img[i + 1], img[i + 2])
            };
            let ink = |x: u32| (0..H).filter(|&y| { let (r, g, _) = px(x, y); r > 170 && g > 140 }).count();
            let cols: Vec<u32> = (0..W).filter(|&x| ink(x) > 0).collect();
            let (x0, x1) = (*cols.first()?, *cols.last()? + 1);
            let mut yellow = [0usize; WORDS];
            let mut white = [0usize; WORDS];
            for x in x0..x1 {
                let part = ((x - x0) as usize * WORDS / (x1 - x0) as usize).min(WORDS - 1);
                for y in 0..H {
                    match px(x, y) {
                        (r, g, b) if r > 170 && g > 140 && b < 100 => yellow[part] += 1,
                        (r, g, b) if r > 170 && g > 170 && b > 170 => white[part] += 1,
                        _ => {}
                    }
                }
            }
            Some((0..WORDS).map(|i| yellow[i] > white[i]).collect())
        })
        .collect()
}

/// Exports (all, or frames `range.0..range.1`) and lists every frame whose lit words differ from
/// the preview's: (absolute frame, export, preview).
fn mismatches(p: &Project, out: &Path, max_inputs: usize, range: Option<(i64, i64)>) -> Vec<String> {
    let fps = p.settings.fps;
    let mut s = ExportSettings::from_project(p);
    s.range = range.map(|(a, b)| (frame_time(fps, a), frame_time(fps, b)));
    let job = build_limited(p, &s, &font, out, max_inputs).unwrap();
    run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
    let first = range.map_or(0, |r| r.0);
    let mut bad = Vec::new();
    for (i, got) in lit_words(out).into_iter().enumerate() {
        let n = i as i64 + first;
        let want = expected(p, n);
        if got != want {
            bad.push(format!("frame {n}: export {got:?}, preview {want:?}"));
        }
    }
    bad
}

fn check(fps: Rational, start: f64, words: &[f64], name: &str) {
    if !cl_media::test_tools() {
        return;
    }
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let mut fails = Vec::new();
    for fill in [false, true] {
        let p = project(fps, start, words, fill);
        for (mode, max, range) in [("single", 500, None), ("range", 500, Some((5, 29))), ("sections", 2, None), ("sections range", 2, Some((5, 29)))] {
            let case = format!("{}/{} {name} fill {fill} {mode}", fps.num, fps.den);
            let bad = mismatches(&p, &dir.path().join(format!("{fill}-{}.mp4", mode.replace(' ', "-"))), max, range);
            if !bad.is_empty() {
                fails.push(format!("{case}: {bad:#?}"));
            }
        }
    }
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}

/// Word starts a quarter to most of a frame off the grid and one exactly on a frame time;
/// clip start 0.6 frame after frame 2.
const WORDS_OFF: [f64; WORDS] = [2.6, 6.3, 9.0, 12.5, 16.8];

#[test]
fn karaoke_switches_on_preview_frames_at_29_97() {
    check(Rational::new(30000, 1001), 2.6, &WORDS_OFF, "off-grid clip");
    check(Rational::new(30000, 1001), 3.0, &[3.0, 4.0, 7.0, 10.25, 13.75], "on-grid clip");
}

#[test]
fn karaoke_switches_on_preview_frames_at_23_976() {
    check(Rational::new(24000, 1001), 2.6, &WORDS_OFF, "off-grid clip");
}

#[test]
fn karaoke_switches_on_preview_frames_at_30() {
    check(Rational::new(30, 1), 2.6, &WORDS_OFF, "off-grid clip");
    check(Rational::new(30, 1), 3.0, &[3.0, 4.0, 7.0, 10.25, 13.75], "on-grid clip");
}
