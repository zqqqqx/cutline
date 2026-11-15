//! Eingebaute Übergänge für die Vorschau: dieselben Formeln wie FFmpegs `xfade` (Export rechnet
//! in RGB, `format=gbrap`), damit Vorschau und Export dasselbe Bild zeigen. Bilder sind RGBA,
//! A und B gleich groß; `t` ist der Fortschritt 0 (nur A) … 1 (nur B).
//!
//! `xfade` zählt rückwärts (`progress` 1 → 0) und mischt mit `mix(a, b, m) = a·m + b·(1 − m)`;
//! beides ist hier übernommen, damit die Formeln eins zu eins vergleichbar bleiben.

/// Sorten, die [`mix`] nachbildet; alle übrigen überblendet die Vorschau.
pub const SUPPORTED: &[&str] = &[
    "fade", "fadefast", "fadeslow", "fadeblack", "fadewhite", "fadegrays", "distance", "dissolve", "wipeleft", "wiperight",
    "wipeup", "wipedown", "wipetl", "wipetr", "wipebl", "wipebr", "smoothleft", "smoothright", "smoothup", "smoothdown",
    "slideleft", "slideright", "slideup", "slidedown", "coverleft", "coverright", "coverup", "coverdown", "revealleft",
    "revealright", "revealup", "revealdown", "circleopen", "circleclose", "circlecrop", "rectcrop", "vertopen", "vertclose",
    "horzopen", "horzclose", "radial", "diagtl", "diagtr", "diagbl", "diagbr", "hlslice", "hrslice", "vuslice", "vdslice",
    "pixelize", "hblur", "zoomin", "squeezeh", "hlwind", "hrwind", "vuwind", "vdwind",
];

pub fn supported(kind: &str) -> bool {
    SUPPORTED.contains(&kind)
}

