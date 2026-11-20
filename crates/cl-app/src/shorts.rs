//! From a long video to a short: hit list from „Highlights finden“, one 9:16 project
//! per hit, refit clips when the aspect ratio changes.

use cl_core::*;
use eframe::egui::{self, RichText};

use crate::app::App;
use crate::captions::CapPreset;
use crate::paths;
use crate::tasks::TaskDone;
use crate::theme::{self, icon};

/// A found moment: project time (s), strength (level / surroundings), chosen
pub struct Hit {
    pub at: f64,
    pub strength: f32,
    pub on: bool,
}

pub struct ShortsState {
    pub hits: Vec<Hit>,
    /// Length per short in seconds
    pub len: f32,
    pub captions: bool,
}

/// How much louder the moment `t` (s) is than its surroundings (±5 s).
pub fn strength(env: &[f32], t: f64) -> f32 {
    let r = cl_analyze::audio::ENV_RATE as f64;
    let mean = |a: f64, b: f64| {
        let (a, b) = (((a * r).max(0.0) as usize).min(env.len()), ((b * r) as usize).min(env.len()));
        env[a..b].iter().sum::<f32>() / (b - a).max(1) as f32
    };
    mean(t - 0.25, t + 0.25) / mean(t - 5.0, t + 5.0).max(1e-6)
}

/// Time range of a short around the moment `at`: lead-in before it, kept within the project.
pub fn short_range(at: f64, len: f64, total: f64) -> (f64, f64) {
    let a = (at - len * 0.6).clamp(0.0, (total - len).max(0.0));
    (a, (a + len).min(total))
}

/// Scaled-down overlay (picture-in-picture, facecam): scale or a scale keyframe below 0.95.
pub fn is_shrunk_overlay(c: &Clip) -> bool {
    c.transform.scale < 0.95 || c.keyframes.0.get(&Prop::Scale).is_some_and(|l| l.iter().any(|k| k.v < 0.95))
}

/// Put a media clip anew into the frame `w`×`h`: fill (crop) or fit entirely.
pub fn fit_clip(c: &mut Clip, m: &MediaRef, w: f32, h: f32, fill: bool) {
    // scaled-down overlays keep position and size
    if is_shrunk_overlay(c) {
        return;
    }
    let (sx, sy) = (w / m.width.max(1) as f32, h / m.height.max(1) as f32);
    // scale 1 = fitted (as in the player/export)
    c.transform.scale = if fill { sx.max(sy) / sx.min(sy) } else { 1.0 };
    c.transform.x = 0.0;
    c.transform.y = 0.0;
    for p in [Prop::X, Prop::Y, Prop::Scale] {
        c.keyframes.0.remove(&p);
    }
}

/// Image clips (video/image with a known size) with their medium
fn visual<'a>(p: &'a Project, c: &Clip) -> Option<&'a MediaRef> {
    match &c.content {
        ClipContent::Media { media, kind: MediaKind::Video | MediaKind::Image } => {
            p.media(*media).filter(|m| m.width > 0 && m.height > 0)
        }
        _ => None,
    }
}

/// Path of the person per clip (points from `cl_analyze::reframe::focus_path`: source seconds from
/// the clip start, center 0…1 in the source frame)
pub type FocusPaths = std::collections::HashMap<ClipId, Vec<(f64, f32, f32)>>;

/// Remembered „Person folgen“ analysis: clip, file, source start, source length (trimming changes it)
pub type FocusKey = (ClipId, std::path::PathBuf, Time, Time);

pub fn focus_key(c: &Clip, m: &MediaRef) -> FocusKey {
    (c.id, m.path.clone(), c.source_time(c.start), c.source_span())
}

/// Own motion (position or zoom keyframes): „Person folgen“ leaves the clip as it is
fn moves(c: &Clip) -> bool {
    [Prop::X, Prop::Y, Prop::Scale].iter().any(|p| c.keyframes.0.get(p).is_some_and(|l| !l.is_empty()))
}

/// Clips that „Person folgen“ follows: frame-filling videos without own motion, with a key
pub fn follow_jobs(p: &Project) -> Vec<(FocusKey, MediaRef)> {
    let (w, h) = (p.settings.width.max(2) as f32, p.settings.height.max(2) as f32);
    p.timeline
        .tracks
        .iter()
        .filter(|t| !t.hidden)
        .flat_map(|t| &t.clips)
        .filter_map(|c| {
            let m = visual(p, c).filter(|m| m.kind == MediaKind::Video)?;
            (!moves(c) && full_frame(c, (m.width as f32, m.height as f32), w, h)).then(|| (focus_key(c, m), m.clone()))
        })
        .collect()
}

/// Path `pts` (see [`FocusPaths`]) as position keyframes in the frame `w`×`h`: the point moves toward
/// the center as far as the image still fully covers the frame; the scale stays.
pub fn follow_keys(c: &mut Clip, m: &MediaRef, (w, h): (f32, f32), pts: &[(f64, f32, f32)]) {
    let k = (w / m.width.max(1) as f32).min(h / m.height.max(1) as f32) * c.transform.scale;
    let (dw, dh) = (m.width as f32 * k, m.height as f32 * k);
    let (mx, my) = (((dw - w) / 2.0).max(0.0), ((dh - h) / 2.0).max(0.0));
    c.keyframes.0.remove(&Prop::X);
    c.keyframes.0.remove(&Prop::Y);
    for (i, (t, fx, fy)) in pts.iter().enumerate() {
        if i % 2 != 0 && i + 1 != pts.len() {
            continue;
        }
        let tl = Time::from_secs_f64(t / c.speed.max(0.01) as f64);
        if tl > c.duration {
            break;
        }
        if mx > 0.5 {
            c.keyframes.set(Prop::X, tl, ((0.5 - fx) * dw).clamp(-mx, mx));
        }
        if my > 0.5 {
            c.keyframes.set(Prop::Y, tl, ((0.5 - fy) * dh).clamp(-my, my));
        }
    }
}

/// ID of the blurred background copy of a clip (`ClipId::derived`)
pub const BACK_SALT: u128 = 0xB1;

