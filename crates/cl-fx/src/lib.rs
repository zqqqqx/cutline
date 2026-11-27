//! Cutline's creative library (phase 2).
//!
//! Everything is described as an FFmpeg filter chain. The same chain runs in the preview
//! (single frame and playback stream) and in the export, so the export looks exactly
//! like the player. Every catalog entry is run against FFmpeg in the tests.
//!
//! - [`EFFECTS`]: video effects (retro, motion, light, distortion, style)
//! - [`FILTERS`]: color looks; strength via cross-fade with the original
//! - [`TRANSITIONS`]: transitions (FFmpeg `xfade`)
//! - [`fx_graph`]: complete clip chain from [`ClipFx`] (flip, adjust, HSL, filters,
//!   effects, LUT, mask)
//! - [`text`]: text templates and text effects, [`stickers`]: SVG stickers, [`sfx`]: sound effects

pub mod audio;
pub mod catalog;
pub mod sfx;
pub mod stickers;
pub mod text;
pub mod xfade;

use cl_core::*;

/// Properties of the image that goes into the chain.
#[derive(Debug, Clone, Copy)]
pub struct Src {
    pub w: u32,
    pub h: u32,
    pub has_alpha: bool,
}

/// A catalog entry: `build(strength 0…1, source)` returns a simple filter chain
/// (joined with commas, without labels).
pub struct FxDef {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    /// The chain can map its strength itself; otherwise it is cross-faded with the original.
    pub native_intensity: bool,
    pub build: fn(f32, Src) -> String,
}

impl std::fmt::Debug for FxDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FxDef({})", self.id)
    }
}

fn ev(w: u32) -> u32 {
    (w.max(2)) & !1
}

pub const EFFECT_CATEGORIES: [&str; 5] = ["Basic", "Retro", "Motion", "Light", "Style"];

pub static EFFECTS: &[FxDef] = &[
    // basic
    FxDef {
        id: "blur",
        name: "Blur",
        category: "Basic",
        native_intensity: true,
        build: |i, _| format!("gblur=sigma={:.2}", 1.0 + 14.0 * i),
    },
    FxDef {
        id: "sharpen",
        name: "Sharpen",
        category: "Basic",
        native_intensity: true,
        build: |i, _| format!("unsharp=5:5:{:.2}", 0.4 + 2.0 * i),
    },
    FxDef {
        id: "bw",
        name: "Black & white",
        category: "Basic",
        native_intensity: true,
        build: |i, _| format!("hue=s={:.3}", 1.0 - i),
    },
    FxDef {
        id: "invert",
        name: "Negative",
        category: "Basic",
        native_intensity: false,
        build: |_, _| "negate".into(),
    },
    FxDef {
        id: "pixelate",
        name: "Pixelate",
        category: "Basic",
        native_intensity: true,
        build: |i, s| {
            let f = 4.0 + 36.0 * i;
            let (w, h) = (
                ((s.w as f32 / f).round() as u32).max(2),
                ((s.h as f32 / f).round() as u32).max(2),
            );
            format!(
                "scale={w}:{h}:flags=area,scale={}:{}:flags=neighbor",
                s.w, s.h
            )
        },
    },
    FxDef {
        id: "vignette",
        name: "Vignette",
        category: "Basic",
        native_intensity: true,
        build: |i, _| format!("vignette=angle={:.3}", 0.2 + 0.9 * i),
    },
    FxDef {
        id: "grain",
        name: "Film grain",
        category: "Basic",
        native_intensity: true,
        build: |i, _| format!("noise=alls={:.0}:allf=t", 6.0 + 34.0 * i),
    },
    FxDef {
        id: "edges",
        name: "Edges",
        category: "Basic",
        native_intensity: false,
        build: |_, _| "edgedetect=mode=colormix:high=0.2:low=0.05".into(),
    },
    // retro
    FxDef {
        id: "vhs",
        name: "VHS",
        category: "Retro",
        native_intensity: false,
        // pixel sizes relative to 1080 lines: the preview computes the chain on its smaller
        // image, otherwise color shift and lines would be much stronger there than in the export
        build: |_, s| {
            let k = s.w.min(s.h).max(1) as f32 / 1080.0;
            let shift = (5.0 * k).round() as i32;
            // lines: ¹⁄₁₈ darkening on average; too small for 1 px lines → even
            let lines = if 3.0 * k >= 2.0 {
                let (t, h) = ((k.round() as i32).max(1), (3.0 * k).round() as i32);
                format!("drawgrid=w=0:h={h}:t={t}:c=black@{:.3}", 0.06 * h as f32 / t as f32)
            } else {
                "drawbox=c=black@0.06:t=fill".into()
            };
            format!(
                "rgbashift=rh={}:bh={shift}:gv={},noise=alls=18:allf=t,eq=saturation=1.35:contrast=1.08,gblur=sigma={:.2},{lines}",
                -shift,
                k.round() as i32,
                (0.7 * k).max(0.1)
            )
        },
    },
    FxDef {
        id: "old-film",
        name: "Old film",
        category: "Retro",
        native_intensity: false,
        build: |_, _| {
            "colorchannelmixer=.393:.769:.189:0:.349:.686:.168:0:.272:.534:.131,noise=alls=24:allf=t,vignette=angle=0.9,eq=brightness='0.04*sin(t*37)':eval=frame".into()
        },
    },
    FxDef {
        id: "sepia",
        name: "Sepia",
        category: "Retro",
        native_intensity: false,
        build: |_, _| "colorchannelmixer=.393:.769:.189:0:.349:.686:.168:0:.272:.534:.131".into(),
    },
    FxDef {
        id: "scanlines",
        name: "Scanlines",
        category: "Retro",
        native_intensity: true,
        build: |i, _| format!("drawgrid=w=0:h=4:t=1:c=black@{:.2}", 0.15 + 0.6 * i),
    },
    FxDef {
        id: "posterize",
        name: "Posterize",
        category: "Retro",
        native_intensity: true,
        build: |i, _| {
            let step = (24.0 + 72.0 * i).round();
            format!("lutrgb=r='trunc(val/{step})*{step}':g='trunc(val/{step})*{step}':b='trunc(val/{step})*{step}'")
        },
    },
    FxDef {
        id: "comic",
        name: "Comic",
        category: "Retro",
        native_intensity: false,
        build: |_, _| {
            "lutrgb=r='trunc(val/64)*64':g='trunc(val/64)*64':b='trunc(val/64)*64',unsharp=7:7:2.5,eq=saturation=1.4".into()
        },
    },
    // motion
    FxDef {
        id: "shake",
        name: "Shake",
        category: "Motion",
        native_intensity: true,
        build: |i, s| {
            let a = 0.03 * i;
            let (cw, ch) = (ev((s.w as f32 * 0.9) as u32), ev((s.h as f32 * 0.9) as u32));
            format!("crop={cw}:{ch}:x='(iw-ow)/2+sin(t*47)*iw*{a:.4}':y='(ih-oh)/2+cos(t*39)*ih*{a:.4}',scale={}:{}", s.w, s.h)
        },
    },
    FxDef {
        id: "zoom-pulse",
        name: "Zoom pulse",
        category: "Motion",
        native_intensity: true,
        build: |i, s| {
            // scale by the frame's time and crop centered: frame count and timestamps
            // stay as they are (`zoompan` would assign its own with a fixed frame rate)
            let z = format!("(1+{:.3}*(0.5+0.5*sin(t*6.2832)))", 0.15 * i);
            format!("scale=w='2*ceil(iw*{z}/2)':h='2*ceil(ih*{z}/2)':eval=frame,crop={}:{},setsar=1", s.w, s.h)
        },
    },
    FxDef {
        id: "motion-blur",
        name: "Motion blur",
        category: "Motion",
        native_intensity: false,
        build: |_, _| "tmix=frames=4:weights='1 1 1 1'".into(),
    },
    FxDef {
        id: "echo",
        name: "Trails",
        category: "Motion",
        native_intensity: false,
        build: |_, _| "tmix=frames=8:weights='1 1 1 2 2 3 4 8'".into(),
    },
    FxDef {
        id: "glitch",
        name: "Glitch",
        category: "Motion",
        native_intensity: false,
        build: |_, _| {
            "rgbashift=rh=-8:bh=8:gv=3,hue=h='if(lt(mod(t,1.1),0.12),120,0)',noise=alls=20:allf=t+u"
                .into()
        },
    },
    FxDef {
        id: "rainbow",
        name: "Rainbow",
        category: "Motion",
        native_intensity: true,
        build: |i, _| format!("hue=h='t*{:.0}'", 60.0 + 240.0 * i),
    },
    // light
    FxDef {
        id: "strobe",
        name: "Strobe",
        category: "Light",
        native_intensity: true,
        build: |i, _| {
            format!(
                "eq=brightness='{:.3}*gt(mod(t,0.25),0.125)':eval=frame",
                0.15 + 0.45 * i
            )
        },
    },
    FxDef {
        id: "flash",
        name: "Flash",
        category: "Light",
        native_intensity: true,
        build: |i, _| {
            format!(
                "eq=brightness='max(0,{:.3}*(1-t*3))':eval=frame",
                0.3 + 0.6 * i
            )
        },
    },
    FxDef {
        id: "dreamy",
        name: "Dreamy",
        category: "Light",
        native_intensity: false,
        build: |_, _| {
            "gblur=sigma=3,eq=brightness=0.05:contrast=0.92:saturation=1.15,colortemperature=temperature=5600".into()
        },
    },
    FxDef {
        id: "glow-edges",
        name: "Glow edges",
        category: "Light",
        native_intensity: false,
        build: |_, _| {
            "edgedetect=mode=colormix:high=0.15:low=0.04,eq=saturation=2.2:brightness=0.06,gblur=sigma=1.2".into()
        },
    },
    FxDef {
        id: "warm-leak",
        name: "Light leak",
        category: "Light",
        native_intensity: false,
        build: |_, _| {
            "colorbalance=rh=0.25:gh=0.08:rm=0.12,eq=brightness='0.06+0.05*sin(t*1.7)':eval=frame"
                .into()
        },
    },
    FxDef {
        id: "chroma",
        name: "Color shift",
        category: "Light",
        native_intensity: true,
        build: |i, _| {
            let px = (2.0 + 10.0 * i).round() as i32;
            format!("rgbashift=rh=-{px}:bh={px}")
        },
    },
    // style
    FxDef {
        id: "heat",
        name: "Thermal",
        category: "Style",
        native_intensity: false,
        build: |_, _| "format=gbrp,pseudocolor=preset=heat".into(),
    },
    FxDef {
        id: "night-vision",
        name: "Night vision",
        category: "Style",
        native_intensity: false,
        build: |_, _| {
            "hue=s=0,colorchannelmixer=0.2:0:0:0:0.3:1.2:0.2:0:0.1:0.2:0.1,noise=alls=22:allf=t,vignette=angle=0.8".into()
        },
    },
    FxDef {
        id: "duotone",
        name: "Duotone",
        category: "Style",
        native_intensity: false,
        build: |_, _| "hue=s=0,colorbalance=rs=0.35:gs=-0.1:bs=0.45:rh=0.4:gh=0.1:bh=-0.25".into(),
    },
    FxDef {
        id: "emboss",
        name: "Emboss",
        category: "Style",
        native_intensity: false,
        build: |_, _| {
            "convolution='-2 -1 0 -1 1 1 0 1 2:-2 -1 0 -1 1 1 0 1 2:-2 -1 0 -1 1 1 0 1 2:-2 -1 0 -1 1 1 0 1 2'".into()
        },
    },
    FxDef {
        id: "lens",
        name: "Lens",
        category: "Style",
        native_intensity: true,
        build: |i, _| {
            format!(
                "lenscorrection=k1={:.3}:k2={:.3}",
                -0.15 - 0.35 * i,
                -0.05 - 0.1 * i
            )
        },
    },
    FxDef {
        id: "cinema-bars",
        name: "Letterbox",
        category: "Style",
        native_intensity: true,
        build: |i, _| {
            let f = 0.06 + 0.1 * i;
            format!("drawbox=x=0:y=0:w=iw:h=ih*{f:.3}:color=black:t=fill,drawbox=x=0:y=ih*{:.3}:w=iw:h=ih*{f:.3}:color=black:t=fill", 1.0 - f)
        },
    },
];

