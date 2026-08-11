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

use clausters_core::{bytes, fft};

use crate::view::{Framing, Renderers, TimelineView};
use crate::viewport::{Axis, Unit, View};

const MAGIC: &[u8; 4] = b"CLSG";
const VERSION: u32 = 2;
/// Reference dB range the stored magnitudes are normalized over. The *display*
/// dB window (which controls contrast) is a cheap shader uniform within this
/// range, so it can change live without recomputing the STFT.
const REF_FLOOR: f32 = -120.0;

/// The widest magnitude texture the renderer uploads — the WebGL2/WebGPU
/// baseline `max_texture_dimension_2d`. [`hop_capped`] raises the hop so a
/// long buffer's frame count stays within it.
pub const MAX_FRAMES: usize = 8192;

/// The most columns a **rolling** transform ([`Stft::rolling`]) retains — half
/// [`MAX_FRAMES`], because a ring is stored twice in one texture (see
/// [`Stft::tex_width`]) and the pair still has to fit the same dimension.
pub const MAX_ROLLING_FRAMES: usize = MAX_FRAMES / 2;

/// The hop to analyze `total_samples` with: the requested `hop`, raised just
/// enough that the STFT yields at most [`MAX_FRAMES`] frames (one texture row
/// per frame). A long file thus trades time resolution for fitting the GPU
/// texture, instead of failing device validation.
pub fn hop_capped(total_samples: usize, window_size: usize, hop: usize) -> usize {
    let needed = total_samples
        .saturating_sub(window_size)
        .div_ceil(MAX_FRAMES.saturating_sub(1).max(1))
        .max(1);
    hop.max(needed)
}

/// Frequency axis mapping for the spectrogram's vertical axis. Beyond the
/// classic linear/log pair, the two perceptual scales (mel and bark) map the
/// display coordinate through the shared closed forms in
/// `clausters_core::scale` — the shader carries the identical formulas.
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

/// The analysis window and its coherent gain: a Hann window, and the sum that
/// normalizes a full-scale sine to about 0 dB. Computed once per transform, not
/// once per column.
pub fn analysis_window(window_size: usize) -> (Vec<f32>, f32) {
    let hann: Vec<f32> = (0..window_size)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / window_size as f32).cos())
        .collect();
    let gain = hann.iter().sum::<f32>() * 0.5;
    (hann, gain)
}

/// **One column of a spectrogram**: `frame` windowed, transformed, and mapped
/// to the normalized 0..1 magnitudes the texture stores.
///
/// It is a free function rather than a method because two paths produce
/// columns and they must produce the *same* ones: the stored transform
/// ([`Stft::compute`], analyzing a whole buffer at once) and the rolling one a
/// retained live view keeps (`host::waterfall`, analyzing a column at a time as
/// the samples arrive). A retained waterfall and an offline spectrogram of the
/// same audio are then the same picture, which is the only reason the renderer,
/// the frequency ruler and the cursor readout can stay one implementation.
///
/// `windowed` and `spectrum` are scratch the caller owns, so a rolling
/// analysis allocates nothing per column.
pub fn column_into(
    frame: &[f32],
    hann: &[f32],
    win_gain: f32,
    windowed: &mut [f32],
    spectrum: &mut [f32],
    out: &mut [f32],
) {
    for (i, w) in windowed.iter_mut().enumerate() {
        *w = frame.get(i).copied().unwrap_or(0.0) * hann[i];
    }
    // The forward FFT lives once in the shared core (`clausters_core::fft`).
    fft::rfft_magnitudes_into(windowed, spectrum);
    for (o, m) in out.iter_mut().zip(spectrum.iter()) {
        let db = 20.0 * (m / win_gain + 1e-9).log10();
        *o = ((db - REF_FLOOR) / -REF_FLOOR).clamp(0.0, 1.0);
    }
}

