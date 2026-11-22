//! Filter graph construction (phase 2): every clip becomes its own image (sprite) with an effect chain
//! (cl-fx) and expressions for position, size, rotation and opacity from keyframes and
//! animations. Clips with a transition are joined into one stream via `xfade`; the
//! total length stays the same (the picture at the cuts is held).

use std::path::Path;

use cl_core::*;
use cl_fx::{atempo_chain, fx_graph, piecewise_expr, speed_setpts, ChainOpts, Src};

use crate::{default_font, f, ExportError, FontResolver, Result};

/// Filter graph under construction.
pub(crate) struct Ctx<'a> {
    pub w: u32,
    pub h: u32,
    pub fps: String,
    pub start: Time,
    pub workdir: &'a Path,
    pub fonts: FontResolver<'a>,
    pub inputs: Vec<String>,
    pub n_input: usize,
    pub graph: Vec<String>,
    pub uniq: usize,
    /// Transparent background instead of black
    pub alpha: bool,
    /// Prepare shader transitions (off inside the pre-pass itself)
    pub shader: bool,
    /// Pre-passes for shader transitions (run before the export, see [`ShaderPass`])
    pub passes: Vec<ShaderPass>,
    /// Number of the first pre-pass (sections share a work folder)
    pub pass_base: usize,
    /// Section of a large export: long clips jump via `-ss` close to the
    /// section start (see [`lead_skip`]) instead of decoding from the clip start
    pub lead: bool,
}

/// Pre-pass of a shader transition. One FFmpeg reads the last frames of clip A and the first
/// of clip B (same clip chains as in the main graph, side by side as RGBA), cl-gpu blends them,
/// a second FFmpeg writes them losslessly (FFV1) to `out_file`. In the main graph `block`
/// replaces the result of the `xfade` (fallback) exactly in this window; if the pre-pass fails,
/// `fallback` takes its place and the `xfade` stays.
pub(crate) struct ShaderPass {
    pub inputs: Vec<String>,
    pub graph_file: String,
    pub out_file: String,
    pub w: u32,
    pub h: u32,
    pub fps: String,
    pub shader: String,
    pub params: Vec<f32>,
    /// Progress per frame (like `xfade`: from the frame time)
    pub progress: Vec<f32>,
    pub block: String,
    pub fallback: String,
}

/// Frame rate as a number (`30000/1001` → 29.97).
fn fps_value(fps: &str) -> f64 {
    let mut it = fps.split('/');
    let n: f64 = it.next().and_then(|x| x.trim().parse().ok()).unwrap_or(30.0);
    let d: f64 = it.next().and_then(|x| x.trim().parse().ok()).unwrap_or(1.0);
    if n > 0.0 && d > 0.0 {
        n / d
    } else {
        30.0
    }
}

/// Frames of the transition window in the joined stream: (first frame, count). `xfade` blends the
/// frames from the first frame time ≥ `off` up to and including `off + d` (frames lie on n/fps).
pub(crate) fn window_frames(off: f64, d: f64, fps: f64) -> (usize, usize) {
    let i0 = (off * fps - 1e-4).ceil().max(0.0) as usize;
    let last = ((off + d) * fps + 1e-4).floor().max(i0 as f64) as usize;
    (i0, last - i0 + 1)
}

impl Ctx<'_> {
    fn input(&mut self, args: Vec<String>) -> usize {
        self.inputs.extend(args);
        self.n_input += 1;
        self.n_input - 1
    }
    fn label(&mut self, p: &str) -> String {
        self.uniq += 1;
        format!("{p}{}", self.uniq)
    }
}

/// Source range of a media clip: (start, length) in the source.
pub(crate) fn source_segment(c: &Clip) -> (Time, Time) {
    let a = c.source_time(c.start);
    let b = c.source_time(c.end());
    (a.min(b), Time((b.0 - a.0).abs()))
}

/// Evaluated appearance, sampled over the clip length (for expressions).
fn samples(c: &Clip, w: f32, h: f32) -> Vec<(f64, Evaluated)> {
    let dur = c.duration.as_secs_f64();
    let n = ((dur * 12.0).ceil() as usize).clamp(2, 240);
    (0..=n)
        .map(|k| {
            let t = dur * k as f64 / n as f64;
            (t, c.eval_at(Time::from_secs_f64(t), w, h))
        })
        .collect()
}

/// Time-dependent value as an expression in `var` (constant if it doesn't change).
fn value_expr(s: &[(f64, Evaluated)], get: impl Fn(&Evaluated) -> f32, var: &str) -> String {
    let pts: Vec<(f64, f64)> = s.iter().map(|(t, e)| (*t, get(e) as f64)).collect();
    let first = pts[0].1;
    if pts.iter().all(|p| (p.1 - first).abs() < 1e-4) {
        return format!("{first:.5}");
    }
    piecewise_expr(&pts, var)
}

fn is_const(e: &str) -> bool {
    e.parse::<f64>().is_ok()
}

/// Clockwise rotation (degrees) with transparent corners and an enlarged area.
pub(crate) fn rotate_filter(deg: f32) -> String {
    if deg.abs() < 0.01 {
        return String::new();
    }
    let rad = deg.to_radians();
    format!("rotate={rad:.6}:c=none:ow=rotw({rad:.6}):oh=roth({rad:.6})")
}

/// A prepared image with a transparent border. Time 0 = clip start.
/// `x`/`y` are expressions in `TT` (clip time).
pub(crate) struct Sprite {
    label: String,
    x: String,
    y: String,
}

/// Append opacity, scale and rotation (static or animated).
fn post_transform(
    ctx: &mut Ctx,
    input: &str,
    base_w: u32,
    base_h: u32,
    s: &[(f64, Evaluated)],
) -> String {
    let mut chain = vec!["format=rgba".to_string()];
    let op = value_expr(s, |e| e.opacity, "T");
    if is_const(&op) {
        let v: f32 = op.parse().unwrap_or(1.0);
        if v < 0.999 {
            chain.push(format!("colorchannelmixer=aa={v:.3}"));
        }
    } else {
        chain.push(format!(
            "geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='alpha(X,Y)*clip({op},0,1)'"
        ));
    }
    // FFmpeg rejects images over ~268 M pixels (the export aborted at zoom 20× on 1080p):
    // diagonal at most 14,000 px.
    // shortcut: larger zoom is capped instead of first cropping to the visible part; crop if someone needs it
    let cap = 14_000.0 / (base_w as f64).hypot(base_h as f64).max(1.0);
    let sc = match value_expr(s, |e| e.scale, "t") {
        v if is_const(&v) => format!("{:.5}", v.parse::<f64>().unwrap_or(1.0).min(cap)),
        v => format!("min({cap:.4},{v})"),
    };
    let rot = value_expr(s, |e| e.rotation_deg, "t");
    let max_s = (s
        .iter()
        .map(|(_, e)| e.scale)
        .fold(0.0f32, f32::max)
        .max(0.01) as f64)
        .min(cap);
    // fixed area for animated size/rotation: newer FFmpeg versions reject
    // changing frame sizes in subsequent filters (rotate, overlay).
    let d = (((base_w as f64 * max_s).hypot(base_h as f64 * max_s)).ceil() as u32 + 2) & !1;
    if is_const(&sc) {
        let v: f64 = sc.parse().unwrap_or(1.0);
        let sw = ((base_w as f64 * v).round() as u32).max(2) & !1;
        let sh = ((base_h as f64 * v).round() as u32).max(2) & !1;
        if (sw, sh) != (base_w, base_h) {
            chain.push(format!("scale={sw}:{sh}:flags=bicubic"));
        }
        if is_const(&rot) {
            chain.push(rotate_filter(rot.parse().unwrap_or(0.0)));
        } else {
            chain.push(format!("rotate=a='({rot})*PI/180':c=none:ow={d}:oh={d}"));
        }
    } else {
        chain.push(format!("scale=w='max(2,trunc({base_w}*({sc})/2)*2)':h='max(2,trunc({base_h}*({sc})/2)*2)':eval=frame:flags=bicubic"));
        chain.push(format!(
            "pad=w={d}:h={d}:x='(ow-iw)/2':y='(oh-ih)/2':color=black@0:eval=frame"
        ));
        if is_const(&rot) {
            let r: f32 = rot.parse().unwrap_or(0.0);
            if r.abs() > 0.01 {
                chain.push(format!("rotate={:.6}:c=none:ow={d}:oh={d}", r.to_radians()));
            }
        } else {
            chain.push(format!("rotate=a='({rot})*PI/180':c=none:ow={d}:oh={d}"));
        }
    }
    chain.retain(|f| !f.is_empty());
    let out = ctx.label("sp");
    ctx.graph
        .push(format!("[{input}]{}[{out}]", chain.join(",")));
    out
}

