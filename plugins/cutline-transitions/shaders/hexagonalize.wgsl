// Based on "Hexagonalize" by Fernando Kuteken (GL Transitions, MIT license), ported to WGSL.
// p0 = steps, p1 = hexagons across the width
fn hx_round(q: f32, r: f32) -> vec2f {
  let s = -q - r;
  var rq = floor(q + 0.5);
  var rr = floor(r + 0.5);
  let rs = floor(s + 0.5);
  let dq = abs(rq - q);
  let dr = abs(rr - r);
  let ds = abs(rs - s);
  if (dq > dr && dq > ds) {
    rq = -rr - rs;
  } else if (dr > ds) {
    rr = -rq - rs;
  }
  return vec2f(rq, rr);
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  var dist = 2.0 * min(progress, 1.0 - progress);
  if (params.p0 > 0.0) { dist = ceil(dist * params.p0) / params.p0; }
  let size = (sqrt(3.0) / 3.0) * dist / params.p1;
  var point = uv;
  if (dist > 0.0) {
    var pt = uv;
    pt.y /= ratio;
    pt = (pt - 0.5) / size;
    let h = hx_round((sqrt(3.0) / 3.0) * pt.x - pt.y / 3.0, 2.0 / 3.0 * pt.y);
    let x = (sqrt(3.0) * h.x + (sqrt(3.0) / 2.0) * h.y) * size + 0.5;
    let y = 1.5 * h.y * size + 0.5;
    point = vec2f(x, y * ratio);
  }
  return mix(from_color(point), to_color(point), progress);
}
