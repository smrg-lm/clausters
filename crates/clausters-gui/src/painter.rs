//! The GPU half of the host's drawing primitive: a [`Mesh`] uploaded and
//! drawn in one call, its glyphs sampled from the window's atlas texture.
//!
//! The batch itself is the drawing base's (`host::paint`), which knows
//! nothing of a device; this is what puts it on one, beside the other
//! pipelines a window's pass draws (`view`, `waveform`, `spectrogram`).

#[cfg(feature = "font-atlas")]
use clausters_gui_draw::paint::FLOATS_PER_GLYPH_VERTEX;
use clausters_gui_draw::paint::{FLOATS_PER_VERTEX, Mesh};

/// Uploads a [`Mesh`] and draws it.
pub struct Painter {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    capacity_vertices: u64,
    num_vertices: u32,
    /// The glyph vertices of this batch, when the crate was built with a
    /// rasterizer. Absent until the batch first carries a glyph.
    #[cfg(feature = "font-atlas")]
    text: Option<TextLayer>,
    /// **The window's glyph sheet**: the textured pipeline and the window's
    /// copy of the atlas, shared by every batch of the window
    /// ([`Painter::sharing`]) and absent until one of them carries a letter,
    /// so a build with the feature on and no face pays no texture.
    #[cfg(feature = "font-atlas")]
    sheet: std::rc::Rc<std::cell::RefCell<Option<GlyphSheet>>>,
    /// The pass this painter draws into, kept for the glyph sheet's pipeline:
    /// it is built on the first batch that carries a letter, and it has to
    /// agree with the flat one on the format **and** the sample count.
    #[cfg(feature = "font-atlas")]
    target: crate::view::Target,
}

/// One batch's glyph vertices: what a [`Painter`] holds of its text, beside
/// the sheet its window shares.
#[cfg(feature = "font-atlas")]
struct TextLayer {
    vertex_buffer: wgpu::Buffer,
    capacity_vertices: u64,
    num_vertices: u32,
}

#[cfg(feature = "font-atlas")]
impl TextLayer {
    fn new(device: &wgpu::Device) -> Self {
        let float = std::mem::size_of::<f32>() as wgpu::BufferAddress;
        let capacity_vertices = 6 * 256;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyph vertices"),
            size: capacity_vertices * FLOATS_PER_GLYPH_VERTEX as u64 * float,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            vertex_buffer,
            capacity_vertices,
            num_vertices: 0,
        }
    }
}

/// **A window's glyph sheet**: the textured pipeline and the window's copy of
/// the glyph atlas, one for all of its batches.
///
/// Each batch used to carry its own -- a megabyte of texture and a pipeline
/// apiece, three and then four to a window, each copying the whole sheet
/// again whenever the cache rasterized one new glyph. The sheet is the
/// window's, as the comment on the shader always said; a batch holds its
/// vertices.
///
/// **It is as high as the atlas is, and takes the rows that changed.** The
/// atlas starts at a few shelves and doubles as it fills, and this texture is
/// made again at its height when it does; between growths a sync copies from
/// the shelf that was being filled at the last one down, since every row
/// above it is finished.
#[cfg(feature = "font-atlas")]
struct GlyphSheet {
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    /// How many rows the texture has.
    rows: u32,
    /// The atlas version this texture holds: rows are copied only when the
    /// shared cache has rasterized something new.
    version: u64,
    /// The atlas epoch it holds: a sheet packed again is copied whole.
    epoch: u64,
    /// The top of the shelf the atlas was filling when this was last synced.
    shelf: u32,
}

