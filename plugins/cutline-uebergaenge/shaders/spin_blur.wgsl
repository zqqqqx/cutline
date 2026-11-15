// Dreh-Unschärfe: das Bild dreht sich einmal um die Mitte, mit Drehunschärfe; in der schnellsten
// Phase wechselt es zu B. Eigenes Werk des Cutline-Projekts, CC0-1.0.
// p0 = Stärke der Unschärfe
fn sb_mirror(v: vec2f) -> vec2f {
  let m = v - 2.0 * floor(v / 2.0);
  return 1.0 - abs(1.0 - m);
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let t = clamp(progress, 0.0, 1.0);
  let e = select(1.0 - pow(2.0 - 2.0 * t, 3.0) / 2.0, 4.0 * t * t * t, t < 0.5);
  let speed = select(pow(2.0 - 2.0 * t, 2.0), 4.0 * t * t, t < 0.5);
  let angle = 6.283185307 * e;
  // etwas hineinzoomen, damit beim Drehen keine Ecken leer bleiben
  let zoom = 1.0 + 0.4 * (1.0 - abs(2.0 * t - 1.0));
  let aspect = vec2f(ratio, 1.0);
  let d = (uv - 0.5) * aspect / zoom;
  let spread = params.p0 * speed;
  let reveal = smoothstep(0.45, 0.55, t);
  var col = vec4f(0.0);
  var total = 0.0;
  for (var i = 0; i < 24; i++) {
    let k = f32(i) / 23.0 - 0.5;
    let wt = 1.0 - abs(k);
    let a = angle + spread * k;
    let c = cos(a);
    let s = sin(a);
    let q = sb_mirror(0.5 + vec2f(c * d.x - s * d.y, s * d.x + c * d.y) / aspect);
    col += mix(from_color(q), to_color(q), reveal) * wt;
    total += wt;
  }
  return col / total;
}