/// Filters whose result depends on earlier frames (random per frame, averaging over frames,
/// noise filters with memory) or that assign their own timestamps from 0 (`zoompan`): with them
/// a section must not enter mid-clip.
const STATEFUL: &[&str] = &["noise", "tmix", "deflicker", "hqdn3d", "random", "minterpolate", "deshake", "lagfun", "atadenoise", "tblend", "sendcmd", "zoompan"];

/// Whole seconds a section (start `start`) may skip from the start of a clip that begins earlier
/// (input via `-ss` closer instead of decoding from the clip start), with
/// at least 1 s lead for decoder and frame rate conversion. 0 for clips whose picture or
/// audio depends on the history (speed, reverse, speed curve, slow motion, chroma key).
pub(crate) fn lead_skip(c: &Clip, start: Time) -> f64 {
    let into = (start - c.start).as_secs_f64();
    let plain = (c.speed - 1.0).abs() < 1e-6
        && !c.fx.reverse
        && c.fx.speed_curve.is_none()
        && !c.fx.smooth_slowmo
        && c.fx.cutout.is_none()
        && c.fx.person.is_none();
    if !plain || into < 2.0 {
        return 0.0;
    }
    (into - 1.0).floor()
}

/// [`lead_skip`] for the picture: rounded down to a common multiple of source frame, project frame
/// and 1 µs. Only then does the jump hit a source frame (same timestamps as from the clip start) and the
/// project's frame grid (`fps` picks the same frames); whole seconds aren't enough at 29.97 fps.
/// Without a known source rate no jump.
pub(crate) fn video_skip(c: &Clip, src: Option<Rational>, project: Rational, start: Time) -> f64 {
    fn gcd(a: u128, b: u128) -> u128 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let skip = lead_skip(c, start);
    let Some(src) = src.filter(|r| r.num > 0 && r.den > 0 && skip > 0.0) else {
        return 0.0;
    };
    // frame duration den/num reduced; lcm of fractions = lcm of numerators / gcd of denominators
    let reduced = |r: Rational| {
        let g = gcd(r.num as u128, r.den.max(1) as u128).max(1);
        (r.den.max(1) as u128 / g, r.num.max(1) as u128 / g)
    };
    let ((ds, ns), (dp, np)) = (reduced(src), reduced(project));
    let step_us = ds / gcd(ds, dp) * dp * 1_000_000 / gcd(gcd(ns, np), 1_000_000);
    let skip_us = (skip * 1e6) as u128;
    (skip_us / step_us * step_us) as f64 / 1e6
}

/// Does the clip's effect chain have filters with memory ([`STATEFUL`])?
pub(crate) fn fx_has_memory(fx: &ClipFx) -> bool {
    let opts = ChainOpts { t_offset: 0.0, apply_reverse: false, lut: None, matte: None };
    let g = fx_graph(fx, Src { w: 64, h: 64, has_alpha: false }, &opts, "i", "o", "u").join(";");
    STATEFUL.iter().any(|s| g.contains(s)) || counts_frames(&g)
}

/// Does an expression use the frame counter from filter start (`n`/`N` in eq, hue, geq, crop, overlay …,
/// e.g. from package chains)? After a jump it counts from the section instead of from the clip.
pub(crate) fn counts_frames(g: &str) -> bool {
    g.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').any(|w| w == "n" || w == "N")
}

fn media_sprite(ctx: &mut Ctx, c: &Clip, m: &MediaRef, image: bool, skip: f64) -> Result<Sprite> {
    let (w, h) = (ctx.w, ctx.h);
    let dur = c.duration.as_secs_f64();
    let path = m.path.to_string_lossy().into_owned();
    let (_, seg_len) = source_segment(c);
    // animated image (GIF/APNG): play once and hold the last frame, or loop;
    // only still-image readers know `-loop 1`
    let anim = if image { cl_media::animation(&m.path) } else { None };
    let i = if image {
        let looping: &[&str] = match &anim {
            None => &["-loop", "1"],
            Some(a) if a.looping => &["-stream_loop", "-1"],
            Some(_) => &[],
        };
        ctx.input(looping.iter().map(|s| s.to_string()).chain([
            "-t".into(),
            f(dur + 0.1),
            "-i".into(),
            path,
        ]).collect())
    } else {
        let (seg_in, _) = source_segment(c);
        ctx.input(cl_media::decoder_args(&m.path).into_iter().map(String::from).chain([
            "-ss".into(),
            f(seg_in.as_secs_f64() + skip),
            "-t".into(),
            f((seg_len.as_secs_f64() - skip).max(0.04)),
            "-i".into(),
            path,
        ]).collect())
    };
    // base size: fitted as in the player (contain)
    let fit = (w as f64 / m.width.max(1) as f64).min(h as f64 / m.height.max(1) as f64);
    let bw = ((m.width as f64 * fit).round() as u32).max(2) & !1;
    let bh = ((m.height as f64 * fit).round() as u32).max(2) & !1;
    // skipped start: timestamps stay clip time (animations, keyframes, effects)
    let mut pre = vec![if skip > 0.0 { format!("setpts=PTS-STARTPTS+round({}/TB)", f(skip)) } else { "setpts=PTS-STARTPTS".to_string() }];
    if !image {
        if c.fx.reverse {
            pre.push("reverse".into());
            pre.push("setpts=PTS-STARTPTS".into());
        }
        let sp = speed_setpts(
            c.speed,
            c.fx.speed_curve.as_deref(),
            seg_len.as_secs_f64(),
            dur,
        );
        if !sp.is_empty() {
            pre.push(sp);
        }
        // smooth slow motion: in-between frames via motion estimation instead of doubled frames
        if c.fx.smooth_slowmo && (c.speed < 0.999 || c.fx.speed_curve.is_some()) {
            pre.push(format!(
                "minterpolate=fps={}:mi_mode=mci:mc_mode=aobmc:me_mode=bidir:vsbmc=1",
                ctx.fps
            ));
        }
    }
    if skip > 0.0 {
        // first frame exactly at `skip` (without jump: at 0), frames after it as without jump
        pre.push(format!("fps={}:start_time={}", ctx.fps, f(skip)));
    } else {
        pre.push(format!("fps={}", ctx.fps));
    }
    // only after `fps`: FFmpeg 7 forgets the frame rate after `setpts`, `tpad` would then hold nothing.
    // videos shorter than the clip (rounding) hold the last frame
    if !image {
        pre.push(format!("tpad=stop_mode=clone:stop_duration={}", f(0.5)));
    } else if anim.is_some() {
        pre.push(format!("tpad=stop_mode=clone:stop_duration={}", f(dur + 0.1)));
    }
    pre.push(format!("scale={bw}:{bh}:flags=bicubic"));
    // one frame more than the clip: `trim` rounds to whole frames, `place` may need ceil(dur·fps)
    // frames; the extra one is never shown
    let keep = dur + 1.0 / fps_value(&ctx.fps);
    pre.push(if skip > 0.0 { format!("trim=end={}", f(keep)) } else { format!("trim=duration={}", f(keep)) });
    let pl = ctx.label("pre");
    ctx.graph.push(format!("[{i}:v]{}[{pl}]", pre.join(",")));
    // copy the LUT into the work folder (no path escapes in the graph)
    let lut = match &c.fx.lut {
        Some(l) if Path::new(l).is_file() => {
            let name = format!("{}.cube", ctx.label("lut"));
            std::fs::copy(l, ctx.workdir.join(&name))?;
            Some(name)
        }
        _ => None,
    };
    // remove background: mask video with an identical time axis as a second input
    let mut matte = None;
    if let Some(mp) =
        c.fx.cutout
            .as_ref()
            .filter(|p| !image && Path::new(p).is_file())
    {
        let (seg_in, _) = source_segment(c);
        let mi = ctx.input(vec![
            "-ss".into(),
            f(seg_in.as_secs_f64()),
            "-t".into(),
            f(seg_len.as_secs_f64().max(0.04)),
            "-i".into(),
            mp.clone(),
        ]);
        let mut mpre: Vec<String> = pre
            .iter()
            .filter(|p| !p.starts_with("scale="))
            .cloned()
            .collect();
        mpre.retain(|p| !p.starts_with("trim="));
        mpre.push(format!("scale={bw}:{bh}:flags=bilinear"));
        mpre.push(format!("trim=duration={}", f(keep)));
        mpre.push("format=gray".into());
        let ml = ctx.label("mt");
        ctx.graph.push(format!("[{mi}:v]{}[{ml}]", mpre.join(",")));
        matte = Some(ml);
    }
    let fl = ctx.label("fx");
    let u = ctx.label("u");
    let opts = ChainOpts {
        t_offset: skip,
        apply_reverse: false,
        lut,
        matte,
    };
    let mut fx = c.fx.clone();
    fx.reverse = false;
    fx.censor = c.censor_in_clip_time(None);
    ctx.graph.extend(fx_graph(
        &fx,
        Src {
            w: bw,
            h: bh,
            has_alpha: image || m.has_alpha,
        },
        &opts,
        &pl,
        &fl,
        &u,
    ));
    let fl = crop_layer(ctx, &fl, bw, bh, c.crop);
    let s = samples(c, w as f32, h as f32);
    let label = post_transform(ctx, &fl, bw, bh, &s);
    Ok(Sprite {
        label,
        x: value_expr(&s, |e| e.x, "TT"),
        y: value_expr(&s, |e| e.y, "TT"),
    })
}

