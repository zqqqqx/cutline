//! Soundeffekte, lokal mit FFmpeg synthetisiert (keine Lizenzfragen, keine Downloads).

use std::path::Path;
use std::process::Stdio;

pub struct Sfx {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    /// lavfi-Quelle inkl. Filterkette
    pub source: &'static str,
}

pub const SFX_CATEGORIES: [&str; 4] = ["Übergang", "Oberfläche", "Spannung", "Natur"];

pub static SFX: &[Sfx] = &[
    Sfx { id: "whoosh", name: "Whoosh", category: "Übergang", source: "anoisesrc=d=0.8:c=pink:a=0.7,bandpass=f=900:width_type=h:w=700,afade=t=in:d=0.35,afade=t=out:st=0.4:d=0.4" },
    Sfx { id: "swoosh-fast", name: "Swoosh kurz", category: "Übergang", source: "anoisesrc=d=0.4:c=white:a=0.5,highpass=f=1500,afade=t=in:d=0.15,afade=t=out:st=0.15:d=0.25" },
    Sfx { id: "riser", name: "Anstieg", category: "Übergang", source: "aevalsrc='0.35*sin(2*PI*(200*t+300*t*t))*t/2':d=2" },
    Sfx { id: "impact", name: "Einschlag", category: "Übergang", source: "aevalsrc='0.9*sin(2*PI*52*t)*exp(-t*3)+0.3*sin(2*PI*90*t)*exp(-t*8)':d=1.4" },
    Sfx { id: "laser", name: "Laser", category: "Übergang", source: "aevalsrc='0.5*sin(2*PI*(1500*t-2500*t*t))*exp(-t*4)':d=0.5" },
    Sfx { id: "glitch", name: "Glitch", category: "Übergang", source: "aevalsrc='0.4*sgn(sin(2*PI*(90+300*gt(sin(2*PI*7*t),0))*t))*gt(sin(2*PI*13*t),-0.3)':d=0.6" },
    Sfx { id: "pop", name: "Pop", category: "Oberfläche", source: "aevalsrc='0.8*sin(2*PI*(600-2000*t)*t)*exp(-t*30)':d=0.15" },
    Sfx { id: "click", name: "Klick", category: "Oberfläche", source: "aevalsrc='0.9*sin(2*PI*2000*t)*exp(-t*200)':d=0.05" },
    Sfx { id: "ding", name: "Ding", category: "Oberfläche", source: "aevalsrc='0.5*sin(2*PI*1320*t)*exp(-t*4)+0.25*sin(2*PI*2640*t)*exp(-t*6)':d=1.5" },
    Sfx { id: "notify", name: "Hinweiston", category: "Oberfläche", source: "aevalsrc='0.4*sin(2*PI*if(lt(t,0.15),880,1320)*t)*exp(-mod(t,0.15)*8)':d=0.45" },
    Sfx { id: "coin", name: "Münze", category: "Oberfläche", source: "aevalsrc='0.4*sin(2*PI*if(lt(t,0.08),988,1319)*t)*exp(-t*5)':d=0.6" },
    Sfx { id: "error", name: "Fehler", category: "Oberfläche", source: "aevalsrc='0.35*sgn(sin(2*PI*150*t))*exp(-t*2)':d=0.4" },
    Sfx { id: "typing", name: "Tippen", category: "Oberfläche", source: "aevalsrc='0.6*sin(2*PI*1800*t)*exp(-mod(t,0.11)*150)':d=1.2" },
    Sfx { id: "bleep", name: "Zensur-Piep", category: "Oberfläche", source: "aevalsrc='0.4*sin(2*PI*1000*t)':d=1" },
    Sfx { id: "tick", name: "Ticken", category: "Spannung", source: "aevalsrc='0.7*sin(2*PI*3000*t)*exp(-mod(t,0.5)*300)':d=3" },
    Sfx { id: "heartbeat", name: "Herzschlag", category: "Spannung", source: "aevalsrc='0.9*sin(2*PI*50*t)*(exp(-mod(t,0.9)*25)+0.7*exp(-mod(t+0.68,0.9)*25))':d=2.7" },
    Sfx { id: "drone", name: "Brummen", category: "Spannung", source: "aevalsrc='0.25*sin(2*PI*55*t)+0.15*sin(2*PI*82.5*t)*(0.5+0.5*sin(2*PI*0.3*t))':d=4,afade=t=in:d=1,afade=t=out:st=3:d=1" },
    Sfx { id: "suspense", name: "Spannung", category: "Spannung", source: "aevalsrc='0.2*sin(2*PI*(110+20*t)*t)+0.1*sin(2*PI*(233-10*t)*t)':d=4,afade=t=in:d=1.5" },
    Sfx { id: "wind", name: "Wind", category: "Natur", source: "anoisesrc=d=4:c=brown:a=0.6,lowpass=f=500,tremolo=f=0.4:d=0.6,afade=t=in:d=1,afade=t=out:st=3:d=1" },
    Sfx { id: "rain", name: "Regen", category: "Natur", source: "anoisesrc=d=4:c=white:a=0.25,highpass=f=1800,lowpass=f=9000,afade=t=in:d=0.5,afade=t=out:st=3.5:d=0.5" },
    Sfx { id: "waves", name: "Wellen", category: "Natur", source: "anoisesrc=d=6:c=pink:a=0.5,lowpass=f=1200,tremolo=f=0.18:d=0.85,afade=t=in:d=1,afade=t=out:st=5:d=1" },
    Sfx { id: "applause", name: "Applaus", category: "Natur", source: "anoisesrc=d=3:c=white:a=0.35,highpass=f=900,tremolo=f=14:d=0.7,afade=t=in:d=0.3,afade=t=out:st=2.2:d=0.8" },
];

pub fn sfx(id: &str) -> Option<&'static Sfx> {
    SFX.iter().find(|s| s.id == id)
}

/// Erzeugt die WAV-Datei (48 kHz Stereo). Vorhandene Dateien werden wiederverwendet.
pub fn render(s: &Sfx, out: &Path) -> Result<(), String> {
    if out.is_file() {
        return Ok(());
    }
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let o = cl_media::ffmpeg_command()
        .map_err(|e| e.to_string())?
        .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", s.source, "-ar", "48000", "-ac", "2", "-c:a", "pcm_s16le"])
        .arg(out)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(())
    } else {
        let _ = std::fs::remove_file(out);
        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_sfx_render_and_are_audible() {
        if !cl_media::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        for s in SFX {
            let p = dir.path().join(format!("{}.wav", s.id));
            render(s, &p).unwrap_or_else(|e| panic!("{}: {e}", s.id));
            let m = cl_media::probe(&p).unwrap();
            assert!(m.duration.as_secs_f64() > 0.03, "{}", s.id);
            let peak = cl_media::waveform(&p, 50).unwrap().into_iter().fold(0.0, f32::max);
            assert!(peak > 0.05, "{} ist stumm ({peak})", s.id);
        }
        assert!(SFX.len() >= 20);
    }
}
