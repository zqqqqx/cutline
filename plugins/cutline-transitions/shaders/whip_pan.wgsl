// Whip pan: the camera whips from A to B, with directional motion blur.
// Original work of the Cutline project, CC0-1.0.
// (p0, p1) = direction the image moves in (one axis, length 1)

// A lies in the unit square, B adjoins it against the motion
fn wp_pick(w: vec2f, m: vec2f) -> vec4f {
  if (dot(w - 0.5, -m) <= 0.5) { return from_color(clamp(w, vec2f(0.0), vec2f(1.0))); }
  return to_color(clamp(w + m, vec2f(0.0), vec2f(1.0)));
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let m = vec2f(params.p0, params.p1);
  let t = clamp(progress, 0.0, 1.0);
  // fast start and stop (quartic), speed 0 at both ends
  let e = select(1.0 - pow(2.0 - 2.0 * t, 4.0) / 2.0, 8.0 * t * t * t * t, t < 0.5);
  let speed = select(pow(2.0 - 2.0 * t, 3.0), 8.0 * t * t * t, t < 0.5);
  let w = uv - m * e;
  let spread = 0.3 * speed;
  var col = vec4f(0.0);
  var total = 0.0;
  for (var i = 0; i < 32; i++) {
    let k = f32(i) / 31.0 - 0.5;
    let wt = 1.0 - abs(k);
    col += wp_pick(w + m * spread * k, m) * wt;
    total += wt;
  }
  return col / total;
}
