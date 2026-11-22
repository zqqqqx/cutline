//! Export robustness: small random projects (fixed seed) with all clip kinds and
//! combinations, special characters in texts and paths, edge cases and error cases.
//! Rendered projects stay small (≤ 3 s, ≤ 320×180); test media are made with 2 FFmpeg threads.
//! `huge_projects_build` only builds an 8K graph with 500 inputs, it renders nothing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cl_core::*;
use cl_export::{build, run, Codec, ExportError, ExportSettings, Format};

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

struct Media {
    video: MediaRef,
    image: MediaRef,
    gif: MediaRef,
    wav: MediaRef,
}

/// Test media in `dir` (the name may contain special characters).
fn media(dir: &Path) -> Media {
    std::fs::create_dir_all(dir).unwrap();
    let (v, i, g, a) = (dir.join("clip 'ä'.mp4"), dir.join("bild 50% [1].png"), dir.join("anim, 50%.gif"), dir.join("ton;x.wav"));
    ffmpeg(
        &["-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=3", "-f", "lavfi", "-i", "sine=f=440:d=3", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"],
        &v,
    );
    ffmpeg(&["-f", "lavfi", "-i", "color=orange:s=120x90", "-frames:v", "1"], &i);
    ffmpeg(&["-f", "lavfi", "-i", "testsrc2=s=64x48:r=10:d=0.5"], &g);
    ffmpeg(&["-f", "lavfi", "-i", "sine=f=220:d=3", "-ac", "2"], &a);
    let p = |f: &Path| cl_media::probe(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
    Media { video: p(&v), image: p(&i), gif: p(&g), wav: p(&a) }
}

fn no_fonts(_: Option<&str>) -> Option<(PathBuf, u32)> {
    None
}

fn export(p: &Project, s: &ExportSettings, out: &Path) -> cl_export::Result<()> {
    let job = build(p, s, &no_fonts, out)?;
    run(&job, Arc::new(AtomicBool::new(false)), |_| {})
}

/// (duration s, video stream?, audio stream?)
fn streams(f: &Path) -> (f64, bool, bool) {
    let o = cl_media::ffmpeg_command().unwrap().args(["-hide_banner", "-nostdin", "-i"]).arg(f).output().unwrap();
    let e = String::from_utf8_lossy(&o.stderr);
    let d = e
        .split("Duration: ")
        .nth(1)
        .and_then(|r| r.split(',').next())
        .and_then(|t| {
            let v: Vec<f64> = t.trim().split(':').filter_map(|x| x.parse().ok()).collect();
            (v.len() == 3).then(|| v[0] * 3600.0 + v[1] * 60.0 + v[2])
        })
        .unwrap_or(-1.0);
    (d, e.contains("Video:"), e.contains("Audio:"))
}

/// Small deterministic random (xorshift).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn f(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (self.next() % 10_000) as f64 / 10_000.0 * (hi - lo)
    }
    fn chance(&mut self, p: f64) -> bool {
        self.f(0.0, 1.0) < p
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[(self.next() % v.len() as u64) as usize]
    }
}

const SPECIAL: &[&str] = &["O'Brien: 50% [a], b\\c", "Ümläute & Emoji 🎬✨", "{x} ; = # \"z\"", "-->\n\nzwei Absätze", "%{pts} %d %%"];

fn secs(s: f64) -> Time {
    Time::from_secs_f64(s)
}

