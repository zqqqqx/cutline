//! Colour metadata of exports: every frame and the stream say BT.709, limited range (primaries,
//! transfer and matrix), whatever feeds a stretch (tagged, untagged or full-range video, a still,
//! only generated text) and however the export runs (one pass, sections, range, target size,
//! every codec). The pixels match the tags: colour patches decoded as BT.709 limited come back as
//! the RGB that went in.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use cl_core::*;
use cl_export::{build, build_limited, run, Codec, Encoder, ExportSettings, Format};

const W: u32 = 128;
const H: u32 = 72;
/// The tags every frame of a delivered video has to carry (keys sorted).
const BT709: &str = "color_primaries=bt709|color_range=tv|color_space=bt709|color_transfer=bt709";
/// RGB of the four patches (32 px wide each, left to right): unsaturated colours, so that a
/// wrong matrix or range shows instead of clipping away, and a dark grey for the range.
const PATCHES: [[u8; 3]; 4] = [[200, 60, 40], [40, 180, 90], [50, 70, 200], [40, 40, 40]];

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

/// The patches as an RGB picture (exact values, no YUV step yet).
fn patch_src() -> String {
    let mut s = format!("color=c=black:s={W}x{H}:r=30:d=2,format=rgb24");
    for (i, [r, g, b]) in PATCHES.iter().enumerate() {
        s += &format!(",drawbox=x={}:y=0:w=32:h={H}:color=0x{r:02x}{g:02x}{b:02x}:t=fill", i * 32);
    }
    s
}

/// Sources with the patches: video tagged BT.709 limited, untagged video (BT.601 limited, as
/// FFmpeg writes it), video tagged BT.709 full range, PNG still.
fn sources(dir: &Path) -> [MediaRef; 4] {
    std::fs::create_dir_all(dir).unwrap();
    let x264 = ["-c:v", "libx264", "-preset", "ultrafast", "-crf", "4"];
    let tag = |range: &str| format!("setparams=range={range}:colorspace=bt709:color_primaries=bt709:color_trc=bt709");
    let files = [dir.join("tagged.mp4"), dir.join("untagged.mp4"), dir.join("full.mp4"), dir.join("still.png")];
    let vf709 = format!("scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,{}", tag("tv"));
    let vffull = format!("scale=out_color_matrix=bt709:out_range=pc,format=yuv420p,{}", tag("pc"));
    let src = patch_src();
    // the tests run in parallel and share these files: written once, the others wait
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
    for (f, vf) in files[..3].iter().zip([vf709.as_str(), "format=yuv420p", vffull.as_str()]) {
        let mut a = vec!["-f", "lavfi", "-i", &src, "-vf", vf];
        a.extend(x264);
        ffmpeg(&a, f);
    }
    ffmpeg(&["-f", "lavfi", "-i", &src, "-frames:v", "1"], &files[3]);
    });
    files.map(|f| cl_media::probe(&f).unwrap_or_else(|e| panic!("{}: {e}", f.display())))
}

/// 0–1 s only a text (generated picture), 1–2 s still, 2–3 s untagged video, 3–4 s tagged
/// video, 4–5 s full-range video: with two inputs per section the first section has no tagged
/// input, the second only tagged ones.
fn project(m: &[MediaRef; 4]) -> Project {
    let mut p = Project::new("Farbe");
    p.settings.width = W;
    p.settings.height = H;
    p.settings.fps = Rational::new(30, 1);
    p.media.extend(m.iter().cloned());
    let s = Time::from_secs_f64;
    let mut v = Track::new(TrackKind::Video);
    for (i, at) in [(3, 1.0), (1, 2.0), (0, 3.0), (2, 4.0)] {
        let kind = if i == 3 { MediaKind::Image } else { MediaKind::Video };
        v.clips.push(Clip::new(ClipContent::Media { media: m[i].id, kind }, s(at), s(1.0)));
    }
    p.timeline.tracks.push(v);
    let mut t = Track::new(TrackKind::Text);
    t.clips.push(Clip::new(ClipContent::Text(TextStyle { text: "Text".into(), size: 20.0, ..Default::default() }), s(0.0), s(1.0)));
    p.timeline.tracks.push(t);
    p
}

fn media_dir() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("color-media")
}

fn export(p: &Project, s: &ExportSettings, out: &Path, sections: bool) {
    let job = if sections { build_limited(p, s, &no_fonts, out, 2).unwrap() } else { build(p, s, &no_fonts, out).unwrap() };
    assert_eq!(job.sections() > 1, sections, "sections mode as intended");
    run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
}

/// `key=value|…` of an ffprobe compact line without `pts_time`, keys sorted.
fn tags(line: &str) -> String {
    let mut v: Vec<&str> = line.split('|').filter(|kv| kv.starts_with("color_")).collect();
    v.sort();
    v.join("|")
}

fn probe(f: &Path, entries: &str) -> Vec<String> {
    let o = cl_media::ffprobe_command()
        .unwrap()
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", entries, "-of", "compact=p=0"])
        .arg(f)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.trim().is_empty()).map(String::from).collect()
}

