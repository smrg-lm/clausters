//! The GPU half of the host's drawing primitive: a [`Mesh`] uploaded and
//! drawn in one call, its glyphs sampled from the window's atlas texture.
//!
//! The batch itself is the drawing base's (`host::paint`), which knows
//! nothing of a device; this is what puts it on one, beside the other
//! pipelines a window's pass draws (`view`, `waveform`, `spectrogram`).

use clausters_gui_draw::paint::{FLOATS_PER_GLYPH_VERTEX, FLOATS_PER_VERTEX, Mesh};

/// A window's sheet, shared by every batch it draws ([`Painter::sharing`]).
type Shared = std::rc::Rc<std::cell::RefCell<Option<Sheet>>>;

/// Uploads a [`Mesh`] and draws it.
pub struct Painter {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    capacity_vertices: u64,
    num_vertices: u32,
    /// The glyph vertices of this batch, when the crate was built with a
    /// rasterizer. Absent until the batch first carries a glyph.
    #[cfg(feature = "font-atlas")]
    text: Option<Layer>,
    /// **The window's glyph sheet**: the textured pipeline and the window's
    /// copy of the atlas, shared by every batch of the window
    /// ([`Painter::sharing`]) and absent until one of them carries a letter,
    /// so a build with the feature on and no face pays no texture.
    #[cfg(feature = "font-atlas")]
    sheet: Shared,
    /// The picture vertices of this batch, absent until it first carries a
    /// picture.
    pictures: Option<Layer>,
    /// **The window's sheet of pictures**, shared as the glyphs' is and made
    /// on the first picture any batch of the window carries.
    picture_sheet: Shared,
    /// The pass this painter draws into, kept for the sheets' pipelines: each
    /// is built on the first batch that needs it, and it has to agree with
    /// the flat one on the format **and** the sample count.
    target: crate::view::Target,
}

/// One batch's textured vertices -- its glyphs or its pictures: what a
/// [`Painter`] holds beside the sheet its window shares.
struct Layer {
    vertex_buffer: wgpu::Buffer,
    capacity_vertices: u64,
    num_vertices: u32,
}

impl Layer {
    fn new(device: &wgpu::Device) -> Self {
        let float = std::mem::size_of::<f32>() as wgpu::BufferAddress;
        let capacity_vertices = 6 * 256;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("textured vertices"),
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

    /// `verts`, moved to clip space for a viewport of `fw` x `fh` at `origin`,
    /// into this layer's buffer.
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        verts: &[f32],
        (ox, oy): (f32, f32),
        fw: f32,
        fh: f32,
    ) {
        let mut clip = Vec::with_capacity(verts.len());
        for v in verts.as_chunks::<FLOATS_PER_GLYPH_VERTEX>().0 {
            clip.push(((v[0] - ox) / fw) * 2.0 - 1.0);
            clip.push(1.0 - ((v[1] - oy) / fh) * 2.0);
            clip.extend_from_slice(&v[2..FLOATS_PER_GLYPH_VERTEX]);
        }
        let needed = (verts.len() / FLOATS_PER_GLYPH_VERTEX) as u64;
        if needed > self.capacity_vertices {
            self.capacity_vertices = needed.next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("textured vertices"),
                size: self.capacity_vertices
                    * FLOATS_PER_GLYPH_VERTEX as u64
                    * std::mem::size_of::<f32>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&clip));
        self.num_vertices = needed as u32;
    }

    /// Draws this layer through `sheet`.
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, sheet: &Shared) {
        if self.num_vertices == 0 {
            return;
        }
        if let Some(sheet) = sheet.borrow().as_ref() {
            pass.set_pipeline(&sheet.pipeline);
            pass.set_bind_group(0, &sheet.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..self.num_vertices, 0..1);
        }
    }
}

/// **Which sheet a [`Sheet`] copies**: the glyphs' coverage or the
/// pictures' colors -- the same shelf-packed sheet on the card, read by a
/// pipeline of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SheetKind {
    /// The glyph atlas: one coverage byte a texel, sampled nearest.
    #[cfg(feature = "font-atlas")]
    Glyphs,
    /// The pictures: four bytes a texel, sampled linearly, since a picture
    /// is drawn at any size.
    Images,
}

