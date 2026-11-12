@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
  return mix(textureSample(src, samp, uv), textureSample(dst, samp, uv), u.progress);
}
