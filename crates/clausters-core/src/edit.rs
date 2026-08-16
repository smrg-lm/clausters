//! Destructive edits over a span of interleaved samples — the verbs an audio
//! editor applies to material.
//!
//! Three of them, and the set is small on purpose: `gain` with a shape covers
//! constant gain, fades in and out, silence and each half of a crossfade;
//! [`replace`] is what a pencil stroke and a paste produce; [`reverse`] is
//! itself. What is *not* here is anything with a timeline — a fade is
//! arithmetic over a span and an effect is a graph, and the second belongs to
//! the engine (`server::nrtsession`), not to this module. That line, and not
//! "does it need a UGen", is what decides where an edit operation lives.
//!
//! `normalize` is deliberately absent for the same reason it is not a verb: it
//! is a measurement (`crate::measure`) followed by a [`gain`], composed by
//! whoever wants it, and building it in would freeze one policy for what
//! "normalized" means.
//!
//! **Nothing here allocates or does I/O**, and nothing here knows what a file
//! or a buffer is: a span of interleaved `f32` plus a channel count is the
//! whole vocabulary, which is what lets the same function serve a server
//! command, an offline session and a client that has the samples in hand.
//!
//! **Spans are frames, not flat indices.** A selection is a stretch of time
//! across every channel, so that is what these take; the flat interleaved
//! index the `/buffer_set*` commands speak is a different unit for a different
//! job (writing samples you already have, at a position you already computed).
//!
//! The shape vocabulary is [`crate::envshape`]'s — the SuperCollider shape
//! numbers the whole system already speaks: `EnvGen` plays them, the
//! breakpoint editor draws them, and a fade uses them rather than inventing a
//! third spelling of "curve".

use crate::envshape::shape_value;

/// What went wrong with a span, named rather than clamped: an edit that
/// silently did less than it was asked would lose exactly the material the
/// caller believes it changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    /// `channels` was zero, or the data is not a whole number of frames.
    Shape,
    /// The span runs past the end of the material.
    Span,
    /// A replacement whose length is not the span's, in frames.
    Length,
}

impl core::fmt::Display for EditError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EditError::Shape => write!(f, "channel count and sample count disagree"),
            EditError::Span => write!(f, "the span runs past the end of the material"),
            EditError::Length => write!(f, "the replacement is not the length of the span"),
        }
    }
}

impl std::error::Error for EditError {}

/// Frames in `data`, or [`EditError::Shape`] if the two disagree.
fn frames_of(data: &[f32], channels: usize) -> Result<usize, EditError> {
    if channels == 0 || !data.len().is_multiple_of(channels) {
        return Err(EditError::Shape);
    }
    Ok(data.len() / channels)
}

/// The flat sample range a frame span covers, bounds-checked.
fn range(
    data: &[f32],
    channels: usize,
    start: usize,
    frames: usize,
) -> Result<core::ops::Range<usize>, EditError> {
    let total = frames_of(data, channels)?;
    let end = start.checked_add(frames).ok_or(EditError::Span)?;
    if end > total {
        return Err(EditError::Span);
    }
    Ok(start * channels..end * channels)
}

/// The factor a [`gain`] applies across its span: from one level to another
/// along an envelope shape.
///
/// It is a value rather than four arguments because the four only ever travel
/// together, and because that is what makes a constant gain say itself —
/// [`Fade::constant`] against [`Fade::from_to`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fade {
    pub from: f32,
    pub to: f32,
    /// A [`crate::envshape`] shape number.
    pub shape: i32,
    /// Read only by the custom-curvature shape.
    pub curve: f32,
}

impl Fade {
    /// One factor for the whole span.
    pub fn constant(factor: f32) -> Self {
        Self {
            from: factor,
            to: factor,
            shape: crate::envshape::SHAPE_LINEAR,
            curve: 0.0,
        }
    }

    /// A sweep along `shape`; `curve` matters only for the custom-curvature
    /// one.
    pub fn from_to(from: f32, to: f32, shape: i32, curve: f32) -> Self {
        Self {
            from,
            to,
            shape,
            curve,
        }
    }
}