/// Frame-filling layer: an image clip, not scaled down, without a crop, or cropped and still
/// covering at least 90 % of the frame (layout cells cover less), position also with keyframes
/// near the center
fn full_frame(c: &Clip, (mw, mh): (f32, f32), w: f32, h: f32) -> bool {
    let near = |p: Prop, v: f32, lim: f32| std::iter::once(v).chain(c.keyframes.0.get(&p).into_iter().flatten().map(|k| k.v)).all(|v| v.abs() <= lim);
    // covered share of an edge `l`: visible range (center `pos`, media edge `m`, crop `a`/`b`)
    let d = c.transform.scale * (w / mw).min(h / mh);
    let part = |pos: f32, m: f32, a: f32, b: f32, l: f32| ((pos + d * m * (0.5 - b)).min(l / 2.0) - (pos + d * m * (a - 0.5)).max(-l / 2.0)).max(0.0) / l;
    let cr = c.crop.clamped();
    let covered = || part(c.transform.x, mw, cr.left, cr.right, w) * part(c.transform.y, mh, cr.top, cr.bottom, h) >= 0.9;
    !is_shrunk_overlay(c) && (c.crop.is_none() || covered()) && near(Prop::X, c.transform.x, 0.1 * w) && near(Prop::Y, c.transform.y, 0.1 * h)
}

/// Center `c` (px from the frame center) of an object with half size `half` on an edge `l` →
/// proportionally onto edge `l2` (half size there `half2`); if it lay fully in the frame, it stays inside.
fn place(c: f32, half: f32, l: f32, half2: f32, l2: f32) -> f32 {
    let v = c * l2 / l;
    if c.abs() + half <= l / 2.0 + 0.5 {
        let m = (l2 / 2.0 - half2).max(0.0);
        v.clamp(-m, m)
    } else {
        v
    }
}

fn scale_keys(c: &mut Clip, p: Prop, f: impl Fn(f32) -> f32) {
    for k in c.keyframes.0.get_mut(&p).into_iter().flatten() {
        k.v = f(k.v);
    }
}

/// Size of a text block (font, wrap width in px; unlimited without `auto_wrap`)
pub type Measure<'a> = &'a dyn Fn(&TextStyle, f32) -> (f32, f32);

/// Text measurement as in the export (wrapping, font from `fonts`); each font is read only once.
pub fn text_measure(fonts: &crate::thumb::Fonts) -> impl Fn(&TextStyle, f32) -> (f32, f32) + '_ {
    let cache = std::cell::RefCell::new(std::collections::HashMap::<Option<String>, std::rc::Rc<(Vec<u8>, u32)>>::new());
    move |st: &TextStyle, wrap: f32| {
        let f = cache.borrow_mut().entry(st.font_family.clone()).or_insert_with(|| crate::thumb::font_data(fonts, st.font_family.as_deref()).into()).clone();
        let mut st = st.clone();
        if st.auto_wrap && wrap.is_finite() {
            let txt = if st.uppercase { st.text.to_uppercase() } else { st.text.clone() };
            st.text = cl_fonts::wrap_text(&f.0, f.1, st.size.clamp(1.0, 1000.0), &txt, wrap);
        }
        crate::thumb::measure_in(&f.0, f.1, &st)
    }
}

/// Rebuild a snapshot for another format `w`×`h` (export in another aspect ratio, shorts).
/// - frame-filling image clips: fill or fit as on a format change (zoom, shift and
///   keyframes proportionally); with „Einpassen“ a muted, blurred, filling copy lies
///   below on each track (same times, transitions, speed, crop, censoring)
/// - placed image clips (picture-in-picture, layouts, logo, sticker) and text: position proportional, size
///   in the same ratio to the shorter edge, what lay fully in the frame stays inside; text is
///   wrapped at the new width or scaled down
/// - masks of effect/filter elements (fraction of the frame) follow the frame-filling content
/// - crop, censoring and clip masks lie in source or clip coordinates and stay
/// - „Person folgen“: frame-filling clips in `focus` without own motion follow their path ([`follow_keys`])
pub fn reframe(p: &Project, (w2, h2): (u32, u32), fill: bool, focus: &FocusPaths, measure: Measure) -> Project {
    let mut out = p.clone();
    let (w, h) = (p.settings.width.max(2) as f32, p.settings.height.max(2) as f32);
    let (nw, nh) = (w2.max(2) as f32, h2.max(2) as f32);
    let g = crate::export_ui::gcd(w2.max(1), h2.max(1));
    (out.settings.width, out.settings.height, out.settings.aspect) = (w2, h2, AspectPreset::Ratio(w2 / g, h2 / g));
    // Content of the frame-filling layers: factor in pixels; objects: ratio of the shorter edges
    let k = if fill { (nw / w).max(nh / h) } else { (nw / w).min(nh / h) };
    let ks = nw.min(nh) / w.min(h);
    let mut backs: Vec<(usize, Track)> = Vec::new();
    for (ti, t) in out.timeline.tracks.iter_mut().enumerate() {
        let mut back: Vec<Clip> = Vec::new();
        let mut needs_back = false;
        for c in &mut t.clips {
            if let Some(m) = visual(p, c) {
                let (mw, mh) = (m.width as f32, m.height as f32);
                let fit = |w: f32, h: f32| (w / mw).min(h / mh);
                // scale 1 = fitted; filling: so large that nothing stays empty
                let cover = { let (sx, sy) = (nw / mw, nh / mh); sx.max(sy) / sx.min(sy) };
                let base = |c: &mut Clip, f: f32| {
                    let q = f * fit(nw, nh) / fit(w, h);
                    c.transform.scale *= f;
                    c.transform.x *= q;
                    c.transform.y *= q;
                    scale_keys(c, Prop::Scale, |v| v * f);
                    scale_keys(c, Prop::X, |v| v * q);
                    scale_keys(c, Prop::Y, |v| v * q);
                };
                if full_frame(c, (mw, mh), w, h) {
                    let orig = c.clone();
                    base(c, if fill { cover } else { 1.0 });
                    if let Some(pts) = focus.get(&c.id).filter(|_| fill && !moves(&orig)) {
                        follow_keys(c, m, (nw, nh), pts);
                    }
                    if !fill {
                        let shown = c.transform.scale * fit(nw, nh);
                        needs_back |= shown * mw < nw - 1.0 || shown * mh < nh - 1.0;
                        let mut b = orig;
                        b.id = c.id.derived(BACK_SALT);
                        b.group = None;
                        base(&mut b, cover);
                        if !b.fx.effects.iter().any(|e| e.id == "blur") {
                            b.fx.effects.push(FxRef::new("blur"));
                        }
                        back.push(b);
                    }
                } else {
                    let cr = c.crop.clamped();
                    let d = c.transform.scale * fit(w, h);
                    let (ox, oy) = (mw * (cr.left - cr.right) / 2.0, mh * (cr.top - cr.bottom) / 2.0);
                    let (hw, hh) = (d * mw * (1.0 - cr.left - cr.right) / 2.0, d * mh * (1.0 - cr.top - cr.bottom) / 2.0);
                    // cropped clips (layout cells): the whole layout fits into the new frame, otherwise they overlap
                    let ks = if cr.is_none() { ks } else { ks.min(nw / w).min(nh / h) };
                    let (f, d2) = (ks * fit(w, h) / fit(nw, nh), d * ks);
                    let px = |v: f32| place(v + d * ox, hw, w, hw * ks, nw) - d2 * ox;
                    let py = |v: f32| place(v + d * oy, hh, h, hh * ks, nh) - d2 * oy;
                    c.transform.scale *= f;
                    (c.transform.x, c.transform.y) = (px(c.transform.x), py(c.transform.y));
                    scale_keys(c, Prop::Scale, |v| v * f);
                    scale_keys(c, Prop::X, px);
                    scale_keys(c, Prop::Y, py);
                }
                continue;
            }
            match &mut c.content {
                ClipContent::Text(st) => {
                    let sc = c.transform.scale.max(0.05);
                    // wrapping as in the export at 90 % of the frame width
                    let aw = st.auto_wrap;
                    let wrap = |cw: f32| if aw { cw * 0.9 / sc } else { f32::INFINITY };
                    let (tw, th) = measure(st, wrap(w));
                    st.size *= ks;
                    if let Some(o) = &mut st.outline {
                        o.0 *= ks;
                    }
                    let (mut tw2, mut th2) = measure(st, wrap(nw));
                    // wider than the new frame (without wrapping, very long words): smaller
                    let shrink = (nw * 0.9 / (tw2 * sc).max(1.0)).min(1.0);
                    if shrink < 1.0 {
                        st.size *= shrink;
                        if let Some(o) = &mut st.outline {
                            o.0 *= shrink;
                        }
                        (tw2, th2) = (tw2 * shrink, th2 * shrink);
                    }
                    let px = |v: f32| place(v, tw * sc / 2.0, w, tw2 * sc / 2.0, nw);
                    let py = |v: f32| place(v, th * sc / 2.0, h, th2 * sc / 2.0, nh);
                    (c.transform.x, c.transform.y) = (px(c.transform.x), py(c.transform.y));
                    scale_keys(c, Prop::X, px);
                    scale_keys(c, Prop::Y, py);
                }
                ClipContent::Effect(_) | ClipContent::Filter(_) => {
                    if let Some(mk) = &mut c.fx.mask {
                        let (ax, ay) = (k * w / nw, k * h / nh);
                        (mk.cx, mk.w) = (0.5 + (mk.cx - 0.5) * ax, mk.w * ax);
                        (mk.cy, mk.h) = (0.5 + (mk.cy - 0.5) * ay, mk.h * ay);
                    }
                }
                ClipContent::Media { .. } => {}
            }
        }
        if needs_back {
            // transition only if the following clip of the track also has a copy
            let copied: Vec<ClipId> = back.iter().map(|b| b.id).collect();
            for b in &mut back {
                let i = t.clips.iter().position(|c| c.id.derived(BACK_SALT) == b.id).expect("Kopie eines Clips der Spur");
                if !t.clips.get(i + 1).is_some_and(|n| copied.contains(&n.id.derived(BACK_SALT))) {
                    b.transition_out = None;
                }
            }
            let mut bt = Track::new(t.kind);
            (bt.muted, bt.hidden, bt.clips) = (true, t.hidden, back);
            backs.push((ti, bt));
        }
    }
    for (ti, bt) in backs.into_iter().rev() {
        out.timeline.tracks.insert(ti, bt);
    }
    out
}