pub static FILTERS: &[FxDef] = &[
    FxDef {
        id: "natural",
        name: "Natural",
        category: "Portrait",
        native_intensity: false,
        build: |_, _| "eq=saturation=1.1:contrast=1.05".into(),
    },
    FxDef {
        id: "vivid",
        name: "Vivid",
        category: "Portrait",
        native_intensity: false,
        build: |_, _| "eq=saturation=1.45:contrast=1.1,vibrance=intensity=0.3".into(),
    },
    FxDef {
        id: "pastel",
        name: "Pastel",
        category: "Portrait",
        native_intensity: false,
        build: |_, _| "eq=saturation=0.7:brightness=0.05,curves=all='0/0.1 1/1'".into(),
    },
    FxDef {
        id: "bright",
        name: "Light~bright",
        category: "Portrait",
        native_intensity: false,
        build: |_, _| "curves=preset=lighter".into(),
    },
    FxDef {
        id: "warm",
        name: "Warm",
        category: "Landscape",
        native_intensity: false,
        build: |_, _| "colortemperature=temperature=4600".into(),
    },
    FxDef {
        id: "cool",
        name: "Cool",
        category: "Landscape",
        native_intensity: false,
        build: |_, _| "colortemperature=temperature=9000".into(),
    },
    FxDef {
        id: "golden",
        name: "Golden hour",
        category: "Landscape",
        native_intensity: false,
        build: |_, _| "colortemperature=temperature=3800,eq=saturation=1.2".into(),
    },
    FxDef {
        id: "forest",
        name: "Forest",
        category: "Landscape",
        native_intensity: false,
        build: |_, _| "colorbalance=gm=0.12:rs=-0.05:bs=0.03,eq=saturation=1.1".into(),
    },
    FxDef {
        id: "sunset",
        name: "Sunset",
        category: "Landscape",
        native_intensity: false,
        build: |_, _| "colorbalance=rm=0.18:gm=0.02:bm=-0.12:rh=0.1,eq=saturation=1.15".into(),
    },
    FxDef {
        id: "cinematic",
        name: "Cinema",
        category: "Film",
        native_intensity: false,
        build: |_, _| {
            "colorbalance=rs=-0.12:bs=0.16:rh=0.14:bh=-0.12,curves=preset=medium_contrast,eq=saturation=0.9".into()
        },
    },
    FxDef {
        id: "teal-orange",
        name: "Teal & Orange",
        category: "Film",
        native_intensity: false,
        build: |_, _| "colorbalance=rs=-0.15:gs=0.02:bs=0.18:rh=0.15:gh=0.03:bh=-0.15".into(),
    },
    FxDef {
        id: "vintage",
        name: "Vintage",
        category: "Film",
        native_intensity: false,
        build: |_, _| "curves=preset=vintage".into(),
    },
    FxDef {
        id: "cross",
        name: "Cross process",
        category: "Film",
        native_intensity: false,
        build: |_, _| "curves=preset=cross_process".into(),
    },
    FxDef {
        id: "matte",
        name: "Matte",
        category: "Film",
        native_intensity: false,
        build: |_, _| "curves=all='0/0.12 1/0.92',eq=saturation=0.85".into(),
    },
    FxDef {
        id: "bleach",
        name: "Bleach bypass",
        category: "Film",
        native_intensity: false,
        build: |_, _| "eq=saturation=0.45:contrast=1.3".into(),
    },
    FxDef {
        id: "polaroid",
        name: "Instant",
        category: "Film",
        native_intensity: false,
        build: |_, _| "curves=preset=vintage,eq=saturation=0.9,vignette=angle=0.5".into(),
    },
    FxDef {
        id: "mono",
        name: "Mono",
        category: "Black & white",
        native_intensity: false,
        build: |_, _| "hue=s=0,eq=contrast=1.15".into(),
    },
    FxDef {
        id: "noir",
        name: "Noir",
        category: "Black & white",
        native_intensity: false,
        build: |_, _| "hue=s=0,curves=preset=strong_contrast".into(),
    },
    FxDef {
        id: "silver",
        name: "Silver",
        category: "Black & white",
        native_intensity: false,
        build: |_, _| "hue=s=0,curves=all='0/0.08 0.5/0.55 1/0.95'".into(),
    },
    FxDef {
        id: "moody",
        name: "Moody",
        category: "Mood",
        native_intensity: false,
        build: |_, _| "eq=brightness=-0.05:saturation=0.75,curves=preset=darker".into(),
    },
    FxDef {
        id: "cyber",
        name: "Cyber",
        category: "Mood",
        native_intensity: false,
        build: |_, _| "colorbalance=rs=0.1:bs=0.25:rh=0.2:bh=0.2:gm=-0.1,eq=saturation=1.3".into(),
    },
    FxDef {
        id: "contrast",
        name: "Contrast",
        category: "Mood",
        native_intensity: false,
        build: |_, _| "curves=preset=strong_contrast".into(),
    },
];

pub const FILTER_CATEGORIES: [&str; 5] =
    ["Portrait", "Landscape", "Film", "Black & white", "Mood"];

/// Display name of an effect/filter (built-in or from a package).
pub fn fx_name(id: &str) -> String {
    effect(id)
        .or_else(|| filter(id))
        .map(|d| d.name.to_string())
        .or_else(|| catalog::name_any(id))
        .unwrap_or_else(|| id.to_string())
}

pub fn effect(id: &str) -> Option<&'static FxDef> {
    EFFECTS.iter().find(|e| e.id == id)
}
pub fn filter(id: &str) -> Option<&'static FxDef> {
    FILTERS.iter().find(|e| e.id == id)
}

/// Transitions: (id, name, category). The id is the name of the `xfade` variant.
pub static TRANSITIONS: &[(&str, &str, &str)] = &[
    ("fade", "Cross dissolve", "Basic"),
    ("dissolve", "Dissolve", "Basic"),
    ("fadeblack", "Fade to black", "Basic"),
    ("fadewhite", "Fade to white", "Basic"),
    ("fadegrays", "Fade to gray", "Basic"),
    ("fadefast", "Quick fade", "Basic"),
    ("fadeslow", "Slow fade", "Basic"),
    ("distance", "Distance", "Basic"),
    ("wipeleft", "Wipe left", "Wipe"),
    ("wiperight", "Wipe right", "Wipe"),
    ("wipeup", "Wipe up", "Wipe"),
    ("wipedown", "Wipe down", "Wipe"),
    ("wipetl", "Wipe ↖", "Wipe"),
    ("wipetr", "Wipe ↗", "Wipe"),
    ("wipebl", "Wipe ↙", "Wipe"),
    ("wipebr", "Wipe ↘", "Wipe"),
    ("smoothleft", "Smooth left", "Wipe"),
    ("smoothright", "Smooth right", "Wipe"),
    ("smoothup", "Smooth up", "Wipe"),
    ("smoothdown", "Smooth down", "Wipe"),
    ("slideleft", "Slide left", "Motion"),
    ("slideright", "Slide right", "Motion"),
    ("slideup", "Slide up", "Motion"),
    ("slidedown", "Slide down", "Motion"),
    ("coverleft", "Cover left", "Motion"),
    ("coverright", "Cover right", "Motion"),
    ("coverup", "Cover up", "Motion"),
    ("coverdown", "Cover down", "Motion"),
    ("revealleft", "Reveal left", "Motion"),
    ("revealright", "Reveal right", "Motion"),
    ("revealup", "Reveal up", "Motion"),
    ("revealdown", "Reveal down", "Motion"),
    ("zoomin", "Zoom in~into", "Motion"),
    ("squeezeh", "Squeeze horizontal", "Motion"),
    ("hlwind", "Wind left", "Motion"),
    ("hrwind", "Wind right", "Motion"),
    ("vuwind", "Wind up", "Motion"),
    ("vdwind", "Wind down", "Motion"),
    ("circleopen", "Circle open", "Shape"),
    ("circleclose", "Circle close", "Shape"),
    ("circlecrop", "Circle crop", "Shape"),
    ("rectcrop", "Rectangle crop", "Shape"),
    ("radial", "Radial", "Shape"),
    ("vertopen", "Vertical open", "Shape"),
    ("vertclose", "Vertical close", "Shape"),
    ("horzopen", "Horizontal open", "Shape"),
    ("horzclose", "Horizontal close", "Shape"),
    ("diagtl", "Diagonal ↖", "Shape"),
    ("diagbr", "Diagonal ↘", "Shape"),
    ("diagtr", "Diagonal ↗", "Shape"),
    ("diagbl", "Diagonal ↙", "Shape"),
    ("hlslice", "Slice left", "Effect"),
    ("hrslice", "Slice right", "Effect"),
    ("vuslice", "Slice up", "Effect"),
    ("vdslice", "Slice down", "Effect"),
    ("pixelize", "Pixelate", "Effect"),
    ("hblur", "Smear", "Effect"),
];

pub const TRANSITION_CATEGORIES: [&str; 5] = ["Basic", "Wipe", "Motion", "Shape", "Effect"];

/// Name of a built-in or package transition (built-in: German, the UI translates).
pub fn transition_name(id: &str) -> String {
    TRANSITIONS
        .iter()
        .find(|t| t.0 == id)
        .map(|t| t.1.to_string())
        .or_else(|| catalog::name_any(id))
        .unwrap_or_else(|| "Transition".into())
}

/// Effective length (s) of the transition at the end of `a` to the neighbour `b`: at most as long as either
/// of the two clips. Half of it reaches into each clip, so two transitions on
/// one clip share its length. Preview and export both compute with this.
pub fn transition_secs(a: &Clip, b: &Clip) -> f64 {
    a.transition_out
        .as_ref()
        .map_or(0.0, |t| t.duration.as_secs_f64())
        .min(a.duration.as_secs_f64())
        .min(b.duration.as_secs_f64())
        .max(0.0)
}

/// Segment from (x, y) to (x, y)
type Seg = ((f64, f64), (f64, f64));

/// Piecewise linear function as an FFmpeg expression (variable `var`), points (x, y) sorted.
///
/// Points on a straight line are merged (long constant sections cost nothing),
/// and the case distinction is a balanced tree instead of a chain: FFmpeg aborts on
/// too deeply nested expressions ("Missing ')' or too many args").
pub fn piecewise_expr(points: &[(f64, f64)], var: &str) -> String {
    let slope = |a: (f64, f64), b: (f64, f64)| {
        if (b.0 - a.0).abs() < 1e-9 {
            0.0
        } else {
            (b.1 - a.1) / (b.0 - a.0)
        }
    };
    // remove duplicate x values, merge points on a straight line
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for &p in points {
        if pts
            .last()
            .is_some_and(|l: &(f64, f64)| (p.0 - l.0).abs() < 1e-9)
        {
            *pts.last_mut().expect("not empty") = p;
            continue;
        }
        while pts.len() >= 2 {
            let (a, b) = (pts[pts.len() - 2], pts[pts.len() - 1]);
            let (k1, k2) = (slope(a, b), slope(b, p));
            if (k1 - k2).abs() <= 1e-6 * k1.abs().max(k2.abs()).max(1.0) {
                pts.pop();
            } else {
                break;
            }
        }
        pts.push(p);
    }
    match pts.as_slice() {
        [] => "0".into(),
        [p] => format!("{:.5}", p.1),
        _ => {
            let segs: Vec<Seg> = pts.windows(2).map(|w| (w[0], w[1])).collect();
            fn tree(
                segs: &[Seg],
                var: &str,
                slope: &dyn Fn((f64, f64), (f64, f64)) -> f64,
            ) -> String {
                if let [(a, b)] = segs {
                    let k = slope(*a, *b);
                    if k == 0.0 {
                        return format!("{:.5}", a.1);
                    }
                    return format!("{:.5}+({var}-{:.5})*{k:.6}", a.1, a.0);
                }
                let mid = segs.len() / 2;
                format!(
                    "if(lt({var},{:.5}),{},{})",
                    segs[mid].0 .0,
                    tree(&segs[..mid], var, slope),
                    tree(&segs[mid..], var, slope)
                )
            }
            let (first, last) = (pts[0], pts[pts.len() - 1]);
            format!(
                "if(lt({var},{:.5}),{:.5},if(lt({var},{:.5}),{},{:.5}))",
                first.0,
                first.1,
                last.0,
                tree(&segs, var, &slope),
                last.1
            )
        }
    }
}

