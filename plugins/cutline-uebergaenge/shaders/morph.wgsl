// Nach „Morph“ von paniq (GL Transitions, MIT-Lizenz), nach WGSL portiert.
// p0 = Stärke der Verschiebung
fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let ca = from_color(uv);
  let cb = to_color(uv);
  let oa = (ca.rg + ca.b) * 0.5 * 2.0 - 1.0;
  let ob = (cb.rg + cb.b) * 0.5 * 2.0 - 1.0;
  let oc = mix(oa, ob, 0.5) * params.p0;
  return mix(from_color(uv + oc * progress), to_color(uv - oc * (1.0 - progress)), progress);
}
