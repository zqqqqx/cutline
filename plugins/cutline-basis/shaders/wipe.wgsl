@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
  let soft = max(u.p0.y, 0.0001);
  let edge = smoothstep(u.progress - soft, u.progress + soft, uv.x * (1.0 + soft * 2.0) - soft);
  return mix(textureSample(dst, samp, uv), textureSample(src, samp, uv), edge);
}
