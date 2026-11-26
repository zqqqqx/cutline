//! Continuous values frame by frame for a clip starting off the frame grid: export frame `n` shows
//! keyframes, animations and effect strength at the time the preview evaluates for frame `n`
//! (frame time minus clip start), not at the layer's own time (which starts at the clip's first
//! frame, up to one frame later than the clip start). Full export, a range starting inside the
//! clip, and sections mode.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Once};

use cl_core::*;
use cl_export::{build_limited, run, ExportSettings};

const W: u32 = 320;
const H: u32 = 180;
/// Backdrop grey and the white of the image / text
const GREY: f64 = 40.0;
const WHITE: f64 = 255.0;

fn font(_: Option<&str>) -> Option<(PathBuf, u32)> {
    Some((Path::new(env!("CARGO_MANIFEST_DIR")).join("../cl-app/assets/fonts/Anton-Regular.ttf"), 0))
}

/// Flat grey video and a white image (made once per test binary run: the cases run in parallel)
fn media() -> (MediaRef, MediaRef) {
    static MAKE: Once = Once::new();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let (v, i) = (dir.join("layer-time-grey.mp4"), dir.join("layer-time-white.png"));
    MAKE.call_once(|| {
        std::fs::create_dir_all(dir).unwrap();
        for (src, out, enc) in [
            ("color=c=0x282828:s=64x36:r=30:d=4", &v, &["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"][..]),
            ("color=c=white:s=64x36:d=1", &i, &["-frames:v", "1"][..]),
        ] {
            let o = cl_media::ffmpeg_command()
                .unwrap()
                .args(["-v", "error", "-y", "-nostdin", "-f", "lavfi", "-i", src])
                .args(enc)
                .arg(out)
                .output()
                .unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        }
    });
    (cl_media::probe(&v).unwrap(), cl_media::probe(&i).unwrap())
}

fn frame_time(fps: Rational, n: i64) -> Time {
    Time::from_secs_f64(n as f64 / fps.as_f64())
}

/// Sample spacing of the export's expressions (1 s clip: 12 per second); keyframes on it are exact
fn tick(k: u32) -> Time {
    Time::from_secs_f64(k as f64 / 12.0)
}

fn linear(c: &mut Clip, p: Prop, pts: &[(u32, f32)]) {
    c.keyframes.0.insert(p, pts.iter().map(|&(k, v)| Keyframe { t: tick(k), v, ease: Ease::Linear }).collect());
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    /// Opacity keyframes with a one-sample ramp (as steep as the export can follow)
    Opacity,
    /// Position keyframes: 200 px in two samples
    Position,
    /// Text with a fade intro of one sample, then a hard end
    TextIntro,
    /// Effect element with strength keyframes
    Strength,
}

/// Four grey clips (sections mode splits there) and the clip under test from frame 2.6 for 1 s.
fn project(fps: Rational, kind: Kind) -> Project {
    let mut p = Project::new("Layer time");
    (p.settings.width, p.settings.height, p.settings.fps) = (W, H, fps);
    let (grey, white) = media();
    let mut tr = Track::new(TrackKind::Video);
    for k in 0..4 {
        tr.clips.push(Clip::new(ClipContent::Media { media: grey.id, kind: MediaKind::Video }, frame_time(fps, 9 * k), frame_time(fps, 9 * k + 9) - frame_time(fps, 9 * k)));
    }
    p.media.push(grey);
    p.timeline.tracks.push(tr);
    let c0 = Time((2.6e6 / fps.as_f64()).round() as i64);
    let len = Time::from_secs_f64(1.0);
    let image = ClipContent::Media { media: white.id, kind: MediaKind::Image };
    let (track, c) = match kind {
        Kind::Opacity => {
            let mut c = Clip::new(image, c0, len);
            linear(&mut c, Prop::Opacity, &[(0, 0.2), (3, 0.2), (4, 1.0), (8, 1.0), (11, 0.0)]);
            (TrackKind::Video, c)
        }
        Kind::Position => {
            let mut c = Clip::new(image, c0, len);
            c.transform.scale = 0.25;
            linear(&mut c, Prop::X, &[(0, -100.0), (3, -100.0), (5, 100.0)]);
            (TrackKind::Video, c)
        }
        Kind::TextIntro => {
            let st = TextStyle { text: "H".into(), size: 80.0, shadow: false, ..Default::default() };
            let mut c = Clip::new(ClipContent::Text(st), c0, len);
            c.anim.intro = Some(AnimRef { id: "fade".into(), duration: tick(1) });
            (TrackKind::Text, c)
        }
        Kind::Strength => {
            let mut c = Clip::new(ClipContent::Effect(FxRef::new("invert")), c0, len);
            linear(&mut c, Prop::Strength, &[(0, 0.0), (3, 0.0), (4, 1.0), (8, 1.0), (11, 0.0)]);
            (TrackKind::Effect, c)
        }
    };
    p.media.push(white);
    let mut t = Track::new(track);
    t.clips.push(c);
    p.timeline.tracks.push(t);
    p
}

