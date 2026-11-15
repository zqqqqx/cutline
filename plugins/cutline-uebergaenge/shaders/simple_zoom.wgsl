// Nach „SimpleZoom“ von 0gust1 (GL Transitions, MIT-Lizenz), nach WGSL portiert.
// p0 = Anteil des Übergangs, der gezoomt wird
fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let q = clamp(params.p0, 0.2, 1.0);
  let amount = smoothstep(0.0, q, progress);
  let z = 0.5 + (uv - 0.5) * (1.0 - amount);
  return mix(from_color(z), to_color(uv), smoothstep(q - 0.2, 1.0, progress));
}