/// Crop after effects and mask (like the preview, which crops the finished texture), before
/// scaling and rotation: the edges become transparent, the image size stays so the rest
/// stays in place and pivot and position remain the same.
fn crop_layer(ctx: &mut Ctx, input: &str, bw: u32, bh: u32, k: Crop) -> String {
    if k.is_none() {
        return input.to_string();
    }
    let px = |v: f32, n: u32| ((v * n as f32).round() as u32).min(n - 1);
    let (x, y) = (px(k.left, bw), px(k.top, bh));
    let cw = bw.saturating_sub(x + px(k.right, bw)).max(1);
    let ch = bh.saturating_sub(y + px(k.bottom, bh)).max(1);
    let out = ctx.label("crop");
    // rgba before `crop`: otherwise crop rounds the offset to even pixels with 4:2:0
    ctx.graph.push(format!(
        "[{input}]format=rgba,crop={cw}:{ch}:{x}:{y},pad={bw}:{bh}:{x}:{y}:color=black@0[{out}]"
    ));
    out
}

/// Slant of italic text: top edge shifted right by ¼ of the height (like egui in the preview).
const ITALIC_SHEAR: f32 = 0.25;

fn text_sprite(ctx: &mut Ctx, c: &Clip, st: &TextStyle) -> Result<Option<Sprite>> {
    let Some((font_path, idx)) =
        (ctx.fonts)(st.font_family.as_deref()).or_else(|| default_font().map(|p| (p, 0)))
    else {
        return Ok(None);
    };
    let n = ctx.label("t");
    // one copy per font file, not per text clip (1000 subtitles × 15 MB font)
    let key = font_path.to_string_lossy();
    let h = key.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    let font_file = format!(
        "font_{h:016x}.{}",
        font_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("ttf")
    );
    if !ctx.workdir.join(&font_file).is_file() {
        std::fs::copy(&font_path, ctx.workdir.join(&font_file))?;
    }
    let data = std::fs::read(&font_path).unwrap_or_default();
    // project file/MCP can contain 1e6 (MCP sets at most 600)
    let size = st.size.clamp(1.0, 1000.0);
    let mut txt = if st.uppercase {
        st.text.to_uppercase()
    } else {
        st.text.clone()
    };
    if st.auto_wrap {
        txt = cl_fonts::wrap_text(
            &data,
            idx,
            size,
            &txt,
            ctx.w as f32 * 0.9 / c.transform.scale.max(0.05),
        );
    }
    let lines: Vec<&str> = txt.split('\n').collect();
    let width = |l: &str| cl_fonts::text_width(&data, idx, size, l).unwrap_or(size * 0.6 * l.chars().count() as f32);
    let tw = lines.iter().map(|l| width(l)).fold(0.0f32, f32::max);
    let ls = st.line_spacing.max(0.5);
    // line spacing like the preview (egui: line height 1.25 × size × line spacing)
    let pitch = size * 1.25 * ls;
    let (asc, desc) = cl_fonts::v_metrics(&data, idx, size).unwrap_or((size * 0.9, size * 0.25));
    let block = pitch * lines.len() as f32;
    let th = (size * 1.3 * ls * lines.len() as f32).max(block);
    let outline = st.outline.map(|o| o.0).unwrap_or(0.0);
    // a set shadow style may point far away: room for its offset (`None` keeps the old pad)
    let shadow = st.shadow_style.filter(|_| st.shadow).map(|s| (s.offset(), s.color));
    let reach = shadow.map_or(0, |((dx, dy), _)| dx.abs().max(dy.abs()));
    // soft shadow: its own blurred layer under the text (hard ones stay drawtext's own shadow);
    // the blur fades out within about 3 sigma
    let soft = st.shadow_style.filter(|s| st.shadow && s.sigma() > 0.0);
    let fade = soft.map_or(0.0, |s| (3.0 * s.sigma()).ceil());
    let pad = (size * 0.4 + outline * 2.0 + 8.0).ceil() + reach as f32 + fade;
    // very long lines without wrap: limit the area (the rest is outside the image anyway)
    // italic: shearing needs room at the sides
    let slant = if st.italic { ITALIC_SHEAR * th } else { 0.0 };
    let lw = (((tw + pad * 2.0 + slant).ceil() as u32 + 1) & !1).min(8192);
    let lh = (((th + pad * 2.0).ceil() as u32 + 1) & !1).min(8192);
    let (lwf, lhf) = (lw as f32, lh as f32);
    let rgba = |c: [u8; 4]| {
        format!(
            "0x{:02x}{:02x}{:02x}@{:.3}",
            c[0],
            c[1],
            c[2],
            c[3] as f32 / 255.0
        )
    };
    // every line with its own drawtext at a self-computed baseline: FFmpeg 8.1 draws a
    // line break in `textfile` as a box (and shifts the line), and `y_align=baseline`
    // does not depend on the letters of the line. Lines centered in the block, font centered in the line.
    let top = lhf / 2.0 - block / 2.0;
    let mut files = Vec::new();
    for (i, line) in lines.iter().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let file = format!("text_{n}_{i}.txt");
        std::fs::write(ctx.workdir.join(&file), line.as_bytes())?;
        let x = match st.align {
            a if a < 0 => format!("{:.1}", (lwf - tw) / 2.0),
            a if a > 0 => format!("{:.1}-text_w", (lwf + tw) / 2.0),
            _ => "(w-text_w)/2".to_string(),
        };
        let baseline = top + i as f32 * pitch + (pitch - asc - desc) / 2.0 + asc;
        files.push((file, x, format!("{baseline:.1}")));
    }
    let core = |dx: f32, col: [u8; 4], file: &str, x: &str, y: &str| {
        format!(
            "drawtext=fontfile={font_file}:textfile={file}:expansion=none:fontsize={size:.1}:fontcolor={}:x={x}+{dx:.1}:y={y}:y_align=baseline",
            rgba(col),
        )
    };
    // drawtext blends its shadow's alpha into the transparent layer with itself (opacity a comes out
    // as a², measured with FFmpeg 7.1 and 8.1): write √a there. On an opaque box that holds the whole
    // shadow the colour is right, but the box's alpha drops to 1 − a(1 − a) there (60 %: 0.76, so over
    // grey video 58 % instead of 60 %, over black 70 %; √a would give 73 % / 81 %). On a translucent box
    // √a is the closer value (white 50 % box: 30 % → 36 %, 60 % → 59 %).
    let on_box = st.background.is_some_and(|bg| bg[3] == 255) && {
        let (dx, dy) = st.shadow_style.map_or((3, 3), |s| s.offset());
        dx.abs().max(dy.abs()) as f32 <= (size * 0.25).max(4.0)
    };
    let shadow_alpha = |a: f32| if on_box { a } else { a.sqrt() };
    let draw_one = |col: [u8; 4], file: &str, x: &str, y: &str| {
        let mut dt = core(0.0, col, file, x, y);
        if let Some((bw, col)) = st.outline {
            dt.push_str(&format!(":borderw={bw:.1}:bordercolor={}", rgba(col)));
        }
        match shadow {
            Some(_) if soft.is_some() => {}
            // not on the bold second print: drawtext would paint that shadow over the first print
            // (and double the opacity where both overlap); the preview leaves it out the same way
            Some(((dx, dy), c)) => dt.push_str(&format!(
                ":shadowx={dx}:shadowy={dy}:shadowcolor=0x{:02x}{:02x}{:02x}@{:.3}",
                c[0],
                c[1],
                c[2],
                shadow_alpha(c[3] as f32 / 255.0)
            )),
            None if st.shadow => dt.push_str(&format!(":shadowx=3:shadowy=3:shadowcolor=black@{}", if on_box { "0.6".into() } else { format!("{:.3}", shadow_alpha(0.6)) })),
            None => {}
        }
        // bold and italic like the preview (`draw_text` in the player) from the style's switches, not
        // from the font file's weight: a second imprint offset by 3 % of the size, shear
        if st.bold {
            dt.push(',');
            dt.push_str(&core(size * 0.03, col, file, x, y));
        }
        dt
    };
    // box: one box around all lines (widest line × block), padding as before
    let backdrop = st.background.map(|bg| {
        let b = (size * 0.25).max(4.0);
        let x = match st.align {
            a if a < 0 => (lwf - tw) / 2.0,
            a if a > 0 => (lwf + tw) / 2.0 - tw,
            _ => (lwf - tw) / 2.0,
        };
        let r = |v: f32| v.round().max(0.0) as i64;
        let (bx, by) = (r(x - b), r(top + (pitch - asc - desc) / 2.0 - b));
        let bw = r(tw + 2.0 * b).min(lw as i64 - bx);
        let bh = r(block - (pitch - asc - desc) + 2.0 * b).min(lh as i64 - by);
        format!("drawbox=x={bx}:y={by}:w={bw}:h={bh}:color={}:t=fill:replace=1", rgba(bg))
    });
    let draw = |col: [u8; 4]| {
        // soft shadow: the box lies under the shadow layer
        let mut v: Vec<String> = backdrop.clone().filter(|_| soft.is_none()).into_iter().collect();
        v.extend(files.iter().map(|(file, x, y)| draw_one(col, file, x, y)));
        if v.is_empty() {
            "null".to_string()
        } else {
            v.join(",")
        }
    };
    let shear = if st.italic { format!(",shear=shx={ITALIC_SHEAR}:fillcolor=black@0") } else { String::new() };
    let dur = c.duration.as_secs_f64();
    let src = ctx.label("tx");
    // soft shadow: the text is composed on its own and laid over (box and) shadow, then sheared as a whole
    let (out, shear_text) = match soft {
        Some(_) => (ctx.label("st"), String::new()),
        None => (src.clone(), shear.clone()),
    };
    // one frame more, as in `media_sprite`
    let layer = format!("color=c=black@0.0:s={lw}x{lh}:r={}:d={},format=rgba", ctx.fps, f(dur + 1.0 / fps_value(&ctx.fps)));
    let boxes = match &st.karaoke {
        Some(k) => {
            let geo = Geo { data: &data, idx, size, tw, lw, lh, pitch, align: st.align, bold: st.bold };
            karaoke_states(&geo, &txt, &k.starts(&st.text, c.duration), k.fill, dur)
        }
        None => Vec::new(),
    };
    match &st.karaoke {
        Some(k) if !boxes.is_empty() => {
            // karaoke: cut out the highlighted version per word and overlay it for its time
            let base = ctx.label("kb");
            ctx.graph.push(format!("{layer},{}[{base}]", draw(st.color)));
            let parts: Vec<String> = boxes.iter().map(|_| ctx.label("kp")).collect();
            ctx.graph.push(format!(
                "{layer},{},split={}{}",
                draw(k.color),
                parts.len(),
                parts.iter().map(|p| format!("[{p}]")).collect::<String>()
            ));
            let mut cur = base;
            for (b, part) in boxes.iter().zip(parts) {
                let (cut, out) = (ctx.label("kc"), ctx.label("ko"));
                ctx.graph.push(format!("[{part}]crop={}:{}:{}:{}[{cut}]", b.w, b.h, b.x, b.y));
                ctx.graph.push(format!(
                    "[{cur}][{cut}]overlay={}:{}:format=rgb:enable='{}'[{out}]",
                    b.x, b.y, b.enable
                ));
                cur = out;
            }
            ctx.graph.push(format!("[{cur}]null{shear_text}[{out}]"));
        }
        _ => ctx.graph.push(format!("{layer},{}{shear_text}[{out}]", draw(st.color))),
    }
    if let Some(sh) = soft {
        // the text in the shadow colour, opaque (outline as border, bold print included, so overlaps
        // do not add up), blurred, then the shadow's opacity; the layer's own colour is the shadow
        // colour so the blur only fades the alpha
        let ((dx, dy), [r, g, b, a]) = (sh.offset(), sh.color);
        let ink = [r, g, b, 255];
        let prints: Vec<String> = files
            .iter()
            .map(|(file, x, y)| {
                let y = format!("{y}{dy:+}");
                let mut dt = core(dx as f32, ink, file, x, &y);
                if let Some((bw, _)) = st.outline {
                    dt.push_str(&format!(":borderw={bw:.1}:bordercolor={}", rgba(ink)));
                }
                if st.bold {
                    dt.push(',');
                    dt.push_str(&core(dx as f32 + size * 0.03, ink, file, x, &y));
                }
                dt
            })
            .collect();
        let sl = ctx.label("sl");
        ctx.graph.push(format!(
            "color=c=0x{r:02x}{g:02x}{b:02x}@0.0:s={lw}x{lh}:r={}:d={},format=rgba{}{},gblur=sigma={:.2},colorchannelmixer=aa={:.3}[{sl}]",
            ctx.fps,
            f(dur + 1.0 / fps_value(&ctx.fps)),
            if prints.is_empty() { "" } else { "," },
            prints.join(","),
            sh.sigma(),
            a as f32 / 255.0,
        ));
        // box: the shadow of the text only lies on it, as with the hard shadow
        let under = match &backdrop {
            Some(bx) => {
                let u = ctx.label("su");
                ctx.graph.push(format!("{layer},{bx}[{u}]"));
                ctx.graph.push(format!("[{u}][{sl}]overlay=0:0:format=rgb[{u}b]"));
                format!("{u}b")
            }
            None => sl,
        };
        ctx.graph.push(format!("[{under}][{out}]overlay=0:0:format=rgb{shear}[{src}]"));
    }
    let s = samples(c, ctx.w as f32, ctx.h as f32);
    let label = post_transform(ctx, &src, lw, lh, &s);
    Ok(Some(Sprite {
        label,
        x: value_expr(&s, |e| e.x, "TT"),
        y: value_expr(&s, |e| e.y, "TT"),
    }))
}