/// Evaluates a curve produced by [`piecewise_expr`] directly (for tests and preview).
#[cfg(test)]
fn piecewise_eval(points: &[(f64, f64)], x: f64) -> f64 {
    if x < points[0].0 {
        return points[0].1;
    }
    for w in points.windows(2) {
        if x < w[1].0 {
            let k = if (w[1].0 - w[0].0).abs() < 1e-9 {
                0.0
            } else {
                (w[1].1 - w[0].1) / (w[1].0 - w[0].0)
            };
            return w[0].1 + (x - w[0].0) * k;
        }
    }
    points[points.len() - 1].1
}

/// Helper for filter graphs with unique labels.
pub struct GraphBuilder<'a> {
    pub lines: Vec<String>,
    cur: String,
    pending: Vec<String>,
    uniq: &'a str,
    n: usize,
}

impl<'a> GraphBuilder<'a> {
    pub fn new(input: &str, uniq: &'a str) -> Self {
        Self {
            lines: Vec::new(),
            cur: input.to_string(),
            pending: Vec::new(),
            uniq,
            n: 0,
        }
    }
    fn label(&mut self, what: &str) -> String {
        self.n += 1;
        format!("{}{what}{}", self.uniq, self.n)
    }
    pub fn push(&mut self, f: impl Into<String>) {
        let f = f.into();
        if !f.is_empty() {
            self.pending.push(f);
        }
    }
    pub fn flush(&mut self) -> String {
        if !self.pending.is_empty() {
            let l = self.label("c");
            self.lines
                .push(format!("[{}]{}[{l}]", self.cur, self.pending.join(",")));
            self.pending.clear();
            self.cur = l;
        }
        self.cur.clone()
    }
    /// Lay the chain over the original with strength `i`.
    pub fn blended(&mut self, chain: &str, i: f32) {
        if i >= 0.999 {
            self.push(chain.to_string());
            return;
        }
        if i <= 0.001 {
            return;
        }
        self.push("format=gbrap");
        let cur = self.flush();
        let (a, b, f, o) = (
            self.label("a"),
            self.label("b"),
            self.label("f"),
            self.label("o"),
        );
        self.lines.push(format!("[{cur}]split[{a}][{b}]"));
        self.lines.push(format!("[{b}]{chain},format=gbrap[{f}]"));
        self.lines.push(format!(
            "[{f}][{a}]blend=all_mode=normal:all_opacity={i:.3}[{o}]"
        ));
        self.cur = o;
    }
    pub fn finish(mut self, output: &str) -> Vec<String> {
        let cur = self.flush();
        self.lines.push(format!("[{cur}]null[{output}]"));
        self.lines
    }
    pub fn current(&mut self) -> String {
        self.flush()
    }
    pub fn set_current(&mut self, l: String) {
        self.cur = l;
    }
    pub fn next_label(&mut self, what: &str) -> String {
        self.label(what)
    }
}

/// Starting point for the chain.
#[derive(Debug, Clone)]
pub struct ChainOpts {
    /// Time offset (s): `t` in effects counts from the clip start
    pub t_offset: f64,
    /// Run reverse in the graph (export; the preview maps the time itself)
    pub apply_reverse: bool,
    /// LUT file, already escaped for filter options
    pub lut: Option<String>,
    /// Label of a mask stream (grayscale, same size/time) for „Remove background“
    pub matte: Option<String>,
}

/// Adjustments as a filter chain (empty when neutral).
pub fn adjust_chain(a: &Adjust) -> Vec<String> {
    let mut v = Vec::new();
    if a.is_identity() {
        return v;
    }
    let (b, c, s) = (a.brightness, a.contrast, a.saturation);
    if b != 0.0 || c != 0.0 || s != 0.0 {
        v.push(format!(
            "eq=brightness={:.3}:contrast={:.3}:saturation={:.3}",
            b * 0.3,
            1.0 + c * 0.6,
            (1.0 + s).max(0.0)
        ));
    }
    if a.temperature != 0.0 {
        v.push(format!(
            "colortemperature=temperature={:.0}",
            6500.0 - a.temperature * 3500.0
        ));
    }
    if a.tint != 0.0 {
        let t = a.tint;
        v.push(format!(
            "colorbalance=rm={:.3}:gm={:.3}:bm={:.3}",
            t * 0.1,
            -t * 0.15,
            t * 0.1
        ));
    }
    if a.hue != 0.0 {
        v.push(format!("hue=h={:.1}", a.hue));
    }
    for (k, (code, ..)) in HSL_COLORS.iter().enumerate() {
        let [h, s, l] = a.hsl[k];
        if h != 0.0 || s != 0.0 || l != 0.0 {
            v.push(format!(
                "huesaturation=hue={:.1}:saturation={:.3}:intensity={:.3}:colors={code}:strength=6",
                h * 60.0,
                s,
                l * 0.5
            ));
        }
    }
    if a.highlights != 0.0 || a.shadows != 0.0 || a.fade != 0.0 {
        let lo = (0.15 * a.fade.max(0.0)).clamp(0.0, 0.4);
        let hi = (1.0 - 0.05 * a.fade.max(0.0)).clamp(0.6, 1.0);
        let sh = (0.25 + 0.12 * a.shadows).clamp(0.01, 0.49);
        let hl = (0.75 + 0.12 * a.highlights).clamp(0.51, 0.99);
        v.push(format!(
            "curves=all='0/{lo:.3} 0.25/{sh:.3} 0.75/{hl:.3} 1/{hi:.3}'"
        ));
    }
    if a.sharpen > 0.0 {
        v.push(format!("unsharp=5:5:{:.2}", a.sharpen * 2.0));
    } else if a.sharpen < 0.0 {
        v.push(format!("gblur=sigma={:.2}", -a.sharpen * 6.0));
    }
    if a.vignette > 0.0 {
        v.push(format!("vignette=angle={:.3}", a.vignette * 1.2));
    }
    if a.grain > 0.0 {
        v.push(format!("noise=alls={:.0}:allf=t", a.grain * 40.0));
    }
    v
}

/// Mask expression (0…1) for `geq` in pixel coordinates X/Y, image size W/H.
pub fn mask_expr(m: &Mask) -> String {
    let (s, c) = m.rotation_deg.to_radians().sin_cos();
    let (cx, cy) = (format!("{:.4}*W", m.cx), format!("{:.4}*H", m.cy));
    // rotated coordinates relative to the center
    let u = format!("((X-{cx})*{c:.5}+(Y-{cy})*{s:.5})");
    let v = format!("(-(X-{cx})*{s:.5}+(Y-{cy})*{c:.5})");
    let f = (m.feather.max(0.0005) * 2.0) as f64;
    let base = match m.shape {
        MaskShape::Linear => format!("clip(0.5-{v}/({f:.4}*H+1),0,1)"),
        MaskShape::Mirror => format!("clip(({:.4}*H/2-abs({v}))/({f:.4}*H+1)+0.5,0,1)", m.h),
        MaskShape::Circle => {
            format!(
                "clip((1-hypot({u}/({:.4}*W/2+0.001),{v}/({:.4}*H/2+0.001)))/{f:.4}+0.5,0,1)",
                m.w, m.h
            )
        }
        MaskShape::Rect => {
            format!("clip((1-max(abs({u})/({:.4}*W/2+0.001),abs({v})/({:.4}*H/2+0.001)))/{f:.4}+0.5,0,1)", m.w, m.h)
        }
    };
    if m.invert {
        format!("(1-{base})")
    } else {
        base
    }
}

/// Complete clip chain: input `input` → output `output` (RGBA with a mask, otherwise open).
pub fn fx_graph(
    fx: &ClipFx,
    src: Src,
    o: &ChainOpts,
    input: &str,
    output: &str,
    uniq: &str,
) -> Vec<String> {
    let mut g = GraphBuilder::new(input, uniq);
    if o.apply_reverse && fx.reverse {
        g.push("reverse");
    }
    // rounded: setpts truncates otherwise (29.97 fps: 5.5055 s → one frame too early)
    g.push(format!("setpts=PTS-STARTPTS+round({:.6}/TB)", o.t_offset));
    // before everything else: position in the source image (before flip, stabilization, keying)
    for cs in &fx.censor {
        censor_graph(&mut g, cs, src);
    }
    if fx.deflicker {
        g.push("deflicker=size=7:mode=pm");
    }
    if fx.denoise > 0.0 {
        let k = fx.denoise.clamp(0.0, 1.0);
        g.push(format!(
            "hqdn3d=luma_spatial={:.2}:chroma_spatial={:.2}:luma_tmp={:.2}:chroma_tmp={:.2}",
            1.0 + 7.0 * k,
            1.0 + 5.0 * k,
            2.0 + 10.0 * k,
            2.0 + 8.0 * k
        ));
    }
    let edge = fx.edge.clamped();
    // color key as a gray mask when background effect or edge values need one; never next to a
    // person mask (older projects with both keep their image: key applied after the person)
    let chroma_matte = fx
        .chroma
        .filter(|_| o.matte.is_none() && fx.cutout.is_none() && (fx.person.is_some() || !edge.is_identity()));
    let mut matte = o.matte.clone();
    // stabilization is skipped with a person mask (it would no longer fit)
    if matte.is_none() && fx.stabilize > 0.0 {
        let k = fx.stabilize.clamp(0.0, 1.0);
        let r = 16 * (1.0 + 3.0 * k).round() as u32;
        let z = 1.0 - 0.08 * k;
        let (cw, ch) = (
            ((src.w as f32 * z) as u32).max(2) & !1,
            ((src.h as f32 * z) as u32).max(2) & !1,
        );
        g.push(format!(
            "deshake=rx={r}:ry={r}:edge=mirror,crop={cw}:{ch},scale={}:{}",
            src.w, src.h
        ));
    }
    if let Some(k) = &chroma_matte {
        let cur = g.current();
        let (main, key, km) = (g.next_label("ck"), g.next_label("cka"), g.next_label("ckm"));
        g.lines.push(format!("[{cur}]split[{main}][{key}]"));
        g.lines.push(format!("[{key}]{},format=rgba,alphaextract[{km}]", chroma_filter(k)));
        g.set_current(main);
        if let Some(d) = despill_filter(k) {
            g.push(d);
        }
        matte = Some(km);
    }
    if let Some(m) = matte.as_mut().filter(|_| !edge.is_identity()) {
        *m = edge_graph(&mut g, m, &edge, src);
    }
    let mut person_done = false;
    if let (Some(m), Some(pf)) = (&matte, &fx.person) {
        person_graph(&mut g, m, pf, src);
        person_done = true;
    }
    if person_done {
    } else if let Some(m) = &matte {
        // remove background: mask as alpha channel
        g.push("format=rgb24");
        let cur = g.current();
        let l = g.next_label("cm");
        g.lines.push(format!("[{cur}][{m}]alphamerge[{l}]"));
        g.set_current(l);
    }
    if fx.flip_h {
        g.push("hflip");
    }
    if fx.flip_v {
        g.push("vflip");
    }
    if let Some(k) = fx.chroma.as_ref().filter(|_| chroma_matte.is_none()) {
        g.push(chroma_filter(k));
        if let Some(d) = despill_filter(k) {
            g.push(d);
        }
    }
    let src = Src {
        has_alpha: src.has_alpha
            || (matte.is_some()
                && (!person_done || matches!(fx.person, Some(cl_core::PersonFx::Outline { transparent: true, .. }))))
            || (fx.chroma.is_some() && chroma_matte.is_none()),
        ..src
    };
    let colour_work = !fx.adjust.is_identity()
        || fx.filter.is_some()
        || !fx.effects.is_empty()
        || o.lut.is_some();
    // rescue alpha (stickers, keying, green screen) across the color filters
    let mut alpha = None;
    if src.has_alpha && colour_work {
        g.push("format=rgba");
        let cur = g.current();
        let (c, a, am) = (g.next_label("k"), g.next_label("al"), g.next_label("am"));
        g.lines.push(format!("[{cur}]split[{c}][{a}]"));
        g.lines.push(format!("[{a}]alphaextract[{am}]"));
        g.set_current(c);
        alpha = Some(am);
    }
    for f in adjust_chain(&fx.adjust) {
        g.push(f);
    }
    if let Some(f) = &fx.filter {
        if let Some(def) = filter(&f.id) {
            g.blended(&(def.build)(1.0, src), f.intensity.clamp(0.0, 1.0));
        } else if let Some(c) = catalog::pack_chain(catalog::Kind::Filter, &f.id, 1.0) {
            g.blended(&c, f.intensity.clamp(0.0, 1.0));
        }
    }
    for e in &fx.effects {
        if let Some(def) = effect(&e.id) {
            let i = e.intensity.clamp(0.0, 1.0);
            if def.native_intensity {
                g.push((def.build)(i, src));
            } else {
                g.blended(&(def.build)(1.0, src), i);
            }
        } else if let Some(c) = catalog::pack_chain(catalog::Kind::Effect, &e.id, e.intensity.clamp(0.0, 1.0)) {
            g.push(c);
        }
    }
    if let Some(l) = &o.lut {
        g.push(format!("lut3d=file={l}"));
    }
    if let Some(am) = alpha {
        g.push("format=rgb24");
        let cur = g.current();
        let l = g.next_label("m");
        g.lines.push(format!("[{cur}][{am}]alphamerge[{l}]"));
        g.set_current(l);
    }
    if let Some(m) = &fx.mask {
        g.push("format=rgba");
        g.push(format!(
            "geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='alpha(X,Y)*{}'",
            mask_expr(m)
        ));
    }
    g.finish(output)
}

