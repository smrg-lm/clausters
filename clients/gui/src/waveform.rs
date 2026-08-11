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
//!   per pixel, read from the peak pyramid — **cross-faded** between the two
//!   levels adjacent to the zoom, so switching levels never pops (see
//!   [`WaveformData::column`]).
//!
//! The data is **multichannel**: one raw buffer and one pyramid per channel
//! (all sharing the time axis), so an editor-grade view draws stacked lanes or
//! overlaid per-channel traces from one `WaveformData`. Geometry is built per
//! channel into one vertex buffer with per-vertex color; the caller draws each
//! channel into its own lane viewport (stacked) or all into one (overlaid).
//!
//! `WaveformRenderer` takes a `wgpu::Device`/`Queue` and a target format and
//! owns nothing windowing-specific, so the identical code drives a native
//! `winit` surface or a `<canvas>` WebGPU surface in a browser.

use std::sync::Arc;

use crate::peaks::{self, MultiPyramid, Pyramid};
use crate::view::{Framing, Renderers, TimelineView};
use crate::viewport::{Axis, Unit, View};

/// At or below this many samples per pixel, draw the raw sample polyline rather
/// than min/max columns.
///
/// **One threshold, for every renderer of a signal.** The mesh path
/// (`host::signal::trace`) re-exports this rather than restating it, so the
/// regime boundary cannot drift between the pipeline and the triangles: the
/// same signal at the same zoom is resolved the same way whichever destination
/// draws it.
pub const LINE_THRESHOLD: f64 = 2.0;

/// Per-channel trace colors (RGBA), cycled when a buffer has more channels.
pub(crate) const CHANNEL_COLORS: [[f32; 4]; 4] = [
    [0.30, 0.78, 0.55, 1.0], // green (the classic mono trace)
    [0.95, 0.72, 0.25, 1.0], // amber
    [0.45, 0.65, 0.95, 1.0], // blue
    [0.90, 0.45, 0.60, 1.0], // rose
];

/// One channel's data: its raw samples (possibly empty, for a cache-only view)
/// plus its peak pyramid.
struct Channel {
    samples: Arc<[f32]>,
    pyramid: Pyramid,
}

/// A waveform's data: per channel, the raw samples (shared, for the zoomed-in
/// regimes) plus a peak pyramid (for the zoomed-out regime). The pyramids are
/// the cache that can be persisted via `peaks::MultiPyramid::write_cache`.
pub struct WaveformData {
    channels: Vec<Channel>,
}

/// A summary, not a dump: the data behind a view is megabytes of samples, and it
/// lives inside the widget tree (a `clip` body), which is `Debug`-printed in
/// logs and tests.
impl std::fmt::Debug for WaveformData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaveformData")
            .field("channels", &self.num_channels())
            .field("samples", &self.total_samples())
            .field("raw", &self.has_raw())
            .finish()
    }
}

impl WaveformData {
    /// A mono waveform from `samples`, building its pyramid at `base_bucket`.
    pub fn new(samples: Arc<[f32]>, base_bucket: usize) -> Self {
        let pyramid = Pyramid::build(&samples, base_bucket);
        Self {
            channels: vec![Channel { samples, pyramid }],
        }
    }

    /// A multichannel waveform from `samples` holding `channels` interleaved
    /// channels (a trailing partial frame is ignored), one pyramid per channel.
    pub fn from_interleaved(samples: &[f32], channels: usize, base_bucket: usize) -> Self {
        let channels = channels.max(1);
        let frames = samples.len() / channels;
        let built = (0..channels)
            .map(|ch| {
                let one: Vec<f32> = (0..frames).map(|f| samples[f * channels + ch]).collect();
                let samples: Arc<[f32]> = one.into();
                let pyramid = Pyramid::build(&samples, base_bucket);
                Channel { samples, pyramid }
            })
            .collect();
        Self { channels: built }
    }

    /// Build from samples and an already-computed pyramid (e.g. read back from a
    /// cache file with `Pyramid::read_cache`). The samples may be **empty** — a
    /// cache-only view (the bulk path where the host maps just the compact
    /// pyramid, never the raw buffer): it renders the resolution-matched overview
    /// from the pyramid, and the zoomed-in raw-sample regimes simply have nothing
    /// finer to show.
    pub fn with_pyramid(samples: Arc<[f32]>, pyramid: Pyramid) -> Self {
        Self {
            channels: vec![Channel { samples, pyramid }],
        }
    }

    /// A multichannel view from already-split raw channels paired with their
    /// pyramids (e.g. a mapped file whose sibling cache was still valid, so
    /// the pyramids were read back instead of rebuilt). Pairs must agree in
    /// length and bucket; the bulk loader validates before calling.
    pub fn from_parts(parts: Vec<(Arc<[f32]>, Pyramid)>) -> Self {
        assert!(!parts.is_empty());
        let channels = parts
            .into_iter()
            .map(|(samples, pyramid)| Channel { samples, pyramid })
            .collect();
        Self { channels }
    }

