// Nach „DirectionalWarp“ von pschroen (GL Transitions, MIT-Lizenz), nach WGSL portiert.
// p0 = Weichheit der Kante
fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  var v = normalize(vec2f(-1.0, 1.0));
  v /= abs(v.x) + abs(v.y);
  let d = v.x * 0.5 + v.y * 0.5;
  let sm = params.p0;
  let m = 1.0 - smoothstep(-sm, 0.0, v.x * uv.x + v.y * uv.y - (d - 0.5 + progress * (1.0 + sm)));
  return mix(from_color((uv - 0.5) * (1.0 - m) + 0.5), to_color((uv - 0.5) * m + 0.5), m);
}
