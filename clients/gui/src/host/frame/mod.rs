//! Rendering one window's widget tree into its wgpu surface -- the shared frame
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
//! and draws the whole frame in one pass -- the editor chrome (rulers,
//! selection, playhead, cursor readout) as a second, *overlay* mesh drawn
//! after the heavy views so it reads on top of them, and a third over a
//! dialog's own heavy views and under nothing ([`Batches`]).
//!
//! **Module layout.** This file is the frame's spine: the GPU slots a heavy
//! view hangs on, the [`FrameInputs`] a front fills, and [`render`] itself --
//! lay out, collect, draw, upload, one pass. The two long halves it calls are
//! its children, one per direction of the frame. [`items`] is the *read* half:
//! the per-widget snapshots and the single tree walk that fills them, kept
//! together because a new item type is a struct plus an arm of that walk.
//! [`draw`] is the *write* half: the three mesh passes over those snapshots,
//! which by then hold no borrow of the host tree.

pub(crate) mod draw;
mod items;
mod underlay;

use crate::host::diag;
use clausters_core::spectra;
use draw::*;
pub(crate) use draw::{draw_time_ruler, marker_at, ruler_strip, ruler_strip_body};
// The arrow's width is the drawing's own business; a gesture asks `marker_at`
// what was hit. The test that pins the reach to it is the one exception.
use items::*;
use underlay::Underlay;

use std::collections::HashMap;
use std::sync::Arc;

use crate::gpu::Gpu;
use crate::spectrogram::{FreqScale, SpectrogramView, Stft};
use crate::view::{Framing, Renderers, TimelineView};
use crate::viewport::View;
use crate::waveform::{WaveformData, WaveformView};

use super::bands::Bands;
use super::chrome;
use super::layout::{self, Rect};
use super::metrics::Metrics;
use crate::canvas::{self, CanvasView};

use super::font;
use super::paint::{Draw, Ink, Mesh, Painter};
use super::ruler::{self, TimeUnit};
use super::status;
use super::theme::{Theme, with_alpha};
use super::timeline::{GroupState, group_key};
use super::widget::element::{Ctx, Loaded, SlotFill, SlotFrame};
use super::widget::{EditorProps, Ruler, RulerY, Widget, WidgetKind};
use super::world::World;
use crate::host::graphics::signal::layers::Paint;

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
/// -- a slot is per window, a group may span windows. The picture is drawn into
/// the window's mesh like every other widget's, so nothing here is GPU state.
pub(crate) struct WaveformSlot {
    pub(crate) view: WaveformView,
    /// **What this view was drawn over and could not answer** -- a zoom finer
    /// than its summary's bucket, over a span it holds neither samples nor a
    /// finer grid for. [`Owed`] says which of the two would settle it.
    ///
    /// It is set by the draw pass, which is the only place that knows the zoom
    /// *and* the span, and read (and cleared) by the leg after the frame,
    /// which is the only place that can ask the server for it. A `Cell`
    /// because the draw pass borrows the slots immutably -- the front is single
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
/// - [`Owed::Summary`] -- a **finer grid** over the span, at about a bucket a
///   column (`/buffer_peaks`, folded by [`WaveformData::set_detail`]). A few
///   kilobytes, one reply, and it is what a zoom above the polyline regime
///   actually wants: asking for the samples there moves a few hundred kilobytes
///   through a 64 KiB carrier to compute a row the server can measure in one
///   pass.
/// - [`Owed::Samples`] -- the **samples themselves** (`/buffer_getRange`, folded
///   by [`WaveformData::set_window`]), for the zoom below which no summary is
///   worth asking for: past `trace::LINE_THRESHOLD` a column is the line
///   between samples and, from where the dots appear, the trace is the samples
///   themselves -- and a bucket is not a sample.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Owed {
    /// The run of samples to read back, `[a, b)` in frames.
    Samples { a: usize, b: usize },
    /// **The run of samples a texture analyzes**, `[a, b)` in frames: read
    /// back as [`Owed::Samples`] is, and asked only of a server buffer,
    /// since a list answers its texture's columns instead.
    Run { a: usize, b: usize },
    /// The span to summarize, `[a, b)` in frames, and the bucket to measure it
    /// at -- finer than the view's own, and coarse enough that one grid holds
    /// the whole span. `seen` is the part of it on screen: a grid already on
    /// its way that covers it is let land rather than asked again.
    Summary {
        a: usize,
        b: usize,
        bucket: usize,
        seen: (usize, usize),
    },
    /// **A spectrogram's columns** over a span: `columns` analysis windows
    /// from frame `start`, one every `hop` -- what a texture of a span owes
    /// when the view moved past it or zoomed to another hop.
    Columns(crate::host::columns::ColumnSpan),
}

/// A `WaveformSlot` for ready data.
pub(crate) fn waveform_slot(data: impl Into<Arc<WaveformData>>) -> WaveformSlot {
    WaveformSlot {
        view: WaveformView::new(data),
        owed: std::cell::Cell::new(None),
    }
}

/// A spectrogram widget's GPU views -- one [`SpectrogramView`] (own STFT and
/// texture) per channel. Navigation lives in the timeline group.
pub(crate) struct SpectrogramSlot {
    pub(crate) views: Vec<SpectrogramView>,
    /// **The columns this view was drawn over and does not hold**, when it
    /// draws a transform of a span (a list's, analyzed only where it is
    /// looked at): set by the draw pass, which knows the view and its width,
    /// and taken by the leg, which asks for them. The waveform's
    /// [`WaveformSlot::owed`], for the texture.
    pub(crate) owed: std::cell::Cell<Option<Owed>>,
    /// **Its columns are of a list that was since redefined**, and the new
    /// one's length: the view keeps drawing them, and owes the columns of the
    /// new one over the take as long as it is now, since the columns it holds
    /// measure the take it was.
    pub(crate) stale: std::cell::Cell<Option<usize>>,
    /// **It was kept across a redefine of its window** ([`carry_spectrograms`])
    /// and the fetch the new tree makes has not landed yet: that fetch is of
    /// the take these columns measure, not of an edit, and finds them current.
    pub(crate) carried: std::cell::Cell<bool>,
}

impl SpectrogramSlot {
    /// **Whether a take of `frames` frames placed over this slot keeps its
    /// columns**, drawn on while the ones it owes come: a transform of a
    /// span does, and owes the take again at its new length (`stale`)
    /// unless the placement is the fetch of a redefine it was kept across,
    /// over a take as long as the one it measures.
    pub(crate) fn keeps_columns(&self, frames: usize) -> bool {
        if !self.views.first().is_some_and(|v| v.stft().is_span()) {
            return false;
        }
        if !(self.carried.replace(false) && self.total_samples() == frames) {
            self.stale.set(Some(frames));
        }
        true
    }

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
    Some(SpectrogramSlot {
        views,
        owed: std::cell::Cell::new(None),
        stale: std::cell::Cell::new(None),
        carried: std::cell::Cell::new(false),
    })
}

/// **What a slot-backed element keeps of the resource that filled its slot.**
///
/// A pyramid is not only a picture: it is the samples the element named, and
/// [`Samples::sample_block`](crate::host::widget::element::Samples::sample_block)
/// reads a copy back out of it. Routing it to the slot alone left the element
/// holding nothing, so a copy over a mapped take -- the very source the clipboard
/// was written for -- refused as if the host could not read it. The two share one
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

/// **A picture of a span keeps the picture of the whole take** the slot it
/// replaces had: the overview under it, or the held picture itself when that
/// was the whole take. One of the whole take replaces both, and one of a take
/// of another length keeps nothing of it, since what it held is not where it
/// was any more.
///
/// **The pictures of the whole take by other analyses are kept too**: the
/// ones the slot it replaces kept, and the one it drew when the new picture
/// is of the whole take by another analysis ([`SpectrogramView::shelve`]).
/// None of them is kept across an edit (a slot whose columns are `stale`), or
/// into a take of another length.
fn keep_overviews(slot: &mut SpectrogramSlot, mut held: SpectrogramSlot) {
    let edited = held.stale.get().is_some();
    for (view, mut was) in slot.views.iter_mut().zip(held.views.drain(..)) {
        if !view.stft().is_span() || !was.stft().is_span() {
            continue;
        }
        let total = view.stft().total_samples();
        let shelved = was.take_shelved();
        let overview = if was.is_whole() {
            Some(was)
        } else {
            was.take_overview()
        };
        let overview = overview.filter(|o| o.stft().total_samples() == total);
        if !edited {
            for kept in shelved.into_iter().rev() {
                if kept.stft().total_samples() == total {
                    view.shelve(kept);
                }
            }
        }
        if view.is_whole() {
            // the one dark column a take is given before its columns is no
            // picture to go back to
            if let Some(o) = overview.filter(|o| !edited && o.stft().n_frames() > 1) {
                let analysis = (
                    o.stft().window_size(),
                    o.stft().analysis().0,
                    o.stft().analysis().1,
                );
                if !view.analyzed_as(analysis) {
                    view.shelve(o);
                }
            }
            continue;
        }
        view.set_overview(overview);
    }
}

/// **The server buffer each view of a window reads**, by widget, out of the
/// references its bulk walk collected: what a redefine compares against to
/// know which pictures still stand ([`carry_slots`]).
pub(crate) fn reads_of(refs: &[(i32, i32, bool)]) -> HashMap<i32, i32> {
    refs.iter()
        .map(|&(widget, buffer, _)| (widget, buffer))
        .collect()
}

/// **What a redefined window keeps of its pictures**: every slot of a view
/// that reads the buffer it read before (`before`, by widget) and still does
/// (`now`, the new tree's references), moved from `old` into `into` where the
/// new tree's own load put nothing.
///
/// A redefine rebuilds the tree, and the slots went with it: a panel shown
/// beside a spectrogram emptied the view and asked for its picture again,
/// which came back a frame or several later -- a blink over something that
/// had not changed. The fetch is still made, since the new element has to be
/// told the take; what it lands in is the picture already on screen.
pub(crate) fn carry_slots<V>(
    before: &HashMap<i32, i32>,
    now: &[(i32, i32, bool)],
    old: HashMap<SlotAt, V>,
    into: &mut HashMap<SlotAt, V>,
) -> Vec<SlotAt> {
    let mut carried = Vec::new();
    for (at, slot) in old {
        let reads = now
            .iter()
            .any(|&(widget, buffer, _)| widget == at.0 && before.get(&widget) == Some(&buffer));
        if reads && !into.contains_key(&at) {
            into.insert(at, slot);
            carried.push(at);
        }
    }
    carried
}

/// [`carry_slots`] for the textures, each kept one marked so the fetch the
/// new tree makes leaves its columns current ([`SpectrogramSlot::carried`]).
pub(crate) fn carry_spectrograms(
    before: &HashMap<i32, i32>,
    now: &[(i32, i32, bool)],
    old: HashMap<SlotAt, SpectrogramSlot>,
    into: &mut HashMap<SlotAt, SpectrogramSlot>,
) {
    for at in carry_slots(before, now, old, into) {
        if let Some(slot) = into.get(&at) {
            slot.carried.set(true);
        }
    }
}

/// **How long the take of each view's own picture is**, by widget: what a
/// navigation group spans, set for every view a window's slots draw.
pub(crate) fn take_totals(
    waveforms: &HashMap<SlotAt, WaveformSlot>,
    spectrograms: &HashMap<SlotAt, SpectrogramSlot>,
) -> Vec<(i32, usize)> {
    let own = |at: &SlotAt| at.1 == super::widget::element::SlotKey::SELF;
    waveforms
        .iter()
        .filter(|(at, _)| own(at))
        .map(|(at, slot)| (at.0, slot.view.total_samples()))
        .chain(
            spectrograms
                .iter()
                .filter(|(at, _)| own(at))
                .map(|(at, slot)| (at.0, slot.total_samples())),
        )
        .collect()
}

