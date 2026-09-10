//! A buffer whose samples are **other buffers' samples**: the stitched buffer.
//!
//! A window onto one buffer is a reader with a start and a span, and that is
//! all a clip needed for as long as a clip was one window. The moment several
//! windows are joined into one — fragments of different files, or of one file
//! in another order — the reader would have to change *which buffer it reads*
//! with sample accuracy, and `bufnum` is an initial-rate control: changing it
//! is a new node. A new node per seam is a control message in the middle of
//! playback, which is exactly what a piece that plays itself from the transport
//! must not need.
//!
//! So the join moves out of the reader and into the buffer. A [`Stitch`] is a
//! list of [`Part`]s — a span of some source buffer, with its own channel
//! mapping and its own edit crossfades — and to every reader it is a buffer
//! like any other: it has frames, channels and a sample rate, and it answers
//! [`Buffer::sample`](crate::dsp::buffer::Buffer::sample). Which part a frame
//! belongs to is resolved on the audio thread, and resolving it allocates
//! nothing, locks nothing and reads no file: the sources are `Arc`s cloned when
//! the stitch was built, on the network thread.
//!
//! # What it costs, and why the cursor is here
//!
//! One lookup per sample. A reader almost always asks for the frame after the
//! one it just asked for, so the last part used is remembered and checked
//! before anything else; the part after it is checked second, which
//! is what crossing a seam looks like. Only a real jump pays the binary search,
//! over a list that is as long as the join has pieces. The cursor is shared by
//! every reader of the buffer and is a *hint*: two readers at two places make
//! each other miss, and a miss is a binary search and not a wrong answer.
//!
//! # A stitch is read, never written
//!
//! There are no cells to write: a stitched buffer owns no samples. Writing one
//! would mean writing *through* to whichever source a frame happens to land on,
//! which turns one edit into an edit of several takes and is the opposite of
//! what a join is for. So the cells are absent rather than empty
//! ([`Buffer::cells`](crate::dsp::buffer::Buffer::cells) answers `None`) and
//! every write path refuses a stitch by name.
//!
//! That takes nothing away, because **a stitch is replaced rather than
//! edited** — the same rule the server already states for `/buffer_alloc`,
//! `/buffer_read` and `/buffer_gen`, which install a new buffer whole. Re-cutting
//! a join costs the list of parts and not the samples, and the reader that was
//! running keeps running.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::buffer::Buffer;

/// One source's contribution to a stitched buffer.
pub struct Part {
    /// Where the samples come from. The `Arc` is what keeps a source alive for
    /// as long as something is stitched over it, so freeing the take a join was
    /// cut from does not silence the join.
    pub src: Arc<Buffer>,
    /// The first frame read from `src`.
    pub src_start: usize,
    /// How many frames this part contributes.
    pub frames: usize,
    /// A linear fade over this many frames at the part's start, and at its end.
    /// Two spans that do not continue each other make a step, which is a click
    /// however well the frames themselves are read — this is the few
    /// milliseconds every editor puts on a cut, and it belongs here because
    /// here is where the seam is.
    pub fade_in: usize,
    pub fade_out: usize,
    /// `1 / (fade + 1)` for each side, computed once: a fade is a multiply per
    /// sample rather than two integer-to-float conversions and a divide.
    fade_in_step: f32,
    fade_out_step: f32,
    /// Whether either fade is set at all. The common part has neither, and the
    /// whole crossfade arm is then one predicted branch.
    faded: bool,
    /// Stitched channel → source channel, one entry per channel of the stitched
    /// buffer. A negative entry is silence. It is **routing and not level**: a
    /// mono take heard on both sides of a stereo clip is `[0, 0]` here and a pan
    /// law in the graph, because what is loud is the mixer's question and what
    /// is where is this one's.
    pub map: Box<[i32]>,
    /// The first frame of this part **in the stitched buffer** — the cumulative
    /// sum, computed once at build so a lookup is a comparison.
    start: usize,
}

impl std::fmt::Debug for Part {
    /// Shape only; a part points at millions of samples.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Part")
            .field("start", &self.start)
            .field("frames", &self.frames)
            .field("src_start", &self.src_start)
            .finish_non_exhaustive()
    }
}

/// What a part is made of before its place in the stitch is known — the shape a
/// command parses into, so [`Stitch::new`] is the one place that computes the
/// cumulative starts.
pub struct PartSpec {
    pub src: Arc<Buffer>,
    pub src_start: usize,
    pub frames: usize,
    pub fade_in: usize,
    pub fade_out: usize,
    pub map: Vec<i32>,
}

/// The list of parts a stitched buffer reads, plus the cursor that makes
/// reading it forward cost a comparison.
#[derive(Debug)]
pub struct Stitch {
    parts: Box<[Part]>,
    frames: usize,
    /// The part index last resolved. A hint shared by every reader; see the
    /// module docs.
    cursor: AtomicUsize,
}