    /// A cache-only multichannel view from a mapped [`MultiPyramid`] (no raw
    /// samples; every regime renders from the per-channel pyramids).
    pub fn with_multi_pyramid(multi: MultiPyramid) -> Self {
        let channels = multi
            .into_channels()
            .into_iter()
            .map(|pyramid| Channel {
                samples: Arc::from([] as [f32; 0]),
                pyramid,
            })
            .collect();
        Self { channels }
    }

    /// The buffer length the view spans, in per-channel samples. Taken from the
    /// pyramid (which is built over the whole buffer), so a cache-only view with
    /// no raw `samples` still reports the right length.
    pub fn total_samples(&self) -> usize {
        self.channels[0].pyramid.total_samples()
    }

    /// How many channels this waveform holds.
    pub fn num_channels(&self) -> usize {
        self.channels.len()
    }

    /// Channel 0's pyramid (the persistable cache of a mono view).
    pub fn pyramid(&self) -> &Pyramid {
        &self.channels[0].pyramid
    }

    /// Whether raw samples are present. A cache-only view (`with_pyramid` with an
    /// empty buffer) has only the peak pyramid, so every regime — including the
    /// zoomed-in ones — must render from it; reading the empty raw buffer would
    /// instead collapse the wave to a flat line (it "disappears" on zoom-in).
    pub fn has_raw(&self) -> bool {
        !self.channels[0].samples.is_empty()
    }

    /// Min/max of channel `ch` for a pixel column spanning `[s0, s1)`, choosing
    /// the cheapest accurate source for the given `samples_per_px`: raw samples
    /// when finer than the pyramid's base bucket, the pyramid otherwise —
    /// **cross-faded** between the two adjacent levels so zooming never pops
    /// when the level selection switches.
    pub fn column(&self, ch: usize, samples_per_px: f64, s0: f64, s1: f64) -> (f32, f32) {
        let Some(channel) = self.channels.get(ch) else {
            return (0.0, 0.0);
        };
        let pyramid = &channel.pyramid;
        if samples_per_px < pyramid.base_bucket() as f64 && self.has_raw() {
            let a = (s0.floor().max(0.0) as usize).min(channel.samples.len());
            let b = (s1.ceil() as usize).clamp(a, channel.samples.len());
            peaks::min_max(&channel.samples[a..b]).unwrap_or((0.0, 0.0))
        } else {
            // At or above the base bucket, or whenever there is no raw buffer to
            // resolve finer (a cache-only view): read the pyramid. `level_for`
            // clamps to level 0, so zooming past the cache shows its finest
            // overview rather than collapsing to a flat line.
            level_crossfade(pyramid, samples_per_px, s0, s1)
        }
    }

    /// Single-sample access for the line regime, clamped to bounds.
    fn samples_at(&self, ch: usize, i: usize) -> f32 {
        self.channels
            .get(ch)
            .and_then(|c| c.samples.get(i))
            .copied()
            .unwrap_or(0.0)
    }
}

/// A pyramid column blended between the level matching `samples_per_px` and
/// the next coarser one, weighted by the fractional position of the zoom
/// between their bucket sizes (log2). At exactly a level's bucket the blend is
/// pure fine; approaching the next level's bucket it converges to pure coarse
/// — which is where `level_for` switches — so the min/max envelope is
/// continuous across the switch instead of popping.
fn level_crossfade(pyramid: &Pyramid, samples_per_px: f64, s0: f64, s1: f64) -> (f32, f32) {
    let level = pyramid.level_for(samples_per_px);
    let (lo, hi) = pyramid.column(level, s0, s1).unwrap_or((0.0, 0.0));
    let Some(bucket) = pyramid.level_bucket(level) else {
        return (lo, hi);
    };
    if samples_per_px <= bucket as f64 || level + 1 >= pyramid.num_levels() {
        return (lo, hi);
    }
    let t = (samples_per_px / bucket as f64).log2().clamp(0.0, 1.0) as f32;
    let (clo, chi) = pyramid.column(level + 1, s0, s1).unwrap_or((lo, hi));
    (lo + (clo - lo) * t, hi + (chi - hi) * t)
}

/// The vertical margin the trace leaves inside its lane: the value domain's
/// full span maps to this fraction of the lane's height. Shared with the
/// amplitude ruler and the cursor readout so a tick labeled 1.0 sits exactly on
/// the trace's full-scale line.
pub(crate) const AMP_MARGIN: f32 = 0.92;

/// The **default value domain** of a trace: full-scale amplitude. An element
/// that names no `min`/`max` is audio, and audio is bipolar about zero.
pub const DEFAULT_DOMAIN: (f32, f32) = (-1.0, 1.0);

