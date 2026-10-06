//! Spectrogram view: an STFT analysis (the cache) and a GPU renderer that maps
//! it through `viewport::View`, reusing the same navigation as the waveform.
//!
//! The expensive part is the one-time STFT, treated as a cache exactly like the
//! peak pyramid: it lives in memory and serializes to/from a flat, mmap-friendly
//! buffer or file. Rendering is then constant-cost regardless of zoom - a single
//! full-screen quad samples the magnitude texture, and `View` only changes which
//! horizontal (time) slice of the texture is sampled. The GPU's linear filtering
//! gives resolution-matched down-sampling when zoomed out, so we never draw more
//! than the screen needs.

pub use clausters_core::stft::{
    REF_FLOOR, Stft, analysis_window, column_into, hop_capped, max_frames, quantize,
};

use crate::view::{Framing, Renderers, TimelineView};
use crate::viewport::{Axis, Unit, View};

/// The widest magnitude texture the renderer uploads -- the WebGL2/WebGPU
/// baseline `max_texture_dimension_2d`. It bounds what is **on the card at
/// once**, not what a transform holds: a stored transform keeps every column
/// its analysis made, and its view uploads the stretch it is showing at the
/// level of detail the screen can show ([`SpectrogramView`]).
pub const MAX_FRAMES: usize = 8192;

/// The most columns a **rolling** transform ([`Stft::rolling`]) retains -- half
/// [`MAX_FRAMES`], because a ring is stored twice in one texture (see
/// [`Stft::tex_width`]) and the pair still has to fit the same dimension.
pub const MAX_ROLLING_FRAMES: usize = MAX_FRAMES / 2;

/// Frequency axis mapping for the spectrogram's vertical axis. Beyond the
/// classic linear/log pair, the two perceptual scales (mel and bark) map the
/// display coordinate through the shared closed forms in
/// `clausters_core::scale` -- the shader carries the identical formulas.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FreqScale {
    Linear,
    Log,
    Mel,
    Bark,
}

impl FreqScale {
    /// The scale's shader index (the `freq.z` uniform).
    pub(crate) fn index(self) -> u32 {
        match self {
            FreqScale::Linear => 0,
            FreqScale::Log => 1,
            FreqScale::Mel => 2,
            FreqScale::Bark => 3,
        }
    }

