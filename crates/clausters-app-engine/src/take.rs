//! **The application reads its takes**: a take's parts over files, rendered
//! into the rings of the engine's fed streams as the readers ask.
//!
//! The take is the application's, as the document says it (the parts of a
//! [`SegmentPart`] list: a file's span, its fades, its channel map, its rate),
//! and so is its reading: the engine plays frames it is handed through a fed
//! stream (`clausters::dsp::fed`) and knows nothing of parts or files. What
//! a part *is* when read is the core's, the one every reader of a list binds:
//! [`PartSpan::read_with`] composes a frame from its source through the
//! part's map, rate and fades, and [`Reader`] reads a span of a PCM file.
//!
//! [`TakeFeed`] is one take's reading: a fed stream per channel of the take,
//! each mono, and one thread of the application serving all of them. A
//! request is a frame and an epoch from that channel's reader, and the
//! thread renders from there, reading the transport's loop straight
//! through, and publishes the epoch once the first frames are in -- the
//! protocol `clausters::dsp::fed` states. [`TakeFeed::set`] puts a new list
//! in place after an edit, and every reader fades into it from where it is.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::Duration;

use clausters_core::threads::Spawned;

use clausters::dsp::ListWrap;
use clausters::dsp::fed::{Fed, Feeder, feed};
use clausters_core::pcm::{Layout, Reader};
use clausters_core::segments::{Cut, FadeShape, PartSpan, part_at, place, rate_between};
use clausters_document::clipboard::SegmentPart;

/// The frames rendered in one turn for a channel.
const CHUNK: usize = 4096;

/// The first turn after a refill renders this few, so the frames the reader
/// is waiting for arrive as soon as one short read allows.
const FIRST_CHUNK: usize = 256;

/// How long the thread sleeps with nothing to do; a request wakes it sooner.
const PARK: Duration = Duration::from_millis(2);

/// How many files the thread keeps open.
const OPEN_FILES: usize = 8;

/// **A take as it is read**: its parts placed end to end at the take's rate,
/// each over the layout of its file.
#[derive(Debug)]
pub struct ReadList {
    parts: Vec<PartSpan>,
    layouts: Vec<Arc<Layout>>,
    frames: u64,
    channels: usize,
}

impl ReadList {
    /// The list `parts` make, `channels` wide at `rate`, each file's header
    /// read once through `headers` (a file a later list names again is not
    /// opened again). A file that cannot be read is a refusal naming it.
    pub fn of(
        parts: &[SegmentPart],
        channels: usize,
        rate: f64,
        headers: &mut Headers,
    ) -> Result<ReadList, String> {
        let channels = channels.max(1);
        let mut layouts = Vec::with_capacity(parts.len());
        let mut cuts = Vec::with_capacity(parts.len());
        for part in parts {
            let layout = headers.layout(&part.path)?;
            let map = (0..channels)
                .map(|c| match &part.channels {
                    Some(map) => map.get(c).copied().unwrap_or(-1),
                    None => (c % part.source_channels.max(1) as usize) as i32,
                })
                .collect();
            let cut = Cut {
                src_start: part.start as usize,
                frames: part.frames as usize,
                fade_in: part.fade_in as usize,
                fade_out: part.fade_out as usize,
                fade_in_shape: FadeShape {
                    shape: part.fade_in_shape.shape,
                    curve: part.fade_in_shape.curve as f32,
                },
                fade_out_shape: FadeShape {
                    shape: part.fade_out_shape.shape,
                    curve: part.fade_out_shape.curve as f32,
                },
                map,
            };
            let ratio = rate_between(part.source_rate, rate).unwrap_or(1.0);
            cuts.push((cut, layout.frames as usize, ratio));
            layouts.push(layout);
        }
        let (parts, frames) = place(cuts);
        Ok(ReadList {
            parts,
            layouts,
            frames: frames as u64,
            channels,
        })
    }