/// Stream tags and the runs of equal frame tags `(from s, tags, frames)`.
fn color_tags(f: &Path) -> (String, Vec<(String, String, usize)>) {
    let stream = probe(f, "stream=color_range,color_space,color_primaries,color_transfer").first().map(|l| tags(l)).unwrap_or_default();
    let mut runs: Vec<(String, String, usize)> = Vec::new();
    for l in probe(f, "frame=pts_time,color_range,color_space,color_primaries,color_transfer") {
        let t = tags(&l);
        let at = l.split('|').find_map(|kv| kv.strip_prefix("pts_time=")).unwrap_or("?").to_string();
        match runs.last_mut() {
            Some(r) if r.1 == t => r.2 += 1,
            _ => runs.push((at, t, 1)),
        }
    }
    (stream, runs)
}

/// Tags not BT.709 limited on the stream or on any frame: a finding into `fails`.
fn check_bt709(f: &Path, what: &str, min_frames: usize, fails: &mut Vec<String>) {
    let (stream, runs) = color_tags(f);
    let frames: usize = runs.iter().map(|r| r.2).sum();
    assert!(frames >= min_frames, "{what}: {frames} frames");
    let codec = probe(f, "stream=codec_name").first().cloned().unwrap_or_default();
    // ProRes is limited range by definition and MOV `colr nclc` has no range field: FFprobe 7.1
    // reports the stream range unknown there (the frames say tv)
    let stream_ok = stream == BT709 || (codec.contains("prores") && stream == BT709.replace("color_range=tv", "color_range=unknown"));
    if !stream_ok || runs.len() != 1 || runs[0].1 != BT709 {
        fails.push(format!("{what}: colour tags must be BT.709 limited\n  stream: {stream}\n  frames (from s, tags, count): {runs:?}"));
    }
    // H.264/HEVC: also in the bitstream (VUI), not only in the container (MP4 `colr`, which
    // hides an encoder that writes none, e.g. Media Foundation)
    if !codec.contains("h264") && !codec.contains("hevc") {
        return;
    }
    // MPEG-TS carries no colour fields: what ffprobe reads there comes from the bitstream
    let raw = f.with_extension("ts");
    ffmpeg(&["-i", &f.to_string_lossy(), "-map", "0:v", "-c", "copy", "-f", "mpegts"], &raw);
    let (_, runs) = color_tags(&raw);
    if runs.len() != 1 || runs[0].1 != BT709 {
        fails.push(format!("{what}: bitstream colour tags must be BT.709 limited\n  frames (from s, tags, count): {runs:?}"));
    }
}

fn assert_none(fails: Vec<String>) {
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}