/// Dimensions of the text area for the karaoke cut-outs (as `drawtext` sets the text)
struct Geo<'a> {
    data: &'a [u8],
    idx: u32,
    size: f32,
    /// widest line (estimated like the area size)
    tw: f32,
    lw: u32,
    lh: u32,
    /// Line spacing (as in `text_sprite`)
    pitch: f32,
    align: i8,
    bold: bool,
}

/// Cut-out of the highlighted version and when it applies
#[derive(Debug)]
struct KBox {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    enable: String,
}

/// Rectangle per word of `txt` (already wrapped) on the text area, with the time in which it
/// is highlighted (`starts` relative to the clip start, like `karaoke_word`). The rectangles reach
/// to the middle of the spaces and the line gaps: both versions differ only in
/// the fill color of the letters, so small measuring errors don't show.
fn karaoke_states(g: &Geo, txt: &str, starts: &[Time], fill: bool, dur: f64) -> Vec<KBox> {
    let width = |s: &str| cl_fonts::text_width(g.data, g.idx, g.size, s).unwrap_or(g.size * 0.6 * s.chars().count() as f32);
    let space = width(" ");
    let bold = if g.bold { g.size * 0.03 } else { 0.0 };
    let lines: Vec<&str> = txt.split('\n').collect();
    let n = lines.len() as f32;
    let (lw, lh) = (g.lw as f32, g.lh as f32);
    let mut out = Vec::new();
    let mut i = 0;
    for (li, line) in lines.iter().enumerate() {
        let line_w = width(line);
        let left = match g.align {
            a if a < 0 => (lw - g.tw) / 2.0,
            a if a > 0 => (lw + g.tw) / 2.0 - line_w,
            _ => (lw - line_w) / 2.0,
        };
        let cy = lh / 2.0 + (li as f32 - (n - 1.0) / 2.0) * g.pitch;
        let y0 = if li == 0 { 0.0 } else { (cy - g.pitch / 2.0).max(0.0) };
        let y1 = if li + 1 == lines.len() { lh } else { (cy + g.pitch / 2.0).min(lh) };
        for r in word_ranges(line) {
            let Some(&a) = starts.get(i) else { break };
            let next = starts.get(i + 1).copied();
            i += 1;
            let a = a.as_secs_f64();
            let b = if fill { None } else { next.map(|b| b.as_secs_f64()) };
            if a >= dur || b.is_some_and(|b| b <= 0.0 || b <= a) {
                continue;
            }
            let x0 = (left + width(&line[..r.start]) - space / 2.0).max(0.0);
            let x1 = (left + width(&line[..r.end]) + space / 2.0 + bold).min(lw);
            let (x, y) = ((x0.floor() as u32).min(g.lw - 1), (y0.floor() as u32).min(g.lh - 1));
            let w = (x1.ceil() as u32).saturating_sub(x).clamp(1, g.lw - x);
            let h = (y1.ceil() as u32).saturating_sub(y).clamp(1, g.lh - y);
            let enable = match b {
                Some(b) => format!("gte(t,{})*lt(t,{})", f(a), f(b)),
                None => format!("gte(t,{})", f(a)),
            };
            out.push(KBox { x, y, w, h, enable });
        }
    }
    out
}

