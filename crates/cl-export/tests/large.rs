//! Große Projekte (tausende Clips): Befehlszeile, Abschnitte, Messung.
//! Testmedien klein und kurz, Pfade künstlich lang (tiefe Ordner).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use cl_core::*;
use cl_export::{build, run, ExportSettings};

fn ffmpeg(args: &[&str], out: &Path) {
    let o = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-y", "-nostdin", "-threads", "2"])
        .args(args)
        .arg(out)
        .output()
        .unwrap();
    assert!(o.status.success(), "ffmpeg {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

fn no_fonts(_: Option<&str>) -> Option<(PathBuf, u32)> {
    None
}

/// Ordner mit Pfadlänge ≈ `len` Zeichen unter `root`.
fn deep_dir(root: &Path, len: usize) -> PathBuf {
    let mut d = root.to_path_buf();
    while d.to_string_lossy().len() + 20 < len {
        d = d.join("ordner_mit_langem_namen");
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Vier Testdateien (60 s): zwei Videos mit Ton (verschiedene Farben/Töne), ein Video ohne Ton, ein MP3.
fn media(dir: &Path) -> Vec<MediaRef> {
    let files = [
        ("a_video_mit_ton.mp4", vec!["-f", "lavfi", "-i", "testsrc2=s=96x54:r=30:d=60", "-f", "lavfi", "-i", "sine=f=300:d=60:r=48000", "-c:v", "libx264", "-preset", "ultrafast", "-g", "30", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"]),
        ("b_video_mit_ton.mp4", vec!["-f", "lavfi", "-i", "smptebars=s=96x54:r=30:d=60", "-f", "lavfi", "-i", "sine=f=900:d=60:r=44100", "-c:v", "libx264", "-preset", "ultrafast", "-g", "30", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"]),
        ("c_video_ohne_ton.mp4", vec!["-f", "lavfi", "-i", "rgbtestsrc=s=96x54:r=60:d=60", "-c:v", "libx264", "-preset", "ultrafast", "-g", "60", "-pix_fmt", "yuv420p"]),
        ("d_musik.mp3", vec!["-f", "lavfi", "-i", "sine=f=550:d=60:r=44100", "-c:a", "libmp3lame", "-b:a", "128k"]),
    ];
    files
        .iter()
        .map(|(n, a)| {
            let f = dir.join(n);
            if !f.is_file() {
                ffmpeg(a, &f);
            }
            cl_media::probe(&f).unwrap()
        })
        .collect()
}

/// `n` Clips auf 4 Spuren (3 Video, 1 Ton) über `secs` Sekunden, Clips aus `m` reihum.
fn big_project(m: &[MediaRef], n: usize, secs: f64) -> Project {
    let mut p = Project::new("Groß");
    p.settings.width = 96;
    p.settings.height = 54;
    p.settings.fps = Rational::new(60, 1);
    p.media.extend(m.iter().cloned());
    let per = n / 4;
    let len = secs / per as f64;
    for t in 0..4 {
        let mut tr = Track::new(if t == 3 { TrackKind::Audio } else { TrackKind::Video });
        for i in 0..per {
            let md = if t == 3 { &m[3] } else { &m[(i + t) % 3] };
            let kind = if t == 3 { MediaKind::Audio } else { MediaKind::Video };
            let start = Time::from_secs_f64(i as f64 * len).snap_to_frame(p.settings.fps);
            let end = Time::from_secs_f64((i + 1) as f64 * len).snap_to_frame(p.settings.fps);
            let mut c = Clip::new(ClipContent::Media { media: md.id, kind }, start, end - start);
            c.source_in = Time::from_secs_f64((i * 7 % 50) as f64 + 0.25 * t as f64);
            if t > 0 {
                c.transform.scale = 0.4;
                c.transform.x = 20.0 * t as f32 - 30.0;
            }
            tr.clips.push(c);
        }
        p.timeline.tracks.push(tr);
    }
    p
}

/// Zeichen der Befehlszeile wie unter Windows (Argumente mit Leerzeichen getrennt, ggf. in Anführungszeichen).
fn cmd_chars(args: &[String]) -> usize {
    args.iter().map(|a| a.len() + 3).sum()
}

/// Messung: Projekt mit 2500 Clips (26 min, Pfade ~130 Zeichen) – Länge der Befehlszeile,
/// Startversuch, Fehlertext. `cargo test --release -p cl-export --test large -- --ignored --nocapture measure_2500`
#[test]
#[ignore]
fn measure_2500_clips() {
    let root = tempfile::tempdir().unwrap();
    let dir = deep_dir(root.path(), 110);
    let m = media(&dir);
    println!("Pfadlänge: {}", m[0].path.to_string_lossy().len());
    let p = big_project(&m, 2500, 26.0 * 60.0);
    let out = root.path().join("gross.mp4");
    let t0 = Instant::now();
    let job = build(&p, &ExportSettings::from_project(&p), &no_fonts, &out).unwrap();
    println!("build: {:?}, Argumente: {}, Zeichen: {}", t0.elapsed(), job.args.len(), cmd_chars(&job.args));
    let t0 = Instant::now();
    let mut first = None;
    let r = run(&job, Arc::new(AtomicBool::new(false)), |x| {
        if x > 0.0 && first.is_none() {
            first = Some(t0.elapsed());
            println!("erster Fortschritt nach {:?}", t0.elapsed());
        }
    });
    println!("run: {:?} nach {:?}", r.as_ref().err().map(|e| e.to_string()), t0.elapsed());
}

// ---------- Abschnitte gleich Durchgang (echtes FFmpeg, klein) ----------

fn run_job(job: &cl_export::Job) {
    run(job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap();
}

/// Rohdaten eines Stroms (`-map`), z. B. `0:v` als rgb24 oder `0:a` als s16le/f32le.
fn decode(f: &Path, map: &str, fmt: &[&str]) -> Vec<u8> {
    let o = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(f)
        .args(["-map", map])
        .args(fmt)
        .arg("-")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}

fn rgb(f: &Path) -> Vec<u8> {
    decode(f, "0:v", &["-f", "rawvideo", "-pix_fmt", "rgb24"])
}

/// Zeitstempel der Bilder (ffprobe) – Anzahl und Monotonie.
fn frame_times(f: &Path) -> Vec<f64> {
    let o = cl_media::ffprobe_command()
        .unwrap()
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "frame=pts_time", "-of", "csv=p=0"])
        .arg(f)
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().trim_end_matches(',').parse().ok()).collect()
}

/// Ende eines Stroms in Sekunden (ffprobe, aus den Paketen).
fn stream_secs(f: &Path, sel: &str) -> f64 {
    let o = cl_media::ffprobe_command()
        .unwrap()
        .args(["-v", "error", "-select_streams", sel, "-show_entries", "packet=pts_time,duration_time", "-of", "csv=p=0"])
        .arg(f)
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| {
            let v: Vec<f64> = l.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (v.len() == 2).then(|| v[0] + v[1])
        })
        .fold(0.0, f64::max)
}

fn mean_diff(a: &[u8], b: &[u8]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (*x as i32 - *y as i32).abs() as f64).sum::<f64>() / a.len().max(1) as f64
}

/// Größte mittlere Abweichung je Bild (rgb24, 128×72).
fn worst_frame(a: &[u8], b: &[u8]) -> f64 {
    let fs = 128 * 72 * 3;
    a.chunks(fs).zip(b.chunks(fs)).map(|(x, y)| mean_diff(x, y)).fold(0.0, f64::max)
}

/// Kleines Projekt mit allem, was an einer Abschnittsgrenze schiefgehen kann: Übergang,
/// Keyframe-Bewegung, Text-Animation, Tempo-Kurve, Effekt-Clip darüber, langer Hintergrund
/// (springt per `-ss`), Musik mit Lautstärke-Keyframes (Ducking) und Blenden, Originalton.
fn rich_project(dir: &Path) -> Project {
    let m = media(dir);
    let mut p = Project::new("Nähte");
    p.settings.width = 128;
    p.settings.height = 72;
    p.settings.fps = Rational::new(30, 1);
    p.media.extend(m.iter().cloned());
    let s = Time::from_secs_f64;
    let clip = |md: &MediaRef, kind, a: f64, len: f64, src: f64| {
        let mut c = Clip::new(ClipContent::Media { media: md.id, kind }, s(a), s(len));
        c.source_in = s(src);
        c
    };
    // Hintergrund über die ganze Länge (unten), darüber Schnitte
    let mut bg = Track::new(TrackKind::Video);
    bg.clips.push(clip(&m[2], MediaKind::Video, 0.0, 12.0, 3.0));
    let mut v = Track::new(TrackKind::Video);
    let mut c1 = clip(&m[0], MediaKind::Video, 0.0, 2.5, 1.0);
    c1.transform.scale = 0.6;
    c1.keyframes.set(Prop::X, Time::ZERO, -40.0);
    c1.keyframes.set(Prop::X, s(2.5), 40.0);
    c1.fade_in = s(0.3);
    let mut c2 = clip(&m[1], MediaKind::Video, 2.5, 3.0, 5.0);
    c2.transition_out = Some(TransitionRef { id: "fade".into(), duration: s(1.0) });
    let mut c3 = clip(&m[0], MediaKind::Video, 5.5, 2.5, 20.0);
    c3.fade_out = s(0.4);
    let mut c4 = clip(&m[1], MediaKind::Video, 8.0, 3.5, 10.0);
    c4.fx.speed_curve = Some("montage".into());
    v.clips.extend([c1, c2, c3, c4]);
    let mut tx = Track::new(TrackKind::Text);
    let mut t = Clip::new(ClipContent::Text(TextStyle { text: "Naht".into(), size: 20.0, ..Default::default() }), s(4.0), s(4.0));
    t.anim.intro = Some(AnimRef { id: "slide-left".into(), duration: s(1.5) });
    t.anim.outro = Some(AnimRef { id: "fade".into(), duration: s(1.0) });
    tx.clips.push(t);
    let mut fx = Track::new(TrackKind::Effect);
    fx.clips.push(Clip::new(ClipContent::Effect(FxRef::new("shake")), s(3.0), s(3.0)));
    let mut au = Track::new(TrackKind::Audio);
    let mut mu = clip(&m[3], MediaKind::Audio, 0.5, 11.0, 2.0);
    mu.keyframes.set(Prop::Volume, s(2.0), 0.0);
    mu.keyframes.set(Prop::Volume, s(3.0), -12.0);
    mu.keyframes.set(Prop::Volume, s(6.0), -12.0);
    mu.keyframes.set(Prop::Volume, s(7.0), 0.0);
    mu.fade_in = s(0.5);
    mu.fade_out = s(1.0);
    au.clips.push(mu);
    p.timeline.tracks.extend([bg, v, tx, fx, au]);
    p
}

/// Exportiert einmal im Durchgang und einmal in Abschnitten (höchstens `max` Eingänge).
fn both(p: &Project, s: &ExportSettings, dir: &Path, name: &str, max: usize) -> (PathBuf, PathBuf, usize) {
    let ext = s.format.extension();
    let (one, cut) = (dir.join(format!("{name}_eins.{ext}")), dir.join(format!("{name}_teile.{ext}")));
    let job = build(p, s, &no_fonts, &one).unwrap();
    assert_eq!(job.sections(), 0, "kleines Projekt: ein Durchgang");
    run_job(&job);
    let job = cl_export::build_limited(p, s, &no_fonts, &cut, max).unwrap();
    let n = job.sections();
    assert!(n >= 2, "{name}: {n} Abschnitte");
    run_job(&job);
    (one, cut, n)
}

fn wav_samples(f: &Path) -> Vec<i16> {
    decode(f, "0:a", &["-f", "s16le"]).chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
}

/// Ton: in Abschnitten sample-gleich mit dem Durchgang (Begrenzer und Lautheit über das Ganze).
#[test]
fn sections_audio_equals_one_pass() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = rich_project(dir.path());
    for loud in [None, Some(-16.0)] {
        let mut s = ExportSettings::from_project(&p);
        s.format = cl_export::Format::Wav;
        s.loudness = loud;
        let (one, cut, n) = both(&p, &s, dir.path(), &format!("ton{}", loud.is_some()), 2);
        let (a, b) = (wav_samples(&one), wav_samples(&cut));
        assert_eq!(a.len(), b.len(), "Länge in Samples ({n} Abschnitte)");
        if let Some(i) = a.iter().zip(&b).position(|(x, y)| (*x as i32 - *y as i32).abs() > 1) {
            panic!("erste Abweichung bei Sample {} ({} s): {:?} / {:?}", i / 2, i as f64 / 96_000.0, &a[i..(i + 8).min(a.len())], &b[i..(i + 8).min(b.len())]);
        }
        let worst = a.iter().zip(&b).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap();
        assert!(worst <= 1, "Lautheit {loud:?}: größte Abweichung {worst} (16 bit)");
        assert!(a.iter().any(|x| x.abs() > 1000), "Ton hörbar");
    }
}

/// Bild: ProRes (je Bild unabhängig kodiert) in Abschnitten bildgleich mit dem Durchgang.
#[test]
fn sections_video_equals_one_pass_prores() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = rich_project(dir.path());
    let mut s = ExportSettings::from_project(&p);
    s.format = cl_export::Format::Mov;
    s.codec = cl_export::Codec::ProRes;
    let (one, cut, _) = both(&p, &s, dir.path(), "prores", 3);
    let (a, b) = (rgb(&one), rgb(&cut));
    let fs = 128 * 72 * 3;
    assert_eq!(a.len() / fs, 360, "Bilder im Durchgang");
    assert_eq!(a.len(), b.len(), "gleich viele Bilder");
    for (k, (x, y)) in a.chunks(fs).zip(b.chunks(fs)).enumerate() {
        let d = mean_diff(x, y);
        assert!(d < 0.01, "Bild {k}: mittlere Abweichung {d}");
    }
    let (wa, wb) = (wav_samples(&one), wav_samples(&cut));
    // Abschnitte: Ton genau 12 s. Der Durchgang endet in MOV gelegentlich schon mit dem letzten
    // Clip (11,5 s, bekannt aus dem alten Weg) – verglichen wird die gemeinsame Länge.
    assert_eq!(wb.len(), 12 * 48_000 * 2, "Ton in Abschnitten: volle Länge");
    assert!(wa.len() >= 11 * 48_000 * 2);
    let worst = wa.iter().zip(&wb).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap();
    assert!(worst <= 1, "PCM-Ton: größte Abweichung {worst}");
}

/// H.264 + AAC: Bildzahl, monotone Zeitstempel, Dauer und A/V-Versatz, Bild nahe am Durchgang,
/// Ton (einmal kodiert) gleich; dazu Kapitel und Fast-Start.
#[test]
fn sections_h264_aac_seams() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut p = rich_project(dir.path());
    p.timeline.markers.push(Marker { id: 1, at: Time::from_secs_f64(5.0), duration: Time::ZERO, name: "Mitte".into(), note: String::new(), color: 0 });
    let s = ExportSettings::from_project(&p);
    let (one, cut, n) = both(&p, &s, dir.path(), "h264", 3);
    let (ta, tb) = (frame_times(&one), frame_times(&cut));
    assert_eq!(ta.len(), 360, "Bilder im Durchgang");
    assert_eq!(tb.len(), ta.len(), "Bilder in {n} Abschnitten");
    for w in tb.windows(2) {
        assert!(w[1] > w[0] && w[1] - w[0] < 1.5 / 30.0, "Zeitstempel {w:?}");
    }
    let (va, aa) = (stream_secs(&cut, "v:0"), stream_secs(&cut, "a:0"));
    assert!((va - 12.0).abs() < 1.0 / 30.0, "Bild {va}");
    assert!((va - aa).abs() < 1.0 / 30.0, "A/V-Versatz {va} / {aa}");
    let worst = worst_frame(&rgb(&one), &rgb(&cut));
    assert!(worst < 4.0, "größte mittlere Bildabweichung {worst}");
    // AAC einmal über das Ganze kodiert: gleich wie im Durchgang (erste 10 s; der Durchgang endet
    // gelegentlich schon mit dem letzten Clip, siehe ProRes-Test)
    let (x, y) = (decode(&one, "0:a", &["-f", "f32le"]), decode(&cut, "0:a", &["-f", "f32le"]));
    let n = 10 * 48_000 * 8;
    assert!(x.len() >= n && y.len() >= n);
    assert!(x[..n] == y[..n], "AAC einmal über das Ganze kodiert");
    let o = cl_media::ffprobe_command().unwrap().args(["-v", "error", "-show_chapters", "-of", "csv=p=0"]).arg(&cut).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout).lines().count(), 2, "Kapitel");
    let head = std::fs::read(&cut).unwrap();
    let moov = head.windows(4).position(|w| w == b"moov").unwrap();
    let mdat = head.windows(4).position(|w| w == b"mdat").unwrap();
    assert!(moov < mdat, "Fast-Start");
}

