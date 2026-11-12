@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
  let c = textureSample(src, samp, uv);
  let warm = c.rgb * vec3<f32>(1.08, 1.0, 0.88) + vec3<f32>(0.02, 0.01, 0.0);
  return vec4<f32>(mix(c.rgb, warm, u.p0.x), c.a);
}
