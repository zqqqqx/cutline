// Zoom punch: A zooms in (or out) with radial blur, B comes in magnified and
// zooms back to normal size (never shrunk: no empty edges on B).
// Original work of the Cutline project, CC0-1.0.
// p0 = direction (1 = in, -1 = out), p1 = strength of the blur, p2 = zoom factor
fn zp_mirror(v: vec2f) -> vec2f {
  let m = v - 2.0 * floor(v / 2.0);
  return 1.0 - abs(1.0 - m);
}

// image at logarithmic magnification z, blur over the distance v (backwards)
fn zp_sample(uv: vec2f, z: f32, v: f32, b_side: bool) -> vec4f {
  var col = vec4f(0.0);
  var total = 0.0;
  for (var i = 0; i < 20; i++) {
    let k = f32(i) / 19.0;
    let wt = 1.0 - 0.7 * k;
    let q = zp_mirror(0.5 + (uv - 0.5) * exp(-(z - v * k)));
    col += select(from_color(q), to_color(q), b_side) * wt;
    total += wt;
  }
  return col / total;
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let t = clamp(progress, 0.0, 1.0);
  let dir = select(-1.0, 1.0, params.p0 >= 0.0);
  let l = log(max(params.p2, 1.01));
  let sa = min(2.0 * t, 1.0);
  let sb = max(2.0 * t - 1.0, 0.0);
  // A speeds up until the middle, B slows down after it
  let za = dir * l * sa * sa * sa;
  let zb = l * (1.0 - sb) * (1.0 - sb) * (1.0 - sb);
  let va = dir * l * params.p1 * sa * sa;
  let vb = -l * params.p1 * (1.0 - sb) * (1.0 - sb);
  let m = smoothstep(0.47, 0.53, t);
  if (m <= 0.0) { return zp_sample(uv, za, va, false); }
  if (m >= 1.0) { return zp_sample(uv, zb, vb, true); }
  return mix(zp_sample(uv, za, va, false), zp_sample(uv, zb, vb, true), m);
}
