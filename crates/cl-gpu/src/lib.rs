//! Transitions as WGSL shaders, rendered on the GPU (headless via wgpu).
//!
//! A transition shader contains only the function
//! `fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f` and reads the two images
//! via `from_color(uv)` and `to_color(uv)`. `uv` runs like in GL Transitions from (0, 0) bottom
//! left to (1, 1) top right, `ratio` = width/height of the output. Parameters are available as
//! `params.p0` … `params.p7` (default values from the manifest). This crate provides the wrapper
//! (uniforms, sampler, full-screen triangle).
//!
//! Blending happens in gamma-encoded RGB like FFmpeg `xfade` (textures without sRGB conversion)
//! so start and end join exactly to the frames before and after.
//!
//! Without a GPU adapter (or with `CUTLINE_NO_GPU`) [`gpu`] returns `None`; callers then fall
//! back to their `xfade` fallback. Driver errors become `Err`, never a panic.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Number of shader parameters (`params.p0` … `params.p7`).
pub const PARAMS: usize = 8;

/// Wrapper around the shader of a transition.
const PRELUDE: &str = "struct ClParams { progress: f32, ratio: f32, pad0: f32, pad1: f32, p0: f32, p1: f32, p2: f32, p3: f32, p4: f32, p5: f32, p6: f32, p7: f32 };
@group(0) @binding(0) var<uniform> params: ClParams;
@group(0) @binding(1) var cl_from: texture_2d<f32>;
@group(0) @binding(2) var cl_to: texture_2d<f32>;
@group(0) @binding(3) var cl_samp: sampler;
fn from_color(uv: vec2f) -> vec4f { return textureSampleLevel(cl_from, cl_samp, vec2f(uv.x, 1.0 - uv.y), 0.0); }
fn to_color(uv: vec2f) -> vec4f { return textureSampleLevel(cl_to, cl_samp, vec2f(uv.x, 1.0 - uv.y), 0.0); }
struct ClVertex { @builtin(position) pos: vec4f, @location(0) uv: vec2f };
@vertex fn cl_vs(@builtin(vertex_index) i: u32) -> ClVertex {
  let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
  var o: ClVertex;
  o.pos = vec4f(p, 0.0, 1.0);
  o.uv = p * 0.5 + 0.5;
  return o;
}
@fragment fn cl_fs(v: ClVertex) -> @location(0) vec4f {
  return clamp(transition(v.uv, params.progress, params.ratio), vec4f(0.0), vec4f(1.0));
}
";

/// Complete WGSL code (wrapper + shader).
pub fn wrap(shader: &str) -> String {
    format!("{PRELUDE}\n{shader}\n")
}

/// Checks a transition shader without GPU (naga: parse and validate).
pub fn validate(shader: &str) -> Result<(), String> {
    let src = wrap(shader);
    let module = naga::front::wgsl::parse_str(&src).map_err(|e| e.emit_to_string(&src))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .map_err(|e| e.emit_to_string(&src))?;
    Ok(())
}

/// An image as RGBA (8 bits per channel, rows top to bottom, without row padding).
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub rgba: &'a [u8],
    pub width: u32,
    pub height: u32,
}

impl Frame<'_> {
    fn check(&self, max: u32) -> Result<(), String> {
        if self.width == 0 || self.height == 0 || self.width > max || self.height > max {
            return Err(format!("Frame size {}x{} not possible", self.width, self.height));
        }
        if self.rgba.len() != self.width as usize * self.height as usize * 4 {
            return Err("Frame data does not match the size".into());
        }
        Ok(())
    }
}

struct Tex {
    size: (u32, u32),
    tex: wgpu::Texture,
    view: wgpu::TextureView,
}

struct State {
    /// per shader source: finished pipeline or error message (don't rebuild for every frame)
    pipelines: HashMap<String, Result<wgpu::RenderPipeline, String>>,
    from: Option<Tex>,
    to: Option<Tex>,
    out: Option<(Tex, wgpu::Buffer)>,
    uniform: wgpu::Buffer,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
}

/// Headless device (once per process).
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    state: Mutex<State>,
    /// Name of the adapter (for messages and measurements)
    pub adapter: String,
    max_size: u32,
}

