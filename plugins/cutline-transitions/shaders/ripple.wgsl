// Based on "Ripple" by gre (GL Transitions, MIT license), ported to WGSL.
// p0 = wave frequency, p1 = speed
fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let dir = uv - vec2f(0.5);
  let dist = length(dir);
  let offset = dir * (sin(progress * dist * params.p0 - progress * params.p1) + 0.5) / 30.0 * progress;
  return mix(from_color(uv + offset), to_color(uv), smoothstep(0.2, 1.0, progress));
}