/// Project + time range → new 9:16 project (1080×1920, same frame rate): all tracks,
/// clips trimmed to the range and moved to 0, brought into the format as on an export
/// in another aspect ratio ([`reframe`], filling).
/// `captions`: caption tracks and the style applied to them.
pub fn make_short(p: &Project, a: Time, b: Time, name: &str, captions: Option<(&[TrackId], &CapPreset)>, measure: Measure) -> Project {
    let mut out = p.clone();
    out.name = name.to_string();
    out.settings.platform = None;
    for t in &mut out.timeline.tracks {
        let clips = std::mem::take(&mut t.clips);
        for mut c in clips {
            if c.end() <= a || c.start >= b {
                continue;
            }
            let head = if c.start < a { a - c.start } else { Time::ZERO };
            let tail = if c.end() > b { c.end() - b } else { Time::ZERO };
            let src = |d: Time| Time((d.0 as f64 * c.speed.max(0.01) as f64).round() as i64);
            if c.fx.speed_curve.is_some() && (head > Time::ZERO || tail > Time::ZERO) {
                // shortcut: the trim runs at constant speed over the same source range instead of
                // with the matching curve piece; extend once curves can be cut
                let (s1, s2) = (c.source_time(c.start + head), c.source_time(c.end() - tail));
                let d = c.duration - head - tail;
                c.source_in = s1.min(s2);
                c.speed = ((s2.0 - s1.0).abs() as f64 / d.0.max(1) as f64) as f32;
                c.fx.speed_curve = None;
            } else if c.fx.reverse {
                // as when splitting: reversed, the start shows the end of the source
                c.source_in = c.source_in + src(tail);
            } else {
                c.source_in = c.source_in + src(head);
            }
            c.retime_words(|t| t - head);
            if head > Time::ZERO {
                c.keyframes.shift(head);
                c.anim.intro = None;
                c.fade_in = Time::ZERO;
            }
            if tail > Time::ZERO {
                c.anim.outro = None;
                c.transition_out = None;
                c.fade_out = Time::ZERO;
            }
            c.duration = c.duration - head - tail;
            c.start = if c.start > a { c.start - a } else { Time::ZERO };
            t.clips.push(c);
        }
    }
    out.timeline.markers.retain(|m| m.at >= a && m.at < b);
    for m in &mut out.timeline.markers {
        m.at = m.at - a;
    }
    let mut out = reframe(&out, (1080, 1920), true, &FocusPaths::new(), measure);
    if let Some((ids, pre)) = captions {
        for c in out.timeline.tracks.iter_mut().filter(|t| ids.contains(&t.id)).flat_map(|t| t.clips.iter_mut()) {
            if let ClipContent::Text(s) = &mut c.content {
                *s = TextStyle { text: std::mem::take(&mut s.text), ..(pre.style)() };
                c.transform.y = 1920.0 * pre.y;
                c.anim.intro = pre.intro.map(|id| AnimRef {
                    id: id.into(),
                    duration: Time::from_secs_f64(0.18f64.min(c.duration.as_secs_f64() / 3.0)),
                });
            }
        }
    }
    out
}