/// The **zero line** of a value domain, or `None` when the domain does not
/// straddle it and there is no silence to draw.
///
/// It is a line and nothing more. A column is **never** extended to reach it:
/// the GPU pipeline used to clamp every column to zero and the mesh renderers
/// did not, and closing that divergence the other way — by clamping everywhere
/// — was the wrong half to keep. Filling to the baseline **inks a band the
/// signal was never in**: a column covering three samples that all sit at +0.6
/// is drawn from 0 to 0.6, which is a lie at any zoom where cycles are legible,
/// and it needs a threshold nobody can name to decide where that zoom begins.
///
/// The solid body of an overview needs no rule, because at that zoom the data
/// already fills it: a column summarizing hundreds of samples of audio crosses
/// zero by itself. So the envelope is drawn as it is measured, everywhere, and
/// what changes with the zoom is the signal — not the drawing's mind about it.
///
/// **And the zoom could not have been the criterion anyway.** A subsonic
/// signal — a 1 Hz LFO, a control curve, a long envelope — has far more samples
/// than the screen has pixels at any zoom where a whole cycle is visible, so
/// every "fill once the samples no longer fit" rule fills it; and a cycle a
/// second is a *curve*, which is exactly what a filled body destroys. What
/// separates a body from a curve is whether the signal crosses the span inside
/// one column, and the min/max already answers that — measured, per column, at
/// no cost.
pub fn baseline_of(min: f32, max: f32) -> Option<f32> {
    (min < 0.0 && max > 0.0).then_some(0.0)
}

/// Display coordinate of a value in the domain `[min, max]`: 0 at the lane
/// bottom, 1 at its top, with [`AMP_MARGIN`] of headroom left about the
/// domain's centre. The default domain reduces it to `amp * AMP_MARGIN`
/// mapped about the half-lane, which is what every view drew before a domain
/// could be named.
pub fn value_to_display(v: f32, min: f32, max: f32) -> f64 {
    let (centre, half) = domain_centre_half(min, max);
    ((v - centre) as f64 / half as f64 * AMP_MARGIN as f64) * 0.5 + 0.5
}

/// The inverse of [`value_to_display`] — what the cursor's height names.
pub fn display_to_value(d: f64, min: f32, max: f32) -> f32 {
    let (centre, half) = domain_centre_half(min, max);
    centre + ((d - 0.5) * 2.0 / AMP_MARGIN as f64) as f32 * half
}

/// How much of one lane a unit of value covers, before the vertical window is
/// applied — the resolution the cursor readout rounds to.
pub fn value_per_display(min: f32, max: f32) -> f64 {
    let (_, half) = domain_centre_half(min, max);
    2.0 * half as f64 / AMP_MARGIN as f64
}

/// A domain as its centre and half-span, with a degenerate one (`min == max`,
/// or reversed) widened so nothing divides by zero and the value simply sits
/// in the middle of its lane.
fn domain_centre_half(min: f32, max: f32) -> (f32, f32) {
    let (lo, hi) = (min.min(max), min.max(max));
    let half = ((hi - lo) * 0.5).max(f32::MIN_POSITIVE);
    ((lo + hi) * 0.5, half)
}

/// Maps a value to clip-space y through the visible display window
/// `[y0, y0 + y_len)` of the vertical axis (`0, 1` = the full lane). Display
/// coordinate 0 is the lane bottom — the same convention the vertical ruler
/// uses — so a vertical zoom moves the geometry and the ticks identically.
/// Values outside the window fall outside clip space and are clipped by the
/// GPU.
fn value_to_clip(v: f32, domain: (f32, f32), y0: f64, y_len: f64) -> f32 {
    let d = value_to_display(v, domain.0, domain.1);
    (((d - y0) / y_len) * 2.0 - 1.0) as f32
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Columns,
    Line,
}

/// `[x, y, r, g, b, a]` per vertex — position in clip space plus the channel's
/// trace color (the same shape the flat-geometry painter uses).
const FLOATS_PER_VERTEX: usize = 6;

/// Backend-independent waveform renderer: a triangle pipeline (min/max columns)
/// and a line pipeline (raw sample polyline) over one shader.
///
/// **One of these serves a whole window.** A pipeline is a pure function of the
/// device and the target format, so it says nothing about *which* waveform is
/// drawn — the per-element state is [`WaveformGeometry`], and one renderer
/// draws any number of them. That split is what makes a slot cheap enough to
/// give every element in a composition, instead of the shader module and two
/// pipeline objects per widget this used to compile.
///
/// The build `scratch` is shared for the same reason: it is transient space for
/// the frame's geometry, and elements upload one at a time.
pub struct WaveformRenderer {
    column_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    scratch: Vec<f32>,
}

