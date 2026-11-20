// Based on "HeartWipe" by gre (GL Transitions, MIT license), ported to WGSL.
// p0 = edge softness, p1 = heartbeat, p2 = glow at the edge
const HW_O = vec2f(0.0, 0.55);
const HW_LOBE = vec2f(0.25, 0.75);
const HW_R = 0.35355339;

fn hw_dot2(v: vec2f) -> f32 {
  return dot(v, v);
}

// exact distance to the heart (tip at (0, 0), notch at (0, 1))
fn hw_sd(p0: vec2f) -> f32 {
  let p = vec2f(abs(p0.x), p0.y);
  if (p.x + p.y > 1.0) { return sqrt(hw_dot2(p - HW_LOBE)) - HW_R; }
  return sqrt(min(hw_dot2(p - vec2f(0.0, 1.0)), hw_dot2(p - 0.5 * max(p.x + p.y, 0.0)))) * sign(p.x - p.y);
}

// size of the smallest heart around HW_O that contains p
fn hw_scale(p: vec2f) -> f32 {
  let l = length(p);
  if (l < 1e-6) { return 0.0; }
  let d = vec2f(abs(p.x), p.y) / l;
  let m = HW_O - HW_LOBE;
  let b = dot(m, d);
  let t_disk = -b + sqrt(b * b - hw_dot2(m) + HW_R * HW_R);
  var t_side = 1e6;
  if (d.x > d.y) { t_side = HW_O.y / (d.x - d.y); }
  var t_top = 1e6;
  if (d.x + d.y > 0.0) { t_top = (1.0 - HW_O.y) / (d.x + d.y); }
  return l / max(t_disk, min(t_side, t_top));
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let a = from_color(uv);
  let b = to_color(uv);
  let asp = vec2f(ratio, 1.0);
  let c = vec2f(0.5) * asp;
  var full = max(
    max(max(hw_scale(vec2f(0.0) - c), hw_scale(vec2f(ratio, 0.0) - c)),
        max(hw_scale(vec2f(0.0, 1.0) - c), hw_scale(vec2f(ratio, 1.0) - c))),
    hw_scale(vec2f(0.5 * ratio, 1.0) - c));
  let sm = max(params.p0, 0.001);
  full += 4.0 * sm;
  let p = progress;
  let e = p * p * (2.0 - p);
  let env = max(sin(3.14159265 * p), 0.0);
  let beat = sin(6.28318531 * 2.0 * p) * env * env;
  let s = full * clamp(e + 0.09 * params.p1 * beat, 0.0, 1.0);
  if (s <= 0.0) { return a; }
  let q = uv * asp - c;
  let d = s * hw_sd(q / s + HW_O);
  let m = smoothstep(0.0, sm, -d);
  let col = mix(a, b, m);
  let light = exp(-max(d, 0.0) / (0.008 + sm)) * (1.0 - m) * params.p2 * min(1.0, 4.0 * env);
  return col + vec4f(vec3f(1.0, 0.85, 0.9) * light, 0.0);
}