static GPU: OnceLock<Option<Gpu>> = OnceLock::new();

/// The device, if a GPU (or a software adapter) is present.
pub fn gpu() -> Option<&'static Gpu> {
    GPU.get_or_init(|| {
        if std::env::var_os("CUTLINE_NO_GPU").is_some() {
            return None;
        }
        // drivers can crash while requesting: never across the callers
        std::panic::catch_unwind(init).ok().flatten()
    })
    .as_ref()
}

fn init() -> Option<Gpu> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let ask = |fallback: bool| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: fallback,
            compatible_surface: None,
        }))
    };
    let adapter = ask(false).or_else(|| ask(true))?;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("cl-gpu"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
        },
        None,
    ))
    .ok()?;
    // the default would be a panic on uncaught errors
    device.on_uncaptured_error(Box::new(|e| eprintln!("cl-gpu: {e}")));
    let max_size = device.limits().max_texture_dimension_2d;
    let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cl-gpu"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            tex_entry(1),
            tex_entry(2),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("cl-gpu"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cl-gpu params"),
        size: UNIFORM_SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("cl-gpu"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let info = adapter.get_info();
    Some(Gpu {
        device,
        queue,
        state: Mutex::new(State {
            pipelines: HashMap::new(),
            from: None,
            to: None,
            out: None,
            uniform,
            sampler,
            layout,
            pipeline_layout,
        }),
        adapter: format!("{} ({:?})", info.name, info.backend),
        max_size,
    })
}

const UNIFORM_SIZE: u64 = 4 * (4 + PARAMS as u64);
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Render a transition: `from`/`to` of any size (stretched to the output),
/// result as RGBA at `out` size. Shorthand for `gpu()?.render(…)`.
pub fn render(
    shader: &str,
    params: &[f32; PARAMS],
    from: Frame,
    to: Frame,
    out: (u32, u32),
    progress: f32,
) -> Result<Vec<u8>, String> {
    gpu()
        .ok_or_else(|| "no GPU".to_string())?
        .render(shader, params, from, to, out, progress)
}

impl Gpu {
    /// See [`render`]. Pipelines and textures are reused.
    pub fn render(
        &self,
        shader: &str,
        params: &[f32; PARAMS],
        from: Frame,
        to: Frame,
        out: (u32, u32),
        progress: f32,
    ) -> Result<Vec<u8>, String> {
        from.check(self.max_size)?;
        to.check(self.max_size)?;
        if out.0 == 0 || out.1 == 0 || out.0 > self.max_size || out.1 > self.max_size {
            return Err(format!("Output size {}x{} not possible", out.0, out.1));
        }
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.render_locked(&mut st, shader, params, from, to, out, progress)
        }))
        .unwrap_or_else(|_| Err("GPU error".into()))
    }

    /// Build the pipeline for `shader` up front (compiling takes noticeably long the first time);
    /// a later [`render`] reuses it.
    pub fn prepare(&self, shader: &str) -> Result<(), String> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.pipeline(&mut st, shader).map(|_| ())))
            .unwrap_or_else(|_| Err("GPU error".into()))
    }

    fn pipeline(&self, st: &mut State, shader: &str) -> Result<wgpu::RenderPipeline, String> {
        if let Some(p) = st.pipelines.get(shader) {
            return p.clone();
        }
        let built = validate(shader).and_then(|()| {
            self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("cl-gpu transition"),
                source: wgpu::ShaderSource::Wgsl(wrap(shader).into()),
            });
            let p = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("cl-gpu transition"),
                layout: Some(&st.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("cl_vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("cl_fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            });
            match pollster::block_on(self.device.pop_error_scope()) {
                Some(e) => Err(e.to_string()),
                None => Ok(p),
            }
        });
        st.pipelines.insert(shader.to_string(), built.clone());
        built
    }

    fn texture(&self, slot: &mut Option<Tex>, size: (u32, u32), usage: wgpu::TextureUsages) {
        if slot.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cl-gpu"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        *slot = Some(Tex { size, tex, view });
    }

    fn upload(&self, t: &Tex, f: Frame) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t.tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            f.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * f.width),
                rows_per_image: Some(f.height),
            },
            wgpu::Extent3d {
                width: f.width,
                height: f.height,
                depth_or_array_layers: 1,
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn render_locked(
        &self,
        st: &mut State,
        shader: &str,
        params: &[f32; PARAMS],
        from: Frame,
        to: Frame,
        out: (u32, u32),
        progress: f32,
    ) -> Result<Vec<u8>, String> {
        let pipeline = self.pipeline(st, shader)?;
        self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let input = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        self.texture(&mut st.from, (from.width, from.height), input);
        self.texture(&mut st.to, (to.width, to.height), input);
        let padded = (out.0 * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        if st.out.as_ref().is_none_or(|(t, _)| t.size != out) {
            let mut slot = None;
            self.texture(
                &mut slot,
                out,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cl-gpu readback"),
                size: padded as u64 * out.1 as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            st.out = slot.map(|t| (t, buf));
        }
        let (Some(ta), Some(tb), Some((to_tex, buf))) = (&st.from, &st.to, &st.out) else {
            return Err("texture missing".into());
        };
        self.upload(ta, from);
        self.upload(tb, to);
        let mut u = vec![progress, out.0 as f32 / out.1 as f32, 0.0, 0.0];
        u.extend_from_slice(params);
        let bytes: Vec<u8> = u.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.queue.write_buffer(&st.uniform, 0, &bytes);
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cl-gpu"),
            layout: &st.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: st.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&ta.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&tb.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&st.sampler),
                },
            ],
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cl-gpu"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &to_tex.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &to_tex.tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(out.1),
                },
            },
            wgpu::Extent3d {
                width: out.0,
                height: out.1,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);
        let validation = pollster::block_on(self.device.pop_error_scope());
        let oom = pollster::block_on(self.device.pop_error_scope());
        if let Some(e) = validation.or(oom) {
            return Err(e.to_string());
        }
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::Maintain::Wait);
        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e.to_string()),
            Err(_) => return Err("GPU does not respond".into()),
        }
        let row = out.0 as usize * 4;
        let mut px = Vec::with_capacity(row * out.1 as usize);
        {
            let data = slice.get_mapped_range();
            for y in 0..out.1 as usize {
                let s = y * padded as usize;
                px.extend_from_slice(&data[s..s + row]);
            }
        }
        buf.unmap();
        Ok(px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    const CROSSFADE: &str =
        "fn transition(uv: vec2f, progress: f32, ratio: f32) -> vec4f { return mix(from_color(uv), to_color(uv), progress); }";

    /// Reproducible noise (hard on any deviation in sampling)
    pub(crate) fn noise(w: u32, h: u32, seed: u32) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
        (0..w * h * 4)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                // full opacity so colors stay unambiguous
                if i % 4 == 3 {
                    255
                } else {
                    (x >> 24) as u8
                }
            })
            .collect()
    }

    fn max_diff(a: &[u8], b: &[u8]) -> u8 {
        assert_eq!(a.len(), b.len());
        a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
    }

    /// All transitions with a shader from the bundled packages: (ID, source, parameters)
    fn shipped() -> Vec<(String, String, [f32; PARAMS])> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let reg = cl_plugin::Registry::scan([root.as_path()]);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        reg.plugins
            .iter()
            .flat_map(|p| p.manifest.assets.iter().map(move |a| (p, a)))
            .filter(|(_, a)| a.kind == cl_plugin::AssetKind::Transition)
            .filter_map(|(p, a)| {
                let file: PathBuf = p.dir.join(a.shader.as_ref()?);
                let src = std::fs::read_to_string(&file).unwrap();
                let mut prm = [0.0; PARAMS];
                let vals = a.shader_params().unwrap_or_else(|e| panic!("{}: {e}", a.id));
                prm[..vals.len()].copy_from_slice(&vals);
                Some((a.id.clone(), src, prm))
            })
            .collect()
    }

    #[test]
    fn shipped_shaders_are_valid_without_gpu() {
        let list = shipped();
        assert!(list.len() >= 20, "{}", list.len());
        for (id, src, _) in &list {
            validate(src).unwrap_or_else(|e| panic!("{id}: {e}"));
        }
        assert!(validate("fn transition(uv: vec2f) -> vec4f { return vec4f(0.0); }").is_err());
        assert!(validate("broken").is_err());
        validate(CROSSFADE).unwrap();
    }

    #[test]
    fn shipped_transitions_have_known_fallbacks() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let reg = cl_plugin::Registry::scan([root.as_path()]);
        // a package that fails to load would drop its transitions from the check unseen
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);
        let mut n = 0;
        for (_, a) in reg.assets(cl_plugin::AssetKind::Transition) {
            let fb = a.fallback.as_deref().unwrap_or_else(|| panic!("{}: no fallback", a.id));
            assert!(cl_fx::TRANSITIONS.iter().any(|t| t.0 == fb), "{}: {fb}", a.id);
            n += 1;
        }
        assert!(n >= 30, "only {n} shipped transitions");
    }

    #[test]
    fn crossfade_ends_and_middle() {
        let Some(g) = gpu() else {
            eprintln!("no GPU, skipped");
            return;
        };
        let (w, h) = (64, 36);
        let (a, b) = (noise(w, h, 1), noise(w, h, 2));
        let fa = Frame { rgba: &a, width: w, height: h };
        let fb = Frame { rgba: &b, width: w, height: h };
        let p = [0.0; PARAMS];
        assert_eq!(max_diff(&g.render(CROSSFADE, &p, fa, fb, (w, h), 0.0).unwrap(), &a), 0);
        assert_eq!(max_diff(&g.render(CROSSFADE, &p, fa, fb, (w, h), 1.0).unwrap(), &b), 0);
        let mid = g.render(CROSSFADE, &p, fa, fb, (w, h), 0.5).unwrap();
        let mean: Vec<u8> = a.iter().zip(&b).map(|(x, y)| ((*x as f32 + *y as f32) / 2.0).round() as u8).collect();
        assert!(max_diff(&mid, &mean) <= 1);
        // requested size, odd too, and two inputs of different size
        let small = noise(31, 17, 3);
        let fs = Frame { rgba: &small, width: 31, height: 17 };
        let o = g.render(CROSSFADE, &p, fa, fs, (101, 57), 0.3).unwrap();
        assert_eq!(o.len(), 101 * 57 * 4);
        let o = g.render(CROSSFADE, &p, fs, fa, (20, 40), 0.7).unwrap();
        assert_eq!(o.len(), 20 * 40 * 4);
        // wrong data is an error, not a crash
        assert!(g.render(CROSSFADE, &p, Frame { rgba: &a[4..], width: w, height: h }, fb, (w, h), 0.5).is_err());
        assert!(g.render("broken", &p, fa, fb, (w, h), 0.5).is_err());
        assert!(g.render(CROSSFADE, &p, fa, fb, (0, h), 0.5).is_err());
    }

    #[test]
    fn prepare_builds_the_pipeline_ahead() {
        let Some(g) = gpu() else {
            eprintln!("no GPU, skipped");
            return;
        };
        let shader = CROSSFADE.replace("progress)", "progress * 1.0)");
        g.prepare(&shader).unwrap();
        assert!(g.prepare("broken").is_err());
        // the pipeline is ready before the first frame
        assert!(g.state.lock().unwrap().pipelines.get(&shader).is_some_and(|p| p.is_ok()));
    }

    #[test]
    fn every_shipped_shader_starts_with_a_and_ends_with_b() {
        let Some(g) = gpu() else {
            eprintln!("no GPU, skipped");
            return;
        };
        // landscape and portrait
        for (w, h) in [(96, 54), (54, 96)] {
            let (a, b) = (noise(w, h, 7), noise(w, h, 9));
            let fa = Frame { rgba: &a, width: w, height: h };
            let fb = Frame { rgba: &b, width: w, height: h };
            for (id, src, prm) in shipped() {
                let s = g.render(&src, &prm, fa, fb, (w, h), 0.0).unwrap_or_else(|e| panic!("{id}: {e}"));
                assert!(max_diff(&s, &a) <= 1, "{id} {w}x{h}: start ≠ A ({})", max_diff(&s, &a));
                let e = g.render(&src, &prm, fa, fb, (w, h), 1.0).unwrap();
                assert!(max_diff(&e, &b) <= 1, "{id} {w}x{h}: end ≠ B ({})", max_diff(&e, &b));
            }
        }
    }
}
