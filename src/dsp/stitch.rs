//! A buffer whose samples are **other buffers' samples**: the stitched buffer.
//!
//! A window onto one buffer is a reader with a start and a span, and that is
//! all a clip needed for as long as a clip was one window. The moment several
//! windows are joined into one -- fragments of different files, or of one file
//! in another order -- the reader would have to change *which buffer it reads*
//! with sample accuracy, and `bufnum` is an initial-rate control: changing it
//! is a new node. A new node per seam is a control message in the middle of
//! playback, which is exactly what a multitrack that plays itself from the transport
//! must not need.
//!
//! So the join moves out of the reader and into the buffer. A [`Stitch`] is a
//! list of [`Part`]s -- a span of some source buffer, with its own channel
//! mapping and its own edit crossfades -- and to every reader it is a buffer
//! like any other: it has frames, channels and a sample rate, and it answers
//! [`Buffer::sample`](crate::dsp::buffer::Buffer::sample). Which part a frame
//! belongs to is resolved on the audio thread, and resolving it allocates
//! nothing, locks nothing and reads no file: the sources are `Arc`s cloned when
//! the stitch was built, on the network thread.
//!
//! # What it costs, and why a reader holds a run
//!
//! One lookup, **per run rather than per sample**. A reader advances
//! monotonically and a block almost always lies inside one part, so a reader
//! asks which part a frame is in ([`Stitch::run_at`]), holds the answer as a
//! [`PartRun`] and reads the whole block out of it; the per-sample path is
//! what a frame outside the run falls back to, which is the seam, a jump, a
//! modulated rate or a phase running backwards. That is `dsp::buf`'s
//! `Reader` and not this module, because a reader is what
//! knows it is advancing.
//!
//! The lookup it saves was two questions, and a run answers both once: *which
//! part* holds this frame, and *where* that part's source keeps its samples --
//! so a read inside a run is the indexed load a plain buffer costs, not a
//! resolution.
//!
//! The cursor is what makes the remaining lookup cheap. The last part used is
//! remembered and checked before anything else; the part after it is checked
//! second, which is what crossing a seam looks like. Only a real jump pays the
//! binary search, over a list as long as the join has pieces. It is shared by
//! every reader of the buffer and is a *hint*: two readers at two places make
//! each other miss, and a miss is a binary search and not a wrong answer --
//! which also stopped being a contended write on every sample when the run
//! arrived.
//!
//! The numbers are in `tests/stitch_load.rs` (an engine full of readers) and
//! `tests/buffer_stitch.rs` (the read itself), both `--ignored`.
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
//! edited** -- the same rule the server already states for `/buffer_alloc`,
//! `/buffer_read` and `/buffer_gen`, which install a new buffer whole. Re-cutting
//! a join costs the list of parts and not the samples, and the reader that was
//! running keeps running.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use clausters_core::segments::{Cut, PartSpan, rate_between};

use super::buffer::Buffer;

/// One source's contribution to a stitched buffer: the source, and the
/// part's arithmetic over it ([`PartSpan`], the core's, which the GUI host
/// reads its own files through). The span's fields read as the part's own
/// (`part.frames`, `part.map`, `part.start()`).
pub struct Part {
    /// Where the samples come from. The `Arc` is what keeps a source alive for
    /// as long as something is stitched over it, so freeing the take a join was
    /// cut from does not silence the join.
    pub src: Arc<Buffer>,
    /// The number the source was stitched under.
    ///
    /// Carried and never read while playing: a join is `Arc`s once it is built,
    /// and an index is what a *client* speaks. It is here so `/buffer_parts` can
    /// answer a join in the same terms `/buffer_stitch` was given it in, which
    /// is what makes a reply something a client can edit and send back.
    pub src_index: i32,
    /// The cut, its rate, its fades, its map and its place in the join.
    pub span: PartSpan,
}