impl App {
    /// Window with the hit list of „Highlights finden“
    pub fn shorts_window(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.shorts.take() else {
            return;
        };
        let total = self.project.duration().as_secs_f64();
        let has_caps = !self.caption_clips().is_empty();
        let max = st.hits.iter().map(|h| h.strength).fold(0.0f32, f32::max).max(1e-6);
        let (mut make, mut markers) = (false, false);
        // before drawing: a value field gives up focus already on Enter
        let (esc, enter) = crate::dialog::keys(ctx);
        let mut open = !esc;
        crate::dialog::window(ctx, crate::tr("Highlights"), "shorts")
            .default_width(400.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                if crate::dialog::title(ui, "Highlights") {
                    open = false;
                }
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for h in &mut st.hits {
                        let (a, b) = short_range(h.at, st.len as f64, total);
                        ui.horizontal(|ui| {
                            crate::widgets::check(ui, &mut h.on, "");
                            if ui
                                .add(theme::ghost(fmt_t(h.at)))
                                .on_hover_text(crate::tr("Zum Moment springen"))
                                .clicked()
                            {
                                self.playhead = Time::from_secs_f64(h.at);
                            }
                            ui.label(RichText::new(format!("{:.0} s", b - a)).color(theme::TEXT_DIM()));
                            ui.label(
                                RichText::new(crate::tr(format!("Stärke {:.0} %", h.strength / max * 100.0)))
                                    .color(theme::TEXT_DIM()),
                            );
                        });
                    }
                });
                theme::section(ui, "Short");
                theme::row(ui, "Länge je Short", |ui| {
                    crate::widgets::Slider::new(&mut st.len, 10.0..=60.0).default(30.0).suffix(" s").show(ui);
                });
                ui.add_enabled_ui(has_caps, |ui| {
                    let mut on = st.captions && has_caps;
                    crate::widgets::check(ui, &mut on, crate::tr("Untertitel-Stil anwenden"));
                    st.captions = on;
                    if on {
                        self.caption_preset_picker(ui);
                    }
                });
                if !has_caps {
                    ui.label(
                        RichText::new(crate::tr("Keine Untertitel im Projekt – erst Untertitel erzeugen"))
                            .small()
                            .color(theme::TEXT_DIM()),
                    );
                }
                let n = st.hits.iter().filter(|h| h.on).count();
                crate::dialog::footer(ui, |ui| {
                    if ui.add_enabled(n > 0, theme::primary(format!("{}  Als Shorts anlegen", icon::DEVICE_MOBILE))).clicked() {
                        make = true;
                    }
                    if ui.add_enabled(n > 0, theme::secondary(format!("{}  Als Marker setzen", icon::FLAG))).clicked() {
                        markers = true;
                    }
                    if ui.add(theme::secondary("Abbrechen")).clicked() {
                        open = false;
                    }
                });
                if n > 0 && enter {
                    make = true;
                }
            });
        let chosen: Vec<f64> = st.hits.iter().filter(|h| h.on).map(|h| h.at).collect();
        if markers {
            let mut ms = self.project.timeline.markers.clone();
            let first = ms.iter().map(|m| m.id).max().unwrap_or(0) + 1;
            for (i, t) in chosen.iter().enumerate() {
                ms.push(Marker {
                    id: first + i as u64,
                    at: Time::from_secs_f64(*t),
                    duration: Time::ZERO,
                    name: format!("Highlight {}", i + 1),
                    note: String::new(),
                    color: 4,
                });
            }
            self.apply(EditOp::SetMarkers { markers: ms });
            self.toast(crate::theme::count_text(chosen.len(), "{} Highlight als Marker gesetzt", "{} Highlights als Marker gesetzt"));
            open = false;
        }
        if make {
            self.create_shorts(&chosen, st.len as f64, st.captions && has_caps);
            open = false;
        }
        if open {
            self.shorts = Some(st);
        }
    }

    fn create_shorts(&mut self, hits: &[f64], len: f64, captions: bool) {
        self.commit_live();
        let total = self.project.duration().as_secs_f64();
        let tracks = self.caption_tracks();
        let caps = captions.then(|| (tracks.as_slice(), self.caption_preset()));
        let dir = paths::projects_dir();
        let fonts = self.font_files();
        let measure = text_measure(&fonts);
        let mut n = 0;
        for (i, at) in hits.iter().enumerate() {
            let (a, b) = short_range(*at, len, total);
            let name = format!("{} – Short {}", self.project.name, i + 1);
            let proj = make_short(&self.project, Time::from_secs_f64(a), Time::from_secs_f64(b), &name, caps, &measure);
            let base = paths::safe_name(&name);
            let mut p = dir.join(format!("{}.cutline", base.trim()));
            let mut k = 2;
            while p.exists() {
                p = dir.join(format!("{} ({k}).cutline", base.trim()));
                k += 1;
            }
            let r = std::fs::create_dir_all(&dir).and_then(|_| paths::write_atomic(&p, proj.to_json()));
            match r {
                Ok(()) => {
                    self.settings.add_recent(&p);
                    n += 1;
                }
                Err(e) => self.toast_err(e.to_string()),
            }
        }
        if n > 0 {
            self.settings.save();
            self.home.refresh();
            self.toast(format!("{n} Shorts angelegt in {}", dir.display()));
        }
    }

    /// After the aspect ratio changes: refit clips? (one undo step)
    pub fn aspect_fit_dialog(&mut self, ctx: &egui::Context) {
        let Some(s) = self.aspect_fit.clone() else {
            return;
        };
        let mut pick: Option<u8> = None;
        let (mut esc, enter) = crate::dialog::keys(ctx);
        crate::dialog::modal(ctx, "aspect-fit").show(ctx, |ui| {
            ui.set_width(360.0);
            esc |= crate::dialog::title(ui, "Clips neu einpassen?");
            ui.label(RichText::new(crate::tr("Das Seitenverhältnis ändert sich. Wie sollen vorhandene Bilder ins neue Format?")).color(theme::TEXT_DIM()));
            crate::dialog::footer(ui, |ui| {
                if ui.add(theme::primary("Füllen")).on_hover_text(crate::tr("Bild füllt das Format, Ränder werden beschnitten")).clicked() {
                    pick = Some(1);
                }
                if ui.add(theme::secondary("Einpassen")).on_hover_text(crate::tr("Ganzes Bild sichtbar, mit Balken")).clicked() {
                    pick = Some(2);
                }
                if ui.add(theme::secondary("Auto-Reframe")).on_hover_text(crate::tr("Füllen und dem Wichtigen im Bild folgen (Analyse im Hintergrund)")).clicked() {
                    pick = Some(3);
                }
                if ui.add(theme::secondary("So lassen")).clicked() {
                    pick = Some(0);
                }
            });
            if enter {
                pick = Some(1);
            }
        });
        if esc {
            self.aspect_fit = None;
            return;
        }
        let Some(mode) = pick else { return };
        self.aspect_fit = None;
        let (w, h) = (s.width as f32, s.height as f32);
        let clips: Vec<(Clip, MediaRef)> = self
            .project
            .timeline
            .tracks
            .iter()
            .filter(|t| !t.locked)
            .flat_map(|t| t.clips.iter())
            .filter_map(|c| visual(&self.project, c).map(|m| (c.clone(), m.clone())))
            .collect();
        let mut ops = vec![EditOp::SetSettings { settings: s }];
        if mode > 0 {
            for (c, m) in &clips {
                let mut c = c.clone();
                fit_clip(&mut c, m, w, h, mode != 2);
                ops.push(EditOp::ReplaceClip { clip: c });
            }
        }
        self.apply(EditOp::Batch { label: "Seitenverhältnis".into(), ops });
        if mode == 3 {
            // one job for all clips, one after another; the result as one undo step
            let jobs: Vec<(ClipId, MediaRef, f64, f64)> = clips
                .into_iter()
                .filter(|(c, m)| m.kind == MediaKind::Video && !is_shrunk_overlay(c))
                .map(|(c, m)| (c.id, m, c.source_time(c.start).as_secs_f64(), c.source_span().as_secs_f64()))
                .collect();
            if jobs.is_empty() {
                return;
            }
            self.tasks.spawn(ctx, "reframe:fit", "Auto-Reframe analysiert …", move |p, cancel| {
                let n = jobs.len() as f32;
                let (mut done, mut err) = (Vec::new(), None);
                for (i, (id, m, start, span)) in jobs.into_iter().enumerate() {
                    let sub = |v: f32| p((i as f32 + v) / n);
                    let r = cl_analyze::reframe::focus_path(&m.path, m.width, m.height, start, span, &sub, cancel);
                    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        return TaskDone::Failed("Abgebrochen".into());
                    }
                    match r {
                        Ok(points) => done.push((id, points)),
                        Err(e) => err = Some(e),
                    }
                }
                match err {
                    Some(e) if done.is_empty() => TaskDone::Failed(e),
                    _ => TaskDone::Run(Box::new(move |app: &mut App| {
                        app.apply_reframes(&done);
                        if let Some(e) = err {
                            app.toast_err(e);
                        }
                    })),
                }
            });
        }
    }

    /// Does the project have image clips that can be refitted on a format change?
    pub fn has_visual_clips(&self) -> bool {
        self.project.timeline.tracks.iter().flat_map(|t| t.clips.iter()).any(|c| visual(&self.project, c).is_some())
    }
}

