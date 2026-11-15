// Nach „PolkaDotsCurtain“ von bobylito (GL Transitions, MIT-Lizenz), nach WGSL portiert.
// p0 = Punkte über die Breite
fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  if (progress >= 1.0) { return to_color(uv); }
  let next = distance(fract(uv * params.p0), vec2f(0.5)) < progress / distance(uv, vec2f(0.0));
  return select(from_color(uv), to_color(uv), next);
}