impl std::ops::Deref for Part {
    type Target = PartSpan;

    /// A part *is* its span over a source; the source is what the server
    /// adds, so the span's fields and reads are reached through the part.
    fn deref(&self) -> &PartSpan {
        &self.span
    }
}

/// **A part with its source's cells already taken** -- what a reader holds while
/// it stays inside one part.
///
/// The lookup a join costs is two questions, not one: *which part* holds this
/// frame, and *where* that part's source keeps its samples. Resolving the first
/// once a block leaves the second, which is a match on the source's storage per
/// sample; taking it here leaves an indexed load, which is what a plain buffer
/// costs. A source that is itself a join has no cells and takes the general
/// path, one level deeper.
#[derive(Clone, Copy)]
pub struct PartRun<'a> {
    part: &'a Part,
    /// The source's cells, when it has some. A source that is itself a join has
    /// none and takes the general path, one level deeper.
    cells: Option<&'a [std::sync::atomic::AtomicU32]>,
    /// The source's channel count, which is the stride of an interleaved read.
    stride: usize,
}

impl<'a> PartRun<'a> {
    /// Opens a run over `part`.
    #[inline]
    pub fn new(part: &'a Part) -> Self {
        Self {
            part,
            cells: part.src.cells(),
            stride: part.src.channels(),
        }
    }

    /// The part this is a run over.
    #[inline]
    pub fn part(&self) -> &Part {
        self.part
    }

    /// One sample, `inner` frames into the part.
    #[inline]
    pub fn sample(&self, inner: usize, channel: usize) -> f32 {
        // The part was validated when it was built -- its span is inside its
        // source and its map inside the source's channels -- so a frame read
        // here is an indexed load and not a second bounds check.
        self.part
            .read_with(inner, channel, |frame, src_ch| self.frame(frame, src_ch))
    }

    /// One frame of the source, by the cheapest path this run has.
    #[inline]
    fn frame(&self, frame: usize, src_ch: usize) -> f32 {
        match self.cells {
            Some(cells) => Buffer::load(&cells[frame * self.stride + src_ch]),
            None => self.part.src.sample(frame, src_ch),
        }
    }
}

impl Part {
    /// One sample of this part, `inner` frames into it -- the one-off read, for
    /// a caller that is not staying. A caller that is holds a [`PartRun`].
    #[inline]
    pub fn sample(&self, inner: usize, channel: usize) -> f32 {
        PartRun::new(self).sample(inner, channel)
    }
}

impl std::fmt::Debug for Part {
    /// Shape only; a part points at millions of samples.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Part")
            .field("start", &self.start())
            .field("frames", &self.frames)
            .field("src_start", &self.src_start)
            .finish_non_exhaustive()
    }
}

pub use clausters_core::segments::FadeShape;

/// What a part is made of before its place in the stitch is known -- the shape a
/// command parses into, so [`Stitch::new`] is the one place that computes the
/// cumulative starts.
pub struct PartSpec {
    pub src: Arc<Buffer>,
    pub src_index: i32,
    pub src_start: usize,
    /// How many frames this part contributes **to the join** -- the source
    /// span it reads is this times the ratio the rates make ([`Part::rate`]).
    pub frames: usize,
    pub fade_in: usize,
    pub fade_out: usize,
    pub fade_in_shape: FadeShape,
    pub fade_out_shape: FadeShape,
    pub map: Vec<i32>,
}

/// **Where a part's samples are**, as a command names them: a buffer already
/// in the pool, or a file whose shape has not been read yet.
pub enum PartSource {
    Buffer(Arc<Buffer>),
    File(String),
}