/// One waveform element's GPU geometry: the vertex buffer the renderer fills,
/// the per-channel ranges within it, the regime those vertices were built for
/// and the trace palette they were coloured with. Everything here genuinely
/// belongs to the element; everything shared is in [`WaveformRenderer`].
pub struct WaveformGeometry {
    vertex_buffer: wgpu::Buffer,
    capacity_vertices: u64,
    /// One `(first_vertex, count)` per channel.
    ranges: Vec<(u32, u32)>,
    /// One `(first_vertex, count)` per channel of **sample dots** — the marks
    /// on each sample of the polyline, drawn with the triangle pipeline while
    /// the line itself is a strip. Empty at every zoom but the deepest.
    dot_ranges: Vec<(u32, u32)>,
    mode: Mode,
    /// Per-channel trace colors, cycled — [`CHANNEL_COLORS`] until a theme
    /// replaces them ([`Self::set_palette`]).
    palette: [[f32; 4]; 4],
}

impl WaveformGeometry {
    pub fn new(device: &wgpu::Device) -> Self {
        let capacity_vertices = 8192 * 6;
        Self {
            vertex_buffer: new_vertex_buffer(device, capacity_vertices),
            capacity_vertices,
            ranges: Vec::new(),
            dot_ranges: Vec::new(),
            mode: Mode::Columns,
            palette: CHANNEL_COLORS,
        }
    }

    /// Replaces the per-channel trace palette (the theme's series colors).
    pub fn set_palette(&mut self, palette: [[f32; 4]; 4]) {
        self.palette = palette;
    }
}

