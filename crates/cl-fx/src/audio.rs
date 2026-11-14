//! Ton-Werkzeuge: Rauschen entfernen, Stimme hervorheben, Lautheit angleichen,
//! Bass/Höhen und Stimmfilter (Voice Changer) – alles als FFmpeg-Audiokette.

use cl_core::AudioFx;

/// Stimmfilter: (id, Name, FFmpeg-Kette).
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

/// Audiofilter: (id, Name, FFmpeg-Kette).
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

/// Kette für die Ton-Bearbeitung (leer, wenn nichts eingestellt ist).
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
        // Sprachband, Rauschen und Hintergrund absenken, Sprache anheben
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
    if a.normalize {
        v.push("loudnorm=I=-14:TP=-1.5:LRA=11".into());
        v.push("aresample=48000".into());
    }
    if !v.is_empty() {
        v.push("aformat=sample_fmts=fltp:channel_layouts=stereo".into());
    }
    v
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
        if !cl_media::ffmpeg_available() {
            return;
        }
        for (id, ..) in VOICES {
            let a = AudioFx {
                voice: Some(id.to_string()),
                ..Default::default()
            };
            let ch = audio_chain(&a).join(",");
            let peak = run(&ch).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(peak > 0.02, "{id} ist stumm ({peak})");
        }
        for (id, ..) in FILTERS {
            let a = AudioFx {
                filter: Some(id.to_string()),
                ..Default::default()
            };
            let ch = audio_chain(&a).join(",");
            let peak = run(&ch).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(peak > 0.02, "{id} ist stumm ({peak})");
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
            channels: None,
        };
        let peak = run(&audio_chain(&all).join(",")).unwrap();
        assert!(peak > 0.05, "{peak}");
        assert!(audio_chain(&AudioFx::default()).is_empty());
    }
}
