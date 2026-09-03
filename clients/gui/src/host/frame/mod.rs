//! Rendering one window's widget tree into its wgpu surface — the shared frame
//! path, agnostic of platform and of how the host is driven.
//!
//! This is the code the milestone calls "isolate the surface/GPU/loop port":
//! both fronts feed the **same** [`render`] one tree plus its per-window GPU
//! resources, so the browser is pixel-faithful to the desktop by construction,
//! not by a parallel renderer. The native windowed front ([`super::gui`]) calls
//! it with live inputs (the shared-memory bus source, scope histories, the node
//! tree, the held-button highlight); the browser entry point (`super::web`)
//! calls it with the streamed equivalents. It builds the flat-geometry [`Mesh`]
//! from the placed widgets ([`super::layout`] + [`super::paint`]/
//! [`super::font`]), uploads the heavy `waveform`/`spectrogram`/`canvas` views,
//! and draws the whole frame in one pass — the editor chrome (rulers,
//! selection, playhead, cursor readout) as a second, *overlay* mesh drawn
//! after the heavy views so it reads on top of them.
//!
//! **Module layout.** This file is the frame's spine: the GPU slots a heavy
//! view hangs on, the [`FrameInputs`] a front fills, and [`render`] itself —
//! lay out, collect, draw, upload, one pass. The two long halves it calls are
//! its children, one per direction of the frame. [`items`] is the *read* half:
//! the per-widget snapshots and the single tree walk that fills them, kept
//! together because a new item type is a struct plus an arm of that walk.
//! [`draw`] is the *write* half: the three mesh passes over those snapshots,
//! which by then hold no borrow of the host tree.

mod draw;
mod items;

use draw::*;
pub(crate) use draw::{draw_time_ruler, ruler_strip_body};
use items::*;

use std::collections::HashMap;
use std::sync::Arc;

use tracing::warn;

use crate::gpu::Gpu;
use crate::spectrogram::{FreqScale, SpectrogramView, Stft, hop_capped};
use crate::view::{Framing, Renderers, TimelineView};
use crate::viewport::View;
use crate::waveform::{WaveformData, WaveformView};

use super::bands::Bands;
use super::layout::{self, Rect};
use super::metrics::Metrics;
use crate::canvas::{self, CanvasView};

use super::font;
use super::paint::{Draw, Ink, Mesh, Painter};
use super::ruler::{self, TimeUnit};
use super::theme::{Theme, with_alpha};
use super::timeline::{GroupState, group_key};
use super::widget::element::{Ctx, Loaded, SlotFill, SlotFrame, TimeSpace};
use super::widget::{EditorProps, Ruler, RulerY, Widget, WidgetKind};
use super::world::World;
use crate::host::graphics::track;

/// The window clear color: the theme's `background` role as a `wgpu::Color`.
pub(crate) fn clear_color(theme: &Theme) -> wgpu::Color {
    wgpu::Color {
        r: theme.background[0] as f64,
        g: theme.background[1] as f64,
        b: theme.background[2] as f64,
        a: theme.background[3] as f64,
    }
}

/// A waveform widget's data and vertical navigation state. Its horizontal
/// window lives in the widget's timeline group ([`super::timeline`]), not here
/// — a slot is per window, a group may span windows. The picture is drawn into
/// the window's mesh like every other widget's, so nothing here is GPU state.
pub(crate) struct WaveformSlot {
    pub(crate) view: WaveformView,
    /// **What this view was drawn over and could not answer** — a zoom finer
    /// than its summary's bucket, over a span it holds neither samples nor a
    /// finer grid for. [`Owed`] says which of the two would settle it.
    ///
    /// It is set by the draw pass, which is the only place that knows the zoom
    /// *and* the span, and read (and cleared) by the leg after the frame,
    /// which is the only place that can ask the server for it. A `Cell`
    /// because the draw pass borrows the slots immutably — the front is single
    /// threaded, and this is a note left on the way past rather than state.
    pub(crate) owed: std::cell::Cell<Option<Owed>>,
}

/// **What a view is owed for the span it is showing**, and the two are a
/// difference in *shape* rather than in size.
///
/// A picture needs one min/max pair per pixel column. A view that can map the
/// samples has them under its pointer and measures the columns itself; one that
/// cannot has two ways to get the same row, and which is cheaper depends only
/// on the zoom:
///
/// - [`Owed::Summary`] — a **finer grid** over the span, at about a bucket a
///   column (`/buffer_peaks`, folded by [`WaveformData::set_detail`]). A few
///   kilobytes, one reply, and it is what a zoom above the polyline regime
///   actually wants: asking for the samples there moves a few hundred kilobytes
///   through a 64 KiB carrier to compute a row the server can measure in one
///   pass.
/// - [`Owed::Samples`] — the **samples themselves** (`/buffer_getRange`, folded
///   by [`WaveformData::set_window`]), for the zoom below which no summary is
///   worth asking for: past `trace::LINE_THRESHOLD` the trace is the polyline
///   through the samples, and a bucket is not a sample.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Owed {
    /// The run of samples to read back, `[a, b)` in frames.
    Samples { a: usize, b: usize },
    /// The span to summarize, `[a, b)` in frames, and the bucket to measure it
    /// at — finer than the view's own, and coarse enough that one reply holds
    /// the whole span.
    Summary { a: usize, b: usize, bucket: usize },
}

/// A `WaveformSlot` for ready data.
pub(crate) fn waveform_slot(data: impl Into<Arc<WaveformData>>) -> WaveformSlot {
    WaveformSlot {
        view: WaveformView::new(data),
        owed: std::cell::Cell::new(None),
    }
}

/// A spectrogram widget's GPU views — one [`SpectrogramView`] (own STFT and
/// texture) per channel lane. Navigation lives in the timeline group.
pub(crate) struct SpectrogramSlot {
    pub(crate) views: Vec<SpectrogramView>,
}

impl SpectrogramSlot {
    /// The per-channel sample count of this slot's data.
    pub(crate) fn total_samples(&self) -> usize {
        self.views.first().map_or(1, |v| v.total_samples())
    }
}

/// A `SpectrogramSlot` from per-channel analyses (empty `stfts` yields none).
pub(crate) fn spectrogram_slot(
    stfts: Vec<Stft>,
    gpu: &Gpu,
    renderers: &Renderers,
) -> Option<SpectrogramSlot> {
    if stfts.is_empty() {
        return None;
    }
    let views = stfts
        .into_iter()
        .map(|stft| SpectrogramView::new(&gpu.device, &gpu.queue, &renderers.spectrogram, stft))
        .collect();
    Some(SpectrogramSlot { views })
}

/// **What a slot-backed element keeps of the resource that filled its slot.**
///
/// A pyramid is not only a picture: it is the samples the element named, and
/// [`Element::sample_block`](crate::host::widget::element::Element::sample_block)
/// reads a copy back out of it. Routing it to the slot alone left the element
/// holding nothing, so a copy over a mapped take — the very source the clipboard
/// was written for — refused as if the host could not read it. The two share one
/// `Arc`, so keeping it costs a pointer and never a second pyramid.
///
/// Every other form is the picture alone and the element keeps nothing: an
/// analysis is a reading of a signal, not the signal, and what a copy would owe
/// the clipboard is samples.
pub(crate) fn keep_data(widget: &mut Widget, data: &Loaded) {
    match data {
        Loaded::Peaks(peaks) => widget.take_bulk(|| Loaded::Peaks(peaks.clone())),
        Loaded::Shared(shared) => widget.take_bulk(|| Loaded::Shared(shared.clone())),
        _ => false,
    };
}

/// **Puts a resolved bulk resource into the slot its element claimed**, keyed
/// by the id that addresses that element (a clip's body is its container's).
/// Returns the loaded extent in samples, for the navigation group that has to
/// know how long its longest member is.
///
/// The routing is the *form* the loader brought back and nothing else — a
/// pyramid fills a geometry slot, analyses fill a texture slot — which is what
/// lets one function serve a mapped file, a page's `fetch` and a server
/// buffer's reply alike. The forms an element takes home never reach here: the
/// loader forked on `Needs::slot` before calling.
pub(crate) fn place_in_slot(
    data: Loaded,
    id: i32,
    gpu: &Gpu,
    renderers: &Renderers,
    waveforms: &mut HashMap<i32, WaveformSlot>,
    spectrograms: &mut HashMap<i32, SpectrogramSlot>,
) -> Option<usize> {
    match data {
        // A pyramid fills the geometry slot whether it summarizes a copy or a
        // mapping — the slot draws a picture and does not care which.
        Loaded::Peaks(data) | Loaded::Shared(data) => {
            let slot = waveform_slot(data);
            let total = slot.view.total_samples();
            waveforms.insert(id, slot);
            Some(total)
        }
        Loaded::Stfts(stfts) => {
            let slot = spectrogram_slot(stfts, gpu, renderers)?;
            let total = slot.total_samples();
            spectrograms.insert(id, slot);
            Some(total)
        }
        Loaded::Samples(_) | Loaded::Raw { .. } => {
            warn!("widget {id}: raw samples cannot fill a GPU slot");
            None
        }
    }
}

/// Feeds a **retained** time-frequency view the columns its rolling analysis
/// just produced, creating the slot the first time and following a live change
/// of the retained span. Returns the view's length in samples when the picture
/// moved, for the axis that has to know how long it is.
///
/// The upload is **the new columns only**. The texture is allocated once for
/// the whole span and a landing column costs one texel write, so the cost
/// follows the *hop* — where rebuilding the transform each tick made it follow
/// the *span*, and a minute of retention cost eight times an eight-second one
/// to show the same two new columns.
///
/// Both fronts call it, which is what keeps a browser waterfall and a desktop
/// one the same picture built the same way.
fn roll_into_slot(
    slots: &mut HashMap<i32, SpectrogramSlot>,
    id: i32,
    columns: &[f32],
    (window_size, hop, sample_rate): (usize, usize, f32),
    capacity: usize,
    gpu: &Gpu,
    renderers: &Renderers,
) -> Option<usize> {
    // A `/gui_set` of the analysis restarts the roll upstream, so a slot whose
    // ring was built against the old geometry is not the same picture and is
    // rebuilt rather than pushed into.
    let stale = slots.get(&id).is_none_or(|slot| {
        slot.views.first().is_none_or(|v| {
            let s = v.stft();
            !s.is_rolling()
                || (s.window_size(), s.hop(), s.sample_rate()) != (window_size, hop, sample_rate)
        })
    });
    if stale {
        if columns.is_empty() {
            return None;
        }
        let view = SpectrogramView::rolling(
            &gpu.device,
            &gpu.queue,
            &renderers.spectrogram,
            capacity,
            window_size,
            hop,
            sample_rate,
        );
        slots.insert(id, SpectrogramSlot { views: vec![view] });
    }
    let view = slots.get_mut(&id)?.views.first_mut()?;
    view.set_retention(&gpu.device, &gpu.queue, &renderers.spectrogram, capacity);
    view.push_columns(&gpu.queue, columns);
    (view.stft().n_frames() > 0).then(|| view.total_samples())
}

