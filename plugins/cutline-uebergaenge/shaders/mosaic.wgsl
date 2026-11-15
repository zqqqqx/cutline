// Nach „Mosaic“ von Xaychru (GL Transitions, MIT-Lizenz), nach WGSL portiert.
const MO_PI = 3.14159265358979323;

fn mo_rand(v: vec2f) -> f32 {
  return fract(sin(dot(v, vec2f(12.9898, 78.233))) * 43758.5453);
}

fn mo_rot(v: vec2f, a: f32) -> vec2f {
  return vec2f(cos(a) * v.x + sin(a) * v.y, -sin(a) * v.x + cos(a) * v.y);
}

fn mo_cos(x: f32) -> f32 {
  return -cos(x * MO_PI) / 2.0 + 0.5;
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let p = uv - 0.5;
  let rpr = progress * 2.0 - 1.0;
  let z = -(rpr * rpr * 2.0) + 3.0;
  let ci = mo_cos(progress);
  let rp = p * abs(z) + mix(vec2f(0.5), vec2f(2.5, -0.5), ci * ci);
  var mrp = rp - floor(rp);
  let cell = floor(rp);
  let on_end = cell.x == 2.0 && cell.y == -1.0;
  if (!on_end) {
    let ang = floor(mo_rand(cell) * 4.0) * 0.5 * MO_PI;
    mrp = vec2f(0.5) + mo_rot(mrp - vec2f(0.5), ang);
  }
  if (on_end || mo_rand(cell) > 0.5) { return to_color(mrp); }
  return from_color(mrp);
}