fn chroma_filter(k: &ChromaKey) -> String {
    let [r, gg, b] = k.color;
    format!(
        "chromakey=color=0x{r:02x}{gg:02x}{b:02x}:similarity={:.3}:blend={:.3}",
        k.similarity.clamp(0.01, 1.0),
        k.blend.clamp(0.0, 1.0)
    )
}

fn despill_filter(k: &ChromaKey) -> Option<String> {
    let [_, gg, b] = k.color;
    let ty = if b > gg { "blue" } else { "green" };
    (k.spill > 0.0).then(|| format!("format=rgba,despill=type={ty}:mix={:.2}:expand=0.1", k.spill.clamp(0.0, 1.0)))
}

/// Edge of the mask `m` (gray): grow/shrink (blur + threshold, the trick of the outline in
/// `person_graph`), then soften, then invert. Sizes relative to the frame: preview = export.
/// Returns the label of the new mask.
fn edge_graph(g: &mut GraphBuilder, m: &str, e: &MatteEdge, src: Src) -> String {
    let k = src.w.max(src.h) as f32 / 1920.0;
    let mut v = vec!["format=gray".to_string()];
    if e.shift != 0.0 {
        // threshold at a quarter (grow) or three quarters (shrink) of the blurred edge
        v.push(format!("gblur=sigma={:.2}", 30.0 * e.shift.abs() * k));
        v.push(if e.shift > 0.0 { "lut=y='min(255,val*4)'" } else { "lut=y='max(0,val*4-765)'" }.into());
    }
    if e.feather > 0.0 {
        v.push(format!("gblur=sigma={:.2}", 20.0 * e.feather * k));
    }
    if e.invert {
        v.push("negate".into());
    }
    let l = g.next_label("me");
    g.lines.push(format!("[{m}]{}[{l}]", v.join(",")));
    l
}

/// Censor area (path in clip time, see `Clip::censor_in_clip_time`): pixelate or blur a copy
/// and overlay it only inside the area. Blocks, blur and mask
/// depend only on the image size: the smaller preview looks like the export. The mask
/// is built from one row and one column (`geq` with a time curve per pixel would be expensive): per
/// direction 0 inside to 255 outside, added and cut at 128 – rectangle as well as ellipse.
pub fn censor_graph(g: &mut GraphBuilder, cs: &Censor, src: Src) {
    let size = (CENSOR_MIN + cs.strength.clamp(0.0, 1.0) * (CENSOR_MAX - CENSOR_MIN))
        * src.w.min(src.h).max(2) as f32;
    let effect = if cs.blur {
        format!("gblur=sigma={size:.2}")
    } else {
        let n = |v: u32| ((v as f32 / size).round() as u32).max(1);
        format!(
            "scale={}:{}:flags=area,scale={}:{}:flags=neighbor",
            n(src.w),
            n(src.h),
            src.w,
            src.h
        )
    };
    let e = |f: fn(&CensorKey) -> f32| {
        let pts: Vec<(f64, f64)> = cs.path.iter().map(|k| (k.t.as_secs_f64(), f(k) as f64)).collect();
        piecewise_expr(&pts, "T")
    };
    // distance to the center relative to half the size: 1 = edge
    let side = |pos: &str, full: &str, c: String, size: String| {
        let d = format!("2*abs(({pos}+0.5)/{full}-({c}))/({size})");
        if cs.ellipse {
            format!("min(255,128*pow({d},2))")
        } else {
            format!("255*gt({d},1)")
        }
    };
    let (sx, sy) = (side("X", "W", e(|k| k.cx), e(|k| k.w)), side("Y", "H", e(|k| k.cy), e(|k| k.h)));
    let (w, h) = (src.w, src.h);
    let cur = g.current();
    let (a, b, mx, my) = (g.next_label("za"), g.next_label("zb"), g.next_label("zx"), g.next_label("zy"));
    let (kx, ky) = (g.next_label("zkx"), g.next_label("zky"));
    let (fx, mk, top, out) = (g.next_label("zf"), g.next_label("zk"), g.next_label("zt"), g.next_label("zo"));
    g.lines.push(format!("[{cur}]split=4[{a}][{b}][{mx}][{my}]"));
    g.lines.push(format!("[{b}]{effect}[{fx}]"));
    g.lines.push(format!(
        "[{mx}]scale={w}:1:flags=area,format=gray,geq=lum='{sx}',scale={w}:{h}:flags=neighbor[{kx}]"
    ));
    g.lines.push(format!(
        "[{my}]scale=1:{h}:flags=area,format=gray,geq=lum='{sy}',scale={w}:{h}:flags=neighbor[{ky}]"
    ));
    g.lines.push(format!("[{kx}][{ky}]blend=all_mode=addition,lut=c0='255*lte(val,128)'[{mk}]"));
    g.lines.push(format!("[{fx}][{mk}]alphamerge[{top}]"));
    // shortcut: images with alpha become opaque under the area; enough for video and photos
    g.lines.push(format!("[{a}][{top}]overlay=format=auto[{out}]"));
    g.set_current(out);
}

/// Person effects: the person (mask `m`) stays sharp, the background is changed.
/// The result is opaque.
fn person_graph(g: &mut GraphBuilder, m: &str, pf: &cl_core::PersonFx, src: Src) {
    use cl_core::PersonFx;
    let hex = |c: [u8; 3]| format!("0x{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    g.push("format=rgb24");
    let cur = g.current();
    let (fg_src, bg_src, mm) = (g.next_label("pf"), g.next_label("pb"), g.next_label("pm"));
    let fg = g.next_label("pfg");
    let bg = g.next_label("pbg");
    let out = g.next_label("po");
    match pf {
        PersonFx::BlurBackground { amount } => {
            let sigma =
                2.0 + 28.0 * amount.clamp(0.0, 1.0) * (src.w.max(src.h) as f32 / 1920.0).max(0.2);
            g.lines.push(format!("[{cur}]split[{fg_src}][{bg_src}]"));
            g.lines
                .push(format!("[{bg_src}]gblur=sigma={sigma:.2}[{bg}]"));
            g.lines.push(format!("[{fg_src}][{m}]alphamerge[{fg}]"));
            g.lines.push(format!(
                "[{bg}][{fg}]overlay=format=auto,format=rgb24[{out}]"
            ));
        }
        PersonFx::ColorBackground { color } => {
            g.lines.push(format!("[{cur}]split[{fg_src}][{bg_src}]"));
            g.lines.push(format!(
                "[{bg_src}]drawbox=x=0:y=0:w=iw:h=ih:color={}:t=fill[{bg}]",
                hex(*color)
            ));
            g.lines.push(format!("[{fg_src}][{m}]alphamerge[{fg}]"));
            g.lines.push(format!(
                "[{bg}][{fg}]overlay=format=auto,format=rgb24[{out}]"
            ));
        }
        PersonFx::Outline { color, width, transparent } => {
            // widen and blur the mask → colored halo behind the person
            let px =
                (2.0 + 22.0 * width.clamp(0.0, 1.0)) * (src.w.max(src.h) as f32 / 1920.0).max(0.25);
            let (m1, m2, ring, col, base, ringed) = (
                g.next_label("pm1"),
                g.next_label("pm2"),
                g.next_label("pr"),
                g.next_label("pc"),
                g.next_label("pbs"),
                g.next_label("prd"),
            );
            // without background: the outline is the base, the result keeps alpha
            if *transparent {
                g.lines.push(format!("[{cur}]split[{fg_src}][{bg_src}]"));
            } else {
                g.lines
                    .push(format!("[{cur}]split=3[{fg_src}][{bg_src}][{base}]"));
            }
            g.lines.push(format!("[{m}]split[{m1}][{m2}]"));
            g.lines.push(format!(
                "[{m2}]format=gray,gblur=sigma={:.2},lut=y='min(255,val*4)'[{mm}]",
                px / 2.0
            ));
            g.lines.push(format!(
                "[{bg_src}]drawbox=x=0:y=0:w=iw:h=ih:color={}:t=fill[{col}]",
                hex(*color)
            ));
            g.lines.push(format!("[{col}][{mm}]alphamerge[{ring}]"));
            let under = if *transparent {
                ring
            } else {
                g.lines
                    .push(format!("[{base}][{ring}]overlay=format=auto[{ringed}]"));
                ringed
            };
            g.lines.push(format!("[{fg_src}][{m1}]alphamerge[{fg}]"));
            g.lines.push(format!(
                "[{under}][{fg}]overlay=format=auto,format={}[{out}]",
                if *transparent { "rgba" } else { "rgb24" }
            ));
        }
    }
    g.set_current(out);
}

/// Part of a mask file that feeds a source segment, see [`matte_window`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatteWindow {
    /// Start in the mask file (from its frame 0) and length to read
    pub seek: Time,
    pub len: Time,
    /// Source time before the mask begins and after it ends: not cut out there
    pub lead: Time,
    pub tail: Time,
}

/// Mask file covering `span` (`None` = the whole source, frame 0 = source 0) for the source
/// segment `src_start .. src_start + src_len`; `None` when they do not overlap (not cut out at all).
pub fn matte_window(span: Option<MatteSpan>, src_start: Time, src_len: Time) -> Option<MatteWindow> {
    let Some(s) = span else {
        return Some(MatteWindow { seek: src_start, len: src_len, lead: Time::ZERO, tail: Time::ZERO });
    };
    let end = src_start + src_len;
    let (a, b) = (src_start.max(s.start), end.min(s.end));
    (b > a).then(|| MatteWindow { seek: a - s.start, len: b - a, lead: a - src_start, tail: end - b })
}

