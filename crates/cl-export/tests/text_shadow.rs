//! Text shadow in the export graph: without a shadow style (`None`) the graph stays exactly as
//! before the adjustable shadow (pinned strings), a set style reaches `drawtext` and the sprite pad.
//! A fake font file makes the text metrics fall back to fixed values, so the strings do not depend
//! on the fonts of the machine.

use cl_core::*;
use cl_export::{build, ExportSettings};

/// Text layer lines of the graph of a project with one text clip; the font copy's name (hash of a temp path) is normalised.
fn graph(st: &TextStyle) -> String {
    graph_lines(st, |l| l.contains("drawtext="))
}

fn graph_lines(st: &TextStyle, keep: impl Fn(&str) -> bool) -> String {
    let dir = tempfile::tempdir().unwrap();
    let font = dir.path().join("schrift.ttf");
    std::fs::write(&font, b"no real font").unwrap();
    let mut p = Project::new("T");
    (p.settings.width, p.settings.height) = (640, 360);
    p.settings.fps = Rational { num: 30, den: 1 };
    let mut t = Track::new(TrackKind::Text);
    t.clips.push(Clip::new(ClipContent::Text(st.clone()), Time::ZERO, Time::from_secs_f64(2.0)));
    p.timeline.tracks.push(t);
    let resolver = move |_: Option<&str>| Some((font.clone(), 0));
    let job = build(&p, &ExportSettings::from_project(&p), &resolver, &dir.path().join("o.mp4")).unwrap();
    let g = std::fs::read_to_string(job.workdir.path().join("graph.txt")).unwrap();
    // only the text layers (sprite size and every drawtext); the rest of the graph is not about text
    let g: String = g.lines().filter(|l| keep(l)).map(|l| format!("{l}\n")).collect();
    let mut out = String::new();
    for (i, part) in g.split("fontfile=").enumerate() {
        if i > 0 {
            out.push_str("fontfile=F");
            out.push_str(&part[part.find(':').unwrap()..]);
        } else {
            out.push_str(part);
        }
    }
    out
}