impl SheetKind {
    fn label(self) -> &'static str {
        match self {
            #[cfg(feature = "font-atlas")]
            SheetKind::Glyphs => "glyph atlas",
            SheetKind::Images => "picture sheet",
        }
    }

    fn format(self) -> wgpu::TextureFormat {
        match self {
            // One coverage byte per texel: the smallest thing WebGL2 and
            // WebGPU both sample, which is what a glyph sheet is.
            #[cfg(feature = "font-atlas")]
            SheetKind::Glyphs => wgpu::TextureFormat::R8Unorm,
            SheetKind::Images => wgpu::TextureFormat::Rgba8Unorm,
        }
    }

    /// Bytes a texel, and texels a row.
    fn texel(self) -> u32 {
        match self {
            #[cfg(feature = "font-atlas")]
            SheetKind::Glyphs => 1,
            SheetKind::Images => clausters_gui_draw::image::TEXEL,
        }
    }

    fn side(self) -> u32 {
        match self {
            #[cfg(feature = "font-atlas")]
            SheetKind::Glyphs => clausters_gui_draw::font::atlas::SIDE,
            SheetKind::Images => clausters_gui_draw::image::SIDE,
        }
    }

    fn shader(self) -> &'static str {
        match self {
            #[cfg(feature = "font-atlas")]
            SheetKind::Glyphs => include_str!("paint_text.wgsl"),
            SheetKind::Images => include_str!("paint_image.wgsl"),
        }
    }

    fn filter(self) -> wgpu::FilterMode {
        match self {
            // a glyph is rasterized at the size it draws at and its quad is
            // snapped to whole pixels, so every texel lands on its own pixel
            #[cfg(feature = "font-atlas")]
            SheetKind::Glyphs => wgpu::FilterMode::Nearest,
            SheetKind::Images => wgpu::FilterMode::Linear,
        }
    }
}

/// **What a sheet is copied from**: its texels and the counters that say
/// what moved since the last copy.
struct SheetSource<'a> {
    pixels: &'a [u8],
    rows: u32,
    used_rows: u32,
    shelf_top: u32,
    version: u64,
    epoch: u64,
}

#[cfg(feature = "font-atlas")]
impl<'a> From<&'a clausters_gui_draw::font::atlas::Atlas> for SheetSource<'a> {
    fn from(a: &'a clausters_gui_draw::font::atlas::Atlas) -> Self {
        Self {
            pixels: a.pixels(),
            rows: a.rows(),
            used_rows: a.used_rows(),
            shelf_top: a.shelf_top(),
            version: a.version(),
            epoch: a.epoch(),
        }
    }
}

impl<'a> From<&'a clausters_gui_draw::image::ImageAtlas> for SheetSource<'a> {
    fn from(a: &'a clausters_gui_draw::image::ImageAtlas) -> Self {
        Self {
            pixels: a.pixels(),
            rows: a.rows(),
            used_rows: a.used_rows(),
            shelf_top: a.shelf_top(),
            version: a.version(),
            epoch: a.epoch(),
        }
    }
}

