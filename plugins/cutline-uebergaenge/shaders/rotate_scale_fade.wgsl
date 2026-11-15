// Nach „RotateScaleFade“ von Fernando Kuteken (GL Transitions, MIT-Lizenz), nach WGSL portiert;
// Drehung im Seitenverhältnis des Bilds (nicht verzerrt).
// p0 = Umdrehungen, p1 = größte Verkleinerung
fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let aspect = vec2f(ratio, 1.0);
  let d = (uv - vec2f(0.5)) * aspect;
  let angle = 6.283185307 * params.p0 * progress;
  let c = cos(angle);
  let s = sin(angle);
  let scale = mix(params.p1, 1.0, 2.0 * abs(progress - 0.5));
  let ruv = vec2f(0.5) + vec2f(d.x * c - d.y * s, d.x * s + d.y * c) / aspect / scale;
  if (ruv.x < 0.0 || ruv.x > 1.0 || ruv.y < 0.0 || ruv.y > 1.0) {
    return vec4f(0.15, 0.15, 0.15, 1.0);
  }
  return mix(from_color(ruv), to_color(ruv), progress);
}
