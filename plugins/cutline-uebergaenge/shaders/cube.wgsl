// Nach „Cube“ von gre (GL Transitions, MIT-Lizenz), nach WGSL portiert.
// p0 = Perspektive, p1 = Herauszoomen, p2 = Spiegelung, p3 = Abstand zum Boden (%), p4 = Schattierung
const CB_PI = 3.141592653589793;
const CB_EDGE = 0.002;

// Strahl (eye, dir) gegen den um (c, s) gedrehten Würfel: (Abstand, x auf der Fläche, Höhe, Fläche)
fn cb_hit(eye: vec3f, dir: vec3f, w: f32, c: f32, s: f32) -> vec4f {
  let pivot = vec2f(0.5 * w);
  let e = eye.xz - pivot;
  let o = vec2f(e.x * c + e.y * s, -e.x * s + e.y * c) + pivot;
  let d = vec2f(dir.x * c + dir.z * s, -dir.x * s + dir.z * c);
  let dx = select(max(d.x, 1e-6), min(d.x, -1e-6), d.x < 0.0);
  let dy = select(max(d.y, 1e-6), min(d.y, -1e-6), d.y < 0.0);
  let inv = 1.0 / vec2f(dx, dy);
  let t0 = (vec2f(0.0) - o) * inv;
  let t1 = (vec2f(w) - o) * inv;
  let tn = min(t0, t1);
  let tf = max(t0, t1);
  let tin = max(tn.x, tn.y);
  if (tin <= 0.0 || tin > min(tf.x, tf.y)) { return vec4f(-1.0); }
  let hit = o + tin * d;
  let y = eye.y + tin * dir.y;
  if (y < 0.0 || y > 1.0) { return vec4f(-1.0); }
  if (tn.y >= tn.x) {
    if (d.y <= 0.0) { return vec4f(-1.0); }
    return vec4f(tin, hit.x / w, y, 0.0);
  }
  if (d.x <= 0.0) { return vec4f(-1.0); }
  return vec4f(tin, 1.0 - hit.y / w, y, 1.0);
}

fn cb_img(face: f32, p: vec2f) -> vec4f {
  if (face < 0.5) { return from_color(p); }
  return to_color(p);
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let persp = params.p0;
  let shading = params.p4;
  let w = ratio;
  let turn = progress * progress * (3.0 - 2.0 * progress);
  let a = 0.5 * CB_PI * turn;
  var c = cos(a);
  var s = sin(a);
  if (progress >= 1.0) { c = 0.0; s = 1.0; }
  let motion = sin(CB_PI * clamp(progress, 0.0, 1.0));
  let zoom = 1.0 + max(params.p1, 0.0) * motion;
  let dist = 0.8 * max(w, 1.0) / clamp(persp, 0.01, 1.0);
  let eye = vec3f(0.5 * w, 0.5, -dist);
  let sp = (uv - 0.5) * zoom + 0.5;
  let dir = vec3f(sp.x * w, sp.y, 0.0) - eye;
  let px = CB_EDGE * zoom * min(10.0 * motion, 1.0);
  let shade0 = 1.0 - shading * (1.0 - c);
  let shade1 = 1.0 - shading * (1.0 - s);

  var front = vec4f(0.0);
  let b = cb_hit(eye, dir, w, c, s);
  if (b.x > 0.0) {
    let img = cb_img(b.w, vec2f(b.y, b.z));
    let shade = select(shade1, shade0, b.w < 0.5);
    var m = 1.0;
    if (px > 0.0) {
      let e = px * b.x;
      let n = select(vec3f(-c, 0.0, -s), vec3f(s, 0.0, -c), b.w < 0.5);
      let fore = max(abs(dot(n, normalize(dir))), 0.05);
      let edge = select(b.y, 1.0 - b.y, b.w < 0.5);
      m = clamp(min(b.z, 1.0 - b.z) / e, 0.0, 1.0) * clamp(edge * w * fore / e, 0.0, 1.0);
    }
    if (m >= 1.0 && img.a >= 1.0) { return vec4f(img.rgb * shade, img.a); }
    front = vec4f(img.rgb * shade * img.a, img.a) * m;
  }

  // Boden mit Spiegelung und Schatten, blendet mit der Bewegung ein und aus
  var ground = vec4f(0.0);
  let h = 0.005 * max(params.p3, 0.0);
  if (dir.y < 0.0 && motion > 0.0) {
    let tf = -(eye.y + h) / dir.y;
    let f = eye + tf * dir;
    let meye = vec3f(eye.x, -2.0 * h - eye.y, eye.z);
    let mdir = vec3f(dir.x, -dir.y, dir.z);
    let r = cb_hit(meye, mdir, w, c, s);
    if (r.x > 0.0) {
      let img = cb_img(r.w, vec2f(r.y, r.z));
      let k = clamp(params.p2, 0.0, 1.0) * (1.0 - r.z) * (1.0 - r.z);
      ground = vec4f(img.rgb * select(shade1, shade0, r.w < 0.5) * img.a, img.a) * k;
    }
    let q = f.xz - vec2f(0.5 * w);
    let l = vec2f(q.x * c + q.y * s, -q.x * s + q.y * c);
    let soft = 0.03 + 1.5 * h;
    var sh = shading * 0.7 * exp(-length(max(abs(l) - vec2f(0.5 * w), vec2f(0.0))) / soft);
    let bl = l + vec2f(0.5 * w);
    if (l.y < l.x) {
      sh *= from_color(vec2f(clamp(bl.x / w, 0.0, 1.0), 0.0)).a;
    } else {
      sh *= to_color(vec2f(clamp(1.0 - bl.y / w, 0.0, 1.0), 0.0)).a;
    }
    ground = ground * (1.0 - sh) + vec4f(0.0, 0.0, 0.0, sh);
    ground *= min(6.0 * motion, 1.0);
  }
  let color = front + ground * (1.0 - front.a);
  if (color.a > 0.0) { return vec4f(color.rgb / color.a, color.a); }
  return vec4f(0.0);
}
