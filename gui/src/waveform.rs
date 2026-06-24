//! Waveform view: the audio-specific data holder and its GPU renderer, built on
//! the reusable `viewport::View` and `peaks::Pyramid`.
//!
//! The renderer resolves the signal to exactly the rendered resolution, picking
//! one of three regimes by `samples_per_px` so it never wastes work:
//!
//! - **Line** (`samples_per_px <= LINE_THRESHOLD`): so few samples are visible
//!   that individual ones matter; draw a polyline through the raw samples in
//!   range. Vertex count is bounded by the window width, not the buffer.
//! - **Raw columns** (`LINE_THRESHOLD < samples_per_px < base_bucket`): one
//!   min/max column per pixel, computed directly from the raw samples - exact,
//!   and bounded because we only enter here below `base_bucket` samples/px.
//! - **Pyramid columns** (`samples_per_px >= base_bucket`): one min/max column
//!   per pixel, read from the peak pyramid level matching the zoom.
//!
//! `WaveformRenderer` takes a `wgpu::Device`/`Queue` and a target format and
//! owns nothing windowing-specific, so the identical code drives a native
//! `winit` surface or a `<canvas>` WebGPU surface in a browser.

use std::sync::Arc;

use wgpu::util::DeviceExt;

use crate::peaks::{self, Pyramid};
use crate::viewport::View;

/// At or below this many samples per pixel, draw the raw sample polyline rather
/// than min/max columns.
const LINE_THRESHOLD: f64 = 2.0;

/// A waveform's data: the raw samples (shared, for the zoomed-in regimes) plus
/// its peak pyramid (for the zoomed-out regime). The pyramid is the cache that
/// can be persisted via `peaks::Pyramid::write_cache`.
pub struct WaveformData {
    samples: Arc<[f32]>,
    pyramid: Pyramid,
}

impl WaveformData {
    pub fn new(samples: Arc<[f32]>, base_bucket: usize) -> Self {
        let pyramid = Pyramid::build(&samples, base_bucket);
        Self { samples, pyramid }
    }

    /// Build from samples and an already-computed pyramid (e.g. read back from a
    /// cache file with `Pyramid::read_cache`).
    pub fn with_pyramid(samples: Arc<[f32]>, pyramid: Pyramid) -> Self {
        Self { samples, pyramid }
    }

    pub fn total_samples(&self) -> usize {
        self.samples.len()
    }

    pub fn pyramid(&self) -> &Pyramid {
        &self.pyramid
    }

    /// Min/max for a pixel column spanning `[s0, s1)`, choosing the cheapest
    /// accurate source for the given `samples_per_px`: raw samples when finer
    /// than the pyramid's base bucket, the pyramid otherwise.
    pub fn column(&self, samples_per_px: f64, s0: f64, s1: f64) -> (f32, f32) {
        if samples_per_px < self.pyramid.base_bucket() as f64 {
            let a = (s0.floor().max(0.0) as usize).min(self.samples.len());
            let b = (s1.ceil() as usize).clamp(a, self.samples.len());
            peaks::min_max(&self.samples[a..b]).unwrap_or((0.0, 0.0))
        } else {
            let level = self.pyramid.level_for(samples_per_px);
            self.pyramid.column(level, s0, s1).unwrap_or((0.0, 0.0))
        }
    }
}

/// Map `amp` in [-1, 1] to clip-space y, leaving a small vertical margin.
fn amp_to_clip(amp: f32) -> f32 {
    (amp * 0.92).clamp(-1.0, 1.0)
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Columns,
    Line,
}

/// Backend-independent waveform renderer. Holds a triangle pipeline (min/max
/// columns) and a line pipeline (raw sample polyline) sharing one shader, bind
/// group and vertex buffer; `upload_geometry` selects the regime per frame.
pub struct WaveformRenderer {
    column_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    capacity_vertices: u64,
    num_vertices: u32,
    mode: Mode,
    scratch: Vec<[f32; 2]>,
}

impl WaveformRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("waveform shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("waveform.wgsl").into()),
        });

        let color: [f32; 4] = [0.30, 0.78, 0.55, 1.0];
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("waveform uniforms"),
            contents: bytemuck::cast_slice(&color),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("waveform bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("waveform bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waveform pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: (std::mem::size_of::<f32>() * 2) as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            }],
        };

        let make_pipeline = |topology: wgpu::PrimitiveTopology, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: std::slice::from_ref(&vertex_layout),
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let column_pipeline =
            make_pipeline(wgpu::PrimitiveTopology::TriangleList, "waveform columns");
        let line_pipeline = make_pipeline(wgpu::PrimitiveTopology::LineStrip, "waveform line");

        let capacity_vertices = 8192 * 6;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waveform vertices"),
            size: capacity_vertices * 2 * std::mem::size_of::<f32>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            column_pipeline,
            line_pipeline,
            bind_group,
            vertex_buffer,
            capacity_vertices,
            num_vertices: 0,
            mode: Mode::Columns,
            scratch: Vec::new(),
        }
    }

    /// Rebuild and upload the geometry for `view` at `render_width_px` device
    /// pixels. O(render_width_px) in the column regimes, O(visible samples) in
    /// the line regime - both bounded by the screen, never by the buffer.
    pub fn upload_geometry(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &WaveformData,
        view: &View,
        render_width_px: u32,
    ) {
        let w = render_width_px.max(1);
        let spp = view.samples_per_px(w);
        let total = data.total_samples();
        self.scratch.clear();

        if spp <= LINE_THRESHOLD {
            self.mode = Mode::Line;
            let a = (view.start.floor().max(0.0) as usize).min(total);
            let b = ((view.start + view.len).ceil() as usize).min(total);
            for i in a..b {
                let frac = (i as f64 - view.start) / view.len;
                let x = (-1.0 + 2.0 * frac) as f32;
                self.scratch.push([x, amp_to_clip(data.samples_at(i))]);
            }
        } else {
            self.mode = Mode::Columns;
            for x in 0..w {
                let s0 = view.start + view.len * (x as f64 / w as f64);
                let s1 = view.start + view.len * ((x + 1) as f64 / w as f64);
                let (lo, hi) = data.column(spp, s0, s1);
                let xl = -1.0 + 2.0 * (x as f32 / w as f32);
                let xr = -1.0 + 2.0 * ((x + 1) as f32 / w as f32);
                let yb = amp_to_clip(lo.min(0.0));
                let yt = amp_to_clip(hi.max(0.0));
                self.scratch.push([xl, yb]);
                self.scratch.push([xr, yb]);
                self.scratch.push([xr, yt]);
                self.scratch.push([xl, yb]);
                self.scratch.push([xr, yt]);
                self.scratch.push([xl, yt]);
            }
        }

        let needed = self.scratch.len() as u64;
        if needed > self.capacity_vertices {
            self.capacity_vertices = needed.next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("waveform vertices"),
                size: self.capacity_vertices * 2 * std::mem::size_of::<f32>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.scratch));
        self.num_vertices = self.scratch.len() as u32;
    }

    /// Record the draw into an existing render pass.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.num_vertices == 0 {
            return;
        }
        let pipeline = match self.mode {
            Mode::Columns => &self.column_pipeline,
            Mode::Line => &self.line_pipeline,
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.num_vertices, 0..1);
    }
}

impl WaveformData {
    /// Single-sample access for the line regime, clamped to bounds.
    fn samples_at(&self, i: usize) -> f32 {
        self.samples.get(i).copied().unwrap_or(0.0)
    }
}
