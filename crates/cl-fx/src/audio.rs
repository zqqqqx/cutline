//! Audio tools: denoise, enhance voice, normalize loudness,
//! bass/treble and voice filters (voice changer) – all as an FFmpeg audio chain.

use cl_core::AudioFx;

/// Voice filters: (id, name, FFmpeg chain).
pub static VOICES: &[(&str, &str, &str)] = &[
    ("chipmunk", "Chipmunk", "rubberband=pitch=1.7:formant=preserved"),
    ("high", "Hoch", "rubberband=pitch=1.3"),
    ("deep", "Tief", "rubberband=pitch=0.75"),
    ("monster", "Monster", "rubberband=pitch=0.6,aecho=0.8:0.6:40:0.4"),
    ("robot", "Roboter", "afftfilt=real='hypot(re,im)*sin(0)':imag='hypot(re,im)*cos(0)':win_size=512:overlap=0.75,volume=2"),
    ("alien", "Alien", "rubberband=pitch=1.25,vibrato=f=7:d=0.6"),
    ("echo", "Echo", "aecho=0.8:0.7:280:0.45"),
    ("hall", "Halle", "aecho=0.8:0.85:60|120|190|270:0.45|0.35|0.25|0.15"),
    ("radio", "Radio", "highpass=f=450,lowpass=f=3200,acrusher=bits=10:mix=0.25,volume=1.6"),
    ("phone", "Telefon", "highpass=f=320,lowpass=f=3400,volume=1.8"),
    ("megaphone", "Megafon", "highpass=f=700,lowpass=f=3000,acrusher=bits=6:mix=0.35,volume=2"),
    ("vibrato", "Zittern", "vibrato=f=6:d=0.5"),
    ("underwater", "Unter Wasser", "lowpass=f=700,aecho=0.7:0.6:30:0.5,vibrato=f=1.5:d=0.3"),
];

/// Audio filters: (id, name, FFmpeg chain).
pub static FILTERS: &[(&str, &str, &str)] = &[
    ("podcast", "Podcast", "highpass=f=80,acompressor=threshold=0.1:ratio=3:attack=10:release=150:makeup=2,equalizer=f=3000:t=q:w=1.4:g=3"),
    ("warm", "Wärmer", "bass=g=4:f=150,treble=g=-3:f=6000"),
    ("clear", "Klarer", "highpass=f=100,equalizer=f=3500:t=q:w=1.2:g=4,treble=g=2:f=8000"),
    ("bassboost", "Bass-Boost", "bass=g=8:f=90"),
    ("lofi", "Lo-Fi", "lowpass=f=3500,acrusher=bits=8:mix=0.3"),
    ("laptop", "Laptop", "highpass=f=400,lowpass=f=6000"),
    ("stadium", "Stadion", "aecho=0.8:0.8:120|240:0.3|0.2"),
    ("nextroom", "Nebenraum", "lowpass=f=500,volume=1.4"),
    ("vintage", "Vintage", "highpass=f=200,lowpass=f=5000,acrusher=bits=12:mix=0.15"),
    ("wide", "Breiter", "extrastereo=m=2"),
];

/// Chain for audio editing (empty if nothing is set).
pub fn audio_chain(a: &AudioFx) -> Vec<String> {
    let mut v = Vec::new();
    if a.rumble {
        v.push("highpass=f=90:poles=2".into());
    }
    if a.hum {
        for f in [50, 60, 100, 120, 150, 180] {
            v.push(format!("bandreject=f={f}:width_type=q:w=25"));
        }
    }
    if a.declick {
        v.push("adeclick".into());
    }
    if a.voice_isolate {
        // speech band, lower noise and background, raise speech
        v.push("highpass=f=90".into());
        v.push("lowpass=f=9000".into());
        v.push("afftdn=nr=18:nf=-35:tn=1".into());
        v.push("agate=threshold=0.015:ratio=2.5:attack=5:release=180".into());
        v.push("equalizer=f=2800:t=q:w=1.2:g=3".into());
    }
    if a.denoise > 0.0 {
        v.push(format!(
            "afftdn=nr={:.1}:nf=-40:tn=1",
            6.0 + 30.0 * a.denoise.clamp(0.0, 1.0)
        ));
    }
    if a.bass.abs() > 0.05 {
        v.push(format!("bass=g={:.1}:f=110", a.bass.clamp(-12.0, 12.0)));
    }
    if a.treble.abs() > 0.05 {
        v.push(format!(
            "treble=g={:.1}:f=4000",
            a.treble.clamp(-12.0, 12.0)
        ));
    }
    if a.compress > 0.0 {
        let k = a.compress.clamp(0.0, 1.0);
        v.push(format!(
            "acompressor=threshold={:.3}:ratio={:.1}:attack=8:release=160:makeup={:.2}",
            0.25 - 0.2 * k,
            2.0 + 6.0 * k,
            1.0 + 2.0 * k
        ));
    }
    if let Some(ch) = a.filter.as_deref().and_then(|id| FILTERS.iter().find(|x| x.0 == id)).map(|x| x.2) {
        v.push(ch.to_string());
    }
    if let Some(id) = &a.voice {
        if let Some(ch) = VOICES.iter().find(|x| x.0 == id).map(|x| x.2) {
            v.push(ch.to_string());
        }
    }
    // last sound shaping before loudness: the room colours the finished voice; fades and clip volume
    // come later in the clip graph, so they also fade the reverb
    v.extend(reverb(a.reverb, a.reverb_size));
    if a.normalize {
        v.push("loudnorm=I=-14:TP=-1.5:LRA=11".into());
        v.push("aresample=48000".into());
    }
    if !v.is_empty() {
        v.push("aformat=sample_fmts=fltp:channel_layouts=stereo".into());
    }
    v
}

