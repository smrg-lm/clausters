//! **The application reads its takes**: a take's parts over files, rendered
//! into the rings of the engine's fed streams as the readers ask.
//!
//! The take is the application's, as the editor holds it (a [`FileList`]: its
//! tree of parts, each a file's span with its fades and its channel map, and
//! the files they read, shared with the editor), and so is its reading: the engine plays frames it is handed through a fed
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

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clausters_core::threads::Spawned;

use clausters::dsp::ListWrap;
use clausters::dsp::fed::{Fed, Feeder, feed};
use clausters_core::pcm::{Layout, Reader};
use clausters_core::segments::{Cut, FadeShape, PartSpan, rate_between};
use clausters_document::clipboard::SegmentPart;
use clausters_document::parts::tree::Node;
use clausters_document::parts::{FileList, TakeFile, Tree, contributed};
use clausters_document::session::Part;
use clausters_document::{Lifetime, Range, SourceId, SourceRef};

/// The frames rendered in one turn for a channel.
const CHUNK: usize = 4096;

/// The first turn after a refill renders this few, so the frames the reader
/// is waiting for arrive as soon as one short read allows.
const FIRST_CHUNK: usize = 256;

/// How long the thread sleeps with nothing to do, as a safety net: a
/// request, a list put in place and a reader's ring fallen to half wake it
/// (`clausters::dsp::stream`), so a take that is not playing costs no turn of
/// a core five hundred times a second.
const PARK: Duration = Duration::from_millis(100);

/// How many files the thread keeps open.
const OPEN_FILES: usize = 8;

/// **A take as it is read**: its parts, shared with the take the editor
/// holds ([`Tree`]), and the layout of each file they name.
///
/// A part is found where it is read by descending the tree from the frame
/// asked, so making one costs the files the take names and reading from any
/// frame costs the tree's height: an edit of a take of thousands of parts
/// hands the reader a new tree and nothing is placed again. At the list's
/// own rate a node fills the frames the tree counts; at another each part
/// fills its span at the ratio of the two ([`contributed`]), and what a node
/// fills is counted once, the first time it is read, and kept by the node's
/// identity ([`Counts`]) -- so a take read at another rate is not counted
/// again by every edit either.
#[derive(Debug)]
pub struct ReadList {
    tree: Tree,
    files: Arc<BTreeMap<SourceId, TakeFile>>,
    layouts: HashMap<SourceId, Arc<Layout>>,
    rate: f64,
    list_rate: f64,
    channels: usize,
    counts: Arc<Counts>,
}

/// **What each node of a take fills at a rate other than its list's**, by
/// the node's identity: kept by whoever reads one take after another (the
/// engine an application holds), since the nodes an edit did not make are
/// the ones the take before it had. A node's identity is never reused.
#[derive(Debug, Default)]
pub struct Counts {
    filled: Mutex<HashMap<u64, u64>>,
}