/// Weitere Formate: WebM/VP9 mit Alpha, GIF, nur Ton (M4A) – wie im Durchgang.
#[test]
fn sections_other_formats() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = rich_project(dir.path());
    let mut s = ExportSettings::from_project(&p);
    s.format = cl_export::Format::WebM;
    s.codec = cl_export::Codec::Vp9;
    s.transparent = true;
    // ohne Hintergrund: um den verkleinerten ersten Clip bleibt es durchsichtig
    let mut q = p.clone();
    q.timeline.tracks[0].hidden = true;
    let (one, cut, _) = both(&q, &s, dir.path(), "webm", 3);
    assert_eq!(frame_times(&cut).len(), frame_times(&one).len(), "WebM: Bilder");
    // Alphakanal bleibt beim Zusammenfügen erhalten (libvpx liest ihn)
    let ra = cl_media::ffmpeg_command()
        .unwrap()
        .args(["-v", "error", "-c:v", "libvpx-vp9", "-ss", "1", "-i"])
        .arg(&cut)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .output()
        .unwrap();
    assert!(ra.stdout.chunks_exact(4).any(|px| px[3] < 128), "transparente Pixel nach dem Zusammenfügen");
    // mit Alpha vergleichen (ohne libvpx bliebe die Farbe unter durchsichtigen Stellen beliebig)
    let rgba = |f: &Path| {
        let o = cl_media::ffmpeg_command().unwrap().args(["-v", "error", "-c:v", "libvpx-vp9", "-i"]).arg(f).args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"]).output().unwrap();
        o.stdout.chunks_exact(4).flat_map(|px| { let a = px[3] as u32; [px[0] as u32 * a / 255, px[1] as u32 * a / 255, px[2] as u32 * a / 255, a].map(|v| v as u8) }).collect::<Vec<u8>>()
    };
    let (a, b) = (rgba(&one), rgba(&cut));
    assert_eq!(a.len(), b.len());
    let fs = 128 * 72 * 4;
    let d: Vec<f64> = a.chunks(fs).zip(b.chunks(fs)).map(|(x, y)| mean_diff(x, y)).collect();
    let worst = d.iter().cloned().fold(0.0, f64::max);
    assert!(worst < 6.0, "WebM: größte mittlere Bildabweichung {worst}: {:?}", d.iter().enumerate().filter(|x| *x.1 > 6.0).map(|x| x.0).collect::<Vec<_>>());

    let mut g = ExportSettings::from_project(&p);
    g.format = cl_export::Format::Gif;
    let (one, cut, _) = both(&p, &g, dir.path(), "gif", 3);
    let (a, b) = (rgb(&one), rgb(&cut));
    assert_eq!(a.len(), b.len(), "GIF: Bilder");
    assert!(a == b, "GIF: Palette und Bilder wie im Durchgang (Abweichung {})", mean_diff(&a, &b));

    let mut m = ExportSettings::from_project(&p);
    m.format = cl_export::Format::M4a;
    let (one, cut, _) = both(&p, &m, dir.path(), "m4a", 3);
    assert_eq!(decode(&one, "0:a", &["-f", "f32le"]), decode(&cut, "0:a", &["-f", "f32le"]), "M4A");
}

