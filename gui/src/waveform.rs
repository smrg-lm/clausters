//! GPU-agnostic waveform rendering for an editor-style, navigable view of a
//! large audio buffer (millions of samples), with zoom and pan.
//!
//! The core idea is the same one Audacity-class editors use, expressed for the
//! GPU: never try to draw every sample. Precompute a *min/max envelope pyramid*
//! once (`Envelope`), then each frame pick the pyramid level whose resolution
//! matches the current zoom and emit one quad per pixel column spanning that
//! column's [min, max]. Zoom and pan are just changes to the visible sample
//! range, so they are effectively free; the per-frame work is proportional to
//! the window width in pixels, not to the buffer length.
//!
//! `WaveformRenderer` takes a `wgpu::Device`/`Queue` and a target texture
//! format, so it is independent of the windowing backend: the native
//! (`winit`) entry point in `main.rs` drives it today, and the identical code
//! drives a `<canvas>` surface under WebGPU in a browser tomorrow.

use wgpu::util::DeviceExt;

/// One resolution level of the min/max pyramid. `bucket` is how many source
/// samples each `(min[i], max[i])` pair summarizes.
struct Lod {
    bucket: usize,
    min: Vec<f32>,
    max: Vec<f32>,
}

/// A min/max pyramid over an audio buffer. Level 0 buckets `base_bucket`
/// samples each; every higher level halves the resolution (min of mins, max of
/// maxs) until a single bucket spans the whole buffer. Total storage is ~2x the
/// level-0 size, i.e. a small constant fraction of the source buffer.
pub struct Envelope {
    total_samples: usize,
    lods: Vec<Lod>,
}

impl Envelope {
    /// Build the pyramid from mono `samples`. `base_bucket` is the level-0
    /// bucket size (e.g. 256 samples); smaller means finer detail when fully
    /// zoomed in, at the cost of more level-0 storage.
    pub fn build(samples: &[f32], base_bucket: usize) -> Self {
        assert!(base_bucket >= 1);
        let total_samples = samples.len();

        // Level 0: scan the raw samples once.
        let n0 = total_samples.div_ceil(base_bucket);
        let mut min0 = vec![0.0f32; n0];
        let mut max0 = vec![0.0f32; n0];
        for (b, chunk) in samples.chunks(base_bucket).enumerate() {
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for &s in chunk {
                lo = lo.min(s);
                hi = hi.max(s);
            }
            min0[b] = lo;
            max0[b] = hi;
        }

        let mut lods = vec![Lod {
            bucket: base_bucket,
            min: min0,
            max: max0,
        }];

        // Higher levels: merge adjacent pairs of the previous level.
        while lods.last().unwrap().min.len() > 1 {
            let prev = lods.last().unwrap();
            let n = prev.min.len().div_ceil(2);
            let mut min = vec![0.0f32; n];
            let mut max = vec![0.0f32; n];
            for i in 0..n {
                let a = 2 * i;
                let b = (2 * i + 1).min(prev.min.len() - 1);
                min[i] = prev.min[a].min(prev.min[b]);
                max[i] = prev.max[a].max(prev.max[b]);
            }
            lods.push(Lod {
                bucket: prev.bucket * 2,
                min,
                max,
            });
        }

        Self {
            total_samples,
            lods,
        }
    }

    pub fn total_samples(&self) -> usize {
        self.total_samples
    }

    /// Pick the finest level whose bucket size does not exceed
    /// `samples_per_px`, so each pixel column aggregates at least one bucket
    /// (no gaps) while keeping the per-column work bounded. When zoomed in
    /// past level 0, level 0 is used.
    fn pick_lod(&self, samples_per_px: f64) -> &Lod {
        let mut chosen = &self.lods[0];
        for lod in &self.lods {
            if (lod.bucket as f64) <= samples_per_px {
                chosen = lod;
            } else {
                break;
            }
        }
        chosen
    }

