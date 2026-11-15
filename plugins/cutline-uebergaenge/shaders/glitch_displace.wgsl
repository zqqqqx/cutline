// Nach „GlitchDisplace“ von Matt DesLauriers (GL Transitions, MIT-Lizenz), nach WGSL portiert.
fn gd_displace(tex: vec4f, tc: vec2f, dot_depth: f32, tex_depth: f32, strength: f32) -> vec2f {
  let dis = tex * dot_depth + 1.0 - tex * tex_depth;
  let dx = (dis.x - 1.0 + tex_depth * dot_depth) * strength;
  let dy = (dis.y - 1.0 + tex_depth * dot_depth) * strength;
  return tc + vec2f(dx, dy);
}

fn gd_ease1(t: f32) -> f32 {
  if (t <= 0.0 || t >= 1.0) { return clamp(t, 0.0, 1.0); }
  if (t < 0.5) { return 0.5 * pow(2.0, 20.0 * t - 10.0); }
  return -0.5 * pow(2.0, 10.0 - t * 20.0) + 1.0;
}

fn gd_ease2(t: f32) -> f32 {
  if (t >= 1.0) { return 1.0; }
  return 1.0 - pow(2.0, -10.0 * t);
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  var c1 = from_color(uv);
  var c2 = to_color(uv);
  let disp = gd_displace(c1, uv, 0.33, 0.7, 1.0 - gd_ease1(progress));
  let disp2 = gd_displace(c2, uv, 0.33, 0.5, gd_ease2(progress));
  let d1 = to_color(disp);
  var d2 = from_color(disp2);
  let val = gd_ease1(progress);
  let gray = vec3f(dot(min(d2, d1).rgb, vec3f(0.299, 0.587, 0.114)));
  d2 = vec4f(gray * 2.0, d2.a);
  c1 = mix(c1, d2, smoothstep(0.0, 0.5, progress));
  c2 = mix(c2, d1, 1.0 - smoothstep(0.5, 1.0, progress));
  return mix(c1, c2, val);
}