/// Abbrechen mitten in den Abschnitten: kein Ziel, kein `.part`, Arbeitsordner weg.
#[test]
fn cancel_during_sections_leaves_nothing() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = rich_project(dir.path());
    let out = dir.path().join("abbruch.mp4");
    let job = cl_export::build_limited(&p, &ExportSettings::from_project(&p), &no_fonts, &out, 3).unwrap();
    assert!(job.sections() >= 3);
    let work = job.workdir.path().to_path_buf();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    let r = run(&job, cancel, move |x| {
        if x > 0.4 {
            c2.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
    assert!(matches!(r, Err(cl_export::ExportError::Cancelled)), "{:?}", r.err());
    assert!(!out.exists() && !job.part.exists());
    assert!(
        std::fs::read_dir(&work).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("sec_")),
        "Zwischendateien liegen im Arbeitsordner"
    );
    drop(job);
    assert!(!work.exists(), "Arbeitsordner aufgeräumt");
}

/// Fortschritt über alle Abschnitte stetig steigend bis 1; ein Fehler nennt den Zeitbereich.
#[test]
fn progress_is_monotonic_and_errors_name_the_section() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut p = rich_project(dir.path());
    let job = cl_export::build_limited(&p, &ExportSettings::from_project(&p), &no_fonts, &dir.path().join("f.mp4"), 3).unwrap();
    let mut seen = Vec::new();
    run(&job, Arc::new(AtomicBool::new(false)), |x| seen.push(x)).unwrap();
    assert!(seen.windows(2).all(|w| w[1] >= w[0] - 1e-6), "monoton");
    assert!(seen.iter().any(|x| *x > 0.2 && *x < 0.8), "Zwischenstände");
    assert!((seen.last().unwrap() - 1.0).abs() < 1e-6);
    // Datei nach dem Planen kaputt: Fehler mit Zeitbereich, keine halbe Zieldatei
    let bad = dir.path().join("kaputt.mp4");
    std::fs::write(&bad, b"kein Video").unwrap();
    let ClipContent::Media { media, .. } = p.timeline.tracks[1].clips[2].content.clone() else { panic!() };
    p.media.iter_mut().find(|m| m.id == media).unwrap().path = bad;
    let out = dir.path().join("kaputt_out.mp4");
    let job = cl_export::build_limited(&p, &ExportSettings::from_project(&p), &no_fonts, &out, 3).unwrap();
    let e = run(&job, Arc::new(AtomicBool::new(false)), |_| {}).unwrap_err();
    let t = e.to_string();
    assert!(t.starts_with("Abschnitt ") && t.contains('–'), "{t}");
    assert!(!out.exists() && !job.part.exists());
    // fürs Protokoll: der gescheiterte Aufruf des Abschnitts, nicht der nie gestartete Endlauf
    let failed = e.failed_args().expect("Abschnitt");
    assert!(job.section_args().iter().any(|a| a == failed) && failed != job.args.as_slice(), "{failed:?}");
    assert!(failed.iter().any(|a| a.contains("kaputt.mp4")));
}

