//! Beat-Erkennung: Onset-Hüllkurve (Energiezuwachs auf hochpassgefiltertem Ton),
//! Tempo per Autokorrelation (60–190 BPM), Beat-Raster mit bester Phase, jeder Schlag
//! auf das nächste lokale Maximum verfeinert.

use std::path::Path;

const SR: u32 = 11_025;
const HOP: usize = 256;

pub struct Beats {
    pub bpm: f64,
    /// Zeiten (s, relativ zu `start`)
    pub times: Vec<f64>,
}

fn onset_env(x: &[f32]) -> Vec<f32> {
    // Hochpass (Vorverzerrung), Energie je Fenster, log, positive Differenz
    let mut prev = 0.0;
    let hp: Vec<f32> = x
        .iter()
        .map(|v| {
            let y = v - 0.97 * prev;
            prev = *v;
            y
        })
        .collect();
    let e: Vec<f32> = hp.chunks(HOP).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32 + 1e-9).ln()).collect();
    let mut o: Vec<f32> = e.windows(2).map(|w| (w[1] - w[0]).max(0.0)).collect();
    o.insert(0, 0.0);
    // gleitenden Mittelwert abziehen (Dauerton unterdrücken)
    let k = 16;
    let mean: Vec<f32> = (0..o.len()).map(|i| o[i.saturating_sub(k)..(i + k).min(o.len())].iter().sum::<f32>() / (2 * k) as f32).collect();
    o.iter().zip(mean).map(|(v, m)| (v - m).max(0.0)).collect()
}

pub fn detect_samples(x: &[f32]) -> Beats {
    let o = onset_env(x);
    let fr = SR as f64 / HOP as f64; // Hüllkurvenwerte je Sekunde
    if o.len() < (fr * 2.0) as usize {
        return Beats { bpm: 0.0, times: Vec::new() };
    }
    // Tempo: Autokorrelation, leicht zu 120 BPM gewichtet
    let (lo, hi) = ((fr * 60.0 / 190.0) as usize, (fr * 60.0 / 60.0) as usize);
    let mut best = (0usize, 0f64);
    for lag in lo..=hi.min(o.len() / 2) {
        let ac: f64 = o.iter().zip(&o[lag..]).map(|(a, b)| (*a * *b) as f64).sum::<f64>() / (o.len() - lag) as f64;
        let bpm = 60.0 * fr / lag as f64;
        let w = (-((bpm / 120.0).log2().powi(2)) / 1.5).exp();
        if ac * w > best.1 {
            best = (lag, ac * w);
        }
    }
    let lag = best.0.max(1);
    // Vom stärksten Onset aus in beide Richtungen dem Raster folgen; jeder Schlag rastet
    // auf das lokale Maximum ein (so driftet das Raster nicht weg)
    let r = (lag / 6).max(1);
    let refine = |i: usize| -> usize {
        let a = i.saturating_sub(r);
        let b = (i + r + 1).min(o.len());
        let j = (a..b).max_by(|x, y| o[*x].total_cmp(&o[*y])).unwrap_or(i);
        if o[j] > 0.0 { j } else { i }
    };
    let peak = (0..o.len()).max_by(|a, b| o[*a].total_cmp(&o[*b])).unwrap_or(0);
    let mut idx = vec![peak];
    let mut i = peak;
    while i >= lag {
        i = refine(i - lag);
        idx.push(i);
    }
    let mut i = peak;
    while i + lag < o.len() {
        i = refine(i + lag);
        idx.push(i);
    }
    idx.sort_unstable();
    idx.dedup();
    let times: Vec<f64> = idx.into_iter().map(|j| j as f64 / fr).collect();
    Beats { bpm: 60.0 * fr / lag as f64, times }
}

pub fn detect(path: &Path, start: f64, dur: f64) -> Result<Beats, String> {
    let x = crate::decode_audio(path, SR, start, Some(dur))?;
    Ok(detect_samples(&x))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_120_bpm_clicks() {
        // Klicks alle 0,5 s ab 0,25 s, dazu leiser Dauerton
        let n = (SR as f64 * 8.0) as usize;
        let mut x: Vec<f32> = (0..n).map(|i| 0.05 * (i as f32 * 220.0 * std::f32::consts::TAU / SR as f32).sin()).collect();
        for k in 0..16 {
            let s = ((0.25 + k as f64 * 0.5) * SR as f64) as usize;
            for j in 0..400.min(n - s) {
                x[s + j] += 0.8 * (j as f32 * 0.9).sin() * (-(j as f32) / 80.0).exp();
            }
        }
        let b = detect_samples(&x);
        assert!((b.bpm - 120.0).abs() < 4.0, "{}", b.bpm);
        assert!(b.times.len() >= 14, "{:?}", b.times);
        for t in &b.times {
            let off = ((t - 0.25) / 0.5 - ((t - 0.25) / 0.5).round()).abs() * 0.5;
            assert!(off < 0.05, "{t} liegt nicht auf dem Raster");
        }
    }
}
