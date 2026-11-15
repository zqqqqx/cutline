//! Filtergraph-Aufbau (Phase 2): Jeder Clip wird ein eigenes Bild (Sprite) mit Effektkette
//! (cl-fx) und Ausdrücken für Position, Größe, Drehung und Deckkraft aus Keyframes und
//! Animationen. Clips mit Übergang werden per `xfade` zu einem Strom verbunden; die
//! Gesamtlänge bleibt dabei gleich (Bild an den Schnitten wird gehalten).

use std::path::Path;

use cl_core::*;
use cl_fx::{atempo_chain, fx_graph, piecewise_expr, speed_setpts, ChainOpts, Src};

use crate::{default_font, f, ExportError, FontResolver, Result};

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
    /// Shader-Übergänge vorbereiten (aus im Vorlauf selbst)
    pub shader: bool,
    /// Vorläufe für Shader-Übergänge (laufen vor dem Export, siehe [`ShaderPass`])
    pub passes: Vec<ShaderPass>,
    /// Nummer des ersten Vorlaufs (Abschnitte teilen sich einen Arbeitsordner)
    pub pass_base: usize,
    /// Abschnitt eines großen Exports: lange Clips springen per `-ss` nahe an den
    /// Abschnittsanfang (siehe [`lead_skip`]), statt ab Clipbeginn zu dekodieren
    pub lead: bool,
}

/// Vorlauf eines Shader-Übergangs. Ein FFmpeg liest die letzten Bilder von Clip A und die ersten
/// von Clip B (gleiche Clip-Ketten wie im Hauptgraph, nebeneinander als RGBA), cl-gpu mischt sie,
/// ein zweites FFmpeg schreibt sie verlustfrei (FFV1) nach `out_file`. Im Hauptgraph ersetzt
/// `block` das Ergebnis des `xfade` (Rückfall) genau in diesem Fenster; scheitert der Vorlauf,
/// kommt `fallback` an seine Stelle und es bleibt beim `xfade`.
pub(crate) struct ShaderPass {
    pub inputs: Vec<String>,
    pub graph_file: String,
    pub out_file: String,
    pub w: u32,
    pub h: u32,
    pub fps: String,
    pub shader: String,
    pub params: Vec<f32>,
    /// Fortschritt je Bild (wie `xfade`: aus der Bildzeit)
    pub progress: Vec<f32>,
    pub block: String,
    pub fallback: String,
}

/// Bildrate als Zahl (`30000/1001` → 29,97).
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

/// Bilder des Übergangsfensters im verbundenen Strom: (erstes Bild, Anzahl). `xfade` mischt die
/// Bilder ab der ersten Bildzeit ≥ `off` bis einschließlich `off + d` (Bilder liegen auf n/fps).
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
    // FFmpeg lehnt Bilder über ~268 Mio. Pixel ab (der Export brach bei Zoom 20× auf 1080p ab):
    // Diagonale höchstens 14 000 px.
    // shortcut: größerer Zoom wird gekappt statt vorher auf den sichtbaren Teil zugeschnitten; zuschneiden, wenn das jemand braucht
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

/// Filter, deren Ergebnis von früheren Bildern abhängt (Zufall je Bild, Mittelung über Bilder,
/// Rauschfilter mit Gedächtnis): mit ihnen darf ein Abschnitt nicht mitten im Clip einsteigen.
const STATEFUL: &[&str] = &["noise", "tmix", "deflicker", "hqdn3d", "random", "minterpolate", "deshake", "lagfun", "atadenoise", "tblend", "sendcmd"];

/// Ganze Sekunden, die ein Abschnitt (Beginn `start`) vom Anfang eines früher beginnenden Clips
/// überspringen darf (Eingang per `-ss` näher heran, statt ab Clipbeginn zu dekodieren), mit
/// mindestens 1 s Vorlauf für Decoder und Bildraten-Umrechnung. 0 für Clips, deren Bild oder
/// Ton von der Vorgeschichte abhängt (Tempo, Rückwärts, Tempo-Kurve, Zeitlupe, Freistellung).
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

/// Hat die Effektkette des Clips Filter mit Gedächtnis ([`STATEFUL`])?
fn fx_has_memory(fx: &ClipFx) -> bool {
    let opts = ChainOpts { t_offset: 0.0, apply_reverse: false, lut: None, matte: None };
    let g = fx_graph(fx, Src { w: 64, h: 64, has_alpha: false }, &opts, "i", "o", "u").join(";");
    STATEFUL.iter().any(|s| g.contains(s))
}