impl Stitch {
    /// Builds a stitch from the parts in order, computing where each one lands.
    ///
    /// Rejects what cannot be read rather than reading it wrong: a part with no
    /// frames, a map of the wrong width, a source that is itself too deeply
    /// stitched, or a source at another sample rate — a stitch is a join, not a
    /// resampler, and the caller that knows the rates converts first.
    pub fn new(specs: Vec<PartSpec>, channels: usize, sample_rate: f64) -> Result<Self, String> {
        if specs.is_empty() {
            return Err("a stitched buffer needs at least one part".into());
        }
        let mut parts = Vec::with_capacity(specs.len());
        let mut start = 0usize;
        for (i, spec) in specs.into_iter().enumerate() {
            if spec.frames == 0 {
                return Err(format!("part {i}: frames must be positive"));
            }
            if spec.map.len() != channels {
                return Err(format!(
                    "part {i}: the channel map has {} entries, the buffer has {channels} channels",
                    spec.map.len()
                ));
            }
            if spec.src.sample_rate() != sample_rate {
                return Err(format!(
                    "part {i}: source is at {} Hz and the stitch at {sample_rate} Hz; \
                     resample before stitching",
                    spec.src.sample_rate()
                ));
            }
            if depth(&spec.src) >= MAX_DEPTH {
                return Err(format!(
                    "part {i}: sources are stitched more than {MAX_DEPTH} deep"
                ));
            }
            let half = spec.frames / 2;
            let (fade_in, fade_out) = (spec.fade_in.min(half), spec.fade_out.min(half));
            parts.push(Part {
                src: spec.src,
                src_start: spec.src_start,
                frames: spec.frames,
                // Two fades that overlap would multiply into a notch in the
                // middle of the part; each is capped at half so the worst case
                // is a triangle.
                fade_in,
                fade_out,
                fade_in_step: 1.0 / (fade_in + 1) as f32,
                fade_out_step: 1.0 / (fade_out + 1) as f32,
                faded: fade_in > 0 || fade_out > 0,
                map: spec.map.into_boxed_slice(),
                start,
            });
            start += spec.frames;
        }
        Ok(Stitch {
            parts: parts.into_boxed_slice(),
            frames: start,
            cursor: AtomicUsize::new(0),
        })
    }

    /// How many frames the whole join is.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// How many parts it has — what a query reports and a test reads.
    pub fn len(&self) -> usize {
        self.parts.len()
    }

    /// Whether it has no parts. It never does: [`Stitch::new`] refuses one.
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// One sample of the join, resolved through whichever part holds `frame`.
    ///
    /// Out of range reads as 0, like every other buffer read, and so does a
    /// channel the part maps to nothing.
    #[inline]
    pub fn sample(&self, frame: usize, channel: usize) -> f32 {
        if frame >= self.frames {
            return 0.0;
        }
        let part = &self.parts[self.at(frame)];
        let Some(&src_ch) = part.map.get(channel) else {
            return 0.0;
        };
        if src_ch < 0 {
            return 0.0;
        }
        let inner = frame - part.start;
        // The part was validated when it was built — its span is inside the
        // source and its map inside the source's channels — so this is an
        // indexed load and not a second bounds check. A source that is itself a
        // join has no cells and takes the general path.
        let src = &*part.src;
        let at = (part.src_start + inner) * src.channels() + src_ch as usize;
        let value = match src.cells() {
            Some(cells) => Buffer::load(&cells[at]),
            None => src.sample(part.src_start + inner, src_ch as usize),
        };
        if part.faded {
            value * fade(part, inner)
        } else {
            value
        }
    }

    /// Which part holds `frame`. The cursor first, then the part after it, then
    /// the search — see the module docs for why that order is the whole cost
    /// argument.
    #[inline]
    fn at(&self, frame: usize) -> usize {
        let hint = self.cursor.load(Ordering::Relaxed);
        if let Some(part) = self.parts.get(hint)
            && holds(part, frame)
        {
            return hint;
        }
        if let Some(part) = self.parts.get(hint + 1)
            && holds(part, frame)
        {
            self.cursor.store(hint + 1, Ordering::Relaxed);
            return hint + 1;
        }
        // `partition_point` is the first part that starts *after* the frame, so
        // the one before it is the one that holds it. There is always one: the
        // caller checked the frame is in range and the first part starts at 0.
        let found = self.parts.partition_point(|p| p.start <= frame) - 1;
        self.cursor.store(found, Ordering::Relaxed);
        found
    }
}

/// How deeply stitched buffers may point at each other. A stitch of stitches
/// costs one more lookup per level and is legal; a cycle is not expressible
/// (a part holds an `Arc` to a buffer that already exists), but a deep chain
/// is, and it would pay for itself on the audio thread.
pub const MAX_DEPTH: usize = 4;

/// How many stitches deep this buffer's samples are.
fn depth(buffer: &Buffer) -> usize {
    match buffer.stitch() {
        None => 0,
        Some(stitch) => {
            1 + stitch
                .parts
                .iter()
                .map(|p| depth(&p.src))
                .max()
                .unwrap_or(0)
        }
    }
}

#[inline]
fn holds(part: &Part, frame: usize) -> bool {
    frame >= part.start && frame - part.start < part.frames
}

