//! Generates the package `plugins/cutline-elemente` (own animated elements, CC0-1.0).
//!
//! Every element is a function "time → SVG"; the app's SVG renderer renders the frames
//! (resvg with the font Anton), FFmpeg builds an APNG from them (extension `.png`). Endless loops
//! are stored with `-plays 0`, one-off animations with `-plays 1` – that is how the app tells
//! whether to repeat or to stop on the last frame. Deterministic (no randomness).
//!
//! Usage: `cargo run --release -p cl-fx --example gen_elemente [-- <id-part>]`

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::process::Stdio;

const FPS: u32 = 30;
const INK: &str = "#16161A";
const WHITE: &str = "#FFFFFF";
const RED: &str = "#FF2E43";
const YELLOW: &str = "#FFD21F";
const BLUE: &str = "#2F7BFF";
const PINK: &str = "#FF5FA2";
const GREY: &str = "#3A3A44";
const CONFETTI: [&str; 6] = ["#FF2E43", "#FFD21F", "#2F7BFF", "#2BD46A", "#B15CFF", "#FF8A1F"];

/// Color variant: main color, color on top (text/symbol)
#[derive(Clone, Copy)]
struct Pal {
    main: &'static str,
    on: &'static str,
}

struct El {
    id: &'static str,
    de: &'static str,
    en: &'static str,
    cat: &'static str,
    /// Image size in pixels; the drawing uses a viewBox 100 units high
    w: u32,
    h: u32,
    secs: f32,
    looping: bool,
    /// (id, German suffix, English suffix, colors)
    variants: Vec<(&'static str, &'static str, &'static str, Pal)>,
    draw: fn(f32, Pal) -> String,
}

// ------------------------------------------------------------------ Motion

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
/// Fraction of `t` within the section `a..b` (0…1)
fn seg(t: f32, a: f32, b: f32) -> f32 {
    clamp01((t - a) / (b - a))
}
/// Overshoot at the end
fn back(x: f32) -> f32 {
    let (x, s) = (clamp01(x) - 1.0, 2.2);
    1.0 + (s + 1.0) * x * x * x + s * x * x
}
/// Soft ease-out
fn out3(x: f32) -> f32 {
    1.0 - (1.0 - clamp01(x)).powi(3)
}
fn inout(x: f32) -> f32 {
    let x = clamp01(x);
    if x < 0.5 { 4.0 * x * x * x } else { 1.0 - (-2.0 * x + 2.0).powi(3) / 2.0 }
}
/// Damped spring: 1 with oscillation
fn spring(x: f32) -> f32 {
    let x = x.max(0.0);
    1.0 - (-6.0 * x).exp() * (x * 14.0).cos()
}
fn lerp(a: f32, b: f32, x: f32) -> f32 {
    a + (b - a) * x
}
fn tau() -> f32 {
    std::f32::consts::TAU
}

// ------------------------------------------------------------------ Drawing

/// Scale and rotate around (cx, cy)
fn tf(cx: f32, cy: f32, s: f32, rot: f32) -> String {
    format!(r##"transform="translate({cx:.2} {cy:.2}) rotate({rot:.2}) scale({:.4}) translate({:.2} {:.2})""##, s.max(0.0001), -cx, -cy)
}

type Pts = Vec<(f32, f32)>;

fn bezier(p: [(f32, f32); 4], n: usize) -> Pts {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            (a * p[0].0 + b * p[1].0 + c * p[2].0 + d * p[3].0, a * p[0].1 + b * p[1].1 + c * p[2].1 + d * p[3].1)
        })
        .collect()
}

/// Start of a polyline up to fraction `p` of the length (pen effect)
fn prefix(pts: &[(f32, f32)], p: f32) -> Pts {
    let lens: Vec<f32> = pts.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).collect();
    let mut left = lens.iter().sum::<f32>() * clamp01(p);
    let mut out = vec![pts[0]];
    for (w, l) in pts.windows(2).zip(lens) {
        if left >= l {
            out.push(w[1]);
            left -= l;
        } else {
            let k = if l > 0.0 { left / l } else { 0.0 };
            out.push((lerp(w[0].0, w[1].0, k), lerp(w[0].1, w[1].1, k)));
            break;
        }
    }
    out
}

fn d(pts: &[(f32, f32)]) -> String {
    let mut s = String::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        let _ = write!(s, "{}{x:.2} {y:.2} ", if i == 0 { "M" } else { "L" });
    }
    s
}

/// Line with a dark outline (readable on any video)
fn stroke(pts: &[(f32, f32)], color: &str, w: f32) -> String {
    if pts.len() < 2 || pts.windows(2).all(|p| p[0] == p[1]) {
        return String::new();
    }
    let d = d(pts);
    format!(
        r##"<path d="{d}" fill="none" stroke="{INK}" stroke-width="{:.2}" stroke-linecap="round" stroke-linejoin="round"/><path d="{d}" fill="none" stroke="{color}" stroke-width="{w:.2}" stroke-linecap="round" stroke-linejoin="round"/>"##,
        w + 3.0
    )
}

/// Several lines as a unit: first all outlines, then all colors
fn strokes(lines: &[(Pts, &str, f32)]) -> String {
    let mut out = String::new();
    let mut top = String::new();
    for (pts, color, w) in lines {
        let s = stroke(pts, color, *w);
        if let Some(i) = s.find("/><path") {
            out.push_str(&s[..i + 2]);
            top.push_str(&s[i + 2..]);
        }
    }
    out + &top
}

/// Text in Anton with a dark outline
fn text(x: f32, y: f32, size: f32, fill: &str, outline: f32, s: &str) -> String {
    format!(
        r##"<text x="{x:.2}" y="{y:.2}" font-family="Anton" font-size="{size:.2}" text-anchor="middle" fill="{fill}" stroke="{INK}" stroke-width="{outline:.2}" stroke-linejoin="round" paint-order="stroke">{s}</text>"##
    )
}

fn star_points(cx: f32, cy: f32, n: usize, r_out: f32, r_in: f32, rot: f32) -> String {
    let mut s = String::new();
    for i in 0..n * 2 {
        let r = if i % 2 == 0 { r_out } else { r_in };
        let a = (rot - 90.0).to_radians() + i as f32 * std::f32::consts::PI / n as f32;
        let _ = write!(s, "{:.2},{:.2} ", cx + r * a.cos(), cy + r * a.sin());
    }
    s
}