/// A short-time Fourier transform: `n_frames` x `n_bins` normalized magnitudes
/// in `[0, 1]` (dB mapped from `[DB_FLOOR, 0]`), row-major by frame. Frame `f`
/// is centred on samples starting at `f * hop`.
///
/// A transform is **stored** or **rolling**. A stored one is analyzed once and
/// its columns are exactly the ones it holds. A rolling one ([`Stft::rolling`])
/// is a fixed-capacity ring a live view pushes into, one column per hop, the
/// oldest falling off the front — the same magnitudes in the same order, read
/// through [`Stft::column`] instead of straight off `mags`.
pub struct Stft {
    total_samples: usize,
    n_frames: usize,
    n_bins: usize,
    hop: usize,
    window_size: usize,
    sample_rate: f32,
    mags: Vec<f32>,
    /// Ring capacity in columns, or `0` for a stored transform. A rolling
    /// transform's `mags` is always `capacity * n_bins` long, however few
    /// columns have landed.
    capacity: usize,
    /// Ring index of the oldest retained column (always `0` when stored).
    head: usize,
}

impl Stft {
    /// Compute the STFT of mono `samples`. `window_size` must be a power of two;
    /// `hop` is the frame advance (e.g. `window_size / 2`); `sample_rate` is used
    /// for the frequency axis.
    pub fn compute(samples: &[f32], window_size: usize, hop: usize, sample_rate: f32) -> Self {
        assert!(
            fft::supports(window_size) && hop >= 1,
            "window_size must be a supported FFT size {:?}",
            fft::SUPPORTED_SIZES
        );
        let total_samples = samples.len();
        let n_bins = window_size / 2;
        let n_frames = if total_samples < window_size {
            1
        } else {
            1 + (total_samples - window_size) / hop
        };

        let (hann, win_gain) = analysis_window(window_size);
        let mut mags = vec![0.0f32; n_frames * n_bins];
        let mut windowed = vec![0.0f32; window_size];
        let mut spectrum = vec![0.0f32; n_bins]; // n_bins == window_size / 2
        for f in 0..n_frames {
            let start = f * hop;
            let frame: Vec<f32> = (0..window_size)
                .map(|i| samples.get(start + i).copied().unwrap_or(0.0))
                .collect();
            column_into(
                &frame,
                &hann,
                win_gain,
                &mut windowed,
                &mut spectrum,
                &mut mags[f * n_bins..(f + 1) * n_bins],
            );
        }

        Self {
            total_samples,
            n_frames,
            n_bins,
            hop,
            window_size,
            sample_rate,
            mags,
            capacity: 0,
            head: 0,
        }
    }

    /// An empty **rolling** transform: a ring of `capacity` columns a retained
    /// live view pushes into ([`push_column`]), the oldest falling off the front.
    ///
    /// What needs it is that a live picture is not analyzed at a moment: a
    /// retained view adds one column per hop, and recomputing the whole
    /// transform each tick would redo hundreds of FFTs to learn what one of them
    /// says — and, worse, re-upload the whole texture to show it. The ring is
    /// what makes both costs follow the *hop* instead of the span: a landing
    /// column is one FFT and one texel write.
    ///
    /// `capacity` is clamped to [`MAX_ROLLING_FRAMES`].
    ///
    /// [`push_column`]: Stft::push_column
    pub fn rolling(
        capacity: usize,
        n_bins: usize,
        hop: usize,
        window_size: usize,
        sample_rate: f32,
    ) -> Self {
        let capacity = capacity.clamp(1, MAX_ROLLING_FRAMES);
        Stft {
            total_samples: window_size,
            n_frames: 0,
            n_bins,
            hop: hop.max(1),
            window_size,
            sample_rate,
            mags: vec![0.0; capacity * n_bins],
            capacity,
            head: 0,
        }
    }

    /// Nyquist frequency in Hz (the top of the frequency axis).
    pub fn nyquist(&self) -> f32 {
        self.sample_rate * 0.5
    }

