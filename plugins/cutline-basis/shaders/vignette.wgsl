@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
  let c = textureSample(src, samp, uv);
  let d = distance(uv, vec2<f32>(0.5));
  let v = smoothstep(0.8, 0.2, d * (1.0 + u.p0.x));
  return vec4<f32>(mix(u.p1.rgb, c.rgb, v), c.a);
}
