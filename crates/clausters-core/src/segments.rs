//! **One part of a segment list, as arithmetic**: which source frame a frame
//! of the list reads, through which channel, at which rate and under which
//! fade.
//!
//! A segment list is a run of parts, each a span of some source with its own
//! channel map and its own edit fades, and it is read in two places that do
//! not share their samples: the server reads parts over pool buffers and over
//! files, and the GUI host draws parts over files it reads itself. What a part
//! *does* to the frames it reads is the same in both, so it is written here
//! once and each reader supplies only where a source frame comes from
//! ([`PartSpan::read_with`]). Nothing here holds a source.

use crate::channel_matrix::ChannelMatrix;
use crate::envshape::{SHAPE_LINEAR, shape_value};

/// **The shape of a part's fade**: an envelope segment's shape number
/// ([`crate::envshape`]) and its curvature, which only the `curve` shape
/// reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FadeShape {
    pub shape: i32,
    pub curve: f32,
}

impl FadeShape {
    /// A straight ramp, which is what a fade that names no shape is.
    pub const LINEAR: Self = Self {
        shape: SHAPE_LINEAR,
        curve: 0.0,
    };
}

impl Default for FadeShape {
    fn default() -> Self {
        Self::LINEAR
    }
}

/// What a part cuts from its source, before its place in the list is known.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    /// The first frame read from the source.
    pub src_start: usize,
    /// How many frames the part contributes **to the list**.
    pub frames: usize,
    pub fade_in: usize,
    pub fade_out: usize,
    pub fade_in_shape: FadeShape,
    pub fade_out_shape: FadeShape,
    /// List channel -> source channel; a negative entry is silence.
    pub map: Vec<i32>,
}

/// **Frames of the source per frame of the list**: the ratio of the two
/// rates, or `None` when either is not a positive number.
pub fn rate_between(source_rate: f64, list_rate: f64) -> Option<f64> {
    (source_rate > 0.0 && list_rate > 0.0).then(|| source_rate / list_rate)
}

/// **A part placed in its list**: the cut, its place, and what a read of it
/// needs computed once.
#[derive(Clone, Debug)]
pub struct PartSpan {
    /// The first frame read from the source.
    pub src_start: usize,
    /// How many frames this part contributes **to the list**.
    pub frames: usize,
    /// **Frames of the source per frame of the list**: `1.0` for a part whose
    /// samples were written at the list's own rate, the ratio of the two
    /// otherwise. The span a part reads from its source is `frames * rate`.
    pub rate: f64,
    /// A fade over this many frames at the part's start, and at its end:
    /// the few milliseconds an editor puts on a cut so that two spans that do
    /// not continue each other make no step.
    pub fade_in: usize,
    pub fade_out: usize,
    /// The shape each fade is drawn with.
    pub fade_in_shape: FadeShape,
    pub fade_out_shape: FadeShape,
    /// List channel -> source channel, one entry per channel of the list. A
    /// negative entry is silence. It is routing and not level.
    pub map: Box<[i32]>,
    /// `1 / (fade + 1)` for each side, computed once: a fade is a multiply per
    /// sample rather than two conversions and a divide.
    fade_in_step: f32,
    fade_out_step: f32,
    /// Whether either fade is set at all; the common part has neither.
    faded: bool,
    /// Whether [`rate`](Self::rate) is anything but one, chosen once rather
    /// than by a float test per sample.
    resampled: bool,
    /// How many frames the source has, which bounds a read at another rate.
    src_frames: usize,
    /// The first frame of this part **in the list**.
    start: usize,
}

impl PartSpan {
    /// Places `cut` at frame `start` of its list, over a source of
    /// `src_frames` frames read at `rate` ([`rate_between`]). Each fade is
    /// capped at half the part: two fades that overlapped would multiply into
    /// a notch, so the worst case is a triangle.
    pub fn new(cut: Cut, src_frames: usize, rate: f64, start: usize) -> Self {
        let half = cut.frames / 2;
        let (fade_in, fade_out) = (cut.fade_in.min(half), cut.fade_out.min(half));
        Self {
            src_start: cut.src_start,
            frames: cut.frames,
            rate,
            fade_in,
            fade_out,
            fade_in_shape: cut.fade_in_shape,
            fade_out_shape: cut.fade_out_shape,
            map: cut.map.into_boxed_slice(),
            fade_in_step: 1.0 / (fade_in + 1) as f32,
            fade_out_step: 1.0 / (fade_out + 1) as f32,
            faded: fade_in > 0 || fade_out > 0,
            resampled: (rate - 1.0).abs() > f64::EPSILON,
            src_frames,
            start,
        }
    }