fn mixf(a: f32, b: f32, m: f32) -> f32 {
    a * m + b * (1.0 - m)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn fract(x: f32) -> f32 {
    x - x.floor()
}

fn frand(x: f32, y: f32) -> f32 {
    fract((x * 12.9898 + y * 78.233).sin() * 43758.545)
}

/// Zeilen von `out` (Breite `w` Pixel) auf mehrere Kerne verteilt füllen: `f(y, zeile)`.
pub fn rows(out: &mut [u8], w: i32, f: &(dyn Fn(i32, &mut [u8]) + Sync)) {
    let line = w.max(1) as usize * 4;
    let h = out.len() / line;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    let per = h.div_ceil(threads).max(1);
    std::thread::scope(|sc| {
        for (k, part) in out.chunks_mut(per * line).enumerate() {
            sc.spawn(move || {
                for (j, l) in part.chunks_exact_mut(line).enumerate() {
                    f((k * per + j) as i32, l);
                }
            });
        }
    });
}

/// Mischt A und B wie `xfade=transition=kind` beim Fortschritt `t`. `None`: Sorte unbekannt
/// oder Bilder passen nicht zusammen.
pub fn mix(kind: &str, a: &[u8], b: &[u8], w: u32, h: u32, t: f32) -> Option<Vec<u8>> {
    let (wi, hi) = (w as i32, h as i32);
    let n = (w * h * 4) as usize;
    if w == 0 || h == 0 || a.len() < n || b.len() < n || !supported(kind) {
        return None;
    }
    let p = (1.0 - t).clamp(0.0, 1.0);
    let (wf, hf) = (w as f32, h as f32);
    let px = |img: &[u8], x: i32, y: i32| -> [u8; 4] {
        let i = ((y.clamp(0, hi - 1) * wi + x.clamp(0, wi - 1)) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    };
    let mut out = vec![0u8; n];
    // Gewicht von A je Pixel (A·g + B·(1 − g))
    let weighted = |out: &mut [u8], g: &(dyn Fn(f32, f32) -> f32 + Sync)| {
        rows(out, wi, &|y, line| {
            for x in 0..wi {
                let k = g(x as f32, y as f32);
                let i = ((y * wi + x) * 4) as usize;
                for c in 0..4 {
                    line[x as usize * 4 + c] = mixf(a[i + c] as f32, b[i + c] as f32, k).round().clamp(0.0, 255.0) as u8;
                }
            }
        })
    };
    // Pixel aus A oder B an verschobener Stelle
    let pick = |out: &mut [u8], f: &(dyn Fn(i32, i32) -> [u8; 4] + Sync)| {
        rows(out, wi, &|y, line| {
            for x in 0..wi {
                line[x as usize * 4..x as usize * 4 + 4].copy_from_slice(&f(x, y));
            }
        })
    };
    let wrap = |v: i32, len: i32| v % len + len * (v < 0) as i32;
    let ss = |s: f32| smoothstep(0.0, 1.0, s);
    let (cx, cy) = ((wi / 2) as f32, (hi / 2) as f32);
    match kind {
        "fade" => weighted(&mut out, &|_, _| p),
        "fadefast" | "fadeslow" => {
            for i in 0..n {
                let (x0, x1) = (a[i] as f32, b[i] as f32);
                let d = (x0 - x1).abs() / 255.0;
                let e = if kind == "fadefast" { 1.0 + (1.0 + d).ln() } else { 1.0 + (2.0 - d).ln() };
                out[i] = mixf(x0, x1, p.powf(e)).round().clamp(0.0, 255.0) as u8;
            }
        }
        "fadeblack" | "fadewhite" | "fadegrays" => {
            let (k0, k1) = (smoothstep(0.8, 1.0, p), smoothstep(0.2, 1.0, p));
            for px_i in 0..(w * h) as usize {
                let i = px_i * 4;
                let gray = |img: &[u8]| ((img[i] as u32 + img[i + 1] as u32 + img[i + 2] as u32) / 3) as f32;
                for c in 0..4 {
                    let (bg0, bg1) = match kind {
                        _ if c == 3 => (255.0, 255.0),
                        "fadeblack" => (0.0, 0.0),
                        "fadewhite" => (255.0, 255.0),
                        _ => (gray(a), gray(b)),
                    };
                    let v = mixf(mixf(a[i + c] as f32, bg0, k0), mixf(bg1, b[i + c] as f32, k1), p);
                    out[i + c] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        "distance" => {
            for px_i in 0..(w * h) as usize {
                let i = px_i * 4;
                let d: f32 = (0..4).map(|c| (a[i + c] as f32 / 255.0 - b[i + c] as f32 / 255.0).powi(2)).sum::<f32>().sqrt();
                let dist = if d <= p { 1.0 } else { 0.0 };
                for c in 0..4 {
                    let (x0, x1) = (a[i + c] as f32, b[i + c] as f32);
                    out[i + c] = mixf(mixf(x0, x1, dist), x1, p).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        "dissolve" => weighted(&mut out, &|x, y| if frand(x, y) * 2.0 + p * 2.0 - 1.5 >= 0.5 { 1.0 } else { 0.0 }),
        "wipeleft" => {
            let z = (wf * p) as i32 as f32;
            weighted(&mut out, &|x, _| if x > z { 0.0 } else { 1.0 })
        }
        "wiperight" => {
            let z = (wf * (1.0 - p)) as i32 as f32;
            weighted(&mut out, &|x, _| if x > z { 1.0 } else { 0.0 })
        }
        "wipeup" => {
            let z = (hf * p) as i32 as f32;
            weighted(&mut out, &|_, y| if y > z { 0.0 } else { 1.0 })
        }
        "wipedown" => {
            let z = (hf * (1.0 - p)) as i32 as f32;
            weighted(&mut out, &|_, y| if y > z { 1.0 } else { 0.0 })
        }
        "wipetl" | "wipetr" | "wipebl" | "wipebr" => {
            let left = kind == "wipetl" || kind == "wipebl";
            let top = kind == "wipetl" || kind == "wipetr";
            let zw = (wf * if left { p } else { 1.0 - p }) as i32 as f32;
            let zh = (hf * if top { p } else { 1.0 - p }) as i32 as f32;
            weighted(&mut out, &|x, y| {
                let in_x = if left { x <= zw } else { x > zw };
                let in_y = if top { y <= zh } else { y > zh };
                if in_x && in_y { 1.0 } else { 0.0 }
            })
        }
        "smoothleft" => weighted(&mut out, &|x, _| 1.0 - ss(1.0 + x / wf - p * 2.0)),
        "smoothright" => weighted(&mut out, &|x, _| 1.0 - ss(1.0 + (wf - 1.0 - x) / wf - p * 2.0)),
        "smoothup" => weighted(&mut out, &|_, y| 1.0 - ss(1.0 + y / hf - p * 2.0)),
        "smoothdown" => weighted(&mut out, &|_, y| 1.0 - ss(1.0 + (hf - 1.0 - y) / hf - p * 2.0)),
        "slideleft" | "slideright" | "coverleft" | "coverright" | "revealleft" | "revealright" => {
            let z = if kind.ends_with("left") { -p * wf } else { p * wf } as i32;
            pick(&mut out, &|x, y| {
                let zx = z + x;
                let zz = wrap(zx, wi);
                let inside = (0..wi).contains(&zx);
                match (&kind[..5], inside) {
                    ("slide", true) | ("cover", true) => px(b, zz, y),
                    ("slide", false) | ("revea", false) => px(a, zz, y),
                    ("cover", false) => px(a, x, y),
                    _ => px(b, x, y),
                }
            })
        }
        "slideup" | "slidedown" | "coverup" | "coverdown" | "revealup" | "revealdown" => {
            let z = if kind.ends_with("up") { -p * hf } else { p * hf } as i32;
            pick(&mut out, &|x, y| {
                let zy = z + y;
                let zz = wrap(zy, hi);
                let inside = (0..hi).contains(&zy);
                match (&kind[..5], inside) {
                    ("slide", true) | ("cover", true) => px(b, x, zz),
                    ("slide", false) | ("revea", false) => px(a, x, zz),
                    ("cover", false) => px(a, x, y),
                    _ => px(b, x, y),
                }
            })
        }
        "circleopen" | "circleclose" => {
            let z = cx.hypot(cy);
            let q = (p - 0.5) * 3.0;
            if kind == "circleopen" {
                weighted(&mut out, &|x, y| ss((x - cx).hypot(y - cy) / z + q))
            } else {
                weighted(&mut out, &|x, y| 1.0 - ss((x - cx).hypot(y - cy) / z - q))
            }
        }
        "circlecrop" => {
            let z = (2.0 * (p - 0.5).abs()).powi(3) * cx.hypot(cy);
            pick(&mut out, &|x, y| {
                let dist = (x as f32 - cx).hypot(y as f32 - cy);
                if z < dist {
                    [0, 0, 0, 255]
                } else if p < 0.5 {
                    px(b, x, y)
                } else {
                    px(a, x, y)
                }
            })
        }
        "rectcrop" => {
            let zh = ((p - 0.5).abs() * hf) as i32;
            let zw = ((p - 0.5).abs() * wf) as i32;
            pick(&mut out, &|x, y| {
                let inside = (x - wi / 2).abs() < zw && (y - hi / 2).abs() < zh;
                if !inside {
                    [0, 0, 0, 255]
                } else if p < 0.5 {
                    px(b, x, y)
                } else {
                    px(a, x, y)
                }
            })
        }
        "vertopen" => weighted(&mut out, &|x, _| 1.0 - ss(2.0 - ((x - cx) / cx).abs() - p * 2.0)),
        "vertclose" => weighted(&mut out, &|x, _| 1.0 - ss(1.0 + ((x - cx) / cx).abs() - p * 2.0)),
        "horzopen" => weighted(&mut out, &|_, y| 1.0 - ss(2.0 - ((y - cy) / cy).abs() - p * 2.0)),
        "horzclose" => weighted(&mut out, &|_, y| 1.0 - ss(1.0 + ((y - cy) / cy).abs() - p * 2.0)),
        "radial" => weighted(&mut out, &|x, y| {
            1.0 - ss((x - cx).atan2(y - cy) - (p - 0.5) * (std::f32::consts::PI * 2.5))
        }),
        "diagtl" => weighted(&mut out, &|x, y| 1.0 - ss(1.0 + x / wf * y / hf - p * 2.0)),
        "diagtr" => weighted(&mut out, &|x, y| 1.0 - ss(1.0 + (wf - 1.0 - x) / wf * y / hf - p * 2.0)),
        "diagbl" => weighted(&mut out, &|x, y| 1.0 - ss(1.0 + x / wf * (hf - 1.0 - y) / hf - p * 2.0)),
        "diagbr" => weighted(&mut out, &|x, y| 1.0 - ss(1.0 + (wf - 1.0 - x) / wf * (hf - 1.0 - y) / hf - p * 2.0)),
        "hlslice" | "hrslice" | "vuslice" | "vdslice" => weighted(&mut out, &|x, y| {
            let v = match kind {
                "hlslice" => x / wf,
                "hrslice" => (wf - 1.0 - x) / wf,
                "vuslice" => y / hf,
                _ => (hf - 1.0 - y) / hf,
            };
            let s = smoothstep(-0.5, 0.0, v - p * 1.5);
            if s <= fract(10.0 * v) { 1.0 } else { 0.0 }
        }),
        "pixelize" => {
            let d = p.min(1.0 - p);
            let dist = (d * 50.0).ceil() / 50.0;
            let sq = 2.0 * dist * w.min(h) as f32 / 20.0;
            pick(&mut out, &|x, y| {
                let (sx, sy) = if dist > 0.0 {
                    ((((x as f32 / sq).floor() + 0.5) * sq).min(wf - 1.0) as i32, (((y as f32 / sq).floor() + 0.5) * sq).min(hf - 1.0) as i32)
                } else {
                    (x, y)
                };
                let (pa, pb) = (px(a, sx, sy), px(b, sx, sy));
                [0, 1, 2, 3].map(|c| mixf(pa[c] as f32, pb[c] as f32, p).round().clamp(0.0, 255.0) as u8)
            })
        }
        "hblur" => {
            let prog = if p <= 0.5 { p * 2.0 } else { (1.0 - p) * 2.0 };
            let size = 1 + ((wi / 2) as f32 * prog) as i32;
            for y in 0..hi {
                for c in 0..4 {
                    let at = |img: &[u8], x: i32| img[((y * wi + x) * 4) as usize + c] as f32;
                    let (mut s0, mut s1, mut cnt) = (0.0f32, 0.0f32, size as f32);
                    for x in 0..size.min(wi) {
                        s0 += at(a, x);
                        s1 += at(b, x);
                    }
                    for x in 0..wi {
                        out[((y * wi + x) * 4) as usize + c] = mixf(s0 / cnt, s1 / cnt, p).round().clamp(0.0, 255.0) as u8;
                        if x + size < wi {
                            s0 += at(a, x + size) - at(a, x);
                            s1 += at(b, x + size) - at(b, x);
                        } else {
                            s0 -= at(a, x);
                            s1 -= at(b, x);
                            cnt -= 1.0;
                        }
                    }
                }
            }
        }
        "zoomin" => {
            let zf = smoothstep(0.5, 1.0, p);
            let k = smoothstep(0.0, 0.5, p);
            for y in 0..hi {
                for x in 0..wi {
                    let u = zf * (x as f32 / wf - 0.5) + 0.5;
                    let v = zf * (y as f32 / hf - 0.5) + 0.5;
                    let za = px(a, (u * (wf - 1.0)).ceil() as i32, (v * (hf - 1.0)).ceil() as i32);
                    let pb = px(b, x, y);
                    let i = ((y * wi + x) * 4) as usize;
                    for c in 0..4 {
                        out[i + c] = mixf(za[c] as f32, pb[c] as f32, k).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
        // Zeilen von A zur Mitte gestaucht, oben und unten B
        "squeezeh" => pick(&mut out, &|x, y| {
            let z = 0.5 + (y as f32 / hf - 0.5) / p;
            if !(0.0..=1.0).contains(&z) {
                px(b, x, y)
            } else {
                px(a, x, (z * (hf - 1.0)).round() as i32)
            }
        }),
        "hlwind" | "hrwind" | "vuwind" | "vdwind" => weighted(&mut out, &|x, y| {
            let (f, r) = match kind {
                "hlwind" => (x / wf, frand(0.0, y)),
                "hrwind" => ((wf - 1.0 - x) / wf, frand(0.0, y)),
                "vuwind" => (y / hf, frand(x, 0.0)),
                _ => ((hf - 1.0 - y) / hf, frand(x, 0.0)),
            };
            // B weht von der Gegenseite herein, je Zeile/Spalte zufällig versetzt
            smoothstep(0.0, -0.2, f * (1.0 - 0.2) + 0.2 * (1.0 - r) - p * (1.0 + 0.2))
        }),
        _ => return None,
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 320;
    const H: u32 = 180;

    fn ffmpeg(args: &[&str]) -> Vec<u8> {
        let out = cl_media::ffmpeg_command()
            .unwrap()
            .args(["-v", "error", "-nostdin", "-threads", "2", "-filter_threads", "1"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        out.stdout
    }

    /// Vorschau-Formel gegen FFmpegs `xfade` (wie im Export in RGB) bei Fortschritt 0,3 und 0,5:
    /// mittlere Abweichung je Kanal.
    #[test]
    fn preview_matches_ffmpeg_xfade() {
        let dir = tempfile::tempdir().unwrap();
        let (pa, pb) = (dir.path().join("a.png"), dir.path().join("b.png"));
        let src = format!("testsrc2=s={W}x{H}:r=1:d=1");
        for (p, extra) in [(&pa, ""), (&pb, ",hflip,hue=h=150,eq=brightness=0.1")] {
            ffmpeg(&["-y", "-f", "lavfi", "-i", &format!("{src}{extra}"), "-frames:v", "1", p.to_str().unwrap()]);
        }
        let raw = |p: &std::path::Path| ffmpeg(&["-i", p.to_str().unwrap(), "-f", "rawvideo", "-pix_fmt", "rgba", "-"]);
        let (a, b) = (raw(&pa), raw(&pb));
        let mut bad = Vec::new();
        for kind in SUPPORTED {
            // 10 Bilder/s, Übergang über 1 s ab 0: Bild 3 → Fortschritt 0,3, Bild 5 → 0,5
            let got = ffmpeg(&[
                "-loop", "1", "-framerate", "10", "-t", "1", "-i", pa.to_str().unwrap(),
                "-loop", "1", "-framerate", "10", "-t", "1", "-i", pb.to_str().unwrap(),
                "-filter_complex",
                &format!("[0]format=gbrap[a];[1]format=gbrap[b];[a][b]xfade=transition={kind}:duration=1:offset=0,select='eq(n,3)+eq(n,5)',format=rgba"),
                "-fps_mode", "passthrough", "-f", "rawvideo", "-",
            ]);
            let n = (W * H * 4) as usize;
            assert_eq!(got.len(), 2 * n, "{kind}");
            for (k, t) in [(0, 0.3), (1, 0.5)] {
                let ours = mix(kind, &a, &b, W, H, t).unwrap();
                let diff = ours.iter().zip(&got[k * n..(k + 1) * n]).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / n as f64;
                if diff > 4.0 {
                    bad.push(format!("{kind}@{t}: {diff:.1}"));
                }
            }
        }
        assert!(bad.is_empty(), "{bad:?}");
    }
}
