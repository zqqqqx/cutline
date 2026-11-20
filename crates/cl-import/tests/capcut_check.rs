//! Check program (manual): import CapCut drafts and check per clip with audio whether the
//! preview audio (the same path as the player: `spawn_audio_window`) is audible, the right
//! audio stream of the file plays and no audio runs twice. Output only as numbers (no names/paths).
//!
//! Drafts: all subfolders of `CUTLINE_CAPCUT_DIRS` with `draft_content.json`, otherwise the local
//! CapCut drafts. If `frequenzen.json` lies next to a draft folder (file → frequency per audio stream,
//! from the test copy scripts), the audio stream is recognized by its frequency.
//!
//! `cargo test --release -p cl-import --test capcut_check -- --ignored --nocapture`

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

use cl_core::*;

fn drafts() -> Vec<PathBuf> {
    match std::env::var_os("CUTLINE_CAPCUT_DIRS") {
        Some(d) => std::fs::read_dir(d)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("draft_content.json").is_file())
            .collect(),
        None => cl_import::capcut::list_drafts().into_iter().map(|d| d.dir).collect(),
    }
}

/// Preview audio 48 kHz stereo (only the left returned); Err = FFmpeg fails.
fn mix(p: &Project, a: Time, b: Time) -> std::result::Result<Vec<f32>, String> {
    let (mut child, _wd) = cl_export::spawn_audio_window(p, a, b, 48000).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut raw).unwrap();
    let st = child.wait().unwrap();
    if !st.success() || raw.is_empty() {
        return Err(format!("FFmpeg-Fehlstart (exit {:?})", st.code()));
    }
    Ok(raw.chunks_exact(8).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

/// Amplitude of frequency `hz` (Goertzel).
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
#[ignore]
fn capcut_drafts_audio() {
    let max_checks: usize = std::env::var("CUTLINE_CHECK_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    // audible, silent, mix fails, wrong stream, doubled, intentionally not audible
    let mut sum = [0usize; 6];
    for (k, dir) in drafts().iter().enumerate() {
        let freqs: HashMap<PathBuf, Vec<f64>> = dir
            .parent()
            .and_then(|p| std::fs::read_to_string(p.join("frequenzen.json")).ok())
            .and_then(|t| serde_json::from_str::<HashMap<String, Vec<f64>>>(&t).ok())
            .map(|m| m.into_iter().map(|(p, f)| (PathBuf::from(p), f)).collect())
            .unwrap_or_default();
        let t0 = std::time::Instant::now();
        let (p, rep) = match cl_import::capcut::import(dir) {
            Ok(x) => x,
            Err(e) => {
                println!("Entwurf {k}: Import scheitert: {e}");
                continue;
            }
        };
        let muted = p.timeline.tracks.iter().filter(|t| t.muted).count();
        println!(
            "draft {k}: import {:.1} s, {} clips, {} media, {} missing, {} tracks ({muted} muted), skipped {:?}",
            t0.elapsed().as_secs_f64(),
            rep.clips,
            p.media.len(),
            rep.missing.len(),
            p.timeline.tracks.len(),
            rep.skipped
        );
        // audible clips: (clip, media, audio stream of the file)
        let mut audible = Vec::new();
        for tr in &p.timeline.tracks {
            for c in &tr.clips {
                let ClipContent::Media { media, kind } = &c.content else { continue };
                let Some(m) = p.media(*media).filter(|m| *kind != MediaKind::Image && m.has_audio && m.is_local()) else { continue };
                if tr.muted || c.disabled || c.volume_db <= -59.9 {
                    sum[5] += 1;
                    continue;
                }
                audible.push((c, m, c.audio_stream.unwrap_or(0).min(m.audio_streams.saturating_sub(1))));
            }
        }
        // doubled: the same file and audio stream audible twice at the same source position at once
        // (split original audio in addition to the video's audio)
        let mut dup = 0;
        for (i, (a, ma, sa)) in audible.iter().enumerate() {
            if audible[i + 1..]
                .iter()
                .any(|(b, mb, sb)| ma.id == mb.id && sa == sb && a.start < b.end() && b.start < a.end() && a.source_in - a.start == b.source_in - b.start)
            {
                dup += 1;
            }
        }
        sum[4] += dup;
        let step = audible.len().div_ceil(max_checks).max(1);
        let (mut ok, mut silent, mut failed, mut wrong) = (0, 0, 0, 0);
        for (c, m, stream) in audible.iter().step_by(step) {
            let mid = c.start + Time(c.duration.0 / 2);
            let (a, b) = ((mid - Time(250_000)).max(c.start), (mid + Time(250_000)).min(c.end()));
            let mut one = p.clone();
            for t in &mut one.timeline.tracks {
                t.clips.retain(|x| x.id == c.id);
            }
            match mix(&one, a, b) {
                Err(e) => {
                    failed += 1;
                    println!("  Mix scheitert: {e}");
                }
                Ok(x) => {
                    if x.iter().fold(0f32, |m, v| m.max(v.abs())) < 1e-4 {
                        silent += 1;
                        continue;
                    }
                    ok += 1;
                    if let Some(f) = freqs.get(&m.path).filter(|f| f.len() > 1) {
                        let s = (*stream as usize).min(1);
                        let (want, other) = (amp(&x, f[s]), amp(&x, f[1 - s]));
                        if want < 4.0 * other {
                            wrong += 1;
                            println!("  wrong audio stream: expected {} ({want:.3}), other ({other:.3})", s + 1);
                        }
                    }
                }
            }
        }
        println!(
            "  checked {} of {} audible clips: audible {ok}, silent {silent}, mix fails {failed}, wrong audio stream {wrong}; audible twice {dup}",
            ok + silent + failed,
            audible.len()
        );
        // mix the whole project at samples (like the player: all clips in the window)
        let total = p.duration();
        let fails = (0..10)
            .filter(|s| {
                let at = Time(total.0 / 10 * s);
                mix(&p, at, (at + Time(1_000_000)).min(total)).is_err()
            })
            .count();
        println!("  Projekt-Mix an 10 Stellen: {fails} gescheitert");
        sum[0] += ok;
        sum[1] += silent;
        sum[2] += failed + fails;
        sum[3] += wrong;
    }
    println!(
        "total: audible {} silent {} failed {} wrong stream {} doubled {} intentionally not audible (muted/−60 dB/off) {}",
        sum[0], sum[1], sum[2], sum[3], sum[4], sum[5]
    );
}