/// A part as `/buffer_stitch` names it, before its file -- if it names one --
/// has been looked at. [`resolve`] turns a list of these into [`PartSpec`]s.
///
/// Reading a file's shape is I/O, so it is the job's and never the parse's:
/// natively the NRT thread opens the file, and in a page the host reads it
/// where the page's files are. Both then validate the parts through the same
/// [`resolve`].
pub struct PartRequest {
    pub src: PartSource,
    pub src_index: i32,
    pub src_start: usize,
    pub frames: usize,
    pub fade_in: usize,
    pub fade_out: usize,
    pub fade_in_shape: FadeShape,
    pub fade_out_shape: FadeShape,
    pub map: Vec<i32>,
}

/// The files `parts` name, each once, in the order they first appear.
pub fn files(parts: &[PartRequest]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in parts {
        if let PartSource::File(path) = &part.src
            && !out.contains(path)
        {
            out.push(path.clone());
        }
    }
    out
}

/// **Resolves a command's parts**: each file through `shape_of`, once, and
/// every part checked against its source -- the span inside it and the map
/// inside its channels -- whichever kind the source is.
pub fn resolve(
    parts: Vec<PartRequest>,
    mut shape_of: impl FnMut(&str) -> Result<crate::dsp::pcm::Layout, String>,
) -> Result<Vec<PartSpec>, String> {
    let mut opened: Vec<Arc<Buffer>> = Vec::new();
    let mut specs = Vec::with_capacity(parts.len());
    for (n, part) in parts.into_iter().enumerate() {
        let (src, label) = match part.src {
            PartSource::Buffer(b) => (b, format!("buffer {}", part.src_index)),
            PartSource::File(path) => {
                let found = opened
                    .iter()
                    .find(|b| b.file().is_some_and(|l| l.path == path))
                    .cloned();
                let b = match found {
                    Some(b) => b,
                    None => {
                        let layout = shape_of(&path).map_err(|e| format!("part {n}: {e}"))?;
                        let b = Arc::new(Buffer::on_file(layout));
                        opened.push(Arc::clone(&b));
                        b
                    }
                };
                (b, format!("file {path}"))
            }
        };
        let end = part.src_start + part.frames;
        if end > src.frames() {
            return Err(format!(
                "part {n}: {label} has {} frames and the part asks for {end}",
                src.frames()
            ));
        }
        for (c, &to) in part.map.iter().enumerate() {
            if to >= src.channels() as i32 {
                return Err(format!(
                    "part {n}: channel {c} maps to source channel {to} and {label} has {}",
                    src.channels()
                ));
            }
        }
        specs.push(PartSpec {
            src,
            src_index: part.src_index,
            src_start: part.src_start,
            frames: part.frames,
            fade_in: part.fade_in,
            fade_out: part.fade_out,
            fade_in_shape: part.fade_in_shape,
            fade_out_shape: part.fade_out_shape,
            map: part.map,
        });
    }
    Ok(specs)
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
    /// Whether some part's source is a file (`Buffer::reads_from_disk`).
    on_disk: bool,
}

