//! The `canvas` widget: a script-supplied WGSL shader run over the widget area.
//!
//! Custom visuals, ShaderToy-style. The script sends a fragment shader as a
//! property; the host runs it on a full-viewport triangle and feeds it a small
//! set of uniforms — the viewport `resolution`, the elapsed `time`, and four
//! `params`. The params are driven two ways, which is the point of the widget:
//! from the **script** (`/gui_set param0 …`, an OSC value) and from a
//! **control bus read out of shared memory each frame** (the `buses` mapping),
//! exactly the zero-message path the meters use. So a scripted shader animates
//! from OSC parameters and from live server audio at once.
//!
//! The user writes only a `shade` function; this module wraps it with a fixed
//! prelude (the uniform block + a full-screen-triangle vertex shader) and a
//! `fs_main` that calls it. A shader that fails to compile is caught (a wgpu
//! validation error scope) and leaves the canvas un-painted with a warning,
//! never crashing the host.

use std::time::Instant;

use tracing::warn;

/// The number of `params` the uniform block carries (a `vec4`).
pub const PARAM_COUNT: usize = 4;

/// The WGSL prepended to the user's `shade`: the uniform block and a vertex
/// shader emitting one screen-covering triangle with a top-left `uv` in 0..1.
const PRELUDE: &str = r#"
struct Uniforms {
    resolution: vec2<f32>,
    time: f32,
    _pad: f32,
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let xy = corners[vi];
    var out: VsOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = vec2<f32>((xy.x + 1.0) * 0.5, (1.0 - xy.y) * 0.5);
    return out;
}
"#;

/// The WGSL appended after the user's `shade`: the fragment entry that calls it.
const FOOTER: &str = r#"
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return shade(in.uv, in.pos);
}
"#;

/// The shader used when a `canvas` omits one: a moving color field, so an empty
/// canvas still shows it is live (and documents the `shade` signature).
pub const DEFAULT_SHADER: &str = r#"
fn shade(uv: vec2<f32>, frag: vec4<f32>) -> vec4<f32> {
    let t = u.time;
    return vec4<f32>(uv.x, uv.y, 0.5 + 0.5 * sin(t), 1.0);
}
"#;

/// A `canvas`'s GPU resources: the pipeline compiled from the user's shader
/// (`None` when it failed to compile), the per-frame uniform buffer and its bind
/// group. Rebuilt in place when the shader property changes ([`set_shader`]).
pub struct CanvasView {
    pipeline: Option<wgpu::RenderPipeline>,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    pipeline_layout: wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    /// The user `shade` source the current pipeline was built from, to skip
    /// recompiling unchanged shaders (and to avoid retrying a broken one).
    shader_src: String,
    /// When this canvas started, for the `time` uniform.
    start: Instant,
}

impl CanvasView {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, shader_src: &str) -> Self {
        // 8 f32: resolution.xy, time, pad, params.xyzw.
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("canvas uniforms"),
            size: 8 * std::mem::size_of::<f32>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = build_pipeline(device, format, &pipeline_layout, shader_src);
        Self {
            pipeline,
            uniform_buffer,
            bind_group,
            pipeline_layout,
            format,
            shader_src: shader_src.to_string(),
            start: Instant::now(),
        }
    }

    /// Recompiles the pipeline when `shader_src` differs from the current one.
    /// A failed compile leaves the canvas un-painted (no panic); the source is
    /// stored regardless, so a broken shader is not retried every frame.
    pub fn set_shader(&mut self, device: &wgpu::Device, shader_src: &str) {
        if shader_src == self.shader_src {
            return;
        }
        self.pipeline = build_pipeline(device, self.format, &self.pipeline_layout, shader_src);
        self.shader_src = shader_src.to_string();
    }

    /// Seconds since this canvas was created (the `time` uniform).
    pub fn elapsed(&self) -> f32 {
        self.start.elapsed().as_secs_f32()
    }

    /// Writes the per-frame uniforms (viewport size, elapsed time, params).
    pub fn upload(
        &self,
        queue: &wgpu::Queue,
        resolution: [f32; 2],
        time: f32,
        params: [f32; PARAM_COUNT],
    ) {
        let data: [f32; 8] = [
            resolution[0],
            resolution[1],
            time,
            0.0,
            params[0],
            params[1],
            params[2],
            params[3],
        ];
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&data));
    }

    /// Records the full-viewport draw (a single triangle). A no-op when the
    /// shader failed to compile.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(pipeline) = &self.pipeline {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// Compiles the user `shade` (wrapped with the prelude/footer) into a pipeline,
/// capturing any WGSL validation error instead of letting it panic the host.
fn build_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    layout: &wgpu::PipelineLayout,
    user_src: &str,
) -> Option<wgpu::RenderPipeline> {
    let full = format!("{PRELUDE}\n{user_src}\n{FOOTER}");
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("canvas user shader"),
        source: wgpu::ShaderSource::Wgsl(full.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("canvas pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    if let Some(err) = pollster::block_on(scope.pop()) {
        warn!("canvas shader failed to compile: {err}");
        return None;
    }
    Some(pipeline)
}