/// Reverb from three cascaded multi-tap `aecho` stages (feed-forward, so the cascade makes the tail
/// dense: 4×4×4 echoes). Room size stretches the delays (×0.5…2) and the decay time (RT60 0.4…2.4 s);
/// the tap gains follow that decay and are scaled by the amount. Each stage lowers its output by
/// 1/√(1+Σgain²), so the overall loudness stays about the same and sustained sound does not clip.
fn reverb(amount: f32, size: f32) -> Vec<String> {
    if amount <= 0.0 {
        return Vec::new();
    }
    let (a, s) = (amount.clamp(0.0, 1.0), size.clamp(0.0, 1.0));
    let (stretch, rt60) = (0.5 + 1.5 * s, 0.4 + 2.0 * s);
    [[11.0, 17.0, 29.0, 41.0], [59.0, 89.0, 131.0, 193.0], [263.0, 379.0, 523.0, 701.0]]
        .iter()
        .map(|taps: &[f32; 4]| {
            let ms: Vec<f32> = taps.iter().map(|t| (t * stretch).round()).collect();
            let gains: Vec<f32> = ms.iter().map(|t| (0.5 * a * 10f32.powf(-3.0 * t / 1000.0 / rt60)).max(0.001)).collect();
            let out = 1.0 / (1.0 + gains.iter().map(|g| g * g).sum::<f32>()).sqrt();
            let join = |v: &[f32], d: usize| v.iter().map(|x| format!("{x:.d$}")).collect::<Vec<_>>().join("|");
            format!("aecho=1:{out:.3}:{}:{}", join(&ms, 0), join(&gains, 3))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    fn run(chain: &str) -> Result<f32, String> {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("o.wav");
        let o = cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-nostdin",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "aevalsrc='0.3*sin(2*PI*220*t)+0.05*random(0)':d=2:s=48000:c=stereo",
            ])
            .args(["-af", chain])
            .arg(&out)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).into_owned());
        }
        Ok(cl_media::waveform(&out, 50)
            .unwrap()
            .into_iter()
            .fold(0.0, f32::max))
    }

    #[test]
    fn every_voice_and_tool_runs_and_stays_audible() {
        if !cl_media::test_tools() {
            return;
        }
        for (id, ..) in VOICES {
            let a = AudioFx {
                voice: Some(id.to_string()),
                ..Default::default()
            };
            let ch = audio_chain(&a).join(",");
            let peak = run(&ch).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(peak > 0.02, "{id} is silent ({peak})");
        }
        for (id, ..) in FILTERS {
            let a = AudioFx {
                filter: Some(id.to_string()),
                ..Default::default()
            };
            let ch = audio_chain(&a).join(",");
            let peak = run(&ch).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(peak > 0.02, "{id} is silent ({peak})");
        }
        let all = AudioFx {
            denoise: 0.7,
            voice_isolate: true,
            normalize: true,
            voice: None,
            bass: 4.0,
            treble: -3.0,
            hum: true,
            rumble: true,
            declick: true,
            compress: 0.6,
            filter: Some("podcast".into()),
            ..Default::default()
        };
        let peak = run(&audio_chain(&all).join(",")).unwrap();
        assert!(peak > 0.05, "{peak}");
        assert!(audio_chain(&AudioFx::default()).is_empty());
    }
    /// Settings whose chains were captured from 0.15.4 (tests/data/audio_chains_0.15.4.txt).
    fn pinned_settings() -> Vec<AudioFx> {
        let mut v = vec![
            AudioFx {
                denoise: 0.7,
                voice_isolate: true,
                normalize: true,
                bass: 4.0,
                treble: -3.0,
                hum: true,
                rumble: true,
                declick: true,
                compress: 0.6,
                filter: Some("podcast".into()),
                ..Default::default()
            },
            AudioFx { rumble: true, declick: true, voice: Some("chipmunk".into()), ..Default::default() },
            AudioFx { hum: true, filter: Some("lofi".into()), voice: Some("robot".into()), denoise: 0.25, ..Default::default() },
        ];
        v.extend(VOICES.iter().map(|(id, ..)| AudioFx { voice: Some(id.to_string()), ..Default::default() }));
        v.extend(FILTERS.iter().map(|(id, ..)| AudioFx { filter: Some(id.to_string()), ..Default::default() }));
        v.extend([0.0, 0.3, 1.0].map(|k| AudioFx { voice_isolate: true, compress: k, bass: -2.5, ..Default::default() }));
        v
    }

    #[test]
    fn chains_with_default_new_values_are_unchanged() {
        let want: Vec<&str> = include_str!("../tests/data/audio_chains_0.15.4.txt").lines().collect();
        let got: Vec<String> = pinned_settings().iter().map(|a| audio_chain(a).join(",")).collect();
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(&want) {
            assert_eq!(g, w);
        }
    }

    #[test]
    fn reverb_adds_echo_stages_shaped_by_amount_and_size() {
        let r = |amount, size| audio_chain(&AudioFx { reverb: amount, reverb_size: size, ..Default::default() });
        let mid = r(0.5, 0.5);
        assert_eq!(mid.len(), 4, "three aecho stages + aformat: {mid:?}");
        assert!(mid[..3].iter().all(|f| f.starts_with("aecho=1:")), "{mid:?}");
        let delays = |c: &[String]| c[2].split(':').nth(2).unwrap().to_string();
        let gains = |c: &[String]| c[0].split(':').nth(3).unwrap().to_string();
        assert_ne!(delays(&r(0.5, 0.0)), delays(&r(0.5, 1.0)), "size changes the delays");
        assert_eq!(delays(&r(0.2, 0.5)), delays(&r(0.9, 0.5)), "amount keeps the delays");
        assert_ne!(gains(&r(0.2, 0.5)), gains(&r(0.9, 0.5)), "amount changes the mix");
        // reverb sits after the other tools and before loudness normalisation
        let both = audio_chain(&AudioFx { reverb: 0.5, compress: 0.5, normalize: true, ..Default::default() });
        assert!(both[0].starts_with("acompressor") && both[1].starts_with("aecho") && both[4].starts_with("loudnorm"), "{both:?}");
        if !cl_media::test_tools() {
            return;
        }
        for a in [0.05, 0.5, 1.0] {
            for s in [0.0, 0.5, 1.0] {
                let peak = run(&r(a, s).join(",")).unwrap_or_else(|e| panic!("{a}/{s}: {e}"));
                assert!(peak > 0.02, "{a}/{s} silent ({peak})");
            }
        }
    }

    /// Peaks per 50 ms of a 0.15 s tone burst followed by silence, through `chain`.
    fn burst(chain: &[String]) -> Vec<f32> {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("b.wav");
        let mut cmd = cl_media::ffmpeg_command().unwrap();
        cmd.args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i"])
            .arg("aevalsrc='if(lt(t,0.15),0.5*sin(2*PI*440*t),0)':d=3:s=48000:c=stereo");
        if !chain.is_empty() {
            cmd.args(["-af", &chain.join(",")]);
        }
        let o = cmd.args(["-c:a", "pcm_f32le"]).arg(&out).stdin(Stdio::null()).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        cl_media::waveform(&out, 20).unwrap()
    }

    #[test]
    fn reverb_rings_after_a_burst_and_keeps_the_dry_level() {
        if !cl_media::test_tools() {
            return;
        }
        let rev = |amount, size| burst(&audio_chain(&AudioFx { reverb: amount, reverb_size: size, ..Default::default() }));
        let dry = burst(&[]);
        let tail = |w: &[f32], from: usize| w[from..].iter().sum::<f32>();
        assert!(tail(&dry, 4) < 0.001, "silence stays silent without reverb: {dry:?}");
        let small = rev(0.8, 0.0);
        let large = rev(0.8, 1.0);
        let mid = rev(0.8, 0.5);
        assert!(mid[6] > 0.01 && mid[6] > mid[16], "tail present and decaying: {mid:?}");
        assert!(tail(&large, 12) > 2.0 * tail(&small, 12) + 0.01, "large room rings longer:\nsmall {small:?}\nlarge {large:?}");
        let head = |w: &[f32]| w[..3].iter().fold(0f32, |m, x| m.max(*x));
        for w in [&small, &mid, &large, &rev(1.0, 1.0)] {
            // the echoes overlap the 0.15 s burst itself, so allow +1 dB (measured: at most +0.8 dB)
            assert!(head(w) <= head(&dry) * 1.12, "dry part not louder: {} vs {}", head(w), head(&dry));
            assert!(w.iter().all(|x| *x < 0.99), "no clipping: {w:?}");
        }
    }
}