/// Export frames `n0..n1` showing something that covers `start..end` (absolute timeline times)
/// by the preview's rule [`Time::first_frame_from`], and the frame duration. Absolute frame numbers
/// minus the export start's frame (n0 may be negative): relative µs times round differently
/// when the export starts at a frame time off whole µs.
fn frame_span(ctx: &Ctx, start: Time, end: Time) -> (i64, i64, f64) {
    let fps = Rational::parse(&ctx.fps).unwrap_or(Rational::new(30, 1));
    // export frame 0 = first frame at or after the export start (it need not lie on a frame)
    let s = ctx.start.first_frame_from(fps);
    (start.first_frame_from(fps) - s, end.first_frame_from(fps) - s, 1.0 / fps.as_f64())
}

/// Place the sprite covering `start..end` (absolute timeline times) on `cur`: on the frames
/// [`frame_span`] gives, its first frame on the first of them.
fn place(ctx: &mut Ctx, cur: &str, sp: &Sprite, start: Time, end: Time) -> String {
    let (n0, n1, fd) = frame_span(ctx, start, end);
    let t0 = n0 as f64 * fd;
    let shifted = ctx.label("pl");
    // rounded like `trim` (otherwise setpts cuts off: at 29.97 fps a clip was one frame off in a
    // single pass compared with a section that starts inside it)
    let shift = if n0 >= 0 {
        format!("setpts=PTS-STARTPTS+round({}/TB)", f(t0))
    } else {
        // 1 µs early: frame -n0 stays although `f` rounds; `trim` rounds `start` to the time base,
        // so a larger margin (half a frame) would keep the frame before at 1/fps
        format!("trim=start={},setpts=PTS-STARTPTS", f(-t0 - 1e-6))
    };
    ctx.graph.push(format!("[{}]{shift}[{shifted}]", sp.label));
    let var = format!("(t-{})", f(t0));
    let x = sp.x.replace("TT", &var);
    let y = sp.y.replace("TT", &var);
    let out = ctx.label("base");
    // frames n0..n1 with half-frame margins: frame times are never compared for equality
    let (a, b) = ((n0.max(0) as f64 - 0.5) * fd, (n1 as f64 - 0.5) * fd);
    ctx.graph.push(format!(
        "[{cur}][{shifted}]overlay=x='(W-w)/2+{x}':y='(H-h)/2+{y}':eval=frame:eof_action=pass{}:enable='between(t,{},{})'[{out}]",
        if ctx.alpha { ":format=auto" } else { "" },
        f(a),
        f(b)
    ));
    out
}

/// Effect/filter element: copy the whole image up to here, apply the effect (optionally only
/// within the mask) and overlay it for the element's duration. With strength keyframes
/// the result is mixed with the original over time. On the frames [`frame_span`] gives.
fn effect_layer(ctx: &mut Ctx, cur: &str, c: &Clip, r: &FxRef, filter: bool) -> Result<String> {
    let (n0, n1, fd) = frame_span(ctx, c.start, c.end());
    // covers no export frame: build nothing (an empty stream through the whole chain)
    if n1 <= n0.max(0) {
        return Ok(cur.to_string());
    }
    let kfs = c.keyframes.get(Prop::Strength);
    let mut r = r.clone();
    if !kfs.is_empty() {
        r.intensity = 1.0;
    }
    let fx = ClipFx {
        effects: if filter { Vec::new() } else { vec![r.clone()] },
        filter: filter.then(|| r.clone()),
        mask: c.fx.mask,
        ..Default::default()
    };
    // only the active frames are processed (not the whole video)
    let (s0, s1) = (n0.max(0) as f64 * fd, n1 as f64 * fd);
    // element time of the first processed frame (export starts inside the element: not 0)
    let into = (-n0).max(0) as f64 * fd;
    // by frame index (exact; `trim=start/end` rounds to the time base, a margin landed on a frame)
    let window = format!("trim=start_frame={}:end_frame={n1},setpts=PTS-STARTPTS", n0.max(0));
    let (a, b, bw, fo) = (ctx.label("ea"), ctx.label("eb"), ctx.label("ew"), ctx.label("ef"));
    let o = ctx.label("eo");
    // without strength keyframes only two branches: a third with nullsink makes ffmpeg
    // compute the whole video up front (long start pause without progress)
    if kfs.is_empty() {
        ctx.graph.push(format!("[{cur}]split=2[{a}][{b}]"));
    } else {
        ctx.graph.push(format!("[{cur}]split=3[{a}][{b}][{o}]"));
    }
    ctx.graph.push(format!("[{b}]{window}[{bw}]"));
    let uniq = ctx.label("e");
    let opts = ChainOpts {
        // if the export starts inside the element, the effect time continues there (not from 0)
        t_offset: into,
        apply_reverse: false,
        lut: None,
        matte: None,
    };
    let src = Src {
        w: ctx.w,
        h: ctx.h,
        has_alpha: ctx.alpha,
    };
    ctx.graph.extend(fx_graph(&fx, src, &opts, &bw, &fo, &uniq));
    let fmt = if ctx.alpha { ":format=auto" } else { "" };
    let (pre, top, out) = (ctx.label("ep"), ctx.label("et"), ctx.label("base"));
    // effect time ran from `s0 - t0`: from 0 again for the overlay
    let rebase = if into > 0.0 { "setpts=PTS-STARTPTS," } else { "" };
    if kfs.is_empty() {
        ctx.graph.push(format!("[{fo}]{rebase}format=rgba,setsar=1[{pre}]"));
    } else {
        // bake in the mask (effect over original), then set the opacity per frame via sendcmd
        // (fast, instead of a formula per pixel)
        let (ow, fm) = (ctx.label("eow"), ctx.label("em"));
        ctx.graph.push(format!("[{o}]{window}[{ow}]"));
        let fr = if rebase.is_empty() {
            fo.clone()
        } else {
            let fr = ctx.label("er");
            ctx.graph.push(format!("[{fo}]{rebase}null[{fr}]"));
            fr
        };
        ctx.graph.push(format!("[{ow}][{fr}]overlay=eof_action=pass{fmt}[{fm}]"));
        let mix = ctx.label("mix");
        let lo = into; // offset in case the export starts inside the element
        let k = |tau: f64| {
            c.keyframes
                .eval(Prop::Strength, Time::from_secs_f64(lo + tau))
                .unwrap_or(1.0)
                .clamp(0.0, 1.0)
        };
        let step = 1.0 / 60.0;
        let n = ((s1 - s0) / step).ceil() as usize;
        let cmds: String = (0..=n)
            .map(|i| {
                let tau = i as f64 * step;
                format!("{} colorchannelmixer@{mix} aa {:.4};\n", f(tau), k(tau))
            })
            .collect();
        let file = format!("{mix}.cmd");
        std::fs::write(ctx.workdir.join(&file), cmds)?;
        ctx.graph.push(format!(
            "[{fm}]format=rgba,sendcmd=f={file},colorchannelmixer@{mix}=aa={:.4},setsar=1[{pre}]",
            k(0.0)
        ));
    }
    // blank frames from their own source before it (instead of a PTS offset): otherwise overlay waits for the
    // first effect frame and buffers the whole video until s0 (long start pause, lots of RAM)
    if s0 > 0.0 {
        let pad = ctx.label("epad");
        ctx.graph.push(format!(
            "color=c=black@0.0:s={}x{}:r={},trim=end_frame={},format=rgba,setsar=1[{pad}]",
            ctx.w,
            ctx.h,
            ctx.fps,
            // exactly n0 frames (`d` rounded to µs gave one more at 23/30 s)
            n0
        ));
        ctx.graph.push(format!("[{pad}][{pre}]concat=n=2:v=1:a=0[{top}]"));
    } else {
        ctx.graph.push(format!("[{pre}]null[{top}]"));
    }
    ctx.graph.push(format!(
        "[{a}][{top}]overlay=eof_action=pass{fmt}:enable='between(t,{},{})'[{out}]",
        f(s0 - fd / 2.0),
        f(s1 - fd / 2.0)
    ));
    Ok(out)
}