/// The preview's value at absolute frame `n` (time of the frame minus clip start); `None`: the clip
/// is not shown there
fn expected(p: &Project, kind: Kind, n: i64) -> Option<f64> {
    let c = &p.timeline.tracks[1].clips[0];
    let t = frame_time(p.settings.fps, n);
    c.contains(t).then(|| {
        let e = c.eval_at(t - c.start, W as f32, H as f32);
        match kind {
            Kind::Opacity | Kind::TextIntro => e.opacity as f64,
            Kind::Position => W as f64 / 2.0 + e.x as f64,
            Kind::Strength => c.strength_at(t - c.start) as f64,
        }
    })
}

/// Measured value per decoded frame (`None`: no block visible): opacity from the
/// centre's brightness (image) or the brightest pixel (text), the bright block's centre column,
/// or the invert strength from the centre's brightness.
fn measured(f: &Path, kind: Kind) -> Vec<Option<f64>> {
    let o = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(f)
        .args(["-fps_mode", "passthrough", "-pix_fmt", "gray", "-f", "rawvideo", "-"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    o.stdout
        .chunks((W * H) as usize)
        .map(|img| {
            let px = |x: u32, y: u32| img[(y * W + x) as usize] as f64;
            let centre = (H / 2 - 10..H / 2 + 10).flat_map(|y| (W / 2 - 10..W / 2 + 10).map(move |x| (x, y))).map(|(x, y)| px(x, y)).sum::<f64>() / 400.0;
            let v = match kind {
                Kind::Opacity => (centre - GREY) / (WHITE - GREY),
                Kind::TextIntro => (img.iter().map(|&v| v as f64).fold(0.0, f64::max) - GREY) / (WHITE - GREY),
                Kind::Strength => (centre - GREY) / (WHITE - 2.0 * GREY),
                Kind::Position => {
                    let cols: Vec<u32> = (0..W).filter(|&x| px(x, H / 2) > 150.0).collect();
                    return (!cols.is_empty()).then(|| (cols[0] + cols[cols.len() - 1] + 1) as f64 / 2.0);
                }
            };
            Some(v)
        })
        .collect()
}

/// Exports (all, or frames `range.0..range.1`) and lists every frame whose measured value differs
/// from the preview's by more than `tol`: (absolute frame, export, preview).
fn mismatches(p: &Project, kind: Kind, tol: f64, out: &Path, max_inputs: usize, range: Option<(i64, i64)>) -> Vec<String> {
    let fps = p.settings.fps;
    let mut s = ExportSettings::from_project(p);
    s.range = range.map(|(a, b)| (frame_time(fps, a), frame_time(fps, b)));
    let job = build_limited(p, &s, &font, out, max_inputs).unwrap();
    run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
    let first = range.map_or(0, |r| r.0);
    let mut bad = Vec::new();
    for (i, got) in measured(out, kind).into_iter().enumerate() {
        let n = i as i64 + first;
        // no clip: opacity / strength 0
        let want = expected(p, kind, n).or((!matches!(kind, Kind::Position)).then_some(0.0));
        let ok = match (got, want) {
            (Some(g), Some(w)) => (g - w).abs() <= tol,
            (None, None) => true,
            _ => false,
        };
        if !ok {
            bad.push(format!("frame {n}: export {got:?}, preview {want:?}"));
        }
    }
    bad
}

fn check(kind: Kind, tol: f64) {
    if !cl_media::test_tools() {
        return;
    }
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let mut fails = Vec::new();
    for fps in [Rational::new(30, 1), Rational::new(30000, 1001)] {
        let p = project(fps, kind);
        for (mode, max, range) in [("single", 500, None), ("range", 500, Some((9, 33))), ("sections", 2, None)] {
            let case = format!("{}/{} {kind:?} {mode}", fps.num, fps.den);
            let bad = mismatches(&p, kind, tol, &dir.path().join(format!("{}-{mode}.mp4", fps.den)), max, range);
            if !bad.is_empty() {
                fails.push(format!("{case}: {bad:#?}"));
            }
        }
    }
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}

#[test]
fn opacity_keyframes_follow_preview_frames_off_grid() {
    check(Kind::Opacity, 0.05);
}

#[test]
fn position_keyframes_follow_preview_frames_off_grid() {
    check(Kind::Position, 3.0);
}

#[test]
fn text_intro_follows_preview_frames_off_grid() {
    check(Kind::TextIntro, 0.05);
}

#[test]
fn effect_strength_follows_preview_frames_off_grid() {
    check(Kind::Strength, 0.05);
}
