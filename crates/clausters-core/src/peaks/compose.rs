//! **A segment list's summary, composed from its sources'**: the statistics
//! of any span of a list, answered from what each part's source already
//! keeps.
//!
//! A list owns no samples: each part is a span of a source, so the summary of
//! a span of the list is each part's share of it, folded. Between its fades a
//! part *is* its source, and its share is the source's own summary over the
//! frames it reads; a fade is the list's own samples (the source's times a
//! gain), so it is read; and a channel the part maps to nothing is silence.
//! An edit makes a new list over the same sources, and what that costs the
//! summary is the fades and the edges off the sources' grid: the rest is
//! levels already kept.
//!
//! The server composes a join over pool buffers this way, and the GUI host a
//! list over files it reads itself, so the walk is written once here and each
//! supplies its sources ([`PartSamples`]). A reader whose samples arrive later
//! -- a page's files, read with promises -- notes what it was asked for and
//! did not hold, brings it, and answers again.

use crate::segments::PartSpan;

use super::SpanStats;

/// **Where a list's parts find their samples**: the one thing that differs
/// between the server's pool buffers and the host's files.
pub trait PartSamples<P> {
    /// The source's summary of its channel `src_ch` over its frames `a..b`,
    /// growing `read` by the samples read to answer it.
    ///
    /// `ends` says which end of the span is one of the part's own edges -- a
    /// cut, or where a fade meets the body -- rather than the edge of the span
    /// asked. A source that answers a coarse span approximately (its edges
    /// rounded to a grid it keeps) still answers a cut exactly, since past a
    /// cut is material the list does not hold.
    fn summary(
        &mut self,
        part: &P,
        src_ch: usize,
        a: usize,
        b: usize,
        ends: Ends,
        read: &mut usize,
    ) -> SpanStats;

    /// One frame of the source, on its channel `src_ch`: what a fade is read
    /// from.
    fn frame(&mut self, part: &P, frame: usize, src_ch: usize) -> f32;
}

/// Which ends of a span handed to [`PartSamples::summary`] are the part's own
/// edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ends {
    pub start: bool,
    pub end: bool,
}

/// **The statistics of the list's `channel` over its frames `a..b`**, from
/// `parts` placed in order (each one's [`PartSpan`] says where it lands).
pub fn list_span<P: AsRef<PartSpan>>(
    parts: &[P],
    samples: &mut impl PartSamples<P>,
    channel: usize,
    a: usize,
    b: usize,
    read: &mut usize,
) -> SpanStats {
    let mut stats = SpanStats::EMPTY;
    let from = parts.partition_point(|p| p.as_ref().end() <= a);
    for part in parts[from..].iter().take_while(|p| p.as_ref().start() < b) {
        let span = part.as_ref();
        let (ia, ib) = (
            a.max(span.start()) - span.start(),
            b.min(span.end()) - span.start(),
        );
        stats.merge(part_span(part, samples, channel, ia, ib, read));
    }
    stats
}

/// One part's frames `ia..ib`, counted from its start, on the list's
/// `channel`.
fn part_span<P: AsRef<PartSpan>>(
    part: &P,
    samples: &mut impl PartSamples<P>,
    channel: usize,
    ia: usize,
    ib: usize,
    read: &mut usize,
) -> SpanStats {
    let span = part.as_ref();
    let src_ch = match span.map.get(channel) {
        Some(&ch) if ch >= 0 => ch as usize,
        _ => return SpanStats::silence(ib - ia),
    };
    let mut stats = SpanStats::EMPTY;
    for (from, to, faded) in pieces(span) {
        let (lo, hi) = (from.max(ia), to.min(ib));
        if lo >= hi {
            continue;
        }
        let ends = Ends {
            start: lo == from,
            end: hi == to,
        };
        if faded {
            let values: Vec<f32> = (lo..hi)
                .map(|i| span.read_with(i, channel, |f, c| samples.frame(part, f, c)))
                .collect();
            *read += hi - lo;
            stats.merge(SpanStats::of(&values));
        } else {
            stats.merge(body_span(part, samples, src_ch, (lo, hi), ends, read));
        }
    }
    stats
}

/// A part's three pieces, counted from its start: the fade in, the body and
/// the fade out, each with whether it is faded.
fn pieces(span: &PartSpan) -> [(usize, usize, bool); 3] {
    let body = (span.fade_in, span.frames - span.fade_out);
    [
        (0, body.0, true),
        (body.0, body.1, false),
        (body.1, span.frames, true),
    ]
}

/// The source frames a body's frames `lo..hi` are summarized over: frame for
/// frame at the list's own rate, and at another the source frames it reads
/// between.
fn body_frames(span: &PartSpan, lo: usize, hi: usize) -> (usize, usize) {
    if !span.resampled() {
        return (span.src_start + lo, span.src_start + hi);
    }
    let first = (span.src_start as f64 + lo as f64 * span.rate).floor() as usize;
    let last = (span.src_start as f64 + (hi - 1) as f64 * span.rate).ceil() as usize;
    (first, (last + 1).min(span.src_frames()))
}