    /// **One sample of this part, read through `frame`**: the source's frame
    /// and channel in, its sample out. The map, the rate and the fades are
    /// this part's whoever holds the samples, which is what lets a part over
    /// a buffer and a part over a file be one logic.
    ///
    /// **A part at another rate** lands between two of its source's frames
    /// and reads the line between, the reading `PlayBuf` and `BufRd` make.
    /// It is bounded here, because the last frame of a part whose span does
    /// not divide evenly lands a fraction past its source.
    #[inline]
    pub fn read_with(
        &self,
        inner: usize,
        channel: usize,
        mut frame: impl FnMut(usize, usize) -> f32,
    ) -> f32 {
        let Some(&src_ch) = self.map.get(channel) else {
            return 0.0;
        };
        if src_ch < 0 {
            return 0.0;
        }
        let src_ch = src_ch as usize;
        let value = if self.resampled {
            let last = self.src_frames.saturating_sub(1);
            let pos = self.src_start as f64 + inner as f64 * self.rate;
            let f0 = (pos as usize).min(last);
            let frac = pos - f0 as f64;
            crate::resample::Interpolator::played(
                frame(f0, src_ch),
                frame((f0 + 1).min(last), src_ch),
                frac,
            )
        } else {
            frame(self.src_start + inner, src_ch)
        };
        if self.faded {
            value * self.gain(inner)
        } else {
            value
        }
    }

    /// **The fade's gain `inner` frames into the part**: one outside both
    /// fades.
    #[inline]
    pub fn gain(&self, inner: usize) -> f32 {
        let mut gain = 1.0f32;
        if inner < self.fade_in {
            let t = (inner + 1) as f32 * self.fade_in_step;
            let FadeShape { shape, curve } = self.fade_in_shape;
            gain *= shape_value(shape, curve, 0.0, 1.0, t);
        }
        let from_end = self.frames - 1 - inner;
        if from_end < self.fade_out {
            // The segment from full level to silence, `1 - t` of the way
            // along: linear, it is the ramp it always was.
            let t = (from_end + 1) as f32 * self.fade_out_step;
            let FadeShape { shape, curve } = self.fade_out_shape;
            gain *= shape_value(shape, curve, 1.0, 0.0, 1.0 - t);
        }
        gain
    }

    /// **The source frames `inner` frames `lo..hi` of this part read**, as a
    /// half-open range: what a reader that fetches its samples has to fetch
    /// so that [`PartSpan::read_with`] finds every frame it asks for. One more
    /// than the span at another rate, for the second frame of the last
    /// interpolation; never past the source's end.
    pub fn source_span(&self, lo: usize, hi: usize) -> (usize, usize) {
        let frames = self.src_frames;
        if hi <= lo {
            return (self.src_start, self.src_start);
        }
        if !self.resampled {
            return (self.src_start + lo, (self.src_start + hi).min(frames));
        }
        let first = (self.src_start as f64 + lo as f64 * self.rate) as usize;
        let last = (self.src_start as f64 + (hi - 1) as f64 * self.rate) as usize + 2;
        (first.min(frames), last.min(frames))
    }

    /// The first frame of this part **in the list**.
    #[inline]
    pub fn start(&self) -> usize {
        self.start
    }

    /// One past its last frame there.
    #[inline]
    pub fn end(&self) -> usize {
        self.start + self.frames
    }

    /// Whether it reads its source at another rate than the list's.
    #[inline]
    pub fn resampled(&self) -> bool {
        self.resampled
    }

    /// Whether either fade is set.
    #[inline]
    pub fn faded(&self) -> bool {
        self.faded
    }

    /// How many frames its source has.
    #[inline]
    pub fn src_frames(&self) -> usize {
        self.src_frames
    }
}

