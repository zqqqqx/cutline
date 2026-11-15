// Nach „Ripple“ von gre (GL Transitions, MIT-Lizenz), nach WGSL portiert.
// p0 = Frequenz der Wellen, p1 = Geschwindigkeit
fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let dir = uv - vec2f(0.5);
  let dist = length(dir);
  let offset = dir * (sin(progress * dist * params.p0 - progress * params.p1) + 0.5) / 30.0 * progress;
  return mix(from_color(uv + offset), to_color(uv), smoothstep(0.2, 1.0, progress));
}
