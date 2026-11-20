// Light leak: a warm streak of light wipes diagonally from left to right, B behind it.
// Original work of the Cutline project, CC0-1.0.
// p0 = brightness of the light
fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let t = clamp(progress, 0.0, 1.0);
  let e = t * t * (3.0 - 2.0 * t);
  let x = uv.x + (uv.y - 0.5) * 0.3;
  let front = mix(-0.45, 1.45, e);
  let m = 1.0 - smoothstep(front - 0.15, front + 0.15, x);
  let c = mix(from_color(uv), to_color(uv), m);
  let env = sqrt(1.0 - abs(2.0 * t - 1.0));
  let d1 = (x - front) / 0.22;
  let d2 = (x - front + 0.18) / 0.35;
  let leak = (vec3f(1.0, 0.62, 0.25) * exp(-d1 * d1) + vec3f(0.95, 0.25, 0.15) * 0.6 * exp(-d2 * d2)) * env * params.p0;
  // "screen" blend: light brightens without burning out
  return vec4f(1.0 - (1.0 - c.rgb) * (1.0 - clamp(leak, vec3f(0.0), vec3f(1.0))), c.a);
}