    /// How many frames the take comes to.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// How many channels it is.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Renders `n` frames of channel `channel` from `at` into `out`; a span
    /// past the end, or of a file that does not read, is silence.
    fn render(&self, at: u64, n: usize, channel: usize, out: &mut Vec<f32>, files: &mut Files) {
        out.clear();
        out.resize(n, 0.0);
        let at = at as usize;
        let mut k = 0;
        while k < n {
            let Some(i) = part_at(&self.parts, at + k) else {
                break;
            };
            let part = &self.parts[i];
            let inner = at + k - part.start();
            let len = (part.end() - (at + k)).min(n - k);
            let layout = &self.layouts[i];
            let (first, end) = part.source_span(inner, inner + len);
            let width = layout.channels.max(1);
            let span = files.read(layout, first as u64, end - first);
            for (j, cell) in out[k..k + len].iter_mut().enumerate() {
                *cell = part.read_with(inner + j, channel, |f, sc| {
                    f.checked_sub(first)
                        .and_then(|f| span.get(f * width + sc))
                        .copied()
                        .unwrap_or(0.0)
                });
            }
            k += len;
        }
    }
}

/// **The headers of the files a take names**, read once each.
#[derive(Debug, Default)]
pub struct Headers {
    read: HashMap<String, Arc<Layout>>,
}

impl Headers {
    /// The layout of the file at `path`, read the first time it is asked.
    pub fn layout(&mut self, path: &str) -> Result<Arc<Layout>, String> {
        if let Some(layout) = self.read.get(path) {
            return Ok(Arc::clone(layout));
        }
        let layout = Arc::new(Layout::open(path)?);
        self.read.insert(path.to_string(), Arc::clone(&layout));
        Ok(layout)
    }
}

/// The files the reading thread has open, the most recently read first, and
/// the span last read.
struct Files {
    open: Vec<Reader>,
    span: Vec<f32>,
}

impl Files {
    /// Frames `first..first + frames` of the file `layout` describes,
    /// interleaved; none when it does not read, which plays as silence.
    fn read(&mut self, layout: &Layout, first: u64, frames: usize) -> &[f32] {
        let found = self.open.iter().position(|r| r.layout() == layout);
        let reader = match found {
            Some(i) => Some(self.open.remove(i)),
            None => match Reader::open(layout) {
                Ok(reader) => Some(reader),
                Err(e) => {
                    tracing::warn!("take reader: {e}; the part is silent");
                    None
                }
            },
        };
        self.span.clear();
        if let Some(mut reader) = reader {
            if reader.read(first, frames, &mut self.span).is_err() {
                self.span.clear();
            }
            self.open.insert(0, reader);
            self.open.truncate(OPEN_FILES);
        }
        &self.span
    }
}

/// Where one channel's reader is being served.
#[derive(Default)]
struct Served {
    /// The next frame to render, once a request named one.
    next: Option<u64>,
    /// The transport's loop, read straight through.
    wrap: Option<ListWrap>,
    /// Whether the next render is a refill's first.
    first: bool,
}

/// **The list a reading thread takes next**, handed over with no lock: the
/// application's turn may be a page's main thread, which cannot wait on one
/// the reader holds. The newest list placed wins; the reader takes it once.
#[derive(Debug, Default)]
struct Placed(AtomicPtr<ReadList>);

impl Placed {
    /// Places `list`, dropping one the reader had not taken yet.
    fn put(&self, list: Arc<ReadList>) {
        let old = self
            .0
            .swap(Arc::into_raw(list).cast_mut(), Ordering::AcqRel);
        if !old.is_null() {
            // SAFETY: every pointer in the slot came from `Arc::into_raw`, and
            // the swap gave this one to no one else.
            drop(unsafe { Arc::from_raw(old) });
        }
    }

