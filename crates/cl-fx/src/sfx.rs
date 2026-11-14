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

pub const SFX_CATEGORIES: [&str; 6] = ["Übergang", "Gaming", "Comedy", "Oberfläche", "Spannung", "Natur"];

pub static SFX: &[Sfx] = &[
    Sfx { id: "whoosh", name: "Whoosh", category: "Übergang", source: "anoisesrc=d=0.8:c=pink:a=0.7,bandpass=f=900:width_type=h:w=700,afade=t=in:d=0.35,afade=t=out:st=0.4:d=0.4" },
    Sfx { id: "swoosh-fast", name: "Swoosh kurz", category: "Übergang", source: "anoisesrc=d=0.4:c=white:a=0.5,highpass=f=1500,afade=t=in:d=0.15,afade=t=out:st=0.15:d=0.25" },
    Sfx { id: "riser", name: "Anstieg", category: "Übergang", source: "aevalsrc='0.35*sin(2*PI*(200*t+300*t*t))*t/2':d=2" },
    Sfx { id: "impact", name: "Einschlag", category: "Übergang", source: "aevalsrc='0.85*sin(2*PI*(60*t-8*t*t))*exp(-t*2.6)+0.25*sin(2*PI*120*t)*exp(-t*9)+0.4*(random(0)*2-1)*exp(-t*30)':d=1.6,lowpass=f=6000" },
    Sfx { id: "boom", name: "Bass-Drop", category: "Übergang", source: "aevalsrc='0.9*sin(2*PI*(80*t-25*t*t))*exp(-t*1.6)':d=2.5" },
    Sfx { id: "braam", name: "Kino-Braam", category: "Übergang", source: "aevalsrc='0.22*(sin(2*PI*55*t)+0.6*sin(2*PI*110.5*t)+0.4*sin(2*PI*164*t)+0.35*sgn(sin(2*PI*54.6*t)))*min(1,t*8)*exp(-t*0.9)':d=3.5,lowpass=f=900" },
    Sfx { id: "swipe", name: "Wischen", category: "Übergang", source: "anoisesrc=d=0.3:c=white:a=0.6,bandpass=f=3000:width_type=h:w=2500,afade=t=in:d=0.1,afade=t=out:st=0.1:d=0.2" },
    Sfx { id: "reverse-cymbal", name: "Rückwärts-Becken", category: "Übergang", source: "anoisesrc=d=2:c=white:a=0.5,highpass=f=4000,afade=t=in:d=1.9:curve=exp,afade=t=out:st=1.9:d=0.1" },
    Sfx { id: "tapestop", name: "Bandstopp", category: "Übergang", source: "aevalsrc='0.4*sin(2*PI*(220*t-100*t*t))*(1-t/1.2)':d=1.1" },
    Sfx { id: "levelup", name: "Level Up", category: "Gaming", source: "aevalsrc='0.3*sgn(sin(2*PI*if(lt(t,0.1),523,if(lt(t,0.2),659,if(lt(t,0.3),784,1047)))*t))*exp(-mod(t,0.1)*6)*lt(t,0.6)':d=0.65,lowpass=f=4000" },
    Sfx { id: "victory", name: "Sieg-Fanfare", category: "Gaming", source: "aevalsrc='0.28*sgn(sin(2*PI*if(lt(t,0.15),523,if(lt(t,0.3),659,if(lt(t,0.45),784,1047)))*t))*(lt(t,0.45)*exp(-mod(t,0.15)*4)+gte(t,0.45)*exp(-(t-0.45)*1.5))':d=1.6,lowpass=f=3500" },
    Sfx { id: "jump", name: "Sprung", category: "Gaming", source: "aevalsrc='0.3*sgn(sin(2*PI*(300*t+1800*t*t)))*exp(-t*6)':d=0.3,lowpass=f=3500" },
    Sfx { id: "powerdown", name: "Power-down", category: "Gaming", source: "aevalsrc='0.3*sgn(sin(2*PI*(900*t-700*t*t)))*exp(-t*2)':d=0.6,lowpass=f=2500" },
    Sfx { id: "hitmarker", name: "Treffer", category: "Gaming", source: "aevalsrc='0.6*sin(2*PI*3200*t)*exp(-t*70)+0.3*(random(0)*2-1)*exp(-t*120)':d=0.1" },
    Sfx { id: "countdown", name: "Countdown", category: "Gaming", source: "aevalsrc='0.4*sin(2*PI*if(lt(t,3),880,1760)*t)*lt(mod(t,1),0.15)':d=3.3" },
    Sfx { id: "airhorn", name: "Airhorn", category: "Comedy", source: "aevalsrc='0.2*(sgn(sin(2*PI*415*t))+sgn(sin(2*PI*523*t))+sgn(sin(2*PI*622*t)))*lt(mod(t,0.55),0.42)*lt(t,1.6)':d=1.7,lowpass=f=3000" },
    Sfx { id: "rimshot", name: "Ba-dum-tss", category: "Comedy", source: "aevalsrc='0.8*sin(2*PI*180*t)*exp(-t*25)+0.8*sin(2*PI*130*max(0,t-0.18))*exp(-max(0,t-0.18)*25)*gte(t,0.18)+0.35*(random(0)*2-1)*exp(-max(0,t-0.45)*4)*gte(t,0.45)':d=1.6" },
    Sfx { id: "boing", name: "Boing", category: "Comedy", source: "aevalsrc='0.5*sin(2*PI*(180+60*sin(2*PI*14*t)*exp(-t*3))*t)*exp(-t*3)':d=1" },
    Sfx { id: "scratch", name: "Plattenkratzer", category: "Comedy", source: "aevalsrc='0.6*(random(0)*2-1)*abs(sin(2*PI*3*t))':d=0.7,bandpass=f=1800:width_type=h:w=1500" },
    Sfx { id: "kaching", name: "Kasse", category: "Comedy", source: "aevalsrc='0.35*(sin(2*PI*2093*t)+0.6*sin(2*PI*2637*t))*exp(-max(0,t-0.12)*5)*gte(t,0.12)+0.5*(random(0)*2-1)*exp(-t*40)':d=1.3" },
    Sfx { id: "shutter", name: "Kamera", category: "Oberfläche", source: "aevalsrc='(random(0)*2-1)*(exp(-t*60)+0.8*gte(t,0.09)*exp(-(t-0.09)*60))':d=0.3,highpass=f=1500" },
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
        .args([
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            s.source,
            "-ar",
            "48000",
            "-ac",
            "2",
            "-c:a",
            "pcm_s16le",
        ])
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
            let peak = cl_media::waveform(&p, 50)
                .unwrap()
                .into_iter()
                .fold(0.0, f32::max);
            assert!(peak > 0.05, "{} ist stumm ({peak})", s.id);
        }
        assert!(SFX.len() >= 39);
        // Kategorien vollständig und eindeutige IDs
        assert!(SFX.iter().all(|s| SFX_CATEGORIES.contains(&s.category)));
        let mut ids: Vec<&str> = SFX.iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), SFX.len());
    }
}