/// Scales `frames` frames from `start` by `fade`.
///
/// One verb, four jobs: a constant factor is a plain gain, `0.0 -> 1.0` a fade
/// in, `1.0 -> 0.0` a fade out, `0.0 -> 0.0` silence. Every channel of a frame
/// is scaled by the same factor, so a fade cannot tilt a stereo image.
///
/// The factor is evaluated per frame at `t = i / frames`, and the last frame
/// therefore does not quite reach `to` — the envelope convention, where the
/// target is committed when the segment ends. A fade to silence that must land
/// on exact zeros is [`silence`].
pub fn gain(
    data: &mut [f32],
    channels: usize,
    start: usize,
    frames: usize,
    fade: Fade,
) -> Result<(), EditError> {
    let Fade {
        from,
        to,
        shape,
        curve,
    } = fade;
    let span = range(data, channels, start, frames)?;
    if frames == 0 {
        return Ok(());
    }
    // A constant factor is the common case (plain gain, silence) and needs no
    // shape evaluation at all.
    if from == to {
        for s in &mut data[span] {
            *s *= from;
        }
        return Ok(());
    }
    let n = frames as f32;
    for (i, frame) in data[span].chunks_exact_mut(channels).enumerate() {
        let g = shape_value(shape, curve, from, to, i as f32 / n);
        for s in frame {
            *s *= g;
        }
    }
    Ok(())
}

/// Silences `frames` frames from `start` — [`gain`] with both ends at zero,
/// named because it is what a caller means and because it needs no shape.
pub fn silence(
    data: &mut [f32],
    channels: usize,
    start: usize,
    frames: usize,
) -> Result<(), EditError> {
    let span = range(data, channels, start, frames)?;
    data[span].fill(0.0);
    Ok(())
}

/// Writes `samples` over `frames` frames from `start`.
///
/// The replacement must be exactly the span's length in samples
/// (`frames * channels`) — a short one would leave the tail of the span stale
/// and a long one would run past it, and both are the caller having computed
/// something other than what they asked to replace.
pub fn replace(
    data: &mut [f32],
    channels: usize,
    start: usize,
    frames: usize,
    samples: &[f32],
) -> Result<(), EditError> {
    let span = range(data, channels, start, frames)?;
    if samples.len() != span.len() {
        return Err(EditError::Length);
    }
    data[span].copy_from_slice(samples);
    Ok(())
}