#[cfg(feature = "font-atlas")]
impl GlyphSheet {
    /// A texture of `rows` rows and the bind group that samples it.
    fn texture(
        device: &wgpu::Device,
        bind_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        rows: u32,
    ) -> (wgpu::Texture, wgpu::BindGroup) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: clausters_gui_draw::font::atlas::SIDE,
                height: rows.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // One coverage byte per texel: the smallest thing WebGL2 and
            // WebGPU both sample, which is what a glyph sheet is.
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyph bind group"),
            layout: bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        (texture, bind_group)
    }

    fn new(device: &wgpu::Device, target: crate::view::Target, rows: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyph shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("paint_text.wgsl").into()),
        });
        // Nearest: a glyph is rasterized at the size it draws at and its quad
        // is snapped to whole pixels, so every texel lands on its own pixel.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyph atlas sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyph bind layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glyph pipeline layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let float = std::mem::size_of::<f32>() as wgpu::BufferAddress;
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: FLOATS_PER_GLYPH_VERTEX as wgpu::BufferAddress * float,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 2 * float,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 4 * float,
                    shader_location: 2,
                },
            ],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyph pipeline"),
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
                    format: target.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: target.multisample(),
            multiview_mask: None,
            cache: None,
        });
        let (texture, bind_group) = Self::texture(device, &bind_layout, &sampler, rows);
        Self {
            pipeline,
            bind_layout,
            sampler,
            texture,
            bind_group,
            rows,
            // 0 is "nothing uploaded": the atlas starts at 0 too and only
            // counts up once it holds a glyph, so the first sheet is copied.
            version: 0,
            epoch: 0,
            shelf: 0,
        }
    }

    /// Brings this window's texture up to the shared cache's sheet, if it is
    /// behind it: made again at the atlas' height where that grew, and the
    /// rows written since the last sync copied in -- all of them for a sheet
    /// that was made or packed again.
    fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &clausters_gui_draw::font::atlas::Atlas,
    ) {
        let pixels = atlas.pixels();
        if self.version == atlas.version() || pixels.is_empty() {
            return;
        }
        let side = clausters_gui_draw::font::atlas::SIDE;
        let mut from = self.shelf;
        if atlas.rows() != self.rows {
            (self.texture, self.bind_group) =
                Self::texture(device, &self.bind_layout, &self.sampler, atlas.rows());
            self.rows = atlas.rows();
            from = 0;
        }
        if self.epoch != atlas.epoch() {
            from = 0;
        }
        let to = atlas.used_rows();
        let from = from.min(to);
        if to > from {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: from,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels[(from * side) as usize..(to * side) as usize],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(side),
                    rows_per_image: Some(to - from),
                },
                wgpu::Extent3d {
                    width: side,
                    height: to - from,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.version = atlas.version();
        self.epoch = atlas.epoch();
        self.shelf = atlas.shelf_top();
    }
}