fn fmt_t(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_is_trimmed_shifted_and_vertical() {
        let mut p = Project::new("Lang");
        p.settings.width = 1920;
        p.settings.height = 1080;
        let m = MediaRef {
            id: MediaId::new(),
            path: "a.mp4".into(),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(60.0),
            width: 1920,
            height: 1080,
            fps: None,
            has_audio: true,
            audio_streams: 1,
            video_codec: None,
            audio_codec: None,
        };
        let mid = m.id;
        p.media.push(m);
        let mut t = Track::new(TrackKind::Video);
        let mut c = Clip::new(ClipContent::Media { media: mid, kind: MediaKind::Video }, Time::from_secs_f64(5.0), Time::from_secs_f64(20.0));
        c.source_in = Time::from_secs_f64(1.0);
        t.clips.push(c);
        // lies fully outside
        t.clips.push(Clip::new(ClipContent::Media { media: mid, kind: MediaKind::Video }, Time::from_secs_f64(40.0), Time::from_secs_f64(5.0)));
        p.timeline.tracks.push(t);
        let s = make_short(&p, Time::from_secs_f64(10.0), Time::from_secs_f64(20.0), "Lang – Short 1", None, &rough);
        assert_eq!((s.settings.width, s.settings.height), (1080, 1920));
        assert_eq!(s.settings.fps, p.settings.fps);
        let cs = &s.timeline.tracks[0].clips;
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].start, Time::ZERO);
        assert!((cs[0].duration.as_secs_f64() - 10.0).abs() < 1e-3);
        assert!((cs[0].source_in.as_secs_f64() - 6.0).abs() < 1e-3);
        // 16:9 fills 9:16: (1920/1080)² ≈ 3.16
        assert!((cs[0].transform.scale - 3.16).abs() < 0.01, "{}", cs[0].transform.scale);
        assert_eq!(short_range(5.0, 30.0, 100.0), (0.0, 30.0));
        assert_eq!(short_range(95.0, 30.0, 100.0), (70.0, 100.0));
        let env: Vec<f32> = (0..1000).map(|i| if (495..505).contains(&i) { 1.0 } else { 0.1 }).collect();
        assert!(strength(&env, 5.0) > strength(&env, 2.0));
    }

    fn long_project() -> (Project, MediaId) {
        let mut p = Project::new("Lang");
        p.settings.width = 1920;
        p.settings.height = 1080;
        let m = MediaRef {
            id: MediaId::new(),
            path: "a.mp4".into(),
            kind: MediaKind::Video,
            duration: Time::from_secs_f64(60.0),
            width: 1920,
            height: 1080,
            fps: None,
            has_audio: true,
            audio_streams: 1,
            video_codec: None,
            audio_codec: None,
        };
        let id = m.id;
        p.media.push(m);
        (p, id)
    }

    #[test]
    fn animated_pip_keeps_its_keyframes() {
        let (p, mid) = long_project();
        let mut c = Clip::new(ClipContent::Media { media: mid, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(4.0));
        c.keyframes.set(Prop::Scale, Time::ZERO, 0.4);
        c.keyframes.set(Prop::Scale, Time::from_secs_f64(2.0), 0.6);
        assert!(is_shrunk_overlay(&c));
        let before = c.clone();
        fit_clip(&mut c, &p.media[0], 1080.0, 1920.0, true);
        assert_eq!(c, before);
    }

    #[test]
    fn trimmed_speed_curve_keeps_the_same_source_frames() {
        let (mut p, mid) = long_project();
        let mut t = Track::new(TrackKind::Video);
        let mut c = Clip::new(ClipContent::Media { media: mid, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(20.0));
        c.fx.speed_curve = Some("montage".into());
        let (s1, s2) = (c.source_time(Time::from_secs_f64(5.0)), c.source_time(Time::from_secs_f64(15.0)));
        t.clips.push(c);
        p.timeline.tracks.push(t);
        let s = make_short(&p, Time::from_secs_f64(5.0), Time::from_secs_f64(15.0), "S", None, &rough);
        let c = &s.timeline.tracks[0].clips[0];
        assert!(c.fx.speed_curve.is_none());
        assert_eq!(c.source_in, s1);
        assert!((c.source_time(c.end()).as_secs_f64() - s2.as_secs_f64()).abs() < 1e-3);
    }

    #[test]
    fn short_text_stays_inside_the_narrow_frame() {
        let (mut p, _) = long_project();
        let mut t = Track::new(TrackKind::Text);
        let mut c = Clip::new(ClipContent::Text(TextStyle { size: 96.0, ..TextStyle::default() }), Time::ZERO, Time::from_secs_f64(5.0));
        c.transform.x = 700.0;
        c.keyframes.set(Prop::X, Time::ZERO, -900.0);
        t.clips.push(c);
        p.timeline.tracks.push(t);
        let s = make_short(&p, Time::ZERO, Time::from_secs_f64(5.0), "S", None, &rough);
        let c = &s.timeline.tracks[0].clips[0];
        assert!(c.transform.x.abs() <= 540.0, "{}", c.transform.x);
        assert!(c.keyframes.0[&Prop::X].iter().all(|k| k.v.abs() <= 540.0));
        let ClipContent::Text(st) = &c.content else { panic!() };
        // same size relative to the shorter edge (1080 → 1080)
        assert!((st.size - 96.0).abs() < 0.01, "{}", st.size);
    }

    #[test]
    fn slightly_cropped_main_clip_still_fills_the_frame() {
        let (mut p, mid) = long_project();
        let mut c = Clip::new(ClipContent::Media { media: mid, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(5.0));
        // borders gone, zoomed in again: still covers the frame
        c.crop = Crop { left: 0.05, right: 0.05, ..Default::default() };
        c.transform.scale = 1.12;
        let id = c.id;
        let mut t = Track::new(TrackKind::Video);
        t.clips.push(c);
        p.timeline.tracks.push(t);
        let o = reframe(&p, (1080, 1920), true, &FocusPaths::new(), &rough);
        let r = shown(&o, clip(&o, id), Time::ZERO);
        assert!(covers(&o, r), "fill: bars {r:?}");
        // Fit: blurred background below
        let o = reframe(&p, (1080, 1920), false, &FocusPaths::new(), &rough);
        assert_eq!(o.timeline.tracks.len(), 2);
        assert_eq!(o.timeline.tracks[0].clips[0].id, id.derived(BACK_SALT));
        // shorts fill it too
        let s = make_short(&p, Time::ZERO, Time::from_secs_f64(5.0), "S", None, &rough);
        assert!(covers(&s, shown(&s, clip(&s, id), Time::ZERO)));
    }

    /// Rough text measurement: 0.6 × size per character, wrapping at `wrap`
    fn rough(st: &TextStyle, wrap: f32) -> (f32, f32) {
        let (mut w, mut lines) = (0.0f32, 0.0f32);
        for l in st.text.split('\n') {
            let lw = l.chars().count() as f32 * st.size * 0.6;
            let n = if wrap.is_finite() { (lw / wrap).ceil().max(1.0) } else { 1.0 };
            (w, lines) = (w.max(lw.min(wrap)), lines + n);
        }
        (w, st.size * 1.3 * lines)
    }

    fn media(w: u32, h: u32, kind: MediaKind) -> MediaRef {
        MediaRef {
            id: MediaId::new(),
            path: "m".into(),
            kind,
            duration: Time::from_secs_f64(60.0),
            width: w,
            height: h,
            fps: None,
            has_audio: kind == MediaKind::Video,
            audio_streams: 1,
            video_codec: None,
            audio_codec: None,
        }
    }

    /// Content with its own rule on rebuild
    struct Sample {
        p: Project,
        base: ClipId,
        kb: ClipId,
        pip: ClipId,
        side: [ClipId; 2],
        logo: ClipId,
        crop: ClipId,
        texts: Vec<ClipId>,
        fx: ClipId,
    }

    /// Project `w`×`h`: frame-filling video with censoring and a transition, then one with keyframes
    /// (zoom and pan); picture-in-picture bottom right with motion, split screen side by side,
    /// logo top right, scaled-down clip with a crop, title without wrapping, text bottom center,
    /// captions, filter with a mask over the left half
    fn sample(w: u32, h: u32) -> Sample {
        let (wf, hf) = (w as f32, h as f32);
        let mut p = Project::new("Probe");
        (p.settings.width, p.settings.height) = (w, h);
        let (v, logo) = (media(w, h, MediaKind::Video), media(500, 500, MediaKind::Image));
        let secs = Time::from_secs_f64;
        let vid = |a: f64, b: f64| Clip::new(ClipContent::Media { media: v.id, kind: MediaKind::Video }, secs(a), secs(b - a));
        let track = |kind, clips: Vec<Clip>| Track { clips, ..Track::new(kind) };
        let mut base = vid(0.0, 5.0);
        base.fx.censor = vec![Censor { path: vec![CensorKey { t: Time::ZERO, cx: 0.3, cy: 0.4, w: 0.2, h: 0.3 }], ..Default::default() }];
        base.transition_out = Some(TransitionRef { id: "fade".into(), duration: secs(0.5) });
        let mut kb = vid(5.0, 10.0);
        kb.keyframes.set(Prop::Scale, Time::ZERO, 1.0);
        kb.keyframes.set(Prop::Scale, secs(5.0), 1.2);
        kb.keyframes.set(Prop::X, Time::ZERO, 0.0);
        kb.keyframes.set(Prop::X, secs(5.0), 0.05 * wf);
        let mut pip = vid(0.0, 5.0);
        pip.transform = crate::arrange::in_corner(wf, hf, (wf, hf), 0.28, 3);
        pip.keyframes.set(Prop::X, Time::ZERO, pip.transform.x);
        pip.keyframes.set(Prop::X, secs(5.0), pip.transform.x - 0.05 * wf);
        let cells = crate::arrange::arrange(wf, hf, &[(wf, hf), (wf, hf)], crate::arrange::Arrange::Side);
        let side: Vec<Clip> = cells.iter().map(|(t, cr)| Clip { transform: *t, crop: *cr, ..vid(10.0, 15.0) }).collect();
        let mut lg = Clip::new(ClipContent::Media { media: logo.id, kind: MediaKind::Image }, Time::ZERO, secs(15.0));
        lg.transform = crate::arrange::in_corner(wf, hf, (500.0, 500.0), 0.12, 1);
        let mut crop = vid(0.0, 5.0);
        crop.transform = Transform { x: -wf / 4.0, y: 0.0, scale: 0.5, rotation_deg: 0.0 };
        crop.crop = Crop { left: 0.2, ..Default::default() };
        let text = |s: &str, size: f32, wrap: bool, y: f32| {
            let mut c = Clip::new(ClipContent::Text(TextStyle { text: s.into(), size, auto_wrap: wrap, ..Default::default() }), Time::ZERO, secs(5.0));
            c.transform.y = y * hf;
            c
        };
        let texts = vec![
            text("Ein sehr langer Titel", 96.0, false, -0.3),
            text("Unten mittig", 64.0, true, 0.38),
            text("Das ist ein ziemlich langer Untertitel, der umbrechen muss", 64.0, true, 0.4),
        ];
        let mut fx = Clip::new(ClipContent::Filter(FxRef::new("noir")), Time::ZERO, secs(15.0));
        fx.fx.mask = Some(Mask { cx: 0.25, cy: 0.5, w: 0.5, h: 1.0, ..Mask::new(MaskShape::Rect) });
        let s = Sample {
            base: base.id,
            kb: kb.id,
            pip: pip.id,
            side: [side[0].id, side[1].id],
            logo: lg.id,
            crop: crop.id,
            texts: texts.iter().map(|c| c.id).collect(),
            fx: fx.id,
            p: Project::new(""),
        };
        let mut main = track(TrackKind::Video, vec![base, kb]);
        main.main = true;
        p.timeline.tracks = vec![
            main,
            track(TrackKind::Video, vec![pip, side[0].clone()]),
            track(TrackKind::Video, vec![side[1].clone()]),
            track(TrackKind::Sticker, vec![lg]),
            track(TrackKind::Video, vec![crop]),
            track(TrackKind::Text, texts),
            track(TrackKind::Filter, vec![fx]),
        ];
        p.media = vec![v, logo];
        Sample { p, ..s }
    }

    fn clip(p: &Project, id: ClipId) -> &Clip {
        p.find_clip(id).expect("Clip da").1
    }

    /// Visible rect of an image clip at clip time `t` (x0, y0, x1, y1 from the frame center)
    fn shown(p: &Project, c: &Clip, t: Time) -> [f32; 4] {
        let m = visual(p, c).unwrap();
        let (w, h) = (p.settings.width as f32, p.settings.height as f32);
        let e = c.eval_at(t, w, h);
        let d = e.scale * (w / m.width as f32).min(h / m.height as f32);
        let (mw, mh) = (m.width as f32 * d, m.height as f32 * d);
        let cr = c.crop;
        [e.x - mw / 2.0 + cr.left * mw, e.y - mh / 2.0 + cr.top * mh, e.x + mw / 2.0 - cr.right * mw, e.y + mh / 2.0 - cr.bottom * mh]
    }

    fn text_rect(p: &Project, c: &Clip) -> [f32; 4] {
        let ClipContent::Text(st) = &c.content else { panic!("no text") };
        let w = p.settings.width as f32;
        let (tw, th) = rough(st, if st.auto_wrap { w * 0.9 / c.transform.scale } else { f32::INFINITY });
        let (x, y) = (c.transform.x, c.transform.y);
        [x - tw / 2.0, y - th / 2.0, x + tw / 2.0, y + th / 2.0]
    }

    fn inside(p: &Project, r: [f32; 4]) -> bool {
        let (w, h) = (p.settings.width as f32 / 2.0 + 1.0, p.settings.height as f32 / 2.0 + 1.0);
        r[0] >= -w && r[1] >= -h && r[2] <= w && r[3] <= h
    }

    fn covers(p: &Project, r: [f32; 4]) -> bool {
        let (w, h) = (p.settings.width as f32 / 2.0 - 1.0, p.settings.height as f32 / 2.0 - 1.0);
        r[0] <= -w && r[1] <= -h && r[2] >= w && r[3] >= h
    }

    fn center(r: [f32; 4]) -> (f32, f32) {
        ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
    }

    #[test]
    fn reframe_keeps_every_kind_of_content_in_the_new_frame() {
        let cases = [((1920, 1080), (9, 16)), ((1080, 1920), (16, 9)), ((1920, 1080), (1, 1)), ((1920, 1080), (4, 5))];
        for ((w, h), (a, b)) in cases {
            let s = sample(w, h);
            let mut ps = s.p.settings.clone();
            ps.apply_aspect(AspectPreset::Ratio(a, b));
            let (w2, h2) = (ps.width, ps.height);
            for fill in [true, false] {
                let o = reframe(&s.p, (w2, h2), fill, &FocusPaths::new(), &rough);
                let case = format!("{w}×{h} → {w2}×{h2} {}", if fill { "Füllen" } else { "Einpassen" });
                assert_eq!((o.settings.width, o.settings.height, o.settings.aspect), (w2, h2, AspectPreset::Ratio(a, b)), "{case}");
                // frame-filling: covers the area or lies fully within it
                let r = shown(&o, clip(&o, s.base), Time::ZERO);
                assert!(if fill { covers(&o, r) } else { inside(&o, r) }, "{case}: main clip {r:?}");
                // Zoom and pan: start like the main clip, keyframes in the same ratio
                let kb = clip(&o, s.kb);
                let r = shown(&o, kb, Time::ZERO);
                assert!(if fill { covers(&o, r) } else { inside(&o, r) }, "{case}: keyframes at the start {r:?}");
                let sk = &kb.keyframes.0[&Prop::Scale];
                assert!((sk[1].v / sk[0].v - 1.2).abs() < 1e-3, "{case}: zoom {sk:?}");
                assert!(covers(&o, shown(&o, kb, Time::from_secs_f64(5.0))) || !fill, "{case}: keyframes at the end");
                // censoring and crop in source coordinates: unchanged
                assert_eq!(clip(&o, s.base).fx.censor, clip(&s.p, s.base).fx.censor, "{case}");
                assert_eq!(clip(&o, s.crop).crop, clip(&s.p, s.crop).crop, "{case}");
                // Overlays: fully in the frame, in the same corner, size relative to the shorter edge
                let ks = w2.min(h2) as f32 / w.min(h) as f32;
                // Cropped clips (layout cells): the whole layout fits into the new frame
                let kl = ks.min(w2 as f32 / w as f32).min(h2 as f32 / h as f32);
                for (id, name, ks) in [(s.pip, "Bild-in-Bild", ks), (s.logo, "Logo", ks), (s.crop, "Zuschnitt", kl)] {
                    for t in [0.0, 5.0] {
                        let t = Time::from_secs_f64(t);
                        let (old, new) = (shown(&s.p, clip(&s.p, id), t), shown(&o, clip(&o, id), t));
                        assert!(inside(&o, new), "{case}: {name} {new:?}");
                        let (c0, c1) = (center(old), center(new));
                        let same = |a: f32, b: f32| a.abs() < 1.0 || a.signum() == b.signum();
                        assert!(same(c0.0, c1.0) && same(c0.1, c1.1), "{case}: {name} corner {c0:?} → {c1:?}");
                        assert!(((new[2] - new[0]) / (old[2] - old[0]) - ks).abs() < 1e-3, "{case}: {name} size");
                    }
                }
                // Split screen: stays side by side (left stays left), size relative to the shorter edge
                let (l, r) = (shown(&o, clip(&o, s.side[0]), Time::ZERO), shown(&o, clip(&o, s.side[1]), Time::ZERO));
                assert!(center(l).0 < center(r).0, "{case}: split screen {l:?} {r:?}");
                assert!(l[2] <= r[0] + 1.0 && inside(&o, l) && inside(&o, r), "{case}: split screen overlaps {l:?} {r:?}");
                // Text: does not stick out over the edge, top stays top, bottom stays bottom
                for id in &s.texts {
                    let (c0, c1) = (clip(&s.p, *id), clip(&o, *id));
                    let r = text_rect(&o, c1);
                    assert!(inside(&o, r), "{case}: text {r:?} {:?}", c1.content);
                    assert_eq!(c0.transform.y.signum(), c1.transform.y.signum(), "{case}");
                }
                // the filter mask follows the frame-filling content: the left edge moves with it
                let (m0, m1) = (clip(&s.p, s.fx).fx.mask.unwrap(), clip(&o, s.fx).fx.mask.unwrap());
                let k = if fill { (w2 as f32 / w as f32).max(h2 as f32 / h as f32) } else { (w2 as f32 / w as f32).min(h2 as f32 / h as f32) };
                let edge = |m: &Mask, w: u32| (m.cx - m.w / 2.0 - 0.5) * w as f32;
                assert!((edge(&m1, w2) - k * edge(&m0, w)).abs() < 0.5, "{case}: mask {m0:?} → {m1:?}");
                // Fit: muted, blurred, filling copy directly below the main track
                if fill {
                    assert_eq!(o.timeline.tracks.len(), s.p.timeline.tracks.len(), "{case}");
                } else {
                    assert_eq!(o.timeline.tracks.len(), s.p.timeline.tracks.len() + 1, "{case}");
                    let bt = &o.timeline.tracks[0];
                    assert!(bt.muted && !bt.main && o.timeline.tracks[1].main, "{case}");
                    assert_eq!(bt.clips.len(), 2);
                    for (b, id) in bt.clips.iter().zip([s.base, s.kb]) {
                        let c = clip(&s.p, id);
                        assert_eq!(b.id, id.derived(BACK_SALT));
                        assert!(b.fx.effects.iter().any(|e| e.id == "blur"), "{case}");
                        assert_eq!((b.start, b.duration, b.speed, &b.fx.censor, b.crop), (c.start, c.duration, c.speed, &c.fx.censor, c.crop));
                        assert!(covers(&o, shown(&o, b, Time::ZERO)), "{case}: background");
                    }
                    assert_eq!(bt.clips[0].transition_out, clip(&s.p, s.base).transition_out, "{case}: transition as above");
                }
            }
        }
    }

    /// 16:9 project with a frame-filling video (4 s) and one with its own pan after it
    fn follow_project() -> (Project, ClipId, ClipId) {
        let (mut p, mid) = long_project();
        let a = Clip::new(ClipContent::Media { media: mid, kind: MediaKind::Video }, Time::ZERO, Time::from_secs_f64(4.0));
        let mut b = Clip::new(ClipContent::Media { media: mid, kind: MediaKind::Video }, Time::from_secs_f64(4.0), Time::from_secs_f64(4.0));
        b.keyframes.set(Prop::X, Time::ZERO, 0.0);
        b.keyframes.set(Prop::X, Time::from_secs_f64(4.0), 50.0);
        let (ia, ib) = (a.id, b.id);
        let mut t = Track::new(TrackKind::Video);
        t.clips = vec![a, b];
        p.timeline.tracks.push(t);
        (p, ia, ib)
    }

    /// The person walks from left (0.2) to right (0.8): 16:9 → 9:16 follows them, the image always covers
    #[test]
    fn follow_person_keeps_the_moving_focus_in_the_frame() {
        let (p, a, _) = follow_project();
        let path: Vec<(f64, f32, f32)> = (0..=8).map(|i| (i as f64 * 0.5, 0.2 + 0.075 * i as f32, 0.5)).collect();
        let o = reframe(&p, (1080, 1920), true, &FocusPaths::from([(a, path.clone())]), &rough);
        let c = clip(&o, a);
        for (t, fx, _) in &path {
            let r = shown(&o, c, Time::from_secs_f64(*t));
            assert!(covers(&o, r), "{t}: bars {r:?}");
            // point of the person in the new frame: centered
            let x = r[0] + fx * (r[2] - r[0]);
            assert!(x.abs() < 20.0, "{t}: person at {x}");
        }
        // start: left third of the source frame in view, end: the right one
        let (r0, r1) = (shown(&o, c, Time::ZERO), shown(&o, c, Time::from_secs_f64(4.0)));
        let third = |r: [f32; 4], f: f32| r[0] + f * (r[2] - r[0]);
        assert!(third(r0, 1.0 / 3.0) > 0.0 && third(r1, 2.0 / 3.0) < 0.0, "{r0:?} {r1:?}");
    }

    /// Without a path like „Füllen“; own keyframes stay as „Füllen“ sets them
    #[test]
    fn follow_person_without_path_or_on_own_keyframes_is_plain_fill() {
        let (p, a, b) = follow_project();
        let plain = reframe(&p, (1080, 1920), true, &FocusPaths::new(), &rough);
        let path = vec![(0.0, 0.1, 0.5), (4.0, 0.9, 0.5)];
        let o = reframe(&p, (1080, 1920), true, &FocusPaths::from([(a, vec![]), (b, path)]), &rough);
        assert_eq!(o, plain);
        assert_eq!(follow_jobs(&p).iter().map(|j| j.0 .0).collect::<Vec<_>>(), [a], "own pan is not analyzed");
    }

    /// Trimmed clip: new key, the old path no longer fits
    #[test]
    fn focus_key_changes_when_the_clip_is_trimmed() {
        let (mut p, a, _) = follow_project();
        let k0 = follow_jobs(&p)[0].0.clone();
        let c = &mut p.timeline.tracks[0].clips[0];
        c.source_in = Time::from_secs_f64(1.0);
        let k1 = follow_jobs(&p)[0].0.clone();
        p.timeline.tracks[0].clips[0].duration = Time::from_secs_f64(3.0);
        let k2 = follow_jobs(&p)[0].0.clone();
        assert!(k0 != k1 && k1 != k2 && k0.0 == a && k2.0 == a, "{k0:?} {k1:?} {k2:?}");
    }
}
