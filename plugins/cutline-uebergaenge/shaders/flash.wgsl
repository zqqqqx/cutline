// Flash: A is overexposed to white, B emerges from it.
// Original work of the Cutline project, CC0-1.0.
// p0 = strength of the flash
fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let t = clamp(progress, 0.0, 1.0);
  let f = params.p0 * pow(1.0 - abs(2.0 * t - 1.0), 2.5);
  let c = mix(from_color(uv), to_color(uv), smoothstep(0.42, 0.58, t));
  // like overexposure: bright areas turn white first
  return vec4f(1.0 - (1.0 - c.rgb) * (1.0 - f), c.a);
}
