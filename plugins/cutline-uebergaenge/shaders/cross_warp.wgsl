// Nach „CrossWarp“ von Eke Péter (GL Transitions, MIT-Lizenz), nach WGSL portiert.
fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let x = smoothstep(0.0, 1.0, progress * 2.0 + uv.x - 1.0);
  return mix(from_color((uv - 0.5) * (1.0 - x) + 0.5), to_color((uv - 0.5) * x + 0.5), x);
}