/// One STFT per channel for a spectrogram lane set: de-interleaved `channels`,
/// analyzed at `window_size`/`hop` (the hop raised by [`hop_capped`] so a long
/// buffer fits the magnitude texture) and `sample_rate` (48 kHz when unknown,
/// so the frequency axis is still drawable). Shared by both fronts and every
/// data source (mapped path, fetched buffer, inline samples).
pub(crate) fn stft_lanes(
    channels: Vec<Vec<f32>>,
    window_size: usize,
    hop: usize,
    sample_rate: f64,
) -> Vec<Stft> {
    let sr = if sample_rate > 0.0 {
        sample_rate as f32
    } else {
        48_000.0
    };
    channels
        .into_iter()
        .map(|ch| {
            let hop = hop_capped(ch.len(), window_size, hop);
            Stft::compute(&ch, window_size, hop, sr)
        })
        .collect()
}

/// De-interleaves `channels` channels out of a flat buffer (a trailing partial
/// frame is ignored) — the front half of [`stft_lanes`] for inline sources.
pub(crate) fn deinterleave(samples: &[f32], channels: usize) -> Vec<Vec<f32>> {
    let channels = channels.max(1);
    let frames = samples.len() / channels;
    (0..channels)
        .map(|ch| (0..frames).map(|f| samples[f * channels + ch]).collect())
        .collect()
}

/// How long a filled slot's picture turned out to be, in samples — what the
/// widget's navigation axis has to know, or the visible window falls back to a
/// span the size of the body and the whole picture draws stretched.
///
/// The two are set through different doors: a stored extent is fixed and joins
/// the navigation group as it is, while a **rolling** one slides — the axis
/// follows the newest column until someone navigates it and then holds where
/// they left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Extent {
    Stored(usize),
    Rolling(usize),
}

/// **The window's GPU slots are gone** — a fresh device, a re-attached canvas
/// — so every widget that had filled one hands its content over again on the
/// next [`fill_slots`]. The device is the front's, and this is how the tree is
/// told that what it gave away did not survive it.
pub(crate) fn slots_dropped(widget: &mut Widget) {
    widget.kind.slot_dropped();
    for child in &mut widget.children {
        slots_dropped(child);
    }
}

/// **Uploads whatever the tree has for its GPU slots**, keyed by the id that
/// addresses each widget: its own, or — for a clip's body, which carries none —
/// its container's. Returns the extents the fills produced, for the caller to
/// register with the navigation groups once the tree borrow is over.
///
/// This is the filling half of the slot seam, and the whole of it: an element
/// hands over a pyramid, a set of analyses or the columns its rolling transform
/// just produced ([`WidgetKind::fill`]), and this walk uploads them. It asks
/// every widget the same question and learns nothing about any of them — where
/// the two fronts each used to walk the tree twice, once matching on the
/// presentation to build a slot out of an element's inline samples and once
/// reaching into a waterfall's transform for the columns of the tick.
///
/// Cheap to call every tick by construction: an element with nothing new hands
/// back `None`, so a still window uploads nothing.
pub(crate) fn fill_slots(
    widget: &mut Widget,
    owner: Option<i32>,
    gpu: &Gpu,
    renderers: &Renderers,
    waveforms: &mut HashMap<i32, WaveformSlot>,
    spectrograms: &mut HashMap<i32, SpectrogramSlot>,
    out: &mut Vec<(i32, Extent)>,
) {
    let owner = widget.id.or(owner);
    if let (Some(id), Some(fill)) = (owner, widget.kind.fill()) {
        let extent = match fill {
            // A fill over a slot that is already there **keeps the view**: the
            // picture is the element's and the navigation is the eye's, so a
            // refill — which is what a destructive edit produces — must not
            // snap the amplitude window back to full scale mid-stroke.
            SlotFill::Geometry(data) => {
                let total;
                match waveforms.get_mut(&id) {
                    Some(slot) => {
                        slot.view.set_data(data);
                        total = slot.view.total_samples();
                    }
                    None => {
                        let slot = waveform_slot(data);
                        total = slot.view.total_samples();
                        waveforms.insert(id, slot);
                    }
                }
                Some(Extent::Stored(total))
            }
            SlotFill::Texture(stfts) => spectrogram_slot(stfts, gpu, renderers).map(|slot| {
                let total = slot.total_samples();
                spectrograms.insert(id, slot);
                Extent::Stored(total)
            }),
            SlotFill::Columns {
                columns,
                window_size,
                hop,
                sample_rate,
                capacity,
            } => roll_into_slot(
                spectrograms,
                id,
                &columns,
                (window_size, hop, sample_rate),
                capacity,
                gpu,
                renderers,
            )
            .map(Extent::Rolling),
        };
        out.extend(extent.map(|e| (id, e)));
    }
    for child in &mut widget.children {
        fill_slots(child, owner, gpu, renderers, waveforms, spectrograms, out);
    }
}

/// The body a timeline view draws into: its rect minus the time-ruler strip
/// under it (when the x ruler is on) and the gutter band to its left — each
/// ruler gets its own space instead of overlaying the view.
/// `indent` is the **group's** gutter, not this view's own `ruler_w`: a
/// waveform sharing an axis with a lane or a roll starts its trace where they
/// start their body, and draws its value ruler into the whole band.
pub(crate) fn timeline_body(
    rect: Rect,
    editor: &EditorProps,
    has_label: bool,
    indent: f32,
    metrics: &Metrics,
) -> Rect {
    let (mut x, mut y, mut w, mut h) = (rect.x, rect.y, rect.w, rect.h);
    // The label strip comes off the top, the same one every other view
    // reserves ([`crate::host::widget::size::label_strip`]) -- a heavy view is
    // not a different kind of widget just because its picture is a texture.
    //
    // **Plus the gap under the caption**, which is the one part of the strip a
    // heavy view has to reserve for itself: everywhere else it arrives with
    // [`crate::host::graphics::controls::body_rect`]'s inset, and a timeline
    // body has none on purpose -- the picture runs edge to edge, so a take's
    // samples fill the box it is given. Without it the caption sits on the
    // samples. The formula is `body_rect`'s vertical half, spelled out.
    let strip = crate::host::widget::size::label_strip(has_label, metrics.text_scale, metrics);
    let strip = if has_label {
        strip + metrics.pad
    } else {
        strip
    };
    let strip = strip.min(h);
    y += strip;
    h -= strip;
    if editor.ruler != Ruler::Off {
        h = (h - metrics.ruler_h).max(0.0);
    }
    let indent = indent.min(w);
    x += indent;
    w = (w - indent).max(0.0);
    Rect::new(x, y, w, h)
}

/// What a drag is holding while this frame is drawn — the frame's answer to
/// *whose* affordances may light up.
///
/// A grip is a promise about the next press, so during a drag it belongs to the
/// clip already held and to nothing else: another clip lighting up would offer
/// a grab that is not on the table, and the held clip's own grip must not blink
/// out every time the pointer wanders between two snap steps (the clip moves in
/// steps; the pointer does not).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Grab {
    /// Nothing is held: affordances follow the pointer.
    #[default]
    None,
    /// Something else is held (a control, a curve, the axis): no clip lights up.
    Other,
    /// This clip is held, by the named side — `None` for a move, where the side
    /// is still the pointer's.
    Clip(i32, Option<crate::host::graphics::track::ClipSide>),
}

/// The live inputs the frame needs beyond the tree and the GPU resources. The
/// native front fills them from its state; the browser front passes the
/// streamed equivalents.
///
/// Two kinds of thing, deliberately separated. [`world`](Self::world) is what
/// nobody owns — the outside, identical for every element of the frame. The
/// fields beside it are **one widget's own interaction state**, fed back down
/// so that widget can draw itself mid-gesture; each of them is a widget that
/// cannot yet hold its own state, and each disappears as its leaf moves behind
/// [`Element`](super::widget::Element).
pub(crate) struct FrameInputs<'a> {
    /// The host's size roles: every layout and paint site of this frame reads
    /// its spacing, control and text sizes from here (see
    /// [`super::metrics`]).
    pub(crate) metrics: &'a Metrics,
    /// The read-only per-frame facts no widget owns (see [`World`]).
    pub(crate) world: World<'a>,
    /// The id of the widget holding the keyboard focus in this window, if any:
    /// the frame rings it, and the element draws whatever else being focused
    /// means to it (a field's caret and selection).
    pub(crate) focused: Option<i32>,
    /// What a drag is holding right now (see [`Grab`]).
    pub(crate) grab: Grab,
}

impl Default for FrameInputs<'_> {
    fn default() -> Self {
        // A 'static empty table for the no-transport case (the world brings its
        // own empties).
        static METRICS: std::sync::OnceLock<Metrics> = std::sync::OnceLock::new();
        Self {
            metrics: METRICS.get_or_init(Metrics::default),
            world: World::default(),
            focused: None,
            grab: Grab::None,
        }
    }
}

/// The shared state a placed timeline widget draws with — the window, the
/// selection and the playhead of its navigation group, which is where all
/// three live. A widget in no group yet (nothing registered its data) falls
/// back to its own def-time props over `fallback`, the window it would have
/// seeded: the same values the group is about to take.
fn chrome_for(
    inputs: &FrameInputs,
    id: i32,
    editor: &EditorProps,
    fallback: impl FnOnce() -> View,
) -> GroupState {
    match inputs.world.timelines.state(group_key(id, editor.link)) {
        Some(state) => *state,
        None => GroupState::seed(editor, fallback()),
    }
}

