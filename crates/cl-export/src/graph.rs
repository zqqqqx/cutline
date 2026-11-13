//! Filtergraph-Aufbau (Phase 2): Jeder Clip wird ein eigenes Bild (Sprite) mit Effektkette
//! (cl-fx) und Ausdrücken für Position, Größe, Drehung und Deckkraft aus Keyframes und
//! Animationen. Clips mit Übergang werden per `xfade` zu einem Strom verbunden; die
//! Gesamtlänge bleibt dabei gleich (Bild an den Schnitten wird gehalten).

use std::path::Path;

use cl_core::*;
use cl_fx::{atempo_chain, fx_graph, piecewise_expr, speed_setpts, ChainOpts, Src};

use crate::{default_font, f, FontResolver, Result};

/// Laufender Aufbau eines Filtergraphen.
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
    /// Transparenter Hintergrund statt Schwarz
    pub alpha: bool,
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

/// Quellbereich eines Medienclips: (Start, Länge) in der Quelle.
pub(crate) fn source_segment(c: &Clip) -> (Time, Time) {
    let a = c.source_time(c.start);
    let b = c.source_time(c.end());
    (a.min(b), Time((b.0 - a.0).abs()))
}

/// Ausgewertete Darstellung, abgetastet über die Cliplänge (für Ausdrücke).
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

/// Zeitabhängiger Wert als Ausdruck in `var` (konstant, wenn er sich nicht ändert).
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

/// Drehung im Uhrzeigersinn (Grad) mit transparenten Ecken und vergrößerter Fläche.
pub(crate) fn rotate_filter(deg: f32) -> String {
    if deg.abs() < 0.01 {
        return String::new();
    }
    let rad = deg.to_radians();
    format!("rotate={rad:.6}:c=none:ow=rotw({rad:.6}):oh=roth({rad:.6})")
}

/// Ein vorbereitetes Bild mit transparentem Rand. Zeit 0 = Clipbeginn.
/// `x`/`y` sind Ausdrücke in `TT` (Clipzeit).
pub(crate) struct Sprite {
    label: String,
    x: String,
    y: String,
}

/// Deckkraft, Skalierung und Drehung (statisch oder animiert) anhängen.
fn post_transform(ctx: &mut Ctx, input: &str, base_w: u32, base_h: u32, s: &[(f64, Evaluated)]) -> String {
    let mut chain = vec!["format=rgba".to_string()];
    let op = value_expr(s, |e| e.opacity, "T");
    if is_const(&op) {
        let v: f32 = op.parse().unwrap_or(1.0);
        if v < 0.999 {
            chain.push(format!("colorchannelmixer=aa={v:.3}"));
        }
    } else {
        chain.push(format!("geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='alpha(X,Y)*clip({op},0,1)'"));
    }
    let sc = value_expr(s, |e| e.scale, "t");
    let rot = value_expr(s, |e| e.rotation_deg, "t");
    let max_s = s.iter().map(|(_, e)| e.scale).fold(0.0f32, f32::max).max(0.01) as f64;
    // Feste Fläche für animierte Größe/Drehung: neuere FFmpeg-Versionen lehnen
    // wechselnde Bildgrößen in nachfolgenden Filtern (rotate, overlay) ab.
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
        chain.push(format!("pad=w={d}:h={d}:x='(ow-iw)/2':y='(oh-ih)/2':color=black@0:eval=frame"));
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
    ctx.graph.push(format!("[{input}]{}[{out}]", chain.join(",")));
    out
}