/// Ein Abschnitt des großen Projekts zum Nachmessen (Bereich 0–50 s).
#[test]
#[ignore]
fn profile_one_section() {
    let root = tempfile::tempdir().unwrap();
    let keep = std::env::var("CL_DUMP").map(PathBuf::from).unwrap_or_else(|_| root.path().to_path_buf());
    let dir = deep_dir(&keep, 130);
    let m = media(&dir);
    let p = big_project(&m, 2500, 26.0 * 60.0);
    let mut s = ExportSettings::from_project(&p);
    s.range = Some((Time::ZERO, Time::from_secs_f64(50.0)));
    let t0 = Instant::now();
    let max = std::env::var("CL_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(100);
    let job = cl_export::build_limited(&p, &s, &no_fonts, &root.path().join("a.mp4"), max).unwrap();
    println!("Abschnitte: {}, build {:?}", job.sections(), t0.elapsed());
    if let Ok(d) = std::env::var("CL_DUMP") {
        for e in std::fs::read_dir(job.workdir.path()).unwrap() {
            let e = e.unwrap();
            std::fs::copy(e.path(), Path::new(&d).join(e.file_name())).unwrap();
        }
        std::fs::write(Path::new(&d).join("args.txt"), job.section_args()[0].join("
")).unwrap();
        return;
    }
    let t0 = Instant::now();
    let mut first = None;
    run(&job, Arc::new(AtomicBool::new(false)), |x| {
        if x > 0.0 && first.is_none() {
            first = Some(t0.elapsed());
        }
    })
    .unwrap();
    println!("erster Fortschritt {:?}, gesamt {:?}", first, t0.elapsed());
}

/// Grenze mitten in einem Effekt-Clip und einem Übergang, ohne Hintergrund (auch mit Alpha):
/// ProRes bildgleich mit dem Durchgang. Vorher lief die Effektzeit im Bereichs-Export ab 0 statt
/// weiter (Wackeln usw. sprang an der Grenze).
#[test]
fn boundary_inside_effect_and_transition() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut q = rich_project(dir.path());
    q.timeline.tracks[0].hidden = true;
    for alpha in [false, true] {
        let mut s = ExportSettings::from_project(&q);
        s.format = cl_export::Format::Mov;
        s.codec = cl_export::Codec::ProRes;
        s.transparent = alpha;
        let (one, cut, _) = both(&q, &s, dir.path(), &format!("naht{alpha}"), 3);
        let fs = 128 * 72 * 3;
        let (a, b) = (rgb(&one), rgb(&cut));
        assert_eq!(a.len(), b.len());
        let bad: Vec<usize> = a.chunks(fs).zip(b.chunks(fs)).enumerate().filter(|(_, (x, y))| mean_diff(x, y) > 0.01).map(|x| x.0).collect();
        assert!(bad.is_empty(), "Alpha {alpha}: abweichende Bilder {bad:?}");
    }
}

/// Durchgang und Abschnitte je 6-mal parallel (Last): der Ton ist immer genau 12 s lang. Vorher
/// endete der Durchgang unter Last gelegentlich schon mit dem letzten Clip (11,5 s).
#[test]
fn audio_length_under_load() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = rich_project(dir.path());
    let mut s = ExportSettings::from_project(&p);
    s.format = cl_export::Format::Wav;
    let lens: Vec<(usize, usize)> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..6)
            .map(|k| {
                let (p, s, d) = (&p, &s, dir.path());
                sc.spawn(move || {
                    let (a, b) = (d.join(format!("l{k}_eins.wav")), d.join(format!("l{k}_teile.wav")));
                    run_job(&build(p, s, &no_fonts, &a).unwrap());
                    run_job(&cl_export::build_limited(p, s, &no_fonts, &b, 2).unwrap());
                    (wav_samples(&a).len() / 2, wav_samples(&b).len() / 2)
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(lens.iter().all(|&(a, b)| a == 576_000 && b == 576_000), "Samples (Durchgang, Abschnitte): {lens:?}");
}

/// Diashow: `n` Bild-Clips (je `len` s) mit Überblendungen zwischen allen – eine einzige
/// Übergangsgruppe mit `n` Eingängen.
fn slideshow(img: &Path, n: usize, len: f64) -> Project {
    let m = MediaRef {
        id: MediaId::new(),
        path: img.to_path_buf(),
        kind: MediaKind::Image,
        duration: Time::ZERO,
        width: 32,
        height: 18,
        fps: None,
        has_audio: false,
        audio_streams: 0,
        video_codec: None,
        audio_codec: None,
    };
    let mut p = Project::new("Diashow");
    p.settings.width = 32;
    p.settings.height = 18;
    p.settings.fps = Rational::new(10, 1);
    p.media.push(m.clone());
    let mut t = Track::new(TrackKind::Video);
    for i in 0..n {
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Image }, Time::from_secs_f64(i as f64 * len), Time::from_secs_f64(len));
        if i + 1 < n {
            c.transition_out = Some(TransitionRef { id: "fade".into(), duration: Time::from_secs_f64(len / 3.0) });
        }
        t.clips.push(c);
    }
    p.timeline.tracks.push(t);
    p
}

/// 160 Fotos mit Übergängen (mehr Eingänge als ein Abschnitt fasst, Kette unteilbar): ein
/// Durchgang wie in 0.11.1 statt „Zu viele gleichzeitige Clips“.
#[test]
fn long_transition_chain_builds_as_one_pass() {
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("foto.png");
    std::fs::write(&img, b"x").unwrap();
    let p = slideshow(&img, 160, 3.0);
    let job = build(&p, &ExportSettings::from_project(&p), &no_fonts, &dir.path().join("d.mp4")).unwrap();
    assert_eq!(job.sections(), 0);
    assert_eq!(job.args.iter().filter(|a| *a == "-i").count(), 160);
}

/// Kette über der Eingangs-Grenze (hier 4) exportiert mit echtem FFmpeg vollständig.
#[test]
fn long_transition_chain_exports() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("foto.png");
    ffmpeg(&["-f", "lavfi", "-i", "testsrc2=s=32x18", "-frames:v", "1"], &img);
    let p = slideshow(&img, 6, 0.6);
    let out = dir.path().join("kette.mp4");
    let job = cl_export::build_limited(&p, &ExportSettings::from_project(&p), &no_fonts, &out, 4).unwrap();
    assert_eq!(job.sections(), 0);
    run_job(&job);
    assert_eq!(frame_times(&out).len(), 36);
}

/// Langer Hintergrund mit „Zoom-Puls“ (`zoompan` vergibt eigene Zeitstempel ab 0): in Abschnitten
/// bildgleich mit dem Durchgang. Vorher sprang der Clip per `-ss` nahe an den Abschnitt und lief
/// danach um den Sprung zu früh.
#[test]
fn zoom_pulse_on_long_clip_across_sections() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut q = rich_project(dir.path());
    q.timeline.tracks[0].clips[0].fx.effects.push(FxRef::new("zoom-pulse"));
    let mut s = ExportSettings::from_project(&q);
    s.format = cl_export::Format::Mov;
    s.codec = cl_export::Codec::ProRes;
    let (one, cut, _) = both(&q, &s, dir.path(), "zoompuls", 3);
    let fs = 128 * 72 * 3;
    let (a, b) = (rgb(&one), rgb(&cut));
    assert_eq!(a.len(), b.len());
    let bad: Vec<usize> = a.chunks(fs).zip(b.chunks(fs)).enumerate().filter(|(_, (x, y))| mean_diff(x, y) > 0.01).map(|x| x.0).collect();
    assert!(bad.is_empty(), "abweichende Bilder {bad:?}");
}