/// **Puts a resolved bulk resource into the slot its element claimed**, keyed
/// by the id that addresses that element (a clip's body is its container's).
/// Returns the loaded extent in samples, for the navigation group that has to
/// know how long its longest member is.
///
/// The routing is the *form* the loader brought back and nothing else -- a
/// pyramid fills a geometry slot, analyses fill a texture slot -- which is what
/// lets one function serve a mapped file, a page's `fetch` and a server
/// buffer's reply alike. The forms an element takes home never reach here: the
/// loader forked on `Needs::slot` before calling.
pub(crate) fn place_in_slot(
    data: Loaded,
    id: SlotAt,
    gpu: &Gpu,
    renderers: &Renderers,
    waveforms: &mut HashMap<SlotAt, WaveformSlot>,
    spectrograms: &mut HashMap<SlotAt, SpectrogramSlot>,
) -> Option<usize> {
    match data {
        // A pyramid fills the geometry slot whether it summarizes a copy or a
        // mapping -- the slot draws a picture and does not care which.
        Loaded::Peaks(data) | Loaded::Shared(data) => {
            let slot = waveform_slot(data);
            let total = slot.view.total_samples();
            waveforms.insert(id, slot);
            Some(total)
        }
        Loaded::Stfts(stfts) => {
            let held = spectrograms.remove(&id);
            let Some(mut slot) = spectrogram_slot(stfts, gpu, renderers) else {
                if let Some(held) = held {
                    spectrograms.insert(id, held);
                }
                return None;
            };
            if let Some(held) = held {
                keep_overviews(&mut slot, held);
            }
            let total = slot.total_samples();
            spectrograms.insert(id, slot);
            Some(total)
        }
        Loaded::Samples(_) | Loaded::Raw { .. } => {
            diag::warn!("widget {}: raw samples cannot fill a GPU slot", id.0);
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
/// follows the *hop* -- where rebuilding the transform each tick made it follow
/// the *span*, and a minute of retention cost eight times an eight-second one
/// to show the same two new columns.
///
/// Both fronts call it, which is what keeps a browser waterfall and a desktop
/// one the same picture built the same way.
fn roll_into_slot(
    slots: &mut HashMap<SlotAt, SpectrogramSlot>,
    id: SlotAt,
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
        slots.insert(
            id,
            SpectrogramSlot {
                views: vec![view],
                owed: std::cell::Cell::new(None),
                stale: std::cell::Cell::new(None),
                carried: std::cell::Cell::new(false),
            },
        );
    }
    let view = slots.get_mut(&id)?.views.first_mut()?;
    view.set_retention(&gpu.device, &gpu.queue, &renderers.spectrogram, capacity);
    view.push_columns(&gpu.queue, columns);
    (view.stft().n_frames() > 0).then(|| view.total_samples())
}

/// One STFT per channel for a spectrogram's channel stack: de-interleaved `channels`,
/// analyzed at `window_size`/`hop` (the hop raised by `stft::hop_capped` only for
/// a buffer whose transform would take more memory than one keeps) and
/// `sample_rate` (48 kHz when unknown,
/// so the frequency axis is still drawable). Shared by both fronts and every
/// data source (mapped path, fetched buffer, inline samples).
pub(crate) fn stft_channels(
    channels: Vec<Vec<f32>>,
    (window_size, hop, wintype): (usize, usize, i32),
    sample_rate: f64,
) -> Vec<Stft> {
    let sr = if sample_rate > 0.0 {
        sample_rate as f32
    } else {
        48_000.0
    };
    channels
        .into_iter()
        // The one analysis every end makes, so a cache a client wrote and
        // a take the host mapped are the same transform. A window the FFT
        // has no size for was refused when the props were read.
        .filter_map(|ch| Stft::analyze_with(&ch, window_size, hop, sr, wintype))
        .collect()
}

/// De-interleaves `channels` channels out of a flat buffer (a trailing partial
/// frame is ignored) -- the front half of [`stft_channels`] for inline sources.
pub(crate) fn deinterleave(samples: &[f32], channels: usize) -> Vec<Vec<f32>> {
    let channels = channels.max(1);
    let frames = samples.len() / channels;
    (0..channels)
        .map(|ch| (0..frames).map(|f| samples[f * channels + ch]).collect())
        .collect()
}

/// How long a filled slot's picture turned out to be, in samples -- what the
/// widget's navigation axis has to know, or the visible window falls back to a
/// span the size of the body and the whole picture draws stretched.
///
/// The two are set through different doors: a stored extent is fixed and joins
/// the navigation group as it is, while a **rolling** one slides -- the axis
/// follows the newest column until someone navigates it and then holds where
/// they left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Extent {
    Stored(usize),
    Rolling(usize),
}

/// **The window's GPU slots are gone** -- a fresh device, a re-attached canvas
/// -- so every widget that had filled one hands its content over again on the
/// next [`fill_slots`]. The device is the front's, and this is how the tree is
/// told that what it gave away did not survive it.
pub(crate) fn slots_dropped(widget: &mut Widget) {
    widget.kind.slot_dropped();
    for child in &mut widget.children {
        slots_dropped(child);
    }
}
/// **Which GPU slot**: the widget that holds the picture, and which of its own
/// pictures it is ([`SlotKey`](super::widget::element::SlotKey)).
///
/// A pair rather than an id because a view may hold several: a multitrack's
/// boxes are windows onto several takes, and a time-frequency box samples a
/// texture of its own.
pub(crate) type SlotAt = (i32, super::widget::element::SlotKey);

/// **Uploads whatever the tree has for its GPU slots**, keyed by the id that
/// addresses each widget: its own, or -- for a clip's body, which carries none --
/// its container's. Returns the extents the fills produced, for the caller to
/// register with the navigation groups once the tree borrow is over.
///
/// This is the filling half of the slot seam, and the whole of it: an element
/// hands over a pyramid, a set of analyses or the columns its rolling transform
/// just produced ([`WidgetKind::fills`]), and this walk uploads them. It asks
/// every widget the same question and learns nothing about any of them -- where
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
    waveforms: &mut HashMap<SlotAt, WaveformSlot>,
    spectrograms: &mut HashMap<SlotAt, SpectrogramSlot>,
    out: &mut Vec<(i32, Extent)>,
) {
    let owner = widget.id.or(owner);
    for (key, fill) in owner.into_iter().flat_map(|id| {
        widget
            .kind
            .fills()
            .into_iter()
            .map(move |(key, f)| ((id, key), f))
    }) {
        let id = key;
        let extent = match fill {
            // A fill over a slot that is already there **keeps the view**: the
            // picture is the element's and the navigation is the eye's, so a
            // refill -- which is what a destructive edit produces -- must not
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
        out.extend(extent.map(|e| (id.0, e)));
    }
    for child in &mut widget.children {
        fill_slots(child, owner, gpu, renderers, waveforms, spectrograms, out);
    }
}

/// The body a timeline view draws into: its rect minus the time-ruler strip
/// under it (when the x ruler is on) and the gutter band to its left -- each
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

/// What a drag is holding while this frame is drawn -- the frame's answer to
/// *whose* affordances may light up.
///
/// A grip is a promise about the next press, so during a drag it belongs to the
/// clip already held and to nothing else: another clip lighting up would offer
/// a grab that is not on the table, and the held clip's own grip must not blink
/// out every time the pointer wanders between two snap steps (the clip moves in
/// steps; the pointer does not).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) enum Grab {
    /// Nothing is held: affordances follow the pointer.
    #[default]
    None,
    /// Something else is held (a control, a curve, the axis): no clip lights up.
    Other,
    /// A **marquee** is being swept over the element `id`, and this is the
    /// rectangle: the frame draws it, through the routine every swept selection
    /// in the window is drawn with. It is here rather than in the element
    /// because the drag is the machine's -- one marquee, wherever a hand sweeps
    /// one.
    Marquee(i32, Rect),
}

/// The live inputs the frame needs beyond the tree and the GPU resources. The
/// native front fills them from its state; the browser front passes the
/// streamed equivalents.
///
/// Two kinds of thing, deliberately separated. [`world`](Self::world) is what
/// nobody owns -- the outside, identical for every element of the frame. The
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
    /// **This window's status bar**: what it has said and whether it is open
    /// (see [`super::status`]). `None` for a window that has said nothing yet
    /// -- the band is still carved, because a window that carries a bar carries
    /// it before it has anything to put in it.
    pub(crate) status: Option<&'a status::Status>,
    /// **What is open over this window**: the lists and the tip of the popup
    /// layer (see [`super::popup`]). The host's, like the status bar, and
    /// drawn last so it covers whatever it opened over.
    pub(crate) popups: Option<&'a super::popup::Popups>,
    /// **The host's clipboard**, which a tool's paste is offered against
    /// ([`super::actions`]). `None` draws every tool as its owner's reading
    /// alone would.
    pub(crate) clipboard: Option<&'a super::clipboard::Clip>,
    /// Whether the view this window's actions address holds a selection
    /// ([`super::Host::window_selected`]), for the tools that need one.
    pub(crate) selected: Option<bool>,
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
            status: None,
            popups: None,
            clipboard: None,
            selected: None,
        }
    }
}

/// The shared state a placed timeline widget draws with -- the window, the
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
        Some(state) => state,
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

/// **What the view could not draw, and in what shape** -- or `None` when it
/// answered for everything it showed.
///
/// A column finer than the summary's base bucket can only come from something
/// finer than the summary: a view holding neither draws the bucket, which is
/// the honest picture and one the eye has stopped getting anything new from.
/// So something is owed only when the view is zoomed past its summary over a
/// span nothing it holds answers for: not zoomed out (the summary *is* the
/// answer), and not answered already (mapped samples, a whole owned buffer, a
/// window over this span, a finer grid over this span).
///
/// **Which shape is owed is a question about the zoom alone**, and it is
/// [`Owed`]'s whole subject: zoomed out the picture is min/max columns, which a
/// finer *summary* answers in one reply of a few kilobytes; zoomed in far
/// enough the summary stops being cheaper than the samples it describes -- and
/// past that the trace is the polyline through the samples, where only the
/// samples will do. [`detail_bucket`] draws the line and picks the grid: a
/// column holds two buckets or more, so the position error of the fold stays
/// under half a column, and the bucket is coarsened until the whole span fits
/// one grid, because a second grid would replace the first rather than extend
/// it.
///
/// **The span asked reaches half a view past either edge** ([`OWED_REACH`]),
/// and it is asked again once the view comes within a quarter of a view of
/// the edge of what is held ([`OWED_NEAR`]). A pan reveals what is beside
/// the view, and what is beside it was being drawn from the coarsest summary
/// there is until a reply came -- over a long take a block of seconds,
/// drawn where the music is, at every step of a pan. Held a little past the
/// view, a pan at a hand's pace stays inside what is held while the next
/// span comes, which is the same margin the columns of a spectrogram keep.
///
/// **A take still being written is asked for what is behind the frontier, and
/// never for what is past it** (`written`, the `fills` prop's frontier). Past
/// it there is nothing to read: the buffer holds the zeros it was allocated
/// with, and a run or a bucket over them would claim measured silence over
/// audio that has not arrived. Behind it the samples are **final** -- a recorder
/// writes forward and does not come back, and the frontier is what the writer
/// says it has already written -- so a span that ends there is as readable as
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
    // The view and `reach` frames past either edge, clamped to the samples.
    // The frontier is the ceiling, and it is the only thing `fills` does here.
    // No margin under it: the frames the report counts were written before it
    // was measured, so the last of them is as settled as the first. The margin
    // that would be needed is *above* -- which is not a margin but the clamp
    // itself, since what the report has not counted yet may not be there.
    let span = |reach: f64| {
        let a = ((nav.start - reach).floor().max(0.0) as usize).min(total);
        let b = ((nav.start + nav.len + reach).ceil().max(0.0) as usize).clamp(a, total);
        (a, written.map_or(b, |w| b.min(w as usize)))
    };
    let seen = span(0.0);
    if seen.1 <= seen.0 {
        return None;
    }
    let near = span(nav.len * OWED_NEAR);
    if data.covers(near.0, near.1) {
        return None;
    }
    let (a, b) = span(nav.len * OWED_REACH);
    match detail_bucket(per_px, b - a, base) {
        Some(bucket) if !data.detail_covers(near.0, near.1, per_px) => {
            Some(Owed::Summary { a, b, bucket, seen })
        }
        Some(_) => None,
        None => Some(Owed::Samples { a, b }),
    }
}

/// The most frames a texture over a summary asks back at once: about
/// forty seconds at 48 kHz. Wider, the run around the view's middle is
/// analyzed, and the rest of the view waits for a zoom.
pub(crate) const TEXTURE_SPAN: usize = 1 << 21;

/// **The samples a texture over a summary owes the view `nav`**, or `None`
/// when what it holds covers it.
///
/// A summary is peaks, and peaks are no spectrum: a view that cannot map
/// its samples (a page, a remote server) drew its trace from the summary at
/// any zoom and its spectrogram from nothing. So a view that draws a
/// texture asks the run around what it shows whatever the zoom, as
/// [`owed`] asks it only past the summary's bucket, and analyzes that run
/// where it lies. A list asks its columns instead ([`columns_owed`]).
pub(crate) fn texture_owed(view: &WaveformView, nav: &View, written: Option<u64>) -> Option<Owed> {
    let data = view.data();
    let total = data.total_samples();
    if nav.len <= 0.0 || total == 0 {
        return None;
    }
    // the view and `reach` frames past either edge, clamped to what is
    // written, and no wider than [`TEXTURE_SPAN`] about the view's middle
    let middle = (nav.start + nav.len * 0.5).max(0.0) as usize;
    let span = |reach: f64| {
        let a = ((nav.start - reach).floor().max(0.0) as usize).min(total);
        let b = ((nav.start + nav.len + reach).ceil().max(0.0) as usize).clamp(a, total);
        let (a, b) = if b - a > TEXTURE_SPAN {
            let a = middle
                .saturating_sub(TEXTURE_SPAN / 2)
                .min(total.saturating_sub(TEXTURE_SPAN));
            (a, (a + TEXTURE_SPAN).min(total))
        } else {
            (a, b)
        };
        (a, written.map_or(b, |w| b.min(w as usize).max(a)))
    };
    let near = span(nav.len * OWED_NEAR);
    if near.1 <= near.0 || data.covers(near.0, near.1) {
        return None;
    }
    let (a, b) = span(nav.len * OWED_REACH);
    Some(Owed::Run { a, b })
}

/// **The columns a texture of a span owes the view `nav`**, `width` pixels
/// wide, or `None` when what it holds draws it: a transform of the whole
/// signal never owes any.
///
/// A picture of a list is two: **the whole take**, about a column to the
/// pixel when the view shows all of it, which is drawn wherever nothing finer
/// is held, and **the span shown**, asked when the view is narrower than the
/// whole take's columns resolve. A whole take analyzed as the file's
/// spectral summary is owed first, and again whenever the list changes, at
/// the length the take has now, and whenever the view is told to analyze
/// that way again (`analysis`: the window, the hop between windows and their
/// shape): it is asked at least a column of the summary apart
/// (`clausters_core::spectra`), so it is read from the summary with no
/// sample, and a view finer than that asks for its span. A whole take of any
/// other analysis is asked ahead of the view only after an edit: it reads
/// and analyzes every frame of the take, so after a change of analysis the
/// span shown comes first and the whole take is asked only by a view that
/// shows it. The span asked
/// reaches half a view past either edge ([`OWED_REACH`]), once the view
/// itself is held (a span comes whole, so what is on screen is asked alone
/// first when nothing held draws it), and is asked again
/// once the view comes within a quarter of a view of the edge of what is
/// held ([`OWED_NEAR`]), as a waveform's detail is: a pan or a zoom out at a
/// hand's pace stays inside what is held while the next span comes, instead
/// of drawing the edges it reveals from the whole take's columns. Both hops
/// are a power of two, so a small zoom and a pan ask for the columns the
/// answerer already holds, and **neither is finer than the hop between the
/// analysis's windows**: a column narrower than that would be one more window
/// a few frames from the last, which measures nothing a window on the
/// analysis's grid did not, and a view zoomed past it draws each window as a
/// block as wide as its hop.
pub(crate) fn columns_owed(
    slot: &SpectrogramSlot,
    nav: &View,
    width: f64,
    analysis: (usize, usize, i32),
) -> Option<Owed> {
    let view = slot.views.first()?;
    // a picture of this analysis kept aside is brought back as the frame is
    // uploaded ([`SpectrogramView::recall`]), and nothing is asked for it
    if slot.stale.get().is_none() && view.can_recall(analysis) {
        return None;
    }
    let overview = if view.is_whole() {
        Some(view)
    } else {
        view.overview()
    };
    columns_owed_of(
        view.stft(),
        overview.map(|o| o.stft()),
        slot.stale.get(),
        nav,
        width,
        analysis,
    )
}