    /// The next scale in the cycling order (the `L` key).
    pub(crate) fn next(self) -> FreqScale {
        match self {
            FreqScale::Linear => FreqScale::Log,
            FreqScale::Log => FreqScale::Mel,
            FreqScale::Mel => FreqScale::Bark,
            FreqScale::Bark => FreqScale::Linear,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    /// x = start_frac, y = len_frac of the visible time window; z = the
    /// Nyquist frequency in Hz (the mel/bark mappings need the absolute axis).
    time: [f32; 4],
    /// x = d0, y = d1 of the visible frequency window in display coordinates
    /// [0, 1]; z = the frequency-scale index (0 linear, 1 log, 2 mel,
    /// 3 bark); w = normalized log-axis floor.
    freq: [f32; 4],
    /// x = lo_frac, y = hi_frac of the display dB window within the stored
    /// reference range (the colour scale); z = colormap index; w = the layer's
    /// own alpha.
    db: [f32; 4],
    /// xy = scale, zw = offset of the [`Framing`] that places the picture
    /// inside its viewport -- the identity for a view the window shows whole.
    rect: [f32; 4],
}

/// GPU renderer for spectrograms: the pipeline that samples a magnitude texture
/// over a full-screen quad, plus the bind-group layout its textures are built
/// against.
///
/// **One of these serves a whole window** -- see [`Renderers`]. It carries
/// nothing about any particular analysis; a spectrogram element's own state is
/// a [`SpectrogramTexture`]. The split matters most here, because a
/// spectrogram builds one view *per channel*: an eight-channel analysis used to
/// compile eight shader modules and eight pipelines to draw eight textures that
/// differ only in their contents.
///
/// [`Renderers`]: crate::view::Renderers
pub struct SpectrogramRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

/// One spectrogram channel's GPU state: the magnitude texture, its sampler and
/// the uniforms that place the visible time/frequency window and dB scale. Drawn
/// through the window's shared [`SpectrogramRenderer`].
pub struct SpectrogramTexture {
    /// Kept so a rolling ring can write its new columns into the texture it
    /// already has, instead of building a new one per tick.
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
}

impl SpectrogramTexture {
    /// A magnitude texture of `width` x `height` texels, bound against
    /// `renderer`'s layout, with nothing written into it yet.
    ///
    /// Width is time and height is frequency: row 0 is bin 0 (low frequency),
    /// and the shader flips y so low frequencies sit at the bottom. R8Unorm is
    /// used (not R32Float) because single-channel 32-bit float is not
    /// linearly *filterable* without an optional GPU feature, whereas R8Unorm
    /// is filterable everywhere (including WebGPU) and is a quarter the size;
    /// the magnitudes are already normalized to [0, 1], so 8 bits are ample
    /// for the colormap.
    fn sized(
        device: &wgpu::Device,
        renderer: &SpectrogramRenderer,
        width: usize,
        height: usize,
    ) -> Self {
        let size = wgpu::Extent3d {
            width: width.max(1) as u32,
            height: height.max(1) as u32,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("spectrogram texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("spectrogram sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spectrogram uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spectrogram bg"),
            layout: &renderer.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        Self {
            texture,
            bind_group,
            uniform_buffer,
        }
    }

    /// Writes the whole texture from `texels`, row-major by bin (`height`
    /// rows of `width`). `write_texture` (unlike a buffer copy) does not
    /// require 256-byte row alignment, so the tight `width`-byte rows are fine.
    fn write_all(&self, queue: &wgpu::Queue, texels: &[u8], width: usize, height: usize) {
        let size = wgpu::Extent3d {
            width: width.max(1) as u32,
            height: height.max(1) as u32,
            depth_or_array_layers: 1,
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width),
                rows_per_image: Some(size.height),
            },
            size,
        );
    }

    /// Uploads **the whole of** `stft` as a texture and binds it against
    /// `renderer`'s layout: what a rolling ring is drawn from, and a stored
    /// transform no wider than a texture.
    ///
    /// The width is the *texture's*, not the frame count: a rolling ring is
    /// stored twice so its visible window never wraps (see `tex_width`). For
    /// a stored transform the two are the same number.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &SpectrogramRenderer,
        stft: &Stft,
    ) -> Self {
        let (w, h) = (stft.tex_width().max(1), stft.n_bins().max(1));
        let texture = Self::sized(device, renderer, w, h);
        // The texture is row-major by frequency bin (height rows), but a column
        // is a run of bins, so transpose into a [bin][frame] upload buffer and
        // quantize to u8.
        //
        // Columns are placed by `texel_of`, and a rolling one is written twice
        // (once in each copy of the ring); a stored transform's column `f` lands
        // at texel `f`, so it uploads exactly the bytes it always did.
        let mut transposed = vec![0u8; w * h];
        for f in 0..stft.n_frames() {
            let col = stft.column(f);
            let slot = stft.texel_of(f);
            for (b, m) in col.iter().enumerate().take(h) {
                let q = quantize(*m);
                transposed[b * w + slot] = q;
                if stft.capacity() > 0 {
                    transposed[b * w + slot + stft.capacity()] = q;
                }
            }
        }
        texture.write_all(queue, &transposed, w, h);
        texture
    }

    /// Writes one already-quantized column into texel column `texel`, and into
    /// its mirror `capacity` texels to the right when the transform is rolling.
    ///
    /// This is the whole point of the ring: a landing column costs `n_bins`
    /// bytes twice, where rebuilding the picture costs the span -- 384 KB a tick
    /// for an eight-second waterfall, and eight times that for a minute of it.
    fn write_column(&self, queue: &wgpu::Queue, texel: usize, capacity: usize, col: &[u8]) {
        let size = wgpu::Extent3d {
            width: 1,
            height: col.len() as u32,
            depth_or_array_layers: 1,
        };
        let layout = wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(1),
            rows_per_image: Some(col.len() as u32),
        };
        for x in [texel, texel + capacity] {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: x as u32,
                        y: 0,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                col,
                layout,
                size,
            );
        }
    }

    fn write_uniforms(&self, queue: &wgpu::Queue, u: &Uniforms) {
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(u));
    }
}

