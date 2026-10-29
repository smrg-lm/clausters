//! **One region of a multitrack, as arithmetic**: which frame of its source
//! a frame of its track reads, through which channel, at which rate and under
//! which fades and level.
//!
//! A region is a window onto a source placed on a track: it starts at a
//! position, lasts a length, reads its source from a second of it at a
//! playrate, may wrap around the source, fades in and out at its own edges,
//! and its level follows its own gain curve. A track is read by summing the
//! regions that cross each of its frames, and what a region *does* to the
//! frames it reads is written here once, for every reader of a track: the
//! application's take reader, a drawing of it, and a test that compares the
//! two. Each reader supplies only where a source frame comes from
//! ([`RegionSpan::read_with`]), as a part of a list does
//! ([`crate::segments::PartSpan`]).
//!
//! # The multitrack's own edges
//!
//! A region's fades are the multitrack's, not a part's: each is capped at the
//! region's span and not at half of it, two fades that meet multiply, and a
//! fade of `n` frames is `(p + 1) / max(n, 1)` of the way through its
//! segment at `p` frames into it, read through an envelope segment's shape
//! ([`crate::envshape`]). That is what a region has always sounded like.
//!
//! # The implicit map
//!
//! A region plays its source through the implicit conversion
//! ([`crate::channel_matrix::ChannelMatrix::implicit`]): a mono source on
//! every channel of its track at unity, a wider one channel by channel. Where
//! a track sits in the master is the track's strip's to say, not the region's.

use std::sync::Arc;

use crate::envshape::shape_value;
use crate::resample::Interpolator;
use crate::segments::{FadeShape, channel_map};

/// **A curve over a region, on its track's frames**: one value every `step`
/// frames from frame `at`, holding its ends -- what the region's gain follows.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    /// Where the first value sits, in frames of the track.
    pub at: f64,
    /// How many frames one value covers.
    pub step: f64,
    /// The values.
    pub table: Arc<[f32]>,
}

impl Curve {
    /// The curve at frame `k` of the track: the line between the two values
    /// it falls between, the first before it and the last after it.
    #[inline]
    pub fn at(&self, k: f64) -> f32 {
        let Some((&first, rest)) = self.table.split_first() else {
            return 1.0;
        };
        if rest.is_empty() || self.step <= 0.0 {
            return first;
        }
        let phase = ((k - self.at) / self.step).max(0.0);
        let i = phase as usize;
        if i + 1 >= self.table.len() {
            return self.table[self.table.len() - 1];
        }
        let frac = (phase - i as f64) as f32;
        self.table[i] + frac * (self.table[i + 1] - self.table[i])
    }
}

/// **What a region reads, before its source's length is known**: its place
/// on the track, its window onto its source, its edges and its level.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionCut {
    /// Where it begins on the track, in frames of the track.
    pub at: f64,
    /// How long it sounds, in frames of the track.
    pub span: f64,
    /// The frame of its source its own zero reads.
    pub start: f64,
    /// **Frames of the source per frame of the track**: the source's rate
    /// over the track's, times the region's playrate.
    pub step: f64,
    /// Whether it wraps around the end of its source.
    pub looping: bool,
    /// Its fade in and fade out, in frames of the track, each at most its
    /// span.
    pub fade_in: f64,
    pub fade_out: f64,
    /// The shape each fade is drawn with.
    pub fade_in_shape: FadeShape,
    pub fade_out_shape: FadeShape,
    /// Track channel -> source channel; a negative entry is silence.
    pub map: Vec<i32>,
    /// Its gain curve, when it has one; unity otherwise.
    pub gain: Option<Curve>,
}

impl RegionCut {
    /// The implicit map of a source `source` channels wide onto a track
    /// `track` channels wide ([`channel_map`]).
    pub fn implicit_map(source: usize, track: usize) -> Vec<i32> {
        channel_map(None, source, track)
    }
}

/// **A region over a source of a known length**: the cut, and the frames of
/// the track it sounds on.
#[derive(Clone, Debug)]
pub struct RegionSpan {
    cut: RegionCut,
    src_frames: usize,
    first: u64,
    end: u64,
}

impl RegionSpan {
    /// `cut` over a source of `src_frames` frames.
    pub fn new(cut: RegionCut, src_frames: usize) -> RegionSpan {
        let first = cut.at.max(0.0).ceil() as u64;
        let end = (cut.at + cut.span.max(0.0)).max(0.0).ceil() as u64;
        RegionSpan {
            cut,
            src_frames,
            first,
            end: end.max(first),
        }
    }