/// Reverses `frames` frames from `start`, in place.
///
/// Frames are reversed, not samples: a stereo pair stays a stereo pair, and
/// the channel order inside each frame is untouched.
pub fn reverse(
    data: &mut [f32],
    channels: usize,
    start: usize,
    frames: usize,
) -> Result<(), EditError> {
    let span = range(data, channels, start, frames)?;
    let region = &mut data[span];
    let mut lo = 0usize;
    let mut hi = frames;
    while lo + 1 < hi {
        hi -= 1;
        let (a, b) = (lo * channels, hi * channels);
        for c in 0..channels {
            region.swap(a + c, b + c);
        }
        lo += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envshape::{SHAPE_LINEAR, SHAPE_STEP};

    fn ramp(frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels).map(|i| i as f32 + 1.0).collect()
    }

    #[test]
    fn a_constant_gain_scales_only_its_span() {
        let mut d = ramp(4, 2);
        gain(&mut d, 2, 1, 2, Fade::constant(0.5)).unwrap();
        assert_eq!(d, vec![1.0, 2.0, 1.5, 2.0, 2.5, 3.0, 7.0, 8.0]);
    }

    #[test]
    fn a_linear_fade_scales_every_channel_of_a_frame_alike() {
        let mut d = vec![1.0; 8]; // 4 frames, stereo
        gain(&mut d, 2, 0, 4, Fade::from_to(0.0, 1.0, SHAPE_LINEAR, 0.0)).unwrap();
        for frame in d.chunks_exact(2) {
            assert_eq!(frame[0], frame[1], "a fade must not tilt the image");
        }
        // t = i/4, so 0, 0.25, 0.5, 0.75 — the target is committed at the end.
        assert_eq!(d, vec![0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75]);
    }

    #[test]
    fn silence_is_exact_where_a_fade_only_tends_to_zero() {
        let mut faded = vec![1.0; 4];
        gain(
            &mut faded,
            1,
            0,
            4,
            Fade::from_to(1.0, 0.0, SHAPE_LINEAR, 0.0),
        )
        .unwrap();
        assert!(faded[3] > 0.0, "the last frame has not reached the target");
        let mut zeroed = vec![1.0; 4];
        silence(&mut zeroed, 1, 0, 4).unwrap();
        assert_eq!(zeroed, vec![0.0; 4]);
    }

    #[test]
    fn a_step_shape_is_the_target_from_the_first_frame() {
        let mut d = vec![1.0; 4];
        gain(&mut d, 1, 0, 4, Fade::from_to(0.0, 0.5, SHAPE_STEP, 0.0)).unwrap();
        assert_eq!(d, vec![0.5; 4]);
    }

    #[test]
    fn reverse_turns_frames_around_and_leaves_channels_in_place() {
        let mut d = ramp(4, 2); // L R pairs: (1,2) (3,4) (5,6) (7,8)
        reverse(&mut d, 2, 0, 4).unwrap();
        assert_eq!(d, vec![7.0, 8.0, 5.0, 6.0, 3.0, 4.0, 1.0, 2.0]);
    }

    #[test]
    fn reverse_of_an_odd_span_keeps_its_middle_frame() {
        let mut d = ramp(3, 1);
        reverse(&mut d, 1, 0, 3).unwrap();
        assert_eq!(d, vec![3.0, 2.0, 1.0]);
    }

    #[test]
    fn reverse_inside_a_span_leaves_the_rest_alone() {
        let mut d = ramp(4, 1);
        reverse(&mut d, 1, 1, 2).unwrap();
        assert_eq!(d, vec![1.0, 3.0, 2.0, 4.0]);
    }

    #[test]
    fn replace_wants_exactly_the_span() {
        let mut d = ramp(4, 2);
        assert_eq!(
            replace(&mut d, 2, 1, 2, &[0.0; 3]),
            Err(EditError::Length),
            "a replacement of the wrong length is the caller having computed \
             something other than what they asked to replace"
        );
        replace(&mut d, 2, 1, 2, &[9.0; 4]).unwrap();
        assert_eq!(d, vec![1.0, 2.0, 9.0, 9.0, 9.0, 9.0, 7.0, 8.0]);
    }

    #[test]
    fn a_span_past_the_end_fails_rather_than_doing_less() {
        let mut d = ramp(4, 2);
        for e in [
            gain(&mut d, 2, 3, 2, Fade::constant(0.5)),
            silence(&mut d, 2, 5, 1),
            reverse(&mut d, 2, 0, 5),
        ] {
            assert_eq!(e, Err(EditError::Span));
        }
        assert_eq!(d, ramp(4, 2), "and nothing was written on the way out");
    }

    #[test]
    fn a_shape_that_does_not_divide_the_channels_is_refused() {
        let mut d = vec![0.0; 5];
        assert_eq!(
            gain(&mut d, 2, 0, 1, Fade::constant(1.0)),
            Err(EditError::Shape)
        );
        assert_eq!(silence(&mut d, 0, 0, 0), Err(EditError::Shape));
    }

    #[test]
    fn an_empty_span_is_a_no_op_rather_than_an_error() {
        let mut d = ramp(2, 2);
        gain(&mut d, 2, 2, 0, Fade::from_to(0.0, 1.0, SHAPE_LINEAR, 0.0)).unwrap();
        reverse(&mut d, 2, 1, 0).unwrap();
        assert_eq!(d, ramp(2, 2));
    }
}
