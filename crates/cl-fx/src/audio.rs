//! Audio tools: denoise, enhance voice, normalize loudness,
//! bass/treble and voice filters (voice changer) – all as an FFmpeg audio chain.

use cl_core::AudioFx;

/// Voice filters: (id, name, FFmpeg chain).
pub static VOICES: &[(&str, &str, &str)] = &[
    ("chipmunk", "Chipmunk", "rubberband=pitch=1.7:formant=preserved"),
    ("high", "High", "rubberband=pitch=1.3"),
    ("deep", "Deep", "rubberband=pitch=0.75"),
    ("monster", "Monster", "rubberband=pitch=0.6,aecho=0.8:0.6:40:0.4"),
    ("robot", "Robot", "afftfilt=real='hypot(re,im)*sin(0)':imag='hypot(re,im)*cos(0)':win_size=512:overlap=0.75,volume=2"),
    ("alien", "Alien", "rubberband=pitch=1.25,vibrato=f=7:d=0.6"),
    ("echo", "Echo", "aecho=0.8:0.7:280:0.45"),
    ("hall", "Hall", "aecho=0.8:0.85:60|120|190|270:0.45|0.35|0.25|0.15"),
    ("radio", "Radio", "highpass=f=450,lowpass=f=3200,acrusher=bits=10:mix=0.25,volume=1.6"),
    ("phone", "Phone", "highpass=f=320,lowpass=f=3400,volume=1.8"),
    ("megaphone", "Megaphone", "highpass=f=700,lowpass=f=3000,acrusher=bits=6:mix=0.35,volume=2"),
    ("vibrato", "Tremble", "vibrato=f=6:d=0.5"),
    ("underwater", "Underwater", "lowpass=f=700,aecho=0.7:0.6:30:0.5,vibrato=f=1.5:d=0.3"),
];

/// Audio filters: (id, name, FFmpeg chain).
pub static FILTERS: &[(&str, &str, &str)] = &[
    ("podcast", "Podcast", "highpass=f=80,acompressor=threshold=0.1:ratio=3:attack=10:release=150:makeup=2,equalizer=f=3000:t=q:w=1.4:g=3"),
    ("warm", "Warmer", "bass=g=4:f=150,treble=g=-3:f=6000"),
    ("clear", "Clearer", "highpass=f=100,equalizer=f=3500:t=q:w=1.2:g=4,treble=g=2:f=8000"),
    ("bassboost", "Bass boost", "bass=g=8:f=90"),
    ("lofi", "Lo-fi", "lowpass=f=3500,acrusher=bits=8:mix=0.3"),
    ("laptop", "Laptop", "highpass=f=400,lowpass=f=6000"),
    ("stadium", "Stadium", "aecho=0.8:0.8:120|240:0.3|0.2"),
    ("nextroom", "Next room", "lowpass=f=500,volume=1.4"),
    ("vintage", "Vintage", "highpass=f=200,lowpass=f=5000,acrusher=bits=12:mix=0.15"),
    ("wide", "Wider", "extrastereo=m=2"),
];

/// Chain for audio editing (empty if nothing is set).
pub fn audio_chain(a: &AudioFx) -> Vec<String> {
    audio_chain_tagged(a, "")
}