    /// The cut it reads.
    pub fn cut(&self) -> &RegionCut {
        &self.cut
    }

    /// The first frame of the track it sounds on.
    #[inline]
    pub fn first(&self) -> u64 {
        self.first
    }

    /// One past the last.
    #[inline]
    pub fn end(&self) -> u64 {
        self.end
    }

    /// How many frames its source has.
    #[inline]
    pub fn src_frames(&self) -> usize {
        self.src_frames
    }

    /// **The source position frame `k` of the track reads**, wrapped around
    /// the source when the region loops and held inside it otherwise.
    #[inline]
    pub fn position(&self, k: u64) -> f64 {
        let raw = (k as f64 - self.cut.at) * self.cut.step + self.cut.start;
        let frames = self.src_frames as f64;
        if frames <= 0.0 {
            0.0
        } else if self.cut.looping {
            raw.rem_euclid(frames)
        } else {
            raw.clamp(0.0, frames - 1.0)
        }
    }

    /// **The level at frame `k` of the track**: the fade in, the fade out
    /// and the gain curve; silence outside the region.
    #[inline]
    pub fn gain(&self, k: u64) -> f32 {
        if k < self.first || k >= self.end {
            return 0.0;
        }
        let p = k as f64 - self.cut.at;
        let mut gain = 1.0f32;
        if self.cut.fade_in > 0.0 {
            let t = ((p + 1.0) / self.cut.fade_in.max(1.0)).clamp(0.0, 1.0) as f32;
            let FadeShape { shape, curve } = self.cut.fade_in_shape;
            gain *= shape_value(shape, curve, 0.0, 1.0, t);
        }
        if self.cut.fade_out > 0.0 {
            let left = (self.cut.span - p) / self.cut.fade_out.max(1.0);
            let t = (1.0 - left).clamp(0.0, 1.0) as f32;
            let FadeShape { shape, curve } = self.cut.fade_out_shape;
            gain *= shape_value(shape, curve, 1.0, 0.0, t);
        }
        if let Some(curve) = &self.cut.gain {
            gain *= curve.at(k as f64);
        }
        gain
    }

    /// **One sample of the region at frame `k` of the track**, channel
    /// `channel` of it, read through `frame` (the source's frame and channel
    /// in, its sample out): between two source frames the line between,
    /// the reading every reader of a buffer makes, the upper frame wrapping
    /// to the first when the region loops.
    #[inline]
    pub fn read_with(
        &self,
        k: u64,
        channel: usize,
        mut frame: impl FnMut(usize, usize) -> f32,
    ) -> f32 {
        let gain = self.gain(k);
        if gain == 0.0 {
            return 0.0;
        }
        let Some(&src_ch) = self.cut.map.get(channel) else {
            return 0.0;
        };
        if src_ch < 0 || self.src_frames == 0 {
            return 0.0;
        }
        let src_ch = src_ch as usize;
        let pos = self.position(k);
        let f0 = (pos as usize).min(self.src_frames - 1);
        let frac = pos - f0 as f64;
        let value = if frac == 0.0 {
            frame(f0, src_ch)
        } else {
            let f1 = if f0 + 1 < self.src_frames {
                f0 + 1
            } else if self.cut.looping {
                0
            } else {
                f0
            };
            Interpolator::played(frame(f0, src_ch), frame(f1, src_ch), frac)
        };
        value * gain
    }

    /// **Where a run of the track from `k` reads its source without
    /// wrapping**: the first frame at or after `k`, before `to`, whose
    /// position wraps around the source -- `to` when none does. A reader
    /// fetches a run's source frames in one read ([`Self::source_span`]).
    pub fn run_end(&self, k: u64, to: u64) -> u64 {
        if !self.cut.looping || self.src_frames == 0 || self.cut.step <= 0.0 || k >= to {
            return to;
        }
        let frames = self.src_frames as f64;
        let raw = (k as f64 - self.cut.at) * self.cut.step + self.cut.start;
        let next = ((raw / frames).floor() + 1.0) * frames;
        let at = self.cut.at + (next - self.cut.start) / self.cut.step;
        (at.ceil().max(k as f64 + 1.0) as u64).min(to)
    }