/// The source's summary under a body's frames `lo..hi`.
///
/// At the list's own rate this is the source's span, exact. At another a part
/// reads between two source frames, and a line between two samples never
/// leaves them: the extremes are the source's over the frames it reads
/// between, which bound the list's, and the energy is the source's over that
/// span, taken as the list's mean.
fn body_span<P: AsRef<PartSpan>>(
    part: &P,
    samples: &mut impl PartSamples<P>,
    src_ch: usize,
    (lo, hi): (usize, usize),
    ends: Ends,
    read: &mut usize,
) -> SpanStats {
    let span = part.as_ref();
    let (a, b) = body_frames(span, lo, hi);
    let stats = samples.summary(part, src_ch, a, b, ends, read);
    if !span.resampled() {
        return stats;
    }
    if stats.count == 0 {
        return SpanStats::silence(hi - lo);
    }
    let ms = stats.sum_sq / stats.count as f64;
    SpanStats {
        sum_sq: ms * (hi - lo) as f64,
        count: hi - lo,
        ..stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peaks::{Interleaved, Kept};
    use crate::segments::{Cut, FadeShape, part_at, place};

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed.max(1);
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    /// Sources as interleaved samples, each with the summary kept beside it.
    struct Takes {
        data: Vec<(Vec<f32>, usize)>,
        kept: Vec<Kept>,
    }

    impl Takes {
        fn new(data: Vec<(Vec<f32>, usize)>) -> Self {
            let kept = data
                .iter()
                .map(|(d, ch)| Kept::unmeasured(d.len() / ch, *ch, 256))
                .collect();
            Self { data, kept }
        }
    }

    /// A part and the source it reads.
    struct Over(usize, PartSpan);

    impl AsRef<PartSpan> for Over {
        fn as_ref(&self) -> &PartSpan {
            &self.1
        }
    }

    impl PartSamples<Over> for Takes {
        fn summary(
            &mut self,
            part: &Over,
            ch: usize,
            a: usize,
            b: usize,
            _ends: Ends,
            read: &mut usize,
        ) -> SpanStats {
            let (data, channels) = &self.data[part.0];
            let views: Vec<_> = (0..*channels)
                .map(|c| Interleaved::new(data, *channels, c))
                .collect();
            let sources: Vec<_> = views.iter().collect();
            self.kept[part.0].span(&sources, ch, a, b, read)
        }

        fn frame(&mut self, part: &Over, frame: usize, ch: usize) -> f32 {
            let (data, channels) = &self.data[part.0];
            data[frame * channels + ch]
        }
    }

    fn cut(src_start: usize, frames: usize, map: Vec<i32>) -> Cut {
        Cut {
            src_start,
            frames,
            fade_in: 0,
            fade_out: 0,
            fade_in_shape: FadeShape::LINEAR,
            fade_out_shape: FadeShape::LINEAR,
            map,
        }
    }

    /// **A list answers what its samples hold, bucket for bucket**: two
    /// sources, cuts off the grid, shaped fades, a map that swaps and
    /// silences, and a part at another rate.
    #[test]
    fn a_list_composes_its_summary_from_its_sources() {
        let a = (noise(20_000 * 2, 5), 2);
        let b = (noise(9_000, 9), 1);
        let mut faded = cut(4_321, 6_000, vec![0, -1]);
        faded.fade_in = 300;
        faded.fade_out = 517;
        faded.fade_out_shape = FadeShape {
            shape: crate::envshape::SHAPE_CURVE,
            curve: -3.0,
        };
        let (spans, frames) = place([
            (cut(100, 1_234, vec![0, 1]), 20_000, 1.0),
            (faded, 20_000, 1.0),
            (cut(13, 2_500, vec![0, 0]), 9_000, 1.0),
            (cut(50, 3_000, vec![1, 0]), 20_000, 0.75),
        ]);
        let parts: Vec<Over> = spans
            .into_iter()
            .zip([0, 0, 1, 0])
            .map(|(s, src)| Over(src, s))
            .collect();
        let mut takes = Takes::new(vec![a, b]);
        let sample = |takes: &mut Takes, f: usize, ch: usize| {
            let p = &parts[part_at(&parts, f).unwrap()];
            p.1.read_with(f - p.1.start(), ch, |sf, sc| takes.frame(p, sf, sc))
        };
        for bucket in [256, 1_024, 4_096] {
            for i in 0..frames / bucket {
                for ch in 0..2 {
                    let (s, e) = (i * bucket, (i + 1) * bucket);
                    let mut read = 0;
                    let got = list_span(&parts, &mut takes, ch, s, e, &mut read);
                    let direct: Vec<f32> = (s..e).map(|f| sample(&mut takes, f, ch)).collect();
                    let want = SpanStats::of(&direct);
                    assert_eq!(got.count, want.count, "{bucket} {i} {ch}");
                    if parts
                        .iter()
                        .any(|p| p.1.resampled() && p.1.start() < e && p.1.end() > s)
                    {
                        // At another rate the extremes bound the samples'.
                        assert!(got.min <= want.min && got.max >= want.max);
                        continue;
                    }
                    assert_eq!(
                        (got.min, got.max),
                        (want.min, want.max),
                        "{bucket} {i} {ch}"
                    );
                    let (g, w) = (got.bucket()[2], want.bucket()[2]);
                    assert!(
                        (g - w).abs() <= 1e-5 * w.abs().max(1e-3),
                        "{bucket} {i} {ch}"
                    );
                }
            }
        }
    }
}