/// Nodes whose count is kept at most; past it they are counted again.
const COUNTS_HELD: usize = 1 << 20;

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
        let mut files: BTreeMap<SourceId, TakeFile> = BTreeMap::new();
        let mut ids: HashMap<&str, SourceId> = HashMap::new();
        let counted = parts.iter().map(|part| {
            let next = SourceId(ids.len() as u64);
            let id = *ids.entry(part.path.as_str()).or_insert(next);
            files.entry(id).or_insert_with(|| TakeFile {
                path: part.path.as_str().into(),
                peaks: None,
                channels: part.source_channels,
                frames: part.source_frames,
                rate: part.source_rate,
            });
            let read = Part {
                source: SourceRef {
                    source: id,
                    lifetime: Lifetime::Temporary,
                    generation: 0,
                    range: Some(Range {
                        start: part.start,
                        end: part.start + part.frames,
                    }),
                },
                fade_in: part.fade_in,
                fade_out: part.fade_out,
                fade_in_shape: part.fade_in_shape,
                fade_out_shape: part.fade_out_shape,
                channels: part.channels.clone(),
            };
            // a segment part's frames are the list's already
            (read, part.frames)
        });
        let tree = Tree::from_counted(counted.collect::<Vec<_>>());
        let list = FileList {
            channels: channels as u32,
            rate,
            tree,
            files: Arc::new(files),
        };
        ReadList::over(&list, rate, headers, &Arc::default())
    }

    /// **`list` read at `rate`**, each file's header read once through
    /// `headers`, what its nodes fill at `rate` kept in `counts`.
    pub fn over(
        list: &FileList,
        rate: f64,
        headers: &mut Headers,
        counts: &Arc<Counts>,
    ) -> Result<ReadList, String> {
        let mut layouts = HashMap::new();
        for (id, file) in list.files.iter() {
            layouts.insert(*id, headers.layout(&file.path)?);
        }
        Ok(ReadList {
            tree: list.tree.clone(),
            files: Arc::clone(&list.files),
            layouts,
            rate,
            list_rate: list.rate,
            channels: list.channels.max(1) as usize,
            counts: Arc::clone(counts),
        })
    }

    /// How many frames the take comes to, at the rate it is read.
    pub fn frames(&self) -> u64 {
        self.tree.root().map_or(0, |root| self.filled(root))
    }

    /// How many channels it is.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// **The frames `node` fills at the rate it is read**: the tree's own
    /// count at the list's rate, and at another each part's span at the
    /// ratio, summed and kept.
    fn filled(&self, node: &Node) -> u64 {
        if self.rate == self.list_rate {
            return node.frames();
        }
        if let Some(n) = self
            .counts
            .filled
            .lock()
            .ok()
            .and_then(|f| f.get(&node.id()).copied())
        {
            return n;
        }
        let n = match (node.part(), node.children()) {
            (Some(part), _) => {
                let span = part.source.range.map_or(0, |r| r.len());
                let from = self
                    .files
                    .get(&part.source.source)
                    .map_or(self.rate, |f| f.rate);
                contributed(span, from, self.rate)
            }
            (None, Some((l, r))) => self.filled(l) + self.filled(r),
            (None, None) => 0,
        };
        if let Ok(mut filled) = self.counts.filled.lock() {
            if filled.len() >= COUNTS_HELD {
                filled.clear();
            }
            filled.insert(node.id(), n);
        }
        n
    }

    /// **The parts frames `a..b` cross**, each with where it starts and how
    /// many frames it fills at the rate it is read.
    fn crossing<'a>(
        &self,
        node: &'a Node,
        start: u64,
        (a, b): (u64, u64),
        out: &mut Vec<(u64, u64, &'a Part)>,
    ) {
        let end = start + self.filled(node);
        if end <= a || start >= b {
            return;
        }
        match (node.part(), node.children()) {
            (Some(part), _) => out.push((start, end - start, part)),
            (None, Some((l, r))) => {
                let mid = start + self.filled(l);
                self.crossing(l, start, (a, b), out);
                self.crossing(r, mid, (a, b), out);
            }
            (None, None) => {}
        }
    }

    /// One part where it is read: its span over its file, and the file's
    /// layout.
    fn placed(&self, start: u64, frames: u64, part: &Part) -> Option<(PartSpan, &Arc<Layout>)> {
        let id = part.source.source;
        let (file, layout) = (self.files.get(&id)?, self.layouts.get(&id)?);
        let range = part.source.range?;
        let map = (0..self.channels)
            .map(|c| match &part.channels {
                Some(map) => map.get(c).copied().unwrap_or(-1),
                None => (c % file.channels.max(1) as usize) as i32,
            })
            .collect();
        let cut = Cut {
            src_start: range.start as usize,
            frames: frames as usize,
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
        let ratio = rate_between(file.rate, self.rate).unwrap_or(1.0);
        let span = PartSpan::new(cut, layout.frames as usize, ratio, start as usize);
        Some((span, layout))
    }

    /// Renders `n` frames of channel `channel` from `at` into `out`; a span
    /// past the end, or of a file that does not read, is silence.
    fn render(&self, at: u64, n: usize, channel: usize, out: &mut Vec<f32>, files: &mut Files) {
        out.clear();
        out.resize(n, 0.0);
        let Some(root) = self.tree.root() else {
            return;
        };
        let mut parts = Vec::new();
        self.crossing(root, 0, (at, at + n as u64), &mut parts);
        let at = at as usize;
        for (start, frames, part) in parts {
            let Some((part, layout)) = self.placed(start, frames, part) else {
                continue;
            };
            let from = part.start().max(at);
            let k = from - at;
            let inner = from - part.start();
            let len = (part.end() - from).min(n - k);
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
struct Handed(AtomicPtr<ReadList>);

impl Handed {
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

impl Drop for Handed {
    fn drop(&mut self) {
        drop(self.take());
    }
}

/// **One take's reading**: a mono fed stream per channel, and the thread of
/// the application that renders the take into them.
pub struct TakeFeed {
    streams: Vec<Arc<Fed>>,
    placed: Arc<Handed>,
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
        let placed = Arc::new(Handed::default());
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
fn serve(mut feeders: Vec<Feeder>, placed: Arc<Handed>, stop: Arc<AtomicBool>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::parts::Rates;

    /// A mono float file at `rate`, each frame told apart.
    fn file(name: &str, rate: u32, frames: usize) -> String {
        let path = std::env::temp_dir().join(format!("take-{name}-{}.wav", std::process::id()));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for f in 0..frames {
            w.write_sample(0.25 + f as f32 / (1 << 20) as f32).unwrap();
        }
        w.finalize().unwrap();
        path.to_string_lossy().into_owned()
    }

    fn part(start: u64, end: u64) -> Part {
        Part {
            source: SourceRef {
                source: SourceId(7),
                lifetime: Lifetime::Temporary,
                generation: 0,
                range: Some(Range { start, end }),
            },
            fade_in: 0,
            fade_out: 0,
            fade_in_shape: Default::default(),
            fade_out_shape: Default::default(),
            channels: None,
        }
    }

    fn read(list: &ReadList) -> Vec<f32> {
        let mut files = Files {
            open: Vec::new(),
            span: Vec::new(),
        };
        let mut out = Vec::new();
        list.render(0, list.frames() as usize, 0, &mut out, &mut files);
        out
    }

    /// **A take read at a rate other than its file's** is the same take
    /// counted part by part at that rate -- what the flat list gave -- and
    /// an edit that shares its nodes is counted only where it made new ones.
    #[test]
    fn a_take_read_at_another_rate_is_counted_once_per_node() {
        let path = file("rate", 44_100, 20_000);
        let parts = [part(0, 4_410), part(10_000, 14_410), part(2_000, 6_000)];
        let rates = std::collections::BTreeMap::new();
        let tree = Tree::from_parts(
            &parts,
            &Rates {
                join: 44_100.0,
                of: &rates,
            },
        )
        .unwrap();
        let files: BTreeMap<SourceId, TakeFile> = [(
            SourceId(7),
            TakeFile {
                path: path.as_str().into(),
                peaks: None,
                channels: 1,
                frames: 20_000,
                rate: 44_100.0,
            },
        )]
        .into();
        let list = FileList {
            channels: 1,
            rate: 44_100.0,
            tree,
            files: Arc::new(files),
        };
        let counts = Arc::new(Counts::default());
        let mut headers = Headers::default();
        let over = ReadList::over(&list, 48_000.0, &mut headers, &counts).unwrap();
        // the flat list at 48 kHz, each part's frames counted at the ratio
        let flat: Vec<SegmentPart> = parts
            .iter()
            .map(|p| {
                let r = p.source.range.unwrap();
                SegmentPart {
                    path: path.clone(),
                    start: r.start,
                    frames: contributed(r.len(), 44_100.0, 48_000.0),
                    fade_in: 0,
                    fade_out: 0,
                    fade_in_shape: Default::default(),
                    fade_out_shape: Default::default(),
                    channels: None,
                    source_channels: 1,
                    source_frames: 20_000,
                    source_rate: 44_100.0,
                }
            })
            .collect();
        let of = ReadList::of(&flat, 1, 48_000.0, &mut headers).unwrap();
        assert_eq!(over.frames(), of.frames());
        assert_eq!(read(&over), read(&of));
        // every node counted once
        let counted = counts.filled.lock().unwrap().len();
        assert_eq!(counted, 2 * parts.len() - 1);
        // an edit sharing the take's nodes counts the few it made
        let edited = FileList {
            tree: list
                .tree
                .insert(
                    0,
                    &list.tree,
                    &Rates {
                        join: 44_100.0,
                        of: &rates,
                    },
                )
                .unwrap(),
            ..list.clone()
        };
        let again = ReadList::over(&edited, 48_000.0, &mut headers, &counts).unwrap();
        assert_eq!(again.frames(), 2 * of.frames());
        let made = counts.filled.lock().unwrap().len() - counted;
        assert!(made <= 4, "the edit was counted again: {made} nodes");
        let _ = std::fs::remove_file(&path);
    }
}