/// The **placed** navigation window a member's own data is drawn through: the
/// group window shifted so the member's data sample 0 lands at timeline
/// position `offset`. The GPU body upload uses this (its data is in local
/// sample units); the time ruler and the selection/playhead overlay keep the
/// timeline-unit window. At `offset = 0` (the un-placed default) it is the
/// identity.
fn placed_nav(nav: &View, offset: f64) -> View {
    View {
        start: nav.start - offset,
        len: nav.len,
    }
}

/// **The span a view was asked to draw and could not answer**, or `None` when
/// it could — the fetch this frame is owed.
///
/// A column finer than the summary's base bucket can only come from samples;
/// a view holding none for that span draws the bucket, which is the honest
/// picture and one the eye has stopped getting anything new from. So the
/// answer is the visible span, clamped to the samples, and it is `None` in
/// every other case: zoomed out (the summary *is* the answer), or covered
/// already (mapped samples, a whole owned buffer, a window over this span).
///
/// The span is the window as drawn and not a margin around it: a window is
/// fetched because somebody is looking at it, and guessing where they will
/// look next is a cache policy this deliberately does not have.
///
/// **What the view could not draw, and in what shape** — or `None` when it
/// answered for everything it showed.
///
/// A column finer than the summary's base bucket can only come from something
/// finer than the summary: a view holding neither draws the bucket, which is
/// the honest picture and one the eye has stopped getting anything new from.
/// So the answer is the visible span, clamped to the samples, and it is `None`
/// in every other case: zoomed out (the summary *is* the answer), or answered
/// already (mapped samples, a whole owned buffer, a window over this span, a
/// finer grid over this span).
///
/// **Which shape is owed is a question about the zoom alone**, and it is
/// [`Owed`]'s whole subject: zoomed out the picture is min/max columns, which a
/// finer *summary* answers in one reply of a few kilobytes; zoomed in far
/// enough the summary stops being cheaper than the samples it describes — and
/// past that the trace is the polyline through the samples, where only the
/// samples will do. [`detail_bucket`] draws the line and picks the grid: a
/// column holds two buckets or more, so the position error of the fold stays
/// under half a column, and the bucket is coarsened until the whole span fits
/// one reply, because a second reply would replace the first rather than extend
/// it.
///
/// The span is the window as drawn and not a margin around it: a window is
/// fetched because somebody is looking at it, and guessing where they will
/// look next is a cache policy this deliberately does not have.
///
/// **A take still being written is asked for what is behind the frontier, and
/// never for what is past it** (`written`, the `fills` prop's frontier). Past
/// it there is nothing to read: the buffer holds the zeros it was allocated
/// with, and a run or a bucket over them would claim measured silence over
/// audio that has not arrived. Behind it the samples are **final** — a recorder
/// writes forward and does not come back, and the frontier is what the writer
/// says it has already written — so a span that ends there is as readable as
/// any other, and a page zoomed past its summary sees the samples rather than
/// the bucket the stream reports.
///
/// This is what makes a page's zoom the window's: natively the samples are the
/// mapped cells, so a view answers for any span with nothing told to it and
/// this function returns `None` on `covers` alone. A client that cannot map
/// took the same picture only down to the bucket, because `fills` was reading
/// as *this take is not readable* rather than as *this much of it exists*.
fn owed(view: &WaveformView, nav: &View, width: f64, written: Option<u64>) -> Option<Owed> {
    let data = view.data();
    let total = data.total_samples();
    if width <= 0.0 || nav.len <= 0.0 || total == 0 {
        return None;
    }
    let per_px = nav.len / width;
    let base = data.base_bucket();
    if per_px >= base as f64 {
        return None; // the summary answers at this zoom, exactly as it should
    }
    let a = (nav.start.floor().max(0.0) as usize).min(total);
    let b = (nav.start + nav.len).ceil().max(0.0) as usize;
    let b = b.clamp(a, total);
    // The frontier is the ceiling, and it is the only thing `fills` does here.
    // No margin under it: the frames the report counts were written before it
    // was measured, so the last of them is as settled as the first. The margin
    // that would be needed is *above* — which is not a margin but the clamp
    // itself, since what the report has not counted yet may not be there.
    let b = written.map_or(b, |w| b.min(w as usize));
    if b <= a || data.covers(a, b) {
        return None;
    }
    match detail_bucket(per_px, b - a, base) {
        Some(bucket) if !data.detail_covers(a, b, per_px) => Some(Owed::Summary { a, b, bucket }),
        Some(_) => None,
        None => Some(Owed::Samples { a, b }),
    }
}

/// **How many buckets one `/buffer_peaks` reply carries** (`docs/schemas.md`),
/// which is what bounds the span one request can summarize.
const DETAIL_REPLY_BUCKETS: usize = 4096;

/// **The grid to ask for a span of `span` frames at `per_px` samples a pixel**,
/// or `None` where no summary is worth asking for and the samples are the
/// answer.
///
/// Two rules meet here. The picture wants **two buckets a column or more**, so
/// the fold's position error stays under half a pixel — one bucket a column
/// would put it at a whole one, which is the resolution the coarse summary
/// already has and the reason this is being asked for at all. And the span has
/// to fit **one reply**: a detail grid is replaced rather than extended (one
/// grid per view, like one window per view), so a span that would need two
/// requests is summarized at a coarser bucket instead, which is still finer
/// than the view's own and still one round trip.
///
/// It returns a power of two so a zoom holds still: a grid at `bucket` answers
/// every column from `bucket` samples wide upwards (the levels above it are the
/// same pyramid), so zooming *out* is free and zooming *in* asks again only
/// after a factor of two.
fn detail_bucket(per_px: f64, span: usize, base: usize) -> Option<usize> {
    // Two buckets to a column, and never finer than a summary is worth.
    let target = per_px * 0.5;
    if span == 0 || target < MIN_DETAIL_BUCKET as f64 {
        return None;
    }
    let mut bucket = MIN_DETAIL_BUCKET;
    while (bucket * 2) as f64 <= target {
        bucket *= 2;
    }
    // Coarsened until one reply holds the whole span.
    while bucket < base && span.div_ceil(bucket) > DETAIL_REPLY_BUCKETS {
        bucket *= 2;
    }
    (bucket < base).then_some(bucket)
}

/// **The bucket below which a summary is not worth asking for**, and the number
/// is the wire's own. A bucket is three floats (min, max, mean square) where a
/// sample is one, so a grid only pays where a bucket holds *well* more than
/// three samples: at four it carries three quarters of what it describes, which
/// is no saving at all for a second grid to keep, and at sixteen it carries a
/// fifth. Below that the samples are both nearly as cheap and **exact**, and
/// they answer every deeper zoom as well — a grid answers only down to its own
/// bucket.
///
/// With a column holding two buckets or more, this puts the crossing at about
/// **thirty-two samples a pixel**: coarser than that a view asks for the grid,
/// finer than it a view reads the samples and keeps them.
const MIN_DETAIL_BUCKET: usize = 16;

/// Maps sample position `s` into `body`'s x range through `nav`.
fn sample_to_x(s: f64, nav: &View, body: Rect) -> f32 {
    (body.x as f64 + (s - nav.start) / nav.len * body.w as f64) as f32
}

/// **The ink a placed widget draws with**: the opacity its subtree resolved to
/// ([`Widget::alpha`]) and its declared corner radius in the pixels of the
/// space it was placed in — the wire's number through the placement's own
/// table, exactly like every other declared length, so a widget inside a zoomed
/// workspace rounds by as much as it grew.
///
/// One function because both meshes and every draw pass ask the same question,
/// and because it is the only place the two props meet the frame at all: an
/// element is never told it is being faded.
///
/// [`Widget::alpha`]: super::widget::Widget::alpha
pub(crate) fn ink_of(p: &layout::Placed) -> Ink {
    Ink {
        alpha: p.widget.alpha,
        radius: p.widget.radius.map_or(0.0, |r| p.metrics.px(r)),
    }
}

/// The lane sub-rectangle `ch` of `lanes` inside `body` (stacked top to
/// bottom, no gap — the divider line is overlay chrome).
///
/// The **third** row a view stacks, beside a roll's semitone and a
/// multitrack's lane, and the same structure: a band of the vertical axis. So
/// it is a [`Bands`] like the other two, on its uniform arm — a channel stack
/// divides its body evenly because every channel is worth the same picture.
pub(crate) fn lane_rect(body: Rect, lanes: usize, ch: usize) -> Rect {
    let lanes = lanes.max(1);
    let (y, h) = Bands::uniform(lanes, body.h / lanes as f32).band(ch);
    Rect::new(body.x, body.y + y, body.w, h)
}

/// The time-ruler unit of `editor` (the beats grid rides its props).
fn time_unit(editor: &EditorProps) -> TimeUnit {
    match editor.ruler {
        Ruler::Samples => TimeUnit::Samples,
        Ruler::Beats => TimeUnit::Beats {
            tempo: editor.tempo,
            beat_at: editor.beat_at,
            quant: editor.quant,
        },
        _ => TimeUnit::Seconds,
    }
}

/// The stacked-lane index under window y `cy` (clamped into range).
pub(crate) fn lane_at(body: Rect, lanes: usize, cy: f64) -> usize {
    let rel = ((cy - body.y as f64) / body.h.max(1.0) as f64).clamp(0.0, 1.0);
    ((rel * lanes as f64) as usize).min(lanes.saturating_sub(1))
}