/// Hardware-Encoder, die hier starten (NVENC und Media Foundation, H.264/HEVC): Abschnitte per `-c:v copy` zusammengefügt
/// ergeben alle Bilder, monotone Zeitstempel und decodieren ohne Fehler.
#[test]
fn sections_hardware_encoders_concat() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let hw: Vec<_> = cl_export::available_encoders().into_iter().filter(|e| *e != cl_export::Encoder::Software).collect();
    if hw.is_empty() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = rich_project(dir.path());
    for e in hw {
        for codec in [cl_export::Codec::H264, cl_export::Codec::Hevc] {
            let mut s = ExportSettings::from_project(&p);
            (s.encoder, s.codec) = (e, codec);
            let out = dir.path().join(format!("hw_{e:?}_{codec:?}.mp4"));
            let job = cl_export::build_limited(&p, &s, &no_fonts, &out, 3).unwrap();
            assert!(job.sections() >= 3);
            run_job(&job);
            let t = frame_times(&out);
            assert_eq!(t.len(), 360, "{e:?} {codec:?}: Bilder");
            assert!(t.windows(2).all(|w| w[1] > w[0]), "{e:?} {codec:?}: Zeitstempel");
            let o = cl_media::ffmpeg_command().unwrap().args(["-v", "error", "-nostdin", "-i"]).arg(&out).args(["-f", "null", "-"]).output().unwrap();
            assert!(o.status.success() && o.stderr.is_empty(), "{e:?} {codec:?}: {}", String::from_utf8_lossy(&o.stderr));
        }
    }
}