    /// The list placed since the last take, if one was.
    fn take(&self) -> Option<Arc<ReadList>> {
        let new = self.0.swap(std::ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: as in `put`: the swap made this pointer ours alone.
        (!new.is_null()).then(|| unsafe { Arc::from_raw(new) })
    }
}

impl Drop for Placed {
    fn drop(&mut self) {
        drop(self.take());
    }
}

/// **One take's reading**: a mono fed stream per channel, and the thread of
/// the application that renders the take into them.
pub struct TakeFeed {
    streams: Vec<Arc<Fed>>,
    placed: Arc<Placed>,
    stop: Arc<AtomicBool>,
    reader: Option<Spawned>,
    shared: Vec<Arc<clausters::dsp::fed::FeedShared>>,
}

impl TakeFeed {
    /// The reading of a take `channels` wide, rendering nothing until a list
    /// is [`set`](Self::set).
    pub fn start(channels: usize) -> TakeFeed {
        let (streams, feeders): (Vec<Arc<Fed>>, Vec<Feeder>) = (0..channels.max(1))
            .map(|_| {
                let (fed, feeder) = feed();
                (Arc::new(fed), feeder)
            })
            .unzip();
        let shared = feeders.iter().map(|f| Arc::clone(f.shared())).collect();
        let placed = Arc::new(Placed::default());
        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let (placed, stop) = (Arc::clone(&placed), Arc::clone(&stop));
            clausters_core::threads::spawn("take-reader", move || serve(feeders, placed, stop))
                .expect("failed to spawn a take's reader")
        };
        TakeFeed {
            streams,
            placed,
            stop,
            reader: Some(reader),
            shared,
        }
    }

    /// The fed stream of channel `channel`, for the buffer its reader plays.
    pub fn stream(&self, channel: usize) -> Option<&Arc<Fed>> {
        self.streams.get(channel)
    }

    /// How many channels it reads.
    pub fn channels(&self) -> usize {
        self.streams.len()
    }

    /// **Reads `list` from now on**: every channel's reader fades from what
    /// its ring holds into a refill of it, from where it is.
    pub fn set(&self, list: Arc<ReadList>) {
        self.placed.put(list);
        for shared in &self.shared {
            shared.generation.fetch_add(1, Ordering::AcqRel);
        }
        if let Some(reader) = &self.reader {
            reader.unpark();
        }
    }
}

impl Drop for TakeFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            reader.unpark();
            // waited for natively; in a page it ends on its own
            reader.join();
        }
    }
}

/// The reading thread: each turn, every channel's latest request taken and a
/// chunk rendered from where it left off, until nothing is left to do.
fn serve(mut feeders: Vec<Feeder>, placed: Arc<Placed>, stop: Arc<AtomicBool>) {
    for feeder in &feeders {
        feeder.wake_me();
    }
    let mut served: Vec<Served> = feeders.iter().map(|_| Served::default()).collect();
    let mut list: Option<Arc<ReadList>> = None;
    let mut files = Files {
        open: Vec::new(),
        span: Vec::new(),
    };
    let mut frames = Vec::new();
    while !stop.load(Ordering::Acquire) {
        if let Some(new) = placed.take() {
            list = Some(new);
        }
        let mut worked = false;
        for (channel, (feeder, at)) in feeders.iter_mut().zip(&mut served).enumerate() {
            if let Some(request) = feeder.latest() {
                at.next = Some(request.frame);
                at.wrap = request.wrap;
                at.first = true;
            }
            let (Some(l), Some(from)) = (list.as_ref(), at.next) else {
                feeder.publish();
                continue;
            };
            let total = l.frames();
            if from >= total {
                feeder.publish();
                continue;
            }
            let want = if at.first { FIRST_CHUNK } else { CHUNK };
            let room = feeder.room();
            // A read stops at the loop's end, and the next one starts at its
            // start: the ring holds the loop straight through.
            let left = ListWrap::before(at.wrap, from).unwrap_or(total - from) as usize;
            let n = want.min(room).min(left);
            if n == 0 || (!at.first && room < want && left > room) {
                feeder.publish();
                continue;
            }
            l.render(
                from,
                n,
                channel.min(l.channels() - 1),
                &mut frames,
                &mut files,
            );
            feeder.push(&frames);
            at.next = Some(ListWrap::advance(at.wrap, from, n as u64));
            at.first = false;
            feeder.publish();
            worked = true;
        }
        if !worked {
            std::thread::park_timeout(PARK);
        }
    }
}
