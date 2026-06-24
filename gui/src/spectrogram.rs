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

use std::sync::Arc;

use crate::bytes;
use crate::view::TimelineView;
use crate::viewport::View;

const MAGIC: &[u8; 4] = b"CLSG";
const VERSION: u32 = 1;
/// Magnitudes below this many dB map to 0 (floor of the colour scale).
const DB_FLOOR: f32 = -90.0;

/// In-place iterative radix-2 Cooley-Tukey FFT. `re`/`im` must be the same
/// power-of-two length. A compact, testable stand-in; a production build would
/// swap in `rustfft` without touching the rest of the module.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);

    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    // Butterflies.
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wstep_r, wstep_i) = (ang.cos(), ang.sin());
        let half = len / 2;
        let mut base = 0;
        while base < n {
            let (mut wr, mut wi) = (1.0f32, 0.0f32);
            for k in 0..half {
                let a = base + k;
                let b = base + k + half;
                let tr = wr * re[b] - wi * im[b];
                let ti = wr * im[b] + wi * re[b];
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let nwr = wr * wstep_r - wi * wstep_i;
                wi = wr * wstep_i + wi * wstep_r;
                wr = nwr;
            }
            base += len;
        }
        len <<= 1;
    }
}

/// A short-time Fourier transform: `n_frames` x `n_bins` normalized magnitudes
/// in `[0, 1]` (dB mapped from `[DB_FLOOR, 0]`), row-major by frame. Frame `f`
/// is centred on samples starting at `f * hop`.
pub struct Stft {
    total_samples: usize,
    n_frames: usize,
    n_bins: usize,
    hop: usize,
    window_size: usize,
    mags: Vec<f32>,
}