fn new_vertex_buffer(device: &wgpu::Device, vertices: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("waveform vertices"),
        size: vertices * (FLOATS_PER_VERTEX * std::mem::size_of::<f32>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl WaveformRenderer {
    pub fn new(device: &wgpu::Device, target: crate::view::Target) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("waveform shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("waveform.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("waveform pipeline layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: (std::mem::size_of::<f32>() * FLOATS_PER_VERTEX) as wgpu::BufferAddress,
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
                        format: target.format,
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
                multisample: target.multisample(),
                multiview_mask: None,
                cache: None,
            })
        };
        let column_pipeline =
            make_pipeline(wgpu::PrimitiveTopology::TriangleList, "waveform columns");
        let line_pipeline = make_pipeline(wgpu::PrimitiveTopology::LineStrip, "waveform line");

        Self {
            column_pipeline,
            line_pipeline,
            scratch: Vec::new(),
        }
    }

    fn push_vertex(&mut self, f: Framing, x: f32, y: f32, color: [f32; 4]) {
        let (x, y) = f.apply(x, y);
        self.scratch
            .extend_from_slice(&[x, y, color[0], color[1], color[2], color[3]]);
    }

    /// Rebuild and upload `geom` for `view` at `render_width_px` device
    /// pixels, mapping values through the element's `domain` and the visible
    /// vertical display window `y_window` (`(0.0, 1.0)` = the full axis).
    /// O(render_width_px) per channel in the column regimes, O(visible samples)
    /// in the line regime - both bounded by the screen, never by the buffer.
    ///
    /// `lane_h_px` is the height one lane is drawn at, and it is here for two
    /// reasons: a column is never inked thinner than a pixel, so a stretch the
    /// signal barely moves in stays visible instead of collapsing — the tail of
    /// a decay, the sustain of an envelope — and a **sample dot** is as round
    /// in a short lane as in a tall one. Both are the mesh renderer's rules
    /// ([`crate::host::signal::trace`]) applied to the pipeline that had
    /// neither. `dot_radius` is the size table's `point_radius`; `0` marks no
    /// samples.
    // Everything the element used to hold is now passed in — which is the
    // point of the split, and what makes the list long. Grouping it back into
    // a struct would only re-create the object this milestone took apart.
    #[allow(clippy::too_many_arguments)]
    pub fn upload_geometry(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        geom: &mut WaveformGeometry,
        data: &WaveformData,
        view: &View,
        render_width_px: u32,
        domain: (f32, f32),
        y_window: (f64, f64),
        lane_h_px: f32,
        dot_radius: f32,
        framings: &[Framing],
    ) {
        let w = render_width_px.max(1);
        let spp = view.samples_per_px(w);
        let total = data.total_samples();
        let (y0, y_len) = (y_window.0, y_window.1.max(crate::viewport::MIN_SPAN));
        // The least a column is inked: one physical pixel of the lane,
        // expressed in the clip space the vertices are built in.
        let min_ink = if lane_h_px > 0.0 {
            2.0 / lane_h_px
        } else {
            0.0
        };
        self.scratch.clear();
        geom.ranges.clear();
        geom.dot_ranges.clear();

        geom.mode = if spp <= LINE_THRESHOLD && data.has_raw() {
            Mode::Line
        } else {
            Mode::Columns
        };
        // **Sample dots**, on the same rule the mesh renderer uses: once
        // consecutive samples stand three radii apart, each one is marked. The
        // line between them is an interpolation the drawing invents; the dot is
        // what says which points of it are data — and what sample-level editing
        // will take hold of, which is why it is the radius a break-point is
        // drawn at.
        let spacing = w as f32 / (view.len.max(1e-9) as f32);
        let mark_samples =
            geom.mode == Mode::Line && crate::host::signal::trace::dots_fit(spacing, dot_radius);
        // A dot is a square in clip space, so its half-extent is one radius of
        // the axis it lies on — the two differ, since a lane is not as tall as
        // it is wide.
        let dot_r_x = 2.0 * dot_radius / w as f32;
        let dot_r_y = if lane_h_px > 0.0 {
            2.0 * dot_radius / lane_h_px
        } else {
            0.0
        };
        for ch in 0..data.num_channels() {
            let color = geom.palette[ch % geom.palette.len()];
            // The lane's framing, since each channel is drawn with its own
            // viewport: a stack can have its top lane whole and its bottom one
            // cut by the window edge.
            let f = framings.get(ch).copied().unwrap_or(Framing::IDENTITY);
            let first = (self.scratch.len() / FLOATS_PER_VERTEX) as u32;
            let mut dots: Vec<(f32, f32)> = Vec::new();
            match geom.mode {
                Mode::Line => {
                    let a = (view.start.floor().max(0.0) as usize).min(total);
                    let b = ((view.start + view.len).ceil() as usize).min(total);
                    for i in a..b {
                        let frac = (i as f64 - view.start) / view.len;
                        let x = (-1.0 + 2.0 * frac) as f32;
                        let y = value_to_clip(data.samples_at(ch, i), domain, y0, y_len);
                        self.push_vertex(f, x, y, color);
                        if mark_samples {
                            dots.push((x, y));
                        }
                    }
                }
                Mode::Columns => {
                    for x in 0..w {
                        let s0 = view.start + view.len * (x as f64 / w as f64);
                        let s1 = view.start + view.len * ((x + 1) as f64 / w as f64);
                        let (lo, hi) = data.column(ch, spp, s0, s1);
                        let xl = -1.0 + 2.0 * (x as f32 / w as f32);
                        let xr = -1.0 + 2.0 * ((x + 1) as f32 / w as f32);
                        let mut yb = value_to_clip(lo, domain, y0, y_len);
                        let mut yt = value_to_clip(hi, domain, y0, y_len);
                        // ...and never thinner than a pixel of the lane, so a
                        // flat stretch inks a hairline rather than nothing.
                        if yt - yb < min_ink {
                            let mid = (yt + yb) * 0.5;
                            yb = mid - min_ink * 0.5;
                            yt = mid + min_ink * 0.5;
                        }
                        self.push_vertex(f, xl, yb, color);
                        self.push_vertex(f, xr, yb, color);
                        self.push_vertex(f, xr, yt, color);
                        self.push_vertex(f, xl, yb, color);
                        self.push_vertex(f, xr, yt, color);
                        self.push_vertex(f, xl, yt, color);
                    }
                }
            }
            let count = (self.scratch.len() / FLOATS_PER_VERTEX) as u32 - first;
            geom.ranges.push((first, count));
            // The dots go after the strip, as triangles: the line pipeline is a
            // `LineStrip` and cannot draw them, so they are their own range and
            // their own draw.
            let dot_first = (self.scratch.len() / FLOATS_PER_VERTEX) as u32;
            for (x, y) in dots {
                let (rx, ry) = (dot_r_x, dot_r_y);
                for (vx, vy) in [
                    (x - rx, y - ry),
                    (x + rx, y - ry),
                    (x + rx, y + ry),
                    (x - rx, y - ry),
                    (x + rx, y + ry),
                    (x - rx, y + ry),
                ] {
                    self.push_vertex(f, vx, vy, color);
                }
            }
            let dot_count = (self.scratch.len() / FLOATS_PER_VERTEX) as u32 - dot_first;
            geom.dot_ranges.push((dot_first, dot_count));
        }

        let needed = (self.scratch.len() / FLOATS_PER_VERTEX) as u64;
        if needed > geom.capacity_vertices {
            geom.capacity_vertices = needed.next_power_of_two();
            geom.vertex_buffer = new_vertex_buffer(device, geom.capacity_vertices);
        }
        queue.write_buffer(&geom.vertex_buffer, 0, bytemuck::cast_slice(&self.scratch));
    }

    fn bind(&self, pass: &mut wgpu::RenderPass<'_>, geom: &WaveformGeometry) {
        let pipeline = match geom.mode {
            Mode::Columns => &self.column_pipeline,
            Mode::Line => &self.line_pipeline,
        };
        pass.set_pipeline(pipeline);
        pass.set_vertex_buffer(0, geom.vertex_buffer.slice(..));
    }

    /// Record every channel's draw into an existing render pass (the overlaid
    /// form — all traces share the caller's viewport). One draw per channel so
    /// the line strips do not connect across channels.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, geom: &WaveformGeometry) {
        if geom.ranges.iter().all(|(_, count)| *count == 0) {
            return;
        }
        self.bind(pass, geom);
        for (first, count) in &geom.ranges {
            if *count > 0 {
                pass.draw(*first..*first + *count, 0..1);
            }
        }
        self.draw_dots(pass, geom, None);
    }

    /// The sample dots of one channel, or of every channel when `only` is
    /// `None`. They are triangles, so they are drawn with the **column**
    /// pipeline whatever regime the trace itself is in — which is also why they
    /// are recorded after it: a strip and a triangle list cannot share a draw.
    fn draw_dots(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        geom: &WaveformGeometry,
        only: Option<usize>,
    ) {
        let any = match only {
            Some(ch) => geom.dot_ranges.get(ch).is_some_and(|(_, c)| *c > 0),
            None => geom.dot_ranges.iter().any(|(_, c)| *c > 0),
        };
        if !any {
            return;
        }
        pass.set_pipeline(&self.column_pipeline);
        pass.set_vertex_buffer(0, geom.vertex_buffer.slice(..));
        for (ch, (first, count)) in geom.dot_ranges.iter().enumerate() {
            if *count > 0 && only.is_none_or(|c| c == ch) {
                pass.draw(*first..*first + *count, 0..1);
            }
        }
    }

    /// Record one channel's draw (the stacked form — the caller sets that
    /// channel's lane viewport first).
    pub fn draw_channel(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        geom: &WaveformGeometry,
        ch: usize,
    ) {
        let Some(&(first, count)) = geom.ranges.get(ch) else {
            return;
        };
        if count > 0 {
            self.bind(pass, geom);
            pass.draw(first..first + count, 0..1);
        }
        self.draw_dots(pass, geom, Some(ch));
    }
}

