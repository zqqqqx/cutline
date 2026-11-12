@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
  let o = vec2<f32>(0.02 * u.p0.x * sin(u.time * 6.2831 * u.p0.y), 0.0);
  let r = textureSample(src, samp, uv + o).r;
  let g = textureSample(src, samp, uv).g;
  let b = textureSample(src, samp, uv - o).b;
  return vec4<f32>(r, g, b, textureSample(src, samp, uv).a);
}
