// Reißschwenk: die Kamera reißt von A nach B, mit gerichteter Bewegungsunschärfe.
// Eigenes Werk des Cutline-Projekts, CC0-1.0.
// (p0, p1) = Richtung, in die sich das Bild bewegt (eine Achse, Länge 1)

// A liegt im Einheitsquadrat, B schließt entgegen der Bewegung an
fn wp_pick(w: vec2f, m: vec2f) -> vec4f {
  if (dot(w - 0.5, -m) <= 0.5) { return from_color(clamp(w, vec2f(0.0), vec2f(1.0))); }
  return to_color(clamp(w + m, vec2f(0.0), vec2f(1.0)));
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let m = vec2f(params.p0, params.p1);
  let t = clamp(progress, 0.0, 1.0);
  // schnelles Anreißen und Abbremsen (quartisch), Tempo 0 an beiden Enden
  let e = select(1.0 - pow(2.0 - 2.0 * t, 4.0) / 2.0, 8.0 * t * t * t * t, t < 0.5);
  let speed = select(pow(2.0 - 2.0 * t, 3.0), 8.0 * t * t * t, t < 0.5);
  let w = uv - m * e;
  let spread = 0.3 * speed;
  var col = vec4f(0.0);
  var total = 0.0;
  for (var i = 0; i < 32; i++) {
    let k = f32(i) / 31.0 - 0.5;
    let wt = 1.0 - abs(k);
    col += wp_pick(w + m * spread * k, m) * wt;
    total += wt;
  }
  return col / total;
}