fn random_project(seed: u64, m: &Media) -> Project {
    let mut r = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
    let mut p = Project::new(&format!("Zufall {seed}"));
    p.settings.width = 320;
    p.settings.height = 180;
    for x in [&m.video, &m.image, &m.gif, &m.wav] {
        p.media.push(x.clone());
    }
    let total = r.f(2.0, 3.0);
    // main track: video clips one after another, some with transitions
    let mut main = Track::new(TrackKind::Video);
    main.main = true;
    let mut t = 0.0;
    while t < total - 0.2 {
        let len = r.f(0.3, 1.2).min(total - t);
        let image = r.chance(0.2);
        let (media, kind) = if image { (m.image.id, MediaKind::Image) } else { (m.video.id, MediaKind::Video) };
        let mut c = Clip::new(ClipContent::Media { media, kind }, secs(t), secs(len));
        if !image {
            c.source_in = secs(r.f(0.0, 1.5));
            match r.next() % 5 {
                0 => c.speed = r.f(0.25, 2.0) as f32,
                1 => c.fx.speed_curve = Some(r.pick(SPEED_CURVES).0.into()),
                2 => c.fx.reverse = true,
                _ => {}
            }
            c.fade_in = secs(r.f(0.0, 2.0));
            c.fade_out = secs(r.f(0.0, 2.0));
        }
        if r.chance(0.5) {
            // also transitions longer than the clip and unknown ids
            let id = if r.chance(0.1) { "gibt-es-nicht'[x];movie=a".to_string() } else { r.pick(cl_fx::TRANSITIONS).0.to_string() };
            c.transition_out = Some(TransitionRef { id, duration: secs(r.f(0.1, 1.5)) });
        }
        if r.chance(0.3) {
            c.fx.effects.push(FxRef::new(r.pick(cl_fx::EFFECTS).id));
        }
        if r.chance(0.3) {
            c.fx.filter = Some(FxRef::new(r.pick(cl_fx::FILTERS).id));
        }
        if r.chance(0.2) {
            c.fx.mask = Some(Mask::new(*r.pick(&[MaskShape::Linear, MaskShape::Mirror, MaskShape::Circle, MaskShape::Rect])));
        }
        if r.chance(0.3) {
            c.fx.flip_h = true;
        }
        if r.chance(0.2) {
            c.fx.adjust.brightness = r.f(-1.0, 1.0) as f32;
            c.fx.adjust.vignette = r.f(0.0, 1.0) as f32;
        }
        main.clips.push(c);
        // sometimes a gap: the transition of the previous clip then points into nothing
        t += len + if r.chance(0.15) { r.f(0.1, 0.4) } else { 0.0 };
    }
    p.timeline.tracks.push(main);
    // overlays: image, animated image, video PiP with keyframes and animations
    for _ in 0..(r.next() % 3) {
        let mut tr = Track::new(TrackKind::Video);
        let (media, kind) = *r.pick(&[(m.image.id, MediaKind::Image), (m.gif.id, MediaKind::Image), (m.video.id, MediaKind::Video)]);
        let a = r.f(0.0, total - 0.5);
        let mut c = Clip::new(ClipContent::Media { media, kind }, secs(a), secs(r.f(0.04, total - a)));
        c.transform = Transform { x: r.f(-200.0, 200.0) as f32, y: r.f(-100.0, 100.0) as f32, scale: r.f(0.1, 2.0) as f32, rotation_deg: r.f(-360.0, 360.0) as f32 };
        c.opacity = r.f(0.0, 1.0) as f32;
        if r.chance(0.5) {
            c.keyframes.set(Prop::X, Time::ZERO, -100.0);
            c.keyframes.set(Prop::X, c.duration, 100.0);
            c.keyframes.set(Prop::Opacity, Time::ZERO, 0.2);
            c.keyframes.set(Prop::Scale, c.duration, 1.5);
        }
        if r.chance(0.5) {
            c.anim.intro = Some(AnimRef { id: r.pick(ANIMATIONS).0.into(), duration: secs(0.3) });
            c.anim.outro = Some(AnimRef { id: r.pick(ANIMATIONS).0.into(), duration: secs(5.0) });
        }
        tr.clips.push(c);
        p.timeline.tracks.push(tr);
    }
    // texts with special characters
    let mut tx = Track::new(TrackKind::Text);
    let mut a = 0.0;
    while a < total - 0.3 {
        let len = r.f(0.2, 1.0).min(total - a);
        let mut st = TextStyle { text: r.pick(SPECIAL).to_string(), size: r.f(8.0, 60.0) as f32, ..Default::default() };
        st.uppercase = r.chance(0.3);
        st.shadow = r.chance(0.5);
        st.background = r.chance(0.3).then_some([0, 0, 0, 128]);
        st.outline = r.chance(0.3).then_some((2.0, [255, 0, 0, 255]));
        st.font_family = r.chance(0.3).then(|| "Gibt's: nicht, [x]".into());
        tx.clips.push(Clip::new(ClipContent::Text(st), secs(a), secs(len)));
        a += len + r.f(0.0, 0.3);
    }
    p.timeline.tracks.push(tx);
    // effect and filter elements
    if r.chance(0.6) {
        let mut e = Track::new(TrackKind::Effect);
        let mut c = Clip::new(ClipContent::Effect(FxRef::new(r.pick(cl_fx::EFFECTS).id)), secs(r.f(0.0, 1.0)), secs(r.f(0.3, 1.5)));
        if r.chance(0.5) {
            c.keyframes.set(Prop::Strength, Time::ZERO, 0.0);
            c.keyframes.set(Prop::Strength, c.duration, 1.0);
        }
        e.clips.push(c);
        p.timeline.tracks.push(e);
    }
    if r.chance(0.6) {
        let mut f = Track::new(TrackKind::Filter);
        f.clips.push(Clip::new(ClipContent::Filter(FxRef::new(r.pick(cl_fx::FILTERS).id)), secs(r.f(0.0, 1.0)), secs(r.f(0.3, 1.5))));
        p.timeline.tracks.push(f);
    }
    // audio: volume keyframes, 0 volume, fades, channels
    let mut au = Track::new(TrackKind::Audio);
    au.muted = r.chance(0.2);
    let mut c = Clip::new(ClipContent::Media { media: m.wav.id, kind: MediaKind::Audio }, secs(r.f(0.0, 0.5)), secs(r.f(0.5, 2.0)));
    c.volume_db = *r.pick(&[0.0f32, -6.0, -200.0, 12.0]);
    if r.chance(0.5) {
        c.keyframes.set(Prop::Volume, Time::ZERO, -30.0);
        c.keyframes.set(Prop::Volume, c.duration, 0.0);
    }
    c.fade_in = secs(r.f(0.0, 3.0));
    c.speed = *r.pick(&[1.0f32, 0.5, 2.0]);
    au.clips.push(c);
    p.timeline.tracks.push(au);
    p.settings.channels = *r.pick(&[Channels::Auto, Channels::Stereo, Channels::Mono]);
    p
}

