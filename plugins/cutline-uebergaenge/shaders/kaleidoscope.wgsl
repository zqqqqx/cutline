// Nach „Kaleidoscope“ von nwoeanhinnogaehr (GL Transitions, MIT-Lizenz), nach WGSL portiert.
// p0 = Geschwindigkeit, p1 = Winkel, p2 = Verlauf
fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  var p = uv - 0.5;
  var t = pow(progress, params.p2) * params.p0;
  for (var i = 0; i < 7; i++) {
    p = vec2f(sin(t) * p.x + cos(t) * p.y, sin(t) * p.y - cos(t) * p.x);
    t += params.p1;
    let m = p - 2.0 * floor(p / 2.0);
    p = abs(m - 1.0);
  }
  return mix(
    mix(from_color(uv), to_color(uv), progress),
    mix(from_color(p), to_color(p), progress),
    1.0 - 2.0 * abs(progress - 0.5)
  );
}
