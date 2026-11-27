// Based on "DreamyZoom" by Zeh Fernando (GL Transitions, MIT license), ported to WGSL.
// p0 = rotation, p1 = magnification
const DZ_DEG = 0.03926990816987241548078304229099;

fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f {
  let first = progress < 0.5;
  let phase = select((progress - 0.5) * 2.0, progress * 2.0, first);
  let ang = select(mix(-params.p0 * DZ_DEG, 0.0, phase), mix(0.0, params.p0 * DZ_DEG, phase), first);
  let sc = select(mix(params.p1, 1.0, phase), mix(1.0, params.p1, phase), first);
  var p = (uv - vec2f(0.5)) / sc * vec2f(ratio, 1.0);
  let angle = atan2(p.y, p.x) + ang;
  let dist = length(p);
  p = vec2f(cos(angle) * dist / ratio + 0.5, sin(angle) * dist + 0.5);
  var c = to_color(p);
  if (first) { c = from_color(p); }
  // the flash brightens the image, not the transparent areas
  let flash = select(mix(1.0, 0.0, phase), mix(0.0, 1.0, phase), first);
  return vec4f(c.rgb + flash, c.a);
}