/// Renders `tree` into `gpu`'s surface, using the window's `painter`/`overlay`
/// (chrome under and over the heavy views), the `waveforms`/`spectrograms`/
/// `canvases` GPU resources and (read-only) `scopes` histories, plus `inputs`
/// for the live values. One immutable mesh-building pass over the placed
/// widgets, then the GPU uploads and the single render pass.
#[allow(clippy::too_many_arguments)] // the per-window resource set, both fronts
pub(crate) fn render(
    gpu: &mut Gpu,
    renderers: &mut Renderers,
    painter: &mut Painter,
    overlay: &mut Painter,
    waveforms: &mut HashMap<i32, WaveformSlot>,
    spectrograms: &mut HashMap<i32, SpectrogramSlot>,
    canvases: &mut HashMap<i32, CanvasView>,
    tree: &Widget,
    inputs: &FrameInputs,
    theme: &Theme,
) {
    let (fb_w, fb_h) = (gpu.config.width.max(1), gpu.config.height.max(1));
    let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    // The lanes' clips are placed on the axis their group currently stands at,
    // so the layout of a multitrack follows the zoom and the pan.
    let placed = layout::layout_on(area, tree, inputs.metrics, &|id, link| {
        inputs.world.timelines.nav(group_key(id, link))
    });
    let mut mesh = Mesh::new();
    let mut over = Mesh::new();
    let collected = collect_widgets(&placed, &mut mesh, inputs, theme);

    draw_timeline_meshes(
        &mut mesh,
        &mut over,
        &collected,
        waveforms,
        spectrograms,
        inputs,
        theme,
    );
    draw_static_meshes(&mut mesh, &mut over, &collected, inputs, theme, tree);
    draw_element_overlays(&mut over, &placed, inputs, theme);

    mesh.set_clip(None);
    over.set_clip(None);
    mesh.set_ink(Ink::default());
    over.set_ink(Ink::default());
    painter.upload(&gpu.device, &gpu.queue, &mesh, fb_w, fb_h);
    overlay.upload(&gpu.device, &gpu.queue, &over, fb_w, fb_h);
    for item in &collected.timeline_items {
        // The body the element stated when it described its frame: one
        // rectangle, so the picture and the chrome around it agree.
        let body = item.body;
        match &item.kind {
            // A waveform's picture is triangles, and they went into the
            // window's mesh with the rest of the chrome: nothing to prepare.
            TimelineKind::Waveform { .. } => {}
            TimelineKind::Spectrogram { freq, look } => {
                if let Some(slot) = spectrograms.get_mut(&item.id) {
                    let nav = chrome_for(inputs, item.id, &item.editor, || {
                        View::full(slot.total_samples())
                    })
                    .nav;
                    let nav = placed_nav(&nav, item.editor.offset);
                    let lanes = slot.views.len();
                    for (ch, view) in slot.views.iter_mut().enumerate() {
                        view.set_display(
                            look.db_floor,
                            look.db_ceil,
                            look.freq_scale,
                            look.colormap.max(0) as u32,
                        );
                        view.set_freq_window(freq.0, freq.1);
                        view.set_framing(framing_of(lane_rect(body, lanes, ch), fb_w, fb_h));
                        view.upload(
                            &gpu.device,
                            &gpu.queue,
                            renderers,
                            &nav,
                            body.w.max(1.0) as u32,
                        );
                    }
                }
            }
        }
    }
    // The spectral clip bodies: the same texture, uploaded against the clip's
    // own axis instead of the group's window — which is the whole difference
    // between a spectral *view* of a file and a spectral *clip* of it.
    for item in &collected.spectral_bodies {
        if let Some(slot) = spectrograms.get_mut(&item.id) {
            let lanes = slot.views.len();
            for (ch, view) in slot.views.iter_mut().enumerate() {
                view.set_display(
                    item.db_floor,
                    item.db_ceil,
                    item.freq_scale,
                    item.colormap.max(0) as u32,
                );
                view.set_framing(framing_of(lane_rect(item.rect, lanes, ch), fb_w, fb_h));
                view.upload(
                    &gpu.device,
                    &gpu.queue,
                    renderers,
                    &item.local,
                    item.rect.w.max(1.0) as u32,
                );
            }
        }
    }
    // Recompile any canvas whose shader changed, then push its per-frame uniforms
    // (viewport size, elapsed time, resolved params).
    for frame in &collected.canvas_frames {
        if let Some(view) = canvases.get_mut(&frame.id) {
            view.set_shader(&gpu.device, &frame.shader);
            let time = view.elapsed();
            let res = [frame.body.w.max(1.0), frame.body.h.max(1.0)];
            let framing = framing_of(frame.body, fb_w, fb_h);
            view.upload(&gpu.queue, res, time, frame.params, framing);
        }
    }

    let frame = match gpu.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
        _ => {
            // No drawable this turn (outdated/timed-out surface — e.g. the
            // compositor stopped consuming a covered window's frames):
            // reconfigure and ask for another redraw, so the frame that was
            // requested is not silently dropped and the window never shows
            // stale state once it is presentable again.
            gpu.surface.configure(&gpu.device, &gpu.config);
            gpu.window.request_redraw();
            return;
        }
    };
    let target = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gui frame"),
        });
    // Antialiasing is a property of the **attachment**, so it is the whole of
    // what MSAA changes here: with it on, every pipeline draws into the
    // multisampled texture and the GPU resolves that into the surface as the
    // pass ends. One flag, one texture per window, nothing per widget.
    let (attachment, resolve_target) = match gpu.msaa_view() {
        Some(ms) => (ms, Some(&target)),
        None => (&target, None),
    };
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gui pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: attachment,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear_color(theme)),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        painter.draw(&mut pass);
        for item in &collected.timeline_items {
            // The body the element stated when it described its frame: one
            // rectangle, so the picture and the chrome around it agree.
            let body = item.body;
            if body.w < 1.0 || body.h < 1.0 {
                continue;
            }
            if !apply_scissor(&mut pass, item.clip, fb_w, fb_h) {
                continue;
            }
            match &item.kind {
                TimelineKind::Waveform { .. } => {}
                TimelineKind::Spectrogram { .. } => {
                    let Some(slot) = spectrograms.get(&item.id) else {
                        continue;
                    };
                    let lanes = slot.views.len();
                    for (ch, view) in slot.views.iter().enumerate() {
                        let lane = lane_rect(body, lanes, ch);
                        let (x, y, w, h) = clamp_viewport(lane, fb_w, fb_h);
                        if w >= 1.0 && h >= 1.0 {
                            pass.set_viewport(x, y, w, h, 0.0, 1.0);
                            view.draw(&mut pass, renderers);
                        }
                    }
                }
            }
        }
        for item in &collected.spectral_bodies {
            let Some(slot) = spectrograms.get(&item.id) else {
                continue;
            };
            if item.rect.w < 1.0
                || item.rect.h < 1.0
                || !apply_scissor(&mut pass, item.clip, fb_w, fb_h)
            {
                continue;
            }
            let lanes = slot.views.len();
            for (ch, view) in slot.views.iter().enumerate() {
                let lane = lane_rect(item.rect, lanes, ch);
                let (x, y, w, h) = clamp_viewport(lane, fb_w, fb_h);
                if w >= 1.0 && h >= 1.0 {
                    pass.set_viewport(x, y, w, h, 0.0, 1.0);
                    view.draw(&mut pass, renderers);
                }
            }
        }
        for frame in &collected.canvas_frames {
            if frame.body.w >= 1.0
                && frame.body.h >= 1.0
                && let Some(view) = canvases.get(&frame.id)
                && apply_scissor(&mut pass, frame.clip, fb_w, fb_h)
            {
                let (x, y, w, h) = clamp_viewport(frame.body, fb_w, fb_h);
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                view.draw(&mut pass);
            }
        }
        // The editor chrome reads over the heavy views: reset the viewport
        // (and the scissor) to the full framebuffer first (the overlay mesh is
        // in window space, already geometry-clipped where it needed to be).
        pass.set_viewport(0.0, 0.0, fb_w as f32, fb_h as f32, 0.0, 1.0);
        pass.set_scissor_rect(0, 0, fb_w, fb_h);
        overlay.draw(&mut pass);
    }
    gpu.queue.submit(std::iter::once(encoder.finish()));
    // The winit present contract: lets winit attach the compositor frame
    // callback to this commit, so later `request_redraw`s are delivered (and
    // throttled) correctly — without it, Wayland redraw delivery can stall on
    // an unfocused or covered window until the compositor repaints it anyway.
    gpu.window.pre_present_notify();
    frame.present();
}

/// Applies a placed widget's clip as the pass scissor (the full framebuffer
/// when it has none), returning `false` when the clip is empty — the caller
/// skips the draw entirely. The heavy views draw through `set_viewport`, which
/// *positions and scales* but does not cut; a scrolled view poking out of its
/// `scroll` container is cut by this scissor, the GPU sibling of the mesh's
/// geometric clip. What the scissor cannot reach is the **window** edge, since
/// a viewport may not leave the attachment at all — that is [`Framing`]'s
/// half of the same job.
fn apply_scissor(
    pass: &mut wgpu::RenderPass<'_>,
    clip: Option<Rect>,
    fb_w: u32,
    fb_h: u32,
) -> bool {
    let Some(c) = clip else {
        pass.set_scissor_rect(0, 0, fb_w, fb_h);
        return true;
    };
    let x = c.x.clamp(0.0, fb_w as f32) as u32;
    let y = c.y.clamp(0.0, fb_h as f32) as u32;
    let w = (c.w.max(0.0) as u32).min(fb_w - x);
    let h = (c.h.max(0.0) as u32).min(fb_h - y);
    if w == 0 || h == 0 {
        return false;
    }
    pass.set_scissor_rect(x, y, w, h);
    true
}