/// QSV, AMF und AV1 über NVENC (Zusammenfügen ungeprüft): Abschnitte verlustfrei, der Endlauf
/// kodiert einmal mit dem gewählten Encoder statt `-c:v copy`.
#[test]
fn unproven_hardware_encoders_encode_once() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = rich_project(dir.path());
    use cl_export::{Codec, Encoder};
    for (e, codec, name, safe) in [
        (Encoder::Qsv, Codec::H264, "h264_qsv", false),
        (Encoder::Amf, Codec::Hevc, "hevc_amf", false),
        (Encoder::Nvenc, Codec::Av1, "av1_nvenc", false),
        (Encoder::Nvenc, Codec::Hevc, "hevc_nvenc", true),
        // geprüft in `sections_hardware_encoders_concat`
        (Encoder::Mf, Codec::H264, "h264_mf", true),
        (Encoder::Mf, Codec::Hevc, "hevc_mf", true),
    ] {
        let mut s = ExportSettings::from_project(&p);
        (s.encoder, s.codec) = (e, codec);
        let job = cl_export::build_limited(&p, &s, &no_fonts, &dir.path().join("x.mp4"), 3).unwrap();
        let sec = &job.section_args()[0];
        assert_eq!(sec.iter().any(|a| a == "ffv1"), !safe, "{name}: Abschnitte");
        assert_eq!(job.args.iter().any(|a| a == name), !safe, "{name}: Endlauf kodiert");
        assert_eq!(job.args.iter().any(|a| a == "copy"), safe, "{name}: copy");
    }
}

