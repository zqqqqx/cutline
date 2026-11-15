// Nach „WindowSlices“ von gre (GL Transitions, MIT-Lizenz), nach WGSL portiert.
// p0 = Anzahl der Streifen, p1 = Weichheit
fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let sm = params.p1;
  let pr = smoothstep(-sm, 0.0, uv.x - progress * (1.0 + sm));
  let s = step(pr, fract(params.p0 * uv.x));
  return mix(from_color(uv), to_color(uv), s);
}
