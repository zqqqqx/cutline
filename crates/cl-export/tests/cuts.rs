//! Cuts between clips that sit edge to edge: every output frame `n` shows the clip that covers
//! the frame time `n/fps` (start <= n/fps < end, the preview's rule) – never the black canvas,
//! never the neighbour (doubled or dropped frame at the cut). Sources are flat grey, one level
//! per source, so the decoded brightness of a frame names the clip it came from.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use cl_core::*;
use cl_export::{build_limited, run, ExportSettings};

fn ffmpeg(args: &[&str], out: &Path) {
    let o = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y", "-nostdin", "-threads", "2"])
        .args(args)
        .arg(out)
        .output()
        .unwrap();
    assert!(o.status.success(), "ffmpeg {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

fn no_fonts(_: Option<&str>) -> Option<(PathBuf, u32)> {
    None
}

/// Grey level of source `i`.
fn level(i: usize) -> u8 {
    40 + 20 * i as u8
}

/// Flat grey sources (4 s) at the usual rates, one with variable frame rate and one whose
/// timestamps start at 1.4 s. Made once per test binary run.
fn sources(dir: &Path) -> Vec<MediaRef> {
    std::fs::create_dir_all(dir).unwrap();
    let rates = ["24", "25", "30000/1001", "30", "50", "60000/1001", "60"];
    let mut out = Vec::new();
    for (i, r) in rates.iter().enumerate() {
        let f = dir.join(format!("src{i}.mp4"));
        if !f.is_file() {
            let src = format!("color=c=0x{0:02x}{0:02x}{0:02x}:s=64x36:r={r}:d=4", level(i));
            ffmpeg(&["-f", "lavfi", "-i", &src, "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"], &f);
        }
        out.push(f);
    }
    let vfr = dir.join("src7_vfr.mkv");
    if !vfr.is_file() {
        let src = format!("color=c=0x{0:02x}{0:02x}{0:02x}:s=64x36:r=30:d=4", level(7));
        ffmpeg(&["-f", "lavfi", "-i", &src, "-vf", "setpts='(N/30+0.012*mod(N,3))/TB'", "-fps_mode", "passthrough", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"], &vfr);
    }
    out.push(vfr);
    let late = dir.join("src8_offset.mkv");
    if !late.is_file() {
        let src = format!("color=c=0x{0:02x}{0:02x}{0:02x}:s=64x36:r=25:d=4", level(8));
        ffmpeg(&["-f", "lavfi", "-i", &src, "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-output_ts_offset", "1.4"], &late);
    }
    out.push(late);
    out.iter().map(|f| cl_media::probe(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()))).collect()
}

fn media_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("cuts-media")
}

/// One video track with clips edge to edge at `cuts` (µs), clip `i` from source `i % n`.
fn project(fps: Rational, m: &[MediaRef], cuts: &[i64]) -> Project {
    let mut p = Project::new("Schnitte");
    p.settings.width = 64;
    p.settings.height = 36;
    p.settings.fps = fps;
    p.media.extend(m.iter().cloned());
    let mut tr = Track::new(TrackKind::Video);
    for (i, w) in cuts.windows(2).enumerate() {
        let md = &m[i % m.len()];
        let mut c = Clip::new(ClipContent::Media { media: md.id, kind: MediaKind::Video }, Time(w[0]), Time(w[1] - w[0]));
        c.source_in = Time(123_457 * (i as i64 % 4));
        tr.clips.push(c);
    }
    p.timeline.tracks.push(tr);
    p
}

/// Level the preview's rule expects on frame `n`: topmost clip with start <= frame time < end
/// (frame time in whole µs, as the playhead stands on it), or black; an "invert" effect clip
/// active by the same rule (`Clip::contains`, as the player) negates what lies below.
fn expected(p: &Project, n: i64) -> u8 {
    let t = frame_time(p, n);
    let mut lv = 0;
    for tr in &p.timeline.tracks {
        for c in tr.clips.iter().filter(|c| c.contains(t)) {
            match &c.content {
                ClipContent::Media { media, .. } => lv = level(p.media.iter().position(|m| m.id == *media).unwrap()),
                ClipContent::Effect(r) if r.id == "invert" => lv = 255 - lv,
                _ => {}
            }
        }
    }
    lv
}

/// Mean grey of every decoded frame.
fn frames(f: &Path) -> Vec<u8> {
    let o = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(f)
        .args(["-vf", "scale=8:8", "-fps_mode", "passthrough", "-pix_fmt", "gray", "-f", "rawvideo", "-"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    o.stdout.chunks(64).map(|c| (c.iter().map(|&b| b as u32).sum::<u32>() / 64) as u8).collect()
}

/// Result of one export: (black frames, frames from the wrong clip, frames, expected frames).
struct Check {
    black: Vec<i64>,
    wrong: Vec<(i64, u8, u8)>,
    count: usize,
    want: usize,
}

/// Frame `n` as a time in whole µs (where the playhead stands on it).
fn frame_time(p: &Project, n: i64) -> Time {
    Time::from_secs_f64(n as f64 / p.settings.fps.as_f64())
}

/// Exports `p` (or only frames `range.0..range.1`) and compares output frame `n` with the
/// preview's rule at absolute frame `n + range.0`.
fn export_and_check(p: &Project, out: &Path, max_inputs: usize, range: Option<(i64, i64)>) -> Check {
    let mut s = ExportSettings::from_project(p);
    s.range = range.map(|(a, b)| (frame_time(p, a), frame_time(p, b)));
    let job = build_limited(p, &s, &no_fonts, out, max_inputs).unwrap();
    run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
    let fr = frames(out);
    let first = range.map_or(0, |r| r.0);
    let want = match range {
        Some((a, b)) => (b - a) as usize,
        None => (p.duration().as_secs_f64() * p.settings.fps.as_f64()).round() as usize,
    };
    let mut c = Check { black: Vec::new(), wrong: Vec::new(), count: fr.len(), want };
    for (n, &got) in fr.iter().enumerate() {
        let exp = expected(p, n as i64 + first);
        if (got as i32 - exp as i32).abs() <= 8 {
            continue;
        }
        if got < 20 {
            c.black.push(n as i64);
        } else {
            c.wrong.push((n as i64, got, exp));
        }
    }
    c
}

fn rates() -> [Rational; 4] {
    [Rational::new(30, 1), Rational::new(60, 1), Rational::new(30000, 1001), Rational::new(60000, 1001)]
}

/// Cut times: `k`-th cut on frame `frames[k]`, shifted by `off(k, frame µs)`.
fn cuts(fps: Rational, frames: &[i64], off: impl Fn(usize, f64) -> f64) -> Vec<i64> {
    let fd = 1e6 / fps.as_f64();
    frames.iter().enumerate().map(|(k, &n)| if k == 0 { 0 } else { (n as f64 * fd + off(k, fd)).round() as i64 }).collect()
}

/// 10 clips of 5–11 frames (about 2.5 s at 30 fps).
const FRAMES: [i64; 11] = [0, 7, 12, 23, 31, 38, 47, 53, 64, 71, 79];

/// Offsets off the frame grid: a few µs to over half a frame, both directions.
fn off_grid(k: usize, fd: f64) -> f64 {
    [3.0, 0.25 * fd, 0.5 * fd - 1.0, 0.5 * fd, 0.5 * fd + 1.0, 0.75 * fd, -3.0, -0.3 * fd, 0.9 * fd, 17.0][k % 10]
}

/// Report of a case; `ok` false collects failures for one assert at the end.
fn report(name: &str, c: &Check, cuts: usize, fails: &mut Vec<String>) {
    let line = format!(
        "{name}: black {} / cuts {cuts}, wrong clip {}, frames {} (expected {}); black at {:?} wrong {:?}",
        c.black.len(),
        c.wrong.len(),
        c.count,
        c.want,
        c.black,
        c.wrong
    );
    println!("{line}");
    if !c.black.is_empty() || !c.wrong.is_empty() || c.count != c.want {
        fails.push(line);
    }
}

#[test]
fn cuts_edge_to_edge_have_no_black_frames() {
    if !cl_media::test_tools() {
        return;
    }
    let m = sources(&media_dir());
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let mut fails = Vec::new();
    for fps in rates() {
        for (case, off) in [("grid", (|_, _| 0.0) as fn(usize, f64) -> f64), ("off-grid", off_grid)] {
            let p = project(fps, &m, &cuts(fps, &FRAMES, off));
            let c = export_and_check(&p, &dir.path().join(format!("{case}-{}-{}.mp4", fps.num, fps.den)), 500, None);
            report(&format!("{}/{} {case}", fps.num, fps.den), &c, FRAMES.len() - 2, &mut fails);
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// Same off-grid cuts through sections mode (at most 2 inputs per FFmpeg).
#[test]
fn cuts_edge_to_edge_in_sections() {
    if !cl_media::test_tools() {
        return;
    }
    let m = sources(&media_dir());
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let mut fails = Vec::new();
    for fps in [Rational::new(30000, 1001), Rational::new(60, 1)] {
        let p = project(fps, &m, &cuts(fps, &FRAMES, off_grid));
        let c = export_and_check(&p, &dir.path().join(format!("sec-{}.mp4", fps.num)), 2, None);
        report(&format!("{}/{} sections", fps.num, fps.den), &c, FRAMES.len() - 2, &mut fails);
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// Cuts made by the editing operations: split at arbitrary µs, trim, move to snap onto the
/// neighbour's end; plus speed/reverse clips and a cut between two tracks.
#[test]
fn cuts_from_editing_ops() {
    if !cl_media::test_tools() {
        return;
    }
    let m = sources(&media_dir());
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let mut fails = Vec::new();
    for fps in rates() {
        let mut p = Project::new("Ops");
        p.settings.width = 64;
        p.settings.height = 36;
        p.settings.fps = fps;
        p.media.extend(m.iter().cloned());
        let mut d = Document::new(p);
        let mut ids = Vec::new();
        let mut at = Time::ZERO;
        for (i, len) in [433_337i64, 287_611, 512_003, 351_999, 299_997].iter().enumerate() {
            let mut c = Clip::new(ClipContent::Media { media: m[i * 2 % m.len()].id, kind: MediaKind::Video }, at, Time(*len));
            c.source_in = Time(200_011);
            if i == 2 {
                c.speed = 1.37;
            }
            if i == 3 {
                c.fx.reverse = true;
            }
            let OpOutput::Clip(id) = d.apply(EditOp::AddClip { track: None, clip: c }, OpSource::Ui).unwrap() else { panic!() };
            ids.push(id);
            at = at + Time(*len);
        }
        // split clip 1 at an arbitrary µs, trim clip 3's end, then snap every clip onto its left neighbour
        d.apply(EditOp::SplitClip { clip: ids[1], at: Time(433_337 + 101_234) }, OpSource::Ui).unwrap();
        let c3 = d.project().find_clip(ids[3]).unwrap().1.clone();
        d.apply(EditOp::TrimClip { clip: ids[3], edge: Edge::End, to: c3.end() - Time(54_321) }, OpSource::Ui).unwrap();
        d.apply(EditOp::TrimClip { clip: ids[0], edge: Edge::Start, to: Time(12_345) }, OpSource::Ui).unwrap();
        let order: Vec<ClipId> = d.project().timeline.tracks[0].clips.iter().map(|c| c.id).collect();
        let first = order[0];
        d.apply(EditOp::MoveClip { clip: first, track: None, start: Time::ZERO }, OpSource::Ui).unwrap();
        for w in order.windows(2) {
            let end = d.project().find_clip(w[0]).unwrap().1.end();
            d.apply(EditOp::MoveClip { clip: w[1], track: None, start: end }, OpSource::Ui).unwrap();
        }
        let mut p = d.project().clone();
        let clips = &p.timeline.tracks[0].clips;
        for w in clips.windows(2) {
            assert_eq!(w[0].end(), w[1].start, "edge to edge");
        }
        // upper track ends exactly where the next lower clip starts
        let cut = clips[1].start;
        let mut top = Track::new(TrackKind::Video);
        let mut tc = Clip::new(ClipContent::Media { media: m[7].id, kind: MediaKind::Video }, Time::ZERO, cut);
        tc.source_in = Time(77_777);
        top.clips.push(tc);
        let n_cuts = clips.len() - 1;
        p.timeline.tracks.push(top);
        let c = export_and_check(&p, &dir.path().join(format!("ops-{}.mp4", fps.num)), 500, None);
        report(&format!("{}/{} ops", fps.num, fps.den), &c, n_cuts, &mut fails);
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// Export of an In/Out range that starts on a frame whose time is not whole µs (30 fps: frames
/// 1 mod 3; 29.97 fps: e.g. 2 mod 3): cuts must stay on the same absolute frames as in the
/// preview and the full export, single pass and sections mode.
#[test]
fn cuts_in_range_export_match_preview() {
    if !cl_media::test_tools() {
        return;
    }
    let m = sources(&media_dir());
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let mut fails = Vec::new();
    let cases = [(Rational::new(30, 1), [3, 4, 5]), (Rational::new(30000, 1001), [5, 13, 6])];
    for (fps, starts) in cases {
        let p = project(fps, &m, &cuts(fps, &FRAMES, |_, _| 0.0));
        for a in starts {
            for (mode, max) in [("single", 500), ("sections", 2)] {
                let c = export_and_check(&p, &dir.path().join(format!("range-{}-{a}-{mode}.mp4", fps.num)), max, Some((a, 75)));
                let n_cuts = FRAMES.iter().filter(|&&n| n > a && n < 75).count();
                report(&format!("{}/{} range {a}..75 {mode}", fps.num, fps.den), &c, n_cuts, &mut fails);
            }
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// Effect clip edges: one grey clip under an effect track with "invert" clips (gaps and an
/// edge-to-edge pair) – every frame inverted exactly when the preview has the effect on.
/// Full export and a range starting off whole µs, single pass and sections mode.
#[test]
fn effect_edges_match_preview() {
    if !cl_media::test_tools() {
        return;
    }
    let m = sources(&media_dir());
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let mut fails = Vec::new();
    // effect clips (start frame, end frame); 23..31 and 31..38 edge to edge
    const FX: [(i64, i64); 5] = [(7, 12), (23, 31), (31, 38), (47, 53), (64, 71)];
    for (fps, range) in [(Rational::new(30, 1), (4, 75)), (Rational::new(30000, 1001), (5, 75))] {
        for (case, off) in [("grid", (|_, _| 0.0) as fn(usize, f64) -> f64), ("off-grid", off_grid)] {
            let mut p = project(fps, &m[..1], &cuts(fps, &[0, 80], |_, _| 0.0));
            let edges: Vec<i64> = FX.iter().flat_map(|&(a, b)| [a, b]).collect();
            let at = cuts(fps, &[[0].as_slice(), &edges].concat(), |k, fd| off(k, fd));
            let mut tr = Track::new(TrackKind::Effect);
            for w in at[1..].chunks(2) {
                tr.clips.push(Clip::new(ClipContent::Effect(FxRef::new("invert")), Time(w[0]), Time(w[1] - w[0])));
            }
            p.timeline.tracks.push(tr);
            for r in [None, Some(range)] {
                for (mode, max) in [("single", 500), ("sections", 2)] {
                    let name = format!("{}/{} fx {case} {r:?} {mode}", fps.num, fps.den);
                    let file = format!("fx-{}-{case}-{}-{mode}.mp4", fps.num, r.is_some());
                    let c = export_and_check(&p, &dir.path().join(file), max, r);
                    report(&name, &c, edges.len(), &mut fails);
                }
            }
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
}