/// Media Foundation (wo `h264_mf` mit Hardware startet): Export H.264 und HEVC mit allen Bildern,
/// richtiger Dauer und dem gewählten Codec.
#[test]
fn media_foundation_export_h264_and_hevc() {
    if !cl_media::ffmpeg_available() || !cl_export::available_encoders().contains(&cl_export::Encoder::Mf) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = rich_project(dir.path());
    for (codec, name) in [(cl_export::Codec::H264, "h264"), (cl_export::Codec::Hevc, "hevc")] {
        let mut s = ExportSettings::from_project(&p);
        (s.encoder, s.codec, s.width, s.height) = (cl_export::Encoder::Mf, codec, 640, 360);
        let out = dir.path().join(format!("mf_{name}.mp4"));
        let job = build(&p, &s, &no_fonts, &out).unwrap();
        assert!(job.args.iter().any(|a| *a == format!("{name}_mf")), "{:?}", job.args);
        run_job(&job);
        let t = frame_times(&out);
        assert_eq!(t.len(), 360, "{name}: Bilder");
        assert!(t.windows(2).all(|w| w[1] > w[0]), "{name}: Zeitstempel");
        let secs = stream_secs(&out, "v:0");
        assert!((secs - 12.0).abs() < 0.05, "{name}: Dauer {secs}");
        let m = cl_media::probe(&out).unwrap();
        assert_eq!(m.video_codec.as_deref(), Some(name));
        assert_eq!((m.width, m.height), (640, 360));
    }
}