/// [`columns_owed`] of the transform a slot draws, the picture of the whole
/// take it has, the length of the take when its list changed since, and the
/// analysis the view asks for.
pub(crate) fn columns_owed_of(
    held: &Stft,
    overview: Option<&Stft>,
    stale: Option<usize>,
    nav: &View,
    width: f64,
    (window, hop, wintype): (usize, usize, i32),
) -> Option<Owed> {
    if !held.is_span() || width <= 0.0 || nav.len <= 0.0 {
        return None;
    }
    let total = stale.unwrap_or(held.total_samples());
    if total == 0 {
        return None;
    }
    // the analysis's own hop: no column is finer
    let finest = if hop == 0 { window / 2 } else { hop }.max(1);
    let span = |start: usize, end: usize, hop: usize| {
        let hop = hop.max(finest);
        // on the grid of its own hop, so a column's windows are the ones on
        // the analysis's grid the answerer finds, not the nearest to it
        let start = start / hop * hop;
        Owed::Columns(crate::host::columns::ColumnSpan {
            start,
            hop,
            columns: end.saturating_sub(start).div_ceil(hop).max(1) + 1,
            window,
            analysis: 0,
            wintype: 0,
            sample_rate: held.sample_rate(),
            total,
        })
    };
    // a picture of another analysis than the one asked for is no picture
    let analyzed = |s: &Stft| s.window_size() == window && s.analysis() == (hop.max(1), wintype);
    let overview = overview.filter(|o| {
        stale.is_none() && o.total_samples() == total && o.n_frames() > 1 && analyzed(o)
    });
    // With no picture of the whole take of this analysis, the whole take is
    // asked first where the file's spectral summary answers it, in a few
    // milliseconds, and after an edit, whose windows the answerer mostly
    // holds already and whose old picture would be drawn at the old length.
    // Another analysis reads and analyzes the whole take for it, a second or
    // more of a long one, so what is on screen comes first and the rest only
    // when a view shows it.
    let summarized = window == spectra::WINDOW && (hop == spectra::HOP || hop == 0) && wintype == 0;
    if overview.is_none() && (summarized || stale.is_some()) {
        let hop = power_hop(total as f64 / width);
        return Some(span(0, total, hop.max(spectra::COLUMN_FRAMES)));
    }
    let want = power_hop(nav.len / width).max(finest);
    // the view and `reach` frames past either edge, clamped to the take
    let span_of = |reach: f64| {
        let a = ((nav.start - reach).floor().max(0.0) as usize).min(total);
        let b = ((nav.start + nav.len + reach).ceil().max(0.0) as usize).clamp(a, total);
        (a, b)
    };
    // a view at the analysis's own hop is drawn by its windows, one a block,
    // and no coarser picture stands in for them
    let coarsest = if want == finest { want } else { want * 2 };
    let covers = |stft: &Stft, (a, b): (usize, usize)| {
        let first = stft.origin();
        let last = first + stft.n_frames().saturating_sub(1) * stft.hop();
        stft.hop() <= coarsest
            && (stft.hop() * 2 >= want || stft.origin() == 0 && last + stft.hop() >= total)
            && first <= a
            && b.min(total.saturating_sub(1)) <= last
    };
    let near = span_of(nav.len * OWED_NEAR);
    if analyzed(held) && covers(held, near) || overview.is_some_and(|o| covers(o, near)) {
        return None;
    }
    // what is on screen first, when nothing held draws it: a span comes
    // whole, and one of the view alone comes in half the time of one that
    // reaches past both edges, which is asked next
    let shown = span_of(0.0);
    let (a, b) = if analyzed(held) && covers(held, shown) {
        span_of(nav.len * OWED_REACH)
    } else {
        shown
    };
    Some(span(a, b, want))
}

/// **The hop for `per_px` frames a pixel**: the largest power of two under
/// it, at least one frame. A view a little wider or narrower asks for the
/// same columns.
fn power_hop(per_px: f64) -> usize {
    let at_least = per_px.floor().max(1.0) as usize;
    1 << (usize::BITS - 1 - at_least.leading_zeros())
}

/// **How far past either edge of a view what it is owed reaches**, in views.
const OWED_REACH: f64 = 0.5;

/// **How close to the edge of what it holds a view may come** before the
/// next span is asked, in views: under [`OWED_REACH`], so the next span is on
/// its way while the one held still covers what is on screen.
const OWED_NEAR: f64 = 0.25;

/// **How many buckets one detail grid may hold**: what bounds the span one
/// grid summarizes. A grid comes over as many replies as it needs (a reply is
/// 8 KiB, `docs/schemas.md`) and is handed to the view whole, so this is a
/// bound on the grid's cost -- two dozen replies of stereo at most from a
/// server, one from the host's own answer over a list -- and not on one
/// reply. It holds a span two views wide ([`OWED_REACH`]) at two buckets a
/// column on a window a few thousand pixels wide.
const DETAIL_GRID_BUCKETS: usize = 8192;

/// **The grid to ask for a span of `span` frames at `per_px` samples a pixel**,
/// or `None` where no summary is worth asking for and the samples are the
/// answer.
///
/// Two rules meet here. The picture wants **two buckets a column or more**, so
/// the fold's position error stays under half a pixel -- one bucket a column
/// would put it at a whole one, which is the resolution the coarse summary
/// already has and the reason this is being asked for at all. And the span has
/// to fit **one grid**: a detail grid is replaced rather than extended (one
/// grid per view, like one window per view), so a span that would need more
/// than [`DETAIL_GRID_BUCKETS`] is summarized at a coarser bucket instead,
/// which is still finer than the view's own.
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
    // Coarsened until one grid holds the whole span.
    while bucket < base && span.div_ceil(bucket) > DETAIL_GRID_BUCKETS {
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
/// they answer every deeper zoom as well -- a grid answers only down to its own
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
/// space it was placed in -- the wire's number through the placement's own
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

/// The row channel `ch` of `channels` occupies inside `body` (stacked top to
/// bottom, no gap -- the divider line is overlay chrome).
///
/// The **third** row a view stacks, beside a roll's semitone and a
/// multitrack's lane, and the same structure: a band of the vertical axis. So
/// it is a [`Bands`] like the other two, on its uniform arm -- a channel stack
/// divides its body evenly because every channel is worth the same picture.
///
/// Same structure, **different word**: `lane` is the arrangement's, a track's
/// contents, and it is not this. The two were one name until the document
/// claimed the first.
pub(crate) fn channel_rect(body: Rect, channels: usize, ch: usize) -> Rect {
    let channels = channels.max(1);
    let (y, h) = Bands::uniform(channels, body.h / channels as f32).band(ch);
    Rect::new(body.x, body.y + y, body.w, h)
}

/// **What a scrolled stack of rows draws within**: the widget's clip, narrowed
/// to the body's height across the widget's width, so a row scrolled out of
/// the body is cut where the body ends while its y-ruler labels beside the
/// body still draw.
pub(crate) fn rows_clip(clip: Option<Rect>, rect: Rect, body: Rect) -> Rect {
    let band = Rect::new(rect.x, body.y, rect.w, body.h);
    clip.map_or(band, |c| c.intersect(band))
}

/// The time-ruler unit of `editor` (the beats grid rides its props).
pub(crate) fn time_unit(editor: &EditorProps) -> TimeUnit<'_> {
    match editor.ruler {
        Ruler::Samples => TimeUnit::Samples,
        Ruler::Beats => TimeUnit::Beats {
            tempo: editor.tempo,
            beat_at: editor.beat_at,
            quant: editor.quant,
            map: editor.tempo_map.as_deref(),
        },
        _ => TimeUnit::Seconds,
    }
}

/// The stacked-channel index under window y `cy` (clamped into range).
pub(crate) fn channel_at(body: Rect, channels: usize, cy: f64) -> usize {
    let rel = ((cy - body.y as f64) / body.h.max(1.0) as f64).clamp(0.0, 1.0);
    ((rel * channels as f64) as usize).min(channels.saturating_sub(1))
}

/// **A window's flat batches**, in the order the pass draws them, with the
/// heavy views' texture passes between them, and what the window keeps of its
/// last whole frame ([`Kept`]).
///
/// - `base`: the window's widgets. The window's own textures go over it.
/// - `over`: what the window's elements draw over their own textures (the
///   selection, the readout) and, with a dialog up, the scrim and the dialog's
///   flat widgets. The dialog's textures go over it.
/// - `live`: what moves -- the lines a clock sweeps, the bars a level fills,
///   a reading set many times a second ([`Element::draw_live`]). The one batch
///   a tick builds again.
/// - `top`: what the dialog's elements draw over their textures, then the
///   host's bands and lists, which cover everything.
///
/// Two batches were enough while nothing with a texture could stand over the
/// window: a spectrogram or a shader `canvas` inside a dialog was drawn with
/// the window's, and so under the dialog that held it.
///
/// [`Element::draw_live`]: super::widget::Element::draw_live
pub(crate) struct Batches {
    pub(crate) base: Painter,
    pub(crate) over: Painter,
    pub(crate) live: Painter,
    pub(crate) top: Painter,
    kept: Kept,
    /// What lies under the live layers, kept on the card ([`Underlay`]).
    underlay: Underlay,
    /// Every frame drawn in one pass, for a test to compare the kept
    /// picture's frames with.
    #[cfg(test)]
    one_pass: bool,
    asked: Asked,
    /// What the last frame cost, phase by phase ([`FrameCost`]).
    cost: FrameCost,
}

/// **What one frame cost, by phase** -- read by the front after each frame
/// and counted against the window (`/gui_activity`), so a profile says where
/// a slow window's frames go rather than only how long they took.
///
/// Timed with the same clock on both fronts (`web_time`), around work that is
/// the same code on both: the one thing that can differ is the platform under
/// `present`, which is why it is a phase of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameCost {
    /// Placing the tree: the layout pass of a whole frame.
    pub layout: std::time::Duration,
    /// Drawing the picture into meshes, past the layout -- or, for a frame of
    /// what moves, the live layers alone.
    pub build: std::time::Duration,
    /// Writing the meshes and the heavy views' textures and uniforms to the
    /// card.
    pub upload: std::time::Duration,
    /// Recording the render pass and submitting it.
    pub encode: std::time::Duration,
    /// Taking a drawable from the surface and presenting it.
    pub present: std::time::Duration,
    /// Bytes of vertices written to the card.
    pub uploaded: u64,
    /// Widgets laid out (a whole frame's placements; `0` for one of what
    /// moves).
    pub placed: u64,
}

/// What the next frame of a window was asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Wanted {
    /// Nobody of this host asked -- the platform did, or a caller that named
    /// no reason -- so the frame is a whole one.
    #[default]
    Unsaid,
    /// Only what moves: a tick found a counter or a fed value moved, or a set
    /// wrote a live prop.
    Live,
    /// The picture: something it is drawn from changed.
    Whole,
}

/// **What the next frame of a window was asked for, and by whom**: the one
/// place the question is kept, so that a front draws the least a frame may
/// be and never less.
///
/// A redraw has two kinds of cause. Something the picture is drawn from
/// changed -- a set, a gesture, a reply, a resize -- or only what moves did.
/// A front says which as it asks ([`want_whole`](Self::want_whole),
/// [`want_live`](Self::want_live)), and the frame reads the answer once.
/// **Whole wins, and so does silence**: a frame somebody asked the whole of
/// is whole whatever a tick asked beside it, and one nobody of this host
/// asked for -- the platform's, or a caller that named no reason -- is whole
/// too, since a picture kept past a change nobody declared is a wrong one.
#[derive(Default)]
struct Asked(std::cell::Cell<Wanted>);

impl Asked {
    fn want_whole(&self) {
        self.0.set(Wanted::Whole);
    }

    fn want_live(&self) {
        if self.0.get() == Wanted::Unsaid {
            self.0.set(Wanted::Live);
        }
    }

    fn whole(&self) -> bool {
        self.0.get() != Wanted::Live
    }

    fn drawn(&self) {
        self.0.set(Wanted::Unsaid);
    }
}

impl Batches {
    /// The batches of a surface drawing into `target`.
    pub(crate) fn new(device: &wgpu::Device, target: crate::view::Target) -> Self {
        // One glyph sheet for the window: the batches share the first's.
        let base = Painter::new(device, target);
        Self {
            over: Painter::sharing(device, target, &base),
            live: Painter::sharing(device, target, &base),
            top: Painter::sharing(device, target, &base),
            base,
            kept: Kept::default(),
            underlay: Underlay::new(device, target),
            #[cfg(test)]
            one_pass: false,
            asked: Asked::default(),
            cost: FrameCost::default(),
        }
    }

    /// What the last frame cost.
    pub(crate) fn cost(&self) -> FrameCost {
        self.cost
    }

    /// **The picture changed**: the next frame lays the window out and draws
    /// all of it. What every redraw is unless it was asked for
    /// [`want_live`](Self::want_live) alone.
    pub(crate) fn want_whole(&self) {
        self.asked.want_whole();
    }

    /// **Only what moves moved**: the next frame may keep the window's
    /// picture and draw its live layers again -- unless something else asked
    /// for the whole of it before that frame is drawn.
    pub(crate) fn want_live(&self) {
        self.asked.want_live();
    }

    /// Whether the frame about to be drawn has to be a whole one, read once
    /// by the front that draws it. A frame nobody of this host asked for is.
    pub(crate) fn whole_wanted(&self) -> bool {
        self.asked.whole()
    }
}

/// What a call to [`render`] came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drawn {
    /// The window laid out and drawn, all of it.
    Whole,
    /// The live layers drawn again over the picture the window kept.
    Live,
    /// Nothing: the live layers were where they are on screen.
    Still,
    /// Nothing, and the frame is still owed: the surface had no drawable.
    Owed,
}

/// **What a window keeps of its last whole frame**, so that a frame drawn
/// because a line moved builds that line and nothing else.
///
/// A window's picture is a function of its tree, of the host's state and of
/// the world; of the world, the clock and the fed values are what moves on
/// its own. Everything an element draws from those is on its live layer, so a
/// frame asked for by a tick alone finds the rest already in the batches --
/// the vertices on the card, the heavy views framed -- and has only to draw
/// the live layers again, in the placements kept here.
///
/// The card still draws the whole window: what is kept is the work of
/// describing it, which is where a frame's time went.
#[derive(Default)]
struct Kept {
    /// Whether the batches hold the picture of the tree as it stands.
    valid: bool,
    /// The framebuffer that picture was laid out for.
    size: (u32, u32),
    /// The color it was cleared to.
    clear: Option<wgpu::Color>,
    /// The glyph atlas' epoch it was drawn at: a sheet packed again since
    /// holds other glyphs where this picture's text points.
    epoch: u64,
    /// Where each element with a live layer was placed.
    places: Vec<LivePlace>,
    /// The heavy views of the window and, with a dialog up, of the dialog.
    collected: Option<Collected>,
    inside: Option<Collected>,
    /// The live layer as it was last drawn.
    live: Mesh,
    /// Where what is drawn over the live layers lies (the host's bands, the
    /// open menus), as triangle bounds joined: a live layer that meets it
    /// is drawn in the single pass ([`underlay::plan`]).
    over: Vec<[f32; 4]>,
}

impl Kept {
    /// Whether anything is drawn on the live layers: a placed element with
    /// one, or a timeline's playhead.
    fn has_live(&self) -> bool {
        !self.places.is_empty()
            || self
                .collected
                .as_ref()
                .is_some_and(|c| c.timeline_items.iter().any(|i| i.axis_owned))
    }
}

/// The glyph atlas' epoch (`font::atlas::epoch`): the same for ever in a
/// build with no rasterizer, where nothing is ever packed again.
fn atlas_epoch() -> u64 {
    #[cfg(feature = "font-atlas")]
    {
        font::atlas::epoch()
    }
    #[cfg(not(feature = "font-atlas"))]
    {
        0
    }
}

/// **How far a live layer has to move to be drawn again**, in device pixels.
/// A quarter: with four samples to the pixel that is the step a line's edge
/// can show, and without them a pixel is, so nothing under it is visible.
const LIVE_SLACK: f32 = 0.25;

/// One element's placement, kept without the tree it was laid out from: the
/// way down to its widget, and what a [`Ctx`] is made of.
struct LivePlace {
    /// The index of each child on the way from the root to the widget.
    path: Vec<usize>,
    rect: Rect,
    clip: Option<Rect>,
    scale: f32,
    metrics: Metrics,
    indent: f32,
    ink: Ink,
}