/// A `WaveformData` paired with its GPU geometry and its vertical (amplitude)
/// display window, satisfying [`TimelineView`]. The pipelines it draws through
/// belong to the window ([`WaveformRenderer`]), not to this.
pub struct WaveformView {
    data: WaveformData,
    geometry: WaveformGeometry,
    /// The vertical display axis: the visible slice of the value domain,
    /// normalized (`0, 1` = no zoom).
    amp: Axis,
    /// The **value domain** the geometry is mapped through — the element's
    /// `min`/`max`, [`DEFAULT_DOMAIN`] when it names none.
    domain: (f32, f32),
    /// The height one lane is drawn at, in physical pixels: the floor a
    /// column's ink is held above.
    lane_h_px: f32,
    /// The radius a sample dot is drawn at (the size table's `point_radius`).
    dot_radius: f32,
    /// The amplitude window's start, snapshotted for absolute drag panning.
    drag_amp_start: f64,
    /// One framing per lane (empty = every lane whole). Set before the upload,
    /// because a waveform's geometry is built on the CPU and the framing goes
    /// into the vertices rather than into a uniform.
    framings: Vec<Framing>,
}

impl WaveformView {
    pub fn new(device: &wgpu::Device, data: WaveformData) -> Self {
        Self {
            data,
            geometry: WaveformGeometry::new(device),
            amp: Axis::normalized(Unit::Norm),
            domain: DEFAULT_DOMAIN,
            lane_h_px: 0.0,
            dot_radius: 0.0,
            drag_amp_start: 0.0,
            framings: Vec::new(),
        }
    }

    /// Sets the **value domain** the geometry maps through — the element's
    /// `min`/`max`. Left alone it is [`DEFAULT_DOMAIN`], full-scale amplitude,
    /// which is what every view that names no bounds draws at.
    pub fn set_domain(&mut self, min: f32, max: f32) {
        self.domain = (min, max);
    }

    /// The domain in force, which the vertical ruler and the cursor readout
    /// must name the same values through.
    pub fn domain(&self) -> (f32, f32) {
        self.domain
    }

    /// Sets the height one lane is drawn at, in physical pixels — the floor a
    /// column's ink is held above (see [`WaveformRenderer::upload_geometry`]).
    pub fn set_lane_height(&mut self, px: f32) {
        self.lane_h_px = px;
    }

    /// Sets the radius a **sample dot** is drawn at: the size table's
    /// `point_radius`, so a sample reads as the same kind of target a curve's
    /// break-point does. `0` marks no samples.
    pub fn set_dot_radius(&mut self, px: f32) {
        self.dot_radius = px;
    }

    /// How many channels the underlying data holds (the lane count).
    pub fn num_channels(&self) -> usize {
        self.data.num_channels()
    }

    /// Sets the visible vertical display window (normalized; clamped) — the
    /// live `y_start`/`y_len` props of the editor-grade widget.
    pub fn set_amp_window(&mut self, start: f64, len: f64) {
        self.amp.set_span(start, len);
    }

    /// Record one channel's draw (see [`WaveformRenderer::draw_channel`]).
    pub fn draw_channel(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        renderer: &WaveformRenderer,
        ch: usize,
    ) {
        renderer.draw_channel(pass, &self.geometry, ch);
    }

