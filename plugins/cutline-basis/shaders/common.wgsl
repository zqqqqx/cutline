// Gemeinsame Schnittstelle aller Cutline-Shader (Effekt-Host, Phase 2).
// Eingänge: src (Clip, linear RGB f16), dst (zweiter Clip bei Übergängen),
// u.time (Sekunden im Clip), u.progress (0..1 bei Übergängen), u.p0..p3 (Parameter).
struct Uniforms { time: f32, progress: f32, resolution: vec2<f32>, p0: vec4<f32>, p1: vec4<f32> };
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