    pub fn total_samples(&self) -> usize {
        self.total_samples
    }
    pub fn n_frames(&self) -> usize {
        self.n_frames
    }
    pub fn n_bins(&self) -> usize {
        self.n_bins
    }
    pub fn hop(&self) -> usize {
        self.hop
    }
    pub fn window_size(&self) -> usize {
        self.window_size
    }
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }
    /// The ring's capacity in columns, or `0` when the transform is stored.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    /// Whether this is a ring a live view pushes into rather than a stored
    /// analysis.
    pub fn is_rolling(&self) -> bool {
        self.capacity > 0
    }
    /// The magnitudes as they sit in memory — frame-major for a stored
    /// transform, and in *ring* order (rotated by `head`) for a rolling one, so
    /// a caller that wants columns in time order asks [`Stft::column`] instead.
    pub fn magnitudes(&self) -> &[f32] {
        &self.mags
    }

    /// Logical column `i` (0 = the oldest retained), whichever texel it lives in.
    pub fn column(&self, i: usize) -> &[f32] {
        let at = self.texel_of(i) * self.n_bins;
        &self.mags[at..at + self.n_bins]
    }

    /// The texture column logical column `i` occupies.
    fn texel_of(&self, i: usize) -> usize {
        if self.capacity > 0 {
            (self.head + i) % self.capacity
        } else {
            i
        }
    }

    /// The width of the magnitude texture this transform is drawn from.
    ///
    /// A rolling ring is stored **twice**, back to back, which is what keeps the
    /// visible window one contiguous run of texels however far the write cursor
    /// has wrapped. The alternative is wrapping in the shader, and a linear
    /// sample across the seam blends the newest column into the oldest — a
    /// visible stripe travelling through the picture. The doubled width is why
    /// [`MAX_ROLLING_FRAMES`] is half [`MAX_FRAMES`].
    pub fn tex_width(&self) -> usize {
        if self.capacity > 0 {
            self.capacity * 2
        } else {
            self.n_frames.max(1)
        }
    }

    /// Appends one analyzed column, dropping the oldest once the ring is full.
    /// Returns the texel column it landed in; its mirror sits `capacity` texels
    /// to the right. A no-op (returning 0) on a stored transform.
    pub fn push_column(&mut self, col: &[f32]) -> usize {
        if self.capacity == 0 {
            return 0;
        }
        let slot = (self.head + self.n_frames) % self.capacity;
        let at = slot * self.n_bins;
        let n = self.n_bins.min(col.len());
        self.mags[at..at + n].copy_from_slice(&col[..n]);
        self.mags[at + n..at + self.n_bins].fill(0.0);
        if self.n_frames < self.capacity {
            self.n_frames += 1;
        } else {
            self.head = (self.head + 1) % self.capacity;
        }
        self.total_samples = self.n_frames.saturating_sub(1) * self.hop + self.window_size;
        slot
    }

    /// Resizes the ring to `capacity` columns, keeping the newest ones. The ring
    /// comes back unrotated (`head` 0), so the caller reallocates the texture and
    /// re-uploads — this is the live `retention` change, not a per-tick cost.
    pub fn set_capacity(&mut self, capacity: usize) {
        let capacity = capacity.clamp(1, MAX_ROLLING_FRAMES);
        if self.capacity == 0 || capacity == self.capacity {
            return;
        }
        let keep = self.n_frames.min(capacity);
        let first = self.n_frames - keep;
        let mut mags = vec![0.0f32; capacity * self.n_bins];
        for i in 0..keep {
            let at = i * self.n_bins;
            mags[at..at + self.n_bins].copy_from_slice(self.column(first + i));
        }
        self.mags = mags;
        self.capacity = capacity;
        self.head = 0;
        self.n_frames = keep;
        self.total_samples = keep.saturating_sub(1) * self.hop + self.window_size;
    }

    /// The visible sample range as a normalized horizontal `[start, start+len]`
    /// across the texture's frame axis, for the renderer's uniform. A rolling
    /// ring measures from its `head` over the doubled width, which is the whole
    /// difference between the two forms as far as the shader is concerned — a
    /// stored transform has `head` 0 and a width of its own frame count, so this
    /// is the plain fraction it always was.
    fn time_fraction(&self, view: &View) -> (f32, f32) {
        let width = self.tex_width() as f64;
        let start = (self.head as f64 + view.start / self.hop as f64) / width;
        let len = (view.len / self.hop as f64) / width;
        (start as f32, len as f32)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        bytes::push_u32(&mut out, VERSION);
        bytes::push_u64(&mut out, self.total_samples);
        bytes::push_u64(&mut out, self.n_frames);
        bytes::push_u64(&mut out, self.n_bins);
        bytes::push_u64(&mut out, self.hop);
        bytes::push_u64(&mut out, self.window_size);
        bytes::push_u32(&mut out, self.sample_rate.to_bits());
        bytes::push_f32s(&mut out, &self.mags);
        out
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        let mut r = bytes::Reader::new(data);
        r.tag(MAGIC)?;
        if r.u32()? != VERSION {
            return None;
        }
        let total_samples = r.usize()?;
        let n_frames = r.usize()?;
        let n_bins = r.usize()?;
        let hop = r.usize()?;
        let window_size = r.usize()?;
        let sample_rate = f32::from_bits(r.u32()?);
        let mags = r.f32_vec(n_frames.checked_mul(n_bins)?)?;
        Some(Self {
            total_samples,
            n_frames,
            n_bins,
            hop,
            window_size,
            sample_rate,
            mags,
            capacity: 0,
            head: 0,
        })
    }

    pub fn write_cache(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        std::fs::write(path, self.to_bytes())
    }

    pub fn read_cache(path: impl AsRef<std::path::Path>) -> std::io::Result<Option<Self>> {
        Ok(Self::from_bytes(&std::fs::read(path)?))
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
    /// reference range (the colour scale); z = colormap index.
    db: [f32; 4],
    /// xy = scale, zw = offset of the [`Framing`] that places the picture
    /// inside its viewport — the identity for a view the window shows whole.
    rect: [f32; 4],
}