/// [`audio_chain`] for a graph with several clips: the reverb needs two labels inside the chain,
/// `tag` keeps them unique per clip.
pub fn audio_chain_tagged(a: &AudioFx, tag: &str) -> Vec<String> {
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
    v.extend(reverb(a, tag));
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

/// Reverb presets: (id, name, [amount, room size, tone, pre-delay ms]). A preset only sets the
/// four controls; "voice" follows the measured reference (pre-delay 10 ms, RT60 about 1.1 s,
/// highs of the tail −7 dB at 4–8 kHz).
pub static REVERBS: &[(&str, &str, [f32; 4])] = &[
    ("room", "Room", [0.35, 0.2, 0.6, 5.0]),
    ("voice", "Voice", [0.4, 0.55, 0.5, 10.0]),
    ("plate", "Plate", [0.4, 0.6, 0.85, 0.0]),
    ("hall", "Hall", [0.45, 0.75, 0.45, 25.0]),
    ("cathedral", "Cathedral", [0.5, 1.0, 0.35, 40.0]),
];

/// Set the reverb controls to a preset; unknown ids change nothing.
pub fn set_reverb(a: &mut AudioFx, id: &str) {
    if let Some((.., [amount, size, tone, predelay])) = REVERBS.iter().find(|p| p.0 == id) {
        (a.reverb, a.reverb_size, a.reverb_tone, a.reverb_predelay) = (*amount, *size, *tone, *predelay);
    }
}

/// Preset whose values the reverb controls hold right now.
pub fn reverb_preset(a: &AudioFx) -> Option<&'static str> {
    REVERBS.iter().find(|p| p.2 == [a.reverb, a.reverb_size, a.reverb_tone, a.reverb_predelay]).map(|p| p.0)
}

/// The room behind the reverb controls.
struct Room {
    /// Seconds before the first reflections
    predelay: f32,
    /// Seconds the early part stays at full level before the decay starts
    hold: f32,
    /// Seconds for the lows to fall by 60 dB
    rt60: f32,
    /// Cut-off (Hz) of the tail's low-pass at its start; it halves every `half` seconds
    cutoff: f32,
    half: f32,
    /// Level of the whole tail against the dry sound
    wet: f32,
}

impl Room {
    fn new(a: &AudioFx) -> Self {
        let k = |x: f32| x.clamp(0.0, 1.0);
        let (size, rt60) = (k(a.reverb_size), 0.3 * 10f32.powf(k(a.reverb_size)));
        Room {
            predelay: a.reverb_predelay.clamp(0.0, 100.0) / 1000.0,
            hold: 0.03 + 0.1 * size,
            rt60,
            cutoff: 1200.0 * (8000f32 / 1200.0).powf(k(a.reverb_tone)),
            half: (rt60 / 3.0).max(0.1),
            // squared: the lower half of the slider covers the light settings (50 % = −12 dB)
            wet: k(a.reverb).powi(2),
        }
    }

    /// Length of the impulse response in whole ms: the tail has fallen by 60 dB.
    fn ms(&self) -> f32 {
        ((self.predelay + self.hold + self.rt60) * 1000.0).round()
    }
}

/// Seconds the reverb rings after the clip's last sample (≤ 3.23 s); 0 without reverb.
pub fn reverb_tail(a: &AudioFx) -> f64 {
    if a.reverb <= 0.0 {
        return 0.0;
    }
    Room::new(a).ms() as f64 / 1000.0
}