/// Sprung an den Abschnitt (`lead_skip`) bei Projekten mit 29,97 und 23,976 fps und Quellen mit
/// gleicher oder anderer Bildrate: ProRes bildgleich mit dem Durchgang. Vorher setzte `setpts` den
/// Versatz abgeschnitten statt gerundet (Clips ein Bild versetzt), und Sprünge um ganze Sekunden
/// verfehlen bei 29,97 fps ab etwa 17 s die Quellbilder.
#[test]
fn lead_skip_with_ntsc_project_and_other_source_rate() {
    if !cl_media::ffmpeg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = |r: &str| {
        let f = dir.path().join(format!("e_{}.mp4", r.replace('/', "_")));
        if !f.is_file() {
            ffmpeg(&["-f", "lavfi", "-i", &format!("testsrc2=s=96x54:r={r}:d=60"), "-c:v", "libx264", "-preset", "ultrafast", "-g", "25", "-pix_fmt", "yuv420p"], &f);
        }
        cl_media::probe(&f).unwrap()
    };
    let s = Time::from_secs_f64;
    // (Projekt, Quelle; None = 60 fps, Bereich ab 25 s mit langem Hintergrund), springt?
    let ntsc = Rational::new(30000, 1001);
    for (fps, rate, long, jumps) in [
        (ntsc, None, false, false),
        (Rational::new(24000, 1001), Some("30000/1001"), false, true),
        (ntsc, Some("24000/1001"), false, true),
        (ntsc, Some("30000/1001"), true, true),
    ] {
        let mut q = rich_project(dir.path());
        q.settings.fps = fps;
        q.timeline.tracks[1].hidden = true;
        if let Some(r) = rate {
            let m = src(r);
            let ClipContent::Media { media, .. } = &mut q.timeline.tracks[0].clips[0].content else { panic!() };
            *media = m.id;
            q.media.push(m);
        }
        let mut st = ExportSettings::from_project(&q);
        if long {
            // Hintergrund 40 s, Bereich 25–28 s, kurze Tonclips erzwingen Grenzen bei 26 und 27 s
            q.timeline.tracks[0].clips[0].duration = s(40.0);
            let mu = q.timeline.tracks[4].clips[0].clone();
            for k in 0..3 {
                let mut c = mu.clone();
                c.id = ClipId::new();
                (c.start, c.duration, c.fade_in, c.fade_out) = (s(25.0 + k as f64), s(1.0), Time::ZERO, Time::ZERO);
                c.keyframes = Default::default();
                q.timeline.tracks[4].clips.push(c);
            }
            st = ExportSettings::from_project(&q);
            st.range = Some((s(25.0), s(28.0)));
        }
        st.format = cl_export::Format::Mov;
        st.codec = cl_export::Codec::ProRes;
        let name = format!("ntsc{}_{}{}", fps.num, rate.unwrap_or("60").replace('/', "_"), if long { "_lang" } else { "" });
        let (one, cut, n) = both(&q, &st, dir.path(), &name, if long { 2 } else { 3 });
        let job = cl_export::build_limited(&q, &st, &no_fonts, &cut, if long { 2 } else { 3 }).unwrap();
        let skipped = (0..n).any(|k| std::fs::read_to_string(job.workdir.path().join(format!("graph_{k}.txt"))).unwrap().contains("start_time="));
        assert_eq!(skipped, jumps, "{name}: Sprung");
        let fs = 128 * 72 * 3;
        let (a, b) = (rgb(&one), rgb(&cut));
        assert_eq!(a.len(), b.len(), "{name}: Bilder");
        let bad: Vec<usize> = a.chunks(fs).zip(b.chunks(fs)).enumerate().filter(|(_, (x, y))| mean_diff(x, y) > 0.01).map(|x| x.0).collect();
        assert!(bad.is_empty(), "{name} ({n} Abschnitte): abweichende Bilder {bad:?}");
    }
}