/// Clips of a track in groups joined by transitions.
pub(crate) fn transition_groups(tr: &Track) -> Vec<Vec<&Clip>> {
    let mut groups: Vec<Vec<&Clip>> = Vec::new();
    for c in tr.clips.iter().filter(|c| !c.disabled) {
        let joined = groups.last().and_then(|g| g.last()).is_some_and(|prev| {
            prev.transition_out.is_some()
                && (c.start.0 - prev.end().0).abs() < 2000
                && matches!(prev.content, ClipContent::Media { .. })
                && matches!(c.content, ClipContent::Media { .. })
        });
        if joined {
            groups.last_mut().expect("vorhanden").push(c);
        } else {
            groups.push(vec![c]);
        }
    }
    groups
}

fn sprite_for(ctx: &mut Ctx, p: &Project, c: &Clip, skip: f64) -> Result<Option<Sprite>> {
    match &c.content {
        ClipContent::Media {
            media,
            kind: kind @ (MediaKind::Video | MediaKind::Image),
        } => {
            let m = p
                .media(*media)
                .ok_or_else(|| ExportError::MissingMedia(vec![media.to_string()]))?
                .clone();
            media_sprite(ctx, c, &m, *kind == MediaKind::Image, skip).map(Some)
        }
        ClipContent::Text(st) => text_sprite(ctx, c, st),
        _ => Ok(None),
    }
}

/// Layer of the `k`-th clip of a transition group: full frame, extended front and back by half
/// the transition (picture held). (label, length, duration of the transition before)
fn group_layer(ctx: &mut Ctx, p: &Project, group: &[&Clip], k: usize) -> Result<Option<(String, f64, f64)>> {
    let (w, h) = (ctx.w, ctx.h);
    let c = group[k];
    let Some(sp) = sprite_for(ctx, p, c, 0.0)? else {
        return Ok(None);
    };
    {
        let len = c.duration.as_secs_f64();
        // same rule as the preview
        let d_prev = if k > 0 { cl_fx::transition_secs(group[k - 1], c) } else { 0.0 };
        let d_next = if k + 1 < group.len() { cl_fx::transition_secs(c, group[k + 1]) } else { 0.0 };
        let canvas = ctx.label("cv");
        ctx.graph.push(format!(
            "color=c=black@0.0:s={w}x{h}:r={}:d={},format=rgba[{canvas}]",
            ctx.fps,
            // the chain's last layer one frame longer, as in `media_sprite` (never shown)
            f(len + if k + 1 == group.len() { 1.0 / fps_value(&ctx.fps) } else { 0.0 })
        ));
        let x = sp.x.replace("TT", "t");
        let y = sp.y.replace("TT", "t");
        let layer = ctx.label("ly");
        ctx.graph.push(format!(
            "[{canvas}][{}]overlay=x='(W-w)/2+{x}':y='(H-h)/2+{y}':eval=frame:eof_action=pass,tpad=start_mode=clone:start_duration={}:stop_mode=clone:stop_duration={},fps={},format=gbrap[{layer}]",
            sp.label,
            f(d_prev / 2.0),
            f(d_next / 2.0),
            ctx.fps
        ));
        let layer_len = len + d_prev / 2.0 + d_next / 2.0;
        Ok(Some((layer, layer_len, d_prev)))
    }
}

/// Join several clips with transitions into one full-frame stream (xfade, length stays).
/// Shader transitions get a pre-pass in addition ([`ShaderPass`]).
fn transition_chain(ctx: &mut Ctx, p: &Project, group: &[&Clip]) -> Result<Option<Sprite>> {
    let fps = fps_value(&ctx.fps);
    let mut acc: Option<(String, f64)> = None;
    // frame in the joined stream where the previous layer starts, and end of the previous window
    let (mut layer_start, mut window_end) = (0usize, 0usize);
    for k in 0..group.len() {
        let Some((layer, layer_len, d_prev)) = group_layer(ctx, p, group, k)? else {
            continue;
        };
        acc = Some(match acc {
            None => (layer, layer_len),
            Some((a, a_len)) => {
                let tr = group[k - 1].transition_out.as_ref().expect("Gruppe");
                let out = ctx.label("xf");
                // the id comes unchecked from project file/MCP: the catalog only delivers built-in
                // xfade names (package transitions: their fallback), everything else becomes `fade`
                let plan = cl_fx::catalog::transition_plan(&tr.id);
                let id = cl_fx::TRANSITIONS
                    .iter()
                    .find(|t| t.0 == plan.xfade)
                    .map_or("fade", |t| t.0);
                let d = d_prev.max(0.04);
                let off = (a_len - d).max(0.0);
                ctx.graph.push(format!(
                    "[{a}][{layer}]xfade=transition={}:duration={}:offset={}[{out}]",
                    id,
                    f(d),
                    f(off)
                ));
                let (i0, n) = window_frames(off, d, fps);
                let mut res = out.clone();
                // shortcut: if two windows overlap (clip shorter than the half transitions), the
                // second stays on the xfade fallback; upgrade if this shows up in real projects
                if let Some((file, params)) = plan.shader.filter(|_| ctx.shader && i0 >= window_end && a_len >= d) {
                    if let Some(l) = shader_pass(ctx, p, group, k, &out, &file, &params, (i0, n), layer_start, (off, d))? {
                        res = l;
                    }
                }
                (layer_start, window_end) = (i0, i0 + n);
                (res, a_len + layer_len - d)
            }
        });
    }
    Ok(acc.map(|(label, _)| {
        let out = format!("{label}r");
        ctx.graph.push(format!("[{label}]format=rgba[{out}]"));
        Sprite {
            label: out,
            x: "0".into(),
            y: "0".into(),
        }
    }))
}

/// Create the pre-pass for the shader transition between `group[k-1]` and `group[k]`; returns the label
/// that replaces the `xfade` output `xf` in the main graph. Window `win` = (first frame, count)
/// in the joined stream, `a_start` = frame where clip A starts there, `od` = (offset, duration).
#[allow(clippy::too_many_arguments)]
fn shader_pass(
    ctx: &mut Ctx,
    p: &Project,
    group: &[&Clip],
    k: usize,
    xf: &str,
    file: &Path,
    params: &[f32],
    win: (usize, usize),
    a_start: usize,
    od: (f64, f64),
) -> Result<Option<String>> {
    let Ok(shader) = std::fs::read_to_string(file) else {
        return Ok(None);
    };
    let ((i0, n), (off, d)) = (win, od);
    let idx = ctx.pass_base + ctx.passes.len();
    // own graph with the same clip chains; own labels/file names (counter far away)
    let mut sub = Ctx {
        w: ctx.w,
        h: ctx.h,
        fps: ctx.fps.clone(),
        start: ctx.start,
        workdir: ctx.workdir,
        fonts: ctx.fonts,
        inputs: Vec::new(),
        n_input: 0,
        graph: Vec::new(),
        uniq: 1_000_000 * (idx + 1),
        alpha: ctx.alpha,
        shader: false,
        passes: Vec::new(),
        pass_base: 0,
        lead: false,
    };
    let (Some((la, ..)), Some((lb, ..))) = (group_layer(&mut sub, p, group, k - 1)?, group_layer(&mut sub, p, group, k)?) else {
        return Ok(None);
    };
    let a0 = i0 - a_start;
    // picture held in case the layer ends one frame early (rounding)
    // `tpad` before `setpts` (FFmpeg 7 forgets the frame rate afterwards)
    sub.graph.push(format!("[{la}]trim=start_frame={a0},tpad=stop_mode=clone:stop_duration=1,setpts=PTS-STARTPTS[pa]"));
    sub.graph.push(format!("[{lb}]tpad=stop_mode=clone:stop_duration=1,setpts=PTS-STARTPTS[pb]"));
    sub.graph.push("[pa][pb]hstack=inputs=2:shortest=1,format=rgba[pout]".into());
    let graph_file = format!("shader_{idx}.txt");
    std::fs::write(ctx.workdir.join(&graph_file), sub.graph.join(";\n"))?;
    let fps = fps_value(&ctx.fps);
    let progress = (0..n)
        .map(|j| ((((i0 + j) as f64 / fps) - off) / d).clamp(0.0, 1.0) as f32)
        .collect();
    let out_file = format!("shader_{idx}.nut");
    let (sv, res) = (ctx.label("shv"), ctx.label("xs"));
    // time base 1/fps: frame j lies exactly on frame i0 + j of the stream
    let (num, den) = ctx.fps.split_once('/').unwrap_or((ctx.fps.as_str(), "1"));
    let block = format!(
        "movie={out_file},settb=expr={den}/{num},setpts=N+{i0},format=gbrap,setsar=1[{sv}];\n[{xf}][{sv}]blend=all_expr=B:eof_action=pass:enable='between(n,{i0},{})'[{res}]",
        i0 + n - 1
    );
    ctx.graph.push(block.clone());
    ctx.passes.push(ShaderPass {
        inputs: sub.inputs,
        graph_file,
        out_file,
        w: ctx.w,
        h: ctx.h,
        fps: ctx.fps.clone(),
        shader,
        params: params.to_vec(),
        progress,
        block,
        fallback: format!("[{xf}]null[{res}]"),
    });
    Ok(Some(res))
}