/// **A window's sheet**: the textured pipeline and the window's copy of the
/// glyph atlas or of the pictures, one for all of its batches.
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
struct Sheet {
    kind: SheetKind,
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

impl Sheet {
    /// A texture of `rows` rows and the bind group that samples it.
    fn texture(
        device: &wgpu::Device,
        kind: SheetKind,
        bind_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        rows: u32,
    ) -> (wgpu::Texture, wgpu::BindGroup) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(kind.label()),
            size: wgpu::Extent3d {
                width: kind.side(),
                height: rows.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: kind.format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(kind.label()),
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

    fn new(device: &wgpu::Device, kind: SheetKind, target: crate::view::Target, rows: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(kind.label()),
            source: wgpu::ShaderSource::Wgsl(kind.shader().into()),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(kind.label()),
            mag_filter: kind.filter(),
            min_filter: kind.filter(),
            ..Default::default()
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(kind.label()),
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
            label: Some(kind.label()),
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
            label: Some(kind.label()),
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
        let (texture, bind_group) = Self::texture(device, kind, &bind_layout, &sampler, rows);
        Self {
            kind,
            pipeline,
            bind_layout,
            sampler,
            texture,
            bind_group,
            rows,
            // 0 is "nothing uploaded": the atlas starts at 0 too and only
            // counts up once it holds something, so the first sheet is copied.
            version: 0,
            epoch: 0,
            shelf: 0,
        }
    }

    /// Brings this window's texture up to the shared sheet, if it is behind
    /// it: made again at the sheet's height where that grew, and the rows
    /// written since the last sync copied in -- all of them for a sheet that
    /// was made or packed again.
    fn sync(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, atlas: SheetSource<'_>) {
        if self.version == atlas.version || atlas.pixels.is_empty() {
            return;
        }
        let row = self.kind.side() * self.kind.texel();
        let mut from = self.shelf;
        if atlas.rows != self.rows {
            (self.texture, self.bind_group) = Self::texture(
                device,
                self.kind,
                &self.bind_layout,
                &self.sampler,
                atlas.rows,
            );
            self.rows = atlas.rows;
            from = 0;
        }
        if self.epoch != atlas.epoch {
            from = 0;
        }
        let to = atlas.used_rows;
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
                &atlas.pixels[(from * row) as usize..(to * row) as usize],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(to - from),
                },
                wgpu::Extent3d {
                    width: self.kind.side(),
                    height: to - from,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.version = atlas.version;
        self.epoch = atlas.epoch;
        self.shelf = atlas.shelf_top;
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
            pictures: None,
            picture_sheet: Default::default(),
            target,
        }
    }

    /// A painter of the same window as `other`: its own vertices, and
    /// `other`'s glyph sheet -- one texture and one textured pipeline for
    /// every batch a window draws.
    pub fn sharing(device: &wgpu::Device, target: crate::view::Target, other: &Painter) -> Self {
        let mut painter = Self::new(device, target);
        #[cfg(feature = "font-atlas")]
        {
            painter.sheet = other.sheet.clone();
        }
        painter.picture_sheet = other.picture_sheet.clone();
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
        self.upload_pictures(device, queue, mesh, origin, fw, fh);
        let pictures = mesh.image_vertices().len();
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
        (clip.len() + glyphs + pictures) * std::mem::size_of::<f32>()
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
        origin: (f32, f32),
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
        self.text
            .get_or_insert_with(|| Layer::new(device))
            .upload(device, queue, verts, origin, fw, fh);
        // The window's sheet, made on the first letter any of its batches
        // carries, and brought up to what the cache has rasterized since.
        let target = self.target;
        clausters_gui_draw::font::atlas::with(|atlas| {
            self.sheet
                .borrow_mut()
                .get_or_insert_with(|| Sheet::new(device, SheetKind::Glyphs, target, atlas.rows()))
                .sync(device, queue, SheetSource::from(&*atlas))
        });
    }

    /// The pictures of the same batch: their vertices, and the sheet of
    /// pictures brought up to what was packed since.
    fn upload_pictures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &Mesh,
        origin: (f32, f32),
        fw: f32,
        fh: f32,
    ) {
        let verts = mesh.image_vertices();
        if verts.is_empty() {
            if let Some(pictures) = &mut self.pictures {
                pictures.num_vertices = 0;
            }
            return;
        }
        self.pictures
            .get_or_insert_with(|| Layer::new(device))
            .upload(device, queue, verts, origin, fw, fh);
        let target = self.target;
        clausters_gui_draw::image::with(|atlas| {
            let source: SheetSource<'_> = (&*atlas).into();
            self.picture_sheet
                .borrow_mut()
                .get_or_insert_with(|| Sheet::new(device, SheetKind::Images, target, source.rows))
                .sync(device, queue, source)
        });
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.num_vertices > 0 {
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..self.num_vertices, 0..1);
        }
        // the pictures of this batch over its flat geometry, and its glyphs
        // over both: one bind group and one draw each for the whole window
        if let Some(pictures) = &self.pictures {
            pictures.draw(pass, &self.picture_sheet);
        }
        #[cfg(feature = "font-atlas")]
        if let Some(text) = &self.text {
            text.draw(pass, &self.sheet);
        }
    }
}