/// Real exports of small random projects: export succeeds, length right to ±1 frame, streams as
/// expected. Finds graph errors that only appear in combinations.
#[test]
fn random_projects_export() {
    if !cl_media::test_tools() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let m = media(&dir.path().join("Medien 'x' [1], ü 50%"));
    let formats = [
        (Format::Mp4, Codec::H264),
        (Format::WebM, Codec::Vp9),
        (Format::Mov, Codec::ProRes),
        (Format::Mkv, Codec::H264),
        (Format::Gif, Codec::H264),
        (Format::Wav, Codec::H264),
        (Format::M4a, Codec::H264),
    ];
    let n: u64 = std::env::var("CUTLINE_RANDOM_PROJECTS").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
    let mut failed = Vec::new();
    for seed in 0..n {
        let p = random_project(seed, &m);
        let (format, codec) = formats[(seed % formats.len() as u64) as usize];
        let mut s = ExportSettings::from_project(&p);
        s.format = format;
        s.codec = codec;
        s.transparent = seed % 3 == 0;
        let out = dir.path().join(format!("aus {seed}: [ä].{}", format.extension()).replace(':', "_"));
        if let Err(e) = export(&p, &s, &out) {
            failed.push(format!("Seed {seed} ({format:?}): {e}"));
            let _ = std::fs::write(dir.path().join(format!("fail-{seed}.json")), p.to_json());
            continue;
        }
        let (d, v, a) = streams(&out);
        let want = p.duration().as_secs_f64();
        // ±1 frame (GIF: at most 15 frames/s), audio containers round to audio packets
        let tol = match format {
            Format::Gif => 1.0 / 15.0 + 0.01,
            f if f.audio_only() => 0.05,
            _ => 1.0 / 30.0 + 0.03,
        };
        if (d - want).abs() > tol {
            failed.push(format!("seed {seed} ({format:?}): length {d:.3} instead of {want:.3}"));
        }
        let want_v = !format.audio_only();
        let want_a = format.audio_only() || format != Format::Gif;
        if (v, a) != (want_v, want_a) {
            failed.push(format!("seed {seed} ({format:?}): streams video {v}/audio {a}"));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// Edge cases that must not crash and export cleanly.
#[test]
fn edge_case_projects_export() {
    if !cl_media::test_tools() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let m = media(dir.path());
    let base = |w, h| {
        let mut p = Project::new("Rand");
        p.settings.width = w;
        p.settings.height = h;
        p.media = vec![m.video.clone(), m.image.clone(), m.gif.clone(), m.wav.clone()];
        p
    };
    let vclip = |a: f64, len: f64| Clip::new(ClipContent::Media { media: m.video.id, kind: MediaKind::Video }, secs(a), secs(len));
    let with = |p: &mut Project, kind, clips: Vec<Clip>| {
        let mut t = Track::new(kind);
        t.clips = clips;
        p.timeline.tracks.push(t);
    };
    let mut cases: Vec<(&str, Project, f64)> = Vec::new();
    // only a gap before the first clip
    let mut p = base(64, 64);
    with(&mut p, TrackKind::Video, vec![vclip(1.5, 0.5)]);
    cases.push(("gap at the start", p, 2.0));
    // whole project shorter than one frame of the live preview (2 frames/s): used to fail
    let mut p = base(64, 64);
    with(&mut p, TrackKind::Video, vec![vclip(0.0, 0.1)]);
    cases.push(("project 0.1 s", p, 0.1));
    // 1 frame long, 1 tick long
    let mut p = base(64, 64);
    with(&mut p, TrackKind::Video, vec![vclip(0.0, 1.0 / 30.0), vclip(1.0 / 30.0, 0.000001), vclip(0.5, 0.5)]);
    cases.push(("1 frame, 1 tick", p, 1.0));
    // transition longer than both clips, transition at the last clip, transition across a gap
    let mut p = base(64, 64);
    let mut a = vclip(0.0, 0.3);
    a.transition_out = Some(TransitionRef { id: "fade".into(), duration: secs(2.0) });
    let mut b = vclip(0.3, 0.2);
    b.transition_out = Some(TransitionRef { id: "wipeleft".into(), duration: secs(5.0) });
    let mut c = vclip(1.0, 0.5);
    c.transition_out = Some(TransitionRef { id: "dissolve".into(), duration: secs(1.0) });
    with(&mut p, TrackKind::Video, vec![a, b, c]);
    cases.push(("transitions too long / into nothing", p, 1.5));
    // split clip with fades longer than the parts (fades not shortened after split)
    let mut p = base(64, 64);
    let mut l = vclip(0.0, 0.4);
    l.fade_in = secs(3.0);
    l.fade_out = secs(3.0);
    let mut r = l.clone();
    r.id = ClipId::new();
    r.start = secs(0.4);
    r.source_in = secs(0.4);
    with(&mut p, TrackKind::Video, vec![l, r]);
    cases.push(("fades longer than parts", p, 0.8));
    // frame rate 1 and 240
    for (num, w) in [(1u32, 32u32), (240, 32)] {
        let mut p = base(w, 32);
        p.settings.fps = Rational::new(num, 1);
        with(&mut p, TrackKind::Video, vec![vclip(0.0, 2.0)]);
        cases.push((if num == 1 { "1 fps" } else { "240 fps" }, p, 2.0));
    }
    // 16×16 and odd size
    let mut p = base(16, 16);
    with(&mut p, TrackKind::Video, vec![vclip(0.0, 0.5)]);
    with(&mut p, TrackKind::Text, vec![Clip::new(ClipContent::Text(TextStyle { text: "Großer Text".into(), size: 200.0, ..Default::default() }), Time::ZERO, secs(0.5))]);
    cases.push(("16×16", p, 0.5));
    let mut p = base(33, 17);
    with(&mut p, TrackKind::Video, vec![vclip(0.0, 0.5)]);
    cases.push(("33×17", p, 0.5));
    // extreme numbers
    let mut p = base(64, 64);
    let mut c = vclip(0.0, 1.0);
    c.transform = Transform { x: 1e9, y: -1e9, scale: 1e6, rotation_deg: 1e7 };
    c.opacity = -5.0;
    c.volume_db = 1000.0;
    c.speed = 1000.0;
    let mut d = vclip(1.0, 0.5);
    d.speed = 0.001;
    d.transform.scale = 0.0;
    d.volume_db = -1000.0;
    d.keyframes.set(Prop::Scale, Time::ZERO, -5.0);
    d.keyframes.set(Prop::X, secs(0.5), 1e12);
    let mut e = Clip::new(ClipContent::Media { media: m.image.id, kind: MediaKind::Image }, secs(0.2), secs(1.0));
    e.transform.scale = 1e-9;
    e.fx.adjust.brightness = 1e9;
    e.fx.adjust.hue = -1e9;
    with(&mut p, TrackKind::Video, vec![c, d]);
    with(&mut p, TrackKind::Video, vec![e]);
    with(&mut p, TrackKind::Text, vec![Clip::new(ClipContent::Text(TextStyle { text: "x".into(), size: 1e6, line_spacing: -3.0, ..Default::default() }), Time::ZERO, secs(0.5))]);
    cases.push(("extreme numbers", p, 1.5));
    // one by one so an error names the cause
    for (name, vol, speed) in [("volume +1000 dB", 1000.0f32, 1.0f32), ("volume −1000 dB", -1000.0, 1.0), ("speed 1000×", 0.0, 1000.0), ("speed 0.001×", 0.0, 0.001)] {
        let mut p = base(64, 64);
        let mut c = vclip(0.0, 1.0);
        c.volume_db = vol;
        c.speed = speed;
        with(&mut p, TrackKind::Video, vec![c]);
        cases.push((name, p, 1.0));
    }
    type Tweak = fn(&mut Clip);
    let tweaks: [(&'static str, Tweak); 8] = [
        ("x/y 1e9", |c| (c.transform.x, c.transform.y) = (1e9, -1e9)),
        ("size 1e6", |c| c.transform.scale = 1e6),
        ("size 0", |c| c.transform.scale = 0.0),
        ("rotation 1e7", |c| c.transform.rotation_deg = 1e7),
        ("opacity −5", |c| c.opacity = -5.0),
        ("Keyframes 1e12/−5", |c| {
            c.keyframes.set(Prop::Scale, Time::ZERO, -5.0);
            c.keyframes.set(Prop::X, secs(0.5), 1e12);
        }),
        ("brightness 1e9", |c| (c.fx.adjust.brightness, c.fx.adjust.hue) = (1e9, -1e9)),
        ("image size 1e-9", |c| c.transform.scale = 1e-9),
    ];
    for (name, f) in tweaks {
        let mut p = base(64, 64);
        let mut c = vclip(0.0, 1.0);
        f(&mut c);
        with(&mut p, TrackKind::Video, vec![c]);
        cases.push((name, p, 1.0));
    }
    let mut p = base(64, 64);
    with(&mut p, TrackKind::Text, vec![Clip::new(ClipContent::Text(TextStyle { text: "x".into(), size: 1e6, line_spacing: -3.0, ..Default::default() }), Time::ZERO, secs(0.5))]);
    cases.push(("text size 1e6", p, 0.5));
    // zoom 20× (as far as the player lets you drag) on Full HD, long line without wrap
    let mut p = base(1920, 1080);
    let mut c = Clip::new(ClipContent::Media { media: m.image.id, kind: MediaKind::Image }, Time::ZERO, secs(0.1));
    c.transform.scale = 20.0;
    let mut k = c.clone();
    k.id = ClipId::new();
    k.start = secs(0.1);
    k.keyframes.set(Prop::Scale, Time::ZERO, 1.0);
    k.keyframes.set(Prop::Scale, secs(0.1), 20.0);
    with(&mut p, TrackKind::Video, vec![c, k]);
    with(&mut p, TrackKind::Text, vec![Clip::new(ClipContent::Text(TextStyle { text: "W".repeat(3000), auto_wrap: false, size: 100.0, ..Default::default() }), Time::ZERO, secs(0.2))]);
    cases.push(("zoom 20× on 1080p", p, 0.2));
    // empty text, very long text, spaces only
    let mut p = base(64, 64);
    let txt = |s: String, a: f64| Clip::new(ClipContent::Text(TextStyle { text: s, ..Default::default() }), secs(a), secs(0.3));
    with(&mut p, TrackKind::Text, vec![txt(String::new(), 0.0), txt("   \n\n ".into(), 0.3), txt("Sehr lang ".repeat(2000), 0.6)]);
    cases.push(("empty/long texts", p, 0.9));
    // 50 tracks
    let mut p = base(64, 64);
    for i in 0..50 {
        let mut c = Clip::new(ClipContent::Media { media: m.image.id, kind: MediaKind::Image }, secs(i as f64 * 0.02), secs(0.5));
        c.transform.x = i as f32;
        with(&mut p, TrackKind::Video, vec![c]);
    }
    cases.push(("50 tracks", p, 1.48));
    let mut failed = Vec::new();
    for (name, p, want) in cases {
        let out = dir.path().join("rand.mp4");
        let _ = std::fs::remove_file(&out);
        match export(&p, &ExportSettings::from_project(&p), &out) {
            Err(e) => failed.push(format!("{name}: {e}")),
            Ok(()) => {
                let (d, v, _) = streams(&out);
                let fps = p.settings.fps.as_f64();
                if !v || (d - want).abs() > 1.0 / fps + 0.05 {
                    failed.push(format!("{name}: length {d:.3} instead of {want:.3}, frame {v}"));
                }
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// 500 clips and 8K: only build the graph (fast, without panic); don't render 8K.
#[test]
fn huge_projects_build() {
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("i.png");
    std::fs::write(&img, b"x").unwrap();
    let m = MediaRef {
        id: MediaId::new(),
        path: img,
        kind: MediaKind::Image,
        duration: Time::ZERO,
        width: 7680,
        height: 4320,
        fps: None,
        has_audio: false,
        audio_streams: 0,
        video_codec: None,
        audio_codec: None,
        has_alpha: false,
    };
    let mut p = Project::new("Groß");
    p.settings.width = 7680;
    p.settings.height = 4320;
    p.media.push(m.clone());
    let mut t = Track::new(TrackKind::Video);
    for i in 0..500 {
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Image }, secs(i as f64 * 0.1), secs(0.1));
        if i % 2 == 0 {
            c.transition_out = Some(TransitionRef { id: "fade".into(), duration: secs(0.05) });
        }
        t.clips.push(c);
    }
    p.timeline.tracks.push(t);
    let mut tx = Track::new(TrackKind::Text);
    for i in 0..500 {
        tx.clips.push(Clip::new(ClipContent::Text(TextStyle { text: format!("Zeile {i}"), ..Default::default() }), secs(i as f64 * 0.1), secs(0.1)));
    }
    p.timeline.tracks.push(tx);
    let job = build(&p, &ExportSettings::from_project(&p), &no_fonts, &dir.path().join("o.mp4")).unwrap();
    // 500 inputs: in sections, every command line below the Windows limit
    assert!(job.sections() > 1);
    assert!(job.section_args().iter().all(|a| a.iter().map(|x| x.len() + 3).sum::<usize>() < 32_000));
    let g = std::fs::read_to_string(job.workdir.path().join("graph_0.txt")).unwrap();
    assert!(g.contains("7680x4320"));
    assert!((job.duration.as_secs_f64() - 50.0).abs() < 1e-6);
}

/// Error cases: clean error instead of crash, no leftovers.
#[test]
fn failures_are_clean() {
    if !cl_media::test_tools() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let m = media(dir.path());
    let mut p = Project::new("Fehler");
    p.settings.width = 64;
    p.settings.height = 64;
    // empty
    assert!(matches!(build(&p, &ExportSettings::from_project(&p), &no_fonts, &dir.path().join("x.mp4")), Err(ExportError::Empty)));
    // missing media file / unknown media id
    let mut gone = m.video.clone();
    gone.id = MediaId::new();
    gone.path = dir.path().join("weg.mp4");
    p.media.push(gone.clone());
    let mut t = Track::new(TrackKind::Video);
    t.clips.push(Clip::new(ClipContent::Media { media: gone.id, kind: MediaKind::Video }, Time::ZERO, secs(1.0)));
    p.timeline.tracks.push(t);
    let s = ExportSettings::from_project(&p);
    assert!(matches!(build(&p, &s, &no_fonts, &dir.path().join("x.mp4")), Err(ExportError::MissingMedia(_))));
    p.media.clear();
    assert!(matches!(build(&p, &s, &no_fonts, &dir.path().join("x.mp4")), Err(ExportError::MissingMedia(_))));
    // 0-byte media and file replaced/shortened afterwards
    let empty = dir.path().join("leer.mp4");
    std::fs::write(&empty, b"").unwrap();
    gone.path = empty;
    p.media.push(gone);
    let e = export(&p, &s, &dir.path().join("x.mp4")).unwrap_err();
    assert!(matches!(e, ExportError::Ffmpeg(_)), "{e}");
    assert!(!dir.path().join("x.part.mp4").exists());
    let short = dir.path().join("kurz.mp4");
    ffmpeg(&["-f", "lavfi", "-i", "testsrc2=s=32x32:r=30:d=0.3", "-f", "lavfi", "-i", "sine=d=0.3", "-pix_fmt", "yuv420p", "-shortest"], &short);
    p.media[0].path = short;
    export(&p, &s, &dir.path().join("ersetzt.mp4")).expect("replaced, shorter file");
    let (d, ..) = streams(&dir.path().join("ersetzt.mp4"));
    assert!((d - 1.0).abs() < 0.07, "{d}");
    // replaced by a file without audio (project says "with audio"): today a clean FFmpeg error
    // shortcut: streams are not re-probed on export (a probe per media costs time), see report
    let silent = dir.path().join("stumm.mp4");
    ffmpeg(&["-f", "lavfi", "-i", "testsrc2=s=32x32:r=30:d=1", "-pix_fmt", "yuv420p"], &silent);
    p.media[0].path = silent;
    assert!(matches!(export(&p, &s, &dir.path().join("stumm-aus.mp4")), Err(ExportError::Ffmpeg(_))));
    assert!(!dir.path().join("stumm-aus.part.mp4").exists());
    // target not writable (folder missing / target is a folder)
    p.media[0] = m.video.clone();
    p.timeline.tracks[0].clips[0].content = ClipContent::Media { media: m.video.id, kind: MediaKind::Video };
    assert!(export(&p, &s, &dir.path().join("fehlt").join("x.mp4")).is_err());
    let blocked = dir.path().join("ordner.mp4");
    std::fs::create_dir(&blocked).unwrap();
    assert!(export(&p, &s, &blocked).is_err());
    // range: empty or reversed
    for range in [(secs(0.5), secs(0.5)), (secs(0.8), secs(0.2))] {
        let mut s = s.clone();
        s.range = Some(range);
        let r = build(&p, &s, &no_fonts, &dir.path().join("r.mp4"));
        assert!(matches!(r, Err(ExportError::Empty)), "{range:?}: {:?}", r.as_ref().err());
    }
    // cancel in the middle of the export: no target, no .part. Deterministic: progress only comes from an
    // `out_time_us=` line, FFmpeg always writes more lines of that block, and `run_ffmpeg` checks the flag first.
    let mut long = p.clone();
    long.timeline.tracks[0].clips[0].duration = secs(3.0);
    long.timeline.tracks[0].clips[0].fx.effects.push(FxRef::new("vhs"));
    let out = dir.path().join("abbruch.mp4");
    let job = build(&long, &ExportSettings::from_project(&long), &no_fonts, &out).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    let r = run(&job, cancel, move |x| {
        if x > 0.05 {
            c2.store(true, Ordering::Relaxed);
        }
    });
    assert!(matches!(r, Err(ExportError::Cancelled)), "{:?}", r.err());
    assert!(!out.exists() && !job.part.exists());
}