/// Four-pointed sparkle star
fn sparkle(cx: f32, cy: f32, r: f32, fill: &str) -> String {
    let k = r * 0.22;
    format!(
        r##"<path d="M{cx:.2} {:.2} Q{:.2} {:.2} {:.2} {cy:.2} Q{:.2} {:.2} {cx:.2} {:.2} Q{:.2} {:.2} {:.2} {cy:.2} Q{:.2} {:.2} {cx:.2} {:.2}Z" fill="{fill}" stroke="{INK}" stroke-width="1.2" stroke-linejoin="round"/>"##,
        cy - r,
        cx + k, cy - k, cx + r,
        cx + k, cy + k, cy + r,
        cx - k, cy + k, cx - r,
        cx - k, cy - k, cy - r
    )
}

const HEART: &str = "M50 88 C20 66 6 50 6 32 C6 18 17 8 30 8 C39 8 46 13 50 20 C54 13 61 8 70 8 C83 8 94 18 94 32 C94 50 80 66 50 88Z";

fn heart(cx: f32, cy: f32, s: f32, rot: f32, fill: &str, opacity: f32) -> String {
    // the heart path has its center at (50, 48)
    format!(
        r##"<g transform="translate({cx:.2} {cy:.2}) rotate({rot:.2}) scale({:.4}) translate(-50 -48)" opacity="{opacity:.3}"><path d="{HEART}" fill="{fill}" stroke="{INK}" stroke-width="{:.2}" stroke-linejoin="round"/></g>"##,
        s.max(0.0001),
        (3.0 / s.max(0.05)).min(14.0)
    )
}

// ------------------------------------------------------------------ Calls to action