/// The placements of `placed[range]` that have a live layer, or `None` when
/// one of them cannot be found again from the root -- a frame that keeps
/// nothing, then, and is drawn whole the next time too.
fn live_places(
    placed: &[layout::Placed<'_>],
    range: std::ops::Range<usize>,
) -> Option<Vec<LivePlace>> {
    let mut out = Vec::new();
    for i in range {
        let p = &placed[i];
        let WidgetKind::Custom(el) = &p.widget.kind else {
            continue;
        };
        if !el.needs().live {
            continue;
        }
        let mut path = Vec::new();
        let mut at = p;
        while let Some(parent) = at.parent.and_then(|n| placed.get(n)) {
            let n = parent
                .widget
                .children
                .iter()
                .position(|child| std::ptr::eq(child, at.widget))?;
            path.push(n);
            at = parent;
        }
        path.reverse();
        out.push(LivePlace {
            path,
            rect: p.rect,
            clip: p.clip,
            scale: p.scale,
            metrics: p.metrics,
            indent: p.indent,
            ink: ink_of(p),
        });
    }
    Some(out)
}

/// **The live layers of the elements in `places`**, into `mesh`: each element
/// found again in `tree` and handed the placement it was laid out in, with
/// the world as it is now.
fn draw_live_places(
    mesh: &mut Mesh,
    tree: &Widget,
    places: &[LivePlace],
    inputs: &FrameInputs,
    theme: &Theme,
) {
    for place in places {
        let Some(widget) = place.path.iter().try_fold(tree, |w, &n| w.children.get(n)) else {
            continue;
        };
        let WidgetKind::Custom(el) = &widget.kind else {
            continue;
        };
        mesh.set_clip(place.clip);
        mesh.set_ink(place.ink);
        let th = widget.theme.as_deref().unwrap_or(theme);
        let m = &place.metrics;
        let ctx = Ctx {
            world: &inputs.world,
            metrics: m,
            rect: place.rect,
            indent: place.indent,
            scale: place.scale,
            clip: place.clip,
            time: widget.id.zip(widget.kind.editor()).and_then(|(id, e)| {
                inputs
                    .world
                    .timelines
                    .space_of(id, e.link, Some(inputs.world.clocks.at(Some(id))))
            }),
            focused: widget.id.is_some() && widget.id == inputs.focused,
            hovered: widget.id.is_some() && widget.id == inputs.popups.and_then(|o| o.hover),
            clock: inputs.world.clocks.at(widget.id),
        };
        el.draw_live(&mut Draw::new(mesh, m, th), &ctx);
        // **The focus ring stays over what moves.** It is drawn with the
        // picture, under this layer, so a live layer that reaches its edge --
        // a multitrack's meters at rest, a column the height of its track --
        // painted over the ring wherever they met. Drawn again here, it is the
        // last thing of the element, as it is in the picture.
        if ctx.focused {
            mesh.set_clip(place.clip);
            mesh.set_ink(Ink::default());
            mesh.border(place.rect, m.focus_ring, th.focus);
        }
    }
    mesh.set_clip(None);
    mesh.set_ink(Ink::default());
}

/// Renders `tree` into `gpu`'s surface, using the window's `batches` (flat
/// geometry under, between and over the heavy views), the `waveforms`/
/// `spectrograms`/`canvases` GPU resources, plus `inputs` for the live values.
///
/// **A whole frame** is one immutable mesh-building pass over the placed
/// widgets, then the GPU uploads and the single render pass. **A frame asked
/// for what moves alone** (`whole` false, and a picture kept to draw it over)
/// builds the live layers again and nothing else -- and when they are where
/// they were on screen, it draws nothing at all, so how often a window is
/// drawn follows how far its lines moved.
///
/// Answers what the frame came to ([`Drawn`]).
#[allow(clippy::too_many_arguments)] // the per-window resource set, both fronts
pub(crate) fn render(
    gpu: &mut Gpu,
    renderers: &mut Renderers,
    batches: &mut Batches,
    waveforms: &mut HashMap<SlotAt, WaveformSlot>,
    spectrograms: &mut HashMap<SlotAt, SpectrogramSlot>,
    canvases: &mut HashMap<i32, CanvasView>,
    tree: &Widget,
    inputs: &FrameInputs,
    theme: &Theme,
    whole: bool,
) -> Drawn {
    let (fb_w, fb_h) = (gpu.config.width.max(1), gpu.config.height.max(1));
    batches.cost = FrameCost::default();
    // **The host's own chrome draws in the window's theme**: the status bar,
    // the menu bar, the lists and the tip that open over the tree, the scrim
    // behind a dialog. They belong to no widget, so they used to take the
    // host's theme and ignore a window that set one of its own.
    let theme = tree.theme.as_deref().unwrap_or(theme);
    // **Only what moves**: the picture is in the batches, so the live layers
    // are drawn again where they were placed -- built first, since a glyph
    // one of them needs may be the one the atlas has to be packed again for,
    // and a picture from before that is not kept.
    let live = (!whole
        && batches.kept.valid
        && batches.kept.size == (fb_w, fb_h)
        && batches.kept.epoch == atlas_epoch())
    .then(|| {
        let started = web_time::Instant::now();
        let mut live = Mesh::new();
        if let Some(collected) = &batches.kept.collected {
            draw_timeline_heads(&mut live, collected, waveforms, spectrograms, inputs, theme);
        }
        draw_live_places(&mut live, tree, &batches.kept.places, inputs, theme);
        batches.cost.build = started.elapsed();
        live
    })
    .filter(|_| batches.kept.epoch == atlas_epoch());
    let drawn = if let Some(live) = live {
        // One that is where it was -- a line that crossed no fraction of a
        // pixel worth showing, a meter at rest -- is the frame already on
        // screen.
        if live.near(&batches.kept.live, LIVE_SLACK) {
            batches.asked.drawn();
            return Drawn::Still;
        }
        let started = web_time::Instant::now();
        let bytes = batches
            .live
            .upload(&gpu.device, &gpu.queue, &live, fb_w, fb_h);
        batches.cost.upload = started.elapsed();
        batches.cost.uploaded = bytes as u64;
        batches.kept.live = live;
        Drawn::Live
    } else {
        draw_whole(
            gpu,
            renderers,
            batches,
            waveforms,
            spectrograms,
            canvases,
            tree,
            inputs,
            theme,
        );
        Drawn::Whole
    };

    // Offscreen there is nothing to present: the batches hold the frame, and
    // a capture draws it into a texture of its own.
    let Some(surface) = gpu.surface() else {
        batches.asked.drawn();
        return drawn;
    };
    let acquiring = web_time::Instant::now();
    let frame = match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
        _ => {
            // No drawable this turn (outdated/timed-out surface -- e.g. the
            // compositor stopped consuming a covered window's frames):
            // reconfigure and ask for another redraw, so the frame that was
            // requested is not silently dropped and the window never shows
            // stale state once it is presentable again. What it was asked for
            // stands: the batches already hold it.
            surface.configure(&gpu.device, &gpu.config);
            gpu.request_redraw();
            return Drawn::Owed;
        }
    };
    let acquired = acquiring.elapsed();
    let encoding = web_time::Instant::now();
    let target = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let encoder = encode_pass(
        gpu,
        renderers,
        batches,
        spectrograms,
        canvases,
        theme,
        &target,
    );
    gpu.queue.submit(std::iter::once(encoder.finish()));
    batches.cost.encode = encoding.elapsed();
    let presenting = web_time::Instant::now();
    // The winit present contract: lets winit attach the compositor frame
    // callback to this commit, so later `request_redraw`s are delivered (and
    // throttled) correctly -- without it, Wayland redraw delivery can stall on
    // an unfocused or covered window until the compositor repaints it anyway.
    if let Some(window) = gpu.window() {
        window.pre_present_notify();
    }
    frame.present();
    batches.cost.present = acquired + presenting.elapsed();
    batches.asked.drawn();
    drawn
}

/// **The passes a window's frame is**, into `target`: the batches as they
/// stand, the heavy views between them, resolved out of the multisampled
/// attachment when the window has one. The screen's frame and a capture's
/// are these passes into two textures, which is what keeps a capture the
/// screen's picture.
///
/// A window with live layers keeps everything else ([`Underlay`]): its
/// picture is drawn into a texture of its own when a whole frame changed it,
/// and every frame copies that texture and draws the live layers over it,
/// multisampled in the boxes they cover, so a frame asked for by a moving
/// line costs a copy and that line. A window with nothing live, with a
/// dialog up (whose frames are all whole ones), or whose live layers meet a
/// menu or a band drawn over them draws everything in one pass.
fn encode_pass(
    gpu: &Gpu,
    renderers: &mut Renderers,
    batches: &mut Batches,
    spectrograms: &HashMap<SlotAt, SpectrogramSlot>,
    canvases: &HashMap<i32, CanvasView>,
    theme: &Theme,
    target: &wgpu::TextureView,
) -> wgpu::CommandEncoder {
    let (fb_w, fb_h) = (gpu.config.width.max(1), gpu.config.height.max(1));
    let fb = (fb_w, fb_h);
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gui frame"),
        });
    let clear = batches.kept.clear.unwrap_or_else(|| clear_color(theme));
    let boxes = (batches.kept.valid && batches.kept.has_live())
        .then(|| underlay::plan(&batches.kept.live, &batches.kept.over, fb))
        .flatten();
    #[cfg(test)]
    let boxes = boxes.filter(|_| !batches.one_pass);
    let Some(boxes) = boxes else {
        batches.underlay.release();
        let mut pass = begin_pass(&mut encoder, gpu, target, clear);
        draw_under(&mut pass, renderers, batches, spectrograms, canvases, fb);
        batches.live.draw(&mut pass);
        draw_above(&mut pass, renderers, batches, spectrograms, canvases, fb);
        drop(pass);
        return encoder;
    };
    if !batches.underlay.holds(fb) {
        let kept = batches.underlay.target(&gpu.device, &gpu.queue, fb);
        let mut pass = begin_pass(&mut encoder, gpu, &kept, clear);
        draw_under(&mut pass, renderers, batches, spectrograms, canvases, fb);
        draw_above(&mut pass, renderers, batches, spectrograms, canvases, fb);
        drop(pass);
        batches.underlay.fresh = true;
    }
    // Multisampled, the live layers are drawn in their boxes, each over the
    // picture under it; single-sampled, straight over the copy.
    let multisampled = gpu.msaa_view().is_some();
    let atlas = if multisampled && !boxes.is_empty() {
        batches.underlay.place(
            &gpu.device,
            &gpu.queue,
            &batches.base,
            &batches.kept.live,
            &boxes,
        )
    } else {
        batches.underlay.unplace();
        None
    };
    if let Some((samples, resolved)) = &atlas {
        let mut pass = begin_into(&mut encoder, samples, Some(resolved), clear);
        batches.underlay.draw_boxes(&mut pass);
        drop(pass);
    }
    if multisampled && atlas.is_none() && !boxes.is_empty() {
        // An atlas the card cannot hold: the single pass's way.
        let mut pass = begin_pass(&mut encoder, gpu, target, clear);
        draw_under(&mut pass, renderers, batches, spectrograms, canvases, fb);
        batches.live.draw(&mut pass);
        draw_above(&mut pass, renderers, batches, spectrograms, canvases, fb);
        drop(pass);
        return encoder;
    }
    let mut pass = begin_into(&mut encoder, target, None, clear);
    batches.underlay.draw_frame(&mut pass);
    if !multisampled {
        pass.set_viewport(0.0, 0.0, fb_w as f32, fb_h as f32, 0.0, 1.0);
        pass.set_scissor_rect(0, 0, fb_w, fb_h);
        batches.live.draw(&mut pass);
    }
    drop(pass);
    encoder
}

/// A pass into `target`, cleared to `clear`.
///
/// Antialiasing is a property of the **attachment**, so it is the whole of
/// what MSAA changes here: with it on, every pipeline draws into the
/// multisampled texture and the GPU resolves that into the target as the
/// pass ends. One flag, one texture per window, nothing per widget.
fn begin_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    gpu: &Gpu,
    target: &wgpu::TextureView,
    clear: wgpu::Color,
) -> wgpu::RenderPass<'e> {
    match gpu.msaa_view() {
        Some(ms) => begin_into(encoder, ms, Some(target), clear),
        None => begin_into(encoder, target, None, clear),
    }
}

/// A pass into `attachment`, resolved into `resolve` when it is
/// multisampled. The samples are not kept past the resolve: no pass reads
/// them again.
fn begin_into<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    attachment: &wgpu::TextureView,
    resolve: Option<&wgpu::TextureView>,
    clear: wgpu::Color,
) -> wgpu::RenderPass<'e> {
    let store = if resolve.is_some() {
        wgpu::StoreOp::Discard
    } else {
        wgpu::StoreOp::Store
    };
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("gui pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: attachment,
            resolve_target: resolve,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(clear),
                store,
            },
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// What lies under the live layers: the flat widgets, the window's heavy
/// views and what its elements draw over them.
fn draw_under(
    pass: &mut wgpu::RenderPass<'_>,
    renderers: &mut Renderers,
    batches: &Batches,
    spectrograms: &HashMap<SlotAt, SpectrogramSlot>,
    canvases: &HashMap<i32, CanvasView>,
    fb: (u32, u32),
) {
    batches.base.draw(pass);
    if let Some(collected) = &batches.kept.collected {
        draw_heavy(pass, renderers, spectrograms, canvases, collected, fb);
    }
    batches.over.draw(pass);
}

/// What is over the live layers: a dialog's heavy views, the host's bands,
/// the menus, the lists and the tip.
fn draw_above(
    pass: &mut wgpu::RenderPass<'_>,
    renderers: &mut Renderers,
    batches: &Batches,
    spectrograms: &HashMap<SlotAt, SpectrogramSlot>,
    canvases: &HashMap<i32, CanvasView>,
    fb: (u32, u32),
) {
    if let Some(inside) = &batches.kept.inside {
        draw_heavy(pass, renderers, spectrograms, canvases, inside, fb);
    }
    batches.top.draw(pass);
}

/// **A window's picture on its way back from the card**: the frame copied
/// into a buffer the CPU can map, and what it takes to read it as pixels.
///
/// Reading it is two steps because a page cannot block on the card: the
/// buffer is asked to be mapped as the copy is made ([`capture`]), and once
/// the card has said it is ([`ready`](Self::ready)) it is read
/// ([`pixels`](Self::pixels)). A native front waits between the two
/// ([`wait`](Self::wait)); a page asks again on its next tick.
pub(crate) struct Readback {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    /// The bytes of one row in the buffer: a copy's rows are aligned to
    /// [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`].
    stride: u32,
    /// Whether the texture's channels are blue first.
    bgra: bool,
    /// What the card said about the mapping, once it has.
    mapped: Arc<std::sync::Mutex<Option<Result<(), String>>>>,
}

impl Readback {
    /// What the card has said about the mapping: `None` until it has.
    pub(crate) fn ready(&self) -> Option<Result<(), String>> {
        self.mapped.lock().ok().and_then(|slot| slot.clone())
    }

    /// **Blocks until the card has answered** (native only: a page's card
    /// answers on the browser's own loop).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn wait(&self, device: &wgpu::Device) -> Result<(), String> {
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        self.ready()
            .unwrap_or_else(|| Err("the card did not map the capture".into()))
    }

    /// The picture, as `(width, height, rgba)` rows top to bottom -- once
    /// [`ready`](Self::ready) said the mapping succeeded.
    pub(crate) fn pixels(&self) -> (u32, u32, Vec<u8>) {
        let row = self.width as usize * 4;
        let mut out = Vec::with_capacity(row * self.height as usize);
        {
            let view = self.buffer.slice(..).get_mapped_range();
            for line in view.chunks(self.stride as usize).take(self.height as usize) {
                out.extend_from_slice(&line[..row]);
            }
        }
        self.buffer.unmap();
        for px in out.as_chunks_mut::<4>().0 {
            if self.bgra {
                px.swap(0, 2);
            }
            // The surface is opaque: what a theme's alpha would leave in the
            // texture is not what the screen shows.
            px[3] = 255;
        }
        (self.width, self.height, out)
    }
}