fn media_sprite(ctx: &mut Ctx, c: &Clip, m: &MediaRef, image: bool) -> Result<Sprite> {
    let (w, h) = (ctx.w, ctx.h);
    let dur = c.duration.as_secs_f64();
    let path = m.path.to_string_lossy().into_owned();
    let (_, seg_len) = source_segment(c);
    let i = if image {
        ctx.input(vec!["-loop".into(), "1".into(), "-t".into(), f(dur + 0.1), "-i".into(), path])
    } else {
        let (seg_in, _) = source_segment(c);
        ctx.input(vec!["-ss".into(), f(seg_in.as_secs_f64()), "-t".into(), f(seg_len.as_secs_f64().max(0.04)), "-i".into(), path])
    };
    // Grundgröße: eingepasst wie im Player (contain)
    let fit = (w as f64 / m.width.max(1) as f64).min(h as f64 / m.height.max(1) as f64);
    let bw = ((m.width as f64 * fit).round() as u32).max(2) & !1;
    let bh = ((m.height as f64 * fit).round() as u32).max(2) & !1;
    let mut pre = vec!["setpts=PTS-STARTPTS".to_string()];
    let _ = &pre;
    if !image {
        if c.fx.reverse {
            pre.push("reverse".into());
            pre.push("setpts=PTS-STARTPTS".into());
        }
        let sp = speed_setpts(c.speed, c.fx.speed_curve.as_deref(), seg_len.as_secs_f64(), dur);
        if !sp.is_empty() {
            pre.push(sp);
        }
        // Flüssige Zeitlupe: Zwischenbilder per Bewegungsschätzung statt doppelter Bilder
        if c.fx.smooth_slowmo && (c.speed < 0.999 || c.fx.speed_curve.is_some()) {
            pre.push(format!("minterpolate=fps={}:mi_mode=mci:mc_mode=aobmc:me_mode=bidir:vsbmc=1", ctx.fps));
        }
        // Videos, die kürzer als der Clip sind (Rundung), halten das letzte Bild
        pre.push(format!("tpad=stop_mode=clone:stop_duration={}", f(0.5)));
    }
    pre.push(format!("fps={}", ctx.fps));
    pre.push(format!("scale={bw}:{bh}:flags=bicubic"));
    pre.push(format!("trim=duration={}", f(dur)));
    let pl = ctx.label("pre");
    ctx.graph.push(format!("[{i}:v]{}[{pl}]", pre.join(",")));
    // LUT in den Arbeitsordner kopieren (keine Pfad-Escapes im Graphen)
    let lut = match &c.fx.lut {
        Some(l) if Path::new(l).is_file() => {
            let name = format!("{}.cube", ctx.label("lut"));
            std::fs::copy(l, ctx.workdir.join(&name))?;
            Some(name)
        }
        _ => None,
    };
    // Hintergrund entfernen: Maskenvideo mit identischer Zeitachse als zweiter Eingang
    let mut matte = None;
    if let Some(mp) = c.fx.cutout.as_ref().filter(|p| !image && Path::new(p).is_file()) {
        let (seg_in, _) = source_segment(c);
        let mi = ctx.input(vec!["-ss".into(), f(seg_in.as_secs_f64()), "-t".into(), f(seg_len.as_secs_f64().max(0.04)), "-i".into(), mp.clone()]);
        let mut mpre: Vec<String> = pre.iter().filter(|p| !p.starts_with("scale=")).cloned().collect();
        mpre.retain(|p| !p.starts_with("trim="));
        mpre.push(format!("scale={bw}:{bh}:flags=bilinear"));
        mpre.push(format!("trim=duration={}", f(dur)));
        mpre.push("format=gray".into());
        let ml = ctx.label("mt");
        ctx.graph.push(format!("[{mi}:v]{}[{ml}]", mpre.join(",")));
        matte = Some(ml);
    }
    let fl = ctx.label("fx");
    let u = ctx.label("u");
    let opts = ChainOpts { t_offset: 0.0, apply_reverse: false, lut, matte };
    let mut fx = c.fx.clone();
    fx.reverse = false;
    ctx.graph.extend(fx_graph(&fx, Src { w: bw, h: bh, has_alpha: image }, &opts, &pl, &fl, &u));
    let s = samples(c, w as f32, h as f32);
    let label = post_transform(ctx, &fl, bw, bh, &s);
    Ok(Sprite { label, x: value_expr(&s, |e| e.x, "TT"), y: value_expr(&s, |e| e.y, "TT") })
}

