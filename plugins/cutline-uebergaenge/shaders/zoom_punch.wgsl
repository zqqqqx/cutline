// Zoom-Stoß: A zoomt mit radialer Unschärfe hinein (bzw. heraus), B kommt vergrößert herein und
// zoomt auf normale Größe zurück (nie verkleinert: keine leeren Ränder bei B).
// Eigenes Werk des Cutline-Projekts, CC0-1.0.
// p0 = Richtung (1 = hinein, -1 = heraus), p1 = Stärke der Unschärfe, p2 = Zoomfaktor
fn zp_mirror(v: vec2f) -> vec2f {
  let m = v - 2.0 * floor(v / 2.0);
  return 1.0 - abs(1.0 - m);
}

// Bild bei logarithmischer Vergrößerung z, Unschärfe über die Strecke v (nach hinten)
fn zp_sample(uv: vec2f, z: f32, v: f32, b_side: bool) -> vec4f {
  var col = vec4f(0.0);
  var total = 0.0;
  for (var i = 0; i < 20; i++) {
    let k = f32(i) / 19.0;
    let wt = 1.0 - 0.7 * k;
    let q = zp_mirror(0.5 + (uv - 0.5) * exp(-(z - v * k)));
    col += select(from_color(q), to_color(q), b_side) * wt;
    total += wt;
  }
  return col / total;
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let t = clamp(progress, 0.0, 1.0);
  let dir = select(-1.0, 1.0, params.p0 >= 0.0);
  let l = log(max(params.p2, 1.01));
  let sa = min(2.0 * t, 1.0);
  let sb = max(2.0 * t - 1.0, 0.0);
  // A beschleunigt bis zur Mitte, B bremst danach ab
  let za = dir * l * sa * sa * sa;
  let zb = l * (1.0 - sb) * (1.0 - sb) * (1.0 - sb);
  let va = dir * l * params.p1 * sa * sa;
  let vb = -l * params.p1 * (1.0 - sb) * (1.0 - sb);
  let m = smoothstep(0.47, 0.53, t);
  if (m <= 0.0) { return zp_sample(uv, za, va, false); }
  if (m >= 1.0) { return zp_sample(uv, zb, vb, true); }
  return mix(zp_sample(uv, za, va, false), zp_sample(uv, zb, vb, true), m);
}