/// Convert the project to export size (positions/font sizes in project pixels).
fn scaled_project(p: &Project, w: u32) -> Project {
    let k = w as f32 / p.settings.width.max(1) as f32;
    let mut proj = p.clone();
    if (k - 1.0).abs() > 1e-3 {
        for tr in &mut proj.timeline.tracks {
            for c in &mut tr.clips {
                c.transform.x *= k;
                c.transform.y *= k;
                if let ClipContent::Text(st) = &mut c.content {
                    st.size *= k;
                    if let Some(o) = &mut st.outline {
                        o.0 *= k;
                    }
                }
                for prop in [Prop::X, Prop::Y] {
                    if let Some(list) = c.keyframes.0.get_mut(&prop) {
                        for kf in list.iter_mut() {
                            kf.v *= k;
                        }
                    }
                }
            }
        }
    }
    proj
}

/// Video part: all visible tracks from bottom to top. Returns the last label.
pub(crate) fn video_part(ctx: &mut Ctx, p: &Project, end: Time, dur: Time) -> Result<String> {
    let (w, h) = (ctx.w, ctx.h);
    let base = if ctx.alpha {
        "color=c=black@0.0"
    } else {
        "color=c=black"
    };
    ctx.graph.push(format!(
        "{base}:s={w}x{h}:r={}:d={}{}[base0]",
        ctx.fps,
        f(dur.as_secs_f64()),
        if ctx.alpha { ",format=rgba" } else { "" }
    ));
    let mut cur = "base0".to_string();
    let proj = scaled_project(p, w);
    for tr in proj.timeline.tracks.iter().filter(|t| !t.hidden) {
        if matches!(tr.kind, TrackKind::Effect | TrackKind::Filter) {
            for c in tr.clips.iter().filter(|c| !c.disabled) {
                if c.end() <= ctx.start || c.start >= end {
                    continue;
                }
                let (r, filter) = match &c.content {
                    ClipContent::Effect(r) => (r, false),
                    ClipContent::Filter(r) => (r, true),
                    _ => continue,
                };
                cur = effect_layer(ctx, &cur, c, r, filter)?;
            }
            continue;
        }
        for group in transition_groups(tr) {
            let g_start = group[0].start;
            let g_end = group.last().expect("nicht leer").end();
            if g_end <= ctx.start || g_start >= end {
                continue;
            }
            let sprite = if group.len() > 1 {
                transition_chain(ctx, &proj, &group)?
            } else {
                let c = group[0];
                let video = matches!(c.content, ClipContent::Media { kind: MediaKind::Video, .. });
                let skip = if ctx.lead && video && !fx_has_memory(&c.fx) {
                    let src = match &c.content {
                        ClipContent::Media { media, .. } => proj.media(*media).and_then(|m| m.fps),
                        _ => None,
                    };
                    video_skip(c, src, Rational::parse(&ctx.fps).unwrap_or(Rational::new(30, 1)), ctx.start)
                } else {
                    0.0
                };
                sprite_for(ctx, &proj, c, skip)?
            };
            let Some(sp) = sprite else { continue };
            cur = place(ctx, &cur, &sp, g_start, g_end);
        }
    }
    Ok(cur)
}

/// Sections of a speed curve: (source start s, source end s, speed) for equally long pieces
/// of clip time `dur`; `src` = length of the source range.
fn curve_pieces(id: &str, src: f64, dur: f64) -> Vec<(f64, f64, f32)> {
    const N: usize = 24;
    let at = |k: usize| curve_progress(id, k as f32 / N as f32) as f64 * src;
    (0..N)
        .map(|k| (at(k), at(k + 1), ((at(k + 1) - at(k)) / (dur / N as f64).max(1e-6)) as f32))
        .collect()
}

/// Audible clips (with media) touching the range `start..end` – in track and
/// clip order (the order of the inputs in the mix).
pub fn audible_clips(p: &Project, start: Time, end: Time) -> impl Iterator<Item = (&Clip, &MediaRef)> {
    p.timeline
        .tracks
        .iter()
        .filter(|tr| !tr.muted && matches!(tr.kind, TrackKind::Video | TrackKind::Audio))
        .flat_map(|tr| tr.clips.iter())
        .filter(move |c| !c.disabled && c.end() > start && c.start < end)
        .filter(|c| c.volume_db > -59.9 || !c.keyframes.get(Prop::Volume).is_empty())
        .filter_map(|c| match &c.content {
            ClipContent::Media { media, kind } if *kind != MediaKind::Image => {
                // flag first: `is_local` asks the disk, and the export dialog calls this every frame
                p.media(*media).filter(|m| m.has_audio && !c.stream_extracted(m) && m.is_local()).map(|m| (c, m))
            }
            _ => None,
        })
}

/// How the audio mix is built.
#[derive(Clone, Copy, Default)]
pub(crate) struct AudioOpts {
    /// Export waits for the channel check, playback does not (otherwise the UI freezes)
    pub wait_probe: bool,
    /// Limiter at the end; off in sections, there it runs once over the whole
    pub limiter: bool,
    /// Section: long clips via `-ss` close to the start (see [`lead_skip`])
    pub lead: bool,
    /// One input per file and position instead of per clip (preview windows with very many clips)
    pub shared: bool,
    /// Leave out files whose audio FFmpeg cannot read instead of failing the whole mix
    /// (preview; see [`crate::audio_readable`])
    pub skip_bad: bool,
}

/// Audio stream of the file a clip plays (stream specifier after the input number, e.g. `a`).
/// The only place where the stream is chosen – export, sections and preview.
pub(crate) fn audio_stream(c: &Clip, m: &MediaRef) -> String {
    // chosen audio stream of the file; if it is missing (other file linked), the first
    match c.audio_stream.filter(|&n| n > 0 && n < m.audio_streams) {
        Some(n) => format!("a:{n}"),
        None => "a".into(),
    }
}

/// Merge inputs: source ranges `(key, start, end)` with the same key that
/// are at most `gap` seconds apart share one input. Returns per range the
/// number of its input and per input `(key, start, end)`.
pub(crate) fn share_inputs(ranges: &[(usize, f64, f64)], gap: f64) -> (Vec<usize>, Vec<(usize, f64, f64)>) {
    let mut order: Vec<usize> = (0..ranges.len()).collect();
    order.sort_by(|&a, &b| (ranges[a].0, ranges[a].1).partial_cmp(&(ranges[b].0, ranges[b].1)).unwrap_or(std::cmp::Ordering::Equal));
    let mut of = vec![0; ranges.len()];
    let mut groups: Vec<(usize, f64, f64)> = Vec::new();
    for i in order {
        let (k, a, b) = ranges[i];
        match groups.last_mut() {
            Some(g) if g.0 == k && a <= g.2 + gap => g.2 = g.2.max(b),
            _ => groups.push((k, a, b)),
        }
        of[i] = groups.len() - 1;
    }
    (of, groups)
}

