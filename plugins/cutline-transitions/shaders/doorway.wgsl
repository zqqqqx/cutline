// Based on "Doorway" by gre (GL Transitions, MIT license), ported to WGSL.
// p0 = floor reflection, p1 = perspective of the door wings, p2 = depth image B comes from
const DW_PI = 3.141592653589793;

// ray through pixel x onto the left-hinged wing (width w/2, rotated by a): (t, distance)
fn dw_leaf(x: f32, w: f32, eye: f32, a: f32) -> vec2f {
  let c = cos(a);
  let s = sin(a);
  let t = (eye * c - 0.5 * w * s) / (s * (x - 0.5 * w) + eye * c);
  let hit = vec2f(0.5 * w + t * (x - 0.5 * w), eye * (t - 1.0));
  return vec2f(t, hit.x * c - hit.y * s);
}

fn dw_project(p: vec2f, w: f32, eye: f32) -> f32 {
  return 0.5 * w + (p.x - 0.5 * w) * eye / (eye + p.y);
}

fn dw_ease(t: f32) -> f32 {
  if (t < 0.5) { return 4.0 * t * t * t; }
  let u = 1.0 - t;
  return 1.0 - 4.0 * u * u * u;
}

// the room behind: image B comes forward, with a floor reflection
fn dw_room(uv: vec2f, x: f32, edge_l: f32, edge_r: f32, close: f32, dolly: f32) -> vec4f {
  let size = mix(max(params.p2, 1.0), 1.0, dolly);
  let p = (uv - 0.5) * size + 0.5;
  let soft = 0.004 * (size - 1.0) + 1e-6;
  let m = smoothstep(vec2f(-soft), vec2f(soft), p) * smoothstep(vec2f(-soft), vec2f(soft), 1.0 - p);
  var c = to_color(clamp(p, vec2f(0.0), vec2f(1.0))) * (m.x * m.y);
  if (p.y < 0.0) {
    let r = vec2f(p.x, -1.2 * p.y - 0.02);
    let f = params.p0 * (1.0 - r.y) * m.x * smoothstep(0.0, 0.01, r.y);
    c += to_color(clamp(r, vec2f(0.0), vec2f(1.0))) * f * (1.0 - m.y);
  }
  let wl = 0.03 + 0.12 * close;
  let shadow = (1.0 - smoothstep(0.0, wl, x - edge_l)) + (1.0 - smoothstep(0.0, wl, edge_r - x));
  let light = (1.0 + 0.6 * close * close) * (1.0 - 0.7 * close * min(shadow, 1.0));
  return vec4f(min(c.rgb * light, vec3f(c.a)), c.a);
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let w = ratio;
  let x = uv.x * w;
  let eye = max(w, 1.0) * (0.5 + 0.2 / max(params.p1, 0.01));
  let swing = progress * progress * (3.0 - 2.0 * progress);
  let a = min(atan(2.0 * eye / w) + 0.1, 0.5 * DW_PI) * swing;
  let dolly = mix(dw_ease(progress), progress, 0.3);
  let hw = 0.5 * w;
  let front = hw * vec2f(cos(a), -sin(a));
  let thick = 0.02 * w * (1.0 - smoothstep(0.8, 1.0, swing));
  let edge_l = dw_project(front, w, eye);
  let back_l = dw_project(front + thick * vec2f(sin(a), cos(a)), w, eye);
  let close = 1.0 - swing;
  let back = dw_room(uv, x, back_l, w - back_l, close, dolly);
  let left = x < hw;
  let lx = select(w - x, x, left);
  let hit = dw_leaf(lx, w, eye, a);
  let s = clamp(hit.y, 0.0, hw);
  let v = 0.5 + (uv.y - 0.5) * hit.x;
  var leaf = from_color(clamp(vec2f(select(1.0 - s / w, s / w, left), v), vec2f(0.0), vec2f(1.0)));
  leaf = vec4f(leaf.rgb * (1.0 - 0.5 * (1.0 - cos(a)) - 0.25 * sin(a) * (s / hw)), leaf.a);
  var side = from_color(clamp(vec2f(select(0.501, 0.499, left), v), vec2f(0.0), vec2f(1.0)));
  side = vec4f(min(side.rgb * 1.5, vec3f(side.a)), side.a);
  let aa = 0.003 * min(1.0, 20.0 * min(a, close)) + 1e-6;
  let c = mix(back, side, smoothstep(-aa, aa, back_l - lx));
  return mix(c, leaf, smoothstep(-aa, aa, edge_l - lx));
}