/// **The picture of a window, drawn offscreen**: a whole frame of `tree` laid
/// out and drawn into the window's batches ([`draw_whole`]), the one pass
/// ([`encode_pass`]) encoded into a texture of the window's size instead of
/// its surface, and that texture copied into a buffer asked to be mapped.
///
/// Nothing is presented and the window's next frame is what it would have
/// been: the batches hold the tree as it stands, which is what a frame keeps
/// anyway, and what the window was asked to draw is still asked.
#[allow(clippy::too_many_arguments)] // the per-window resource set, both fronts
pub(crate) fn capture(
    gpu: &mut Gpu,
    renderers: &mut Renderers,
    batches: &mut Batches,
    waveforms: &mut HashMap<SlotAt, WaveformSlot>,
    spectrograms: &mut HashMap<SlotAt, SpectrogramSlot>,
    canvases: &mut HashMap<i32, CanvasView>,
    tree: &Widget,
    inputs: &FrameInputs,
    theme: &Theme,
) -> Result<Readback, String> {
    let theme = tree.theme.as_deref().unwrap_or(theme);
    let format = gpu.config.format;
    let bgra = match format {
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
        other => return Err(format!("cannot read back a {other:?} window")),
    };
    draw_whole(
        gpu,
        renderers,
        batches,
        waveforms,
        spectrograms,
        canvases,
        tree,
        inputs,
        theme,
    );
    let (width, height) = (gpu.config.width.max(1), gpu.config.height.max(1));
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gui capture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = encode_pass(
        gpu,
        renderers,
        batches,
        spectrograms,
        canvases,
        theme,
        &target,
    );
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let stride = (width * 4).div_ceil(align) * align;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gui capture readback"),
        size: u64::from(stride) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        size,
    );
    gpu.queue.submit(std::iter::once(encoder.finish()));
    let mapped = Arc::new(std::sync::Mutex::new(None));
    let answer = Arc::clone(&mapped);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            if let Ok(mut slot) = answer.lock() {
                *slot = Some(result.map_err(|e| e.to_string()));
            }
        });
    Ok(Readback {
        buffer,
        width,
        height,
        stride,
        bgra,
        mapped,
    })
}

/// **A whole frame's picture**, into the batches: the tree laid out, every
/// widget drawn, the heavy views framed -- and what the next frames may keep
/// of it ([`Kept`]).
#[allow(clippy::too_many_arguments)] // the per-window resource set, both fronts
fn draw_whole(
    gpu: &mut Gpu,
    renderers: &mut Renderers,
    batches: &mut Batches,
    waveforms: &mut HashMap<SlotAt, WaveformSlot>,
    spectrograms: &mut HashMap<SlotAt, SpectrogramSlot>,
    canvases: &mut HashMap<i32, CanvasView>,
    tree: &Widget,
    inputs: &FrameInputs,
    theme: &Theme,
) {
    let (fb_w, fb_h) = (gpu.config.width.max(1), gpu.config.height.max(1));
    // **Drawn again if the glyph atlas was packed again under it**: the text
    // drawn before that names texels that now hold other glyphs, and a
    // picture is kept, so it would stay wrong for as long as only lines
    // moved. The second pass finds what it needs already on the new sheet.
    let epoch = atlas_epoch();
    let started = web_time::Instant::now();
    let mut picture = picture(tree, inputs, theme, waveforms, spectrograms, (fb_w, fb_h));
    let mut layout = picture.layout;
    if atlas_epoch() != epoch {
        picture = self::picture(tree, inputs, theme, waveforms, spectrograms, (fb_w, fb_h));
        layout += picture.layout;
    }
    batches.cost.layout = layout;
    batches.cost.build = started.elapsed().saturating_sub(layout);
    batches.cost.placed = picture.placed;
    let uploading = web_time::Instant::now();
    let mut uploaded = 0;
    for (batch, mesh) in [
        (&mut batches.base, &picture.base),
        (&mut batches.over, &picture.over),
        (&mut batches.live, &picture.live),
        (&mut batches.top, &picture.top),
    ] {
        uploaded += batch.upload(&gpu.device, &gpu.queue, mesh, fb_w, fb_h);
    }
    batches.cost.uploaded = uploaded as u64;
    for heavy in std::iter::once(&picture.collected).chain(picture.inside.as_ref()) {
        upload_heavy(
            gpu,
            renderers,
            spectrograms,
            canvases,
            heavy,
            inputs,
            (fb_w, fb_h),
        );
    }
    batches.cost.upload = uploading.elapsed();
    let over = underlay::join(&picture.top.triangle_bounds(), 0.0);
    batches.kept = Kept {
        valid: picture.kept,
        size: (fb_w, fb_h),
        clear: Some(clear_color(theme)),
        epoch: atlas_epoch(),
        places: std::mem::take(&mut picture.places),
        collected: Some(picture.collected),
        inside: picture.inside,
        live: picture.live,
        over,
    };
    batches.underlay.fresh = false;
}

/// A whole frame as triangles and snapshots, before any of it reaches a card.
struct Picture {
    base: Mesh,
    over: Mesh,
    live: Mesh,
    top: Mesh,
    /// The heavy views of the window and, with a dialog up, of the dialog.
    collected: Collected,
    inside: Option<Collected>,
    /// Where each element with a live layer was placed.
    places: Vec<LivePlace>,
    /// Whether the frames after this one may keep it: the live batch is where
    /// the live layers belong, and every one of them can be found again.
    kept: bool,
    /// How long placing the tree took, and how many widgets it placed.
    layout: std::time::Duration,
    placed: u64,
}

/// **The picture of `tree` in a framebuffer of `fb`**: laid out, and drawn
/// into the four meshes of a frame. No card is touched, so it is what a test
/// reads -- and what says, mesh against mesh, that nothing but the live layer
/// follows the clock.
fn picture(
    tree: &Widget,
    inputs: &FrameInputs,
    theme: &Theme,
    waveforms: &HashMap<SlotAt, WaveformSlot>,
    spectrograms: &HashMap<SlotAt, SpectrogramSlot>,
    (fb_w, fb_h): (u32, u32),
) -> Picture {
    let window = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    // The status band comes off the top of the frame, before anything is
    // placed: the same call the hit test makes (`Host::content_area`), so the
    // pixels a press lands on are the pixels the tree was drawn on.
    let bar = status::bar(tree, inputs.status, window, inputs.metrics);
    let area = status::content(tree, inputs.status, window, inputs.metrics);
    // ...and the menu bar's off the top, by the same rule.
    let menu_bar = super::menubar::bar(tree, window, inputs.metrics);
    let area = super::menubar::content(tree, area, inputs.metrics);
    // The lanes' clips are placed on the axis their group currently stands at,
    // so the layout of a multitrack follows the zoom and the pan.
    let laying = web_time::Instant::now();
    let placed = layout::layout_on(area, tree, inputs.metrics);
    let layout_time = laying.elapsed();
    let mut mesh = Mesh::new();
    let mut over = Mesh::new();
    let mut live = Mesh::new();
    let mut top = Mesh::new();
    // **A dialog is the tail of the placements** (`layout` places it last), and
    // it is drawn apart: everything before it is the window as it always was,
    // and the dialog goes over all of that, in the overlay.
    let (base, dialog) = placed.split_at(chrome::modal_start(&placed).unwrap_or(placed.len()));
    let collected = collect_widgets(base, &mut mesh, inputs, theme);
    draw_dividers(&mut mesh, &placed, 0..base.len(), inputs, theme);

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
    draw_element_overlays(&mut over, base, inputs, theme);
    draw_bars(&mut over, base, inputs, theme);
    // **What moves, apart**: the window's live layers, kept by the placements
    // they were drawn in so the next tick can draw them alone.
    let places = live_places(&placed, 0..base.len());

    let inside = if dialog.is_empty() {
        draw_timeline_heads(
            &mut live,
            &collected,
            waveforms,
            spectrograms,
            inputs,
            theme,
        );
        draw_live_places(
            &mut live,
            tree,
            places.as_deref().unwrap_or_default(),
            inputs,
            theme,
        );
        None
    } else {
        // The window behind a dialog is out of reach, and looks it: a scrim
        // over the work area, then the dialog's own picture built on the side
        // and laid over it -- its flat widgets, then its textures, then what
        // its elements draw over themselves.
        //
        // What moves in the window is under the scrim with the rest of it, and
        // what moves in the dialog is over the dialog's own pictures, so with a
        // dialog up nothing is kept and every frame is a whole one.
        //
        // **The scrim and the dialog take the live batch, which is free.** A
        // batch draws all of its flat geometry and then all of its glyphs, so
        // a dialog laid into the batch that holds the window's overlays came
        // out under their text: a meter's ladder read through a dialog that
        // reached it, and a label behind the OK button through the button.
        // Drawn in the next batch, everything of the window is under the scrim
        // and the dialog is whole over it.
        draw_timeline_heads(
            &mut over,
            &collected,
            waveforms,
            spectrograms,
            inputs,
            theme,
        );
        draw_live_places(
            &mut over,
            tree,
            places.as_deref().unwrap_or_default(),
            inputs,
            theme,
        );
        let mut under = Mesh::new();
        let mut above = Mesh::new();
        let inside = collect_widgets(dialog, &mut under, inputs, theme);
        draw_dividers(&mut under, &placed, base.len()..placed.len(), inputs, theme);
        draw_timeline_meshes(
            &mut under,
            &mut above,
            &inside,
            waveforms,
            spectrograms,
            inputs,
            theme,
        );
        draw_static_meshes(&mut under, &mut above, &inside, inputs, theme, tree);
        draw_element_overlays(&mut above, dialog, inputs, theme);
        draw_bars(&mut above, dialog, inputs, theme);
        draw_timeline_heads(&mut above, &inside, waveforms, spectrograms, inputs, theme);
        let within = live_places(&placed, base.len()..placed.len());
        draw_live_places(
            &mut above,
            tree,
            within.as_deref().unwrap_or_default(),
            inputs,
            theme,
        );
        live.set_clip(None);
        live.set_ink(Ink::default());
        live.rect(area, with_alpha(theme.background, 0.6));
        live.append(&under);
        top.append(&above);
        Some(inside)
    };

    // Into the top batch after the tree, so the bar reads over whatever ran
    // up to its edge.
    if let Some(band) = bar {
        let progress = match &tree.kind {
            WidgetKind::Window { progress, .. } => progress.as_ref(),
            _ => None,
        };
        draw_status(&mut top, band, inputs, progress, theme);
    }
    // **Work in progress over the window**, while the owner says it runs.
    if let WidgetKind::Window {
        progress: Some(progress),
        ..
    } = &tree.kind
    {
        draw_progress_overlay(&mut top, area, progress, inputs, theme);
    }
    // **The popup layer, last of all**: a list covers what it opened over. It
    // is placed inside `area` -- the window minus the host's bands -- by the
    // function the press hit-tests with, so it never runs under the bar and a
    // row is hit where it is drawn.
    if let Some(band) = menu_bar {
        draw_menu_bar(&mut top, tree, band, inputs, theme);
    }
    if let Some(popups) = inputs.popups {
        draw_popups(&mut top, popups, tree, &placed, window, area, inputs, theme);
    }
    for mesh in [&mut mesh, &mut over, &mut live, &mut top] {
        mesh.set_clip(None);
        mesh.set_ink(Ink::default());
    }
    Picture {
        base: mesh,
        over,
        live,
        top,
        kept: inside.is_none() && places.is_some(),
        places: places.unwrap_or_default(),
        collected,
        inside,
        layout: layout_time,
        placed: placed.len() as u64,
    }
}