fn media_sprite(ctx: &mut Ctx, c: &Clip, m: &MediaRef, image: bool, skip: f64) -> Result<Sprite> {
    let (w, h) = (ctx.w, ctx.h);
    let dur = c.duration.as_secs_f64();
    let path = m.path.to_string_lossy().into_owned();
    let (_, seg_len) = source_segment(c);
    // Bewegtes Bild (GIF/APNG): einmal abspielen und letztes Bild halten bzw. als Schleife;
    // `-loop 1` kennen nur Standbild-Leser
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
        ctx.input(vec![
            "-ss".into(),
            f(seg_in.as_secs_f64() + skip),
            "-t".into(),
            f((seg_len.as_secs_f64() - skip).max(0.04)),
            "-i".into(),
            path,
        ])
    };
    // Grundgröße: eingepasst wie im Player (contain)
    let fit = (w as f64 / m.width.max(1) as f64).min(h as f64 / m.height.max(1) as f64);
    let bw = ((m.width as f64 * fit).round() as u32).max(2) & !1;
    let bh = ((m.height as f64 * fit).round() as u32).max(2) & !1;
    // übersprungener Anfang: Zeitstempel bleiben Clipzeit (Animationen, Keyframes, Effekte)
    let mut pre = vec![if skip > 0.0 { format!("setpts=PTS-STARTPTS+{}/TB", f(skip)) } else { "setpts=PTS-STARTPTS".to_string() }];
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
        // Flüssige Zeitlupe: Zwischenbilder per Bewegungsschätzung statt doppelter Bilder
        if c.fx.smooth_slowmo && (c.speed < 0.999 || c.fx.speed_curve.is_some()) {
            pre.push(format!(
                "minterpolate=fps={}:mi_mode=mci:mc_mode=aobmc:me_mode=bidir:vsbmc=1",
                ctx.fps
            ));
        }
        // Videos, die kürzer als der Clip sind (Rundung), halten das letzte Bild
        pre.push(format!("tpad=stop_mode=clone:stop_duration={}", f(0.5)));
    } else if anim.is_some() {
        pre.push(format!("tpad=stop_mode=clone:stop_duration={}", f(dur + 0.1)));
    }
    if skip > 0.0 {
        // erstes Bild genau auf `skip` (ohne Sprung: auf 0), Bilder danach wie ohne Sprung
        pre.push(format!("fps={}:start_time={}", ctx.fps, f(skip)));
    } else {
        pre.push(format!("fps={}", ctx.fps));
    }
    pre.push(format!("scale={bw}:{bh}:flags=bicubic"));
    pre.push(if skip > 0.0 { format!("trim=end={}", f(dur)) } else { format!("trim=duration={}", f(dur)) });
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
        mpre.push(format!("trim=duration={}", f(dur)));
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
    ctx.graph.extend(fx_graph(
        &fx,
        Src {
            w: bw,
            h: bh,
            has_alpha: image,
        },
        &opts,
        &pl,
        &fl,
        &u,
    ));
    let s = samples(c, w as f32, h as f32);
    let label = post_transform(ctx, &fl, bw, bh, &s);
    Ok(Sprite {
        label,
        x: value_expr(&s, |e| e.x, "TT"),
        y: value_expr(&s, |e| e.y, "TT"),
    })
}

/// Neigung kursiver Schrift: obere Kante um ¼ der Höhe nach rechts (wie egui in der Vorschau).
const ITALIC_SHEAR: f32 = 0.25;