/// Button springs in, mouse pointer clicks (ripple), then „abonniert“
fn sub_button(t: f32, p: Pal, label: &str, done: &str) -> String {
    let (cx, cy) = (150.0, 50.0);
    let pop = back(seg(t, 0.0, 0.45));
    let wob = (1.0 - spring(seg(t, 0.0, 0.8))) * -10.0;
    let click = 0.85;
    let press = if t < click { 1.0 } else { lerp(0.93, 1.0, spring(seg(t, click, click + 0.6))) };
    let clicked = t >= click + 0.08;
    let (fill, on, label) = if clicked { (GREY, WHITE, done) } else { (p.main, p.on, label) };
    let mut s = format!("<g {}>", tf(cx, cy, pop * press, wob));
    let _ = write!(s, r##"<defs><clipPath id="pill"><rect x="30" y="22" width="240" height="56" rx="28"/></clipPath></defs>"##);
    let _ = write!(s, r##"<rect x="30" y="22" width="240" height="56" rx="28" fill="{fill}" stroke="{INK}" stroke-width="3"/>"##);
    // click ripple in the button
    let w = seg(t, click, click + 0.55);
    if w > 0.0 && w < 1.0 {
        let _ = write!(
            s,
            r##"<circle cx="212" cy="58" r="{:.2}" fill="#FFFFFF" opacity="{:.3}" clip-path="url(#pill)"/>"##,
            out3(w) * 150.0,
            0.45 * (1.0 - w)
        );
    }
    if clicked {
        let _ = write!(s, r##"<path d="M54 51 L62 59 L76 43" fill="none" stroke="{on}" stroke-width="6" stroke-linecap="round" stroke-linejoin="round"/>"##);
    }
    let _ = write!(s, r##"<text x="{}" y="62" font-family="Anton" font-size="28" text-anchor="middle" fill="{on}">{label}</text>"##, if clicked { 166 } else { 150 });
    s.push_str("</g>");
    // the mouse pointer comes from the bottom right, clicks and leaves again
    let come = out3(seg(t, 0.4, 0.8));
    let go = inout(seg(t, 1.3, 1.6));
    if come > 0.0 && go < 1.0 {
        let (x, y) = (lerp(300.0, 212.0, come) + go * 70.0, lerp(110.0, 58.0, come) + go * 40.0);
        let k = if (click..click + 0.12).contains(&t) { 0.85 } else { 1.0 };
        let _ = write!(
            s,
            r##"<g transform="translate({x:.2} {y:.2}) scale({k})"><path d="M0 0 L0 26 L7 20 L12 31 L17 29 L12 18 L21 18Z" fill="#FFFFFF" stroke="{INK}" stroke-width="2.5" stroke-linejoin="round"/></g>"##
        );
    }
    s
}

fn bell(t: f32, p: Pal) -> String {
    // ringing 0…0.8 s, then rest until 1.4 s (loop)
    let env = (1.0 - seg(t, 0.55, 0.8)) * seg(t, 0.0, 0.08);
    let a = 24.0 * (-2.2 * t).exp() * (t * tau() / 0.32).sin() * env;
    let clap = -a * 0.6;
    let mut s = String::new();
    // sound arcs
    let wave = seg(t, 0.02, 0.7);
    if wave > 0.0 && wave < 1.0 {
        let o = (1.0 - wave) * 0.9;
        for (k, r) in [(0, 30.0), (1, 38.0)] {
            let r = r + out3(wave) * 6.0 + k as f32;
            for side in [-1.0f32, 1.0] {
                let a0 = (if side < 0.0 { 200.0f32 } else { -20.0f32 }).to_radians();
                let a1 = (if side < 0.0 { 160.0f32 } else { 20.0f32 }).to_radians();
                let pts: Pts = (0..=8).map(|i| {
                    let a = lerp(a0, a1, i as f32 / 8.0);
                    (50.0 + r * a.cos(), 50.0 + r * a.sin())
                }).collect();
                let _ = write!(s, r##"<g opacity="{o:.3}">{}</g>"##, stroke(&pts, p.main, 4.0));
            }
        }
    }
    let _ = write!(s, "<g {}>", tf(50.0, 14.0, 1.0, a));
    let _ = write!(s, "<g {}>", tf(50.0, 70.0, 1.0, clap));
    let _ = write!(s, r##"<circle cx="50" cy="78" r="7" fill="{}" stroke="{INK}" stroke-width="3"/></g>"##, p.main);
    let _ = write!(s, r##"<path d="M50 16 C34 16 29 30 29 43 V58 L21 68 H79 L71 58 V43 C71 30 66 16 50 16Z" fill="{}" stroke="{INK}" stroke-width="3.5" stroke-linejoin="round"/>"##, p.main);
    let _ = write!(s, r##"<circle cx="50" cy="14" r="4.5" fill="{}" stroke="{INK}" stroke-width="3"/></g>"##, p.main);
    s
}

fn thumbs_up(t: f32, p: Pal) -> String {
    let disc = back(seg(t, 0.0, 0.35));
    let thumb = back(seg(t, 0.12, 0.5));
    let rot = (1.0 - spring(seg(t, 0.12, 0.9))) * -35.0;
    let mut s = String::new();
    // rays
    let r = seg(t, 0.3, 0.75);
    if r > 0.0 && r < 1.0 {
        for i in 0..8 {
            let a = (i as f32 * 45.0 + 22.5).to_radians();
            let (r0, r1) = (35.0 + out3(r) * 5.0, 35.0 + out3(r) * 10.0 + 3.0 * (1.0 - r));
            let pts = vec![(50.0 + r0 * a.cos(), 50.0 + r0 * a.sin()), (50.0 + r1 * a.cos(), 50.0 + r1 * a.sin())];
            let _ = write!(s, r##"<g opacity="{:.3}">{}</g>"##, 1.0 - r * r, stroke(&pts, YELLOW, 3.5));
        }
    }
    let _ = write!(s, r##"<circle cx="50" cy="50" r="{:.2}" fill="{}" stroke="{INK}" stroke-width="3"/>"##, 31.0 * disc, p.main);
    let _ = write!(
        s,
        r##"<g {}><g transform="translate(50 51) scale(0.43) translate(-52 -51)"><path d="M10 46 H26 V90 H10Z" fill="{on}" stroke="{INK}" stroke-width="5" stroke-linejoin="round"/><path d="M30 46 L48 12 C56 10 60 16 58 26 L54 40 H82 C90 40 94 48 90 56 L82 84 C80 88 76 90 72 90 H30Z" fill="{on}" stroke="{INK}" stroke-width="5" stroke-linejoin="round"/></g></g>"##,
        tf(50.0, 70.0, thumb, rot),
        on = p.on
    );
    s
}

fn heart_beat(t: f32, p: Pal) -> String {
    let b = |c: f32, w: f32| (-((t - c) / w).powi(2)).exp();
    let s = 0.82 + 0.16 * b(0.12, 0.06) + 0.1 * b(0.36, 0.07);
    heart(50.0, 50.0, s, 0.0, p.main, 1.0)
}

fn share(t: f32, p: Pal) -> String {
    let x = back(seg(t, 0.0, 0.5));
    let rot = (1.0 - spring(seg(t, 0.0, 0.9))) * -30.0;
    let sc = lerp(0.5, 1.0, back(seg(t, 0.0, 0.4)));
    format!(
        r##"<g transform="translate({:.2} 0)"><g {}><path d="M58 18 L92 48 L58 78 V61 C36 61 21 68 9 86 C13 58 28 37 58 35Z" fill="{}" stroke="{INK}" stroke-width="3.5" stroke-linejoin="round"/></g></g>"##,
        (1.0 - x) * -45.0,
        tf(50.0, 52.0, sc * 0.84, rot),
        p.main
    )
}

fn link_below(t: f32, p: Pal, line1: &str, line2: &str) -> String {
    // viewBox 200×100: text on top, arrow bobs (loop 1 s)
    let bob = (t * tau()).sin() * 5.0;
    let mut s = text(100.0, 26.0, 21.0, WHITE, 5.0, line1);
    s.push_str(&text(100.0, 48.0, 21.0, WHITE, 5.0, line2));
    let _ = write!(
        s,
        r##"<g transform="translate(0 {bob:.2})"><path d="M92 56 H108 V72 H119 L100 92 L81 72 H92Z" fill="{}" stroke="{INK}" stroke-width="3" stroke-linejoin="round"/></g>"##,
        p.main
    );
    s
}

fn badge(t: f32, p: Pal, word: &str) -> String {
    let sc = back(seg(t, 0.0, 0.45));
    let rot = (1.0 - spring(seg(t, 0.0, 0.9))) * -120.0;
    let tx = back(seg(t, 0.25, 0.6));
    format!(
        r##"<g {}><polygon points="{}" fill="{}" stroke="{INK}" stroke-width="3" stroke-linejoin="round"/><g {}>{}</g></g>"##,
        tf(50.0, 50.0, sc, rot),
        star_points(50.0, 50.0, 14, 41.0, 34.0, 0.0),
        p.main,
        tf(50.0, 50.0, tx, -10.0),
        format!(r##"<text x="50" y="60" font-family="Anton" font-size="27" text-anchor="middle" fill="{}">{word}</text>"##, p.on)
    )
}

// ------------------------------------------------------------------ Pointing

fn drawn_arrow(t: f32, p: Pal, pts: Pts) -> String {
    let line = inout(seg(t, 0.0, 0.5));
    let head = seg(t, 0.45, 0.75);
    let n = pts.len();
    let mut lines = vec![(prefix(&pts, line), p.main, 6.0)];
    if head > 0.0 {
        let (tip, from) = (pts[n - 1], pts[n - 6]);
        let a = (tip.1 - from.1).atan2(tip.0 - from.0);
        for (k, side) in [(0.0, 1.0f32), (0.35, -1.0)] {
            let b = a + side * 2.5;
            let end = (tip.0 + 18.0 * b.cos(), tip.1 + 18.0 * b.sin());
            lines.push((prefix(&[tip, end], out3(seg(head, k, k + 0.65))), p.main, 6.0));
        }
    }
    strokes(&lines)
}

fn arrow_straight(t: f32, p: Pal) -> String {
    // slightly curved as if by hand
    drawn_arrow(t, p, bezier([(16.0, 84.0), (34.0, 66.0), (56.0, 44.0), (80.0, 22.0)], 40))
}

fn arrow_curved(t: f32, p: Pal) -> String {
    drawn_arrow(t, p, bezier([(12.0, 76.0), (26.0, 20.0), (62.0, 94.0), (82.0, 28.0)], 60))
}

fn circle_mark(t: f32, p: Pal) -> String {
    let pts: Pts = (0..=90)
        .map(|i| {
            let k = i as f32 / 90.0;
            let a = (-160.0f32).to_radians() + k * tau() * 1.12;
            let r = 1.0 + 0.04 * (3.0 * a + 1.0).sin() + 0.06 * k;
            (50.0 + 38.0 * r * a.cos(), 51.0 + 27.0 * r * a.sin())
        })
        .collect();
    stroke(&prefix(&pts, inout(seg(t, 0.0, 0.7))), p.main, 5.5)
}

fn underline(t: f32, p: Pal) -> String {
    // viewBox 300×100: stroke there, shorter one back
    let a: Pts = (0..=40).map(|i| {
        let k = i as f32 / 40.0;
        (lerp(16.0, 284.0, k), 46.0 + 5.0 * (k * 3.0).sin() - 4.0 * k)
    }).collect();
    let b: Pts = (0..=30).map(|i| {
        let k = i as f32 / 30.0;
        (lerp(270.0, 46.0, k), 58.0 + 4.0 * (k * 2.5 + 1.0).sin())
    }).collect();
    let mut s = stroke(&prefix(&a, inout(seg(t, 0.0, 0.45))), p.main, 7.0);
    s.push_str(&stroke(&prefix(&b, inout(seg(t, 0.4, 0.75))), p.main, 7.0));
    s
}

fn pop_glyph(t: f32, p: Pal, glyph: &str) -> String {
    let sc = back(seg(t, 0.0, 0.4));
    let rot = (1.0 - spring(seg(t, 0.0, 1.0))) * 25.0;
    format!(
        r##"<g {}><text x="50" y="86" font-family="Anton" font-size="84" text-anchor="middle" fill="{}" stroke="{INK}" stroke-width="5" stroke-linejoin="round" paint-order="stroke">{glyph}</text></g>"##,
        tf(50.0, 80.0, sc, rot),
        p.main
    )
}

fn focus_corners(t: f32, p: Pal) -> String {
    let k = lerp(1.3, 1.0, back(seg(t, 0.0, 0.55)));
    let o = seg(t, 0.0, 0.15);
    let mut s = format!(r##"<g opacity="{o:.3}">"##);
    for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let (cx, cy) = (50.0 + sx * 32.0 * k, 50.0 + sy * 32.0 * k);
        let pts = vec![(cx, cy - sy * 16.0), (cx, cy), (cx - sx * 16.0, cy)];
        s.push_str(&stroke(&pts, p.main, 5.0));
    }
    let dot = back(seg(t, 0.4, 0.7));
    let _ = write!(s, r##"<circle cx="50" cy="50" r="{:.2}" fill="{}" stroke="{INK}" stroke-width="2"/></g>"##, 3.5 * dot, p.main);
    s
}

fn target(t: f32, p: Pal) -> String {
    let sc = lerp(1.25, 1.0, back(seg(t, 0.0, 0.6)));
    let rot = lerp(90.0, 0.0, out3(seg(t, 0.0, 0.7)));
    let o = seg(t, 0.0, 0.15);
    let circle: Pts = (0..=48).map(|i| {
        let a = i as f32 / 48.0 * tau();
        (50.0 + 24.0 * a.cos(), 50.0 + 24.0 * a.sin())
    }).collect();
    let mut lines = vec![(circle, p.main, 4.5)];
    for (a, b) in [((50.0, 14.0), (50.0, 33.0)), ((50.0, 67.0), (50.0, 86.0)), ((14.0, 50.0), (33.0, 50.0)), ((67.0, 50.0), (86.0, 50.0))] {
        lines.push((vec![a, b], p.main, 4.5));
    }
    let dot = back(seg(t, 0.45, 0.75));
    format!(
        r##"<g opacity="{o:.3}" {}>{}</g><circle cx="50" cy="50" r="{:.2}" fill="{}" stroke="{INK}" stroke-width="2"/>"##,
        tf(50.0, 50.0, sc, rot),
        strokes(&lines),
        4.5 * dot,
        p.main
    )
}

// ------------------------------------------------------------------ Time

fn arc(cx: f32, cy: f32, r: f32, from: f32, to: f32) -> Pts {
    let n = (((to - from).abs() / 6.0).ceil() as usize).max(2);
    (0..=n)
        .map(|i| {
            let a = lerp(from, to, i as f32 / n as f32).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn countdown(t: f32, p: Pal, from: u32) -> String {
    let i = (t.floor() as u32).min(from - 1);
    let f = t - i as f32;
    let n = from - i;
    let all = 1.0 - seg(t, from as f32 - 0.25, from as f32);
    let start = back(seg(t, 0.0, 0.35));
    let mut s = format!(
        r##"<g {}><circle cx="50" cy="50" r="38" fill="#000000" fill-opacity="0.5"/><circle cx="50" cy="50" r="38" fill="none" stroke="{}" stroke-width="5"/>"##,
        tf(50.0, 50.0, all * start, 0.0),
        p.main
    );
    let last = i + 1 == from;
    let pop = back(seg(f, 0.0, 0.3)) * if last { 1.0 } else { 1.0 - seg(f, 0.94, 1.0) };
    let _ = write!(s, "<g {}>{}</g></g>", tf(50.0, 50.0, pop, 0.0), text(50.0, 72.0, 58.0, WHITE, 5.0, &n.to_string()));
    s
}

fn progress(t: f32, p: Pal, secs: f32) -> String {
    // viewBox 800×100 (bar across the whole width), linear: shows the time
    let k = clamp01(t / secs);
    let w = 768.0 * k;
    format!(
        r##"<rect x="16" y="34" width="768" height="32" rx="16" fill="#000000" fill-opacity="0.45" stroke="#FFFFFF" stroke-opacity="0.35" stroke-width="2"/>{}"##,
        if w > 0.5 {
            format!(r##"<rect x="16" y="34" width="{:.2}" height="32" rx="16" fill="{}"/>"##, w.max(32.0).min(768.0), p.main)
        } else {
            String::new()
        }
    )
}

fn rec(t: f32, _p: Pal) -> String {
    // viewBox 200×100, dot blinks softly (loop 1 s)
    let on = 0.5 + 0.5 * (t * tau()).cos();
    let on = on.powf(0.6);
    format!(
        r##"<circle cx="44" cy="50" r="17" fill="{RED}" fill-opacity="{:.3}" stroke="{INK}" stroke-opacity="{:.3}" stroke-width="3"/>{}"##,
        0.15 + 0.85 * on,
        0.3 + 0.7 * on,
        text(122.0, 70.0, 54.0, WHITE, 4.0, "REC")
    )
}

fn spinner(t: f32, p: Pal) -> String {
    let rot = t * 360.0;
    let len = 70.0 + 160.0 * (0.5 - 0.5 * (t * tau()).cos());
    let pts = arc(50.0, 50.0, 30.0, rot - 90.0, rot - 90.0 + len);
    let ring = arc(50.0, 50.0, 30.0, 0.0, 360.0);
    format!(
        r##"<path d="{}" fill="none" stroke="#000000" stroke-opacity="0.35" stroke-width="10"/>{}"##,
        d(&ring),
        stroke(&pts, p.main, 8.0)
    )
}

// ------------------------------------------------------------------ Particles

/// Fixed pseudo-random (deterministic)
fn rnd(i: u32, k: u32) -> f32 {
    let mut x = i.wrapping_mul(747_796_405).wrapping_add(k.wrapping_mul(2_891_336_453)).wrapping_add(12345);
    x ^= x >> 13;
    x = x.wrapping_mul(1_274_126_177);
    x ^= x >> 16;
    (x % 10_000) as f32 / 10_000.0
}

fn piece(x: f32, y: f32, rot: f32, flip: f32, color: &str, o: f32, round: bool) -> String {
    if round {
        format!(r##"<circle cx="{x:.2}" cy="{y:.2}" r="2.4" fill="{color}" opacity="{o:.3}" stroke="{INK}" stroke-width="0.6"/>"##)
    } else {
        format!(
            r##"<rect x="-2" y="-3.5" width="4" height="7" rx="0.8" fill="{color}" opacity="{o:.3}" stroke="{INK}" stroke-width="0.6" transform="translate({x:.2} {y:.2}) rotate({rot:.1}) scale({:.3} 1)"/>"##,
            flip.abs().max(0.15)
        )
    }
}

fn confetti_rain(t: f32, _p: Pal) -> String {
    // loop 2 s: every piece falls one or two full heights in 2 s
    let mut s = String::new();
    for i in 0..30 {
        let laps = 1.0 + (rnd(i, 1) * 2.0).floor();
        let y = ((rnd(i, 2) + laps * t / 2.0).fract()) * 116.0 - 8.0;
        let x = 6.0 + rnd(i, 3) * 88.0 + 4.0 * ((t / 2.0 * 2.0 + rnd(i, 4)) * tau()).sin();
        let rot = rnd(i, 5) * 360.0 + 360.0 * (t / 2.0) * if i % 2 == 0 { 1.0 } else { -1.0 };
        let flip = ((t / 2.0 * 3.0 + rnd(i, 6)) * tau()).cos();
        s.push_str(&piece(x, y, rot, flip, CONFETTI[i as usize % 6], 1.0, i % 7 == 0));
    }
    s
}

fn confetti_pop(t: f32, _p: Pal) -> String {
    let mut s = String::new();
    for i in 0..46 {
        let a = lerp(-150.0, -30.0, rnd(i, 1)).to_radians();
        let v = 190.0 + 140.0 * rnd(i, 2);
        // air drag slows the shot, then every piece sinks evenly
        let drag = (1.0 - (-3.0 * t).exp()) / 3.0;
        let x = 50.0 + a.cos() * v * drag * 0.8;
        let y = 98.0 + a.sin() * v * drag + 38.0 * t;
        let rot = rnd(i, 3) * 360.0 + 540.0 * t * if i % 2 == 0 { 1.0 } else { -1.0 };
        let flip = ((t * 2.5 + rnd(i, 4)) * tau()).cos();
        let o = 1.0 - seg(t, 1.15 + 0.2 * rnd(i, 5), 1.6);
        s.push_str(&piece(x, y, rot, flip, CONFETTI[i as usize % 6], o, i % 6 == 0));
    }
    s
}

fn sparks(t: f32, p: Pal) -> String {
    let mut s = String::new();
    let flash = seg(t, 0.0, 0.08) * (1.0 - seg(t, 0.08, 0.3));
    if flash > 0.0 {
        let _ = write!(s, r##"<circle cx="50" cy="50" r="{:.2}" fill="#FFFFFF" opacity="{:.3}"/>"##, 6.0 + 10.0 * flash, flash);
    }
    for i in 0..18 {
        let a = (i as f32 * 20.0 + rnd(i, 1) * 10.0).to_radians();
        let reach = 30.0 + 16.0 * rnd(i, 2);
        let head = out3(seg(t, 0.0, 0.55 + 0.15 * rnd(i, 3))) * reach + 6.0;
        let tail = out3(seg(t, 0.12, 0.8)) * reach + 6.0;
        if head - tail > 0.3 {
            let pts = vec![(50.0 + tail * a.cos(), 50.0 + tail * a.sin()), (50.0 + head * a.cos(), 50.0 + head * a.sin())];
            s.push_str(&stroke(&pts, if i % 3 == 0 { WHITE } else { p.main }, 2.6));
        }
    }
    s
}

fn glitter(t: f32, p: Pal) -> String {
    let spots = [(24.0, 26.0, 13.0), (70.0, 20.0, 9.0), (78.0, 62.0, 14.0), (36.0, 74.0, 10.0), (52.0, 46.0, 7.0), (14.0, 56.0, 7.0), (88.0, 86.0, 7.0)];
    let mut s = String::new();
    for (i, (x, y, r)) in spots.into_iter().enumerate() {
        let ph = (t / 1.5 + i as f32 / 7.0 * 3.0).fract();
        let k = (ph * tau()).sin().max(0.0).powf(1.5);
        if k > 0.02 {
            let _ = write!(s, "<g {}>{}</g>", tf(x, y, k, ph * 90.0), sparkle(x, y, r, if i % 3 == 1 { WHITE } else { p.main }));
        }
    }
    s
}

fn hearts_rise(t: f32, _p: Pal) -> String {
    let mut s = String::new();
    for i in 0..9 {
        let ph = (t / 2.0 + i as f32 / 9.0).fract();
        let y = 104.0 - ph * 112.0;
        let x = 14.0 + rnd(i, 1) * 72.0 + 6.0 * ((ph + rnd(i, 2)) * tau()).sin();
        let sc = 0.11 + 0.08 * rnd(i, 3);
        let o = seg(ph, 0.0, 0.12) * (1.0 - seg(ph, 0.7, 1.0));
        s.push_str(&heart(x, y, sc, 12.0 * ((ph * 2.0) * tau()).sin(), if i % 2 == 0 { RED } else { PINK }, o));
    }
    s
}

// ------------------------------------------------------------------ Speech bubbles, frames

fn bubble(t: f32, p: Pal) -> String {
    let sc = back(seg(t, 0.0, 0.45));
    format!(
        r##"<g {}><path transform="translate(50 50) scale(0.84) translate(-50 -50)" d="M50 10 C76 10 94 24 94 42 C94 60 76 73 52 73 C47 73 42 72 38 71 L18 88 L24 66 C12 60 6 52 6 42 C6 24 24 10 50 10Z" fill="{}" stroke="{INK}" stroke-width="4" stroke-linejoin="round"/></g>"##,
        tf(25.0, 81.0, sc, (1.0 - spring(seg(t, 0.0, 0.8))) * -12.0),
        p.main
    )
}

fn thought(t: f32, p: Pal) -> String {
    let mut s = String::new();
    for (k, (x, y, r)) in [(0.0, (16.0, 92.0, 4.0)), (0.12, (26.0, 80.0, 6.5))] {
        let _ = write!(s, r##"<circle cx="{x}" cy="{y}" r="{:.2}" fill="{}" stroke="{INK}" stroke-width="3"/>"##, r * back(seg(t, k, k + 0.25)), p.main);
    }
    let sc = back(seg(t, 0.22, 0.65));
    let _ = write!(
        s,
        r##"<g {}><path transform="translate(54 40) scale(0.84) translate(-54 -40)" d="M30 64 C16 64 10 52 18 44 C10 34 20 20 34 24 C38 12 56 10 62 20 C72 12 88 18 86 32 C96 38 94 54 82 56 C82 66 68 72 58 64 C52 72 36 72 30 64Z" fill="{}" stroke="{INK}" stroke-width="3.5" stroke-linejoin="round"/></g>"##,
        tf(40.0, 64.0, sc, 0.0),
        p.main
    );
    s
}

fn lower_third(t: f32, p: Pal) -> String {
    // viewBox 400×100
    let a = back(seg(t, 0.0, 0.35));
    let main = out3(seg(t, 0.12, 0.6)) * 1.0;
    let over = lerp(1.03, 1.0, seg(t, 0.6, 0.85));
    let sub = out3(seg(t, 0.3, 0.8));
    let mut s = String::new();
    if main > 0.0 {
        let _ = write!(s, r##"<rect x="22" y="22" width="{:.2}" height="40" fill="{INK}" fill-opacity="0.88" stroke="#FFFFFF" stroke-opacity="0.3" stroke-width="1"/>"##, 360.0 * main * if main >= 1.0 { over } else { 1.0 });
    }
    if sub > 0.0 {
        let _ = write!(s, r##"<rect x="22" y="64" width="{:.2}" height="18" fill="{}"/>"##, 230.0 * sub, p.main);
    }
    let _ = write!(s, r##"<rect x="10" y="{:.2}" width="9" height="{:.2}" fill="{}"/>"##, lerp(52.0, 22.0, a), 60.0 * a, p.main);
    s
}

fn frame_corners(t: f32, p: Pal) -> String {
    let mut lines = Vec::new();
    for (i, (sx, sy)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].into_iter().enumerate() {
        let k = inout(seg(t, i as f32 * 0.08, i as f32 * 0.08 + 0.45));
        let (cx, cy) = (50.0 + sx * 42.0, 50.0 + sy * 42.0);
        let corner = [(cx, cy - sy * 26.0), (cx, cy), (cx - sx * 26.0, cy)];
        lines.push((prefix(&corner, k), p.main, 5.0));
    }
    strokes(&lines)
}

fn frame_full(t: f32, p: Pal) -> String {
    // runs around the border starting from the top left
    let r = [(7.0, 7.0), (93.0, 7.0), (93.0, 93.0), (7.0, 93.0), (7.0, 7.0)];
    stroke(&prefix(&r, inout(seg(t, 0.0, 0.8))), p.main, 4.5)
}

// ------------------------------------------------------------------ List

fn elements() -> Vec<El> {
    let pal = |main, on| Pal { main, on };
    let red = ("rot", "rot", "red", pal(RED, WHITE));
    let yellow = ("gelb", "gelb", "yellow", pal(YELLOW, INK));
    let white = ("weiss", "weiß", "white", pal(WHITE, INK));
    let blue = ("blau", "blau", "blue", pal(BLUE, WHITE));
    let pink = ("rosa", "rosa", "pink", pal(PINK, WHITE));
    let one = |p: Pal| vec![("", "", "", p)];
    let sq = |id, de, en, cat, secs, looping, variants, draw| El { id, de, en, cat, w: 360, h: 360, secs, looping, variants, draw };
    vec![
        // calls to action
        El { id: "abonnieren", de: "Abonnieren", en: "Subscribe, German", cat: "Aufrufe", w: 480, h: 160, secs: 1.7, looping: false,
            variants: vec![red, white], draw: |t, p| sub_button(t, p, "ABONNIEREN", "ABONNIERT") },
        El { id: "subscribe", de: "Subscribe", en: "Subscribe", cat: "Aufrufe", w: 480, h: 160, secs: 1.7, looping: false,
            variants: vec![red, white], draw: |t, p| sub_button(t, p, "SUBSCRIBE", "SUBSCRIBED") },
        sq("glocke", "Glocke", "Bell", "Aufrufe", 1.4, true, vec![white, yellow], bell),
        sq("daumen-hoch", "Daumen hoch", "Thumbs up", "Aufrufe", 1.0, false, vec![blue, red], thumbs_up),
        sq("herz-puls", "Herz pulsiert", "Beating heart", "Aufrufe", 1.0, true, vec![red, pink], heart_beat),
        sq("teilen", "Teilen", "Share", "Aufrufe", 1.0, false, vec![white, yellow], share),
        El { id: "link-unten", de: "Link in der Beschreibung", en: "Link in description, German", cat: "Aufrufe", w: 400, h: 200, secs: 1.0, looping: true,
            variants: one(yellow.3), draw: |t, p| link_below(t, p, "LINK IN DER", "BESCHREIBUNG") },
        El { id: "link-below", de: "Link in description", en: "Link in description", cat: "Aufrufe", w: 400, h: 200, secs: 1.0, looping: true,
            variants: one(yellow.3), draw: |t, p| link_below(t, p, "LINK IN THE", "DESCRIPTION") },
        sq("neu", "Neu", "New, German", "Aufrufe", 1.0, false, vec![yellow, red], |t, p| badge(t, p, "NEU")),
        sq("new", "New", "New", "Aufrufe", 1.0, false, vec![yellow, red], |t, p| badge(t, p, "NEW")),
        // pointing
        sq("pfeil-gerade", "Pfeil zeichnet sich", "Drawn arrow", "Zeigen", 0.9, false, vec![red, yellow, white], arrow_straight),
        sq("pfeil-geschwungen", "Pfeil geschwungen", "Curved arrow", "Zeigen", 0.9, false, vec![red, yellow, white], arrow_curved),
        sq("kreis-markierung", "Einkreisen", "Circle it", "Zeigen", 0.8, false, vec![red, yellow], circle_mark),
        El { id: "unterstreichen", de: "Unterstreichen", en: "Underline", cat: "Zeigen", w: 480, h: 160, secs: 0.8, looping: false,
            variants: vec![red, yellow], draw: underline },
        sq("ausrufezeichen", "Ausrufezeichen", "Exclamation mark", "Zeigen", 1.0, false, vec![red, yellow], |t, p| pop_glyph(t, p, "!")),
        sq("fragezeichen", "Fragezeichen", "Question mark", "Zeigen", 1.0, false, vec![red, yellow], |t, p| pop_glyph(t, p, "?")),
        sq("fokus", "Fokus-Ecken", "Focus corners", "Zeigen", 0.8, false, vec![white, yellow], focus_corners),
        sq("zielkreuz", "Zielkreuz", "Crosshair", "Zeigen", 0.9, false, vec![red, white], target),
        // time
        sq("countdown-3", "Countdown 3-2-1", "Countdown 3-2-1", "Zeit", 3.0, false, vec![red], |t, p| countdown(t, p, 3)),
        sq("countdown-5", "Countdown 5-4-3-2-1", "Countdown 5-4-3-2-1", "Zeit", 5.0, false, vec![red], |t, p| countdown(t, p, 5)),
        El { id: "fortschritt-5", de: "Fortschritt 5 s", en: "Progress 5 s", cat: "Zeit", w: 640, h: 80, secs: 5.0, looping: false,
            variants: vec![red, white], draw: |t, p| progress(t, p, 5.0) },
        El { id: "fortschritt-10", de: "Fortschritt 10 s", en: "Progress 10 s", cat: "Zeit", w: 640, h: 80, secs: 10.0, looping: false,
            variants: vec![red, white], draw: |t, p| progress(t, p, 10.0) },
        El { id: "rec", de: "REC", en: "REC", cat: "Zeit", w: 400, h: 200, secs: 1.0, looping: true, variants: one(red.3), draw: rec },
        sq("laden", "Lade-Kreis", "Loading circle", "Zeit", 1.0, true, vec![white, blue], spinner),
        // particles
        sq("konfetti-regen", "Konfetti-Regen", "Confetti rain", "Partikel", 2.0, true, one(red.3), confetti_rain),
        sq("konfetti-knall", "Konfetti-Knall", "Confetti pop", "Partikel", 1.6, false, one(red.3), confetti_pop),
        sq("funken", "Funken", "Sparks", "Partikel", 0.9, false, vec![yellow, blue], sparks),
        sq("glitzer", "Sterne-Glitzer", "Twinkling stars", "Partikel", 1.5, true, vec![yellow, white], glitter),
        sq("herzen", "Herzen steigen auf", "Rising hearts", "Partikel", 2.0, true, one(red.3), hearts_rise),
        // speech bubbles, frames
        sq("sprechblase", "Sprechblase", "Speech bubble", "Sprechblasen", 0.8, false, vec![white, yellow], bubble),
        sq("denkblase", "Denkblase", "Thought bubble", "Sprechblasen", 0.9, false, vec![white], thought),
        El { id: "bauchbinde", de: "Bauchbinde", en: "Lower third", cat: "Rahmen", w: 800, h: 200, secs: 1.0, looping: false,
            variants: vec![red, blue, yellow], draw: lower_third },
        sq("ecken", "Bildecken", "Frame corners", "Rahmen", 0.9, false, vec![white, yellow], frame_corners),
        sq("rahmen", "Bildrahmen", "Picture frame", "Rahmen", 0.9, false, vec![white, red], frame_full),
    ]
}

fn render(e: &El, p: Pal, t: f32) -> Vec<u8> {
    let vw = 100.0 * e.w as f32 / e.h as f32;
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {vw} 100" width="{vw}" height="100">{}</svg>"##,
        (e.draw)(t, p)
    );
    let mut px = cl_fx::stickers::render_svg_wh(&svg, e.w, e.h).unwrap_or_else(|| panic!("{}: SVG", e.id));
    // premultiplied → straight
    for c in px.chunks_mut(4) {
        let a = c[3] as u32;
        if a > 0 && a < 255 {
            for v in &mut c[..3] {
                *v = ((*v as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    px
}

/// Shared palette (≤ 256 colors with alpha) for all frames of an element: the most frequent
/// colors fixed, the rest via k-means over premultiplied RGBA. Halves the file size
/// compared with RGBA, soft edges and fades stay (full alpha per palette color).
fn palette(frames: &[Vec<u8>]) -> (Vec<[u8; 4]>, std::collections::HashMap<u32, u8>) {
    use std::collections::HashMap;
    let key = |c: &[u8]| if c[3] == 0 { 0 } else { u32::from_le_bytes([c[0], c[1], c[2], c[3]]) };
    let mut hist: HashMap<u32, u64> = HashMap::new();
    for f in frames {
        for c in f.chunks(4) {
            *hist.entry(key(c)).or_default() += 1;
        }
    }
    hist.insert(0, u64::MAX / 2);
    let mut colors: Vec<(u32, u64)> = hist.into_iter().collect();
    // fixed order (deterministic): by frequency, then value
    colors.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let pm = |k: u32| {
        let [r, g, b, a] = k.to_le_bytes();
        let f = a as f32 / 255.0;
        [r as f32 * f, g as f32 * f, b as f32 * f, a as f32]
    };
    let dist = |a: &[f32; 4], b: &[f32; 4]| (0..4).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>();
    const FIXED: usize = 48;
    let n = colors.len().min(256);
    let mut cent: Vec<[f32; 4]> = colors[..n].iter().map(|c| pm(c.0)).collect();
    let pts: Vec<([f32; 4], f32)> = colors.iter().map(|c| (pm(c.0), c.1.min(1 << 40) as f32)).collect();
    let mut assign = vec![0usize; pts.len()];
    if colors.len() > 256 {
        for _ in 0..8 {
            for (i, (p, _)) in pts.iter().enumerate() {
                assign[i] = (0..n).min_by(|&a, &b| dist(p, &cent[a]).total_cmp(&dist(p, &cent[b]))).unwrap();
            }
            let mut sum = vec![([0.0f64; 4], 0.0f64); n];
            for (i, (p, w)) in pts.iter().enumerate() {
                let s = &mut sum[assign[i]];
                for k in 0..4 {
                    s.0[k] += p[k] as f64 * *w as f64;
                }
                s.1 += *w as f64;
            }
            for (j, (s, w)) in sum.into_iter().enumerate().skip(FIXED) {
                if w > 0.0 {
                    cent[j] = [0, 1, 2, 3].map(|k| (s[k] / w) as f32);
                }
            }
        }
    }
    let pal: Vec<[u8; 4]> = cent
        .iter()
        .map(|c| {
            let a = c[3].round().clamp(0.0, 255.0);
            if a < 1.0 {
                return [0, 0, 0, 0];
            }
            let st = |v: f32| (v * 255.0 / a).round().clamp(0.0, 255.0) as u8;
            [st(c[0]), st(c[1]), st(c[2]), a as u8]
        })
        .collect();
    let pal_pm: Vec<[f32; 4]> = pal.iter().map(|c| pm(u32::from_le_bytes(*c))).collect();
    let map = colors
        .iter()
        .map(|c| {
            let p = pm(c.0);
            let i = (0..pal.len()).min_by(|&a, &b| dist(&p, &pal_pm[a]).total_cmp(&dist(&p, &pal_pm[b]))).unwrap();
            (c.0, i as u8)
        })
        .collect();
    (pal, map)
}

fn encode(e: &El, p: Pal, out: &Path) {
    let n = (e.secs * FPS as f32).round() as u32;
    // loop: frame n would be frame 0 again; one-off: final state as the last frame
    let count = if e.looping { n } else { n + 1 };
    let frames: Vec<Vec<u8>> = (0..count).map(|k| render(e, p, k as f32 / FPS as f32)).collect();
    let (pal, map) = palette(&frames);
    let mut pal_bytes = vec![0u8; 1024];
    for (i, c) in pal.iter().enumerate() {
        // FFmpeg PAL8: ARGB per entry as u32 (little endian: B, G, R, A)
        pal_bytes[i * 4..i * 4 + 4].copy_from_slice(&[c[2], c[1], c[0], c[3]]);
    }
    let mut child = cl_media::ffmpeg_command()
        .expect("FFmpeg")
        .args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "pal8", "-s"])
        .arg(format!("{}x{}", e.w, e.h))
        .args(["-framerate", &FPS.to_string(), "-i", "-", "-plays", if e.looping { "0" } else { "1" }])
        .args(["-pred", "mixed", "-compression_level", "9", "-f", "apng"])
        .arg(out)
        .stdin(Stdio::piped())
        .spawn()
        .expect("FFmpeg starten");
    let mut stdin = child.stdin.take().unwrap();
    for f in &frames {
        let idx: Vec<u8> = f
            .chunks(4)
            .map(|c| map[&if c[3] == 0 { 0 } else { u32::from_le_bytes([c[0], c[1], c[2], c[3]]) }])
            .collect();
        stdin.write_all(&idx).unwrap();
        stdin.write_all(&pal_bytes).unwrap();
    }
    drop(stdin);
    assert!(child.wait().unwrap().success(), "{}", out.display());
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn main() {
    let only = std::env::args().nth(1);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/cutline-elemente");
    let files = root.join("elemente");
    std::fs::create_dir_all(&files).unwrap();
    let mut toml = String::from(
        "# Generated by crates/cl-fx/examples/gen_elemente.rs – do not edit by hand.\n\
         # Looping or one-shot is stored in the file itself (APNG repetitions); the tag\n\
         # \"schleife\" (loop) is only for search.\n\
         [plugin]\n\
         id = \"cutline.elemente\"\n\
         version = \"1.0.0\"\n\
         api = 1\n\
         kind = \"asset-pack\"\n\
         name = { de = \"Bewegte Elemente\", en = \"Animated elements\" }\n\
         description = { de = \"Eigene bewegte Elemente zum Drüberlegen\", en = \"Original animated overlay elements\" }\n\
         authors = [\"Cutline-Projekt\"]\n\
         license = \"CC0-1.0\"\n\
         source = \"https://github.com/zqqqqx/cutline\"\n",
    );
    for e in elements() {
        for (vid, vde, ven, p) in &e.variants {
            let id = if vid.is_empty() { e.id.to_string() } else { format!("{}-{vid}", e.id) };
            let name = |base: &str, v: &str| if v.is_empty() { base.to_string() } else { format!("{base} ({v})") };
            let file = format!("elemente/{id}.png");
            if only.as_deref().is_none_or(|o| id.contains(o)) {
                encode(&e, *p, &root.join(&file));
                println!("{id}: {} KB", std::fs::metadata(root.join(&file)).unwrap().len() / 1024);
            }
            let _ = write!(
                toml,
                "\n[[asset]]\nid = {}\nkind = \"sticker\"\nname = {{ de = {}, en = {} }}\ncategory = [{}]\nfile = {}\n{}",
                toml_str(&id),
                toml_str(&name(e.de, vde)),
                toml_str(&name(e.en, ven)),
                toml_str(e.cat),
                toml_str(&file),
                if e.looping { "tags = [\"schleife\"]\n" } else { "" }
            );
        }
    }
    std::fs::write(root.join("plugin.toml"), toml).unwrap();
}
