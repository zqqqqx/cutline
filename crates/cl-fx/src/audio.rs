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
    // strengths scale each filter's own parameter between "no effect" and the full setting
    if a.rumble {
        // cut-off from 20 Hz (inaudible) up to 90 Hz
        v.push(format!("highpass=f={}:poles=2", num(20.0 + 70.0 * amount(a.rumble_amount))));
    }
    if a.hum {
        // notch depth via the filter's own dry/wet mix (sample-aligned, so no comb filtering)
        let mix = mix(a.hum_amount);
        for f in [50, 60, 100, 120, 150, 180] {
            v.push(format!("bandreject=f={f}:width_type=q:w=25{mix}"));
        }
    }
    if a.declick {
        // detection threshold from 100 (almost nothing counts as a click) down to the default 2
        v.push(match a.declick_amount {
            k if k >= 1.0 => "adeclick".into(),
            k => format!("adeclick=t={}", num((2.0 / k.max(0.02)).min(100.0))),
        });
    }
    if a.voice_isolate {
        // speech band, lower noise and background, raise speech
        let k = amount(a.voice_isolate_amount);
        v.push(format!("highpass=f={}", num(20.0 + 70.0 * k)));
        v.push(format!("lowpass=f={}", num(20000.0 - 11000.0 * k)));
        v.push(format!("afftdn=nr={}:nf=-35:tn=1", num((18.0 * k).max(1.0))));
        v.push(format!("agate=threshold=0.015:ratio={}:attack=5:release=180", num(1.0 + 1.5 * k)));
        v.push(format!("equalizer=f=2800:t=q:w=1.2:g={}", num(3.0 * k)));
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
        v.push(scale_preset(ch, a.filter_amount));
    }
    if let Some(id) = &a.voice {
        if let Some(ch) = VOICES.iter().find(|x| x.0 == id).map(|x| x.2) {
            v.push(scale_preset(ch, a.voice_amount));
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

fn amount(k: f32) -> f32 {
    k.clamp(0.0, 1.0)
}

/// `:mix=k` for filters with a built-in dry/wet mix; nothing at full strength.
fn mix(k: f32) -> String {
    if k >= 1.0 { String::new() } else { format!(":mix={}", num(amount(k))) }
}

/// Number without trailing zeros (90, 2.5, 0.015).
fn num(x: f32) -> String {
    format!("{x:.3}").trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Can a voice/audio filter preset be weakened? The robot voice (FFT resynthesis) has no
/// parameter for it, and its FFT latency would comb-filter a dry/wet mix.
pub fn preset_adjustable(chain: &str) -> bool {
    !chain.contains("afftfilt")
}

/// Preset chain at strength `k`: every filter scaled toward "no change" by its own parameter
/// (pitch ratio, echo decays, vibrato depth, gain, stereo width, crusher mix) or, for EQ/compressor
/// filters, by their built-in sample-aligned dry/wet mix. Unchanged at full strength.
fn scale_preset(chain: &str, k: f32) -> String {
    if k >= 1.0 || !preset_adjustable(chain) {
        return chain.into();
    }
    let k = amount(k);
    let f = |v: &str| v.parse::<f32>().unwrap_or(1.0);
    let filters: Vec<String> = chain
        .split(',')
        .map(|filter| {
            let (name, args) = filter.split_once('=').unwrap_or((filter, ""));
            if matches!(name, "highpass" | "lowpass" | "bandreject" | "bass" | "treble" | "equalizer" | "acompressor") {
                return format!("{filter}{}", mix(k));
            }
            let args: Vec<String> = args
                .split(':')
                .enumerate()
                .map(|(i, arg)| match (name, arg.split_once('=')) {
                    ("rubberband", Some(("pitch", v))) => format!("pitch={}", num(f(v).powf(k))),
                    ("vibrato", Some(("d", v))) => format!("d={}", num((f(v) * k).max(0.01))),
                    ("acrusher", Some(("mix", v))) => format!("mix={}", num(f(v) * k)),
                    ("extrastereo", Some(("m", v))) => format!("m={}", num(1.0 + (f(v) - 1.0) * k)),
                    ("volume", None) => num(f(arg).powf(k)),
                    ("aecho", None) if i == 3 => arg.split('|').map(|d| num((f(d) * k).max(0.001))).collect::<Vec<_>>().join("|"),
                    _ => arg.to_string(),
                })
                .collect();
            format!("{name}={}", args.join(":"))
        })
        .collect();
    filters.join(",")
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
        render("aevalsrc='if(lt(t,0.15),0.5*sin(2*PI*440*t),0)':d=3:s=48000:c=stereo", chain)
    }

    /// Peaks per 50 ms of the lavfi source `src` through `chain`.
    fn render(src: &str, chain: &[String]) -> Vec<f32> {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("b.wav");
        let mut cmd = cl_media::ffmpeg_command().unwrap();
        cmd.args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i"]).arg(src);
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
    /// Every strength below 100 % changes the chain and runs; the robot voice has no strength.
    #[test]
    fn strengths_change_the_chain_and_run() {
        let full = AudioFx { hum: true, rumble: true, declick: true, voice_isolate: true, ..Default::default() };
        let weak = |k: f32| AudioFx { hum_amount: k, rumble_amount: k, declick_amount: k, voice_isolate_amount: k, ..full.clone() };
        let mut chains = vec![];
        for (k, want) in [(0.5, "highpass=f=55:poles=2,bandreject=f=50:width_type=q:w=25:mix=0.5"), (0.0, "highpass=f=20:poles=2,bandreject=f=50:width_type=q:w=25:mix=0")] {
            let c = audio_chain(&weak(k)).join(",");
            assert!(c.starts_with(want), "{c}");
            assert!(c.contains(if k > 0.0 { "adeclick=t=4," } else { "adeclick=t=100," }), "{c}");
            chains.push(c);
        }
        assert!(chains[0].contains("highpass=f=55,lowpass=f=14500,afftdn=nr=9:nf=-35:tn=1,agate=threshold=0.015:ratio=1.75:attack=5:release=180,equalizer=f=2800:t=q:w=1.2:g=1.5"), "{}", chains[0]);
        for (id, _, ch) in VOICES.iter().chain(FILTERS) {
            let a = AudioFx {
                voice: VOICES.iter().any(|v| v.0 == *id).then(|| id.to_string()),
                filter: FILTERS.iter().any(|v| v.0 == *id).then(|| id.to_string()),
                voice_amount: 0.5,
                filter_amount: 0.5,
                ..Default::default()
            };
            let c = audio_chain(&a).join(",");
            assert_eq!(c.starts_with(&format!("{ch},")), !preset_adjustable(ch), "{id}: {c}");
            chains.push(c);
        }
        assert!(!preset_adjustable(VOICES.iter().find(|v| v.0 == "robot").unwrap().2));
        assert_eq!(scale_preset("rubberband=pitch=0.6,aecho=0.8:0.6:40:0.4", 0.5), "rubberband=pitch=0.775,aecho=0.8:0.6:40:0.2");
        assert_eq!(
            scale_preset("acrusher=bits=8:mix=0.3,volume=1.6,extrastereo=m=2,vibrato=f=6:d=0.5,bass=g=8:f=90", 0.5),
            "acrusher=bits=8:mix=0.15,volume=1.265,extrastereo=m=1.5,vibrato=f=6:d=0.25,bass=g=8:f=90:mix=0.5"
        );
        if !cl_media::test_tools() {
            return;
        }
        for c in chains {
            let peak = run(&c).unwrap_or_else(|e| panic!("{c}: {e}"));
            assert!(peak > 0.02, "{c} is silent ({peak})");
        }
    }

    /// Peak of a steady `hz` tone after the filters settled.
    fn tone_level(hz: u32, chain: &[String]) -> f32 {
        render(&format!("aevalsrc='0.5*sin(2*PI*{hz}*t)':d=3:s=48000:c=stereo"), chain)[30..55].iter().fold(0.0, |m, x| m.max(*x))
    }

    #[test]
    fn hum_and_rumble_strength_scale_the_cut() {
        if !cl_media::test_tools() {
            return;
        }
        let cases: [(u32, AudioFx, fn(&mut AudioFx, f32)); 2] = [
            (50, AudioFx { hum: true, ..Default::default() }, |a, k| a.hum_amount = k),
            (30, AudioFx { rumble: true, ..Default::default() }, |a, k| a.rumble_amount = k),
        ];
        for (hz, on, set) in cases {
            let off = tone_level(hz, &[]);
            let full = tone_level(hz, &audio_chain(&on));
            let mut half = on.clone();
            set(&mut half, 0.5);
            let half = tone_level(hz, &audio_chain(&half));
            assert!(off > half * 1.2 && half > full * 1.2, "{hz} Hz: off {off}, 50 % {half}, 100 % {full}");
        }
    }
}