/// GPU renderer for spectrograms: the pipeline that samples a magnitude texture
/// over a full-screen quad, plus the bind-group layout its textures are built
/// against.
///
/// **One of these serves a whole window** — see [`Renderers`]. It carries
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
    /// Uploads `stft`'s magnitudes as a texture and binds it against
    /// `renderer`'s layout.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &SpectrogramRenderer,
        stft: &Stft,
    ) -> Self {
        // Magnitudes -> a 2D texture: width = frames (time), height = bins
        // (frequency). Row 0 is bin 0 (low frequency); the shader flips y so
        // low frequencies sit at the bottom. R8Unorm is used (not R32Float)
        // because single-channel 32-bit float is not linearly *filterable*
        // without an optional GPU feature, whereas R8Unorm is filterable
        // everywhere (including WebGPU) and is half the size; the magnitudes are
        // already normalized to [0, 1], so 8 bits are ample for the colormap.
        // The width is the *texture's*, not the frame count: a rolling ring is
        // stored twice so its visible window never wraps (see `tex_width`). For
        // a stored transform the two are the same number.
        let size = wgpu::Extent3d {
            width: stft.tex_width().max(1) as u32,
            height: stft.n_bins().max(1) as u32,
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
        // The texture is row-major by frequency bin (height rows), but a column
        // is a run of bins, so transpose into a [bin][frame] upload buffer and
        // quantize to u8. `write_texture` (unlike a buffer copy) does not require
        // 256-byte row alignment, so the tight `width`-byte rows are fine.
        //
        // Columns are placed by `texel_of`, and a rolling one is written twice
        // (once in each copy of the ring); a stored transform's column `f` lands
        // at texel `f`, so it uploads exactly the bytes it always did.
        let (w, h) = (size.width as usize, size.height as usize);
        let mut transposed = vec![0u8; w * h];
        for f in 0..stft.n_frames() {
            let col = stft.column(f);
            let slot = stft.texel_of(f);
            for (b, m) in col.iter().enumerate().take(h) {
                let q = (m.clamp(0.0, 1.0) * 255.0).round() as u8;
                transposed[b * w + slot] = q;
                if stft.capacity() > 0 {
                    transposed[b * w + slot + stft.capacity()] = q;
                }
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &transposed,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width),
                rows_per_image: Some(size.height),
            },
            size,
        );
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

    /// Writes one already-quantized column into texel column `texel`, and into
    /// its mirror `capacity` texels to the right when the transform is rolling.
    ///
    /// This is the whole point of the ring: a landing column costs `n_bins`
    /// bytes twice, where rebuilding the picture costs the span — 384 KB a tick
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
                    blend: None,
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
pub struct SpectrogramView {
    stft: Stft,
    texture: SpectrogramTexture,
    /// The vertical display axis: the visible slice of the frequency display
    /// coordinate, normalized (`0, 1` = the whole axis).
    freq: Axis,
    scale: FreqScale,
    db_floor: f32,
    db_ceil: f32,
    /// 0 = viridis, 1 = magma, 2 = grayscale.
    colormap: u32,
    /// The frequency window's start, snapshotted for absolute drag panning.
    drag_freq_start: f64,
    /// Where this view's picture sits inside the viewport it is drawn with.
    framing: Framing,
    /// The quantized column a rolling push uploads through — held so a landing
    /// column allocates nothing.
    scratch: Vec<u8>,
}

impl SpectrogramView {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &SpectrogramRenderer,
        stft: Stft,
    ) -> Self {
        let texture = SpectrogramTexture::new(device, queue, renderer, &stft);
        Self {
            stft,
            texture,
            freq: Axis::normalized(Unit::Hz),
            scale: FreqScale::Log,
            db_floor: -90.0,
            db_ceil: 0.0,
            colormap: 0,
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
    /// columns and the texture is rebuilt around them — one full upload per
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
    /// grayscale). Cheap — everything lands in the shader uniforms, so a live
    /// `/gui_set` retunes the view with zero recompute.
    pub fn set_display(&mut self, db_floor: f32, db_ceil: f32, scale: FreqScale, colormap: u32) {
        self.db_floor = db_floor;
        self.db_ceil = db_ceil;
        self.scale = scale;
        self.colormap = colormap % 3;
    }

    /// The normalized bottom of the log frequency axis (~20 Hz / Nyquist) — the
    /// same `f_lo` the shader's display→bin mapping uses, exposed so a ruler
    /// places its ticks with the identical geometry.
    pub fn log_floor(&self) -> f32 {
        (20.0 / self.stft.nyquist()).clamp(1e-5, 0.5)
    }

    /// Sets the visible frequency window from normalized display coordinates
    /// (`start, len` with `0, 1` = the full axis; clamped) — the live
    /// `y_start`/`y_len` props of the editor-grade widget. The internal view
    /// keeps the display-coordinate convention (scaled by `n_bins`), so the
    /// shader's display→bin mapping is untouched.
    pub fn set_freq_window(&mut self, start: f64, len: f64) {
        self.freq.set_span(start, len);
    }

    /// Sets where the picture sits inside the viewport it is drawn with (see
    /// [`Framing`]) — a shader uniform, so a view cut by the window edge costs
    /// nothing extra to draw.
    pub fn set_framing(&mut self, framing: Framing) {
        self.framing = framing;
    }

    /// Build the GPU uniforms from the current time `view` and display state.
    ///
    /// The frequency window is expressed in *display* coordinates `[0, 1]`
    /// (the screen's vertical axis), not in bins. The linear/log mapping from
    /// that display coordinate to a normalized bin happens in the shader over
    /// the full axis, so zoom/pan use a plain linear screen anchor and the
    /// point under the cursor stays fixed in both modes.
    fn uniforms(&self, view: &View) -> Uniforms {
        let (start, len) = self.stft.time_fraction(view);

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
            db: [lo, hi, self.colormap as f32, 0.0],
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
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _renderers: &mut Renderers,
        view: &View,
        _render_width_px: u32,
    ) {
        let u = self.uniforms(view);
        self.texture.write_uniforms(queue, &u);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, renderers: &Renderers) {
        renderers.spectrogram.draw(pass, &self.texture);
    }

    /// `L` cycles the frequency scale (linear → log → mel → bark); `[` / `]`
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
    use std::f32::consts::PI;

    // The FFT correctness tests (impulse -> flat, cosine -> single bin) now live
    // with the shared implementation in `clausters_core::fft`; here we test the
    // STFT built on it.

    #[test]
    fn stft_locates_sine_frequency() {
        // A 1 kHz sine at 48 kHz, window 1024 -> bin = 1000/48000*1024 ~= 21.
        let sr = 48_000.0f32;
        let freq = 1000.0f32;
        let samples: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();
        let stft = Stft::compute(&samples, 1024, 512, sr);
        let nb = stft.n_bins();
        // Average magnitude per bin across frames; the max should be near bin 21.
        let mut acc = vec![0.0f32; nb];
        for row in stft.magnitudes().chunks_exact(nb) {
            for (acc_b, &m) in acc.iter_mut().zip(row) {
                *acc_b += m;
            }
        }
        let peak = (0..nb)
            .max_by(|&a, &b| acc[a].partial_cmp(&acc[b]).unwrap())
            .unwrap();
        let expected = (freq / sr * 1024.0).round() as usize;
        assert!(
            (peak as i32 - expected as i32).abs() <= 1,
            "peak bin {peak}, expected ~{expected}"
        );
    }

    #[test]
    fn hop_capped_bounds_the_frame_count() {
        // Short buffers keep the requested hop; long ones raise it just enough.
        assert_eq!(hop_capped(10_000, 1024, 512), 512);
        let long = 10_000_000;
        let hop = hop_capped(long, 1024, 512);
        assert!(hop > 512);
        let n_frames = 1 + (long - 1024) / hop;
        assert!(n_frames <= MAX_FRAMES, "{n_frames} frames");
    }

    #[test]
    fn cache_round_trip() {
        let samples: Vec<f32> = (0..5000).map(|i| (i as f32 * 0.02).sin()).collect();
        let stft = Stft::compute(&samples, 256, 128, 44_100.0);
        let back = Stft::from_bytes(&stft.to_bytes()).expect("parse");
        assert_eq!(stft.n_frames(), back.n_frames());
        assert_eq!(stft.n_bins(), back.n_bins());
        assert_eq!(stft.total_samples(), back.total_samples());
        assert_eq!(stft.nyquist(), back.nyquist());
        assert_eq!(stft.magnitudes(), back.magnitudes());
    }

    /// A ring column for column `i` of a ramp, so a test can tell them apart.
    fn marked(n_bins: usize, i: usize) -> Vec<f32> {
        vec![i as f32 / 255.0; n_bins]
    }

    /// The ring keeps the newest columns in time order once the cursor has
    /// wrapped, and reports the texel each one landed in - which is what the
    /// texture writes against.
    #[test]
    fn the_ring_wraps_and_keeps_the_newest_in_order() {
        let bins = 4;
        let mut stft = Stft::rolling(3, bins, 32, 64, 48_000.0);
        assert_eq!(stft.n_frames(), 0);
        for i in 0..3 {
            assert_eq!(stft.push_column(&marked(bins, i)), i, "fills in order");
        }
        assert_eq!(stft.n_frames(), 3);
        // Two more: the oldest two fall off and their texels are reused.
        assert_eq!(stft.push_column(&marked(bins, 3)), 0);
        assert_eq!(stft.push_column(&marked(bins, 4)), 1);
        assert_eq!(stft.n_frames(), 3, "the span is a cap");
        let seen: Vec<f32> = (0..3).map(|i| stft.column(i)[0] * 255.0).collect();
        assert_eq!(seen, vec![2.0, 3.0, 4.0], "oldest first, newest last");
        assert_eq!(stft.total_samples(), 2 * 32 + 64);
    }

    /// The ring is stored twice so the visible window is one contiguous run of
    /// texels - which is the invariant `time_fraction` maps against.
    #[test]
    fn a_rolling_window_is_contiguous_in_the_texture() {
        let bins = 4;
        let mut stft = Stft::rolling(4, bins, 32, 64, 48_000.0);
        assert_eq!(stft.tex_width(), 8);
        for i in 0..6 {
            stft.push_column(&marked(bins, i));
        }
        // Head is at texel 2 with four columns retained: 2..6, past the wrap
        // and still one run inside the doubled width.
        let view = View {
            start: 0.0,
            len: (stft.n_frames() - 1) as f64 * 32.0,
        };
        let (start, len) = stft.time_fraction(&view);
        assert!((start - 2.0 / 8.0).abs() < 1e-6, "{start}");
        assert!((len - 3.0 / 8.0).abs() < 1e-6, "{len}");
        assert!(start + len <= 1.0, "the window never leaves the texture");
    }

    /// A stored transform is the degenerate ring: no doubling, no offset, and
    /// the same fraction the renderer always uploaded.
    #[test]
    fn a_stored_transform_maps_exactly_as_it_did() {
        let samples: Vec<f32> = (0..5000).map(|i| (i as f32 * 0.02).sin()).collect();
        let stft = Stft::compute(&samples, 256, 128, 48_000.0);
        assert!(!stft.is_rolling());
        assert_eq!(stft.tex_width(), stft.n_frames());
        let view = View {
            start: 1000.0,
            len: 2000.0,
        };
        let frames = stft.n_frames() as f64;
        let (start, len) = stft.time_fraction(&view);
        assert_eq!(start, ((view.start / 128.0) / frames) as f32);
        assert_eq!(len, ((view.len / 128.0) / frames) as f32);
        // ...and its columns are read straight off the magnitudes.
        assert_eq!(stft.column(7), &stft.magnitudes()[7 * 128..8 * 128]);
    }

    /// A live `retention` change resizes the ring around the newest columns
    /// rather than restarting the picture.
    #[test]
    fn resizing_the_ring_keeps_the_newest_columns() {
        let bins = 4;
        let mut stft = Stft::rolling(6, bins, 32, 64, 48_000.0);
        for i in 0..6 {
            stft.push_column(&marked(bins, i));
        }
        stft.set_capacity(3);
        assert_eq!(stft.capacity(), 3);
        assert_eq!(stft.n_frames(), 3);
        let seen: Vec<f32> = (0..3).map(|i| stft.column(i)[0] * 255.0).collect();
        assert_eq!(seen, vec![3.0, 4.0, 5.0]);
        // Growing keeps them too, and the ring goes on filling from there.
        stft.set_capacity(5);
        stft.push_column(&marked(bins, 6));
        let seen: Vec<f32> = (0..stft.n_frames())
            .map(|i| stft.column(i)[0] * 255.0)
            .collect();
        assert_eq!(seen, vec![3.0, 4.0, 5.0, 6.0]);
    }

    /// A rolling transform pushed the columns of a signal is the same picture
    /// the stored analysis of it computes - the property the whole live path
    /// rests on, now that the two hold their magnitudes differently.
    #[test]
    fn a_rolling_transform_is_the_stored_one() {
        let (ws, hop, sr) = (256usize, 64usize, 48_000.0f32);
        let samples: Vec<f32> = (0..2048)
            .map(|i| (2.0 * PI * 3000.0 * i as f32 / sr).sin())
            .collect();
        let stored = Stft::compute(&samples, ws, hop, sr);
        let (hann, gain) = analysis_window(ws);
        let mut rolling = Stft::rolling(stored.n_frames(), ws / 2, hop, ws, sr);
        let mut windowed = vec![0.0; ws];
        let mut spectrum = vec![0.0; ws / 2];
        let mut col = vec![0.0; ws / 2];
        for f in 0..stored.n_frames() {
            let at = f * hop;
            column_into(
                &samples[at..at + ws],
                &hann,
                gain,
                &mut windowed,
                &mut spectrum,
                &mut col,
            );
            rolling.push_column(&col);
        }
        assert_eq!(rolling.n_frames(), stored.n_frames());
        assert_eq!(rolling.total_samples(), stored.total_samples());
        for f in 0..stored.n_frames() {
            assert_eq!(rolling.column(f), stored.column(f), "column {f}");
        }
    }
}