/// Audio: all audible clips (speed, reverse, volume keyframes,
/// fade in/out, extension with silence) are mixed.
pub(crate) fn audio_part(
    p: &Project,
    start: Time,
    end: Time,
    inputs: &mut Vec<String>,
    next_input: &mut usize,
    graph: &mut Vec<String>,
    o: AudioOpts,
) -> String {
    let clips: Vec<(&Clip, &MediaRef)> = audible_clips(p, start, end)
        .filter(|(_, m)| !o.skip_bad || crate::audio_readable(&m.path, o.wait_probe))
        .collect();
    // shared inputs: per clip (input, offset in the input); the input is distributed with asplit
    let mut shared: Vec<(usize, f64)> = Vec::new();
    if o.shared {
        let mut keys: Vec<(String, String)> = Vec::new();
        let ranges: Vec<(usize, f64, f64)> = clips
            .iter()
            .map(|(c, m)| {
                let key = (m.path.to_string_lossy().into_owned(), audio_stream(c, m));
                let k = keys.iter().position(|x| *x == key).unwrap_or_else(|| {
                    keys.push(key);
                    keys.len() - 1
                });
                let (a, len) = source_segment(c);
                (k, a.as_secs_f64(), a.as_secs_f64() + len.as_secs_f64().max(0.02))
            })
            .collect();
        let (of, groups) = share_inputs(&ranges, 10.0);
        let first = *next_input;
        for (g, (k, a, b)) in groups.iter().enumerate() {
            inputs.extend(["-ss".into(), f(*a), "-t".into(), f(b - a), "-i".into(), keys[*k].0.clone()]);
            let users = of.iter().filter(|&&x| x == g).count();
            let outs: String = (0..users).map(|u| format!("[s{}u{u}]", first + g)).collect();
            graph.push(format!("[{}:{}]asplit={users}{outs}", first + g, keys[*k].1));
        }
        *next_input += groups.len();
        shared = of.iter().enumerate().map(|(j, &g)| (first + g, ranges[j].1 - groups[g].1)).collect();
    }
    let mut taken = std::collections::HashMap::<usize, usize>::new();
    let mut labels = Vec::new();
    for (j, (c, m)) in clips.into_iter().enumerate() {
        let dur = c.duration.as_secs_f64();
        let (seg_in, seg_len) = source_segment(c);
        // jump to the section start only without speed and without audio processing with memory
        let skip = if o.lead && !o.shared && cl_fx::audio::audio_chain(&c.audio).is_empty() { lead_skip(c, start) } else { 0.0 };
        let mut chain = Vec::new();
        let (i, mut src) = if o.shared {
            let (g, off) = shared[j];
            let u = taken.entry(g).or_insert(0);
            let src = format!("s{g}u{u}");
            *u += 1;
            chain.push(format!("atrim=start={}:end={}", f(off), f(off + seg_len.as_secs_f64().max(0.02))));
            (*next_input + j, src)
        } else {
            let i = *next_input;
            *next_input += 1;
            inputs.extend([
                "-ss".into(),
                f(seg_in.as_secs_f64() + skip),
                "-t".into(),
                f((seg_len.as_secs_f64() - skip).max(0.02)),
                "-i".into(),
                m.path.to_string_lossy().into_owned(),
            ]);
            (i, format!("{i}:{}", audio_stream(c, m)))
        };
        chain.extend([
            "aresample=48000".to_string(),
            "aformat=sample_fmts=fltp:channel_layouts=stereo".into(),
            if skip > 0.0 { format!("asetpts=PTS-STARTPTS+{}/TB", f(skip)) } else { "asetpts=PTS-STARTPTS".into() },
        ]);
        // channels: e.g. microphone only left → to both ears
        let mode = c.audio.channels.unwrap_or(p.settings.channels);
        let silent = (mode == Channels::Auto)
            .then(|| cl_media::silent_channel_opt(&m.path, o.wait_probe))
            .flatten();
        if let Some(pan) = mode.filter(silent) {
            chain.push(pan.into());
        }
        if c.fx.reverse {
            chain.push("areverse".into());
        }
        if let Some(id) = &c.fx.speed_curve {
            // speed curve: split the source into sections, constant atempo per section
            let pieces = curve_pieces(id, seg_len.as_secs_f64(), dur);
            let outs: String = (0..pieces.len()).map(|k| format!("[c{i}p{k}]")).collect();
            graph.push(format!("[{src}]{},asplit={}{outs}", chain.join(","), pieces.len()));
            for (k, (a, b, speed)) in pieces.iter().enumerate() {
                let at = atempo_chain(*speed);
                graph.push(format!(
                    "[c{i}p{k}]atrim={}:{},asetpts=PTS-STARTPTS,{}[c{i}q{k}]",
                    f(*a),
                    f(*b),
                    if at.is_empty() { "anull".into() } else { at }
                ));
            }
            let ins: String = (0..pieces.len()).map(|k| format!("[c{i}q{k}]")).collect();
            graph.push(format!("{ins}concat=n={}:v=0:a=1[c{i}]", pieces.len()));
            src = format!("c{i}");
            chain.clear();
        } else {
            let at = atempo_chain(c.speed);
            if !at.is_empty() {
                chain.push(at);
            }
        }
        chain.extend(cl_fx::audio::audio_chain(&c.audio));
        // audio stretched longer: fill with silence (counted from the first sample read)
        chain.push(format!("apad=whole_dur={}", f(dur - skip)));
        chain.push(format!("atrim=0:{}", f(dur)));
        chain.push(if skip > 0.0 { format!("asetpts=PTS-STARTPTS+{}/TB", f(skip)) } else { "asetpts=PTS-STARTPTS".into() });
        let vol = c.keyframes.get(Prop::Volume);
        if vol.is_empty() {
            if c.volume_db.abs() > 0.01 {
                chain.push(format!("volume={:.2}dB", c.volume_db));
            }
        } else {
            let pts: Vec<(f64, f64)> = vol
                .iter()
                .map(|k| (k.t.as_secs_f64(), k.v as f64))
                .collect();
            chain.push(format!(
                "volume='pow(10,({})/20)':eval=frame",
                piecewise_expr(&pts, "t")
            ));
        }
        // fades at most as long as the clip (split/speed/trim shorten it), otherwise
        // FFmpeg rejects a negative `st`
        let (fade_in, fade_out) = (c.fade_in.as_secs_f64().min(dur), c.fade_out.as_secs_f64().min(dur));
        if fade_in > 0.0 {
            chain.push(format!("afade=t=in:d={}", f(fade_in)));
        }
        if fade_out > 0.0 {
            chain.push(format!(
                "afade=t=out:st={}:d={}",
                f((dur - fade_out).max(0.0)),
                f(fade_out)
            ));
        }
        let t0 = (c.start - start).as_secs_f64();
        if t0 < 0.0 {
            chain.push(format!("atrim=start={}", f(-t0)));
            chain.push("asetpts=PTS-STARTPTS".into());
        }
        let label = format!("a{i}");
        if t0 > 0.0 {
            // silence from its own source before it instead of adelay: with a long offset
            // adelay delivers nothing for a long time, the muxer then holds back the video (no progress)
            let fmt = "aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo";
            graph.push(format!("[{src}]{},{fmt}[{label}x]", chain.join(",")));
            graph.push(format!("anullsrc=r=48000:cl=stereo,atrim=0:{},{fmt}[{label}s]", f(t0)));
            graph.push(format!("[{label}s][{label}x]concat=n=2:v=0:a=1[{label}]"));
        } else {
            graph.push(format!("[{src}]{}[{label}]", chain.join(",")));
        }
        labels.push(label);
    }
    let dur = f((end - start).as_secs_f64());
    if labels.is_empty() {
        graph.push(format!("anullsrc=r=48000:cl=stereo,atrim=0:{dur}[aout]"));
    } else {
        // silence over the whole length as the last input: amix then ends exactly at the end. With
        // apad alone the audio occasionally ended with the last clip under load (measured: 6 of
        // 6 parallel exports, 2 of them 0.5 s too short)
        graph.push(format!("anullsrc=r=48000:cl=stereo,atrim=0:{dur},aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo[afull]"));
        let ins: String = labels.iter().map(|l| format!("[{l}]")).chain(["[afull]".to_string()]).collect();
        let limit = if o.limiter { format!("{},", limiter()) } else { String::new() };
        graph.push(format!(
            "{ins}amix=inputs={}:normalize=0:duration=longest,{limit}apad=whole_dur={dur},atrim=0:{dur},aformat=sample_fmts=fltp:channel_layouts=stereo[aout]",
            labels.len() + 1,
        ));
    }
    "aout".into()
}

/// Limiter against clipping of overlapping clips; below it bit-identical (level/latency off or compensated)
pub(crate) fn limiter() -> String {
    format!("alimiter=limit={}:level=false:latency=true", f(10f64.powf(LIMIT_DB / 20.0)))
}

/// Peak after mixing; margin to −1 dBFS because AAC/MP3 overshoot slightly when encoding.
pub(crate) const LIMIT_DB: f64 = -1.5;
