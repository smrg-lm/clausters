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
//! A multitrack's track is read the same way, from its regions rather than
//! from parts back to back ([`PlacedRead`]): each region is a window onto
//! its source placed on the track, read through the core's
//! [`RegionSpan::read_with`], and the regions that cross a frame are summed.
//! What a reading thread renders is either ([`TakeRead`]).
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
use clausters_core::regions::RegionSpan;
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
        let map = clausters_core::segments::channel_map(
            part.channels.as_deref(),
            file.channels as usize,
            self.channels,
        );
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

/// **What a region reads**, as its reader holds it: a file's layout, or a
/// join's parts read as a take of their own.
#[derive(Debug)]
enum Source {
    File(Arc<Layout>),
    Parts(ReadList),
}

/// **A multitrack's track as it is read**: its regions, each a window onto
/// a source placed on the track's frames, and what each source is.
///
/// A frame is the sum of the regions that cross it. They are kept by where
/// they start, with the longest one's length, so the regions that cross a
/// span are found by two searches and read one by one: a track of
/// thousands of regions costs, per turn, the regions under the span.
#[derive(Debug)]
pub struct PlacedRead {
    regions: Vec<RegionSpan>,
    sources: Vec<Arc<Source>>,
    longest: u64,
    channels: usize,
    frames: u64,
}

impl PlacedRead {
    /// **`list` read as a track `channels` wide**, each file's header read
    /// once through `headers`, a join's parts read at the join's own rate
    /// (each region's step counts the source's rate). A file that cannot be
    /// read is a refusal naming it; a region whose source the list does not
    /// hold is silent.
    pub fn over(
        list: &clausters_apps::editing::held::PlacedList,
        channels: usize,
        headers: &mut Headers,
        counts: &Arc<Counts>,
    ) -> Result<PlacedRead, String> {
        use clausters_apps::editing::held::PlacedSource;
        let mut read: HashMap<SourceId, (Arc<Source>, usize)> = HashMap::new();
        for (id, source) in list.sources.iter() {
            let (source, frames) = match source {
                PlacedSource::File(file) => {
                    let layout = headers.layout(&file.path)?;
                    let frames = layout.frames as usize;
                    (Source::File(layout), frames)
                }
                PlacedSource::Parts(parts) => {
                    let parts = ReadList::over(parts, parts.rate, headers, counts)?;
                    let frames = parts.frames() as usize;
                    (Source::Parts(parts), frames)
                }
            };
            read.insert(*id, (Arc::new(source), frames));
        }
        let mut regions = Vec::new();
        let mut sources = Vec::new();
        for placed in list.regions.iter() {
            let Some((source, frames)) = read.get(&placed.source) else {
                continue;
            };
            let span = RegionSpan::new(placed.cut.clone(), *frames);
            if span.end() > span.first() {
                regions.push(span);
                sources.push(Arc::clone(source));
            }
        }
        let mut order: Vec<usize> = (0..regions.len()).collect();
        order.sort_by_key(|&i| regions[i].first());
        let regions: Vec<RegionSpan> = order.iter().map(|&i| regions[i].clone()).collect();
        let sources: Vec<Arc<Source>> = order.iter().map(|&i| Arc::clone(&sources[i])).collect();
        let longest = regions
            .iter()
            .map(|r| r.end() - r.first())
            .max()
            .unwrap_or(0);
        Ok(PlacedRead {
            regions,
            sources,
            longest,
            channels: channels.max(1),
            frames: list.frames,
        })
    }

    /// How many frames the track comes to.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// How many channels it is.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// **Frames `at..at + n` of channel `channel`**, read now on the
    /// caller's thread: what a measure or a test reads of a track.
    pub fn read(&self, at: u64, n: usize, channel: usize) -> Vec<f32> {
        let mut files = Files {
            open: Vec::new(),
            span: Vec::new(),
        };
        let mut out = Vec::new();
        self.render(at, n, channel, &mut out, &mut files);
        out
    }