    /// Replaces the per-channel trace palette (the theme's series colors).
    pub fn set_palette(&mut self, palette: [[f32; 4]; 4]) {
        self.geometry.set_palette(palette);
    }

    /// Sets where each lane's picture sits inside the viewport it is drawn with
    /// (see [`Framing`]) — one per lane, in the order the lanes are drawn.
    /// Clearing it (an empty slice) is the whole-element case.
    pub fn set_framings(&mut self, framings: &[Framing]) {
        self.framings.clear();
        self.framings.extend_from_slice(framings);
    }
}

impl TimelineView for WaveformView {
    fn total_samples(&self) -> usize {
        self.data.total_samples()
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderers: &mut Renderers,
        view: &View,
        render_width_px: u32,
    ) {
        renderers.waveform.upload_geometry(
            device,
            queue,
            &mut self.geometry,
            &self.data,
            view,
            render_width_px,
            self.domain,
            self.amp.span(),
            self.lane_h_px,
            self.dot_radius,
            &self.framings,
        );
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, renderers: &Renderers) {
        renderers.waveform.draw(pass, &self.geometry);
    }

    fn on_vertical_zoom(&mut self, factor: f64, anchor: f64) -> bool {
        self.amp.zoom(factor, anchor);
        true
    }

    fn on_vertical_drag_begin(&mut self) {
        self.drag_amp_start = self.amp.start();
    }