impl Stitch {
    /// Builds a stitch from the parts in order, computing where each one lands.
    ///
    /// **A part whose source is at another rate is read at that rate**, not
    /// refused: its samples run `src_sr / sample_rate` frames per frame of the
    /// join, which is the same crossing a reader makes over a buffer of any
    /// rate. A join is still not a *converter* -- it owns no samples and stores
    /// none, so nothing here is written out at the join's rate; the ratio is
    /// carried and read through, and the join goes on being a view of the takes
    /// it is spans of. A caller wanting converted samples renders them, which
    /// is a different verb.
    ///
    /// Rejects what cannot be read rather than reading it wrong: a part with no
    /// frames, a map of the wrong width, a source that is itself too deeply
    /// stitched, or a source whose rate is not a positive number.
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
            let Some(rate) = rate_between(spec.src.sample_rate(), sample_rate) else {
                return Err(format!(
                    "part {i}: a rate of zero is no axis -- source {} Hz, stitch {sample_rate} Hz",
                    spec.src.sample_rate()
                ));
            };
            // A list that reads a file is played by a stream reader, which
            // reads its file parts ahead of the transport; one nested inside
            // another would be a file read at random, which is what no reader
            // here does. Its parts are named instead.
            if spec.src.reads_from_disk() && spec.src.file().is_none() {
                return Err(format!(
                    "part {i}: a list that reads a file is not a source of another list -- \
                     name its parts instead"
                ));
            }
            if depth(&spec.src) >= MAX_DEPTH {
                return Err(format!(
                    "part {i}: sources are stitched more than {MAX_DEPTH} deep"
                ));
            }
            let src_frames = spec.src.frames();
            let span = PartSpan::new(
                Cut {
                    src_start: spec.src_start,
                    frames: spec.frames,
                    fade_in: spec.fade_in,
                    fade_out: spec.fade_out,
                    fade_in_shape: spec.fade_in_shape,
                    fade_out_shape: spec.fade_out_shape,
                    map: spec.map,
                },
                src_frames,
                rate,
                start,
            );
            parts.push(Part {
                src: spec.src,
                src_index: spec.src_index,
                span,
            });
            start += spec.frames;
        }
        let on_disk = parts.iter().any(|p| p.src.file().is_some());
        Ok(Stitch {
            parts: parts.into_boxed_slice(),
            frames: start,
            cursor: AtomicUsize::new(0),
            on_disk,
        })
    }

    /// **Writes the join's samples into `cells`**, interleaved over
    /// `channels`: what a server sharing the join copies into the region a
    /// peer maps. Part by part through a [`PartRun`], so each sample is the
    /// indexed load a plain buffer costs rather than a lookup of its part.
    pub fn copy_into(&self, cells: &[std::sync::atomic::AtomicU32], channels: usize) {
        for part in self.parts.iter() {
            let run = PartRun::new(part);
            let first = part.start() * channels;
            let span = &cells[first..(first + part.frames * channels).min(cells.len())];
            for (inner, frame) in span.chunks(channels).enumerate() {
                for (ch, cell) in frame.iter().enumerate() {
                    cell.store(run.sample(inner, ch).to_bits(), Ordering::Relaxed);
                }
            }
        }
    }

    /// Whether some part reads a file -- a list played by a stream reader and
    /// refused by everything that reads at random.
    pub fn reads_from_disk(&self) -> bool {
        self.on_disk
    }

    /// The part holding `frame` and its index, for a reader walking the list
    /// in order (a stream reader filling ahead). `None` past the end.
    pub fn part_at(&self, frame: usize) -> Option<(usize, &Part)> {
        (frame < self.frames).then(|| {
            let i = self.parts.partition_point(|p| p.start() <= frame) - 1;
            (i, &self.parts[i])
        })
    }

    /// How many frames the whole join is.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// How many parts it has -- what a query reports and a test reads.
    pub fn len(&self) -> usize {
        self.parts.len()
    }

    /// The parts in order -- what `/buffer_parts` reports and what a join editor
    /// reads to draw the seams it would move.
    pub fn parts(&self) -> &[Part] {
        &self.parts
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
        match self.run_at(frame) {
            Some(part) => part.sample(frame - part.start(), channel),
            None => 0.0,
        }
    }

    /// **The part holding `frame`**, for a caller that is about to read a whole
    /// run out of it rather than one sample.
    ///
    /// This is the lookup [`Stitch::sample`] does every time, handed out once:
    /// a reader advances monotonically and a block almost always lies inside
    /// one part, so holding the part costs one resolution a block instead of
    /// two a sample. `None` past the end.
    #[inline]
    pub fn run_at(&self, frame: usize) -> Option<&Part> {
        (frame < self.frames).then(|| &self.parts[self.at(frame)])
    }

    /// Which part holds `frame`. The cursor first, then the part after it, then
    /// the search -- see the module docs for why that order is the whole cost
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
        let found = self.parts.partition_point(|p| p.start() <= frame) - 1;
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
    frame >= part.start() && frame - part.start() < part.frames
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
            src_index: 0,
            src_start,
            frames,
            fade_in: 0,
            fade_out: 0,
            fade_in_shape: FadeShape::LINEAR,
            fade_out_shape: FadeShape::LINEAR,
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
    /// parts, and each has to resolve on its own -- a reader that got silence
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

    /// **A part at another rate is read at that rate.** A join owns no samples,
    /// so it converts nothing and stores nothing: the part carries the ratio
    /// its rates make and is read through it, the way every reader here crosses
    /// a buffer of any rate.
    #[test]
    fn a_source_at_another_rate_is_read_at_its_own() {
        // A ramp at half the join's rate: one frame of the join is half a frame
        // of the source, so the join reads 0, 0.5, 1, 1.5, ... of it.
        let a = Arc::new(Buffer::new(vec![0.0, 1.0, 2.0, 3.0], 1, 4, 24_000.0));
        let s = Stitch::new(vec![spec(&a, 0, 6)], 1, 48_000.0).expect("built, not refused");
        let read: Vec<f32> = (0..6).map(|f| s.sample(f, 0)).collect();
        assert_eq!(read, vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5]);
        assert_eq!(s.frames(), 6, "the part contributes what it was asked for");
    }

    /// **The last frames of a part that does not divide evenly stay inside its
    /// source.** A ratio makes the span unprovable at build, so the read is
    /// clamped: the end of the join is the source's last frame rather than a
    /// read past it.
    #[test]
    fn a_part_that_runs_past_its_source_stops_at_its_last_frame() {
        let a = Arc::new(Buffer::new(vec![0.0, 1.0], 1, 2, 24_000.0));
        let s = Stitch::new(vec![spec(&a, 0, 6)], 1, 48_000.0).expect("built");
        let read: Vec<f32> = (0..6).map(|f| s.sample(f, 0)).collect();
        assert_eq!(read, vec![0.0, 0.5, 1.0, 1.0, 1.0, 1.0]);
    }

    /// A rate of zero is no axis at all, and a part over one is refused rather
    /// than read at some rate nobody chose.
    #[test]
    fn a_source_with_no_rate_is_refused() {
        let a = Arc::new(Buffer::new(vec![1.0, 2.0], 1, 2, 0.0));
        assert!(Stitch::new(vec![spec(&a, 0, 2)], 1, 48_000.0).is_err());
    }

    /// **A fade plays the shape it names**: linear is the ramp it always was,
    /// and an equal-power fade is the quarter sine an envelope draws.
    #[test]
    fn a_fade_plays_its_shape() {
        use clausters_core::envshape::{SHAPE_WELCH, shape_value};
        let a = take(&[1.0; 8]);
        let read = |shape: FadeShape| {
            let s = Stitch::new(
                vec![PartSpec {
                    fade_in: 3,
                    fade_out: 3,
                    fade_in_shape: shape,
                    fade_out_shape: shape,
                    ..spec(&a, 0, 8)
                }],
                1,
                48_000.0,
            )
            .expect("built");
            (0..8).map(|f| s.sample(f, 0)).collect::<Vec<f32>>()
        };
        assert_eq!(
            read(FadeShape::LINEAR),
            vec![0.25, 0.5, 0.75, 1.0, 1.0, 0.75, 0.5, 0.25]
        );
        let welch = read(FadeShape {
            shape: SHAPE_WELCH,
            curve: 0.0,
        });
        assert_eq!(welch[0], shape_value(SHAPE_WELCH, 0.0, 0.0, 1.0, 0.25));
        assert!(welch[0] > 0.25, "a quarter sine rises fast: {welch:?}");
        assert_eq!(welch[7], shape_value(SHAPE_WELCH, 0.0, 1.0, 0.0, 0.75));
        assert!(welch[7] < 0.4, "and falls fast at its end: {welch:?}");
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