    /// The regions that cross frames `a..b`, by index.
    fn crossing(&self, a: u64, b: u64) -> impl Iterator<Item = usize> + '_ {
        let low = a.saturating_sub(self.longest);
        let from = self.regions.partition_point(|r| r.first() < low);
        let to = self.regions.partition_point(|r| r.first() < b);
        (from..to).filter(move |&i| self.regions[i].end() > a)
    }

    /// **Renders `n` frames of channel `channel` from `at` into `out`**:
    /// every region that crosses them, summed; silence where none does.
    fn render(&self, at: u64, n: usize, channel: usize, out: &mut Vec<f32>, files: &mut Files) {
        out.clear();
        out.resize(n, 0.0);
        let end = at + n as u64;
        let mut fetched: Vec<f32> = Vec::new();
        for i in self.crossing(at, end) {
            let region = &self.regions[i];
            let Some(&src_ch) = region.cut().map.get(channel) else {
                continue;
            };
            if src_ch < 0 {
                continue;
            }
            let src_ch = src_ch as usize;
            let (lo, hi) = (region.first().max(at), region.end().min(end));
            let mut k = lo;
            while k < hi {
                let run = region.run_end(k, hi);
                let (s0, s1) = region.source_span(k, run);
                // the first frame, where a loop's last frame reads towards it
                let head = if region.cut().looping && s1 >= region.src_frames() && s0 > 0 {
                    self.fetch(i, src_ch, 0, 1, &mut fetched, files);
                    fetched.first().copied().unwrap_or(0.0)
                } else {
                    0.0
                };
                self.fetch(i, src_ch, s0, s1, &mut fetched, files);
                for j in k..run {
                    out[(j - at) as usize] += region.read_with(j, channel, |f, _| {
                        if f >= s0 && f < s1 {
                            fetched.get(f - s0).copied().unwrap_or(0.0)
                        } else if f == 0 {
                            head
                        } else {
                            0.0
                        }
                    });
                }
                k = run;
            }
        }
    }

    /// **Source frames `s0..s1` of region `i`'s source, channel `channel`**,
    /// into `into`, one sample a frame; what does not read is silence.
    fn fetch(
        &self,
        i: usize,
        channel: usize,
        s0: usize,
        s1: usize,
        into: &mut Vec<f32>,
        files: &mut Files,
    ) {
        into.clear();
        if s1 <= s0 {
            return;
        }
        match &*self.sources[i] {
            Source::File(layout) => {
                let width = layout.channels.max(1);
                let span = files.read(layout, s0 as u64, s1 - s0);
                into.extend(
                    (0..s1 - s0).map(|f| span.get(f * width + channel).copied().unwrap_or(0.0)),
                );
            }
            Source::Parts(parts) => {
                let channel = channel.min(parts.channels().saturating_sub(1));
                parts.render(s0 as u64, s1 - s0, channel, into, files);
            }
        }
    }
}

/// **What a reading thread renders**: a take's parts back to back, or a
/// track's regions placed on it.
#[derive(Debug, Clone)]
pub enum TakeRead {
    /// An audio editor's take.
    Parts(Arc<ReadList>),
    /// A multitrack's track.
    Placed(Arc<PlacedRead>),
}

impl From<Arc<ReadList>> for TakeRead {
    fn from(list: Arc<ReadList>) -> Self {
        TakeRead::Parts(list)
    }
}

impl From<Arc<PlacedRead>> for TakeRead {
    fn from(list: Arc<PlacedRead>) -> Self {
        TakeRead::Placed(list)
    }
}

impl TakeRead {
    /// How many frames it comes to.
    pub fn frames(&self) -> u64 {
        match self {
            TakeRead::Parts(list) => list.frames(),
            TakeRead::Placed(list) => list.frames(),
        }
    }

    /// How many channels it is.
    pub fn channels(&self) -> usize {
        match self {
            TakeRead::Parts(list) => list.channels(),
            TakeRead::Placed(list) => list.channels(),
        }
    }

    fn render(&self, at: u64, n: usize, channel: usize, out: &mut Vec<f32>, files: &mut Files) {
        match self {
            TakeRead::Parts(list) => list.render(at, n, channel, out, files),
            TakeRead::Placed(list) => list.render(at, n, channel, out, files),
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
struct Handed(AtomicPtr<TakeRead>);

impl Handed {
    /// Places `list`, dropping one the reader had not taken yet.
    fn put(&self, list: Arc<TakeRead>) {
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
    fn take(&self) -> Option<Arc<TakeRead>> {
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
    pub fn set(&self, list: impl Into<TakeRead>) {
        self.placed.put(Arc::new(list.into()));
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
    let mut list: Option<Arc<TakeRead>> = None;
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
