//! **What a window keeps of a view's data**: the slot a waveform's samples
//! and a spectrogram's analysis hang on, what is carried across a redefine,
//! and how a take that arrives is placed into the slot that waits for it.

use super::*;

/// A waveform widget's data and vertical navigation state. Its horizontal
/// window lives in the widget's timeline group ([`crate::host::timeline`]), not here
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
///   column (`/buffer_summary`, folded by [`WaveformData::set_detail`]). A few
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
pub(super) fn keep_overviews(slot: &mut SpectrogramSlot, mut held: SpectrogramSlot) {
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
    let own = |at: &SlotAt| at.1 == crate::host::widget::element::SlotKey::SELF;
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
pub(super) fn roll_into_slot(
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
/// pictures it is ([`SlotKey`](crate::host::widget::element::SlotKey)).
///
/// A pair rather than an id because a view may hold several: a multitrack's
/// boxes are windows onto several takes, and a time-frequency box samples a
/// texture of its own.
pub(crate) type SlotAt = (i32, crate::host::widget::element::SlotKey);

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
