//! Audio analyses for editing tools: loudness curve, pauses (jump cuts, auto ducking),
//! loud moments (highlights) and offset of two recordings of the same scene (sync).

use std::path::Path;

/// Values per second in the loudness curve
pub const ENV_RATE: u32 = 100;

/// RMS curve (linear, 0…1) with 100 values per second.
pub fn envelope(path: &Path, start: f64, dur: Option<f64>) -> Result<Vec<f32>, String> {
    let sr = 16_000u32;
    let x = crate::decode_audio(path, sr, start, dur)?;
    Ok(envelope_samples(&x, sr))
}

pub fn envelope_samples(x: &[f32], sr: u32) -> Vec<f32> {
    let hop = (sr / ENV_RATE) as usize;
    x.chunks(hop)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len().max(1) as f32).sqrt())
        .collect()
}

pub fn db(v: f32) -> f32 {
    20.0 * v.max(1e-6).log10()
}

/// Quiet sections (seconds relative to the curve), longer than `min_pause`.
/// `thresh_db` relative to the loudest value (e.g. −35 dB).
pub fn silences(env: &[f32], thresh_db: f32, min_pause: f64) -> Vec<(f64, f64)> {
    let peak = env.iter().copied().fold(0.0f32, f32::max).max(1e-6);
    let limit = peak * 10f32.powf(thresh_db / 20.0);
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, v) in env
        .iter()
        .enumerate()
        .chain(std::iter::once((env.len(), &f32::MAX)))
    {
        let quiet = *v < limit;
        match (quiet, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                let (a, b) = (s as f64 / ENV_RATE as f64, i as f64 / ENV_RATE as f64);
                if b - a >= min_pause {
                    out.push((a, b));
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// The `n` loudest moments (smoothed over 0.5 s), at least `min_gap` seconds apart.
pub fn loud_moments(env: &[f32], n: usize, min_gap: f64) -> Vec<f64> {
    // smoothed centered (±0.25 s) so the point in time lies in the middle of the event
    let half = (ENV_RATE / 4) as usize;
    let mut pre = vec![0.0f32; env.len() + 1];
    for (i, v) in env.iter().enumerate() {
        pre[i + 1] = pre[i] + v;
    }
    let smooth: Vec<f32> = (0..env.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(env.len()));
            (pre[b] - pre[a]) / (b - a) as f32
        })
        .collect();
    // loud relative to the surroundings (5 s): screams, explosions, cheers
    let ctx = (ENV_RATE * 5) as usize;
    let mut scored: Vec<(f32, usize)> = (0..smooth.len())
        .map(|i| {
            let a = i.saturating_sub(ctx);
            let b = (i + ctx).min(smooth.len());
            let mean = smooth[a..b].iter().sum::<f32>() / (b - a).max(1) as f32;
            (smooth[i] / mean.max(1e-6), i)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut picks: Vec<f64> = Vec::new();
    for (score, i) in scored {
        if picks.len() >= n || score < 1.6 {
            break;
        }
        let t = i as f64 / ENV_RATE as f64;
        if picks.iter().all(|p| (p - t).abs() >= min_gap) {
            picks.push(t);
        }
    }
    picks.sort_by(|a, b| a.partial_cmp(b).unwrap());
    picks
}

/// Offset of `b` relative to `a` in seconds (positive: the event comes later in `b`).
/// Cross-correlation of the envelope changes, search range ±`max_shift`.
pub fn offset(a: &[f32], b: &[f32], max_shift: f64) -> Option<(f64, f32)> {
    let prep = |x: &[f32]| -> Vec<f32> {
        // changes instead of levels: independent of microphone and volume
        let d: Vec<f32> = x.windows(2).map(|w| (w[1] - w[0]).max(0.0)).collect();
        let m = d.iter().sum::<f32>() / d.len().max(1) as f32;
        let v = (d.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / d.len().max(1) as f32)
            .sqrt()
            .max(1e-9);
        d.iter().map(|x| (x - m) / v).collect()
    };
    let (a, b) = (prep(a), prep(b));
    if a.len() < 50 || b.len() < 50 {
        return None;
    }
    let max = (max_shift * ENV_RATE as f64) as i64;
    let mut best = (0i64, f32::MIN);
    for lag in -max..=max {
        let mut s = 0.0f32;
        let mut n = 0usize;
        for (i, av) in a.iter().enumerate() {
            let j = i as i64 + lag;
            if j >= 0 && (j as usize) < b.len() {
                s += av * b[j as usize];
                n += 1;
            }
        }
        if n > 100 {
            let c = s / n as f32;
            if c > best.1 {
                best = (lag, c);
            }
        }
    }
    (best.1 > f32::MIN).then(|| (best.0 as f64 / ENV_RATE as f64, best.1))
}

/// Filler words (German/English), lowercase without punctuation
pub const FILLERS: &[&str] = &[
    "äh", "ähm", "öh", "öhm", "hm", "hmm", "mhm", "äähm", "uh", "um", "uhm", "erm", "ehm", "eh",
];

pub fn is_filler(w: &str) -> bool {
    let w: String = w
        .chars()
        .filter(|c| c.is_alphabetic())
        .collect::<String>()
        .to_lowercase();
    FILLERS.contains(&w.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst_track(len: usize, at: &[usize]) -> Vec<f32> {
        let mut v = vec![0.01f32; len];
        for &a in at {
            for k in 0..20 {
                if a + k < len {
                    v[a + k] = 0.5;
                }
            }
        }
        v
    }

    #[test]
    fn silences_and_highlights_and_offset() {
        let env = burst_track(1000, &[100, 600]);
        let s = silences(&env, -30.0, 0.5);
        assert!(s.iter().any(|(a, b)| *a < 0.1 && *b > 0.9), "{s:?}");
        let h = loud_moments(&env, 5, 2.0);
        assert_eq!(h.len(), 2, "{h:?}");
        assert!((h[0] - 1.0).abs() < 0.3 && (h[1] - 6.0).abs() < 0.3);
        let a = burst_track(1500, &[200, 450, 900, 1200]);
        let b = burst_track(1500, &[237, 487, 937, 1237]);
        let (o, _) = offset(&a, &b, 3.0).unwrap();
        assert!((o - 0.37).abs() < 0.02, "{o}");
        assert!(is_filler("Ähm,") && is_filler("uh") && !is_filler("Hallo"));
    }

    #[test]
    fn audio_helpers_on_empty_flat_and_odd_input() {
        assert!(envelope_samples(&[], 16_000).is_empty());
        assert_eq!(envelope_samples(&[1.0; 10], 16_000), vec![1.0]);
        assert!(silences(&[], -30.0, 0.5).is_empty());
        // everything quiet (silence only): no range, since the level is relative to the loudest
        let flat = vec![0.0f32; 300];
        let s = silences(&flat, -30.0, 0.5);
        assert!(s.iter().all(|(a, b)| a < b && *b <= 3.0 + 1e-9), "{s:?}");
        // a quiet ending is detected up to the end
        let mut env = vec![0.5f32; 100];
        env.extend(vec![0.0; 100]);
        assert_eq!(silences(&env, -30.0, 0.5), vec![(1.0, 2.0)]);
        // too short a pause drops out
        let mut env = vec![0.5f32; 100];
        env.extend(vec![0.0; 10]);
        env.extend(vec![0.5; 100]);
        assert!(silences(&env, -30.0, 0.5).is_empty());
        assert!(loud_moments(&[], 3, 1.0).is_empty());
        assert!(loud_moments(&flat, 3, 1.0).is_empty(), "nothing is louder than the surroundings");
        assert!(loud_moments(&env, 0, 1.0).is_empty());
        let nan = vec![f32::NAN; 200];
        let _ = loud_moments(&nan, 3, 1.0);
        let _ = silences(&nan, -30.0, 0.5);
        assert!(offset(&[0.1; 10], &[0.1; 10], 1.0).is_none(), "too short");
        let a = burst_track(800, &[100, 300, 650]);
        let (o, _) = offset(&a, &a, 2.0).unwrap();
        assert_eq!(o, 0.0);
        assert!(db(0.0).is_finite() && (db(1.0)).abs() < 1e-6);
        for w in ["", "🎬", "...", "Ähm!!", "ÄHM", "hmmm"] {
            let _ = is_filler(w);
        }
        assert!(is_filler("ÄHM") && !is_filler("") && !is_filler("hmmm"));
    }
}
