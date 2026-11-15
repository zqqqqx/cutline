// Nach „FilmBurn“ von Anastasia Dunbar (GL Transitions, MIT-Lizenz), nach WGSL portiert.
// p0 = Zufallswert (Form der Lichtflecken)
const FB_PI = 3.14159265358979323;

fn fb_sigmoid(x: f32, a: f32) -> f32 {
  if (x > 0.5) { return 1.0 - pow(max(2.0 - x * 2.0, 0.0), a) / 2.0; }
  return pow(max(x * 2.0, 0.0), a) / 2.0;
}

fn fb_rand1(co: f32) -> f32 {
  return fract(sin(co * 24.9898 + params.p0) * 43758.5453);
}

fn fb_rand2(co: vec2f) -> f32 {
  return fract(sin(dot(co, vec2f(12.9898, 78.233))) * 43758.5453);
}

fn fb_apow(a: f32, b: f32) -> f32 {
  return pow(abs(a), b) * sign(b);
}

fn fb_smix(a: f32, b: f32, c: f32) -> f32 {
  return mix(a, b, fb_sigmoid(c, 2.0));
}

fn fb_random(co0: vec2f, shft: f32) -> f32 {
  let co = co0 + 10.0;
  let a = fract(sin(dot(co, vec2f(12.9898 + floor(shft) * 0.5, 78.233 + params.p0))) * 43758.5453);
  let b = fract(sin(dot(co, vec2f(12.9898 + floor(shft + 1.0) * 0.5, 78.233 + params.p0))) * 43758.5453);
  return fb_smix(a, b, fract(shft));
}

fn fb_srandom(co: vec2f, shft: f32) -> f32 {
  let f = floor(co);
  return fb_smix(
    fb_smix(fb_random(f, shft), fb_random(f + vec2f(1.0, 0.0), shft), fract(co.x)),
    fb_smix(fb_random(f + vec2f(0.0, 1.0), shft), fb_random(f + vec2f(1.0, 1.0), shft), fract(co.x)),
    fract(co.y)
  );
}

fn fb_blend(p: vec2f) -> vec4f {
  return mix(from_color(p), to_color(p), fb_sigmoid(params.progress, 10.0));
}

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  var f = vec3f(0.0);
  for (var i = 0.0; i < 13.0; i += 1.0) {
    f += vec3f(sin(uv.x * fb_rand1(i) * 6.0 + progress * 8.0 + fb_rand1(i + 1.43))
      * sin(uv.y * fb_rand1(i + 4.4) * 6.0 + progress * 6.0 + fb_rand1(i + 2.4)));
    let spot = vec2f(fb_srandom(vec2f(progress * 1.3), i + 1.0), fb_srandom(vec2f(progress * 0.5), i + 6.25));
    f += vec3f(1.0 - clamp(length(uv - spot) * mix(20.0, 70.0, fb_rand1(i)), 0.0, 1.0));
  }
  f += 4.0;
  f /= 11.0;
  let fc = f * vec3f(1.0, 0.7, 0.6);
  let fe = vec3f(1.0, 2.0 - sin(progress * FB_PI), 1.3);
  f = vec3f(fb_apow(fc.r, fe.r), fb_apow(fc.g, fe.g), fb_apow(fc.b, fe.b));
  f *= sin(progress * FB_PI);

  var p = uv - 0.5;
  p *= 1.0 + fb_srandom(vec2f(progress * 5.0), 6.3) * sin(progress * FB_PI) * 0.05;
  p += 0.5;

  var blurred = vec4f(0.0);
  let blur = sin(progress * FB_PI) * 0.03;
  for (var i = 0.0; i < 50.0; i += 1.0) {
    let a = degrees(i / 50.0 * 360.0);
    let q = vec2f(cos(a), sin(a)) * (fb_rand2(vec2f(i, p.x + p.y)) + blur);
    blurred += fb_blend(p + q * blur);
  }
  blurred /= 50.0;
  return blurred + vec4f(f, 0.0);
}
