// Nach „CrossZoom“ von rectalogic (GL Transitions, MIT-Lizenz), nach WGSL portiert.
// p0 = Stärke der Zoom-Unschärfe
fn cz_rand(co: vec2f) -> f32 {
  return fract(sin(dot(co, vec2f(12.9898, 78.233))) * 43758.5453);
}

fn cz_expo(t: f32) -> f32 {
  if (t <= 0.0) { return 0.0; }
  if (t >= 1.0) { return 1.0; }
  let x = t * 2.0;
  if (x < 1.0) { return 0.5 * pow(2.0, 10.0 * (x - 1.0)); }
  return 0.5 * (2.0 - pow(2.0, -10.0 * (x - 1.0)));
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let center = vec2f(0.25 + 0.5 * progress, 0.5);
  let dissolve = cz_expo(progress);
  // 0 → Stärke → 0
  let strength = -params.p0 / 2.0 * (cos(3.141592653589793 * progress / 0.5) - 1.0);
  let to_center = center - uv;
  let offset = cz_rand(uv);
  var color = vec4f(0.0);
  var total = 0.0;
  for (var t = 0.0; t <= 40.0; t += 1.0) {
    let percent = (t + offset) / 40.0;
    let weight = 4.0 * (percent - percent * percent);
    let q = uv + to_center * percent * strength;
    color += mix(from_color(q), to_color(q), dissolve) * weight;
    total += weight;
  }
  return color / total;
}