fn text_sprite(ctx: &mut Ctx, c: &Clip, st: &TextStyle) -> Result<Option<Sprite>> {
    let Some((font_path, idx)) =
        (ctx.fonts)(st.font_family.as_deref()).or_else(|| default_font().map(|p| (p, 0)))
    else {
        return Ok(None);
    };
    let n = ctx.label("t");
    // eine Kopie je Schriftdatei, nicht je Textclip (1000 Untertitel × 15-MB-Schrift)
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
    // Projektdatei/MCP können 1e6 enthalten (MCP setzt höchstens 600)
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
    let tw = lines
        .iter()
        .map(|l| {
            cl_fonts::text_width(&data, idx, size, l)
                .unwrap_or(size * 0.6 * l.chars().count() as f32)
        })
        .fold(0.0f32, f32::max);
    let ls = st.line_spacing.max(0.5);
    let th = size * 1.3 * ls * lines.len() as f32;
    let outline = st.outline.map(|o| o.0).unwrap_or(0.0);
    let pad = (size * 0.4 + outline * 2.0 + 8.0).ceil();
    // sehr lange Zeilen ohne Umbruch: Fläche begrenzen (Rest liegt ohnehin außerhalb des Bilds)
    // kursiv: Scherung braucht seitlich Platz
    let slant = if st.italic { ITALIC_SHEAR * th } else { 0.0 };
    let lw = (((tw + pad * 2.0 + slant).ceil() as u32 + 1) & !1).min(8192);
    let lh = (((th + pad * 2.0).ceil() as u32 + 1) & !1).min(8192);
    let text_file = format!("text_{n}.txt");
    std::fs::write(ctx.workdir.join(&text_file), txt.as_bytes())?;
    let rgba = |c: [u8; 4]| {
        format!(
            "0x{:02x}{:02x}{:02x}@{:.3}",
            c[0],
            c[1],
            c[2],
            c[3] as f32 / 255.0
        )
    };
    let align = match st.align {
        a if a < 0 => "L",
        a if a > 0 => "R",
        _ => "C",
    };
    let core = |dx: f32| {
        format!(
            "drawtext=fontfile={font_file}:textfile={text_file}:expansion=none:fontsize={size:.1}:fontcolor={}:text_align={align}:x=(w-text_w)/2+{dx:.1}:y=(h-text_h)/2:line_spacing={:.1}",
            rgba(st.color),
            size * 0.3 * ls + size * (ls - 1.0).max(0.0)
        )
    };
    let mut dt = core(0.0);
    if let Some((bw, col)) = st.outline {
        dt.push_str(&format!(":borderw={bw:.1}:bordercolor={}", rgba(col)));
    }
    if let Some(bg) = st.background {
        dt.push_str(&format!(
            ":box=1:boxcolor={}:boxborderw={:.0}",
            rgba(bg),
            (size * 0.25).max(4.0)
        ));
    }
    if st.shadow {
        dt.push_str(":shadowx=3:shadowy=3:shadowcolor=black@0.6");
    }
    // Fett und kursiv wie die Vorschau (`draw_text` im Player) aus den Schaltern des Stils, nicht
    // aus dem Schnitt der Schriftdatei: zweiter Abdruck um 3 % der Größe versetzt, Scherung
    if st.bold {
        dt.push(',');
        dt.push_str(&core(size * 0.03));
    }
    if st.italic {
        dt.push_str(&format!(",shear=shx={ITALIC_SHEAR}:fillcolor=black@0"));
    }
    let dur = c.duration.as_secs_f64();
    let src = ctx.label("tx");
    ctx.graph.push(format!(
        "color=c=black@0.0:s={lw}x{lh}:r={}:d={},format=rgba,{dt}[{src}]",
        ctx.fps,
        f(dur)
    ));
    let s = samples(c, ctx.w as f32, ctx.h as f32);
    let label = post_transform(ctx, &src, lw, lh, &s);
    Ok(Some(Sprite {
        label,
        x: value_expr(&s, |e| e.x, "TT"),
        y: value_expr(&s, |e| e.y, "TT"),
    }))
}