/// The crossfade factor at `inner` frames into a part.
#[inline]
fn fade(part: &Part, inner: usize) -> f32 {
    let mut gain = 1.0f32;
    if inner < part.fade_in {
        gain *= (inner + 1) as f32 * part.fade_in_step;
    }
    let from_end = part.frames - 1 - inner;
    if from_end < part.fade_out {
        gain *= (from_end + 1) as f32 * part.fade_out_step;
    }
    gain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(values: &[f32]) -> Arc<Buffer> {
        Arc::new(Buffer::new(values.to_vec(), 1, values.len(), 48_000.0))
    }

    fn spec(src: &Arc<Buffer>, src_start: usize, frames: usize) -> PartSpec {
        PartSpec {
            src: Arc::clone(src),
            src_start,
            frames,
            fade_in: 0,
            fade_out: 0,
            map: vec![0],
        }
    }

    /// **A jump backwards is a search, not a wrong answer.** The cursor makes
    /// reading forward cost a comparison; what it must never do is answer for
    /// the part it happens to be sitting on.
    #[test]
    fn a_read_out_of_order_lands_in_the_right_part() {
        let a = take(&[1.0, 2.0, 3.0]);
        let b = take(&[10.0, 20.0, 30.0]);
        let s = Stitch::new(
            vec![spec(&a, 0, 3), spec(&b, 0, 3), spec(&a, 0, 3)],
            1,
            48_000.0,
        )
        .expect("built");
        // Forward, then all the way back, then into the third part.
        let order = [0usize, 1, 2, 3, 4, 5, 0, 7, 2, 8, 1];
        let read: Vec<f32> = order.iter().map(|&f| s.sample(f, 0)).collect();
        assert_eq!(
            read,
            vec![1.0, 2.0, 3.0, 10.0, 20.0, 30.0, 1.0, 2.0, 3.0, 3.0, 2.0]
        );
        assert_eq!(s.sample(9, 0), 0.0, "past the end reads as 0");
    }

    /// **A seam is read from both sides.** An interpolated reader asks for the
    /// frame before and the frame after; at a seam those are in two different
    /// parts, and each has to resolve on its own — a reader that got silence
    /// for the far side would click at every cut.
    #[test]
    fn the_two_frames_of_a_seam_come_from_two_parts() {
        let a = take(&[1.0, 1.0]);
        let b = take(&[5.0, 5.0]);
        let s = Stitch::new(vec![spec(&a, 0, 2), spec(&b, 0, 2)], 1, 48_000.0).expect("built");
        assert_eq!(s.sample(1, 0), 1.0, "the last frame of the first part");
        assert_eq!(s.sample(2, 0), 5.0, "the first frame of the second");
        // What `read_lin` computes at the half-frame between them.
        assert_eq!((s.sample(1, 0) + s.sample(2, 0)) / 2.0, 3.0);
    }

    /// **A stitch of stitches is legal and bounded.** It costs one lookup per
    /// level, so the depth is capped rather than left to a caller to discover.
    #[test]
    fn a_join_may_be_joined_again_up_to_the_cap() {
        let a = take(&[1.0, 2.0, 3.0, 4.0]);
        let mut held = Arc::new(Buffer::stitched(
            Stitch::new(vec![spec(&a, 2, 2)], 1, 48_000.0).expect("built"),
            1,
            48_000.0,
        ));
        for _ in 1..MAX_DEPTH {
            let next = Stitch::new(vec![spec(&held, 0, 2)], 1, 48_000.0).expect("built");
            held = Arc::new(Buffer::stitched(next, 1, 48_000.0));
        }
        assert_eq!(held.sample(0, 0), 3.0, "through every level");
        let too_deep = Stitch::new(vec![spec(&held, 0, 2)], 1, 48_000.0);
        assert!(too_deep.is_err(), "one level past the cap is refused");
    }

    /// **A join is not a resampler.** Two rates in one buffer is a question this
    /// has no answer to, so it says so rather than reading the frames at the
    /// wrong speed.
    #[test]
    fn a_source_at_another_rate_is_refused() {
        let a = Arc::new(Buffer::new(vec![1.0, 2.0], 1, 2, 44_100.0));
        let built = Stitch::new(vec![spec(&a, 0, 2)], 1, 48_000.0);
        assert!(
            built.is_err_and(|e| e.contains("resample")),
            "it says what to do"
        );
    }

    /// **Two fades cannot eat each other.** Each is capped at half the part, so
    /// the worst case is a triangle rather than a notch in the middle.
    #[test]
    fn overlapping_fades_are_capped_at_half_the_part() {
        let a = take(&[1.0, 1.0, 1.0, 1.0]);
        let s = Stitch::new(
            vec![PartSpec {
                fade_in: 100,
                fade_out: 100,
                ..spec(&a, 0, 4)
            }],
            1,
            48_000.0,
        )
        .expect("built");
        let read: Vec<f32> = (0..4).map(|f| s.sample(f, 0)).collect();
        assert!(read[0] < read[1], "it rises to the middle: {read:?}");
        assert!(read[2] > read[3], "and falls after it: {read:?}");
        assert!(read.iter().all(|&v| v > 0.0), "and never reaches zero");
    }
}
