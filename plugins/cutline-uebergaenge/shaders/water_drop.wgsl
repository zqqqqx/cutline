// Nach „WaterDrop“ von Paweł Płóciennik (GL Transitions, MIT-Lizenz), nach WGSL portiert.
// p0 = Frequenz der Wellen, p1 = Geschwindigkeit, p2 = Lichtbrechung, p3 = Glanzlichter
fn wd_mirror(v: vec2f) -> vec2f {
  let m = v - 2.0 * floor(v / 2.0);
  return 1.0 - abs(1.0 - m);
}

fn wd_wave(s: f32, k: f32, lambda: f32, t: f32) -> f32 {
  let env = smoothstep(-0.3 * lambda, 0.6 * lambda, s) * exp(-s / (3.5 * lambda));
  return env * sin(k * s + params.p1 * t);
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  if (progress <= 0.0) { return from_color(uv); }
  if (progress >= 1.0) { return to_color(uv); }
  let center = vec2f(0.5);
  let k = 2.0 * max(params.p0, 1.0);
  let lambda = 6.2831853 / k;
  let q = (uv - center) * vec2f(ratio, 1.0);
  let r = length(q);
  var dir = vec2f(0.0);
  if (r > 0.0) { dir = q / r; }
  let far = max(center, 1.0 - center) * vec2f(ratio, 1.0);
  let big_r = length(far);
  let edge = 0.35 * lambda;
  let soft = 0.1 * lambda;
  let t = progress;
  let e = mix(t, 1.0 - (1.0 - t) * (1.0 - t), 0.4);
  let front = mix(-0.3 * lambda, big_r + edge + soft + 0.01, e);
  let s = front - r;
  var calm = 1.0 - smoothstep(0.55, 1.0, t);
  calm *= calm;
  let d = 0.02 * lambda;
  let r0 = max(r - d, 0.0);
  let slope = calm * (wd_wave(front - r - d, k, lambda, t) - wd_wave(front - r0, k, lambda, t)) / ((r + d - r0) * k);
  let offset = dir * slope * params.p2 / vec2f(ratio, 1.0);
  let ca = from_color(wd_mirror(uv - offset));
  let cb = to_color(wd_mirror(uv - offset));
  let m = smoothstep(edge - soft, edge + soft, s);
  var c = mix(vec4f(ca.rgb * ca.a, ca.a), vec4f(cb.rgb * cb.a, cb.a), m);
  let h = calm * wd_wave(s, k, lambda, t);
  let lit = -dot(dir, normalize(vec2f(-0.6, 0.8))) * slope;
  let glint = params.p3 * (0.2 * h + 0.6 * smoothstep(0.2, 0.9, lit) - 0.3 * smoothstep(0.2, 0.9, -lit));
  c = vec4f(clamp(c.rgb + glint * c.a, vec3f(0.0), vec3f(c.a)), c.a);
  if (c.a > 0.0) { return vec4f(c.rgb / c.a, c.a); }
  return vec4f(0.0);
}