/// Reverb by convolution (`afir`) with an impulse response made in the graph itself: the dry
/// sample, then after the pre-delay dense stereo noise with an exponential decay whose highs die
/// faster than its lows (one-pole low-pass with a falling cut-off, lows below 150 Hz removed).
/// Dense noise has no single echoes, so nothing rings metallically; the falling low-pass keeps the
/// tail from hissing. Fixed seeds: preview and export get the same samples.
/// The output is lowered by 1/√(1+2·wet²) (the tail sits in the band of the voice, so it adds more than
/// its plain energy): the overall loudness stays about the same and sustained sound does not clip.
fn reverb(a: &AudioFx, tag: &str) -> Vec<String> {
    if a.reverb <= 0.0 {
        return Vec::new();
    }
    const RATE: f32 = 48000.0;
    let r = Room::new(a);
    let len = r.ms() / 1000.0;
    let envelope = |u: f32| (1.0 - (-u / 0.004).exp()) * (-6.908 * (u - r.hold) / r.rt60).exp().min(1.0);
    let lowpass = |u: f32| 1.0 - (-std::f32::consts::TAU * r.cutoff * 2f32.powf(-u / r.half) / RATE).exp();
    // energy of the tail at gain 1 (1 ms steps; white noise −1…1 has power 1/3, a one-pole
    // low-pass with coefficient k passes k/(2−k) of it)
    let energy: f32 = (0..((len - r.predelay) * 1000.0) as usize)
        .map(|ms| {
            let (e, k) = (envelope(ms as f32 / 1000.0), lowpass(ms as f32 / 1000.0));
            e * e / 3.0 * k / (2.0 - k) * RATE / 1000.0
        })
        .sum();
    let out = 1.0 / (1.0 + 2.0 * r.wet * r.wet).sqrt();
    let gain = r.wet / energy.sqrt() * out;
    // per channel: var 1 = noise seed, 3 = time since the pre-delay, 0 = low-pass, 2 = low-cut
    let channel = |seed: u32| {
        format!(
            "if(eq(n,0),st(1,{seed}),0);st(3,t-{pd:.4});\
             st(0,ld(0)+(1-exp(-{w:.5}*pow(2,-ld(3)/{half:.4})))*(gte(ld(3),0)*(random(1)*2-1)*(1-exp(-ld(3)/0.004))*min(1,exp(-{decay:.4}*(ld(3)-{hold:.4})))-ld(0)));\
             st(2,ld(2)+0.0194*(ld(0)-ld(2)));{gain:.5}*(ld(0)-ld(2))+eq(n,0)*{out:.4}",
            pd = r.predelay,
            w = std::f32::consts::TAU * r.cutoff / RATE,
            half = r.half,
            decay = 6.908 / r.rt60,
            hold = r.hold,
        )
    };
    // silence after the last sample lets the tail ring out (afir stops with its input)
    vec![format!(
        "aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo,apad=pad_dur={len:.3}[rvd{tag}];\
         aevalsrc='{}|{}':d={len:.3}:s=48000[rvi{tag}];[rvd{tag}][rvi{tag}]afir=dry=1:wet=1:irnorm=-1:irlink=0:irgain=1",
        channel(11),
        channel(23)
    )]
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

    fn reverb_fx(amount: f32, size: f32, tone: f32, predelay: f32) -> AudioFx {
        AudioFx { reverb: amount, reverb_size: size, reverb_tone: tone, reverb_predelay: predelay, ..Default::default() }
    }

    #[test]
    fn reverb_chain_is_built_from_the_controls() {
        let r = |amount, size, tone, predelay| audio_chain(&reverb_fx(amount, size, tone, predelay));
        let mid = r(0.5, 0.5, 0.5, 10.0);
        assert_eq!(mid.len(), 2, "reverb + aformat: {mid:?}");
        // pinned: pre-delay 10 ms, hold 80 ms, RT60 0.949 s, low-pass from 3098 Hz halving every 0.316 s
        assert!(mid[0].starts_with("aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo,apad=pad_dur=1.039[rvd];aevalsrc='if(eq(n,0),st(1,11),0);st(3,t-0.0100);st(0,ld(0)+(1-exp(-0.40558*pow(2,-ld(3)/0.3162)))*(gte(ld(3),0)*(random(1)*2-1)*(1-exp(-ld(3)/0.004))*min(1,exp(-7.2817*(ld(3)-0.0800)))-ld(0)));st(2,ld(2)+0.0194*(ld(0)-ld(2)));"), "{}", mid[0]);
        assert!(mid[0].ends_with("+eq(n,0)*0.9428':d=1.039:s=48000[rvi];[rvd][rvi]afir=dry=1:wet=1:irnorm=-1:irlink=0:irgain=1"), "{}", mid[0]);
        assert!(mid[0].contains("|if(eq(n,0),st(1,23),0);"), "second channel has its own noise: {}", mid[0]);
        assert!(!mid[0].contains("aecho"), "no single echoes");
        // each control changes its own part
        assert!(r(0.5, 1.0, 0.5, 10.0)[0].contains("apad=pad_dur=3.140[") && r(0.5, 0.0, 0.5, 10.0)[0].contains("apad=pad_dur=0.340["));
        assert!(r(0.5, 0.5, 0.0, 10.0)[0].contains("exp(-0.15708*pow") && r(0.5, 0.5, 1.0, 10.0)[0].contains("exp(-1.04720*pow"));
        assert!(r(0.5, 0.5, 0.5, 60.0)[0].contains("st(3,t-0.0600)") && r(0.5, 0.5, 0.5, 60.0)[0].contains("pad_dur=1.089["));
        let dry_gain = |c: &[String]| c[0].rsplit("eq(n,0)*").next().unwrap().split('\'').next().unwrap().parse::<f32>().unwrap();
        assert!(dry_gain(&r(0.1, 0.5, 0.5, 10.0)) >= 0.9999 && (dry_gain(&r(1.0, 0.5, 0.5, 10.0)) - 0.5774).abs() < 1e-4);
        // labels carry the clip's tag, so several clips fit into one graph
        let tagged = audio_chain_tagged(&reverb_fx(0.5, 0.5, 0.5, 10.0), "7");
        assert!(tagged[0].contains("[rvd7];") && tagged[0].contains("[rvi7];[rvd7][rvi7]afir"), "{}", tagged[0]);
        // reverb sits after the other tools and before loudness normalisation
        let both = audio_chain(&AudioFx { reverb: 0.5, compress: 0.5, normalize: true, ..Default::default() });
        assert!(both[0].starts_with("acompressor") && both[1].contains("afir") && both[2].starts_with("loudnorm"), "{both:?}");
        assert_eq!(reverb_tail(&reverb_fx(0.5, 0.5, 0.5, 10.0)), 1.039);
        assert_eq!(reverb_tail(&reverb_fx(1.0, 1.0, 0.5, 100.0)), 3.23);
        assert_eq!(reverb_tail(&AudioFx::default()), 0.0);
        if !cl_media::test_tools() {
            return;
        }
        for a in [0.05, 0.5, 1.0] {
            for s in [0.0, 0.5, 1.0] {
                let peak = run(&r(a, s, 0.5, 10.0).join(",")).unwrap_or_else(|e| panic!("{a}/{s}: {e}"));
                assert!(peak > 0.02, "{a}/{s} silent ({peak})");
            }
        }
    }

    #[test]
    fn reverb_presets_set_the_controls() {
        let mut a = AudioFx { compress: 0.3, ..Default::default() };
        assert_eq!(reverb_preset(&a), None);
        set_reverb(&mut a, "voice");
        assert_eq!((a.reverb, a.reverb_size, a.reverb_tone, a.reverb_predelay, a.compress), (0.4, 0.55, 0.5, 10.0, 0.3));
        assert_eq!(reverb_preset(&a), Some("voice"));
        a.reverb_size = 0.6;
        assert_eq!(reverb_preset(&a), None, "changed by hand: no preset");
        set_reverb(&mut a, "nope");
        assert_eq!(a.reverb_size, 0.6, "unknown id changes nothing");
        let mut ids = Vec::new();
        for (id, _, v) in REVERBS {
            set_reverb(&mut a, id);
            assert_eq!(reverb_preset(&a), Some(*id));
            assert!(v[0] > 0.0 && v[0] <= 1.0 && (0.0..=1.0).contains(&v[1]) && (0.0..=1.0).contains(&v[2]) && (0.0..=100.0).contains(&v[3]), "{id}");
            assert!(!audio_chain(&a).is_empty());
            ids.push(*id);
        }
        ids.dedup();
        assert_eq!(ids.len(), REVERBS.len());
    }

    /// Samples (interleaved stereo) of the lavfi source `src` through `chain`.
    fn samples(src: &str, chain: &str) -> Vec<f32> {
        let o = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-nostdin", "-f", "lavfi", "-i", src, "-filter_complex", chain, "-f", "f32le", "-"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        o.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
    }

    /// Bug: the reverb sounded tinny and hissy – a few single echoes (the strongest 13 dB below the
    /// voice after 14 ms), highs as loud as the mids, the same on both ears. Measured on the
    /// reverb's answer to a single click.
    #[test]
    fn reverb_tail_is_dense_damped_and_wide() {
        if !cl_media::test_tools() {
            return;
        }
        let mut a = AudioFx::default();
        set_reverb(&mut a, "voice");
        let chain = audio_chain(&a).join(",");
        let click = "aevalsrc='eq(n,0)|eq(n,0)':d=0.5:s=48000";
        let v = samples(click, &chain);
        assert_eq!(v, samples(click, &chain), "same samples on every run");
        let (left, right): (Vec<f32>, Vec<f32>) = v.chunks_exact(2).map(|s| (s[0], s[1])).unzip();
        assert!((left.len() as f64 / 48000.0 - 0.5 - reverb_tail(&a)).abs() < 0.01, "rings for the tail: {} samples", left.len());
        let dry = left[0];
        assert!((dry - 0.9753).abs() < 1e-3, "dry click {dry}");
        let energy = |s: &[f32]| s.iter().map(|x| (*x as f64).powi(2)).sum::<f64>();
        let db = |x: f64| 10.0 * x.max(1e-20).log10();
        // pre-delay: silence up to 10 ms
        assert!(energy(&left[1..470]) < 1e-12, "nothing before the pre-delay");
        // wet level: amount 40 % = −16 dB against the dry click
        let wet = db(energy(&left[1..]) / (dry as f64).powi(2));
        assert!((wet + 15.9).abs() < 1.0, "wet level {wet} dB");
        // no single echo stands out (before: −13 dB)
        let peak = left[1..].iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(db((peak / dry).powi(2) as f64) < -35.0, "strongest reflection {} dB", db((peak / dry).powi(2) as f64));
        // dense: in every 20 ms of the first 300 ms about a third of the samples lie above the
        // standard deviation, like noise (single echoes: almost none)
        for w in left[480..480 + 14400].chunks(960) {
            let sd = (energy(w) / w.len() as f64).sqrt();
            let above = w.iter().filter(|x| x.abs() as f64 > sd).count() as f64 / w.len() as f64;
            assert!(above > 0.2, "dense reflections: {above}");
        }
        // decay: about −20 dB after 0.4 s like the reference, gone after 1 s
        let level = |from: usize| db(energy(&left[from..from + 2400]) / energy(&left[2400..4800]));
        let (mid, late) = (level(19200), level(48000));
        assert!(mid < -14.0 && mid > -26.0 && late < -45.0, "decay {mid} dB at 0.4 s, {late} dB at 1 s");
        // highs damped: the tail's sample-to-sample difference (its highs) is below its level
        // (white noise: +3 dB), and more so late than early
        let bright = |s: &[f32]| db(s.windows(2).map(|p| ((p[1] - p[0]) as f64).powi(2)).sum::<f64>() / energy(s));
        let (early, late) = (bright(&left[480..9600]), bright(&left[19200..38400]));
        assert!(early < -1.0 && late < early - 1.5, "highs early {early} dB, late {late} dB");
        // wide: both ears get different reflections
        let corr = left[1..].iter().zip(&right[1..]).map(|(l, r)| (*l * *r) as f64).sum::<f64>() / (energy(&left[1..]) * energy(&right[1..])).sqrt();
        assert!(corr.abs() < 0.1, "left/right correlation {corr}");
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
        let rev = |amount, size| burst(&audio_chain(&reverb_fx(amount, size, 0.5, 10.0)));
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
            // the first reflections overlap the 0.15 s burst itself and peaks add up, so allow +2.5 dB
            assert!(head(w) <= head(&dry) * 1.33, "dry part not louder: {} vs {}", head(w), head(&dry));
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
