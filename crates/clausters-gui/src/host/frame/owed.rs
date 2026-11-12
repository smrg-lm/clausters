//! **What a view owes at the zoom it is shown at**: the span of samples, the
//! texture or the columns it has not got yet for the window it draws, so a
//! front asks for exactly that and nothing finer than the screen.

use super::*;

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
pub(super) fn owed(
    view: &WaveformView,
    nav: &View,
    width: f64,
    written: Option<u64>,
) -> Option<Owed> {
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
/// [`owed()`] asks it only past the summary's bucket, and analyzes that run
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

/// **The picture a list's spectrogram starts from**: one dark column per
/// channel over the whole take of `frames`, analyzed at `window_size` and
/// `sample_rate`, which a first draw finds too coarse and asks columns past.
pub(crate) fn dark_columns(
    window_size: usize,
    sample_rate: f32,
    frames: usize,
    channels: usize,
) -> Vec<crate::spectrogram::Stft> {
    let bins = window_size / 2;
    (0..channels.max(1))
        .filter_map(|_| {
            crate::spectrogram::Stft::of_span(
                vec![0.0; bins],
                bins,
                0,
                frames.max(1),
                window_size,
                sample_rate,
                frames,
            )
        })
        .collect()
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
pub(super) fn power_hop(per_px: f64) -> usize {
    let at_least = per_px.floor().max(1.0) as usize;
    1 << (usize::BITS - 1 - at_least.leading_zeros())
}

/// **How far past either edge of a view what it is owed reaches**, in views.
pub(super) const OWED_REACH: f64 = 0.5;

/// **How close to the edge of what it holds a view may come** before the
/// next span is asked, in views: under [`OWED_REACH`], so the next span is on
/// its way while the one held still covers what is on screen.
pub(super) const OWED_NEAR: f64 = 0.25;

/// **How many buckets one detail grid may hold**: what bounds the span one
/// grid summarizes. A grid comes over as many replies as it needs (a reply is
/// 8 KiB, `docs/schemas.md`) and is handed to the view whole, so this is a
/// bound on the grid's cost -- two dozen replies of stereo at most from a
/// server, one from the host's own answer over a list -- and not on one
/// reply. It holds a span two views wide ([`OWED_REACH`]) at two buckets a
/// column on a window a few thousand pixels wide.
pub(super) const DETAIL_GRID_BUCKETS: usize = 8192;

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
pub(super) fn detail_bucket(per_px: f64, span: usize, base: usize) -> Option<usize> {
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
pub(super) const MIN_DETAIL_BUCKET: usize = 16;
