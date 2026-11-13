//! Maske eines Effekt-/Filter-Elements in der Vorschau: Form als konvexe Polygone im
//! Bildschirm, zugeschnitten auf das Clip-Rechteck, und als Textur-Fläche gezeichnet.
//! Gleiche Geometrie wie `cl_fx::mask_expr` (Mitte und Größe als Anteil der Bildgröße,
//! Drehung um die Mitte). Weiche Kanten zeigt nur der Export.

use cl_core::{Mask, MaskShape};
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect};

/// Konvexe Teilflächen der Maske im Bildschirm. `true` = umgekehrt für Kreis/Rechteck:
/// dann gilt „alles außer diesen Flächen“.
pub fn polygons(m: &Mask, canvas: Rect) -> (Vec<Vec<Pos2>>, bool) {
    let (cw, ch) = (canvas.width(), canvas.height());
    let c = canvas.min + vec2(m.cx * cw, m.cy * ch);
    let (s, co) = m.rotation_deg.to_radians().sin_cos();
    let to_screen = |u: f32, v: f32| c + vec2(u * co - v * s, u * s + v * co);
    let big = 4.0 * (cw + ch);
    let quad = |u0: f32, u1: f32, v0: f32, v1: f32| {
        vec![
            to_screen(u0, v0),
            to_screen(u1, v0),
            to_screen(u1, v1),
            to_screen(u0, v1),
        ]
    };
    match m.shape {
        MaskShape::Linear => {
            let p = if m.invert {
                quad(-big, big, 0.0, big)
            } else {
                quad(-big, big, -big, 0.0)
            };
            (vec![p], false)
        }
        MaskShape::Mirror => {
            let hh = m.h * ch / 2.0;
            if m.invert {
                (
                    vec![quad(-big, big, -big, -hh), quad(-big, big, hh, big)],
                    false,
                )
            } else {
                (vec![quad(-big, big, -hh, hh)], false)
            }
        }
        MaskShape::Rect => {
            let (a, b) = (m.w * cw / 2.0, m.h * ch / 2.0);
            (vec![quad(-a, a, -b, b)], m.invert)
        }
        MaskShape::Circle => {
            let (a, b) = (m.w * cw / 2.0, m.h * ch / 2.0);
            let pts = (0..48)
                .map(|i| {
                    let t = i as f32 / 48.0 * std::f32::consts::TAU;
                    to_screen(a * t.cos(), b * t.sin())
                })
                .collect();
            (vec![pts], m.invert)
        }
    }
}

fn cross(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

/// Konvexes Polygon `poly` auf das konvexe Polygon `clip` zuschneiden (Sutherland–Hodgman).
pub fn clip_convex(poly: &[Pos2], clip: &[Pos2]) -> Vec<Pos2> {
    if clip.len() < 3 {
        return Vec::new();
    }
    // Umlaufsinn des Zuschnitts bestimmen
    let area: f32 = (0..clip.len())
        .map(|i| {
            let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut out: Vec<Pos2> = poly.to_vec();
    for i in 0..clip.len() {
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let input = std::mem::take(&mut out);
        if input.is_empty() {
            break;
        }
        let inside = |p: Pos2| cross(a, b, p) * sign >= 0.0;
        for j in 0..input.len() {
            let (p, q) = (input[j], input[(j + 1) % input.len()]);
            let (ip, iq) = (inside(p), inside(q));
            if ip {
                out.push(p);
            }
            if ip != iq {
                let (dp, dq) = (cross(a, b, p), cross(a, b, q));
                let t = dp / (dp - dq);
                out.push(p + (q - p) * t);
            }
        }
    }
    out
}

/// Textur `tex` (auf dem Clip-Viereck `corners`: oben links, oben rechts, unten rechts,
/// unten links) nur innerhalb der Fläche `poly` zeichnen.
pub fn draw_textured(
    painter: &egui::Painter,
    tex: egui::TextureId,
    corners: [Pos2; 4],
    poly: &[Pos2],
    alpha: f32,
) {
    let part = clip_convex(poly, &corners);
    if part.len() < 3 {
        return;
    }
    // Bildschirm → UV über die Kanten des (gedrehten) Clip-Rechtecks
    let o = corners[0];
    let ex = corners[1] - o;
    let ey = corners[3] - o;
    let det = ex.x * ey.y - ex.y * ey.x;
    if det.abs() < 1e-6 {
        return;
    }
    let uv = |p: Pos2| {
        let d = p - o;
        pos2(
            (d.x * ey.y - d.y * ey.x) / det,
            (ex.x * d.y - ex.y * d.x) / det,
        )
    };
    let col = Color32::from_white_alpha((alpha.clamp(0.0, 1.0) * 255.0) as u8);
    let mut mesh = egui::Mesh::with_texture(tex);
    for p in &part {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: *p,
            uv: uv(*p),
            color: col,
        });
    }
    for i in 1..part.len() as u32 - 1 {
        mesh.indices.extend([0, i, i + 1]);
    }
    painter.add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_mask_is_cut_to_the_clip() {
        let canvas = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
        let m = Mask {
            cx: 0.25,
            cy: 0.5,
            w: 0.5,
            h: 1.0,
            feather: 0.0,
            ..Mask::new(MaskShape::Rect)
        };
        let (polys, inv) = polygons(&m, canvas);
        assert!(!inv);
        // Clip deckt nur die rechte Hälfte ab 30 px
        let clip = [
            pos2(30.0, 0.0),
            pos2(100.0, 0.0),
            pos2(100.0, 100.0),
            pos2(30.0, 100.0),
        ];
        let part = clip_convex(&polys[0], &clip);
        let xs: Vec<f32> = part.iter().map(|p| p.x).collect();
        assert!(xs.iter().all(|x| (29.9..=50.1).contains(x)), "{xs:?}");
    }
}
