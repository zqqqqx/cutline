//! Tonspur der Datei je Clip (`Clip::audio_stream`): Vorschau-Ton nimmt die gewählte Spur, eine
//! nicht vorhandene Spur fällt auf die erste zurück, statt den Mix scheitern zu lassen.

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
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let v = dir.path().join("zwei spuren.mp4");
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
    // Wellenform der zweiten Spur lässt sich berechnen
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
    assert!(hi > 0.05 && lo < hi / 10.0, "Spur 2 = 900 Hz: {lo} {hi}");
    for s in [None, Some(0), Some(7)] {
        let (lo, hi) = play(s);
        assert!(lo > 0.05 && hi < lo / 10.0, "{s:?} → erste Spur (300 Hz): {lo} {hi}");
    }
}