/// The part of a widget rect the framebuffer can hold, as `set_viewport` wants
/// it (that call rejects a viewport leaving the attachment).
///
/// It is the **intersection**, not a clamp of the origin: a rect starting above
/// the window keeps its far edge where it is instead of sliding down with its
/// origin. What the viewport still cannot do is cut — it scales whatever the
/// view draws into whatever rectangle it is given — so a view that is only
/// partly visible also gets a [`Framing`] built from this pair, and places its
/// geometry for the full rect inside it.
pub(crate) fn clamp_viewport(r: Rect, fb_w: u32, fb_h: u32) -> (f32, f32, f32, f32) {
    let x0 = r.x.max(0.0);
    let y0 = r.y.max(0.0);
    let x1 = (r.x + r.w).min(fb_w as f32);
    let y1 = (r.y + r.h).min(fb_h as f32);
    (x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// The [`Framing`] a rect is drawn with in this framebuffer: the identity while
/// it fits, and the placement that keeps its picture at a fixed size once the
/// window edge starts cutting it.
pub(crate) fn framing_of(r: Rect, fb_w: u32, fb_h: u32) -> Framing {
    Framing::new((r.x, r.y, r.w, r.h), clamp_viewport(r, fb_w, fb_h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::guidef::GuiNode;

    /// **Every widget survives being squeezed to nothing.**
    ///
    /// This is the one class of state a hand on a window's corner reaches and
    /// no other test does: nothing in this project resizes a window, every
    /// suite draws into a mesh at a size it chose, and every example opens at
    /// the size its GuiDef declares. So a widget *smaller than its own
    /// contents* — a lane under a line of text, a body under its rulers, a
    /// strip under its widest label — is arithmetic nobody exercises, and each
    /// piece of it has a lower bound nobody wrote down. One of them was a
    /// panic (`ruler::draw_ticks_v`, between four pixels and a caption's
    /// height, found 2026-08-26 by dragging a corner in).
    ///
    /// It is deliberately **one table rather than a case per widget**. A case
    /// per widget could assert more — *what* a squeezed widget drops, and in
    /// what order — but nothing would oblige anyone to write one, and a widget
    /// added next year would not have it. Here a new widget is one row, which
    /// is the property that keeps the coverage from rotting. What it asserts is
    /// only the floor: it draws, and it does not panic.
    ///
    /// The walk goes through the **frame**, not through `Element::draw`,
    /// because that is where the crash was: a heavy view's chrome is drawn by
    /// the frame's own pass, and an element-level test would have missed it.
    #[test]
    fn every_widget_survives_being_squeezed_to_nothing() {
        // One row per widget: the wire `type` and the props that give it
        // something to draw. Rulers and labels are on wherever they exist,
        // since a strip and a caption are what a squeeze runs out of room for
        // first.
        const TABLE: &[(&str, &str)] = &[
            ("label", r#"{"id":1,"type":"label","text":"a caption"}"#),
            (
                "button",
                r#"{"id":1,"type":"button","text":"go","label":"run"}"#,
            ),
            (
                "toggle",
                r#"{"id":1,"type":"toggle","value":1,"label":"on"}"#,
            ),
            (
                "knob",
                r#"{"id":1,"type":"knob","min":0,"max":1,"value":0.5,"label":"cutoff"}"#,
            ),
            (
                "slider",
                r#"{"id":1,"type":"slider","min":0,"max":1,"value":0.5,"label":"mix"}"#,
            ),
            (
                "number",
                r#"{"id":1,"type":"number","min":0,"max":9,"value":4,"label":"n"}"#,
            ),
            (
                "text",
                r#"{"id":1,"type":"text","value":"typed","label":"name"}"#,
            ),
            (
                "menu",
                r#"{"id":1,"type":"menu","options":["a","b"],"value":0,"label":"pick"}"#,
            ),
            (
                "meter",
                r#"{"id":1,"type":"meter","value":0.5,"label":"out"}"#,
            ),
            (
                "keys",
                r#"{"id":1,"type":"keys","min":48,"max":72,"label":"piano"}"#,
            ),
            (
                "curve",
                r#"{"id":1,"type":"curve","points":[0,0,0.5,1,1,0],"label":"env"}"#,
            ),
            (
                "notes",
                r#"{"id":1,"type":"notes","notes":[[0,0.5,60,100],[1,0.5,64,100]],
                    "min":48,"max":72,"ruler":"time","label":"roll"}"#,
            ),
            (
                "nodes",
                r#"{"id":1,"type":"nodes","nodes":[[1,"g",0],[2,"beep",1]],"label":"tree"}"#,
            ),
            // The signal family is one type and six pictures, and the crash was
            // in a navigable one with both rulers and a label: every axis it
            // has is a strip that can run out of room.
            (
                "signal/trace",
                r#"{"id":1,"type":"signal","view":"trace","min":-1,"max":1,
                    "data":[0,0.5,-0.5,1,-1,0.25,0,-0.75],
                    "ruler":"time","ruler_y":"norm","label":"a plot"}"#,
            ),
            (
                "signal/trace navigable",
                r#"{"id":1,"type":"signal","view":"trace","navigable":1,"min":-1,"max":1,
                    "data":[0,0.5,-0.5,1,-1,0.25,0,-0.75],
                    "ruler":"time","ruler_y":"db","label":"a take"}"#,
            ),
            (
                "signal/spectrogram navigable",
                r#"{"id":1,"type":"signal","view":"spectrogram","navigable":1,"bus":0,
                    "retention":4.0,"ruler":"time","ruler_y":"hz","label":"waterfall"}"#,
            ),
            (
                "signal/spectrum",
                r#"{"id":1,"type":"signal","view":"spectrum","bus":0,
                    "ruler":"time","ruler_y":"db","label":"spectrum"}"#,
            ),
            (
                "signal/phase",
                r#"{"id":1,"type":"signal","view":"phase","bus":0,"label":"goniometer"}"#,
            ),
            // Containers: a lane with a clip on it, a plane with boxes (the
            // patcher), a free-standing ruler, a scrolling workspace.
            (
                "field/track",
                r#"{"id":1,"type":"field","label":"lane","ruler":"time","header_w":80,
                    "children":[{"id":2,"type":"field","at":0,"dur":2,"label":"clip"}]}"#,
            ),
            (
                "field/timeruler",
                r#"{"id":1,"type":"field","ruler":"time"}"#,
            ),
            (
                "plane/patcher",
                r#"{"id":1,"type":"plane",
                    "boxes":[[0,0,"beep",1,1],[80,60,"out",1,0]],"cords":[[0,0,1,0]]}"#,
            ),
            (
                "plane/scroll",
                r#"{"id":1,"type":"plane","children":[
                {"id":2,"type":"label","text":"inside"}]}"#,
            ),
            (
                "layout",
                r#"{"id":1,"type":"layout","children":[
                {"id":2,"type":"knob","min":0,"max":1,"value":0.5,"label":"k"},
                {"id":3,"type":"label","text":"beside it"}]}"#,
            ),
            (
                "layout/stack",
                r#"{"id":1,"type":"layout","flow":"stack","children":[
                {"id":2,"type":"label","text":"one"},
                {"id":3,"type":"label","text":"two"}]}"#,
            ),
            // A clip with all three bodies is the deepest stack of chrome the
            // catalog has: a take, the events over it and an envelope over
            // both, each with its own axis inside somebody else's rectangle.
            (
                "field/clip with three bodies",
                r#"{"id":1,"type":"field","label":"lane","ruler":"time","children":[
                    {"id":2,"type":"field","at":0,"dur":4,"label":"take",
                     "data":[0,0.5,-0.5,1,-1,0.25,0,-0.75],
                     "notes":[[0,0.5,60,100],[1,0.5,64,100]],
                     "points":[0,0,0.5,1,1,0]}]}"#,
            ),
        ];

        // Engraving is a Cargo feature, so its row is too.
        #[cfg(feature = "notation")]
        const NOTATION: &[(&str, &str)] = &[(
            "score",
            r#"{"id":1,"type":"score","display_list":[],"label":"page"}"#,
        )];
        #[cfg(not(feature = "notation"))]
        const NOTATION: &[(&str, &str)] = &[];

        let theme = Theme::default();
        let inputs = FrameInputs::default();
        let waveforms = HashMap::new();
        let spectrograms = HashMap::new();

        for (name, json) in TABLE.iter().chain(NOTATION) {
            let node = GuiNode::parse(json.as_bytes())
                .unwrap_or_else(|e| panic!("{name}: the row does not parse: {e}"));
            let tree = Widget::from_node(1, &node, &[])
                .unwrap_or_else(|e| panic!("{name}: the row does not build: {e}"));
            // A typo in a row would parse, build as `Unknown`, draw nothing and
            // leave this test green over a widget it never touched. So each row
            // has to be the widget it names, and has to draw at a size that is
            // not a squeeze at all — the walk below only proves it *survives*,
            // never that it was there.
            assert!(
                !matches!(tree.kind, WidgetKind::Unknown(_)),
                "{name}: the row built as an unknown type"
            );

            // Every size on the way down, on each axis alone and on both at
            // once — the diagonal is how the crash was actually reached, and
            // the single axes are where a strip runs out before a body does.
            // Half-pixel steps because a text scale lands on them.
            let mut steps = 0;
            let at = |w: f32, h: f32| {
                let area = Rect::new(0.0, 0.0, w, h);
                let placed = layout::layout_on(area, &tree, inputs.metrics, &|_, _| None);
                let mut mesh = Mesh::new();
                let mut over = Mesh::new();
                let collected = collect_widgets(&placed, &mut mesh, &inputs, &theme);
                draw_timeline_meshes(
                    &mut mesh,
                    &mut over,
                    &collected,
                    &waveforms,
                    &spectrograms,
                    &inputs,
                    &theme,
                );
                draw_static_meshes(&mut mesh, &mut over, &collected, &inputs, &theme, &tree);
                draw_element_overlays(&mut over, &placed, &inputs, &theme);
            };
            {
                let area = Rect::new(0.0, 0.0, 400.0, 300.0);
                let placed = layout::layout_on(area, &tree, inputs.metrics, &|_, _| None);
                let mut mesh = Mesh::new();
                let mut over = Mesh::new();
                let collected = collect_widgets(&placed, &mut mesh, &inputs, &theme);
                draw_static_meshes(&mut mesh, &mut over, &collected, &inputs, &theme, &tree);
                assert!(
                    !mesh.is_empty() || !over.is_empty(),
                    "{name}: draws nothing even with room, so the walk proves nothing"
                );
            }

            let mut v = 240.0f32;
            while v >= 0.0 {
                at(v, 200.0);
                at(300.0, v);
                at(v, v);
                steps += 3;
                v -= 0.5;
            }
            assert!(steps > 1000, "{name}: the walk did not run");
        }
    }

    /// The viewport is the rect **intersected** with the framebuffer, not its
    /// origin clamped into it: a lane starting above the window keeps its far
    /// edge where it is instead of sliding down, and the framing built from the
    /// pair then cuts the picture there.
    #[test]
    fn a_viewport_is_the_intersection_and_the_framing_follows() {
        let (fb_w, fb_h) = (800u32, 600u32);
        // Wholly inside: the viewport is the rect and nothing is framed.
        let inside = Rect::new(10.0, 20.0, 300.0, 120.0);
        assert_eq!(
            clamp_viewport(inside, fb_w, fb_h),
            (10.0, 20.0, 300.0, 120.0)
        );
        assert_eq!(framing_of(inside, fb_w, fb_h), Framing::IDENTITY);

        // Past the bottom: the height is what is left, and the picture keeps
        // its size (a scale of 2 for half of it showing).
        let below = Rect::new(0.0, 560.0, 300.0, 80.0);
        assert_eq!(clamp_viewport(below, fb_w, fb_h), (0.0, 560.0, 300.0, 40.0));
        assert_eq!(framing_of(below, fb_w, fb_h).scale[1], 2.0);

        // Above the top: the far edge stays at y 40, so 40 px are visible -
        // where clamping the origin would have kept the full 80 and slid the
        // whole picture down into the window.
        let above = Rect::new(0.0, -40.0, 300.0, 80.0);
        assert_eq!(clamp_viewport(above, fb_w, fb_h), (0.0, 0.0, 300.0, 40.0));
        let f = framing_of(above, fb_w, fb_h);
        assert_eq!(f.scale[1], 2.0);
        assert!(
            (f.apply(0.0, -1.0).1 + 1.0).abs() < 1e-6,
            "the bottom edge holds"
        );

        // Entirely outside: an empty viewport, which the caller skips.
        let gone = Rect::new(0.0, 700.0, 300.0, 80.0);
        assert_eq!(clamp_viewport(gone, fb_w, fb_h).3, 0.0);
    }

    fn editor(ruler: Ruler, ruler_y: RulerY) -> EditorProps {
        EditorProps {
            ruler,
            ruler_y,
            sample_rate: 0.0,
            bit_depth: 16,
            tempo: 1.0,
            beat_at: 0.0,
            quant: 4.0,
            autofit: true,
            sel_start: 0.0,
            sel_len: 0.0,
            x_start: 0.0,
            x_len: 1.0,
            playhead_at: -1.0,
            playhead: -1.0,
            playhead_loop_start: 0.0,
            playhead_loop_len: 0.0,
            y_start: 0.0,
            y_len: 1.0,
            sel_min: 0.0,
            sel_max: 0.0,
            link: None,
            offset: 0.0,
        }
    }

    /// The whole clip path, from the tree to the geometry: the clip's box, and
    /// then each body over it. This is what the containment has to deliver — the
    /// lane draws no clip, the clip draws no body, and each body draws itself
    /// against the clip's rectangle and axis.
    #[test]
    fn a_clips_bodies_are_collected_and_drawn_over_it() {
        use crate::host::guidef::GuiNode;
        use crate::host::widget::Widget;

        let json = r#"{"type":"window","margin":0,"children":[
            {"id":5,"type":"field","label":"lane","children":[
                {"id":10,"type":"field","offset":0,"dur":400,"data":[0.0,1.0,-1.0,0.5],
                 "notes":[0.0,100.0,60.0],"points":[0.0,0.5,1,0.0,400.0,0.9,1,0.0]}]}]}"#;
        let tree = Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap();
        let m = Metrics::default();
        let inputs = FrameInputs {
            metrics: &m,
            ..FrameInputs::default()
        };
        let area = Rect::new(0.0, 0.0, 800.0, 300.0);
        let placed = layout::layout(area, &tree, &m);
        let mut mesh = Mesh::new();
        let collected = collect_widgets(&placed, &mut mesh, &inputs, &Theme::default());

        assert_eq!(collected.track_items.len(), 1);
        assert_eq!(collected.clip_items.len(), 1);
        assert_eq!(collected.clip_bodies.len(), 3, "a take, a roll and a curve");
        // Every body is drawn against the clip's own rectangle and axis.
        let clip = &collected.clip_items[0];
        for body in &collected.clip_bodies {
            assert_eq!(body.rect, clip.rect);
            assert_eq!(body.dur, 400.0);
        }

        // ...and each of them actually puts geometry down, over the lane and
        // over the clip's own box.
        let paint = |c: &Collected| {
            let (mut base, mut over) = (Mesh::new(), Mesh::new());
            draw_static_meshes(&mut base, &mut over, c, &inputs, &Theme::default(), &tree);
            base.vertex_count()
        };
        let mut collected = collected;
        let full = paint(&collected);
        collected.clip_bodies.clear();
        let no_bodies = paint(&collected);
        collected.clip_items.clear();
        let lane_only = paint(&collected);
        assert!(no_bodies > lane_only, "the clip's box draws over the lane");
        assert!(full > no_bodies, "the bodies draw over the clip's box");
    }

    /// A **bulk** take — the minutes-long one, whose samples never reach the
    /// tree and whose picture comes out of its peak pyramid — draws in its
    /// clip, against the clip's axis.
    ///
    /// The regression it guards: a body is an element, and an element drawn as
    /// a body was reaching its **own** drawing (the plot, which reads the
    /// inline samples and rules its own axes) instead of the body one. A take
    /// with a pyramid and no inline samples is exactly the case where the two
    /// differ visibly — the plot has nothing to read, so the clip came out
    /// empty but for its frame.
    #[test]
    fn a_bulk_take_draws_from_its_pyramid_in_the_clip() {
        use crate::host::guidef::GuiNode;
        use crate::host::widget::Widget;
        use crate::host::widget::element::Loaded;
        use crate::waveform::WaveformData;

        let json = r#"{"type":"window","margin":0,"children":[
            {"id":5,"type":"field","label":"lane","children":[
                {"id":10,"type":"field","offset":0,"dur":100000,"path":"take.f32"}]}]}"#;
        let mut tree =
            Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap();

        // The loader's answer, handed over the way the native walk hands it:
        // a pyramid, and not one sample in the tree.
        let samples: Vec<f32> = (0..100_000)
            .map(|i| (i as f32 * 0.01).sin() * 0.5)
            .collect();
        let body = &mut tree.children[0].children[0].children[0];
        assert!(
            body.kind
                .take_bulk(Loaded::Peaks(Arc::new(WaveformData::new(
                    samples.into(),
                    256
                )))),
            "the take is what the pyramid is for"
        );

        let m = Metrics::default();
        let inputs = FrameInputs {
            metrics: &m,
            ..FrameInputs::default()
        };
        let placed = layout::layout(Rect::new(0.0, 0.0, 800.0, 300.0), &tree, &m);
        let mut mesh = Mesh::new();
        let collected = collect_widgets(&placed, &mut mesh, &inputs, &Theme::default());
        assert_eq!(collected.clip_bodies.len(), 1);

        let paint = |c: &Collected| {
            let (mut base, mut over) = (Mesh::new(), Mesh::new());
            draw_static_meshes(&mut base, &mut over, c, &inputs, &Theme::default(), &tree);
            base.vertex_count()
        };
        let full = paint(&collected);
        let mut bare = collected;
        let width = bare.clip_bodies[0].rect.w as u32;
        bare.clip_bodies.clear();
        let without = paint(&bare);
        // One min/max column per pixel of the clip: the trace, not the baseline
        // alone (which is the six vertices the empty plot came to).
        assert!(
            full - without > width * 4,
            "the take draws its columns ({} vertices over {width} pixels)",
            full - without
        );
    }

    /// **A clip masks what it holds.** Zoomed in far enough that the take is a
    /// polyline through raw samples, the drawing reads the sample before the
    /// left edge and the one after the right — it has to, or the line would
    /// start and end inside the box — and each of them is also marked with a
    /// dot. Those two discs are drawn outside the clip, on the lane beside it,
    /// unless the clip bounds its contents.
    #[test]
    fn a_clips_body_does_not_draw_outside_the_clip() {
        use crate::host::guidef::GuiNode;
        use crate::host::widget::Widget;

        // Twelve samples over a 400-pixel clip: ~33 pixels apart, so the trace
        // is the polyline and every sample carries its dot.
        let json = r#"{"type":"window","margin":0,"children":[
            {"id":5,"type":"field","label":"lane","children":[
                {"id":10,"type":"field","offset":0,"dur":12,
                 "data":[0.0,0.5,-0.5,0.9,-0.9,0.2,-0.2,0.7,-0.7,0.4,-0.4,0.0]}]}]}"#;
        let tree = Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap();
        let m = Metrics::default();
        let inputs = FrameInputs {
            metrics: &m,
            ..FrameInputs::default()
        };
        let placed = layout::layout(Rect::new(0.0, 0.0, 800.0, 300.0), &tree, &m);
        let mut mesh = Mesh::new();
        let collected = collect_widgets(&placed, &mut mesh, &inputs, &Theme::default());
        let body = collected.clip_bodies.first().expect("the take is a body");
        let rect = body.rect;
        assert!(
            body.clip
                .is_some_and(|c| c.x >= rect.x && c.x + c.w <= rect.x + rect.w),
            "the clip hands its body a mask of its own box"
        );

        // The bodies alone: the lane under them spans the whole window, and it
        // is the take's overshoot this is about.
        let mut bodies = collected;
        bodies.track_items.clear();
        bodies.clip_items.clear();
        bodies.ruler_items.clear();
        bodies.timeline_items.clear();
        let (mut base, mut over) = (Mesh::new(), Mesh::new());
        draw_static_meshes(
            &mut base,
            &mut over,
            &bodies,
            &inputs,
            &Theme::default(),
            &tree,
        );
        assert!(!base.is_empty(), "the take draws its polyline and its dots");
        let out = base
            .positions()
            .filter(|(x, _)| *x < rect.x - 0.5 || *x > rect.x + rect.w + 0.5)
            .count();
        assert_eq!(out, 0, "{out} vertices are drawn outside the clip");
    }

    /// **The grip belongs to the clip on top.** Clips may overlap, and the
    /// overlay is painted after every clip's box — so a covered clip lighting
    /// up its edge would draw its affordance over the clip covering it, and
    /// announce a grab the press (which takes the topmost) would not give.
    #[test]
    fn only_the_topmost_clip_under_the_pointer_carries_a_grip() {
        use crate::host::guidef::GuiNode;
        use crate::host::widget::Widget;

        // Two clips on one lane, the second overlapping the first by a sliver
        // and on top (later placements are drawn later). The sliver is the
        // point: a grip is lit only over its own strip, so the one place two
        // clips can both claim the pointer is where one's end strip and the
        // other's start strip land on the same pixels.
        let json = r#"{"type":"window","margin":0,"children":[
            {"id":5,"type":"field","label":"lane","children":[
                {"id":10,"type":"field","offset":0,"dur":600,"data":[0.0,0.5]},
                {"id":11,"type":"field","offset":580,"dur":600,"data":[0.0,0.5]}]}]}"#;
        let tree = Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap();
        let m = Metrics::default();
        let area = Rect::new(0.0, 0.0, 800.0, 200.0);

        let grips = |cursor: (f64, f64)| {
            let world = crate::host::world::World {
                cursor: Some(cursor),
                ..crate::host::world::World::default()
            };
            let inputs = FrameInputs {
                metrics: &m,
                world,
                ..FrameInputs::default()
            };
            let placed = layout::layout(area, &tree, &m);
            let mut mesh = Mesh::new();
            let collected = collect_widgets(&placed, &mut mesh, &inputs, &Theme::default());
            let (mut base, mut over) = (Mesh::new(), Mesh::new());
            draw_static_meshes(
                &mut base,
                &mut over,
                &collected,
                &inputs,
                &Theme::default(),
                &tree,
            );
            (
                collected.clip_items[0].rect,
                collected.clip_items[1].rect,
                over,
            )
        };

        // A pointer in the sliver: it stands on the first clip's **end** strip
        // and on the second clip's **start** strip at once, so both would light
        // — over the same pixels, one arrow pointing each way. Only the top
        // one's may be drawn.
        let (a, b, _) = grips((0.0, 0.0));
        let sliver_x = b.x + m.grip_w * 0.5;
        assert!(
            sliver_x > a.x + a.w - m.grip_w && sliver_x < a.x + a.w,
            "the cursor stands on both strips: {sliver_x} in {a:?} / {b:?}"
        );
        let (_, _, over) = grips((sliver_x as f64, (a.y + a.h * 0.5) as f64));
        let ink: Vec<f32> = over.positions().map(|(x, _)| x).collect();
        assert!(!ink.is_empty(), "something is drawn over the clips");
        // The two strips sit on the same pixels, so the arrow is what tells
        // them apart: the top clip's points *into* its start (rightwards from
        // the strip's left third), the covered one's would point the other way.
        let arrow: Vec<f32> = ink
            .iter()
            .copied()
            .filter(|x| *x >= b.x && *x <= b.x + m.grip_w)
            .collect();
        assert!(!arrow.is_empty(), "a grip is drawn on the shared strip");
        let tip = arrow.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            tip < b.x + m.grip_w * 0.5,
            "the arrow is the top clip's start grip, not the covered clip's end"
        );
        // And the other half of the rule: a pointer over the clip's samples,
        // clear of both strips, lights **nothing** — there is no resize there,
        // and the press goes to whatever the clip's bodies make of it.
        let middle = ((a.x + a.w * 0.5) as f64, (a.y + a.h * 0.5) as f64);
        let (_, _, over) = grips(middle);
        assert_eq!(
            over.positions().count(),
            0,
            "a grip lit from the middle of a clip promises a resize the press does not give"
        );
    }

    /// **A held clip keeps its grip, and nobody else lights up.** A clip moves
    /// in snap steps and the pointer does not, so mid-drag the two part
    /// company: a grip that follows the pointer blinks out between steps and
    /// lights up whatever the pointer drifted over — including the clip the
    /// held one is being dragged across.
    #[test]
    fn a_held_clip_keeps_its_grip_wherever_the_pointer_got_to() {
        use crate::host::graphics::track::ClipSide;
        use crate::host::guidef::GuiNode;
        use crate::host::widget::Widget;

        let json = r#"{"type":"window","margin":0,"children":[
            {"id":5,"type":"field","label":"lane","children":[
                {"id":10,"type":"field","offset":0,"dur":600,"data":[0.0,0.5]},
                {"id":11,"type":"field","offset":400,"dur":600,"data":[0.0,0.5]}]}]}"#;
        let tree = Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap();
        let m = Metrics::default();
        let area = Rect::new(0.0, 0.0, 800.0, 200.0);

        let overlay = |grab: Grab, cursor: Option<(f64, f64)>| {
            let inputs = FrameInputs {
                metrics: &m,
                world: crate::host::world::World {
                    cursor,
                    ..crate::host::world::World::default()
                },
                grab,
                ..FrameInputs::default()
            };
            let placed = layout::layout(area, &tree, &m);
            let mut mesh = Mesh::new();
            let collected = collect_widgets(&placed, &mut mesh, &inputs, &Theme::default());
            let rects: Vec<Rect> = collected.clip_items.iter().map(|c| c.rect).collect();
            let (mut base, mut over) = (Mesh::new(), Mesh::new());
            draw_static_meshes(
                &mut base,
                &mut over,
                &collected,
                &inputs,
                &Theme::default(),
                &tree,
            );
            (rects, over)
        };

        let (rects, _) = overlay(Grab::None, None);
        let (a, b) = (rects[0], rects[1]);
        let ink_in = |over: &Mesh, r: Rect| {
            over.positions()
                .filter(|(x, _)| *x >= r.x && *x <= r.x + r.w)
                .count()
        };

        // The pointer has wandered off the held clip entirely — past the end of
        // the lane, where no clip is. The held clip still shows the edge it is
        // being resized by.
        let far = Some(((b.x + b.w + 50.0) as f64, (a.y + a.h * 0.5) as f64));
        let (_, over) = overlay(Grab::Clip(10, Some(ClipSide::End)), far);
        let start_of_a = Rect::new(a.x, a.y, m.grip_w, a.h);
        let end_of_a = Rect::new(a.x + a.w - m.grip_w, a.y, m.grip_w, a.h);
        assert!(ink_in(&over, end_of_a) > 0, "the held edge stays lit");
        assert_eq!(ink_in(&over, start_of_a), 0, "and only that edge");

        // ...and the pointer standing over *another* clip lights nothing there
        // while the first one is held.
        let over_b = Some(((b.x + 5.0) as f64, (b.y + b.h * 0.5) as f64));
        let (_, over) = overlay(Grab::Clip(10, None), over_b);
        let start_of_b = Rect::new(b.x, b.y, m.grip_w, b.h);
        assert_eq!(
            ink_in(&over, start_of_b),
            0,
            "a clip the drag is crossing offered a grab that is not on the table"
        );

        // A drag that is not a clip's silences every grip.
        let (_, over) = overlay(Grab::Other, over_b);
        assert_eq!(ink_in(&over, start_of_b), 0);
    }

    /// A clip's take drawn as the time-frequency texture: the same clip, the
    /// same placement, another presentation. It leaves the mesh bodies (it is
    /// not geometry) and is collected for the GPU pass under the **clip's** id,
    /// on the **clip's own axis** — which is what makes a spectral clip end
    /// where the clip ends instead of spanning the lane.
    #[test]
    fn a_clips_take_can_be_the_time_frequency_texture() {
        use crate::host::guidef::GuiNode;
        use crate::host::widget::Widget;

        let json = r#"{"type":"window","margin":0,"children":[
            {"id":5,"type":"field","label":"lane","children":[
                {"id":10,"type":"field","offset":0,"dur":400,"view":"spectrogram",
                 "colormap":1,"data":[0.0,1.0,-1.0,0.5]}]}]}"#;
        let tree = Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap();
        let m = Metrics::default();
        let inputs = FrameInputs {
            metrics: &m,
            ..FrameInputs::default()
        };
        let placed = layout::layout(Rect::new(0.0, 0.0, 800.0, 300.0), &tree, &m);
        let mut mesh = Mesh::new();
        let collected = collect_widgets(&placed, &mut mesh, &inputs, &Theme::default());

        assert!(collected.clip_bodies.is_empty(), "not a mesh body");
        assert_eq!(collected.spectral_bodies.len(), 1);
        let body = &collected.spectral_bodies[0];
        assert_eq!(body.id, 10, "the slot is keyed by the clip");
        assert_eq!(body.rect, collected.clip_items[0].rect);
        assert_eq!(body.colormap, 1, "the clip's display props reach the take");
        assert!(
            body.local.start.abs() < 0.5 && (body.local.len - 400.0).abs() < 1.0,
            "the clip's own axis, not the lane's window"
        );
    }

    /// The two paint props reach the frame through one door, and each in its
    /// own units: the opacity is already resolved (it composed down the tree at
    /// the mutation point), while the radius is a **logical** length that the
    /// placement's own table turns into pixels — so a widget seen at a HiDPI
    /// scale rounds by as much as it grew, and one that asked for neither draws
    /// exactly what it always drew.
    #[test]
    fn the_ink_of_a_placement_carries_the_opacity_and_scales_the_radius() {
        use crate::host::guidef::GuiNode;
        use crate::host::widget::{Widget, resolve_style};

        let json = r#"{"type":"window","margin":0,"opacity":0.5,"children":[
            {"id":7,"type":"button","label":"go","radius":6},
            {"id":8,"type":"button","label":"plain"}]}"#;
        let mut tree =
            Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap();
        resolve_style(&mut tree, &Arc::new(Theme::default()));
        for (scale, want_radius) in [(1.0, 6.0), (2.0, 12.0)] {
            let m = Metrics::default().resolved(scale);
            let placed = layout::layout(Rect::new(0.0, 0.0, 400.0, 200.0), &tree, &m);
            let ink = |id: i32| {
                ink_of(
                    placed
                        .iter()
                        .find(|p| p.widget.id == Some(id))
                        .expect("placed"),
                )
            };
            assert_eq!(ink(7).radius, want_radius);
            assert_eq!(ink(7).alpha, 0.5, "the window's fade reaches its buttons");
            assert_eq!(ink(8).radius, 0.0, "a widget that said nothing is square");
        }
    }

    #[test]
    fn timeline_body_reserves_the_ruler_strip_and_the_group_gutter() {
        let rect = Rect::new(10.0, 10.0, 400.0, 200.0);
        let m = Metrics::default();
        // The x ruler takes the bottom strip; the gutter is the group's, so a
        // view alone with its value ruler indents by that ruler's width.
        let body = timeline_body(
            rect,
            &editor(Ruler::Time, RulerY::Norm),
            false,
            m.ruler_w,
            &m,
        );
        assert_eq!(body.h, 200.0 - m.ruler_h);
        assert_eq!(body.x, 10.0 + m.ruler_w);
        assert_eq!(body.w, 400.0 - m.ruler_w);
        // Each is independently optional.
        let x_only = timeline_body(rect, &editor(Ruler::Time, RulerY::Off), false, 0.0, &m);
        assert_eq!((x_only.x, x_only.w), (10.0, 400.0));
        assert_eq!(x_only.h, 200.0 - m.ruler_h);
        let y_only = timeline_body(rect, &editor(Ruler::Off, RulerY::Hz), false, m.ruler_w, &m);
        assert_eq!(y_only.h, 200.0);
        assert_eq!(y_only.x, 10.0 + m.ruler_w);
        assert_eq!(
            timeline_body(rect, &editor(Ruler::Off, RulerY::Off), false, 0.0, &m),
            rect
        );
        // A **labelled** view gives up the same strip a labelled control does:
        // the picture starts below the caption instead of under it. It stacks
        // with the ruler, since the two take opposite ends of the rect.
        let strip = crate::host::widget::size::label_strip(true, m.text_scale, &m) + m.pad;
        assert!(strip > 0.0, "a labelled widget reserves a strip");
        let titled = timeline_body(rect, &editor(Ruler::Off, RulerY::Off), true, 0.0, &m);
        assert_eq!(titled.y, 10.0 + strip);
        assert_eq!(titled.h, 200.0 - strip);
        let titled_ruled = timeline_body(rect, &editor(Ruler::Time, RulerY::Off), true, 0.0, &m);
        assert_eq!(titled_ruled.y, 10.0 + strip);
        assert_eq!(titled_ruled.h, 200.0 - strip - m.ruler_h);
        // The caption's own gap is the reason the strip is not just the text's
        // height: the picture starts a pad below the line, exactly as a
        // control's body does, and never against it.
        assert_eq!(
            titled.y,
            10.0 + crate::host::graphics::controls::label_height(rect.h, true, m.text_scale, &m)
                + m.pad,
            "the same formula controls::body_rect uses vertically"
        );

        // Sharing an axis with a lane, the same view starts its trace where the
        // lane starts its clips — the indent is the axis', not the widget's.
        let shared = timeline_body(
            rect,
            &editor(Ruler::Off, RulerY::Norm),
            false,
            m.header_w,
            &m,
        );
        assert_eq!(shared.x, 10.0 + m.header_w);
    }

    /// **The note the draw pass leaves**: a zoom past the summary over a span
    /// nothing covers is a fetch owed, and every other case is silence.
    #[test]
    fn only_a_zoom_past_the_summary_over_uncovered_samples_asks_for_a_span() {
        use crate::waveform::WaveformData;
        // Long enough that the whole of it, over 800 px, is coarser than a
        // bucket — which is what "zoomed out" means for this question.
        let (bucket, frames) = (256usize, 256 * 4_000);
        let told = WaveformView::new(WaveformData::with_multi_pyramid(
            clausters_core::peaks::MultiPyramid::empty(frames, 1, bucket),
        ));
        let wide = View {
            start: 0.0,
            len: frames as f64,
        };
        assert_eq!(
            owed(&told, &wide, 800.0, None),
            None,
            "zoomed out, the summary is the answer"
        );
        let close = View {
            start: 1_000.0,
            len: 2_000.0,
        };
        assert_eq!(
            owed(&told, &close, 800.0, None),
            Some(Owed::Samples { a: 1_000, b: 3_000 }),
            "past the bucket over samples it cannot answer for"
        );

        // The same view once the run has arrived: nothing more is owed.
        let mut data = WaveformData::with_multi_pyramid(
            clausters_core::peaks::MultiPyramid::empty(frames, 1, bucket),
        );
        assert!(data.set_window(1_000, 1, &vec![0.0; 2_000]));
        let covered = WaveformView::new(data);
        assert_eq!(owed(&covered, &close, 800.0, None), None);

        // And a view that owns its samples never asks.
        let owned = WaveformView::new(WaveformData::from_interleaved(
            &vec![0.0; frames],
            1,
            bucket,
        ));
        assert_eq!(owed(&owned, &close, 800.0, None), None);
    }

    /// **A take being recorded is asked for what is behind its frontier.** The
    /// span is clamped to `written` and never crosses it: a page zoomed past
    /// its summary reads the samples that are final, and nothing over the zeros
    /// the buffer is still holding.
    #[test]
    fn a_recording_asks_for_the_span_behind_its_frontier_and_no_further() {
        use crate::waveform::WaveformData;
        let (bucket, frames) = (256usize, 256 * 4_000);
        let told = WaveformView::new(WaveformData::with_multi_pyramid(
            clausters_core::peaks::MultiPyramid::empty(frames, 1, bucket),
        ));
        let close = View {
            start: 1_000.0,
            len: 2_000.0,
        };
        assert_eq!(
            owed(&told, &close, 800.0, Some(10_000)),
            Some(Owed::Samples { a: 1_000, b: 3_000 }),
            "wholly behind the frontier: the same span a finished take asks for"
        );
        assert_eq!(
            owed(&told, &close, 800.0, Some(2_000)),
            Some(Owed::Samples { a: 1_000, b: 2_000 }),
            "straddling it: only the settled half"
        );
        assert_eq!(
            owed(&told, &close, 800.0, Some(500)),
            None,
            "wholly past it: there is nothing written to read"
        );
        assert_eq!(
            owed(&told, &close, 800.0, Some(0)),
            None,
            "a take nothing has been written into yet"
        );
        // Zoomed out it is still the summary's answer, recording or not.
        let wide = View {
            start: 0.0,
            len: frames as f64,
        };
        assert_eq!(owed(&told, &wide, 800.0, Some(10_000)), None);
    }

    /// **What is owed is a finer summary, until a summary stops being worth
    /// asking for.** The same view at three zooms answers three ways: the
    /// summary it has, a grid it can be sent in one reply, and the samples.
    #[test]
    fn a_zoom_past_the_summary_asks_for_a_finer_grid_and_not_for_the_samples() {
        use crate::waveform::WaveformData;
        let (bucket, frames) = (256usize, 256 * 4_000);
        let told = WaveformView::new(WaveformData::with_multi_pyramid(
            clausters_core::peaks::MultiPyramid::empty(frames, 1, bucket),
        ));
        // 800 px over 256 000 frames: 320 samples a pixel, coarser than the
        // bucket, and the summary is already the answer.
        let out = View {
            start: 0.0,
            len: 256_000.0,
        };
        assert_eq!(owed(&told, &out, 800.0, None), None);

        // 800 px over 51 200 frames: 64 samples a pixel. What draws that is one
        // min/max pair per column, which is 800 pairs -- and asking for the
        // samples would be 51 200 of them, a few hundred kilobytes through a
        // 64 KiB carrier, to compute a few kilobytes' worth.
        let mid = View {
            start: 0.0,
            len: 51_200.0,
        };
        assert_eq!(
            owed(&told, &mid, 800.0, None),
            Some(Owed::Summary {
                a: 0,
                b: 51_200,
                bucket: 32
            }),
            "two buckets a column, finer than the view's own"
        );

        // 800 px over 3 200 frames: four samples a pixel. A grid there would be
        // two samples a bucket -- three floats to describe two -- so the
        // samples are both cheaper and exact.
        let deep = View {
            start: 0.0,
            len: 12_800.0,
        };
        assert_eq!(
            owed(&told, &deep, 800.0, None),
            Some(Owed::Samples { a: 0, b: 12_800 }),
        );
    }

    /// The grid a span is asked at: a power of two, two of them to a column,
    /// coarsened until one reply holds the span, and `None` where the samples
    /// are the better answer.
    #[test]
    fn the_detail_grid_holds_two_buckets_a_column_and_fits_one_reply() {
        let base = 256;
        for (per_px, want) in [
            (256.0, Some(128)),
            (64.0, Some(32)),
            (63.0, Some(16)),
            (32.0, Some(16)),
            (31.9, None),
            (8.0, None),
            (0.5, None),
        ] {
            assert_eq!(
                detail_bucket(per_px, 8_192, base),
                want,
                "at {per_px} samples a pixel"
            );
        }
        // Never as coarse as the view's own summary: there would be nothing to
        // gain, and the report belongs in the pyramid itself.
        assert_eq!(detail_bucket(512.0, 8_192, base), None);
        assert_eq!(detail_bucket(1_000.0, 8_192, base), None);
        // A span too long for one reply is summarized coarser rather than in
        // two, because a grid is replaced and not extended.
        let long = DETAIL_REPLY_BUCKETS * 64;
        assert_eq!(detail_bucket(64.0, long, base), Some(64));
        assert_eq!(
            detail_bucket(64.0, long * 8, base),
            None,
            "not without passing the view's own"
        );
        assert_eq!(detail_bucket(64.0, 0, base), None);
    }

    #[test]
    fn placed_nav_shifts_the_body_window_by_the_offset() {
        let nav = View {
            start: 100.0,
            len: 400.0,
        };
        // The un-placed default is the identity.
        assert_eq!(placed_nav(&nav, 0.0), nav);
        // A member placed at timeline sample 100 draws its data sample 0 there:
        // the local window starts one clip-length earlier.
        let placed = placed_nav(&nav, 100.0);
        assert_eq!((placed.start, placed.len), (0.0, 400.0));
        // Placing further right pushes the local window negative (data before
        // the visible origin) without changing the span.
        let placed = placed_nav(&nav, 250.0);
        assert_eq!((placed.start, placed.len), (-150.0, 400.0));
    }

    #[test]
    fn lane_at_picks_the_lane_under_the_cursor() {
        let body = Rect::new(0.0, 0.0, 400.0, 300.0);
        assert_eq!(lane_at(body, 3, 50.0), 0);
        assert_eq!(lane_at(body, 3, 150.0), 1);
        assert_eq!(lane_at(body, 3, 299.0), 2);
        assert_eq!(lane_at(body, 3, 1000.0), 2, "clamped");
    }

    #[test]
    fn lanes_split_the_body_evenly_and_share_x() {
        let body = Rect::new(0.0, 0.0, 400.0, 300.0);
        let a = lane_rect(body, 3, 0);
        let b = lane_rect(body, 3, 1);
        let c = lane_rect(body, 3, 2);
        assert_eq!(a.h, 100.0);
        assert_eq!((a.x, a.w), (b.x, b.w));
        assert_eq!(b.y, 100.0);
        assert_eq!(c.y + c.h, 300.0);
    }

    #[test]
    fn deinterleave_splits_frames_and_drops_the_partial_tail() {
        let flat = [1.0, -1.0, 2.0, -2.0, 3.0];
        let chans = deinterleave(&flat, 2);
        assert_eq!(chans, vec![vec![1.0, 2.0], vec![-1.0, -2.0]]);
        assert_eq!(deinterleave(&flat, 1).len(), 1);
        assert_eq!(deinterleave(&flat, 1)[0].len(), 5);
    }

    #[test]
    fn stft_lanes_cap_the_hop_for_long_buffers() {
        // A buffer long enough that hop 8 would exceed MAX_FRAMES: the hop is
        // raised so every lane fits the texture.
        let n = 200_000;
        let chan: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin()).collect();
        let lanes = stft_lanes(vec![chan], 256, 8, 48_000.0);
        assert_eq!(lanes.len(), 1);
        assert!(lanes[0].n_frames() <= crate::spectrogram::MAX_FRAMES);
        assert_eq!(lanes[0].total_samples(), n);
    }
}