fn styles() -> Vec<(&'static str, TextStyle)> {
    let base = TextStyle { text: "Hallo Welt".into(), size: 44.0, ..Default::default() };
    let red = Shadow { color: [255, 0, 0, 153], dx: 12.0, dy: -8.0, blur: 0.0 };
    vec![
        ("plain", base.clone()),
        ("no shadow", TextStyle { shadow: false, ..base.clone() }),
        ("bold", TextStyle { bold: true, ..base.clone() }),
        ("outline bold", TextStyle { bold: true, outline: Some((4.0, [0, 0, 0, 255])), ..base.clone() }),
        ("italic two lines", TextStyle { italic: true, text: "Eins\nZwei".into(), ..base.clone() }),
        ("box left", TextStyle { background: Some([0, 0, 0, 170]), align: -1, ..base.clone() }),
        (
            "karaoke",
            TextStyle {
                karaoke: Some(Karaoke { color: [255, 221, 0, 255], fill: true, words: Vec::new() }),
                text: "Eins zwei drei".into(),
                ..base.clone()
            },
        ),
        // stage 1: hard shadow styles (0.17.0)
        ("hard red", TextStyle { shadow_style: Some(red), ..base.clone() }),
        ("hard red bold outline", TextStyle { shadow_style: Some(red), bold: true, outline: Some((4.0, [0, 0, 0, 255])), ..base.clone() }),
        ("hard red italic two lines", TextStyle { shadow_style: Some(red), italic: true, text: "Eins
Zwei".into(), ..base.clone() }),
        (
            "hard red karaoke box",
            TextStyle {
                shadow_style: Some(red),
                background: Some([0, 0, 0, 170]),
                karaoke: Some(Karaoke { color: [255, 221, 0, 255], fill: false, words: Vec::new() }),
                text: "Eins zwei drei".into(),
                ..base.clone()
            },
        ),
    ]
}

/// Graphs captured before the adjustable shadow (`None`) and before the soft shadow (hard styles):
/// neither may change a single byte.
#[test]
fn graphs_without_shadow_style_stay_byte_identical() {
    let want = include_str!("snapshots/text_graphs.txt").replace("\r\n", "\n");
    let got: String = styles().iter().map(|(name, st)| format!("## {name}\n{}\n", graph(st))).collect();
    if want != got {
        for (w, g) in want.lines().zip(got.lines()) {
            assert_eq!(w, g);
        }
        assert_eq!(want, got);
    }
}

/// Sprite size `WxH` of the (first) text layer
fn sprite(g: &str) -> (u32, u32) {
    let s = &g[g.find(":s=").unwrap() + 3..];
    let (w, h) = s[..s.find(':').unwrap()].split_once('x').unwrap();
    (w.parse().unwrap(), h.parse().unwrap())
}

#[test]
fn shadow_style_reaches_drawtext_and_the_pad() {
    let base = TextStyle { text: "Hallo Welt".into(), size: 44.0, bold: true, ..Default::default() };
    let (w0, h0) = sprite(&graph(&base));
    // red, 60 % → 12/−8 (sub-pixel offsets rounded half away from zero)
    let sh = Shadow { color: [255, 0, 0, 153], dx: 11.5, dy: -7.6, blur: 0.0 };
    let g = graph(&TextStyle { shadow_style: Some(sh), ..base.clone() });
    assert_eq!(g.matches(":shadowx=12:shadowy=-8:shadowcolor=0xff0000@0.600").count(), 1, "{g}");
    // bold: two prints, the shadow only under the first (drawn before its text)
    assert_eq!(g.matches("drawtext=").count(), 2, "{g}");
    assert!(g.find("shadowx").unwrap() < g.rfind("drawtext=").unwrap(), "{g}");
    // the pad grows by the larger offset on every side
    assert_eq!(sprite(&g), (w0 + 24, h0 + 24), "{g}");
    // the default look set explicitly: same offset and sprite as without a style
    let g = graph(&TextStyle { shadow_style: Some(Shadow::default()), ..base.clone() });
    assert!(g.contains(":shadowx=3:shadowy=3:shadowcolor=0x000000@0.600"), "{g}");
    assert_eq!(sprite(&g), (w0 + 6, h0 + 6));
    // switch off: no shadow, whatever the style
    let g = graph(&TextStyle { shadow: false, shadow_style: Some(sh), ..base.clone() });
    assert!(!g.contains("shadow"), "{g}");
    assert_eq!(sprite(&g), (w0, h0));
}

/// Soft shadow: its own layer (text in the shadow colour, opaque, outline as border, bold print,
/// offset, blurred, then the opacity) under the text; drawtext's own shadow is gone; the pad grows
/// by the offset and 3 sigma; italic shears text and shadow together, once.
#[test]
fn soft_shadow_builds_its_own_layer() {
    let base = TextStyle { text: "Hallo Welt".into(), size: 44.0, ..Default::default() };
    let soft = Shadow { color: [255, 0, 0, 153], dx: 12.0, dy: -8.0, blur: 6.0 };
    let hard = graph(&TextStyle { shadow_style: Some(Shadow { blur: 0.0, ..soft }), ..base.clone() });
    let g = graph(&TextStyle { shadow_style: Some(soft), ..base.clone() });
    assert!(!g.contains("shadowx"), "{g}");
    let layer = g.lines().find(|l| l.contains("gblur")).unwrap_or_else(|| panic!("{g}"));
    assert!(layer.starts_with("color=c=0xff0000@0.0:"), "{layer}");
    assert!(layer.contains("fontcolor=0xff0000@1.000:x=(w-text_w)/2+12.0:y=") && layer.contains("-8:y_align=baseline"), "{layer}");
    assert!(layer.contains(",gblur=sigma=3.00,colorchannelmixer=aa=0.600["), "{layer}");
    // the text itself in its colour on its own layer, laid over the shadow
    assert!(g.contains("fontcolor=0xffffff@1.000"), "{g}");
    // sigma 3: 9 px more on each side than the hard shadow
    let (w, h) = sprite(&hard);
    assert_eq!(sprite(layer), (w + 18, h + 18), "{g}");
    // bold and outline: the shadow layer has both prints, the first with the outline as border
    let g = graph(&TextStyle { shadow_style: Some(soft), bold: true, outline: Some((4.0, [0, 0, 0, 255])), ..base.clone() });
    let layer = g.lines().find(|l| l.contains("gblur")).unwrap();
    assert_eq!(layer.matches("drawtext=").count(), 2, "{layer}");
    assert!(layer.contains(":borderw=4.0:bordercolor=0xff0000@1.000"), "{layer}");
    // italic: one shear, after text and shadow are put together
    let g = graph_lines(&TextStyle { shadow_style: Some(soft), italic: true, ..base.clone() }, |_| true);
    assert_eq!(g.matches("shear=").count(), 1, "{g}");
    // switched off: no layer
    let g = graph(&TextStyle { shadow: false, shadow_style: Some(soft), ..base.clone() });
    assert!(!g.contains("gblur") && !g.contains("shadow"), "{g}");
}