impl Painter {
    pub fn new(device: &wgpu::Device, target: crate::view::Target) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("paint shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("paint.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("paint pipeline layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: (FLOATS_PER_VERTEX * std::mem::size_of::<f32>()) as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: (2 * std::mem::size_of::<f32>()) as wgpu::BufferAddress,
                    shader_location: 1,
                },
            ],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("paint pipeline"),
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
                    format: target.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: target.multisample(),
            multiview_mask: None,
            cache: None,
        });
        let capacity_vertices = 6 * 256;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("paint vertices"),
            size: capacity_vertices * FLOATS_PER_VERTEX as u64 * std::mem::size_of::<f32>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            vertex_buffer,
            capacity_vertices,
            num_vertices: 0,
            #[cfg(feature = "font-atlas")]
            text: None,
            #[cfg(feature = "font-atlas")]
            sheet: Default::default(),
            #[cfg(feature = "font-atlas")]
            target,
        }
    }

    /// A painter of the same window as `other`: its own vertices, and
    /// `other`'s glyph sheet -- one texture and one textured pipeline for
    /// every batch a window draws.
    pub fn sharing(device: &wgpu::Device, target: crate::view::Target, other: &Painter) -> Self {
        #[cfg(not(feature = "font-atlas"))]
        let _ = other;
        #[allow(unused_mut)] // without a rasterizer there is nothing to share
        let mut painter = Self::new(device, target);
        #[cfg(feature = "font-atlas")]
        {
            painter.sheet = other.sheet.clone();
        }
        painter
    }

    /// Uploads `mesh`, converting its device-pixel positions to clip space
    /// against a framebuffer of `fb_w` x `fb_h` (top-left origin, y flipped).
    /// Answers the bytes of vertices written, for what a frame cost.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &Mesh,
        fb_w: u32,
        fb_h: u32,
    ) -> usize {
        self.upload_at(device, queue, mesh, (0.0, 0.0), (fb_w, fb_h))
    }

    /// Uploads `mesh` as [`upload`](Self::upload) does, for a viewport of
    /// `size` whose top-left corner is the framebuffer's pixel `origin`:
    /// what draws a part of a window into a texture of that part's size.
    pub fn upload_at(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &Mesh,
        origin: (f32, f32),
        size: (u32, u32),
    ) -> usize {
        let (fw, fh) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let (ox, oy) = origin;
        let mut clip = Vec::with_capacity(mesh.flat_vertices().len());
        for v in mesh.flat_vertices().as_chunks::<FLOATS_PER_VERTEX>().0 {
            clip.push(((v[0] - ox) / fw) * 2.0 - 1.0);
            clip.push(1.0 - ((v[1] - oy) / fh) * 2.0);
            clip.extend_from_slice(&v[2..6]);
        }
        let needed = mesh.vertex_count() as u64;
        if needed > self.capacity_vertices {
            self.capacity_vertices = needed.next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("paint vertices"),
                size: self.capacity_vertices
                    * FLOATS_PER_VERTEX as u64
                    * std::mem::size_of::<f32>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&clip));
        self.num_vertices = needed as u32;
        #[cfg(feature = "font-atlas")]
        self.upload_glyphs(device, queue, mesh, origin, fw, fh);
        let glyphs = {
            #[cfg(feature = "font-atlas")]
            {
                mesh.glyph_vertices().len()
            }
            #[cfg(not(feature = "font-atlas"))]
            {
                0
            }
        };
        (clip.len() + glyphs) * std::mem::size_of::<f32>()
    }

    /// The glyph half of the same batch: the vertices into this painter's
    /// buffer, and -- only when the shared cache has rasterized something new --
    /// the coverage sheet into this window's texture.
    #[cfg(feature = "font-atlas")]
    fn upload_glyphs(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &Mesh,
        (ox, oy): (f32, f32),
        fw: f32,
        fh: f32,
    ) {
        let verts = mesh.glyph_vertices();
        if verts.is_empty() {
            if let Some(text) = &mut self.text {
                text.num_vertices = 0;
            }
            return;
        }
        let text = self.text.get_or_insert_with(|| TextLayer::new(device));
        let mut clip = Vec::with_capacity(verts.len());
        for v in verts.as_chunks::<FLOATS_PER_GLYPH_VERTEX>().0 {
            clip.push(((v[0] - ox) / fw) * 2.0 - 1.0);
            clip.push(1.0 - ((v[1] - oy) / fh) * 2.0);
            clip.extend_from_slice(&v[2..FLOATS_PER_GLYPH_VERTEX]);
        }
        let needed = (verts.len() / FLOATS_PER_GLYPH_VERTEX) as u64;
        if needed > text.capacity_vertices {
            text.capacity_vertices = needed.next_power_of_two();
            text.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("glyph vertices"),
                size: text.capacity_vertices
                    * FLOATS_PER_GLYPH_VERTEX as u64
                    * std::mem::size_of::<f32>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&text.vertex_buffer, 0, bytemuck::cast_slice(&clip));
        text.num_vertices = needed as u32;
        // The window's sheet, made on the first letter any of its batches
        // carries, and brought up to what the cache has rasterized since.
        let target = self.target;
        clausters_gui_draw::font::atlas::with(|atlas| {
            self.sheet
                .borrow_mut()
                .get_or_insert_with(|| GlyphSheet::new(device, target, atlas.rows()))
                .sync(device, queue, atlas)
        });
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.num_vertices > 0 {
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..self.num_vertices, 0..1);
        }
        // The glyphs of this batch, over its own flat geometry: one bind group
        // and one draw for every letter in the window.
        #[cfg(feature = "font-atlas")]
        if let Some(text) = &self.text
            && text.num_vertices > 0
            && let Some(sheet) = self.sheet.borrow().as_ref()
        {
            pass.set_pipeline(&sheet.pipeline);
            pass.set_bind_group(0, &sheet.bind_group, &[]);
            pass.set_vertex_buffer(0, text.vertex_buffer.slice(..));
            pass.draw(0..text.num_vertices, 0..1);
        }
    }
}
