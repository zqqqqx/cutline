// RGB-Glitch: Zeilenblöcke springen seitlich, die Farbkanäle laufen auseinander, Zeile für Zeile
// wechselt das Bild. Eigenes Werk des Cutline-Projekts, CC0-1.0.
// p0 = Stärke
fn rg_hash(n: f32) -> f32 {
  return fract(sin(n * 91.3458) * 47453.5453);
}

fn rg_pick(p: vec2f, b: bool) -> vec4f {
  return select(from_color(p), to_color(p), b);
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let t = clamp(progress, 0.0, 1.0);
  let env = params.p0 * pow(1.0 - abs(2.0 * t - 1.0), 1.5);
  // Blöcke wechseln etwa 30-mal je Übergang
  let frame = floor(t * 30.0);
  let row = floor(uv.y * 14.0 + rg_hash(frame) * 3.0);
  let r = rg_hash(row * 7.13 + frame * 13.7);
  var shift = 0.0;
  if (r > 1.0 - 0.6 * env) { shift = (rg_hash(row + frame * 3.1) - 0.5) * 0.3 * env; }
  let use_b = t > 0.5 + (rg_hash(row * 1.7 + 3.1) - 0.5) * 0.3;
  let q = vec2f(uv.x + shift, uv.y);
  let off = vec2f(0.04 * env, 0.0);
  let g = rg_pick(q, use_b);
  return vec4f(rg_pick(q + off, use_b).r, g.g, rg_pick(q - off, use_b).b, g.a);
}