fn text_sprite(ctx: &mut Ctx, c: &Clip, st: &TextStyle) -> Result<Option<Sprite>> {
    let Some((font_path, idx)) = (ctx.fonts)(st.font_family.as_deref()).or_else(|| default_font().map(|p| (p, 0))) else {
        return Ok(None);
    };
    let n = ctx.label("t");
    let font_file = format!("font_{n}.{}", font_path.extension().and_then(|e| e.to_str()).unwrap_or("ttf"));
    std::fs::copy(&font_path, ctx.workdir.join(&font_file))?;
    let data = std::fs::read(&font_path).unwrap_or_default();
    let size = st.size;
    let mut txt = if st.uppercase { st.text.to_uppercase() } else { st.text.clone() };
    if st.auto_wrap {
        txt = cl_fonts::wrap_text(&data, idx, size, &txt, ctx.w as f32 * 0.9 / c.transform.scale.max(0.05));
    }
    let lines: Vec<&str> = txt.split('\n').collect();
    let tw = lines
        .iter()
        .map(|l| cl_fonts::text_width(&data, idx, size, l).unwrap_or(size * 0.6 * l.chars().count() as f32))
        .fold(0.0f32, f32::max);
    let ls = st.line_spacing.max(0.5);
    let th = size * 1.3 * ls * lines.len() as f32;
    let outline = st.outline.map(|o| o.0).unwrap_or(0.0);
    let pad = (size * 0.4 + outline * 2.0 + 8.0).ceil();
    let lw = ((tw + pad * 2.0).ceil() as u32 + 1) & !1;
    let lh = ((th + pad * 2.0).ceil() as u32 + 1) & !1;
    let text_file = format!("text_{n}.txt");
    std::fs::write(ctx.workdir.join(&text_file), txt.as_bytes())?;
    let rgba = |c: [u8; 4]| format!("0x{:02x}{:02x}{:02x}@{:.3}", c[0], c[1], c[2], c[3] as f32 / 255.0);
    let align = match st.align {
        a if a < 0 => "L",
        a if a > 0 => "R",
        _ => "C",
    };
    let mut dt = format!(
        "drawtext=fontfile={font_file}:textfile={text_file}:fontsize={size:.1}:fontcolor={}:text_align={align}:x=(w-text_w)/2:y=(h-text_h)/2:line_spacing={:.1}",
        rgba(st.color),
        size * 0.3 * ls + size * (ls - 1.0).max(0.0)
    );
    if let Some((bw, col)) = st.outline {
        dt.push_str(&format!(":borderw={bw:.1}:bordercolor={}", rgba(col)));
    }
    if let Some(bg) = st.background {
        dt.push_str(&format!(":box=1:boxcolor={}:boxborderw={:.0}", rgba(bg), (size * 0.25).max(4.0)));
    }
    if st.shadow {
        dt.push_str(":shadowx=3:shadowy=3:shadowcolor=black@0.6");
    }
    let dur = c.duration.as_secs_f64();
    let src = ctx.label("tx");
    ctx.graph.push(format!("color=c=black@0.0:s={lw}x{lh}:r={}:d={},format=rgba,{dt}[{src}]", ctx.fps, f(dur)));
    let s = samples(c, ctx.w as f32, ctx.h as f32);
    let label = post_transform(ctx, &src, lw, lh, &s);
    Ok(Some(Sprite { label, x: value_expr(&s, |e| e.x, "TT"), y: value_expr(&s, |e| e.y, "TT") }))
}

/// Sprite zur Zeit `t0` (s, relativ zum Exportbeginn, auch negativ) auf `cur` legen.
fn place(ctx: &mut Ctx, cur: &str, sp: &Sprite, t0: f64, len: f64) -> String {
    let shifted = ctx.label("pl");
    let shift = if t0 >= 0.0 { format!("setpts=PTS-STARTPTS+{}/TB", f(t0)) } else { format!("trim=start={},setpts=PTS-STARTPTS", f(-t0)) };
    ctx.graph.push(format!("[{}]{shift}[{shifted}]", sp.label));
    let var = format!("(t-{})", f(t0));
    let x = sp.x.replace("TT", &var);
    let y = sp.y.replace("TT", &var);
    let out = ctx.label("base");
    let (a, b) = (t0.max(0.0), t0 + len);
    ctx.graph.push(format!(
        "[{cur}][{shifted}]overlay=x='(W-w)/2+{x}':y='(H-h)/2+{y}':eval=frame:eof_action=pass{}:enable='between(t,{},{})'[{out}]",
        if ctx.alpha { ":format=auto" } else { "" },
        f(a),
        f(b)
    ));
    out
}