impl SpectrogramRenderer {
    pub fn new(device: &wgpu::Device, target: crate::view::Target) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spectrogram bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    // The vertex stage reads it too: the framing that places
                    // the picture inside its viewport rides these uniforms.
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("spectrogram shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("spectrogram.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("spectrogram pl"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("spectrogram pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target.format,
                    // **Alpha-blended, because a texture is a layer.** At the
                    // opaque default this is the same picture the unblended
                    // pipeline drew; under a stack it is what lets a
                    // spectrogram be read through, or read through what is
                    // over it.
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

        Self {
            pipeline,
            bind_group_layout,
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, tex: &SpectrogramTexture) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &tex.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// An `Stft` paired with its GPU texture and the display state (frequency window,
/// scale, dB window), satisfying [`TimelineView`].
/// **What of a stored transform is on the card**: a run of columns of one
/// level of its time pyramid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Held {
    /// The pyramid level: each column stands for `2^level` of the transform's.
    pub level: usize,
    /// The first column of that level in the texture, and how many follow.
    pub first: usize,
    pub count: usize,
}

/// **Which columns a view of `visible` level-0 frames, `width_px` wide, has
/// to have on the card**: the level whose columns are about one to the
/// pixel -- never finer than the screen, and coarse enough that the stretch
/// on screen fits a texture -- and the run of it that covers the view with as
/// much again on either side, so a pan redraws from what is there.
///
/// `start` is the first visible level-0 frame (it may be negative, or past
/// the end: a view is not clipped to its data). `levels` is how many the
/// pyramid has above the transform, and `frames_at` how many columns each has.
pub fn hold_for(
    start: f64,
    visible: f64,
    width_px: u32,
    levels: usize,
    frames_at: impl Fn(usize) -> usize,
) -> Held {
    let visible = visible.max(1.0);
    let per_px = visible / f64::from(width_px.max(1));
    // Between one and two columns to the pixel, where the pyramid has it.
    let mut level = if per_px > 1.0 {
        (per_px.log2().floor() as usize).min(levels)
    } else {
        0
    };
    // And never more on screen than half a texture, so the margins fit.
    while level < levels && visible / (1u64 << level) as f64 > (MAX_FRAMES / 2) as f64 {
        level += 1;
    }
    let scale = (1u64 << level) as f64;
    let n = frames_at(level).max(1);
    let a = ((start / scale).floor().max(0.0) as usize).min(n - 1);
    let b = (((start + visible) / scale).ceil().max(0.0) as usize + 1).clamp(a + 1, n);
    let pad = (b - a).min((MAX_FRAMES.saturating_sub(b - a)) / 2);
    let first = a.saturating_sub(pad);
    let count = ((b + pad).min(n) - first).min(MAX_FRAMES);
    Held {
        level,
        first,
        count,
    }
}

pub struct SpectrogramView {
    stft: Stft,
    texture: SpectrogramTexture,
    /// What of a **stored** transform the texture holds; `None` for a rolling
    /// one, whose texture is its whole ring, and before the first frame.
    held: Option<Held>,
    /// The vertical display axis: the visible slice of the frequency display
    /// coordinate, normalized (`0, 1` = the whole axis).
    freq: Axis,
    scale: FreqScale,
    db_floor: f32,
    db_ceil: f32,
    /// 0 = viridis, 1 = magma, 2 = grayscale.
    colormap: u32,
    /// The weight this texture draws at as one layer of a stack; `1` alone on
    /// its body.
    alpha: f32,
    /// The frequency window's start, snapshotted for absolute drag panning.
    drag_freq_start: f64,
    /// Where this view's picture sits inside the viewport it is drawn with.
    framing: Framing,
    /// The quantized column a rolling push uploads through -- held so a landing
    /// column allocates nothing.
    scratch: Vec<u8>,
}

impl SpectrogramView {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &SpectrogramRenderer,
        mut stft: Stft,
    ) -> Self {
        // A rolling ring is on the card whole, and written a column at a
        // time. A stored transform is held by the stretch a frame shows
        // ([`Self::hold`]), so all it needs here is somewhere to bind.
        let texture = if stft.is_rolling() {
            SpectrogramTexture::new(device, queue, renderer, &stft)
        } else {
            stft.build_pyramid();
            SpectrogramTexture::sized(device, renderer, 1, stft.n_bins())
        };
        Self {
            stft,
            texture,
            held: None,
            freq: Axis::normalized(Unit::Hz),
            scale: FreqScale::Log,
            db_floor: -90.0,
            db_ceil: 0.0,
            colormap: 0,
            alpha: 1.0,
            drag_freq_start: 0.0,
            framing: Framing::IDENTITY,
            scratch: Vec::new(),
        }
    }

    /// An empty **retained live** view: a ring of `capacity` columns and the
    /// texture behind it, both allocated once. Columns arrive through
    /// [`push_columns`](SpectrogramView::push_columns).
    pub fn rolling(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &SpectrogramRenderer,
        capacity: usize,
        window_size: usize,
        hop: usize,
        sample_rate: f32,
    ) -> Self {
        let stft = Stft::rolling(capacity, window_size / 2, hop, window_size, sample_rate);
        Self::new(device, queue, renderer, stft)
    }

    /// Pushes newly analyzed columns (frame-major, `n_bins` each) into the ring
    /// and writes **only those texels**. The texture, the bind group and the
    /// analysis all survive the tick.
    pub fn push_columns(&mut self, queue: &wgpu::Queue, columns: &[f32]) {
        let n_bins = self.stft.n_bins();
        let capacity = self.stft.capacity();
        if n_bins == 0 || capacity == 0 {
            return;
        }
        self.scratch.resize(n_bins, 0);
        for col in columns.chunks_exact(n_bins) {
            let texel = self.stft.push_column(col);
            for (o, m) in self.scratch.iter_mut().zip(col) {
                *o = (m.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            self.texture
                .write_column(queue, texel, capacity, &self.scratch);
        }
    }

    /// Follows a live change of the retained span. The ring keeps its newest
    /// columns and the texture is rebuilt around them -- one full upload per
    /// `retention` change, against one per tick before the ring existed.
    pub fn set_retention(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &SpectrogramRenderer,
        capacity: usize,
    ) {
        let wanted = capacity.clamp(1, MAX_ROLLING_FRAMES);
        if !self.stft.is_rolling() || self.stft.capacity() == wanted {
            return;
        }
        self.stft.set_capacity(wanted);
        self.texture = SpectrogramTexture::new(device, queue, renderer, &self.stft);
    }

    /// The analysis this view draws (e.g. for a frequency ruler's Nyquist).
    pub fn stft(&self) -> &Stft {
        &self.stft
    }

    /// Sets the display state from widget props: the dB window (contrast), the
    /// frequency-axis scale and the colormap (0 = viridis, 1 = magma, 2 =
    /// grayscale). Cheap -- everything lands in the shader uniforms, so a live
    /// `/gui_set` retunes the view with zero recompute.
    /// **The weight this texture is drawn at** as one layer of a stack, in
    /// `[0, 1]` -- a uniform write, like every other display control here.
    pub fn set_alpha(&mut self, alpha: f32) {
        self.alpha = alpha.clamp(0.0, 1.0);
    }

    pub fn set_display(&mut self, db_floor: f32, db_ceil: f32, scale: FreqScale, colormap: u32) {
        self.db_floor = db_floor;
        self.db_ceil = db_ceil;
        self.scale = scale;
        self.colormap = colormap % 3;
    }

    /// The normalized bottom of the log frequency axis (~20 Hz / Nyquist) -- the
    /// same `f_lo` the shader's display->bin mapping uses, exposed so a ruler
    /// places its ticks with the identical geometry.
    pub fn log_floor(&self) -> f32 {
        (20.0 / self.stft.nyquist()).clamp(1e-5, 0.5)
    }

    /// Sets the visible frequency window from normalized display coordinates
    /// (`start, len` with `0, 1` = the full axis; clamped) -- the live
    /// `y_start`/`y_len` props of the editor-grade widget. The internal view
    /// keeps the display-coordinate convention (scaled by `n_bins`), so the
    /// shader's display->bin mapping is untouched.
    pub fn set_freq_window(&mut self, start: f64, len: f64) {
        self.freq.set_span(start, len);
    }

    /// Sets where the picture sits inside the viewport it is drawn with (see
    /// [`Framing`]) -- a shader uniform, so a view cut by the window edge costs
    /// nothing extra to draw.
    pub fn set_framing(&mut self, framing: Framing) {
        self.framing = framing;
    }

    /// **Brings onto the card what `view` shows**, `width_px` wide: the run of
    /// columns [`hold_for`] names, uploaded when the texture does not already
    /// cover it at that level. A pan inside the margins and a frame that
    /// moved nothing upload nothing.
    ///
    /// This is what lets a transform be longer than a texture is wide, and
    /// what keeps a zoomed-out picture honest: the columns drawn are the
    /// pyramid's, each the largest of the frames it stands for, where one
    /// texture of every frame left a sampler to pick two of ten.
    fn hold(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &SpectrogramRenderer,
        view: &View,
        width_px: u32,
    ) {
        if self.stft.is_rolling() {
            return;
        }
        let hop = self.stft.hop().max(1) as f64;
        let want = hold_for(
            view.start / hop,
            view.len / hop,
            width_px,
            self.stft.levels(),
            |level| self.stft.level_frames(level),
        );
        let covered = self.held.is_some_and(|held| {
            // What the view needs at this level, without the margins.
            let scale = (1u64 << want.level) as f64;
            let n = self.stft.level_frames(want.level).max(1);
            let a = ((view.start / hop / scale).floor().max(0.0) as usize).min(n - 1);
            let b = (((view.start + view.len) / hop / scale).ceil().max(0.0) as usize + 1)
                .clamp(a + 1, n);
            held.level == want.level && held.first <= a && b <= held.first + held.count
        });
        if covered {
            return;
        }
        let (w, h) = (want.count.max(1), self.stft.n_bins().max(1));
        if self.held.is_none_or(|held| held.count != want.count) {
            self.texture = SpectrogramTexture::sized(device, renderer, w, h);
        }
        // Row-major by bin, a column a run of bins: transposed as it is
        // quantized, through the one scratch column.
        let mut texels = vec![0u8; w * h];
        self.scratch.resize(h, 0);
        for x in 0..want.count {
            self.stft
                .level_column(want.level, want.first + x, &mut self.scratch);
            for (b, q) in self.scratch.iter().enumerate() {
                texels[b * w + x] = *q;
            }
        }
        self.texture.write_all(queue, &texels, w, h);
        self.held = Some(want);
    }

    /// What of the transform is on the card (see [`Held`]).
    pub fn held(&self) -> Option<Held> {
        self.held
    }

    /// The visible sample range as a normalized `[start, start+len]` across
    /// the **texture**: over what is held of a stored transform, and over the
    /// ring of a rolling one ([`Stft::time_fraction`]).
    fn time_fraction(&self, view: &View) -> (f32, f32) {
        match self.held {
            Some(held) if !self.stft.is_rolling() => {
                let scale = self.stft.hop().max(1) as f64 * (1u64 << held.level) as f64;
                let width = held.count.max(1) as f64;
                (
                    ((view.start / scale - held.first as f64) / width) as f32,
                    (view.len / scale / width) as f32,
                )
            }
            _ => self.stft.time_fraction(view.start, view.len),
        }
    }

    /// Build the GPU uniforms from the current time `view` and display state.
    ///
    /// The frequency window is expressed in *display* coordinates `[0, 1]`
    /// (the screen's vertical axis), not in bins. The linear/log mapping from
    /// that display coordinate to a normalized bin happens in the shader over
    /// the full axis, so zoom/pan use a plain linear screen anchor and the
    /// point under the cursor stays fixed in both modes.
    fn uniforms(&self, view: &View) -> Uniforms {
        let (start, len) = self.time_fraction(view);

        let (d0, d1) = (
            self.freq.start() as f32,
            (self.freq.start() + self.freq.len()) as f32,
        );
        // Bottom of the log axis (~20 Hz), normalized to Nyquist.
        let f_lo = (20.0 / self.stft.nyquist()).clamp(1e-5, 0.5);

        let span = -REF_FLOOR;
        let lo = ((self.db_floor - REF_FLOOR) / span).clamp(0.0, 1.0);
        let hi = ((self.db_ceil - REF_FLOOR) / span).clamp(0.0, 1.0);

        Uniforms {
            time: [start, len, self.stft.nyquist(), 0.0],
            freq: [d0, d1, self.scale.index() as f32, f_lo],
            db: [lo, hi, self.colormap as f32, self.alpha],
            rect: [
                self.framing.scale[0],
                self.framing.scale[1],
                self.framing.offset[0],
                self.framing.offset[1],
            ],
        }
    }
}

impl TimelineView for SpectrogramView {
    fn total_samples(&self) -> usize {
        self.stft.total_samples()
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderers: &mut Renderers,
        view: &View,
        render_width_px: u32,
    ) {
        self.hold(device, queue, &renderers.spectrogram, view, render_width_px);
        let u = self.uniforms(view);
        self.texture.write_uniforms(queue, &u);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, renderers: &Renderers) {
        renderers.spectrogram.draw(pass, &self.texture);
    }

    /// `L` cycles the frequency scale (linear -> log -> mel -> bark); `[` / `]`
    /// lower/raise the dB floor (contrast); `/` cycles the colormap.
    fn on_char(&mut self, c: char) -> bool {
        match c {
            'l' | 'L' => {
                self.scale = self.scale.next();
                true
            }
            '[' => {
                self.db_floor = (self.db_floor - 5.0).max(REF_FLOOR + 5.0);
                true
            }
            ']' => {
                self.db_floor = (self.db_floor + 5.0).min(self.db_ceil - 5.0);
                true
            }
            '/' => {
                self.colormap = (self.colormap + 1) % 3;
                true
            }
            _ => false,
        }
    }

    fn on_vertical_zoom(&mut self, factor: f64, anchor: f64) -> bool {
        self.freq.zoom(factor, anchor);
        true
    }

    fn on_vertical_drag_begin(&mut self) {
        self.drag_freq_start = self.freq.start();
    }

    fn on_vertical_drag(&mut self, total: f64) -> bool {
        // Low frequency is at the bottom, so dragging down (total > 0) moves the
        // window down with the cursor. Absolute from the snapshot.
        self.freq
            .set_start(self.drag_freq_start + total * self.freq.len());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The analysis is tested where it lives (`clausters_core::stft`); here,
    // what a view puts on the card of it.

    /// **What is on the card is what the screen can show of what it shows.**
    /// Zoomed in on a transform longer than a texture, the view holds its own
    /// stretch of the transform's columns and as much again either side;
    /// zoomed out, a level with about a column to the pixel; and a short
    /// transform whole.
    #[test]
    fn a_view_holds_the_stretch_it_shows_at_the_screens_level() {
        // 56 250 frames, seven textures wide, with seven levels above them.
        let frames = 56_250usize;
        let at = |level: usize| {
            let mut n = frames;
            for _ in 0..level {
                n = n.div_ceil(2);
            }
            n
        };
        let levels = 7;
        // Zoomed in: 800 frames in 800 pixels, a minute into the file.
        let near = hold_for(5000.0, 800.0, 800, levels, at);
        assert_eq!(near.level, 0, "a column to the pixel: the transform's own");
        assert!(near.first <= 5000 && near.first + near.count >= 5801);
        assert!(
            near.count >= 2400 && near.count <= MAX_FRAMES,
            "with margins"
        );
        // Zoomed out on all of it: sixty frames to the pixel is level five,
        // where there are fewer than two.
        let far = hold_for(0.0, frames as f64, 900, levels, at);
        assert_eq!(far.level, 5);
        assert_eq!((far.first, far.count), (0, at(5)), "all of that level");
        assert!(far.count <= 2 * 900);
        // A view wider than a texture's half of level-0 frames is never held
        // at level 0, however many pixels it claims.
        let wide = hold_for(0.0, 6000.0, 8000, levels, at);
        assert!(wide.level >= 1 && wide.count <= MAX_FRAMES);
        // A short transform, shown whole or in part, is held whole.
        let short = hold_for(100.0, 200.0, 800, 0, |_| 500);
        assert_eq!((short.level, short.first, short.count), (0, 0, 500));
        // A view hanging off either end holds what there is.
        let past = hold_for(-50.0, 100.0, 800, 0, |_| 500);
        assert_eq!(past.first, 0);
        let beyond = hold_for(480.0, 100.0, 800, 0, |_| 500);
        assert_eq!(beyond.first + beyond.count, 500);
    }
}