impl Stft {
    /// Compute the STFT of mono `samples`. `window_size` must be a power of two;
    /// `hop` is the frame advance (e.g. `window_size / 2`).
    pub fn compute(samples: &[f32], window_size: usize, hop: usize) -> Self {
        assert!(window_size.is_power_of_two() && hop >= 1);
        let total_samples = samples.len();
        let n_bins = window_size / 2;
        let n_frames = if total_samples < window_size {
            1
        } else {
            1 + (total_samples - window_size) / hop
        };

        // Hann window and its coherent gain (sum), used to normalize so that a
        // full-scale sine reads ~0 dB.
        let hann: Vec<f32> = (0..window_size)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / window_size as f32).cos())
            .collect();
        let win_gain: f32 = hann.iter().sum::<f32>() * 0.5;

        let mut mags = vec![0.0f32; n_frames * n_bins];
        let mut re = vec![0.0f32; window_size];
        let mut im = vec![0.0f32; window_size];
        for f in 0..n_frames {
            let start = f * hop;
            for i in 0..window_size {
                re[i] = samples.get(start + i).copied().unwrap_or(0.0) * hann[i];
                im[i] = 0.0;
            }
            fft(&mut re, &mut im);
            for b in 0..n_bins {
                let mag = (re[b] * re[b] + im[b] * im[b]).sqrt() / win_gain;
                let db = 20.0 * (mag + 1e-9).log10();
                mags[f * n_bins + b] = ((db - DB_FLOOR) / -DB_FLOOR).clamp(0.0, 1.0);
            }
        }

        Self {
            total_samples,
            n_frames,
            n_bins,
            hop,
            window_size,
            mags,
        }
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
    pub fn magnitudes(&self) -> &[f32] {
        &self.mags
    }

    /// The visible sample range as a normalized horizontal `[start, start+len]`
    /// across the frame axis, for the renderer's uniform.
    fn time_fraction(&self, view: &View) -> (f32, f32) {
        let frames = self.n_frames.max(1) as f64;
        let start = (view.start / self.hop as f64) / frames;
        let len = (view.len / self.hop as f64) / frames;
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
        let mags = r.f32_vec(n_frames.checked_mul(n_bins)?)?;
        Some(Self {
            total_samples,
            n_frames,
            n_bins,
            hop,
            window_size,
            mags,
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
    /// `start_frac`, `len_frac` of the visible time window, then padding to 16.
    time: [f32; 4],
}

/// GPU renderer for an `Stft`: one full-screen quad samples the magnitude
/// texture; `set_view` updates the visible time slice; a colormap turns
/// magnitude into colour in the shader.
pub struct SpectrogramRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    time: [f32; 4],
}

impl SpectrogramRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        stft: &Stft,
    ) -> Self {
        // Magnitudes -> a 2D texture: width = frames (time), height = bins
        // (frequency). Row 0 is bin 0 (low frequency); the shader flips y so
        // low frequencies sit at the bottom. R8Unorm is used (not R32Float)
        // because single-channel 32-bit float is not linearly *filterable*
        // without an optional GPU feature, whereas R8Unorm is filterable
        // everywhere (including WebGPU) and is half the size; the magnitudes are
        // already normalized to [0, 1], so 8 bits are ample for the colormap.
        let size = wgpu::Extent3d {
            width: stft.n_frames().max(1) as u32,
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
        // The texture is row-major by frequency bin (height rows), but `mags` is
        // row-major by frame, so transpose into a [bin][frame] upload buffer and
        // quantize to u8. `write_texture` (unlike a buffer copy) does not require
        // 256-byte row alignment, so the tight `width`-byte rows are fine.
        let (w, h) = (size.width as usize, size.height as usize);
        let mut transposed = vec![0u8; w * h];
        let mags = stft.magnitudes();
        for f in 0..w {
            for b in 0..h {
                transposed[b * w + f] = (mags[f * h + b].clamp(0.0, 1.0) * 255.0).round() as u8;
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

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spectrogram bgl"),
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spectrogram bg"),
            layout: &bind_group_layout,
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
                    format,
                    blend: None,
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

        Self {
            pipeline,
            bind_group,
            uniform_buffer,
            time: [0.0, 1.0, 0.0, 0.0],
        }
    }

    fn set_view(&mut self, queue: &wgpu::Queue, stft: &Stft, view: &View) {
        let (start, len) = stft.time_fraction(view);
        self.time = [start, len, 0.0, 0.0];
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&Uniforms { time: self.time }),
        );
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// An `Stft` paired with its renderer, satisfying [`TimelineView`].
pub struct SpectrogramView {
    stft: Arc<Stft>,
    renderer: SpectrogramRenderer,
}

impl SpectrogramView {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        stft: Arc<Stft>,
    ) -> Self {
        let renderer = SpectrogramRenderer::new(device, queue, format, &stft);
        Self { stft, renderer }
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
        view: &View,
        _render_width_px: u32,
    ) {
        self.renderer.set_view(queue, &self.stft, view);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.renderer.draw(pass);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn fft_delta_is_flat_spectrum() {
        // FFT of a unit impulse is all-ones magnitude.
        let n = 16;
        let mut re = vec![0.0f32; n];
        let mut im = vec![0.0f32; n];
        re[0] = 1.0;
        fft(&mut re, &mut im);
        for k in 0..n {
            let mag = (re[k] * re[k] + im[k] * im[k]).sqrt();
            assert!((mag - 1.0).abs() < 1e-4, "bin {k} mag {mag}");
        }
    }

    #[test]
    fn fft_cosine_peaks_at_its_bin() {
        let n = 64;
        let k0 = 5;
        let mut re: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * k0 as f32 * i as f32 / n as f32).cos())
            .collect();
        let mut im = vec![0.0f32; n];
        fft(&mut re, &mut im);
        let peak = (0..n / 2)
            .max_by(|&a, &b| {
                let ma = re[a] * re[a] + im[a] * im[a];
                let mb = re[b] * re[b] + im[b] * im[b];
                ma.partial_cmp(&mb).unwrap()
            })
            .unwrap();
        assert_eq!(peak, k0);
    }

    #[test]
    fn stft_locates_sine_frequency() {
        // A 1 kHz sine at 48 kHz, window 1024 -> bin = 1000/48000*1024 ~= 21.
        let sr = 48_000.0f32;
        let freq = 1000.0f32;
        let samples: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * PI * freq * i as f32 / sr).sin())
            .collect();
        let stft = Stft::compute(&samples, 1024, 512);
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
    fn cache_round_trip() {
        let samples: Vec<f32> = (0..5000).map(|i| (i as f32 * 0.02).sin()).collect();
        let stft = Stft::compute(&samples, 256, 128);
        let back = Stft::from_bytes(&stft.to_bytes()).expect("parse");
        assert_eq!(stft.n_frames(), back.n_frames());
        assert_eq!(stft.n_bins(), back.n_bins());
        assert_eq!(stft.total_samples(), back.total_samples());
        assert_eq!(stft.magnitudes(), back.magnitudes());
    }
}