/// Clips einer Spur in Gruppen, die über Übergänge verbunden sind.
fn transition_groups(tr: &Track) -> Vec<Vec<&Clip>> {
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

fn sprite_for(ctx: &mut Ctx, p: &Project, c: &Clip) -> Result<Option<Sprite>> {
    match &c.content {
        ClipContent::Media { media, kind: kind @ (MediaKind::Video | MediaKind::Image) } => {
            let m = p.media(*media).expect("geprüft").clone();
            media_sprite(ctx, c, &m, *kind == MediaKind::Image).map(Some)
        }
        ClipContent::Text(st) => text_sprite(ctx, c, st),
        _ => Ok(None),
    }
}

/// Mehrere Clips mit Übergängen zu einem Vollbild-Strom verbinden (xfade, Länge bleibt).
fn transition_chain(ctx: &mut Ctx, p: &Project, group: &[&Clip]) -> Result<Option<Sprite>> {
    let (w, h) = (ctx.w, ctx.h);
    let mut acc: Option<(String, f64)> = None;
    for (k, c) in group.iter().enumerate() {
        let Some(sp) = sprite_for(ctx, p, c)? else { continue };
        let len = c.duration.as_secs_f64();
        let d_of = |cl: &Clip| cl.transition_out.as_ref().map(|t| t.duration.as_secs_f64()).unwrap_or(0.0);
        let d_prev = if k > 0 { d_of(group[k - 1]).min(len).min(group[k - 1].duration.as_secs_f64()) } else { 0.0 };
        let d_next = if k + 1 < group.len() { d_of(c).min(len).min(group[k + 1].duration.as_secs_f64()) } else { 0.0 };
        let canvas = ctx.label("cv");
        ctx.graph.push(format!("color=c=black@0.0:s={w}x{h}:r={}:d={},format=rgba[{canvas}]", ctx.fps, f(len)));
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
        acc = Some(match acc {
            None => (layer, layer_len),
            Some((a, a_len)) => {
                let tr = group[k - 1].transition_out.as_ref().expect("Gruppe");
                let out = ctx.label("xf");
                let d = d_prev.max(0.04);
                let off = (a_len - d).max(0.0);
                ctx.graph.push(format!("[{a}][{layer}]xfade=transition={}:duration={}:offset={}[{out}]", tr.id, f(d), f(off)));
                (out, a_len + layer_len - d)
            }
        });
    }
    Ok(acc.map(|(label, _)| {
        let out = format!("{label}r");
        ctx.graph.push(format!("[{label}]format=rgba[{out}]"));
        Sprite { label: out, x: "0".into(), y: "0".into() }
    }))
}

/// Projekt in Exportgröße umrechnen (Positionen/Schriftgrößen in Projektpixeln).
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

/// Videoteil: alle sichtbaren Spuren von unten nach oben. Gibt das letzte Label zurück.
pub(crate) fn video_part(ctx: &mut Ctx, p: &Project, end: Time, dur: Time) -> Result<String> {
    let (w, h) = (ctx.w, ctx.h);
    let base = if ctx.alpha { "color=c=black@0.0" } else { "color=c=black" };
    ctx.graph.push(format!("{base}:s={w}x{h}:r={}:d={}{}[base0]", ctx.fps, f(dur.as_secs_f64()), if ctx.alpha { ",format=rgba" } else { "" }));
    let mut cur = "base0".to_string();
    let proj = scaled_project(p, w);
    for tr in proj.timeline.tracks.iter().filter(|t| !t.hidden) {
        for group in transition_groups(tr) {
            let g_start = group[0].start;
            let g_end = group.last().expect("nicht leer").end();
            if g_end <= ctx.start || g_start >= end {
                continue;
            }
            let sprite = if group.len() > 1 { transition_chain(ctx, &proj, &group)? } else { sprite_for(ctx, &proj, group[0])? };
            let Some(sp) = sprite else { continue };
            let t0 = (g_start - ctx.start).as_secs_f64();
            cur = place(ctx, &cur, &sp, t0, (g_end - g_start).as_secs_f64());
        }
    }
    Ok(cur)
}

/// Ton: alle hörbaren Clips (Geschwindigkeit, Rückwärts, Lautstärke-Keyframes,
/// Ein-/Ausblenden, Verlängerung mit Stille) werden gemischt.
pub(crate) fn audio_part(p: &Project, start: Time, end: Time, inputs: &mut Vec<String>, next_input: &mut usize, graph: &mut Vec<String>) -> String {
    let mut labels = Vec::new();
    for tr in &p.timeline.tracks {
        if tr.muted || !matches!(tr.kind, TrackKind::Video | TrackKind::Audio) {
            continue;
        }
        for c in tr.clips.iter().filter(|c| !c.disabled) {
            let ClipContent::Media { media, kind } = &c.content else { continue };
            if *kind == MediaKind::Image || c.fx.speed_curve.is_some() {
                continue;
            }
            let Some(m) = p.media(*media) else { continue };
            if !m.has_audio || (c.volume_db <= -59.9 && c.keyframes.get(Prop::Volume).is_empty()) {
                continue;
            }
            if c.end() <= start || c.start >= end {
                continue;
            }
            let dur = c.duration.as_secs_f64();
            let (seg_in, seg_len) = source_segment(c);
            let i = *next_input;
            *next_input += 1;
            inputs.extend(["-ss".into(), f(seg_in.as_secs_f64()), "-t".into(), f(seg_len.as_secs_f64().max(0.02)), "-i".into(), m.path.to_string_lossy().into_owned()]);
            let mut chain = vec!["aresample=48000".to_string(), "aformat=sample_fmts=fltp:channel_layouts=stereo".into(), "asetpts=PTS-STARTPTS".into()];
            if c.fx.reverse {
                chain.push("areverse".into());
            }
            let at = atempo_chain(c.speed);
            if !at.is_empty() {
                chain.push(at);
            }
            chain.extend(cl_fx::audio::audio_chain(&c.audio));
            // Länger gezogener Ton: mit Stille auffüllen
            chain.push(format!("apad=whole_dur={}", f(dur)));
            chain.push(format!("atrim=0:{}", f(dur)));
            chain.push("asetpts=PTS-STARTPTS".into());
            let vol = c.keyframes.get(Prop::Volume);
            if vol.is_empty() {
                if c.volume_db.abs() > 0.01 {
                    chain.push(format!("volume={:.2}dB", c.volume_db));
                }
            } else {
                let pts: Vec<(f64, f64)> = vol.iter().map(|k| (k.t.as_secs_f64(), k.v as f64)).collect();
                chain.push(format!("volume='pow(10,({})/20)':eval=frame", piecewise_expr(&pts, "t")));
            }
            if c.fade_in > Time::ZERO {
                chain.push(format!("afade=t=in:d={}", f(c.fade_in.as_secs_f64())));
            }
            if c.fade_out > Time::ZERO {
                chain.push(format!("afade=t=out:st={}:d={}", f(dur - c.fade_out.as_secs_f64()), f(c.fade_out.as_secs_f64())));
            }
            let t0 = (c.start - start).as_secs_f64();
            if t0 >= 0.0 {
                let ms = (t0 * 1000.0).round() as i64;
                chain.push(format!("adelay={ms}|{ms}"));
            } else {
                chain.push(format!("atrim=start={}", f(-t0)));
                chain.push("asetpts=PTS-STARTPTS".into());
            }
            let label = format!("a{i}");
            graph.push(format!("[{i}:a]{}[{label}]", chain.join(",")));
            labels.push(label);
        }
    }
    let dur = f((end - start).as_secs_f64());
    if labels.is_empty() {
        graph.push(format!("anullsrc=r=48000:cl=stereo,atrim=0:{dur}[aout]"));
    } else {
        let ins: String = labels.iter().map(|l| format!("[{l}]")).collect();
        graph.push(format!(
            "{ins}amix=inputs={}:normalize=0:duration=longest,apad=whole_dur={dur},atrim=0:{dur},aformat=sample_fmts=fltp:channel_layouts=stereo[aout]",
            labels.len()
        ));
    }
    "aout".into()
}