    /// Min/max of the source samples in `[start, end)` at the given level,
    /// reading only the buckets that overlap the range.
    fn column_min_max(&self, lod: &Lod, start: f64, end: f64) -> (f32, f32) {
        let b0 = (start / lod.bucket as f64).floor().max(0.0) as usize;
        let last = lod.min.len().saturating_sub(1);
        let b1 = (((end / lod.bucket as f64).ceil() as usize).saturating_sub(1)).min(last);
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for b in b0..=b1.max(b0) {
            if b > last {
                break;
            }
            lo = lo.min(lod.min[b]);
            hi = hi.max(lod.max[b]);
        }
        if !lo.is_finite() {
            (0.0, 0.0)
        } else {
            (lo, hi)
        }
    }
}

/// The visible window into the buffer, in source-sample units (f64 so that
/// deep zoom stays precise over multi-million-sample buffers).
#[derive(Clone, Copy)]
pub struct View {
    pub start: f64,
    pub len: f64,
}

impl View {
    /// Zoom by `factor` (<1 zooms in) keeping the sample under `anchor`
    /// (0..1 across the window) fixed, then clamp to the buffer bounds.
    pub fn zoom(&mut self, factor: f64, anchor: f64, total: usize) {
        let pivot = self.start + self.len * anchor;
        let new_len = (self.len * factor).clamp(1.0, total as f64);
        self.start = pivot - new_len * anchor;
        self.len = new_len;
        self.clamp(total);
    }

    /// Pan by `dx` fraction of the window width (drag-to-scroll).
    pub fn pan(&mut self, dx: f64, total: usize) {
        self.start += dx * self.len;
        self.clamp(total);
    }

    fn clamp(&mut self, total: usize) {
        let total = total as f64;
        if self.len > total {
            self.len = total;
        }
        self.start = self.start.clamp(0.0, (total - self.len).max(0.0));
    }
}

/// Map `amp` in [-1, 1] to clip-space y, leaving a small vertical margin.
fn amp_to_clip(amp: f32) -> f32 {
    (amp * 0.92).clamp(-1.0, 1.0)
}

/// Backend-independent renderer: hand it a device/queue and the target format
/// and it owns the pipeline; call `upload_geometry` then `draw`.
pub struct WaveformRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    capacity_vertices: u64,
    num_vertices: u32,
}

impl WaveformRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("waveform shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("waveform.wgsl").into()),
        });

        // Fill color uniform (waveform body).
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

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("waveform pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[vertex_layout],
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
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Start with a modest vertex buffer; `upload_geometry` grows it as the
        // window widens (6 vertices per pixel column).
        let capacity_vertices = 8192 * 6;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("waveform vertices"),
            size: capacity_vertices * 2 * std::mem::size_of::<f32>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group,
            vertex_buffer,
            capacity_vertices,
            num_vertices: 0,
        }
    }

    /// Rebuild the per-column geometry for `view` at `width_px` pixels and
    /// upload it. Cheap enough to call every frame: O(width_px).
    pub fn upload_geometry(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        env: &Envelope,
        view: &View,
        width_px: u32,
    ) {
        let width_px = width_px.max(1);
        let samples_per_px = (view.len / width_px as f64).max(1e-9);
        let lod = env.pick_lod(samples_per_px);

        let mut verts: Vec<[f32; 2]> = Vec::with_capacity(width_px as usize * 6);
        for x in 0..width_px {
            let s0 = view.start + view.len * (x as f64 / width_px as f64);
            let s1 = view.start + view.len * ((x + 1) as f64 / width_px as f64);
            let (lo, hi) = env.column_min_max(lod, s0, s1);

            let xl = -1.0 + 2.0 * (x as f32 / width_px as f32);
            let xr = -1.0 + 2.0 * ((x + 1) as f32 / width_px as f32);
            // Guarantee a visible 1px-ish band even for near-silence.
            let yb = amp_to_clip(lo.min(0.0));
            let yt = amp_to_clip(hi.max(0.0));

            // Two triangles for the column quad.
            verts.push([xl, yb]);
            verts.push([xr, yb]);
            verts.push([xr, yt]);
            verts.push([xl, yb]);
            verts.push([xr, yt]);
            verts.push([xl, yt]);
        }

        let needed = verts.len() as u64;
        if needed > self.capacity_vertices {
            self.capacity_vertices = needed.next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("waveform vertices"),
                size: self.capacity_vertices * 2 * std::mem::size_of::<f32>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&verts));
        self.num_vertices = verts.len() as u32;
    }

    /// Record the draw into an existing render pass.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.num_vertices == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.num_vertices, 0..1);
    }
}
