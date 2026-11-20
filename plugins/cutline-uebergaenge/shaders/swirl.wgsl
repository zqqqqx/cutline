// Based on "Swirl" by Sergey Kosarevsky (GL Transitions, MIT license), ported to WGSL.
// p0 = turns in the center, p1 = radius in image heights
const SW_PI = 3.141592653589793;
const SW_TAU = 6.283185307179586;

fn sw_mirror(v: vec2f) -> vec2f {
  let m = v - 2.0 * floor(v / 2.0);
  return mix(m, 2.0 - m, step(vec2f(1.0), m));
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let s = sin(SW_PI * progress);
  let amount = s * s;
  let speed = SW_PI * sin(SW_TAU * progress);
  let reveal = smoothstep(0.3, 0.7, progress);
  let aspect = vec2f(ratio, 1.0);
  let center = vec2f(0.5);
  var d = (uv - center) * aspect;
  let x = clamp(length(d) / max(params.p1, 1e-3), 0.0, 1.0);
  let falloff = (1.0 - x) * (1.0 - x);
  let twist = params.p0 * SW_TAU * falloff;
  let angle = twist * amount;
  let spread = clamp(twist * speed * 0.05, -1.2, 1.2);
  d *= 1.0 - 0.12 * amount * falloff;
  var col = vec4f(0.0);
  var total = 0.0;
  for (var i = 0; i < 12; i++) {
    let k = f32(i) / 11.0 - 0.5;
    let w = 1.0 - abs(k);
    let a = angle + spread * k;
    let c = cos(a);
    let sn = sin(a);
    let q = sw_mirror(center + vec2f(c * d.x - sn * d.y, sn * d.x + c * d.y) / aspect);
    col += mix(from_color(q), to_color(q), reveal) * w;
    total += w;
  }
  return col / total;
}