/// Sprite zur Zeit `t0` (s, relativ zum Exportbeginn, auch negativ) auf `cur` legen.
fn place(ctx: &mut Ctx, cur: &str, sp: &Sprite, t0: f64, len: f64) -> String {
    let shifted = ctx.label("pl");
    let shift = if t0 >= 0.0 {
        format!("setpts=PTS-STARTPTS+{}/TB", f(t0))
    } else {
        format!("trim=start={},setpts=PTS-STARTPTS", f(-t0))
    };
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

/// Effekt-/Filter-Element: Gesamtbild bis hierher kopieren, Effekt anwenden (optional nur
/// innerhalb der Maske) und für die Dauer des Elements darüberlegen. Mit Stärke-Keyframes wird
/// das Ergebnis zeitabhängig mit dem Original gemischt.
fn effect_layer(ctx: &mut Ctx, cur: &str, c: &Clip, r: &FxRef, filter: bool, t0: f64) -> Result<String> {
    let len = c.duration.as_secs_f64();
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
    // Nur das aktive Zeitfenster wird bearbeitet (nicht das ganze Video)
    let (s0, s1) = (t0.max(0.0), t0 + len);
    let window = format!("trim=start={}:end={},setpts=PTS-STARTPTS", f(s0), f(s1));
    let (a, b, bw, fo) = (ctx.label("ea"), ctx.label("eb"), ctx.label("ew"), ctx.label("ef"));
    let o = ctx.label("eo");
    // Ohne Stärke-Keyframes nur zwei Abzweige: ein dritter mit nullsink lässt ffmpeg das
    // ganze Video vorab durchrechnen (lange Startpause ohne Fortschritt)
    if kfs.is_empty() {
        ctx.graph.push(format!("[{cur}]split=2[{a}][{b}]"));
    } else {
        ctx.graph.push(format!("[{cur}]split=3[{a}][{b}][{o}]"));
    }
    ctx.graph.push(format!("[{b}]{window}[{bw}]"));
    let uniq = ctx.label("e");
    let opts = ChainOpts {
        // beginnt der Export mitten im Element, läuft die Effektzeit dort weiter (nicht ab 0)
        t_offset: s0 - t0,
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
    // Effektzeit lief ab `s0 - t0`: für das Überlagern wieder ab 0
    let rebase = if s0 - t0 > 0.0 { "setpts=PTS-STARTPTS," } else { "" };
    if kfs.is_empty() {
        ctx.graph.push(format!("[{fo}]{rebase}format=rgba,setsar=1[{pre}]"));
    } else {
        // Maske einbacken (Effekt über Original), dann die Deckkraft je Bild per sendcmd
        // setzen (schnell, statt einer Formel je Pixel)
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
        let lo = s0 - t0; // Versatz, falls der Export mitten im Element beginnt
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
    // Leerbilder aus eigener Quelle davor (statt PTS-Versatz): sonst wartet overlay auf das
    // erste Effektbild und puffert das ganze Video bis s0 (lange Startpause, viel RAM)
    if s0 > 0.0 {
        let pad = ctx.label("epad");
        ctx.graph.push(format!(
            "color=c=black@0.0:s={}x{}:r={}:d={},format=rgba,setsar=1[{pad}]",
            ctx.w,
            ctx.h,
            ctx.fps,
            f(s0)
        ));
        ctx.graph.push(format!("[{pad}][{pre}]concat=n=2:v=1:a=0[{top}]"));
    } else {
        ctx.graph.push(format!("[{pre}]null[{top}]"));
    }
    ctx.graph.push(format!(
        "[{a}][{top}]overlay=eof_action=pass{fmt}:enable='between(t,{},{})'[{out}]",
        f(s0),
        f(s1)
    ));
    Ok(out)
}

/// Clips einer Spur in Gruppen, die über Übergänge verbunden sind.
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

/// Ebene des `k`-ten Clips einer Übergangsgruppe: Vollbild, vorn und hinten um den halben
/// Übergang verlängert (Bild gehalten). (Label, Länge, Dauer des Übergangs davor)
fn group_layer(ctx: &mut Ctx, p: &Project, group: &[&Clip], k: usize) -> Result<Option<(String, f64, f64)>> {
    let (w, h) = (ctx.w, ctx.h);
    let c = group[k];
    let Some(sp) = sprite_for(ctx, p, c, 0.0)? else {
        return Ok(None);
    };
    {
        let len = c.duration.as_secs_f64();
        // gleiche Regel wie die Vorschau
        let d_prev = if k > 0 { cl_fx::transition_secs(group[k - 1], c) } else { 0.0 };
        let d_next = if k + 1 < group.len() { cl_fx::transition_secs(c, group[k + 1]) } else { 0.0 };
        let canvas = ctx.label("cv");
        ctx.graph.push(format!(
            "color=c=black@0.0:s={w}x{h}:r={}:d={},format=rgba[{canvas}]",
            ctx.fps,
            f(len)
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

/// Mehrere Clips mit Übergängen zu einem Vollbild-Strom verbinden (xfade, Länge bleibt).
/// Shader-Übergänge bekommen zusätzlich einen Vorlauf ([`ShaderPass`]).
fn transition_chain(ctx: &mut Ctx, p: &Project, group: &[&Clip]) -> Result<Option<Sprite>> {
    let fps = fps_value(&ctx.fps);
    let mut acc: Option<(String, f64)> = None;
    // Bild im verbundenen Strom, an dem die vorige Ebene beginnt, und Ende des vorigen Fensters
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
                // Id kommt ungeprüft aus Projektdatei/MCP: der Katalog liefert nur eingebaute
                // xfade-Namen (Paket-Übergänge: ihr Rückfall), alles andere wird „fade“
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
                // shortcut: überlappen sich zwei Fenster (Clip kürzer als die halben Übergänge), bleibt
                // der zweite beim xfade-Rückfall; ausbauen, wenn das bei echten Projekten auffällt
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

/// Vorlauf für den Shader-Übergang zwischen `group[k-1]` und `group[k]` anlegen; gibt das Label
/// zurück, das den `xfade`-Ausgang `xf` im Hauptgraph ersetzt. Fenster `win` = (erstes Bild, Anzahl)
/// im verbundenen Strom, `a_start` = Bild, an dem Clip A dort beginnt, `od` = (Versatz, Dauer).
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
    // eigener Graph mit denselben Clip-Ketten; eigene Labels/Dateinamen (Zähler weit weg)
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
    // Bild gehalten, falls die Ebene ein Bild zu früh endet (Rundung)
    sub.graph.push(format!("[{la}]trim=start_frame={a0},setpts=PTS-STARTPTS,tpad=stop_mode=clone:stop_duration=1[pa]"));
    sub.graph.push(format!("[{lb}]setpts=PTS-STARTPTS,tpad=stop_mode=clone:stop_duration=1[pb]"));
    sub.graph.push("[pa][pb]hstack=inputs=2:shortest=1,format=rgba[pout]".into());
    let graph_file = format!("shader_{idx}.txt");
    std::fs::write(ctx.workdir.join(&graph_file), sub.graph.join(";\n"))?;
    let fps = fps_value(&ctx.fps);
    let progress = (0..n)
        .map(|j| ((((i0 + j) as f64 / fps) - off) / d).clamp(0.0, 1.0) as f32)
        .collect();
    let out_file = format!("shader_{idx}.nut");
    let (sv, res) = (ctx.label("shv"), ctx.label("xs"));
    // Zeitbasis 1/fps: Bild j liegt genau auf Bild i0 + j des Stroms
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
                let t0 = (c.start - ctx.start).as_secs_f64();
                cur = effect_layer(ctx, &cur, c, r, filter, t0)?;
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
                let skip = if ctx.lead && video && !fx_has_memory(&c.fx) { lead_skip(c, ctx.start) } else { 0.0 };
                sprite_for(ctx, &proj, c, skip)?
            };
            let Some(sp) = sprite else { continue };
            let t0 = (g_start - ctx.start).as_secs_f64();
            cur = place(ctx, &cur, &sp, t0, (g_end - g_start).as_secs_f64());
        }
    }
    Ok(cur)
}

/// Abschnitte einer Tempo-Kurve: (Quellstart s, Quellende s, Tempo) für gleich lange Stücke
/// der Clipzeit `dur`; `src` = Länge des Quellbereichs.
fn curve_pieces(id: &str, src: f64, dur: f64) -> Vec<(f64, f64, f32)> {
    const N: usize = 24;
    let at = |k: usize| curve_progress(id, k as f32 / N as f32) as f64 * src;
    (0..N)
        .map(|k| (at(k), at(k + 1), ((at(k + 1) - at(k)) / (dur / N as f64).max(1e-6)) as f32))
        .collect()
}

/// Hörbare Clips (mit Medium), die den Bereich `start..end` berühren – in Spur- und
/// Clipreihenfolge (die Reihenfolge der Eingänge im Mix).
pub(crate) fn audible_clips(p: &Project, start: Time, end: Time) -> impl Iterator<Item = (&Clip, &MediaRef)> {
    p.timeline
        .tracks
        .iter()
        .filter(|tr| !tr.muted && matches!(tr.kind, TrackKind::Video | TrackKind::Audio))
        .flat_map(|tr| tr.clips.iter())
        .filter(move |c| !c.disabled && c.end() > start && c.start < end)
        .filter(|c| c.volume_db > -59.9 || !c.keyframes.get(Prop::Volume).is_empty())
        .filter_map(|c| match &c.content {
            ClipContent::Media { media, kind } if *kind != MediaKind::Image => {
                p.media(*media).filter(|m| m.is_local() && m.has_audio).map(|m| (c, m))
            }
            _ => None,
        })
}

/// Wie der Ton-Mix gebaut wird.
#[derive(Clone, Copy, Default)]
pub(crate) struct AudioOpts {
    /// Export wartet auf die Kanalprüfung, die Wiedergabe nicht (sonst friert die Oberfläche ein)
    pub wait_probe: bool,
    /// Begrenzer am Ende; in Abschnitten aus, dort läuft er einmal über das Ganze
    pub limiter: bool,
    /// Abschnitt: lange Clips per `-ss` nahe an den Anfang (siehe [`lead_skip`])
    pub lead: bool,
    /// Ein Eingang je Datei und Stelle statt je Clip (Vorschau-Fenster mit sehr vielen Clips)
    pub shared: bool,
    /// Dateien, deren Ton FFmpeg nicht lesen kann, auslassen statt den ganzen Mix scheitern
    /// zu lassen (Vorschau; siehe [`crate::audio_readable`])
    pub skip_bad: bool,
}

/// Tonspur der Datei, die ein Clip hört (Stromangabe hinter der Eingangsnummer, z. B. `a`).
/// Die einzige Stelle, an der die Spur gewählt wird – Export, Abschnitte und Vorschau.
pub(crate) fn audio_stream(c: &Clip, m: &MediaRef) -> String {
    // gewählte Tonspur der Datei; fehlt sie (andere Datei verknüpft), die erste
    match c.audio_stream.filter(|&n| n > 0 && n < m.audio_streams) {
        Some(n) => format!("a:{n}"),
        None => "a".into(),
    }
}

/// Eingänge zusammenlegen: Quellbereiche `(Schlüssel, Start, Ende)` mit gleichem Schlüssel, die
/// höchstens `gap` Sekunden auseinanderliegen, teilen sich einen Eingang. Liefert je Bereich die
/// Nummer seines Eingangs und je Eingang `(Schlüssel, Start, Ende)`.
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

/// Ton: alle hörbaren Clips (Geschwindigkeit, Rückwärts, Lautstärke-Keyframes,
/// Ein-/Ausblenden, Verlängerung mit Stille) werden gemischt.
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
    // gemeinsame Eingänge: je Clip (Eingang, Versatz im Eingang); Eingang wird mit asplit verteilt
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
        // Sprung an den Abschnittsanfang nur ohne Tempo und ohne Tonbearbeitung mit Gedächtnis
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
        // Kanäle: z. B. Mikrofon nur links → auf beide Ohren
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
            // Tempo-Kurve: Quelle in Abschnitte teilen, je Abschnitt konstantes atempo
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
        // Länger gezogener Ton: mit Stille auffüllen (gezählt ab dem ersten gelesenen Sample)
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
        // Blenden höchstens so lang wie der Clip (Teilen/Tempo/Trimmen kürzen ihn), sonst
        // lehnt FFmpeg ein negatives `st` ab
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
            // Stille aus eigener Quelle davor statt adelay: bei langem Versatz liefert
            // adelay lange nichts, der Muxer hält dann das Video zurück (kein Fortschritt)
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
        let ins: String = labels.iter().map(|l| format!("[{l}]")).collect();
        let limit = if o.limiter { format!("{},", limiter()) } else { String::new() };
        graph.push(format!(
            "{ins}amix=inputs={}:normalize=0:duration=longest,{limit}apad=whole_dur={dur},atrim=0:{dur},aformat=sample_fmts=fltp:channel_layouts=stereo[aout]",
            labels.len(),
        ));
    }
    "aout".into()
}

/// Begrenzer gegen Übersteuern überlagerter Clips; darunter bitgleich (level/latency aus bzw. ausgeglichen)
pub(crate) fn limiter() -> String {
    format!("alimiter=limit={}:level=false:latency=true", f(10f64.powf(LIMIT_DB / 20.0)))
}

/// Spitze nach dem Mischen; Abstand zu −1 dBFS, weil AAC/MP3 beim Kodieren leicht überschwingen.
pub(crate) const LIMIT_DB: f64 = -1.5;
