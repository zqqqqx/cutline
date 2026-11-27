//! Audio stream of the file per clip (`Clip::audio_stream`): preview audio takes the chosen stream, a
//! missing stream falls back to the first instead of failing the mix.

use std::io::Read;

use cl_core::*;

fn amp(x: &[f32], hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / 48000.0;
    let (mut s1, mut s2) = (0.0, 0.0);
    for &v in x {
        let s = v as f64 + 2.0 * w.cos() * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2).max(0.0).sqrt() * 2.0 / x.len().max(1) as f64
}

#[test]
fn clip_plays_chosen_audio_stream() {
    if !cl_media::test_tools() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let v = dir.path().join("two tracks.mp4");
    let ok = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=160x90:r=30:d=3", "-f", "lavfi", "-i", "sine=f=300:d=3"])
        .args(["-f", "lavfi", "-i", "sine=f=900:d=3", "-map", "0", "-map", "1", "-map", "2", "-c:a", "aac", "-pix_fmt", "yuv420p", "-shortest"])
        .arg(&v)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let m = cl_media::probe(&v).unwrap();
    assert_eq!(m.audio_streams, 2);
    // the waveform of the second stream can be computed
    assert!(!cl_media::waveform_stream(&v, 50, 1).unwrap().is_empty());
    let play = |stream: Option<u32>| {
        let mut p = Project::new("t");
        let mut t = Track::new(TrackKind::Audio);
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Audio }, Time::ZERO, Time(2_000_000));
        c.audio_stream = stream;
        t.clips.push(c);
        p.timeline.tracks.push(t);
        p.media.push(m.clone());
        let (mut child, _wd) = cl_export::spawn_audio_window(&p, Time(500_000), Time(1_500_000), 48000).unwrap();
        let mut raw = Vec::new();
        child.stdout.take().unwrap().read_to_end(&mut raw).unwrap();
        assert!(child.wait().unwrap().success());
        let x: Vec<f32> = raw.chunks_exact(8).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        (amp(&x, 300.0), amp(&x, 900.0))
    };
    let (lo, hi) = play(Some(1));
    assert!(hi > 0.05 && lo < hi / 10.0, "track 2 = 900 Hz: {lo} {hi}");
    for s in [None, Some(0), Some(7)] {
        let (lo, hi) = play(s);
        assert!(lo > 0.05 && hi < lo / 10.0, "{s:?} → first track (300 Hz): {lo} {hi}");
    }
}

/// „Extract audio“ twice on a file with two streams (300 Hz / 900 Hz): the WAV export holds each
/// stream once, the stream extracted from the video is not doubled and the video's other stream
/// stays audible.
#[test]
fn extracted_streams_export_once_each() {
    if !cl_media::test_tools() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let v = dir.path().join("two tracks.mp4");
    let ok = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=160x90:r=30:d=3", "-f", "lavfi", "-i", "sine=f=300:d=3"])
        .args(["-f", "lavfi", "-i", "sine=f=900:d=3", "-map", "0", "-map", "1", "-map", "2", "-c:a", "aac", "-pix_fmt", "yuv420p", "-shortest"])
        .arg(&v)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let m = cl_media::probe(&v).unwrap();
    let dur = Time(2_000_000);
    let video = |stream: Option<u32>, taken: &[u32]| {
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, dur);
        c.audio_stream = stream;
        c.extracted_streams = taken.to_vec();
        c
    };
    let audio = |stream: Option<u32>| {
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Audio }, Time::ZERO, dur);
        c.audio_stream = stream;
        c
    };
    // (300 Hz, 900 Hz) amplitude of the exported sound, 0.5 … 1.5 s
    let export = |name: &str, v: Clip, a: Vec<Clip>| {
        let mut p = Project::new("t");
        let mut vt = Track::new(TrackKind::Video);
        vt.main = true;
        vt.clips.push(v);
        let mut at = Track::new(TrackKind::Audio);
        at.clips = a;
        p.timeline.tracks = vec![vt, at];
        p.media.push(m.clone());
        let out = dir.path().join(format!("{name}.wav"));
        let mut s = cl_export::ExportSettings::from_project(&p);
        s.format = cl_export::Format::Wav;
        let job = cl_export::build(&p, &s, &|_| None, &out).unwrap();
        cl_export::run(&job, std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), |_| {}).unwrap();
        let o = cl_media::ffmpeg_command().unwrap().args(["-v", "error", "-ss", "0.5", "-t", "1", "-i"]).arg(&out).args(["-ac", "1", "-ar", "48000", "-f", "f32le", "-"]).output().unwrap();
        assert!(o.status.success());
        let x: Vec<f32> = o.stdout.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        (amp(&x, 300.0), amp(&x, 900.0))
    };
    let (lo, hi) = export("before", video(None, &[]), vec![]);
    assert!(lo > 0.02 && hi < lo / 10.0, "original: stream 1 only: {lo} {hi}");
    let near = |a: f64| (a / lo - 1.0).abs() < 0.2;
    // stream 1 extracted: once, not doubled by the video
    let (l1, h1) = export("one", video(None, &[0]), vec![audio(None)]);
    assert!(near(l1) && h1 < lo / 10.0, "stream 1 once: {l1} {h1} (original {lo})");
    // video switched to stream 2: audible next to the extracted stream 1
    let (l2, h2) = export("umgestellt", video(Some(1), &[0]), vec![audio(None)]);
    assert!(near(l2) && near(h2), "streams 1 and 2: {l2} {h2}");
    // stream 2 extracted as well: both once each, the video is silent
    let (l3, h3) = export("beide", video(Some(1), &[0, 1]), vec![audio(None), audio(Some(1))]);
    assert!(near(l3) && near(h3), "both extracted once: {l3} {h3}");
}