    fn on_vertical_drag(&mut self, total: f64) -> bool {
        // Dragging down (total > 0) moves the window down with the cursor.
        // Absolute from the snapshot.
        self.amp
            .set_start(self.drag_amp_start + total * self.amp.len());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An alternating +/-0.5 signal: every base bucket has min -0.5, max +0.5.
    fn envelope_signal(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect()
    }

    #[test]
    fn cache_only_view_resolves_zoom_in_from_the_pyramid() {
        // Cache-only: no raw samples, only the pyramid (the bulk `cache=` path).
        let pyramid = Pyramid::build(&envelope_signal(4096), 256);
        let data = WaveformData::with_pyramid(Arc::from([] as [f32; 0]), pyramid);
        assert!(!data.has_raw());
        // Zoomed in past the base bucket (spp < 256): the raw regime would read
        // the empty buffer and collapse to (0, 0) — the disappearing wave. The
        // fallback reads the pyramid's finest level, so the envelope survives.
        let (lo, hi) = data.column(0, 8.0, 0.0, 8.0);
        assert!(
            lo <= -0.4 && hi >= 0.4,
            "cache-only zoom-in should show the pyramid envelope, got ({lo}, {hi})"
        );
    }

    #[test]
    fn raw_view_still_uses_raw_samples_when_zoomed_in() {
        let data = WaveformData::new(Arc::from(envelope_signal(4096)), 256);
        assert!(data.has_raw());
        let (lo, hi) = data.column(0, 8.0, 0.0, 8.0);
        assert!(
            lo <= -0.4 && hi >= 0.4,
            "raw zoom-in lost the signal: ({lo}, {hi})"
        );
    }

    #[test]
    fn interleaved_channels_split_and_share_the_time_axis() {
        // Stereo: channel 0 the envelope, channel 1 silence.
        let inter: Vec<f32> = envelope_signal(2048)
            .into_iter()
            .flat_map(|s| [s, 0.0])
            .collect();
        let data = WaveformData::from_interleaved(&inter, 2, 64);
        assert_eq!(data.num_channels(), 2);
        assert_eq!(data.total_samples(), 2048, "frames, not flat samples");
        let (lo0, hi0) = data.column(0, 128.0, 0.0, 128.0);
        assert!(lo0 <= -0.4 && hi0 >= 0.4, "channel 0 keeps the envelope");
        let (lo1, hi1) = data.column(1, 128.0, 0.0, 128.0);
        assert_eq!((lo1, hi1), (0.0, 0.0), "channel 1 is silent");
        // An out-of-range channel reads zero instead of panicking.
        assert_eq!(data.column(5, 128.0, 0.0, 128.0), (0.0, 0.0));
    }

    #[test]
    fn cache_only_multichannel_view_reads_every_lane() {
        let inter: Vec<f32> = envelope_signal(2048)
            .into_iter()
            .flat_map(|s| [s, s * 0.5])
            .collect();
        let multi = MultiPyramid::build_interleaved(&inter, 2, 64);
        let data = WaveformData::with_multi_pyramid(multi);
        assert_eq!(data.num_channels(), 2);
        assert!(!data.has_raw());
        let (_, hi0) = data.column(0, 8.0, 0.0, 64.0);
        let (_, hi1) = data.column(1, 8.0, 0.0, 64.0);
        assert!(hi0 >= 0.4 && (0.2..0.4).contains(&hi1));
    }

    #[test]
    fn amp_window_maps_geometry_through_the_visible_slice() {
        // Full axis: the classic margin map.
        assert!((value_to_clip(1.0, DEFAULT_DOMAIN, 0.0, 1.0) - AMP_MARGIN).abs() < 1e-6);
        assert_eq!(value_to_clip(0.0, DEFAULT_DOMAIN, 0.0, 1.0), 0.0);
        // Zoomed into the top half: the zero line sits at the bottom edge of
        // clip space and full scale inside the window, above the middle.
        assert!((value_to_clip(0.0, DEFAULT_DOMAIN, 0.5, 0.5) - -1.0).abs() < 1e-6);
        let full = value_to_clip(1.0, DEFAULT_DOMAIN, 0.5, 0.5);
        assert!((0.0..1.0).contains(&full), "{full}");
        // A value below the window leaves clip space (the GPU clips it).
        assert!(value_to_clip(-1.0, DEFAULT_DOMAIN, 0.5, 0.5) < -1.0);
    }

    /// A named domain is the *same* map over another range: its ends land where
    /// full scale lands on the amplitude axis, so the margin is a property of
    /// the lane and not of what the signal happens to measure.
    #[test]
    fn a_named_domain_maps_its_ends_where_full_scale_maps() {
        for (min, max) in [(0.0f32, 1.0f32), (-0.25, 0.75), (20.0, 20_000.0)] {
            for (v, amp) in [(min, -1.0f32), (max, 1.0)] {
                let named = value_to_display(v, min, max);
                let default = value_to_display(amp, DEFAULT_DOMAIN.0, DEFAULT_DOMAIN.1);
                assert!(
                    (named - default).abs() < 1e-9,
                    "[{min}, {max}] end {v} at {named}, full scale at {default}"
                );
            }
            // ...and the inverse names it back, which is what the readout does.
            let mid = (min + max) * 0.5;
            let back = display_to_value(value_to_display(mid, min, max), min, max);
            assert!(
                (back - mid).abs() <= (max - min).abs() * 1e-6,
                "{back} {mid}"
            );
        }
    }

    /// A degenerate domain divides by nothing and parks the value mid-lane,
    /// rather than producing a NaN the vertex buffer would carry to the GPU.
    #[test]
    fn a_degenerate_domain_is_finite() {
        let d = value_to_display(3.0, 3.0, 3.0);
        assert!(d.is_finite(), "{d}");
        assert!(value_to_clip(3.0, (3.0, 3.0), 0.0, 1.0).is_finite());
    }

    /// The fill rule, which the three renderers now share: a domain straddling
    /// zero has a baseline (audio is a deviation from silence), one that does
    /// not is drawn as its own envelope (an envelope, an automation, a
    /// unipolar take).
    #[test]
    fn only_a_domain_that_straddles_zero_has_a_baseline() {
        assert_eq!(baseline_of(-1.0, 1.0), Some(0.0));
        assert_eq!(baseline_of(-0.25, 0.75), Some(0.0));
        assert_eq!(
            baseline_of(0.0, 1.0),
            None,
            "unipolar: no baseline to fill to"
        );
        assert_eq!(baseline_of(20.0, 20_000.0), None, "an offset quantity");
        assert_eq!(baseline_of(-1.0, 0.0), None, "wholly negative");
    }

    #[test]
    fn lod_crossfade_is_continuous_across_a_level_switch() {
        // A signal whose envelope shrinks with time makes adjacent pyramid
        // levels disagree, so a hard level switch would jump. Sample the column
        // just below and just above the switch point (spp = 2 * base_bucket):
        // the cross-faded values must be close.
        let n = 65536;
        let samples: Vec<f32> = (0..n)
            .map(|i| {
                let env = 1.0 - i as f32 / n as f32;
                if i % 2 == 0 { env } else { -env }
            })
            .collect();
        let data = WaveformData::new(Arc::from(samples), 64);
        let (s0, s1) = (40_000.0, 40_256.0);
        let switch = 128.0; // 2 * base_bucket: level_for flips from 0 to 1 here
        let (lo_a, hi_a) = data.column(0, switch - 1e-3, s0, s1);
        let (lo_b, hi_b) = data.column(0, switch + 1e-3, s0, s1);
        assert!(
            (lo_a - lo_b).abs() < 1e-3 && (hi_a - hi_b).abs() < 1e-3,
            "envelope must be continuous at the level switch: ({lo_a},{hi_a}) vs ({lo_b},{hi_b})"
        );
        // And in between the blend moves monotonically toward the coarse level.
        let (_, hi_mid) = data.column(0, 64.0 * 1.5, s0, s1);
        let (_, hi_fine) = data.column(0, 64.0 + 1e-3, s0, s1);
        assert!(
            hi_mid >= hi_fine - 1e-6,
            "blend widens toward the coarse level"
        );
    }
}
