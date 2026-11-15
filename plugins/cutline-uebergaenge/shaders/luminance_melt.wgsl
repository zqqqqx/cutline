// Nach „LuminanceMelt“ von 0gust1 (GL Transitions, MIT-Lizenz), nach WGSL portiert;
// Simplex-Rauschen von Ian McEwan, Ashima Arts (MIT-Lizenz).
// p0 = Richtung (1 = nach unten), p1 = Helligkeitsschwelle, p2 = helle statt dunkle Stellen schmelzen
fn lm_mod289_3(x: vec3f) -> vec3f {
  return x - floor(x * (1.0 / 289.0)) * 289.0;
}

fn lm_mod289_2(x: vec2f) -> vec2f {
  return x - floor(x * (1.0 / 289.0)) * 289.0;
}

fn lm_permute(x: vec3f) -> vec3f {
  return lm_mod289_3(((x * 34.0) + 1.0) * x);
}

fn lm_snoise(v: vec2f) -> f32 {
  let C = vec4f(0.211324865405187, 0.366025403784439, -0.577350269189626, 0.024390243902439);
  var i = floor(v + dot(v, C.yy));
  let x0 = v - i + dot(i, C.xx);
  var i1 = vec2f(0.0, 1.0);
  if (x0.x > x0.y) { i1 = vec2f(1.0, 0.0); }
  var x12 = x0.xyxy + C.xxzz;
  x12 = vec4f(x12.xy - i1, x12.zw);
  i = lm_mod289_2(i);
  let p = lm_permute(lm_permute(i.y + vec3f(0.0, i1.y, 1.0)) + i.x + vec3f(0.0, i1.x, 1.0));
  var m = max(0.5 - vec3f(dot(x0, x0), dot(x12.xy, x12.xy), dot(x12.zw, x12.zw)), vec3f(0.0));
  m = m * m;
  m = m * m;
  let x = 2.0 * fract(p * C.www) - 1.0;
  let h = abs(x) - 0.5;
  let ox = floor(x + 0.5);
  let a0 = x - ox;
  m *= 1.79284291400159 - 0.85373472095314 * (a0 * a0 + h * h);
  let g = vec3f(a0.x * x0.x + h.x * x0.y, a0.yz * x12.xz + h.yz * x12.yw);
  return 130.0 * dot(m, g);
}

fn lm_rand(co: vec2f) -> f32 {
  return fract(sin(dot(co, vec2f(12.9898, 78.233))) * 43758.5453);
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  if (progress <= 0.0) { return from_color(uv); }
  if (progress >= 1.0) { return to_color(uv); }
  let a = from_color(uv);
  let center = vec2f(1.0, select(0.0, 1.0, params.p0 > 0.5));
  let dist = distance(center, uv) - progress * exp(lm_snoise(vec2f(uv.x, 0.0)));
  let r = progress - lm_rand(vec2f(uv.x, 0.1));
  let l = a.r * 0.299 + a.g * 0.587 + a.b * 0.114;
  let hit = select(l < params.p1, l > params.p1, params.p2 > 0.5);
  var m = progress * progress * progress;
  if (dist <= r && hit) { m = 1.0; }
  return mix(a, to_color(uv), m);
}