/// Pushes this frame's textures and uniforms to the heavy views in
/// `collected`: the spectrograms framed on their bodies, and the shader canvases
/// recompiled where their source changed.
fn upload_heavy(
    gpu: &Gpu,
    renderers: &mut Renderers,
    spectrograms: &mut HashMap<SlotAt, SpectrogramSlot>,
    canvases: &mut HashMap<i32, CanvasView>,
    collected: &Collected,
    inputs: &FrameInputs,
    (fb_w, fb_h): (u32, u32),
) {
    for item in &collected.timeline_items {
        // The body the element stated when it described its frame: one
        // rectangle, so the picture and the chrome around it agree.
        let body = item.body;
        // The texture layer, when the stack has one: a waveform's picture is
        // triangles and went into the window's mesh with the rest of the
        // chrome, so there is nothing to prepare for it here.
        if item.look.layers.has(Paint::Spectrogram)
            && let Some(slot) = spectrograms.get_mut(&(item.id, item.key))
        {
            let look = &item.look.look;
            let nav = chrome_for(inputs, item.id, &item.editor, || {
                View::full(slot.total_samples())
            })
            .nav;
            let nav = placed_nav(&nav, item.editor.offset);
            let channels = slot.views.len();
            let rows = if item.look.overlay { 1 } else { channels };
            let alpha = item.look.layers.alpha_of(Paint::Spectrogram).unwrap_or(1.0);
            if slot.stale.get().is_none() {
                for view in &mut slot.views {
                    view.recall(look.analysis);
                }
            }
            for (ch, view) in slot.views.iter_mut().enumerate() {
                view.set_display(
                    look.db_floor,
                    look.db_ceil,
                    look.freq_scale,
                    look.colormap.max(0) as u32,
                );
                view.set_shaping(look.normalize, look.interpolate, look.tuning);
                view.set_alpha(alpha);
                view.set_freq_window(item.look.y.0, item.look.y.1);
                view.set_framing(framing_of(
                    item.look.lanes.rect(body, rows, ch.min(rows - 1)),
                    fb_w,
                    fb_h,
                ));
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
    // The spectral clip bodies: the same texture, uploaded against the clip's
    // own axis instead of the group's window -- which is the whole difference
    // between a spectral *view* of a file and a spectral *clip* of it.
    for item in &collected.spectral_bodies {
        if let Some(slot) = spectrograms.get_mut(&(item.id, item.key)) {
            let channels = slot.views.len();
            for (ch, view) in slot.views.iter_mut().enumerate() {
                view.set_display(
                    item.look.db_floor,
                    item.look.db_ceil,
                    item.look.freq_scale,
                    item.look.colormap.max(0) as u32,
                );
                view.set_shaping(item.look.normalize, item.look.interpolate, item.look.tuning);
                view.set_framing(framing_of(
                    channel_rect(item.rect, channels, ch),
                    fb_w,
                    fb_h,
                ));
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
}

/// Draws the heavy views in `collected` into `pass`, each through its own
/// viewport and scissor, and gives the pass back with the whole framebuffer as
/// both: the flat batch drawn next is in window space, already clipped by its
/// geometry where it needed to be.
fn draw_heavy(
    pass: &mut wgpu::RenderPass<'_>,
    renderers: &Renderers,
    spectrograms: &HashMap<SlotAt, SpectrogramSlot>,
    canvases: &HashMap<i32, CanvasView>,
    collected: &Collected,
    (fb_w, fb_h): (u32, u32),
) {
    for item in &collected.timeline_items {
        // The body the element stated when it described its frame: one
        // rectangle, so the picture and the chrome around it agree.
        let body = item.body;
        if body.w < 1.0 || body.h < 1.0 {
            continue;
        }
        if !apply_scissor(pass, item.clip, fb_w, fb_h) {
            continue;
        }
        if !item.look.layers.has(Paint::Spectrogram) {
            continue;
        }
        let Some(slot) = spectrograms.get(&(item.id, item.key)) else {
            continue;
        };
        let channels = slot.views.len();
        let rows = if item.look.overlay { 1 } else { channels };
        // A stack scrolled past its body draws only inside the body's rows,
        // never over the chrome above or below it.
        if item.look.lanes.overflow(rows, body.h) > 0.0
            && !apply_scissor(
                pass,
                Some(rows_clip(item.clip, item.rect, body)),
                fb_w,
                fb_h,
            )
        {
            continue;
        }
        for (ch, view) in slot.views.iter().enumerate() {
            let row = item.look.lanes.rect(body, rows, ch.min(rows - 1));
            let (x, y, w, h) = clamp_viewport(row, fb_w, fb_h);
            if w >= 1.0 && h >= 1.0 {
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                view.draw(pass, renderers);
            }
        }
    }
    for item in &collected.spectral_bodies {
        let Some(slot) = spectrograms.get(&(item.id, item.key)) else {
            continue;
        };
        if item.rect.w < 1.0 || item.rect.h < 1.0 || !apply_scissor(pass, item.clip, fb_w, fb_h) {
            continue;
        }
        let channels = slot.views.len();
        for (ch, view) in slot.views.iter().enumerate() {
            let row = channel_rect(item.rect, channels, ch);
            let (x, y, w, h) = clamp_viewport(row, fb_w, fb_h);
            if w >= 1.0 && h >= 1.0 {
                pass.set_viewport(x, y, w, h, 0.0, 1.0);
                view.draw(pass, renderers);
            }
        }
    }
    for frame in &collected.canvas_frames {
        if frame.body.w >= 1.0
            && frame.body.h >= 1.0
            && let Some(view) = canvases.get(&frame.id)
            && apply_scissor(pass, frame.clip, fb_w, fb_h)
        {
            let (x, y, w, h) = clamp_viewport(frame.body, fb_w, fb_h);
            pass.set_viewport(x, y, w, h, 0.0, 1.0);
            view.draw(pass);
        }
    }
    pass.set_viewport(0.0, 0.0, fb_w as f32, fb_h as f32, 0.0, 1.0);
    pass.set_scissor_rect(0, 0, fb_w, fb_h);
}

/// Applies a placed widget's clip as the pass scissor (the full framebuffer
/// when it has none), returning `false` when the clip is empty -- the caller
/// skips the draw entirely. The heavy views draw through `set_viewport`, which
/// *positions and scales* but does not cut; a scrolled view poking out of its
/// `scroll` container is cut by this scissor, the GPU sibling of the mesh's
/// geometric clip. What the scissor cannot reach is the **window** edge, since
/// a viewport may not leave the attachment at all -- that is [`Framing`]'s
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
/// origin. What the viewport still cannot do is cut -- it scales whatever the
/// view draws into whatever rectangle it is given -- so a view that is only
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

    /// **A redefine keeps the pictures of the views that read what they
    /// read**: a view on the same buffer keeps every slot it had, a view moved
    /// to another buffer or gone keeps none, and a slot the new load filled
    /// is not replaced by the old one.
    #[test]
    fn a_redefine_keeps_the_pictures_of_views_on_the_same_buffer() {
        use crate::host::widget::element::SlotKey;
        let before = reads_of(&[(1, 7, false), (2, 8, false), (3, 9, false)]);
        let now = [(1, 7, false), (2, 5, false), (4, 9, false)];
        let body = SlotKey(0);
        let old: HashMap<SlotAt, &str> = [
            ((1, SlotKey::SELF), "one"),
            ((1, body), "one's body"),
            ((2, SlotKey::SELF), "two"),
            ((3, SlotKey::SELF), "three"),
        ]
        .into();
        let mut into: HashMap<SlotAt, &str> = HashMap::new();
        carry_slots(&before, &now, old.clone(), &mut into);
        let mut kept: Vec<_> = into.values().copied().collect();
        kept.sort_unstable();
        assert_eq!(kept, ["one", "one's body"]);
        let mut loaded: HashMap<SlotAt, &str> = [((1, SlotKey::SELF), "new")].into();
        carry_slots(&before, &now, old, &mut loaded);
        assert_eq!(loaded[&(1, SlotKey::SELF)], "new");
        assert_eq!(loaded[&(1, body)], "one's body");
    }

    /// **A window drawn offscreen and read back is the window's picture**:
    /// the size of its framebuffer, the theme's background where nothing is
    /// drawn and something else where a slider is -- through the one pass the
    /// screen's frame takes. Skipped, and said so, on a machine where no
    /// adapter answers at all, a software one included.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_window_is_drawn_offscreen_and_read_back() {
        use clausters_core::osc::{OscMessage, OscPacket, OscType};

        let Ok(mut gpu) = pollster::block_on(Gpu::offscreen(320, 200, 4)) else {
            eprintln!("no adapter on this machine: nothing to draw offscreen with");
            return;
        };
        let mut host = crate::host::Host::new();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: crate::host::GUI_DEF.into(),
                args: vec![
                    OscType::Int(1),
                    OscType::String(
                        r#"{"type":"window","w":320,"h":200,"children":[
                            {"id":2,"type":"slider","value":0.7}]}"#
                            .into(),
                    ),
                ],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        let mut renderers = Renderers::new(&gpu.device, gpu.target());
        let mut batches = Batches::new(&gpu.device, gpu.target());
        let inputs = FrameInputs {
            metrics: host.metrics_for(1),
            world: World {
                timelines: host.timelines(),
                ..World::default()
            },
            ..FrameInputs::default()
        };
        let readback = capture(
            &mut gpu,
            &mut renderers,
            &mut batches,
            &mut HashMap::new(),
            &mut HashMap::new(),
            &mut HashMap::new(),
            host.window_def(1).expect("the window is defined"),
            &inputs,
            &host.theme,
        )
        .expect("a plain window reads back");
        readback.wait(&gpu.device).expect("the card maps it");
        let (w, h, rgba) = readback.pixels();
        assert_eq!((w, h), (320, 200));
        let background = clear_color(&host.theme);
        let bg = [background.r, background.g, background.b].map(|c| (c * 255.0).round() as i32);
        let differs = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| (0..3).any(|c| (i32::from(px[c]) - bg[c]).abs() > 2))
            .count();
        assert_eq!(
            &rgba[..3].iter().map(|&c| i32::from(c)).collect::<Vec<_>>(),
            &bg,
            "the corner is the background"
        );
        assert!(
            differs > 500,
            "the slider is drawn: {differs} pixels differ"
        );
    }

    /// **A span is drawn to the take's edges it reaches**: a run read back
    /// from the take's first frame is drawn from the start and one whose
    /// windows reach the end to it, as a whole transform is, where they once
    /// left half a window dark (a strip at the start of a page's
    /// spectrogram). A run inside the take is drawn where its columns are.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_span_reaches_the_edges_of_the_take_it_reaches() {
        let Ok(gpu) = pollster::block_on(Gpu::offscreen(64, 64, 1)) else {
            eprintln!("no adapter on this machine: nothing to draw offscreen with");
            return;
        };
        let renderers = Renderers::new(&gpu.device, gpu.target());
        let total = 96_000;
        let run = |a: usize, b: usize| {
            let samples: Vec<f32> = (a..b).map(|k| (k as f32 * 0.05).sin()).collect();
            let stft = Stft::compute(&samples, 2048, 128, 48_000.0).placed_at(a, total);
            spectrogram_slot(vec![stft], &gpu, &renderers).expect("a slot")
        };
        let whole = run(0, total);
        assert_eq!(whole.views[0].covers(), (f64::NEG_INFINITY, f64::INFINITY));
        assert!(whole.views[0].is_whole(), "the run is the whole take");
        let (a, b) = run(0, 30_000).views[0].covers();
        assert!(a == f64::NEG_INFINITY && b < 30_000.0, "from the start");
        let (a, b) = run(60_000, total).views[0].covers();
        assert!(a > 60_000.0 && b == f64::INFINITY, "to the end");
        let inside = run(20_000, 40_000);
        let (a, b) = inside.views[0].covers();
        assert!(a > 20_000.0 && b < 40_000.0, "where its columns are");
        assert!(!inside.views[0].is_whole());
    }

    /// **A picture of the whole take by an analysis left is kept, and going
    /// back to that analysis draws it again with nothing asked**: over the
    /// whole take the view becomes it, over a span the overview does, and
    /// the one it replaces is kept in its place. An edit keeps none.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_picture_of_an_analysis_left_is_brought_back() {
        let Ok(gpu) = pollster::block_on(Gpu::offscreen(64, 64, 1)) else {
            eprintln!("no adapter on this machine: nothing to draw offscreen with");
            return;
        };
        let renderers = Renderers::new(&gpu.device, gpu.target());
        let total = 480_000;
        let whole = |window: usize| {
            let bins = window / 2 + 1;
            Stft::of_span(
                vec![0.5; bins * 60],
                bins,
                0,
                total / 60 + 1,
                window,
                48_000.0,
                total,
            )
            .expect("a whole take's columns")
            .with_analysis(window / 2, 0)
        };
        let slot = |stft: Stft| spectrogram_slot(vec![stft], &gpu, &renderers).expect("a slot");
        let (a, b) = ((1024, 512, 0), (4096, 2048, 0));
        let nav = View::full(total);

        let mut shown = slot(whole(4096));
        keep_overviews(&mut shown, slot(whole(1024)));
        assert!(shown.views[0].analyzed_as(b));
        assert!(shown.views[0].can_recall(a), "the analysis left is kept");
        assert!(
            columns_owed(&shown, &nav, 400.0, a).is_none(),
            "nothing is asked"
        );
        assert!(shown.views[0].recall(a));
        assert!(shown.views[0].analyzed_as(a), "the whole take is it again");
        assert!(
            shown.views[0].can_recall(b),
            "and the one it replaced is kept"
        );

        let bins = 2049;
        let span = Stft::of_span(
            vec![0.5; bins * 8],
            bins,
            96_000,
            2048,
            4096,
            48_000.0,
            total,
        )
        .expect("a span's columns")
        .with_analysis(2048, 0);
        let mut zoomed = slot(span);
        keep_overviews(&mut zoomed, shown);
        let view = &mut zoomed.views[0];
        assert!(view.overview().is_some_and(|o| o.analyzed_as(a)));
        assert!(view.recall(b), "the overview of the span's own analysis");
        assert!(view.overview().is_some_and(|o| o.analyzed_as(b)));
        assert!(view.can_recall(a));

        zoomed.stale.set(Some(total));
        let mut edited = slot(whole(1024));
        keep_overviews(&mut edited, zoomed);
        assert!(!edited.views[0].can_recall(b), "an edit keeps none");
    }

    /// **A frame of the kept picture is the single pass's frame**: a window
    /// of everything that moves, drawn with its live layers in their boxes
    /// over the kept picture, reads back as the same window drawn in one
    /// pass: exactly when single-sampled, and multisampled but for the few
    /// pixels where a live edge crosses an edge of the picture. Skipped, and
    /// said so, where no adapter answers.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_kept_picture_frame_is_the_single_pass_frame() {
        use crate::host::world::HeadClocks;
        use clausters_core::osc::{OscMessage, OscPacket, OscType};

        struct Flat(f32);
        impl crate::host::BusSource for Flat {
            fn control(&self, _index: usize) -> f32 {
                self.0
            }
        }
        for samples in [1, 4] {
            let Ok(mut gpu) = pollster::block_on(Gpu::offscreen(360, 420, samples)) else {
                eprintln!("no adapter on this machine: nothing to draw offscreen with");
                return;
            };
            let mut host = crate::host::Host::new();
            host.handle_packet(
                OscPacket::Message(OscMessage {
                    addr: crate::host::GUI_DEF.into(),
                    args: vec![OscType::Int(1), OscType::String(MOVING.into())],
                }),
                crate::host::ClientId::Udp(std::net::SocketAddr::from((
                    std::net::Ipv4Addr::LOCALHOST,
                    9000,
                ))),
            );
            let bus = Flat(0.6);
            let inputs = FrameInputs {
                metrics: host.metrics_for(1),
                world: World {
                    bus: Some(&bus),
                    clocks: HeadClocks::uniform(0.4),
                    timelines: host.timelines(),
                    ..World::default()
                },
                ..FrameInputs::default()
            };
            let mut renderers = Renderers::new(&gpu.device, gpu.target());
            let mut batches = Batches::new(&gpu.device, gpu.target());
            let mut read = |one_pass: bool| {
                batches.one_pass = one_pass;
                let readback = capture(
                    &mut gpu,
                    &mut renderers,
                    &mut batches,
                    &mut HashMap::new(),
                    &mut HashMap::new(),
                    &mut HashMap::new(),
                    host.window_def(1).expect("the window is defined"),
                    &inputs,
                    &host.theme,
                )
                .expect("the window reads back");
                readback.wait(&gpu.device).expect("the card maps it");
                (readback.pixels().2, batches.underlay.holds((360, 420)))
            };
            let (kept, held) = read(false);
            assert!(held, "{samples} samples: the window keeps its picture");
            let (single, _) = read(true);
            // A live edge that crosses an edge of the picture inside one
            // pixel blends over the picture's resolved color rather than
            // over each sample of it: both antialiased, a few levels apart
            // on those pixels alone.
            let (mut off, mut worst) = (0, 0);
            for (a, b) in kept
                .as_chunks::<4>()
                .0
                .iter()
                .zip(single.as_chunks::<4>().0)
            {
                let d = (0..3)
                    .map(|c| (i32::from(a[c]) - i32::from(b[c])).abs())
                    .max()
                    .unwrap_or(0);
                off += usize::from(d > 2);
                worst = worst.max(d);
            }
            let edges = if samples == 1 { 0 } else { 360 * 420 / 2000 };
            assert!(
                off <= edges && worst <= 32,
                "{samples} samples: {off} pixels differ, by up to {worst}"
            );
        }
    }

    /// **A window drawn offscreen is the picture kept beside this test**,
    /// within two levels a channel: what a change to the drawing of text, a
    /// slider, a knob or a roll's notes moves, it shows here. The tolerance
    /// is the measured distance between a card and a software adapter
    /// (`docs/decisions.md`, "A capture is the same picture on any adapter"),
    /// so the picture holds on whatever machine runs the suite.
    ///
    /// A change that is meant to move it writes the picture again:
    /// `CLAUSTERS_GUI_BLESS=1 cargo test --lib -- reference_picture`, then
    /// look at `tests/references/window.png` before committing it. Skipped,
    /// and said so, where no adapter answers.
    #[test]
    #[cfg(all(not(target_arch = "wasm32"), feature = "font-atlas"))]
    fn a_window_is_its_reference_picture() {
        let Some((w, h, rgba)) = draw_reference(false) else {
            eprintln!("no adapter on this machine: nothing to compare");
            return;
        };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("references")
            .join("window.png");
        if std::env::var_os("CLAUSTERS_GUI_BLESS").is_some() {
            std::fs::create_dir_all(path.parent().expect("a directory")).expect("made");
            std::fs::write(&path, crate::host::png::encode_rgba(w, h, &rgba)).expect("written");
            eprintln!("wrote {}", path.display());
            return;
        }
        let kept = std::fs::read(&path).expect("the reference picture is kept beside the tests");
        let (kw, kh, reference) =
            crate::host::png::decode_stored(&kept).expect("a picture this host wrote");
        assert_eq!((w, h), (kw, kh), "the window's size");
        let far: Vec<(usize, u8)> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(reference.as_chunks::<4>().0)
            .enumerate()
            .filter_map(|(i, (a, b))| {
                let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
                (d > 2).then_some((i, d))
            })
            .collect();
        assert!(
            far.is_empty(),
            "{} pixels moved more than two levels, the first at ({}, {}) by {}",
            far.len(),
            far[0].0 % w as usize,
            far[0].0 / w as usize,
            far[0].1
        );
    }

    /// The window the reference picture is of, drawn offscreen -- on a
    /// software adapter when `software` -- as `(width, height, rgba)`.
    #[cfg(all(not(target_arch = "wasm32"), feature = "font-atlas"))]
    fn draw_reference(software: bool) -> Option<(u32, u32, Vec<u8>)> {
        use clausters_core::osc::{OscMessage, OscPacket, OscType};

        const TREE: &str = r#"{"type":"window","w":240,"h":150,"layout":"col","children":[
            {"id":2,"type":"label","text":"Eight bars, 0123"},
            {"id":3,"type":"slider","value":0.37},
            {"id":4,"type":"notes","h":70,"min":58,"max":70,
             "sample_rate":48000.0,"notes":[0,24000,60,100,0, 18000,30000,64,100,0,
                                            41000,12000,67,100,0]}]}"#;
        let mut gpu = pollster::block_on(Gpu::offscreen_on(240, 150, 4, software)).ok()?;
        let mut host = crate::host::Host::new();
        #[cfg(feature = "font-atlas")]
        host.load_default_face();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: crate::host::GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(TREE.into())],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        let mut renderers = Renderers::new(&gpu.device, gpu.target());
        let mut batches = Batches::new(&gpu.device, gpu.target());
        let inputs = FrameInputs {
            metrics: host.metrics_for(1),
            world: World {
                timelines: host.timelines(),
                ..World::default()
            },
            ..FrameInputs::default()
        };
        let readback = capture(
            &mut gpu,
            &mut renderers,
            &mut batches,
            &mut HashMap::new(),
            &mut HashMap::new(),
            &mut HashMap::new(),
            host.window_def(1)?,
            &inputs,
            &host.theme,
        )
        .ok()?;
        readback.wait(&gpu.device).ok()?;
        Some(readback.pixels())
    }

    /// **The reference picture holds on a software adapter too**: where a
    /// machine has one, the picture drawn on it is compared with the kept
    /// one exactly as a card's is -- which is what makes two levels the
    /// tolerance, rather than a number picked for one machine.
    #[test]
    #[cfg(all(not(target_arch = "wasm32"), feature = "font-atlas"))]
    fn the_reference_picture_holds_on_a_software_adapter() {
        let (Some(card), Some(software)) = (draw_reference(false), draw_reference(true)) else {
            eprintln!("this machine does not have both adapters");
            return;
        };
        assert_eq!((card.0, card.1), (software.0, software.1));
        let largest = card
            .2
            .as_chunks::<4>()
            .0
            .iter()
            .zip(software.2.as_chunks::<4>().0)
            .map(|(a, b)| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0))
            .max()
            .unwrap_or(0);
        assert!(largest <= 2, "the two adapters differ by {largest} levels");
    }

    /// **How far a software adapter's picture is from a card's**, for the
    /// same window: text, controls with rounded ends, and a roll's notes,
    /// drawn at four samples through each adapter and compared pixel by
    /// pixel. A measurement, run by hand on a machine that has both:
    /// `cargo test --lib -- --ignored --nocapture software_against`.
    #[test]
    #[ignore = "a measurement; needs a card and a software adapter"]
    #[cfg(not(target_arch = "wasm32"))]
    fn software_against_hardware() {
        use clausters_core::osc::{OscMessage, OscPacket, OscType};

        const TREE: &str = r#"{"type":"window","w":480,"h":320,"layout":"col","children":[
            {"id":2,"type":"label","text":"The quick brown fox, 0123456789"},
            {"id":3,"type":"slider","value":0.37},
            {"id":4,"type":"knob","value":0.62,"h":90},
            {"id":5,"type":"notes","h":140,"min":48,"max":72,
             "sample_rate":48000.0,"notes":[0,24000,60,100,0, 18000,30000,64,100,0,
                                            41000,12000,67,100,0]}]}"#;
        let draw = |software: bool, samples: u32| -> Option<(String, Vec<u8>)> {
            let mut gpu =
                pollster::block_on(Gpu::offscreen_on(480, 320, samples, software)).ok()?;
            let mut host = crate::host::Host::new();
            // Text through the typeface where the build rasterizes one.
            #[cfg(feature = "font-atlas")]
            host.load_default_face();
            host.handle_packet(
                OscPacket::Message(OscMessage {
                    addr: crate::host::GUI_DEF.into(),
                    args: vec![OscType::Int(1), OscType::String(TREE.into())],
                }),
                crate::host::ClientId::Udp(std::net::SocketAddr::from((
                    std::net::Ipv4Addr::LOCALHOST,
                    9000,
                ))),
            );
            let mut renderers = Renderers::new(&gpu.device, gpu.target());
            let mut batches = Batches::new(&gpu.device, gpu.target());
            let inputs = FrameInputs {
                metrics: host.metrics_for(1),
                world: World {
                    timelines: host.timelines(),
                    ..World::default()
                },
                ..FrameInputs::default()
            };
            let readback = capture(
                &mut gpu,
                &mut renderers,
                &mut batches,
                &mut HashMap::new(),
                &mut HashMap::new(),
                &mut HashMap::new(),
                host.window_def(1)?,
                &inputs,
                &host.theme,
            )
            .ok()?;
            readback.wait(&gpu.device).ok()?;
            let name = format!("{}, {}x", gpu.adapter, gpu.target().samples);
            Some((name, readback.pixels().2))
        };
        let compare = |what: &str, a: &[u8], b: &[u8]| {
            let mut differ = 0usize;
            let mut largest = 0u8;
            for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
                let d = (0..3).map(|c| pa[c].abs_diff(pb[c])).max().unwrap_or(0);
                differ += usize::from(d > 0);
                largest = largest.max(d);
            }
            let total = a.len() / 4;
            eprintln!(
                "{what}: {differ} of {total} pixels differ ({:.2}%), by at most {largest}",
                100.0 * differ as f64 / total as f64,
            );
        };
        for samples in [4, 1] {
            let (Some((card, a)), Some((software, b))) =
                (draw(false, samples), draw(true, samples))
            else {
                eprintln!("this machine does not have both adapters");
                return;
            };
            eprintln!("card: {card}\nsoftware: {software}");
            compare("card against software", &a, &b);
            if let (Some((_, a2)), Some((_, b2))) = (draw(false, samples), draw(true, samples)) {
                compare("card against itself", &a, &a2);
                compare("software against itself", &b, &b2);
            }
        }
    }

    /// The picture of window 1 of a host holding `json`, with every playhead
    /// reading `clock` and the control buses reading `level`.
    fn picture_at(json: &str, clock: f64, level: f32) -> Picture {
        use crate::host::world::HeadClocks;
        use clausters_core::osc::{OscMessage, OscPacket, OscType};

        /// Every control bus at one value: a meter's level, moved by a test.
        struct Flat(f32);
        impl crate::host::BusSource for Flat {
            fn control(&self, _index: usize) -> f32 {
                self.0
            }
        }

        let mut host = crate::host::Host::new();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: crate::host::GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(json.into())],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        let bus = Flat(level);
        let inputs = FrameInputs {
            metrics: host.metrics_for(1),
            world: World {
                bus: Some(&bus),
                clocks: HeadClocks::uniform(clock),
                timelines: host.timelines(),
                ..World::default()
            },
            ..FrameInputs::default()
        };
        picture(
            host.window_def(1).expect("the window is defined"),
            &inputs,
            &host.theme,
            &HashMap::new(),
            &HashMap::new(),
            (800, 600),
        )
    }

    #[test]
    fn a_panes_playhead_moves_without_a_signal_body() {
        let json = r#"{"type":"window","children":[
            {"type":"pane","id":10,"layer":"points","sample_rate":1,"playhead_at":0,
             "children":[{"type":"curve","id":11,"duration":100,
                "points":[0,0.2,1,0,100,0.8,1,0]}]}]}"#;
        let still = picture_at(json, 0.0, 0.0);
        let moved = picture_at(json, 20.0, 0.0);
        assert!(still.base.near(&moved.base, 0.0));
        assert!(still.over.near(&moved.over, 0.0));
        assert!(
            !still.live.near(&moved.live, LIVE_SLACK),
            "the pane draws its own head without a signal slot"
        );
    }

    /// A window of everything a clock or a level moves that is drawn into
    /// the mesh: a roll and a page, each anchored, a metered multitrack, a
    /// meter, a sweeping bar and a clock's reading beside them.
    const MOVING: &str = r#"{"type":"window","children":[
        {"id":2,"type":"label","text":"0.000 s","live":true,"h":20},
        {"id":3,"type":"notes","notes":[0.0,50.0,60.0,100.0,0.0],
         "playhead_at":0,"sample_rate":48000},
        {"id":4,"type":"score","playhead_at":0,"vb":[100,100],"prims":[],
         "cursors":[{"t":0,"x":10,"y0":10,"y1":90},{"t":500,"x":60,"y0":10,"y1":90}]},
        {"id":5,"type":"multitrack","playhead_at":0,
         "tracks":["one","",100,0,0,1.0,1],"meters":["one",10,12,2],
         "clips":["a","one",0,48000,0,"a",-1]},
        {"id":6,"type":"meter","bus":20,"rate":"control","h":80},
        {"id":7,"type":"progress","h":30}]}"#;

    /// **Nothing but the live layer follows the clock or a level.** The
    /// window's picture -- its base, what is drawn over its textures, the
    /// host's own bands -- is the same triangles wherever the transport is
    /// and whatever a meter reads, which is what lets a tick keep it; and the
    /// live layer is not, which is what a tick draws.
    #[test]
    fn only_the_live_layer_follows_the_clock_and_the_levels() {
        let still = picture_at(MOVING, 0.0, 0.0);
        assert!(still.kept, "nothing here keeps a frame from being kept");
        assert!(
            !still.places.is_empty(),
            "the elements that move are placed for the next tick"
        );
        for (what, clock, level) in [("the clock", 24_000.0, 0.0), ("a level", 0.0, 0.5)] {
            let moved = picture_at(MOVING, clock, level);
            assert!(
                still.base.near(&moved.base, 0.0),
                "the window's picture does not read {what}"
            );
            assert!(
                still.over.near(&moved.over, 0.0),
                "nor does what is drawn over its textures ({what})"
            );
            assert!(still.top.near(&moved.top, 0.0), "nor the host's bands");
            assert!(
                !still.live.near(&moved.live, LIVE_SLACK),
                "the live layer follows {what}"
            );
        }
    }

    /// **The live layers drawn again are the live layers of a whole frame.**
    /// A tick finds each element by the way down to it and hands it the
    /// placement it was laid out in; what comes out is what a whole frame at
    /// the same clock draws there, so a kept picture and a fresh one cannot
    /// be told apart.
    #[test]
    fn a_tick_draws_the_live_layers_a_whole_frame_would() {
        use crate::host::world::HeadClocks;
        use clausters_core::osc::{OscMessage, OscPacket, OscType};

        let mut host = crate::host::Host::new();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: crate::host::GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(MOVING.into())],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        let tree = host.window_def(1).unwrap();
        let at = |clock: f64| FrameInputs {
            metrics: host.metrics_for(1),
            world: World {
                clocks: HeadClocks::uniform(clock),
                timelines: host.timelines(),
                ..World::default()
            },
            ..FrameInputs::default()
        };
        let none = (HashMap::new(), HashMap::new());
        let kept = picture(tree, &at(0.0), &host.theme, &none.0, &none.1, (800, 600));
        let whole = picture(
            tree,
            &at(24_000.0),
            &host.theme,
            &none.0,
            &none.1,
            (800, 600),
        );
        let mut tick = Mesh::new();
        draw_timeline_heads(
            &mut tick,
            &kept.collected,
            &none.0,
            &none.1,
            &at(24_000.0),
            &host.theme,
        );
        draw_live_places(&mut tick, tree, &kept.places, &at(24_000.0), &host.theme);
        assert!(tick.near(&whole.live, 0.0));
    }

    /// **A multitrack's meters stay inside the multitrack**: tracks taller
    /// than the window, so the stack runs past the rectangle it is given at
    /// the bottom, with a label under it -- and the live layer, drawn over
    /// everything else, paints nothing outside the multitrack's placement.
    #[test]
    fn a_tall_multitracks_meters_stay_inside_it() {
        use clausters_core::osc::{OscMessage, OscPacket, OscType};
        const TALL: &str = r#"{"type":"window","children":[
            {"id":2,"type":"label","text":"above","h":40},
            {"id":5,"type":"multitrack",
             "tracks":["one","",250,0,0,1.0,1,"two","",250,0,0,1.0,1,"three","",250,0,0,1.0,1],
             "meters":["one",10,12,2,"two",14,16,2,"three",18,20,2],
             "clips":["a","one",0,48000,0,"a",-1]},
            {"id":7,"type":"label","text":"below","h":40}]}"#;
        let mut host = crate::host::Host::new();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: crate::host::GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(TALL.into())],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        let tree = host.window_def(1).unwrap();
        let inputs = FrameInputs {
            metrics: host.metrics_for(1),
            ..FrameInputs::default()
        };
        let none = (HashMap::new(), HashMap::new());
        let frame = picture(tree, &inputs, &host.theme, &none.0, &none.1, (800, 600));
        let place = frame
            .places
            .iter()
            .find(|p| p.path == [1])
            .expect("the multitrack");
        // The work area: the window less its status band and its menu bar,
        // which are drawn over it after everything else.
        let window = Rect::new(0.0, 0.0, 800.0, 600.0);
        let area = status::content(tree, inputs.status, window, inputs.metrics);
        let area = crate::host::menubar::content(tree, area, inputs.metrics);
        let bounds = place
            .clip
            .map_or(place.rect, |c| c.intersect(place.rect))
            .intersect(area);
        let drawn = frame.live.extent().expect("the meters drew");
        assert!(
            drawn.y >= bounds.y - 0.5 && drawn.y + drawn.h <= bounds.y + bounds.h + 0.5,
            "the live layer spans {}..{}, the multitrack {}..{}",
            drawn.y,
            drawn.y + drawn.h,
            bounds.y,
            bounds.y + bounds.h
        );
    }

    /// **Work in progress is drawn over the window**: a window whose owner
    /// says it is rendering draws a panel over its content -- the dimming,
    /// the panel and its frame, the bar and its fill, the name and the
    /// percent -- on the top layer, and one that is not draws none of it.
    #[test]
    fn work_in_progress_is_drawn_over_the_window() {
        use clausters_core::osc::{OscMessage, OscPacket, OscType};
        let top = |progress: f64| {
            let mut host = crate::host::Host::new();
            let def = format!(
                r#"{{"type":"window","status":false,"progress":{progress},
                    "progress_label":"Reverse",
                    "children":[{{"id":2,"type":"label","text":"take","h":20}}]}}"#
            );
            host.handle_packet(
                OscPacket::Message(OscMessage {
                    addr: crate::host::GUI_DEF.into(),
                    args: vec![OscType::Int(1), OscType::String(def)],
                }),
                crate::host::ClientId::Udp(std::net::SocketAddr::from((
                    std::net::Ipv4Addr::LOCALHOST,
                    9000,
                ))),
            );
            let tree = host.window_def(1).unwrap();
            let none = (HashMap::new(), HashMap::new());
            let inputs = FrameInputs {
                metrics: host.metrics_for(1),
                ..FrameInputs::default()
            };
            picture(tree, &inputs, &host.theme, &none.0, &none.1, (800, 600))
                .top
                .vertex_count()
        };
        let (idle, working) = (top(-1.0), top(0.5));
        assert!(
            working > idle + 4 * 6,
            "a panel over the window: {idle} -> {working} vertices"
        );
    }

    /// **The focus ring is over what moves**: a focused multitrack's live layer
    /// ends with the ring, so the meters drawn there -- a column the height of
    /// its track, at rest -- do not cover the ring where they reach the edge.
    #[test]
    fn the_focus_ring_is_drawn_over_the_live_layer() {
        use clausters_core::osc::{OscMessage, OscPacket, OscType};
        let mut host = crate::host::Host::new();
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: crate::host::GUI_DEF.into(),
                args: vec![OscType::Int(1), OscType::String(MOVING.into())],
            }),
            crate::host::ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
        let tree = host.window_def(1).unwrap();
        let none = (HashMap::new(), HashMap::new());
        let live = |focused: Option<i32>| {
            let inputs = FrameInputs {
                metrics: host.metrics_for(1),
                focused,
                ..FrameInputs::default()
            };
            picture(tree, &inputs, &host.theme, &none.0, &none.1, (800, 600))
                .live
                .vertex_count()
        };
        assert_eq!(
            live(Some(5)),
            live(None) + 4 * 6,
            "the four strips of the ring, last"
        );
    }

    /// **What a whole frame of the score editor costs**, the heaviest window
    /// this host draws: the toolbar's symbols, the palettes, the menus and a
    /// page of two staves, engraved by the real engraver. Prints the median
    /// of a whole frame's picture -- the layout, the meshes, no card -- which
    /// is what a set, a gesture or a resize asks for.
    ///
    /// Run with `cargo test --release --features score --lib
    /// what_a_whole_frame_costs -- --ignored --nocapture`.
    #[cfg(feature = "score")]
    #[test]
    #[ignore = "timing: only meaningful under --release"]
    fn what_a_whole_frame_costs() {
        use clausters_core::notation::{Item, Marks, Pitch, Sheet, Staff, Step, Voice};
        use clausters_core::ratio::Ratio;

        crate::host::font::atlas::set_embedded();
        let notes = |octave: i32| -> Vec<Item> {
            (0..128u64)
                .map(|i| Item::Note {
                    id: 0,
                    pitches: vec![Pitch {
                        step: [Step::C, Step::E, Step::G, Step::B][i as usize % 4],
                        alter: if i % 7 == 0 { 1 } else { 0 },
                        octave,
                        forced: false,
                    }],
                    dur: Ratio::new(1, 8),
                    tie: false,
                    marks: Marks::default(),
                })
                .collect()
        };
        let mut sheet = Sheet {
            staves: vec![
                Staff {
                    clef: "G2".into(),
                    voices: vec![Voice { items: notes(5) }],
                    ..Staff::default()
                },
                Staff {
                    clef: "F4".into(),
                    voices: vec![Voice { items: notes(3) }],
                    ..Staff::default()
                },
            ],
            ..Sheet::default()
        };
        let mut id = 1;
        for staff in &mut sheet.staves {
            for item in &mut staff.voices[0].items {
                if let Item::Note { id: at, .. } = item {
                    *at = id;
                    id += 1;
                }
            }
        }
        sheet.next_id = id;
        let mei = clausters_core::notation::sheet_to_mei(&sheet).unwrap();
        let score = clausters_notation::open(&mei, &clausters_notation::EngraveOptions::default())
            .expect("engraved");
        let mut host = crate::host::Host::new();
        host.owner = Some(crate::host::document::Owner::new(
            clausters_document::Document::empty(),
        ));
        let def = host
            .open_score(
                std::sync::Arc::new(std::sync::Mutex::new(score)),
                "score",
                (1280, 900),
                None,
            )
            .expect("a window");
        let tree = host.window_def(def).unwrap();
        let inputs = FrameInputs {
            metrics: host.metrics_for(def),
            world: World {
                timelines: host.timelines(),
                ..World::default()
            },
            ..FrameInputs::default()
        };
        let none = (HashMap::new(), HashMap::new());
        let draw = || picture(tree, &inputs, &host.theme, &none.0, &none.1, (1280, 900));
        let first = draw();
        let mut took: Vec<f64> = (0..30)
            .map(|_| {
                let start = std::time::Instant::now();
                std::hint::black_box(draw());
                start.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        took.sort_by(f64::total_cmp);
        let verts = |m: &Mesh| m.vertex_count();
        println!(
            "whole frame: {:.3} ms median, {:.3} ms best; {} + {} + {} vertices",
            took[took.len() / 2],
            took[0],
            verts(&first.base),
            verts(&first.over),
            verts(&first.top),
        );
    }

    /// **With a dialog up nothing is kept**, and the window's moving lines and
    /// its overlays stay in the batch under the one the scrim and the dialog
    /// are drawn in: a batch draws its glyphs after all its flat geometry, so
    /// sharing one let the window's text show through the dialog.
    #[test]
    fn a_window_with_a_dialog_up_keeps_no_picture() {
        let dialog = picture_at(
            r#"{"type":"window","children":[
                {"id":3,"type":"notes","notes":[0.0,50.0,60.0,100.0,0.0],"playhead_at":0},
                {"id":9,"type":"layout","modal":true,"title":"t","children":[
                    {"id":10,"type":"label","text":"x"}]}]}"#,
            0.0,
            0.0,
        );
        assert!(!dialog.kept);
        assert!(
            !dialog.over.is_empty(),
            "the window's lines are under the scrim"
        );
        assert!(
            !dialog.live.is_empty(),
            "the scrim and the dialog are in their own batch"
        );
    }

    /// **A frame is the least it may be and never less.** Asked for what
    /// moves alone it may keep the picture; asked for the whole by anyone, or
    /// by nobody of this host, it is whole -- and a tick asking beside a set
    /// does not take the set's frame away.
    #[test]
    fn a_frame_is_whole_unless_only_a_tick_asked_for_it() {
        let asked = Asked::default();
        assert!(asked.whole(), "nobody asked: the platform's frame is whole");
        asked.want_live();
        assert!(!asked.whole(), "a tick alone asks for what moves");
        asked.want_whole();
        asked.want_live();
        assert!(asked.whole(), "a set beside a tick is still drawn whole");
        asked.drawn();
        assert!(asked.whole(), "and the next one starts from nobody asking");
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

    /// **A marker is measured against the numbers it stands among.** It used
    /// to be a fixed nine pixels drawn in the size a roll's OSC flags use,
    /// which read as a speck beside the tick labels at one density and as a
    /// blob at another. The rule is the proportion, so this asserts it at
    /// every scale the metrics are generated at rather than a pixel count.
    #[test]
    fn a_markers_arrow_is_read_against_the_rulers_own_text() {
        for scale in [0.75, 1.0, 1.5, 2.0, 3.0] {
            let m = Metrics::default().at(scale);
            let w = draw::marker_w(&m);
            let cell = crate::host::font::advance(m.caption_scale);
            assert!(
                w >= cell,
                "an arrow narrower than one character of the row it stands on \
                 is the speck this replaced (scale {scale})"
            );
            assert!(
                w <= m.ruler_h,
                "and one wider than the strip is tall would cover the numbers \
                 rather than point at them (scale {scale})"
            );
        }
    }

    fn editor(ruler: Ruler, ruler_y: RulerY) -> EditorProps {
        EditorProps {
            // A test fixture's axis has no air: what it checks is placement,
            // not how far a hand may open the vertical.
            y_headroom: 1.0,
            y_measure: crate::host::widget::YMeasure::Value,
            y_parked: None,
            ruler,
            dir: crate::host::widget::RulerDir::Up,
            ruler_y,
            sample_rate: 0.0,
            bit_depth: 16,
            tempo: 1.0,
            tempo_map: None,
            beat_at: 0.0,
            quant: 4.0,
            grid: 0.0,
            grid_snap: true,
            autofit: true,
            sel_start: 0.0,
            sel_len: 0.0,
            x_start: 0.0,
            x_len: 1.0,
            playhead_at: -1.0,
            playhead: -1.0,
            cursor: -1.0,
            playhead_loop_start: 0.0,
            playhead_loop_len: 0.0,
            y_start: 0.0,
            y_len: 1.0,
            sel_min: 0.0,
            sel_max: 0.0,
            link: None,
            offset: 0.0,
            markers: Vec::new(),
            naming: None,
        }
    }

    /// The two paint props reach the frame through one door, and each in its
    /// own units: the opacity is already resolved (it composed down the tree at
    /// the mutation point), while the radius is a **logical** length that the
    /// placement's own table turns into pixels -- so a widget seen at a HiDPI
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
        // lane starts its clips -- the indent is the axis', not the widget's.
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
        // bucket -- which is what "zoomed out" means for this question.
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
            Some(Owed::Samples { a: 0, b: 4_000 }),
            "past the bucket over samples it cannot answer for: the view and \
             half a view past either edge"
        );

        // A run over the view alone is asked again: a pan would leave it
        // at once, so what is held reaches a quarter of a view past it.
        let mut data = WaveformData::with_multi_pyramid(
            clausters_core::peaks::MultiPyramid::empty(frames, 1, bucket),
        );
        assert!(data.set_window(1_000, 1, &vec![0.0; 2_000]));
        let edge = WaveformView::new(data);
        assert_eq!(
            owed(&edge, &close, 800.0, None),
            Some(Owed::Samples { a: 0, b: 4_000 })
        );

        // The same view once the run asked has arrived: nothing more is owed.
        let mut data = WaveformData::with_multi_pyramid(
            clausters_core::peaks::MultiPyramid::empty(frames, 1, bucket),
        );
        assert!(data.set_window(0, 1, &vec![0.0; 4_000]));
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

    /// **A texture over a summary asks its samples at any zoom**: zoomed
    /// out, where the trace's summary answers, the spectrogram still has
    /// nothing to analyze. What is held covers it once it lands, a view
    /// past [`TEXTURE_SPAN`] asks that much about its middle and is covered
    /// by it, and a view that owns its samples never asks.
    #[test]
    fn a_texture_over_a_summary_asks_its_samples_at_any_zoom() {
        use crate::waveform::WaveformData;
        let (bucket, frames) = (256usize, 256 * 4_000usize);
        let summary = || {
            WaveformData::with_multi_pyramid(clausters_core::peaks::MultiPyramid::empty(
                frames, 1, bucket,
            ))
        };
        let wide = View {
            start: 0.0,
            len: frames as f64,
        };
        let told = WaveformView::new(summary());
        assert_eq!(owed(&told, &wide, 800.0, None), None, "the trace's answer");
        assert_eq!(
            texture_owed(&told, &wide, None),
            Some(Owed::Run { a: 0, b: frames }),
            "the texture's: the whole take, which the view shows"
        );
        let mut data = summary();
        assert!(data.set_window(0, 1, &vec![0.0; frames]));
        assert_eq!(texture_owed(&WaveformView::new(data), &wide, None), None);

        let long = 4 * TEXTURE_SPAN;
        let all = View {
            start: 0.0,
            len: long as f64,
        };
        let told = WaveformView::new(WaveformData::with_multi_pyramid(
            clausters_core::peaks::MultiPyramid::empty(long, 1, bucket),
        ));
        let middle = long / 2 - TEXTURE_SPAN / 2;
        assert_eq!(
            texture_owed(&told, &all, None),
            Some(Owed::Run {
                a: middle,
                b: middle + TEXTURE_SPAN
            })
        );
        let mut data = WaveformData::with_multi_pyramid(
            clausters_core::peaks::MultiPyramid::empty(long, 1, bucket),
        );
        assert!(data.set_window(middle, 1, &vec![0.0; TEXTURE_SPAN]));
        assert_eq!(
            texture_owed(&WaveformView::new(data), &all, None),
            None,
            "the run asked is the run that answers: nothing asked again"
        );

        let owned = WaveformView::new(WaveformData::from_interleaved(
            &vec![0.0; frames],
            1,
            bucket,
        ));
        assert_eq!(texture_owed(&owned, &wide, None), None);
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
            Some(Owed::Samples { a: 0, b: 4_000 }),
            "wholly behind the frontier: the same span a finished take asks for"
        );
        assert_eq!(
            owed(&told, &close, 800.0, Some(2_000)),
            Some(Owed::Samples { a: 0, b: 2_000 }),
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
                b: 76_800,
                bucket: 32,
                seen: (0, 51_200),
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
            Some(Owed::Samples { a: 0, b: 19_200 }),
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
        let long = DETAIL_GRID_BUCKETS * 64;
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
    fn channel_at_picks_the_channel_under_the_cursor() {
        let body = Rect::new(0.0, 0.0, 400.0, 300.0);
        assert_eq!(channel_at(body, 3, 50.0), 0);
        assert_eq!(channel_at(body, 3, 150.0), 1);
        assert_eq!(channel_at(body, 3, 299.0), 2);
        assert_eq!(channel_at(body, 3, 1000.0), 2, "clamped");
    }

    #[test]
    fn channels_split_the_body_evenly_and_share_x() {
        let body = Rect::new(0.0, 0.0, 400.0, 300.0);
        let a = channel_rect(body, 3, 0);
        let b = channel_rect(body, 3, 1);
        let c = channel_rect(body, 3, 2);
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

    /// A channel is analyzed at the hop asked for, however many textures
    /// wide that makes it: what bounds a transform is the memory it takes
    /// (`spectrogram::hop_capped`), and the card is shown a window of it.
    #[test]
    fn stft_channels_keep_the_hop_past_a_textures_width() {
        let n = 200_000;
        let chan: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin()).collect();
        let stacks = stft_channels(vec![chan], (256, 8, 0), 48_000.0);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].hop(), 8);
        assert!(stacks[0].n_frames() > crate::spectrogram::MAX_FRAMES);
        assert!(stacks[0].n_frames() <= crate::spectrogram::max_frames(256));
        assert_eq!(stacks[0].total_samples(), n);
    }
}