/// `tpad` that fills `lead`/`tail` seconds with a mask that keeps the whole picture (white; black
/// when the edge inverts it, so it ends up white). `None` without padding (graph as before).
pub fn matte_pad(lead: f64, tail: f64, invert: bool) -> Option<String> {
    let color = if invert { "black" } else { "white" };
    let mut v = vec![];
    if lead > 0.0 {
        v.push(format!("start_duration={lead:.6}"));
    }
    if tail > 0.0 {
        v.push(format!("stop_duration={tail:.6}"));
    }
    (!v.is_empty()).then(|| format!("tpad={}:color={color}", v.join(":")))
}

/// Mask file of the clip, if it exists (person or object mask).
pub fn matte_file(fx: &ClipFx) -> Option<&String> {
    fx.cutout.as_ref().filter(|p| std::path::Path::new(p).is_file())
}

/// File names of the media whose visible clips name a mask that is not on disk (missing or
/// still being computed): the export would show them without the cutout.
pub fn missing_masks(p: &Project) -> Vec<String> {
    let mut v: Vec<String> = p
        .timeline
        .tracks
        .iter()
        .filter(|t| !t.hidden)
        .flat_map(|t| &t.clips)
        .filter(|c| !c.disabled && c.fx.cutout.is_some() && matte_file(&c.fx).is_none())
        .filter_map(|c| match c.content {
            ClipContent::Media { media, .. } => p.media(media),
            _ => None,
        })
        .map(|m| m.path.file_name().map_or_else(|| m.path.display().to_string(), |n| n.to_string_lossy().into_owned()))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Filter graph for the preview (`[0:v]` → `[out]`, `w`×`h` RGBA), or `None` if the
/// fast path without effects is enough. `t_offset` = clip time of the first frame,
/// `stream` = playback (frame rate and speed), otherwise single frame.
pub fn preview_graph(
    c: &Clip,
    w: u32,
    h: u32,
    t_offset: f64,
    stream: Option<Rational>,
    lut: Option<String>,
) -> Option<String> {
    let speed = c.speed;
    let fx = &c.fx;
    let image = matches!(c.content, ClipContent::Media { kind: MediaKind::Image, .. });
    // mask part for this frame (single frame) or from here to the clip end (stream)
    let src_t = c.source_time(c.start + Time::from_secs_f64(t_offset));
    let src_len = match stream {
        Some(_) => Time((c.source_time(c.end()).0 - src_t.0).max(1)),
        None => Time(1),
    };
    let window = if image {
        // still image: one mask frame
        Some(MatteWindow { seek: Time::ZERO, len: Time(1), lead: Time::ZERO, tail: Time::ZERO })
    } else {
        matte_window(fx.cutout_span, src_t, src_len)
    };
    let cutout = matte_file(fx).zip(window);
    let needs = !fx.is_empty()
        && (fx.flip_h
            || fx.flip_v
            || !fx.adjust.is_identity()
            || fx.filter.is_some()
            || !fx.effects.is_empty()
            || fx.mask.is_some()
            || lut.is_some()
            || fx.chroma.is_some()
            || fx.stabilize > 0.0
            || cutout.is_some()
            || fx.denoise > 0.0
            || fx.deflicker
            || !fx.censor.is_empty());
    let speed_stream = stream.is_some() && ((speed - 1.0).abs() > 1e-3 || fx.speed_curve.is_some());
    if !needs && !speed_stream {
        return None;
    }
    let mut pre = vec!["setpts=PTS-STARTPTS".to_string()];
    let mut matte_pre = vec!["setpts=PTS-STARTPTS".to_string()];
    if let Some(fps) = stream {
        let sped = fx.speed_curve.is_none() && (speed - 1.0).abs() > 1e-3;
        if sped {
            matte_pre.push(format!("setpts=(PTS-STARTPTS)/{speed:.5}"));
        }
        // padding in stream time (source time / speed) after `fps`: FFmpeg 7 forgets the rate after
        // `setpts`. `eof_action=pass` keeps the mask's last frame (the movie source ends at its pts).
        // shortcut: with a speed curve the padding uses speed 1, exact enough for the preview
        let k = if sped { speed.max(0.01) as f64 } else { 1.0 };
        match cutout.and_then(|(_, w)| matte_pad(w.lead.as_secs_f64() / k, w.tail.as_secs_f64() / k, fx.edge.invert)) {
            Some(p) => matte_pre.extend([format!("fps={}/{}:eof_action=pass", fps.num, fps.den), p]),
            None => matte_pre.push(format!("fps={}/{}", fps.num, fps.den)),
        }
    }
    // the picture carries the clip time from `fx_graph` (`setpts=…+t_offset`) and `alphamerge` pairs
    // frames by time: the mask needs the same time, or it runs `t_offset` ahead
    if t_offset > 0.0 {
        matte_pre.push(format!("setpts=PTS-STARTPTS+round({t_offset:.6}/TB)"));
    }
    matte_pre.push(format!("scale={w}:{h}:flags=bilinear,format=gray"));
    if let Some(fps) = stream {
        if fx.speed_curve.is_none() && (speed - 1.0).abs() > 1e-3 {
            pre.push(format!("setpts=(PTS-STARTPTS)/{speed:.5}"));
        }
        if fx.smooth_slowmo && speed < 0.999 {
            // preview: cheap cross-fade instead of full motion estimation
            pre.push(format!(
                "minterpolate=fps={}/{}:mi_mode=blend",
                fps.num, fps.den
            ));
        }
        pre.push(format!("fps={}/{}", fps.num, fps.den));
    }
    pre.push(format!("scale={w}:{h}:flags=bilinear"));
    let mut lines = vec![format!("[0:v]{}[pre]", pre.join(","))];
    let mut matte = None;
    if let Some((m, win)) = cutout {
        let seek = win.seek.as_secs_f64();
        // `movie` seeks to an index point before `seek_point` and passes everything from there: drop
        // the frames before the wanted one (2 ms slack for millisecond timestamps)
        let trim = if seek > 0.0 { format!("trim=start={:.6},", (seek - 0.002).max(0.0)) } else { String::new() };
        lines.push(format!(
            "movie=filename={}:seek_point={seek:.4},{trim}{}[mt]",
            cl_media::escape_path(m),
            matte_pre.join(",")
        ));
        matte = Some("mt".to_string());
    }
    let has_alpha = true;
    let mut fx2 = fx.clone();
    fx2.reverse = false;
    // single frame: only the position at this time (short graph on the command line)
    fx2.censor = c.censor_in_clip_time(stream.is_none().then(|| Time::from_secs_f64(t_offset)));
    let o = ChainOpts {
        t_offset,
        apply_reverse: false,
        lut,
        matte,
    };
    lines.extend(fx_graph(
        &fx2,
        Src { w, h, has_alpha },
        &o,
        "pre",
        "fx",
        "p",
    ));
    lines.push("[fx]format=rgba[out]".into());
    Some(lines.join(";"))
}

/// `atempo` chain for a speed (each stage 0.5…2).
pub fn atempo_chain(speed: f32) -> String {
    let mut s = speed.clamp(0.01, 100.0) as f64;
    let mut v = Vec::new();
    while s > 2.0 {
        v.push("atempo=2".to_string());
        s /= 2.0;
    }
    while s < 0.5 {
        v.push("atempo=0.5".to_string());
        s /= 0.5;
    }
    if (s - 1.0).abs() > 1e-4 {
        v.push(format!("atempo={s:.5}"));
    }
    v.join(",")
}

/// `setpts` for speed or speed curve. Input: source time from 0
/// (length `src_span` s), output: clip time (length `dur` s).
pub fn speed_setpts(speed: f32, curve: Option<&str>, src_span: f64, dur: f64) -> String {
    match curve {
        Some(id) => {
            // sample the inverse of the curve: source time → clip time
            let pts: Vec<(f64, f64)> = (0..=48)
                .map(|k| {
                    let u = k as f32 / 48.0;
                    (curve_progress(id, u) as f64 * src_span, u as f64 * dur)
                })
                .collect();
            format!("setpts='({})/TB'", piecewise_expr(&pts, "T"))
        }
        None if (speed - 1.0).abs() > 1e-4 => format!("setpts=(PTS-STARTPTS)/{speed:.5}"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// Every effect and filter must also run as a playback stream (otherwise a still
    /// image stays during playback).
    #[test]
    fn every_effect_and_filter_streams_for_playback() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.mp4");
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=3", "-pix_fmt", "yuv420p"])
            .arg(&src)
            .status()
            .unwrap();
        assert!(st.success());
        let fps = Rational::new(30, 1);
        let mut failed = Vec::new();
        let ids: Vec<(String, bool)> = EFFECTS
            .iter()
            .map(|e| (e.id.to_string(), false))
            .chain(FILTERS.iter().map(|f| (f.id.to_string(), true)))
            .collect();
        for (id, filter) in ids {
            let mut c = Clip::new(
                ClipContent::Media { media: MediaId::new(), kind: MediaKind::Video },
                Time::ZERO,
                Time::from_secs_f64(3.0),
            );
            if filter {
                c.fx.filter = Some(FxRef::new(&id));
            } else {
                c.fx.effects.push(FxRef::new(&id));
            }
            // every effect and filter needs a graph; without one the clip would play unchanged
            let Some(g) = preview_graph(&c, 320, 180, 0.5, Some(fps), None) else {
                failed.push(format!("{id} (no graph)"));
                continue;
            };
            let s = cl_media::FrameStream::open_fx(&src, Time::from_secs_f64(0.5), 320, 180, fps, Some(&g));
            let n = match s {
                Ok(s) => (0..10).take_while(|_| s.next_blocking().is_some()).count(),
                Err(_) => 0,
            };
            if n < 10 {
                failed.push(format!("{id} ({n} frames)"));
            }
        }
        assert!(failed.is_empty(), "do not run as a stream: {failed:?}");
    }

    /// VHS on the small preview image looks like the Full HD export scaled down to the same
    /// size: line pattern and color shift equally strong (previously much stronger in the preview).
    #[test]
    fn vhs_preview_size_looks_like_the_export() {
        if !cl_media::test_tools() {
            return;
        }
        // image after the chain at 480×270: (line pattern, red−blue color shift at the vertical edge)
        let look = |w: u32, h: u32| -> (f64, f64) {
            let vhs = (effect("vhs").unwrap().build)(1.0, Src { w, h, has_alpha: false });
            let out = cl_media::ffmpeg_command()
                .unwrap()
                .args(["-v", "error", "-nostdin", "-threads", "2", "-f", "lavfi", "-i"])
                .arg("color=c=0x808080:s=1920x1080:d=0.1,drawbox=x=960:y=0:w=960:h=1080:c=white:t=fill")
                .args(["-frames:v", "1", "-vf"])
                .arg(format!("scale={w}:{h}:flags=area,format=rgb24,{vhs},scale=480:270:flags=area,format=rgb24"))
                .args(["-f", "rawvideo", "-"])
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let px = |x: usize, y: usize, c: usize| out.stdout[(y * 480 + x) * 3 + c] as f64;
            let rows: Vec<f64> = (0..270).map(|y| (0..200).map(|x| px(x, y, 1)).sum::<f64>() / 200.0).collect();
            let stripes = (1..269).map(|y| (rows[y] - (rows[y - 1] + rows[y + 1]) / 2.0).abs()).sum::<f64>() / 268.0;
            // red runs left, blue right: columns next to the edge at x = 240
            let shift = (0..270).map(|y| (230..250).map(|x| (px(x, y, 0) - px(x, y, 2)).abs()).sum::<f64>()).sum::<f64>() / (270.0 * 20.0);
            (stripes, shift)
        };
        // old chain: lines 14.6 instead of 3.7, color shift 76 instead of 21
        let (export, preview) = (look(1920, 1080), look(480, 270));
        assert!((export.0 - preview.0).abs() < 4.0, "lines: export {export:?}, preview {preview:?}");
        assert!((export.1 - preview.1).abs() < 4.0, "color offset: export {export:?}, preview {preview:?}");
    }

    fn run_graph(graph: &str, extra_inputs: &[&str]) -> std::result::Result<(), String> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("g.txt"), graph).unwrap();
        let mut cmd = cl_media::ffmpeg_command().unwrap();
        cmd.args([
            "-v",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=160x90:r=15:d=0.6",
        ]);
        for i in extra_inputs {
            cmd.args(["-f", "lavfi", "-i", i]);
        }
        let out = cmd
            .args([
                "-/filter_complex",
                "g.txt",
                "-map",
                "[out]",
                "-f",
                "null",
                "-",
            ])
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }

    const SRC: Src = Src {
        w: 160,
        h: 90,
        has_alpha: false,
    };


    #[test]
    fn full_chain_with_adjust_mask_alpha_and_lut() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        // apostrophe, comma and brackets in the path (previously the LUT was then missing in the preview)
        let lut = dir.path().join("O'Brien, [x]").join("t.cube");
        std::fs::create_dir_all(lut.parent().unwrap()).unwrap();
        let mut cube = String::from("LUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    cube.push_str(&format!(
                        "{} {} {}\n",
                        r as f32 * 0.9,
                        g as f32,
                        b as f32 * 0.8
                    ));
                }
            }
        }
        std::fs::write(&lut, cube).unwrap();
        let mut fx = ClipFx::default();
        fx.adjust = Adjust {
            brightness: 0.2,
            contrast: 0.3,
            saturation: -0.2,
            temperature: 0.5,
            tint: 0.2,
            highlights: -0.4,
            shadows: 0.3,
            sharpen: 0.5,
            vignette: 0.4,
            grain: 0.3,
            fade: 0.3,
            hue: 20.0,
            hsl: [[0.2, 0.3, -0.1]; 6],
        };
        fx.filter = Some(FxRef {
            id: "cinematic".into(),
            intensity: 0.6,
            params: Default::default(),
        });
        fx.effects.push(FxRef::new("vhs"));
        fx.flip_h = true;
        fx.reverse = true;
        for shape in [
            MaskShape::Linear,
            MaskShape::Mirror,
            MaskShape::Circle,
            MaskShape::Rect,
        ] {
            let mut m = Mask::new(shape);
            m.rotation_deg = 30.0;
            m.invert = shape == MaskShape::Circle;
            fx.mask = Some(m);
            let o = ChainOpts {
                t_offset: 1.5,
                apply_reverse: true,
                lut: Some(cl_media::escape_path(&lut.to_string_lossy())),
                matte: None,
            };
            let src = Src {
                w: 160,
                h: 90,
                has_alpha: true,
            };
            let g = fx_graph(&fx, src, &o, "0:v", "out", "y").join(";\n");
            run_graph(&g, &[]).unwrap_or_else(|e| panic!("{shape:?}: {e}\n{g}"));
        }
    }

    #[test]
    fn censor_never_goes_below_the_minimum() {
        let src = Src { w: 1920, h: 1080, has_alpha: false };
        let graph = |blur: bool, strength: f32| {
            let mut g = GraphBuilder::new("i", "u");
            let path = vec![CensorKey { t: Time::ZERO, cx: 0.5, cy: 0.5, w: 0.2, h: 0.3 }];
            censor_graph(&mut g, &Censor { blur, ellipse: false, strength, track: false, path, ..Default::default() }, src);
            g.finish("o").join(";")
        };
        // blocks ≥ 45 px (1080/24), blur σ ≥ 45 px – also at strength below 0
        assert!(graph(false, -1.0).contains("scale=43:24:flags=area"), "{}", graph(false, -1.0));
        assert!(graph(true, -1.0).contains("gblur=sigma=45.00"));
        assert!(graph(true, 1.0).contains("gblur=sigma=135.00"));
        assert!(graph(false, 1.0).contains("scale=14:8:flags=area"));
    }

    #[test]
    fn preview_graph_decodes() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("v.mp4");
        let st = cl_media::ffmpeg_command()
            .unwrap()
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=320x180:r=30:d=2",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&v)
            .status()
            .unwrap();
        assert!(st.success());
        let mut c = Clip::new(
            ClipContent::Media {
                media: MediaId::new(),
                kind: MediaKind::Video,
            },
            Time::ZERO,
            Time::from_secs_f64(2.0),
        );
        assert!(preview_graph(&c, 160, 90, 0.0, None, None).is_none());
        c.fx.effects.push(FxRef::new("vhs"));
        c.fx.mask = Some(Mask::new(MaskShape::Circle));
        c.speed = 2.0;
        let g = preview_graph(&c, 160, 90, 0.5, None, None).unwrap();
        let f = cl_media::decode_frame_fx(&v, Time::from_secs_f64(0.5), 160, 90, Some(&g)).unwrap();
        assert_eq!(f.data.len(), 160 * 90 * 4);
        assert_eq!(
            f.data[3], 0,
            "corner outside the circle mask is transparent"
        );
        let g = preview_graph(&c, 160, 90, 0.0, Some(Rational::new(30, 1)), None).unwrap();
        let s =
            cl_media::FrameStream::open_fx(&v, Time::ZERO, 160, 90, Rational::new(30, 1), Some(&g))
                .unwrap();
        let mut n = 0;
        while s.next_blocking().is_some() {
            n += 1;
        }
        assert!(
            (28..=32).contains(&n),
            "2 s at speed 2 → ~30 frames, were {n}"
        );
    }

    #[test]
    fn chroma_stabilize_and_matte_run() {
        if !cl_media::test_tools() {
            return;
        }
        let mut fx = ClipFx::default();
        fx.chroma = Some(ChromaKey::default());
        fx.stabilize = 0.6;
        fx.adjust.brightness = 0.1;
        let o = ChainOpts {
            t_offset: 0.0,
            apply_reverse: false,
            lut: None,
            matte: None,
        };
        let g = fx_graph(&fx, SRC, &o, "0:v", "out", "c").join(";\n");
        run_graph(&g, &[]).unwrap_or_else(|e| panic!("{e}\n{g}"));
        // mask as the second input
        let mut lines = vec!["[1:v]format=gray[mt]".to_string()];
        let o = ChainOpts {
            t_offset: 0.0,
            apply_reverse: false,
            lut: None,
            matte: Some("mt".into()),
        };
        lines.extend(fx_graph(
            &ClipFx {
                flip_h: true,
                ..Default::default()
            },
            SRC,
            &o,
            "0:v",
            "out",
            "m",
        ));
        let g = lines.join(";\n");
        run_graph(&g, &["color=gray:s=160x90:r=15:d=0.6"]).unwrap_or_else(|e| panic!("{e}\n{g}"));
    }

    #[test]
    fn plugin_chains_run() {
        if !cl_media::test_tools() {
            return;
        }
        use catalog::{Entry, Kind, Pack, PackInfo, Payload};
        // fx_graph reads the global catalog (there is no `_in` variant): empty it again, also on panic
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                catalog::load_packs(Vec::new());
            }
        }
        let _reset = Reset;
        let info = PackInfo {
            id: "t.p".into(),
            name: "T".into(),
            version: "1.0.0".into(),
            description: String::new(),
            license: "CC0-1.0".into(),
            author: String::new(),
            source: None,
        };
        catalog::load_packs(vec![Pack {
            info,
            enabled: true,
            entries: vec![Entry {
                id: "t.p/glow".into(),
                kind: Kind::Effect,
                name: "Glow".into(),
                categories: vec![],
                pack: "t.p".into(),
                license: "CC0-1.0".into(),
                author: String::new(),
                source: None,
                tags: Vec::new(),
                payload: Payload::Ffmpeg("gblur=sigma=2,eq=brightness=0.04*{intensity}".into()),
            }],
        }]);
        let mut fx = ClipFx::default();
        fx.effects.push(FxRef {
            id: "t.p/glow".into(),
            intensity: 0.5,
            params: Default::default(),
        });
        let o = ChainOpts {
            t_offset: 0.0,
            apply_reverse: false,
            lut: None,
            matte: None,
        };
        let g = fx_graph(&fx, SRC, &o, "0:v", "out", "p").join(
            ";
",
        );
        assert!(g.contains("0.04*0.500"), "{g}");
        run_graph(&g, &[]).unwrap();
        assert_eq!(fx_name("t.p/glow"), "Glow");
    }

    #[test]
    fn every_transition_runs_in_ffmpeg() {
        // all xfade variants of the bundled FFmpeg except `custom` and `squeezev`
        // (squeezev crashed FFmpeg at every size when this list was made)
        assert_eq!(TRANSITIONS.len(), 57);
        assert!(!TRANSITIONS.iter().any(|t| t.0 == "squeezev"));
        if !cl_media::test_tools() {
            return;
        }
        for (id, ..) in TRANSITIONS {
            let g = format!("[0:v]format=gbrap[a];[1:v]format=gbrap[b];[a][b]xfade=transition={id}:duration=0.3:offset=0.2[out]");
            run_graph(&g, &["color=red:s=160x90:r=15:d=0.6"])
                .unwrap_or_else(|e| panic!("{id}: {e}"));
        }
    }

    #[test]
    fn long_piecewise_stays_shallow_and_runs_in_ffmpeg() {
        // 240 keypoints: animation at the start, constant afterwards (like a long text clip)
        let mut pts: Vec<(f64, f64)> = (0..240)
            .map(|i| (i as f64 * 0.141, if i < 4 { i as f64 * 0.1 } else { 0.4 }))
            .collect();
        // plus a real curve with many kinks
        pts.extend((240..400).map(|i| (i as f64 * 0.141, ((i as f64) * 0.3).sin())));
        let e = piecewise_expr(&pts, "t");
        let mut depth = 0i32;
        let mut max = 0;
        for c in e.chars() {
            match c {
                '(' => {
                    depth += 1;
                    max = max.max(depth);
                }
                ')' => depth -= 1,
                _ => {}
            }
        }
        assert!(max < 40, "nested too deeply: {max}");
        assert!(e.matches("if(").count() < pts.len());
        if !cl_media::test_tools() {
            return;
        }
        // FFmpeg evaluates the expression (as one audio sample per 1/100 s, `t` = time in s):
        // it matches the curve itself at every sample, also before the first and after the last point
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("g.txt"), format!("aevalsrc=exprs='{e}':s=100:d=58[out]")).unwrap();
        let out = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-nostdin", "-/filter_complex", "g.txt", "-map", "[out]", "-f", "f64le", "-"])
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let got: Vec<f64> = out.stdout.chunks_exact(8).map(|b| f64::from_le_bytes(b.try_into().unwrap())).collect();
        assert_eq!(got.len(), 5800);
        let bad: Vec<String> = got
            .iter()
            .enumerate()
            .filter_map(|(i, y)| {
                let want = piecewise_eval(&pts, i as f64 / 100.0);
                ((y - want).abs() > 1e-3).then(|| format!("t={}: {y} instead of {want}", i as f64 / 100.0))
            })
            .collect();
        assert!(bad.is_empty(), "{} of {}: {:?}", bad.len(), got.len(), &bad[..bad.len().min(5)]);
        // and as a frame-wise size expression, the way text clips use it
        let g = format!(
            "[0:v]scale=w='max(2,trunc(320*(0.5+{})/2)*2)':h=180:eval=frame[out]",
            e
        );
        run_graph(&g, &["color=red:s=320x180:r=10:d=1"]).unwrap();
    }

    #[test]
    fn piecewise_and_speed() {
        let e = piecewise_expr(&[(0.0, 0.0), (1.0, 10.0), (2.0, 10.0)], "t");
        assert!(e.starts_with("if(lt(t,"));
        assert_eq!(atempo_chain(1.0), "");
        assert_eq!(atempo_chain(4.0), "atempo=2,atempo=2.00000");
        assert_eq!(atempo_chain(0.25), "atempo=0.5,atempo=0.50000");
        // don't cap very slow pieces of a speed curve (e.g. 0.5× with „Sphere“)
        let prod: f64 = atempo_chain(0.05).split(',').map(|a| a[7..].parse::<f64>().unwrap()).product();
        assert!((prod - 0.05).abs() < 1e-4, "{prod}");
        if !cl_media::test_tools() {
            return;
        }
        let sp = speed_setpts(1.0, Some("montage"), 0.6, 0.6);
        run_graph(&format!("[0:v]{sp}[out]"), &[]).unwrap();
        run_graph(
            &format!("[0:v]{}[out]", speed_setpts(2.0, None, 0.6, 0.3)),
            &[],
        )
        .unwrap();
        run_graph(
            &format!(
                "[0:v]setpts=PTS-STARTPTS,geq=lum='{}':cb=128:cr=128[out]",
                piecewise_expr(&[(0.0, 10.0), (0.3, 200.0)], "T")
            ),
            &[],
        )
        .unwrap();
    }

    /// Package "Effects and filters" (`plugins/cutline-effects`): every chain runs in FFmpeg on
    /// export input (YUV), keeps the image size, and effects do practically nothing at strength 0.
    /// (RGBA input as in the preview: `every_chain_runs_at_odd_sizes_and_strengths`.)
    #[test]
    fn effects_pack_chains_run() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../plugins/cutline-effects/plugin.toml"
        ))
        .unwrap();
        // shortcut: read lines instead of parsing TOML (cl-fx does not depend on toml); the package writes
        // every chain into one line `ffmpeg = "…"` after its line `kind = "…"`
        let mut assets = Vec::new();
        let mut kind = "";
        for l in src.lines() {
            if let Some(k) = l.strip_prefix("kind = \"") {
                kind = k.trim_end_matches('"');
            }
            if let Some(c) = l.strip_prefix("ffmpeg = \"") {
                assets.push((kind == "filter", c.trim_end_matches('"').to_string()));
            }
        }
        assert!(assets.len() >= 30, "{}", assets.len());
        assert!(assets.iter().all(|(f, c)| *f || c.contains("{intensity}")));
        if !cl_media::test_tools() {
            return;
        }
        let (w, h, n) = (202usize, 114usize, 12usize);
        // frames as RGBA bytes; `fx` = (chain, strength, is filter)
        let render = |fmt: &str, fx: Option<(&str, f32, bool)>| -> Vec<u8> {
            let mut g = GraphBuilder::new("0:v", "e");
            g.push(format!("format={fmt}"));
            match fx {
                Some((c, i, true)) => g.blended(&c.replace("{intensity}", "1.000"), i),
                Some((c, i, false)) => g.push(c.replace("{intensity}", &format!("{i:.3}"))),
                None => {}
            }
            g.push("format=rgba");
            let graph = g.finish("out").join(";");
            let out = cl_media::ffmpeg_command()
                .unwrap()
                .args(["-v", "error", "-nostdin", "-f", "lavfi", "-i"])
                .arg(format!("testsrc2=s={w}x{h}:r=24:d=0.5"))
                .args(["-filter_complex", &graph, "-map", "[out]", "-f", "rawvideo", "-"])
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(out.status.success(), "{graph}\n{}", String::from_utf8_lossy(&out.stderr));
            assert_eq!(out.stdout.len(), w * h * 4 * n, "image size/frame count: {graph}");
            out.stdout
        };
        let plain = render("yuv420p", None);
        for (filter, c) in &assets {
            render("yuv420p", Some((c, 0.6, *filter)));
            render("yuv420p", Some((c, 1.0, *filter)));
            if !filter {
                let zero = render("yuv420p", Some((c, 0.0, false)));
                let diff = zero.iter().zip(&plain).map(|(a, b)| a.abs_diff(*b) as u64).sum::<u64>()
                    as f64
                    / plain.len() as f64;
                assert!(diff < 2.0, "strength 0 changes the image ({diff:.2}): {c}");
            }
        }
    }

    /// Every built-in and every bundled package chain runs at strength 0, 0.5 and 1, also with
    /// odd dimensions and tiny images, and keeps the image size.
    #[test]
    fn every_chain_runs_at_odd_sizes_and_strengths() {
        assert!(EFFECTS.len() >= 30, "{}", EFFECTS.len());
        assert!(FILTERS.len() >= 20);
        chains_at_sizes(&[(161, 91, 2, &[0.0, 0.5, 1.0]), (64, 64, 1, &[1.0])]);
    }

    /// Ignored: the same in portrait Full HD, takes ~1 min and needs FFmpeg.
    /// `cargo test --release -p cl-fx -- --ignored every_chain_runs_in_portrait_full_hd`
    #[test]
    #[ignore]
    fn every_chain_runs_in_portrait_full_hd() {
        chains_at_sizes(&[(1080, 1920, 1, &[0.0, 1.0])]);
    }

    /// (width, height, frames, strengths)
    fn chains_at_sizes(sizes: &[(u32, u32, usize, &[f32])]) {
        if !cl_media::test_tools() {
            return;
        }
        // package chains of all bundled packages (lines as in `effects_pack_chains_run`)
        let mut packs = Vec::new();
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins");
        for d in std::fs::read_dir(root).unwrap().flatten() {
            let Ok(src) = std::fs::read_to_string(d.path().join("plugin.toml")) else { continue };
            let mut kind = "";
            for l in src.lines() {
                if let Some(k) = l.strip_prefix("kind = \"") {
                    kind = k.trim_end_matches('"');
                }
                if let Some(c) = l.strip_prefix("ffmpeg = \"") {
                    // fill further parameters (`{radius}` …) with 1, like a default value
                    let mut c = c.trim_end_matches('"').replace("{intensity}", "\u{1}");
                    while let (Some(a), Some(b)) = (c.find('{'), c.find('}')) {
                        c.replace_range(a..=b.max(a), "1");
                    }
                    packs.push((kind == "filter", c.replace('\u{1}', "{intensity}")));
                }
            }
        }
        assert!(!packs.is_empty());
        let o = ChainOpts { t_offset: 0.0, apply_reverse: false, lut: None, matte: None };
        let mut failed = Vec::new();
        // (width, height, frames, strengths)
        for &(w, h, n, strengths) in sizes {
            let src = Src { w, h, has_alpha: false };
            let mut graphs: Vec<(String, String)> = Vec::new();
            for &i in strengths {
                for def in EFFECTS.iter().chain(FILTERS.iter()) {
                    let mut fx = ClipFx::default();
                    let r = FxRef { id: def.id.into(), intensity: i, params: Default::default() };
                    if FILTERS.iter().any(|f| f.id == def.id) {
                        fx.filter = Some(r);
                    } else {
                        fx.effects.push(r);
                    }
                    graphs.push((format!("{} {i}", def.id), fx_graph(&fx, src, &o, "0:v", "f", "x").join(";")));
                }
                for (filter, c) in &packs {
                    let mut g = GraphBuilder::new("0:v", "e");
                    if *filter {
                        g.blended(&c.replace("{intensity}", "1.000"), i);
                    } else {
                        g.push(c.replace("{intensity}", &format!("{i:.3}")));
                    }
                    graphs.push((format!("{c} {i}"), g.finish("f").join(";")));
                }
            }
            for (name, g) in graphs {
                let graph = format!("{g};[f]format=rgba[out]");
                let out = cl_media::ffmpeg_command()
                    .unwrap()
                    .args(["-v", "error", "-nostdin", "-threads", "2", "-filter_complex_threads", "2", "-f", "lavfi", "-i"])
                    // testsrc2 rounds to even dimensions: generate larger, crop in RGBA
                    .arg(format!("testsrc2=s={}x{}:r=10:d={},format=rgba,crop={w}:{h}:0:0", w + 1, h + 1, n as f64 / 10.0))
                    .args(["-filter_complex", &graph, "-map", "[out]", "-frames:v", &n.to_string(), "-f", "rawvideo", "-"])
                    .stdin(Stdio::null())
                    .output()
                    .unwrap();
                if !out.status.success() {
                    failed.push(format!("{w}x{h} {name}: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("")));
                } else if out.stdout.len() != (w * h * 4) as usize * n {
                    failed.push(format!("{w}x{h} {name}: {} instead of {} bytes", out.stdout.len(), (w * h * 4) as usize * n));
                }
            }
        }
        assert!(failed.is_empty(), "{} Ketten scheitern:\n{}", failed.len(), failed.join("\n"));
    }

    /// Zoom pulse changes neither frame count nor duration – not even at 60 frames/s (with `zoompan`
    /// 1 s became two).
    #[test]
    fn zoom_pulse_keeps_frames_and_duration_at_60_fps() {
        if !cl_media::test_tools() {
            return;
        }
        let def = effect("zoom-pulse").unwrap();
        let chain = (def.build)(1.0, Src { w: 320, h: 180, has_alpha: false });
        let out = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=60:d=1", "-vf", &chain])
            .args(["-f", "framecrc", "-"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let text = String::from_utf8_lossy(&out.stdout);
        let rows: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(rows.len(), 60);
        // column 2 = timestamp of the last frame in 1/60 s
        let last: i64 = rows[59].split(',').nth(2).unwrap().trim().parse().unwrap();
        assert_eq!(last, 59);
        assert!(rows.iter().all(|l| l.contains("86400")), "image size stays 320x180");
    }

    fn matte_case(fx: &ClipFx, matte: bool) -> String {
        let o = ChainOpts { t_offset: 0.0, apply_reverse: false, lut: None, matte: matte.then(|| "mt".into()) };
        fx_graph(fx, SRC, &o, "0:v", "out", "u").join(";")
    }

    fn matte_cases() -> Vec<(&'static str, ClipFx, bool)> {
        let person = |p: PersonFx| ClipFx { person: Some(p), ..Default::default() };
        let chroma = ClipFx { chroma: Some(ChromaKey::default()), ..Default::default() };
        vec![
            ("matte", ClipFx::default(), true),
            ("blur", person(PersonFx::from_index(0)), true),
            ("color", person(PersonFx::from_index(1)), true),
            ("outline", person(PersonFx::from_index(2)), true),
            ("outline_transparent", person(PersonFx::Outline { color: [1, 2, 3], width: 0.5, transparent: true }), true),
            ("chroma", chroma.clone(), false),
            ("chroma_stabilize", ClipFx { stabilize: 0.5, ..chroma.clone() }, false),
            ("cutout_chroma", chroma.clone(), true),
            ("cutout_chroma_color", ClipFx { person: Some(PersonFx::from_index(1)), ..chroma.clone() }, true),
        ]
    }

    /// Identity edge values: the graphs stay byte-identical to the ones before `MatteEdge`.
    #[test]
    fn matte_graphs_unchanged_without_edge() {
        let got: Vec<String> = matte_cases().iter().map(|(n, fx, m)| format!("{n}: {}", matte_case(fx, *m))).collect();
        let want: Vec<&str> = vec![
            r#"matte: [0:v]setpts=PTS-STARTPTS+round(0.000000/TB),format=rgb24[uc1];[uc1][mt]alphamerge[ucm2];[ucm2]null[out]"#,
            r#"blur: [0:v]setpts=PTS-STARTPTS+round(0.000000/TB),format=rgb24[uc1];[uc1]split[upf2][upb3];[upb3]gblur=sigma=5.36[upbg6];[upf2][mt]alphamerge[upfg5];[upbg6][upfg5]overlay=format=auto,format=rgb24[upo7];[upo7]null[out]"#,
            r#"color: [0:v]setpts=PTS-STARTPTS+round(0.000000/TB),format=rgb24[uc1];[uc1]split[upf2][upb3];[upb3]drawbox=x=0:y=0:w=iw:h=ih:color=0x1878ff:t=fill[upbg6];[upf2][mt]alphamerge[upfg5];[upbg6][upfg5]overlay=format=auto,format=rgb24[upo7];[upo7]null[out]"#,
            r#"outline: [0:v]setpts=PTS-STARTPTS+round(0.000000/TB),format=rgb24[uc1];[uc1]split=3[upf2][upb3][upbs12];[mt]split[upm18][upm29];[upm29]format=gray,gblur=sigma=1.35,lut=y='min(255,val*4)'[upm4];[upb3]drawbox=x=0:y=0:w=iw:h=ih:color=0xffffff:t=fill[upc11];[upc11][upm4]alphamerge[upr10];[upbs12][upr10]overlay=format=auto[uprd13];[upf2][upm18]alphamerge[upfg5];[uprd13][upfg5]overlay=format=auto,format=rgb24[upo7];[upo7]null[out]"#,
            r#"outline_transparent: [0:v]setpts=PTS-STARTPTS+round(0.000000/TB),format=rgb24[uc1];[uc1]split[upf2][upb3];[mt]split[upm18][upm29];[upm29]format=gray,gblur=sigma=1.62,lut=y='min(255,val*4)'[upm4];[upb3]drawbox=x=0:y=0:w=iw:h=ih:color=0x010203:t=fill[upc11];[upc11][upm4]alphamerge[upr10];[upf2][upm18]alphamerge[upfg5];[upr10][upfg5]overlay=format=auto,format=rgba[upo7];[upo7]null[out]"#,
            r#"chroma: [0:v]setpts=PTS-STARTPTS+round(0.000000/TB),chromakey=color=0x00b140:similarity=0.180:blend=0.080,format=rgba,despill=type=green:mix=0.50:expand=0.1[uc1];[uc1]null[out]"#,
            r#"chroma_stabilize: [0:v]setpts=PTS-STARTPTS+round(0.000000/TB),deshake=rx=48:ry=48:edge=mirror,crop=152:86,scale=160:90,chromakey=color=0x00b140:similarity=0.180:blend=0.080,format=rgba,despill=type=green:mix=0.50:expand=0.1[uc1];[uc1]null[out]"#,
            r#"cutout_chroma: [0:v]setpts=PTS-STARTPTS+round(0.000000/TB),format=rgb24[uc1];[uc1][mt]alphamerge[ucm2];[ucm2]chromakey=color=0x00b140:similarity=0.180:blend=0.080,format=rgba,despill=type=green:mix=0.50:expand=0.1[uc3];[uc3]null[out]"#,
            r#"cutout_chroma_color: [0:v]setpts=PTS-STARTPTS+round(0.000000/TB),format=rgb24[uc1];[uc1]split[upf2][upb3];[upb3]drawbox=x=0:y=0:w=iw:h=ih:color=0x1878ff:t=fill[upbg6];[upf2][mt]alphamerge[upfg5];[upbg6][upfg5]overlay=format=auto,format=rgb24[upo7];[upo7]chromakey=color=0x00b140:similarity=0.180:blend=0.080,format=rgba,despill=type=green:mix=0.50:expand=0.1[uc8];[uc8]null[out]"#,
        ];
        assert_eq!(got, want, "\n{}", got.join("\n"));
    }

    /// Each edge value changes the mask once, before its first use, and every variant runs in FFmpeg.
    #[test]
    fn edge_values_change_the_matte() {
        let edges = [
            ("feather", MatteEdge { feather: 0.5, ..Default::default() }, "gblur=sigma=0.83"),
            ("grow", MatteEdge { shift: 0.5, ..Default::default() }, "lut=y='min(255,val*4)'"),
            ("shrink", MatteEdge { shift: -0.5, ..Default::default() }, "lut=y='max(0,val*4-765)'"),
            ("invert", MatteEdge { invert: true, ..Default::default() }, "negate"),
        ];
        for (name, base, m) in matte_cases() {
            let plain = matte_case(&base, m);
            for (e, edge, part) in edges {
                let fx = ClipFx { edge, ..base.clone() };
                let g = matte_case(&fx, m);
                if name == "chroma_stabilize" || name == "chroma" {
                    // without person mask the color key turns into the mask
                    assert!(g.contains("alphaextract[uckm"), "{name} + {e}: {g}");
                }
                assert_ne!(g, plain, "{name} + {e}");
                let edge_lines: Vec<&str> = g.split(';').filter(|l| l.contains("[ume")).collect();
                assert_eq!(edge_lines.len(), 2, "{name} + {e}: made once, used once: {g}");
                assert_eq!(edge_lines[0].matches(part).count(), 1, "{name} + {e}: {}", edge_lines[0]);
                if cl_media::test_tools() {
                    let (gl, extra): (String, &[&str]) = if m {
                        (format!("[1:v]format=gray[mt];{g}"), &["color=gray:s=160x90:r=15:d=0.6"])
                    } else {
                        (g.clone(), &[])
                    };
                    run_graph(&gl, extra).unwrap_or_else(|err| panic!("{name} + {e}: {err}\n{gl}"));
                }
            }
        }
    }

    /// Green screen with a background effect: the key becomes a gray mask that feeds the
    /// person effect (despill on the picture), after stabilization.
    #[test]
    fn chroma_with_background_effect_uses_the_mask_path() {
        let fx = ClipFx {
            chroma: Some(ChromaKey::default()),
            person: Some(PersonFx::from_index(1)),
            stabilize: 0.4,
            ..Default::default()
        };
        let g = matte_case(&fx, false);
        let i = |s: &str| g.find(s).unwrap_or_else(|| panic!("{s} missing: {g}"));
        assert!(i("deshake") < i("split[uck"), "{g}");
        assert!(g.contains("chromakey=color=0x00b140:similarity=0.180:blend=0.080,format=rgba,alphaextract[uckm"), "{g}");
        assert!(i("despill") < i("drawbox"), "{g}");
        assert_eq!(g.matches("chromakey").count(), 1, "{g}");
        assert!(g.contains("overlay=format=auto,format=rgb24"), "{g}");
        if cl_media::test_tools() {
            for k in 0..3 {
                let fx = ClipFx { person: Some(PersonFx::from_index(k)), ..fx.clone() };
                let g = matte_case(&fx, false);
                run_graph(&g, &[]).unwrap_or_else(|e| panic!("{e}\n{g}"));
            }
        }
    }

    /// Mask range against a source segment: jump into the file, lead and tail outside, no overlap.
    #[test]
    fn matte_window_cuts_the_mask_range() {
        let s = |a: f64| Time::from_secs_f64(a);
        let span = Some(MatteSpan { start: s(2.0), end: s(5.0) });
        let w = |a: f64, len: f64| matte_window(span, s(a), s(len));
        // whole file (as before): read from the segment start
        assert_eq!(
            matte_window(None, s(1.0), s(3.0)),
            Some(MatteWindow { seek: s(1.0), len: s(3.0), lead: Time::ZERO, tail: Time::ZERO })
        );
        // inside
        assert_eq!(w(3.0, 1.0), Some(MatteWindow { seek: s(1.0), len: s(1.0), lead: Time::ZERO, tail: Time::ZERO }));
        // starts before, ends after: lead and tail
        assert_eq!(w(1.0, 6.0), Some(MatteWindow { seek: Time::ZERO, len: s(3.0), lead: s(1.0), tail: s(2.0) }));
        // only touching or outside: not cut out
        assert_eq!(w(0.0, 2.0), None);
        assert_eq!(w(5.0, 1.0), None);
        // single frame (1 µs) at the first and just before the end
        assert_eq!(w(2.0, 0.000001).map(|w| w.seek), Some(Time::ZERO));
        assert!(w(4.999, 0.000001).is_some() && w(1.999, 0.000001).is_none());
        // padding: white keeps the picture, black when the edge inverts; nothing without padding
        assert_eq!(matte_pad(0.0, 0.0, false), None);
        assert_eq!(matte_pad(0.5, 0.0, false).unwrap(), "tpad=start_duration=0.500000:color=white");
        assert_eq!(matte_pad(0.0, 1.0, true).unwrap(), "tpad=stop_duration=1.000000:color=black");
    }

    /// The export warns about visible clips whose mask is named but not on disk (yet).
    #[test]
    fn missing_masks_lists_visible_clips_without_their_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Project::new("M");
        let m = MediaRef {
            id: MediaId::new(),
            path: dir.path().join("a.mp4"),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(1.0),
            width: 16,
            height: 16,
            fps: None,
            has_audio: false,
            audio_streams: 0,
            video_codec: None,
            audio_codec: None,
            has_alpha: false,
        };
        let mut t = Track::new(TrackKind::Video);
        let mut c = Clip::new(ClipContent::Media { media: m.id, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(1.0));
        p.media.push(m);
        let f = dir.path().join("mask.mkv");
        c.fx.cutout = Some(f.to_string_lossy().into_owned());
        t.clips.push(c);
        p.timeline.tracks.push(t);
        assert_eq!(missing_masks(&p), vec!["a.mp4".to_string()]);
        p.timeline.tracks[0].clips[0].disabled = true;
        assert!(missing_masks(&p).is_empty());
        p.timeline.tracks[0].clips[0].disabled = false;
        std::fs::write(&f, b"x").unwrap();
        assert!(missing_masks(&p).is_empty());
    }

    /// Preview of a clip whose mask covers only 0.5–1.0 s: before and after not cut out, inside
    /// cut out – single frames and a running stream; an inverted mask pads with black.
    #[test]
    fn preview_cuts_out_only_inside_the_mask_range() {
        if !cl_media::test_tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let make = |name: &str, src: &str, args: &[&str]| {
            let p = dir.path().join(name);
            let st = cl_media::ffmpeg_command().unwrap().args(["-v", "error", "-y", "-f", "lavfi", "-i", src]).args(args).arg(&p).status().unwrap();
            assert!(st.success(), "{name}");
            p
        };
        let v = make("v.mp4", "color=white:s=160x90:r=30:d=2", &["-pix_fmt", "yuv420p"]);
        let black = make("black.mkv", "color=black:s=160x90:r=30:d=0.5,format=gray", &["-c:v", "ffv1"]);
        let white = make("white.mkv", "color=white:s=160x90:r=30:d=0.5,format=gray", &["-c:v", "ffv1"]);
        let fps = Rational::new(30, 1);
        for (mask, invert) in [(&black, false), (&white, true)] {
            let mut c = Clip::new(ClipContent::Media { media: MediaId::new(), kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(2.0));
            c.fx.cutout = Some(mask.to_string_lossy().into_owned());
            c.fx.cutout_span = Some(MatteSpan { start: Time::from_secs_f64(0.5), end: Time::from_secs_f64(1.0) });
            c.fx.edge.invert = invert;
            for (t, alpha) in [(0.25, 255), (0.75, 0), (1.5, 255)] {
                // outside the range nothing to do: the fast path without a graph (opaque)
                let g = preview_graph(&c, 160, 90, t, None, None).unwrap_or_default();
                let f = cl_media::decode_frame_fx(&v, Time::from_secs_f64(t), 160, 90, Some(g.as_str()).filter(|g| !g.is_empty())).unwrap();
                assert_eq!(f.data[(45 * 160 + 80) * 4 + 3], alpha, "single frame at {t} (invert {invert})\n{g}");
            }
            let g = preview_graph(&c, 160, 90, 0.0, Some(fps), None).unwrap();
            let s = cl_media::FrameStream::open_fx(&v, Time::ZERO, 160, 90, fps, Some(&g)).unwrap();
            let mut a = vec![];
            while let Some(f) = s.next_blocking() {
                a.push(f.data[(45 * 160 + 80) * 4 + 3]);
            }
            assert!(a.len() >= 58, "stream ran through: {} frames\n{g}", a.len());
            // frame 15 is the first of the mask, frame 30 the first after it
            let want = |i: usize| if (15..30).contains(&i) { 0 } else { 255 };
            let bad: Vec<usize> = (0..a.len()).filter(|&i| a[i] != want(i)).collect();
            assert!(bad.is_empty(), "stream frames off: {bad:?} (invert {invert})\n{a:?}\n{g}");
        }
    }
}