/// **Which channel of its file each channel of the list reads**, the map
/// every reader of a part uses: the server's join, the application's reader
/// and the host's drawing, so what is drawn is what is heard.
///
/// A part's own map wins where it has one, an entry past the file's channels
/// being silence. Without one it is the implicit conversion
/// ([`ChannelMatrix::implicit`]): a **mono** file is read on every channel of
/// the list, which is how a mono take sounds under a stereo one, and a wider
/// file channel by channel, silence past its last. A negative entry is
/// silence.
pub fn channel_map(own: Option<&[i32]>, file_channels: usize, list_channels: usize) -> Vec<i32> {
    let matrix = match own {
        Some(map) => ChannelMatrix::routing(map, file_channels, list_channels),
        None => ChannelMatrix::implicit(file_channels, list_channels),
    };
    matrix
        .as_routing()
        .expect("a routing and the implicit conversion are routings")
}

/// **Places cuts one after another**: each part starts where the last ended.
/// Each cut comes with its source's length in frames and its rate ratio.
pub fn place(cuts: impl IntoIterator<Item = (Cut, usize, f64)>) -> (Vec<PartSpan>, usize) {
    let mut start = 0;
    let parts = cuts
        .into_iter()
        .map(|(cut, src_frames, rate)| {
            let part = PartSpan::new(cut, src_frames, rate, start);
            start += part.frames;
            part
        })
        .collect();
    (parts, start)
}

/// The index of the part holding `frame` in parts placed in order, `None`
/// past the end.
pub fn part_at<P: AsRef<PartSpan>>(parts: &[P], frame: usize) -> Option<usize> {
    let i = parts.partition_point(|p| p.as_ref().start() <= frame);
    (i > 0 && frame < parts[i - 1].as_ref().end()).then(|| i - 1)
}

impl AsRef<PartSpan> for PartSpan {
    fn as_ref(&self) -> &PartSpan {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cut(src_start: usize, frames: usize) -> Cut {
        Cut {
            src_start,
            frames,
            fade_in: 0,
            fade_out: 0,
            fade_in_shape: FadeShape::LINEAR,
            fade_out_shape: FadeShape::LINEAR,
            map: vec![0],
        }
    }

    /// Parts placed in order start where the last ended, and a frame finds
    /// its part.
    #[test]
    fn parts_are_placed_end_to_end() {
        let (parts, frames) = place([(cut(0, 10), 100, 1.0), (cut(50, 5), 100, 1.0)]);
        assert_eq!(frames, 15);
        assert_eq!((parts[1].start(), parts[1].end()), (10, 15));
        assert_eq!(part_at(&parts, 0), Some(0));
        assert_eq!(part_at(&parts, 10), Some(1));
        assert_eq!(part_at(&parts, 15), None);
    }

    /// A mono file is read on every channel; a wider one channel by channel,
    /// silence past it; a part's own map wins, inside its file.
    #[test]
    fn a_part_without_a_map_reads_a_mono_file_on_every_channel() {
        assert_eq!(channel_map(None, 1, 2), [0, 0]);
        assert_eq!(channel_map(None, 1, 4), [0, 0, 0, 0]);
        assert_eq!(channel_map(None, 2, 2), [0, 1]);
        assert_eq!(channel_map(None, 2, 4), [0, 1, -1, -1]);
        assert_eq!(channel_map(None, 4, 2), [0, 1]);
        assert_eq!(channel_map(Some(&[1, 3]), 2, 2), [1, -1]);
        assert_eq!(channel_map(Some(&[0]), 2, 2), [0, -1]);
    }

    /// A fade multiplies the source; the map routes and silences; a part at
    /// another rate reads between frames and stays inside its source.
    #[test]
    fn a_part_reads_through_its_map_rate_and_fades() {
        let src = |f: usize, ch: usize| (f * 10 + ch) as f32;
        let mut c = cut(4, 8);
        c.fade_in = 3;
        c.map = vec![1, -1];
        let part = PartSpan::new(c, 20, 1.0, 0);
        assert_eq!(part.read_with(5, 0, src), 91.0);
        assert_eq!(part.read_with(5, 1, src), 0.0, "a negative map is silence");
        assert!((part.read_with(0, 0, src) - 41.0 * 0.25).abs() < 1e-5);

        let slow = PartSpan::new(cut(0, 8), 4, 0.5, 0);
        assert_eq!(slow.read_with(1, 0, |f, _| f as f32), 0.5);
        assert_eq!(
            slow.read_with(7, 0, |f, _| f as f32),
            3.0,
            "clamped at the end"
        );
        assert_eq!(slow.source_span(0, 8), (0, 4));
    }
}