/// Largest difference per clip between the patches decoded as BT.709 limited (what the tags
/// say; players also assume it for untagged HD) and the RGB that went in: frames 45 (still),
/// 75 (untagged), 105 (tagged), 135 (full range).
fn patch_errors(f: &Path) -> Vec<(&'static str, i32)> {
    let o = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(f)
        .args(["-vf", "scale=in_color_matrix=bt709:in_range=tv,format=rgb24", "-f", "rawvideo", "-"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let size = (W * H * 3) as usize;
    let frames: Vec<&[u8]> = o.stdout.chunks_exact(size).collect();
    [("still", 45), ("untagged", 75), ("tagged BT.709", 105), ("full range", 135)]
        .into_iter()
        .map(|(name, n)| {
            let fr = frames[n];
            let err = PATCHES
                .iter()
                .enumerate()
                .flat_map(|(i, want)| {
                    let px = ((H as usize / 2) * W as usize + i * 32 + 16) * 3;
                    (0..3).map(move |c| (fr[px + c] as i32 - want[c] as i32).abs())
                })
                .max()
                .unwrap();
            (name, err)
        })
        .collect()
}

fn check_patches(f: &Path, what: &str, fails: &mut Vec<String>) {
    let e = patch_errors(f);
    // a wrong matrix or range costs 16–21 here; the rest is quantisation (the overlays composite
    // in 4:2:0, then the encoder): measured up to 9
    if e.iter().any(|x| x.1 > 10) {
        fails.push(format!("{what}: patches decoded as BT.709 limited differ from the input (max per clip): {e:?}"));
    }
}

#[test]
fn sections_and_single_pass_tag_every_frame_bt709() {
    let m = sources(&media_dir());
    let p = project(&m);
    let dir = tempfile::tempdir().unwrap();
    let mut fails = Vec::new();
    for sections in [false, true] {
        let what = if sections { "sections" } else { "single pass" };
        let out = dir.path().join(format!("{sections}.mp4"));
        export(&p, &ExportSettings::from_project(&p), &out, sections);
        check_bt709(&out, what, 150, &mut fails);
        check_patches(&out, what, &mut fails);
    }
    assert_none(fails);
}

/// Range export (what „In Teile aufteilen“ runs per part) and the target size (two passes).
#[test]
fn range_and_target_size_tag_bt709() {
    let m = sources(&media_dir());
    let p = project(&m);
    let dir = tempfile::tempdir().unwrap();
    let mut fails = Vec::new();
    for sections in [false, true] {
        let mut s = ExportSettings::from_project(&p);
        s.range = Some((Time::from_secs_f64(0.5), Time::from_secs_f64(4.5)));
        let out = dir.path().join(format!("range_{sections}.mp4"));
        export(&p, &s, &out, sections);
        check_bt709(&out, &format!("range, sections {sections}"), 120, &mut fails);
        let mut s = ExportSettings::from_project(&p);
        s.target_mb = Some(1);
        let out = dir.path().join(format!("target_{sections}.mp4"));
        export(&p, &s, &out, sections);
        check_bt709(&out, &format!("target size, sections {sections}"), 150, &mut fails);
    }
    assert_none(fails);
}

/// Every software codec and the alpha variants, one pass and sections.
#[test]
fn every_codec_tags_bt709() {
    let m = sources(&media_dir());
    let p = project(&m);
    let dir = tempfile::tempdir().unwrap();
    let mut fails = Vec::new();
    let cases = [
        (Format::Mp4, Codec::Hevc, false),
        (Format::Mp4, Codec::Av1, false),
        (Format::Mkv, Codec::Vp9, false),
        (Format::WebM, Codec::Vp9, false),
        (Format::WebM, Codec::Vp9, true),
        (Format::Mov, Codec::ProRes, false),
        (Format::Mov, Codec::ProRes, true),
    ];
    for (format, codec, transparent) in cases {
        for sections in [false, true] {
            let s = ExportSettings { format, codec, transparent, include_audio: false, ..ExportSettings::from_project(&p) };
            let what = format!("{format:?} {codec:?} alpha {transparent} sections {sections}");
            let out = dir.path().join(format!("{format:?}_{codec:?}_{transparent}_{sections}.{}", format.extension()));
            export(&p, &s, &out, sections);
            check_bt709(&out, &what, 150, &mut fails);
            check_patches(&out, &what, &mut fails);
        }
    }
    assert_none(fails);
}

/// Hardware encoders present on this computer (NVENC, Media Foundation …), sections mode; none
/// here: nothing to check (the arguments are covered by the unit test `color_args_everywhere`).
#[test]
fn hardware_encoders_tag_bt709() {
    let m = sources(&media_dir());
    let p = project(&m);
    let dir = tempfile::tempdir().unwrap();
    let mut fails = Vec::new();
    for &enc in cl_export::available_encoders().iter().filter(|e| **e != Encoder::Software) {
        for codec in [Codec::H264, Codec::Hevc] {
            let s = ExportSettings { encoder: enc, codec, include_audio: false, width: 256, height: 144, ..ExportSettings::from_project(&p) };
            let out = dir.path().join(format!("{enc:?}_{codec:?}.mp4"));
            let job = build_limited(&p, &s, &no_fonts, &out, 2).unwrap();
            if let Err(e) = run(&job, Arc::new(AtomicBool::new(false)), |_| {}) {
                // e.g. HEVC not offered by this GPU's encoder
                eprintln!("{enc:?} {codec:?} not usable here: {e}");
                continue;
            }
            check_bt709(&out, &format!("{enc:?} {codec:?}"), 150, &mut fails);
        }
    }
    assert_none(fails);
}

/// QSV, AMF and AV1 via NVENC join lossless FFV1 sections (NUT, no colour fields) and encode
/// once: the final run tags the frames again before the encoder. Those encoders cannot start on
/// the test machine: the QSV job runs with libx264 swapped in for the final encode.
#[test]
fn lossless_sections_retag_before_the_encoder() {
    let m = sources(&media_dir());
    let p = project(&m);
    let dir = tempfile::tempdir().unwrap();
    let mut fails = Vec::new();
    for (encoder, codec) in [(Encoder::Qsv, Codec::H264), (Encoder::Amf, Codec::Hevc), (Encoder::Nvenc, Codec::Av1)] {
        let s = ExportSettings { encoder, codec, ..ExportSettings::from_project(&p) };
        let job = build_limited(&p, &s, &no_fonts, &dir.path().join("l.mp4"), 2).unwrap();
        assert!(job.sections() > 1);
        let pictures: Vec<_> = job.section_args().into_iter().filter(|a| a.iter().any(|x| x == "-c:v")).collect();
        assert!(!pictures.is_empty() && pictures.iter().all(|a| a.windows(2).any(|w| w == ["-c:v", "ffv1"])), "{encoder:?}: lossless sections");
        let g = std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap();
        assert!(g.contains("[0:v]setparams=range=tv:colorspace=bt709:color_primaries=bt709:color_trc=bt709[vout]"), "{encoder:?}: {g}");
        assert!(job.args.windows(2).any(|w| w == ["-map", "[vout]"]), "{encoder:?}: {:?}", job.args);
        if encoder == Encoder::Qsv {
            let mut job = job;
            for a in &mut job.args {
                match a.as_str() {
                    "h264_qsv" => *a = "libx264".into(),
                    "-global_quality" => *a = "-crf".into(),
                    _ => {}
                }
            }
            run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
            check_bt709(&job.out, "lossless sections", 150, &mut fails);
            check_patches(&job.out, "lossless sections", &mut fails);
        }
    }
    assert_none(fails);
}