    /// **The source frames frames `lo..hi` of the track read**, as a
    /// half-open range, for a run that does not wrap ([`Self::run_end`]):
    /// one more past the last for the line between two frames, never past
    /// the source's end.
    pub fn source_span(&self, lo: u64, hi: u64) -> (usize, usize) {
        if hi <= lo || self.src_frames == 0 {
            return (0, 0);
        }
        let a = self.position(lo);
        let b = self.position(hi - 1);
        let (first, last) = if a <= b { (a, b) } else { (b, a) };
        let first = first as usize;
        let last = (last as usize + 2).min(self.src_frames);
        (first.min(self.src_frames), last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envshape::SHAPE_LINEAR;

    fn cut(at: f64, span: f64) -> RegionCut {
        RegionCut {
            at,
            span,
            start: 0.0,
            step: 1.0,
            looping: false,
            fade_in: 0.0,
            fade_out: 0.0,
            fade_in_shape: FadeShape::LINEAR,
            fade_out_shape: FadeShape::LINEAR,
            map: vec![0],
            gain: None,
        }
    }

    /// A region sounds from its position for its span, and reads its
    /// source from its start, one frame a frame.
    #[test]
    fn a_region_reads_its_window_where_it_sits() {
        let mut c = cut(10.0, 5.0);
        c.start = 3.0;
        let region = RegionSpan::new(c, 100);
        let src = |f: usize, _| f as f32;
        assert_eq!((region.first(), region.end()), (10, 15));
        assert_eq!(region.read_with(9, 0, src), 0.0);
        assert_eq!(region.read_with(10, 0, src), 3.0);
        assert_eq!(region.read_with(14, 0, src), 7.0);
        assert_eq!(region.read_with(15, 0, src), 0.0);
    }

    /// The edges are the multitrack's: `(p + 1) / n` into a fade in, and a
    /// fade out that ends at the span.
    #[test]
    fn the_edges_follow_the_multitrack_s_rule() {
        let mut c = cut(0.0, 8.0);
        c.fade_in = 4.0;
        c.fade_out = 4.0;
        c.fade_in_shape = FadeShape {
            shape: SHAPE_LINEAR,
            curve: 0.0,
        };
        let region = RegionSpan::new(c, 100);
        assert!((region.gain(0) - 0.25).abs() < 1e-6);
        assert!((region.gain(3) - 1.0).abs() < 1e-6);
        // p = 7: 1 - (8 - 7) / 4 = 0.75 of the way to silence
        assert!((region.gain(7) - 0.25).abs() < 1e-6);
    }

    /// A playrate reads between frames; a loop wraps around the source and
    /// a run ends where it does.
    #[test]
    fn a_region_at_another_rate_and_a_loop() {
        let mut c = cut(0.0, 20.0);
        c.step = 0.5;
        let slow = RegionSpan::new(c, 100);
        assert_eq!(slow.read_with(3, 0, |f, _| f as f32), 1.5);
        let mut c = cut(0.0, 20.0);
        c.looping = true;
        c.start = 6.0;
        let looped = RegionSpan::new(c, 8);
        assert_eq!(looped.read_with(1, 0, |f, _| f as f32), 7.0);
        assert_eq!(looped.read_with(2, 0, |f, _| f as f32), 0.0);
        assert_eq!(looped.run_end(0, 20), 2);
        assert_eq!(looped.run_end(2, 20), 10);
        assert_eq!(looped.source_span(2, 10), (0, 8));
    }

    /// A mono source sounds on every channel of its track at unity.
    #[test]
    fn a_mono_region_is_on_every_channel() {
        let mut c = cut(0.0, 4.0);
        c.map = RegionCut::implicit_map(1, 2);
        let region = RegionSpan::new(c, 10);
        assert_eq!(region.read_with(1, 0, |f, _| f as f32), 1.0);
        assert_eq!(region.read_with(1, 1, |f, _| f as f32), 1.0);
    }

    /// The gain curve holds its ends and is a line between its values.
    #[test]
    fn the_gain_curve_is_read_between_its_values() {
        let curve = Curve {
            at: 0.0,
            step: 4.0,
            table: Arc::from(vec![0.0f32, 1.0]),
        };
        assert_eq!(curve.at(-3.0), 0.0);
        assert_eq!(curve.at(2.0), 0.5);
        assert_eq!(curve.at(9.0), 1.0);
    }
}
