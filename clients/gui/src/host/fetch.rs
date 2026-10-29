//! The server-buffer fetch state machine, shared by the native and browser
//! fronts.
//!
//! A `waveform` or `spectrogram` that references a server `buffer` pulls its
//! picture over the client leg: `/buffer_query` for the shape, then its
//! **summary** (`/buffer_peaks`, or the `/buffer_stream` reports of a take
//! being written), and only the **span under the eye** as samples, in chunked
//! `/buffer_getRange` requests whose replies fill a flat interleaved run
//! keeping every channel. A take is never downloaded whole, whatever its
//! size: a summary is a fraction of the samples, so a picture, and the picture
//! after an edit, arrive in a few replies (`docs/decisions.md`, "A host that
//! cannot map draws every take from its summary"). The protocol conversation is
//! transport- and platform-independent -- only *sending* the returned messages
//! and *placing* what arrived differ per front -- so the whole machine lives
//! here, pure and unit-testable without a GPU or a socket. Reassembly is by
//! the reply's explicit `start` offset.

use std::collections::HashMap;
use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscType};

use crate::host::widget::element::SlotKey;

/// Samples per `/buffer_getRange` request when pulling a server buffer: **8 kB
/// of reply**, and the number is the carrier's rather than the buffer's.
///
/// The smallest carrier a reply crosses is the shared ring, 64 KiB, and it is
/// **lossy under pressure by design**: a server whose reply ring is full drops
/// the reply rather than blocking (`osc::server`, "reply ring full"). So the
/// chunk is not a throughput knob, it is what decides how many replies can be
/// in the air at once without any of them being thrown away -- four of these
/// fit, where the old 32 kB chunk let two lanes fill the ring between them and
/// every other lane's answer went missing.
///
/// It is a **trade against round trips**, and that is the reason it is not
/// smaller: a chunk is one request and one reply, so halving it doubles how
/// many frames a span takes to arrive. Small enough that nothing is dropped,
/// large enough that the span under a zoom lands in a few frames.
pub(crate) const BUFFER_CHUNK: usize = 4096;

/// **How many buffers may be downloading at once.** The bound is the carrier's
/// too: our own traffic must not be able to fill the reply ring, or the drops
/// are self-inflicted and a session that zooms four lanes at once loses three
/// of the four answers.
///
/// Nothing refused here fails: a view re-reads the span it cannot draw every
/// time it draws, and an announced edit waits for the buffer's span in flight
/// ([`BufferFetches::queued_span`]). So the lanes resolve one after another
/// instead of racing and losing.
const MAX_IN_FLIGHT: usize = 3;

/// **How many chunk replies may be in the air at once**, over every download
/// and span: the same ring bound as [`MAX_IN_FLIGHT`], counted in what
/// actually fills the ring. A download alone keeps this many requests out;
/// several share them, the one with the fewest out asking first.
///
/// One at a time, a span took a round trip per chunk, and a round trip is a
/// frame or so: the bytes are not the cost, the waiting between them is.
const CHUNKS_IN_FLIGHT: usize = 3;

/// How many frames a conversation may go **without landing anything** before
/// it is treated as lost and asked for again. [`BufferFetches::tick`] runs once
/// per draw, so this is about half a second at sixty frames -- long enough that
/// a working conversation is never restarted between two of its replies, short
/// enough that a dropped one is a hesitation rather than the seconds it used
/// to be.
const STALLED_ASKS: usize = 30;

/// **How many times a buffer the server says does not exist is asked for
/// again** before the want is let go of -- about twenty seconds at thirty
/// frames, with [`STALLED_ASKS`] between two asks.
///
/// Absence is usually *not yet*: a client names a buffer in the same turn it
/// sends the command that makes it (a join's stitch, an allocation, a read),
/// and those complete on the NRT thread after the query is answered. So the
/// first answer is asked again rather than believed. A buffer that stays absent
/// was freed, and holding its want for the session would keep a front awake for
/// nothing.
const ABSENT_ASKS: usize = 20;

/// A widget waiting on a server buffer fetch. What to build from the finished
/// samples is read off the widget's kind at completion, so the machine carries
/// no per-kind parameters.
#[derive(Clone, Copy)]
pub(crate) struct WaveWant {
    pub def_id: i32,
    pub widget_id: i32,
    /// Whether this widget asked for the buffer's **shape** rather than its
    /// samples -- a take being recorded into, whose picture is filled by the
    /// overview the server streams and whose samples are silence until then.
    pub shape_only: bool,
}

/// An in-progress fetch of a server buffer: the flat interleaved samples
/// filled in as `/buffer_getRange.reply` chunks arrive.
struct BufferFetch {
    channels: usize,
    /// Where in the buffer this span starts, as a **flat** index.
    origin: usize,
    /// How many samples are being downloaded from `origin` (flat).
    total: usize,
    samples: Vec<f32>,
    received: usize,
    /// Each chunk's state, by index from `origin` in [`BUFFER_CHUNK`]s.
    chunks: Vec<ChunkState>,
    /// How many of them are asked and not answered.
    in_flight: usize,
    /// How many frames this download has gone with **no progress** -- the clock
    /// is [`BufferFetches::tick`], and every reply that lands anything resets
    /// it.
    stalled: usize,
    /// What `received` was at the last tick, which is what says whether
    /// anything landed since.
    stalled_at: usize,
    /// The frame the span starts at, and who it is for.
    window: (usize, SpanUse),
}

/// Where one chunk of a download is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChunkState {
    Unasked,
    Asked,
    Landed,
}

impl FetchStep {
    /// The step that sends `asked`: nothing, one request or several.
    fn of(mut asked: Vec<OscMessage>) -> Self {
        match asked.len() {
            0 => Self::None,
            1 => Self::Request(asked.remove(0)),
            _ => Self::Requests(asked),
        }
    }
}

impl BufferFetch {
    /// A read of `total` samples from flat index `origin`, at frame `start`
    /// for `span`, nothing asked.
    fn new(channels: usize, origin: usize, total: usize, window: (usize, SpanUse)) -> Self {
        Self {
            channels,
            origin,
            total,
            samples: vec![0.0; total],
            received: 0,
            chunks: vec![ChunkState::Unasked; total.div_ceil(BUFFER_CHUNK)],
            in_flight: 0,
            stalled: 0,
            stalled_at: 0,
            window,
        }
    }

    /// **Asks for the next chunk** not asked yet, if there is one.
    fn ask_one(&mut self, bufnum: i32) -> Option<OscMessage> {
        let i = self
            .chunks
            .iter()
            .position(|state| *state == ChunkState::Unasked)?;
        self.chunks[i] = ChunkState::Asked;
        self.in_flight += 1;
        Some(request_chunk(
            bufnum,
            self.origin + i * BUFFER_CHUNK,
            self.origin + self.total,
        ))
    }

    /// Whether a chunk is left to ask for.
    fn wants_more(&self) -> bool {
        self.chunks.contains(&ChunkState::Unasked)
    }
}

/// **Why a span of a buffer is being read back.** The same request serves two
/// readers, and what they do with the answer is opposite: a zoom puts the run
/// under the summary and leaves the summary alone, while an edit *replaces*
/// what the summary says over that span.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SpanUse {
    /// A view zoomed past what its summary can answer, and which view it is:
    /// two views of one take are at two zooms over two spans, so a window is
    /// one view's and names it.
    Window {
        def_id: i32,
        widget_id: i32,
        /// Which of the widget's pictures ([`SlotKey`]).
        key: SlotKey,
    },
    /// **Another peer wrote here and said so** (`/buffer_touched`). The span
    /// comes back for every view of that buffer, not only the one that asked:
    /// what arrives is the buffer's own samples, so any picture of it is
    /// entitled to them, and one request serves them all.
    Patch,
}

/// What one protocol step asks the driving front to do.
pub(crate) enum FetchStep {
    /// Send this message to the audio server (the next `/buffer_getRange`).
    Request(OscMessage),
    /// Send these, in order: the chunks a download asks for at once.
    Requests(Vec<OscMessage>),
    /// A buffer finished downloading: its interleaved samples and shape, ready
    /// for the waiting widgets. `wants` may be empty if every waiting window
    /// closed meanwhile.
    Done {
        bufnum: i32,
        samples: Arc<[f32]>,
        channels: usize,
        sample_rate: f64,
        wants: Vec<WaveWant>,
    },
    /// A **span** of a buffer arrived, for the one view that asked: put it
    /// under that view's summary as a window.
    Window {
        bufnum: i32,
        want: WaveWant,
        /// Which of the asker's pictures ([`SlotKey`]).
        key: SlotKey,
        start_frame: usize,
        channels: usize,
        samples: Vec<f32>,
    },
    /// **A span another peer wrote** arrived: put it into every view of that
    /// buffer -- samples and the summary over them -- and redraw.
    ///
    /// Unlike [`FetchStep::Window`] this carries no want: the samples are the
    /// buffer's, so whoever draws it takes them.
    Patch {
        bufnum: i32,
        start_frame: usize,
        channels: usize,
        samples: Vec<f32>,
    },
    /// A buffer answered with its **shape**: build an empty summary of this
    /// length and place it, with no samples fetched.
    ///
    /// `ask_summary` says where the summary that fills it comes from. A take
    /// being *written* is `false` -- nothing can be asked for what is not there
    /// yet, and the server pushes it (`/buffer_stream`) as it appears. A take
    /// that stands still is `true`: the front asks for it (`/buffer_peaks`),
    /// which is the same blob folded the same way.
    Empty {
        bufnum: i32,
        frames: usize,
        channels: usize,
        sample_rate: f64,
        ask_summary: bool,
        wants: Vec<WaveWant>,
    },
    /// Nothing to do (an unsolicited or stale reply).
    None,
}

/// The fetch bookkeeping: which widgets wait on which buffer, and each
/// buffer's in-progress download.
#[derive(Default)]
pub(crate) struct BufferFetches {
    /// Widgets awaiting each server buffer number.
    wants: HashMap<i32, Vec<WaveWant>>,
    /// In-progress downloads, by buffer number.
    fetches: HashMap<i32, BufferFetch>,
    /// **Announced edits that arrived while that buffer was busy**, as the one
    /// span covering them, by buffer number. Drained by [`Self::queued_span`]
    /// when the download in flight finishes.
    queued: HashMap<i32, (usize, usize)>,
    /// The channel count each queued span was announced with.
    channels: HashMap<i32, usize>,
    /// **Buffers the server answered as absent** (`frames = -1`), by buffer
    /// number: frames since the last ask, and how many asks have gone out.
    /// Asked again by [`Self::tick`]; see [`ABSENT_ASKS`].
    absent: HashMap<i32, (usize, usize)>,
    /// **The summary conversations**: the walks over a take at the level its
    /// views draw, and the finer grids one view's zoom asks for, at most
    /// [`PEAKS_IN_FLIGHT`] of them answering at once.
    peaks: Vec<PeaksAsk>,
    /// **Takes a plural asker is owed the summary of** -- a multitrack's
    /// clips -- waiting for the asker to be drawn, since its
    /// width is what says the level: `(def_id, widget_id, bufnum, shape)`.
    owed: Vec<(i32, i32, i32, crate::host::widget::element::OwedSummary)>,
    /// **The spectrogram columns a view of a list is owed** over the span it
    /// shows (`super::columns`).
    pub(crate) columns: super::columns::ColumnAsks,
}

/// **How many summary conversations may be answering at once.** A reply is at
/// most 8 KiB (`MAX_STREAM_BYTES` on the server), so two of them beside the
/// [`CHUNKS_IN_FLIGHT`] chunk replies fill the 64 KiB ring and no more; a third
/// waits for one of them to finish rather than for the ring to drop it.
const PEAKS_IN_FLIGHT: usize = 2;

/// What a summary conversation is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PeaksFor {
    /// **The take's summary at the level its views draw**, over the whole of
    /// it, for every view of that buffer at that level.
    Summary,
    /// **A finer grid over the span one view is showing**, for that view.
    Detail {
        def_id: i32,
        widget_id: i32,
        /// Which of the widget's pictures ([`SlotKey`]).
        key: SlotKey,
    },
}

/// **A summary asked for, over a span at one bucket**: the take's own at the
/// level its views draw, or the finer grid one view's zoom wants.
///
/// One request out at a time, each asking from where the last answer ended,
/// and what came back collected here until the span is covered -- so a view
/// is handed the whole grid at once and never draws half of one over the
/// picture it had. It lives here rather than in a front because it needs what
/// the downloads need: a reply that never arrives must not stop it, and a
/// carrier that is allowed to lose one would otherwise leave the view at the
/// picture it had with nothing to notice it.
struct PeaksAsk {
    bufnum: i32,
    bucket: usize,
    /// The span, in frames.
    start: usize,
    end: usize,
    /// The frame the next answer begins at.
    next: usize,
    channels: usize,
    /// The take's rate, which a summary hands on to the views it fills.
    sample_rate: f64,
    /// What came back, bucket-major from `start`, as the wire carries it.
    stats: Vec<f32>,
    /// Whether a request is out, and frames since it went (in ticks of
    /// [`BufferFetches::tick`]).
    asked: bool,
    stalled: usize,
    purpose: PeaksFor,
}

impl PeaksAsk {
    fn request(&self) -> OscMessage {
        detail_request(self.bufnum, self.bucket, self.next, self.end - self.next)
    }
}

/// **A summary conversation that has covered its span**: the grid, whole.
#[derive(Debug)]
pub(crate) struct PeaksDone {
    pub bufnum: i32,
    pub bucket: usize,
    pub start: usize,
    /// One past the last frame the grid covers -- the take's length, for a
    /// summary.
    pub end: usize,
    pub channels: usize,
    pub sample_rate: f64,
    /// Bucket-major and channel-minor from `start`: min, max and mean square.
    pub stats: Vec<f32>,
    pub purpose: PeaksFor,
}

impl BufferFetches {
    /// Registers a widget waiting on `bufnum`. Returns the `/buffer_query` to send
    /// the first time a buffer is wanted (`None` when a query or download for
    /// it is already under way -- the widget just joins the wait).
    pub(crate) fn want(&mut self, def_id: i32, widget_id: i32, bufnum: i32) -> Option<OscMessage> {
        self.register(def_id, widget_id, bufnum, false)
    }

    /// Registers a widget waiting on `bufnum`'s **shape** -- a take being
    /// recorded into. The conversation is the same `/buffer_query`; what
    /// changes is that the reply finishes it, with no samples pulled.
    ///
    /// A buffer wanted both ways downloads: one view being told about a
    /// recording does not excuse another that has to draw the samples.
    pub(crate) fn want_shape(
        &mut self,
        def_id: i32,
        widget_id: i32,
        bufnum: i32,
    ) -> Option<OscMessage> {
        self.register(def_id, widget_id, bufnum, true)
    }

    fn register(
        &mut self,
        def_id: i32,
        widget_id: i32,
        bufnum: i32,
        shape_only: bool,
    ) -> Option<OscMessage> {
        let first = !self.wants.contains_key(&bufnum);
        self.wants.entry(bufnum).or_default().push(WaveWant {
            def_id,
            widget_id,
            shape_only,
        });
        (first && !self.fetches.contains_key(&bufnum)).then(|| OscMessage {
            addr: "/buffer_query".into(),
            args: vec![OscType::Int(bufnum)],
        })
    }

    /// Whether anything is still waiting on the server.
    ///
    /// A front that polls its link (rather than being woken by a thread) reads
    /// this to know it must keep looking: an event-driven loop that sleeps
    /// until the next input would leave a reply sitting in the ring, and the
    /// window would fill in only when the pointer happened to move.
    ///
    /// Both fronts ask, for the same reason under two names: the native loop
    /// schedules a wake-up, and the browser canvas asks for another animation
    /// frame -- either way, a conversation under way is what keeps the front
    /// looking.
    /// The windows waiting on buffer `bufnum`, each once.
    pub(crate) fn waiters_of(&self, bufnum: i32) -> Vec<i32> {
        let mut defs: Vec<i32> = self
            .wants
            .get(&bufnum)
            .map(|wants| wants.iter().map(|w| w.def_id).collect())
            .unwrap_or_default();
        defs.sort_unstable();
        defs.dedup();
        defs
    }

    /// Whether a download is still owed to window `def_id`.
    pub(crate) fn pending_for(&self, def_id: i32) -> bool {
        self.wants
            .values()
            .any(|wants| wants.iter().any(|w| w.def_id == def_id))
    }

    pub(crate) fn pending(&self) -> bool {
        !self.wants.is_empty()
            || !self.fetches.is_empty()
            || !self.absent.is_empty()
            || !self.peaks.is_empty()
            || self.columns.pending()
    }

    /// `/buffer_query.reply` for a buffer we are waiting on: the shape its
    /// summary is built at, or an empty take at once.
    pub(crate) fn on_info(
        &mut self,
        bufnum: i32,
        frames: usize,
        channels: usize,
        sample_rate: f64,
    ) -> FetchStep {
        if !self.wants.contains_key(&bufnum) || self.fetches.contains_key(&bufnum) {
            return FetchStep::None;
        }
        self.absent.remove(&bufnum);
        let channels = channels.max(1);
        let total = frames * channels;
        if total == 0 {
            return self.finish(bufnum, Vec::new(), channels, sample_rate);
        }
        // **Every take is drawn from its summary**, whatever its size: a
        // stand-still one asks for it, one being written is pushed it.
        let being_written = self
            .wants
            .get(&bufnum)
            .is_some_and(|w| w.iter().all(|want| want.shape_only));
        FetchStep::Empty {
            bufnum,
            frames,
            channels,
            sample_rate,
            ask_summary: !being_written,
            wants: self.wants.remove(&bufnum).unwrap_or_default(),
        }
    }

    /// `/buffer_query.reply` for a buffer the server **does not have**
    /// (`frames = -1`): the want stays, nothing is handed to anyone, and the
    /// buffer is asked for again on the frame clock.
    ///
    /// The protocol states absence apart from emptiness, and the two used to be
    /// one at the door: an unallocated answer became a buffer of no frames, the
    /// waiting view was handed an empty take in place of whatever it drew, and
    /// the take was never asked for again. A join a client mints is exactly
    /// that -- its box names the buffer in the turn the stitch is sent, and the
    /// stitch lands on the NRT thread after the query is answered -- so the
    /// joined box drew nothing, for good.
    pub(crate) fn on_absent(&mut self, bufnum: i32) {
        if !self.wants.contains_key(&bufnum) || self.fetches.contains_key(&bufnum) {
            return;
        }
        self.absent.entry(bufnum).or_insert((0, 0));
    }

    /// The chunk replies in the air, over every download and span.
    fn in_flight(&self) -> usize {
        self.fetches.values().map(|fetch| fetch.in_flight).sum()
    }

    /// **The chunks to ask for now**, until [`CHUNKS_IN_FLIGHT`] are out: one
    /// at a time to whichever conversation has the fewest out (the lowest
    /// buffer first among equals), so a take alone keeps the carrier full
    /// and several take turns rather than one starving the rest.
    fn ask(&mut self) -> Vec<OscMessage> {
        let mut out = Vec::new();
        while self.in_flight() < CHUNKS_IN_FLIGHT {
            let Some((&bufnum, fetch)) = self
                .fetches
                .iter_mut()
                .filter(|(_, fetch)| fetch.wants_more())
                .min_by_key(|(bufnum, fetch)| (fetch.in_flight, **bufnum))
            else {
                break;
            };
            out.extend(fetch.ask_one(bufnum));
        }
        out
    }

    /// `/buffer_getRange.reply bufnum [start blob]...`: store each range, then
    /// request the next chunks or hand the span over when all of it arrived.
    ///
    /// **The samples come back as a little-endian `f32` blob**, not as float
    /// arguments -- the reply's own length is what actually came back, so no
    /// declared count can disagree with it. A range that carries nothing (an
    /// unallocated buffer, or a read past a buffer shorter than
    /// `/buffer_query.reply` said) ends the download with what has arrived
    /// rather than re-asking for a chunk the server has already declined.
    pub(crate) fn on_data(&mut self, args: &[OscType]) -> FetchStep {
        let [OscType::Int(bufnum), ranges @ ..] = args else {
            return FetchStep::None;
        };
        let bufnum = *bufnum;
        let done = {
            let Some(fetch) = self.fetches.get_mut(&bufnum) else {
                return FetchStep::None;
            };
            let mut matched = 0usize;
            let mut short = false;
            for range in ranges.chunks(2) {
                let [start, OscType::Blob(bytes)] = range else {
                    continue;
                };
                let Some(start) = index_of(start) else {
                    continue;
                };
                // The reply's `start` is the buffer's own flat index; a window
                // stores it where the window begins. A range that falls outside
                // this fetch's span belongs to **another conversation** -- an
                // abandoned one whose reply arrived late -- and is not this
                // fetch's to read or to be ended by.
                let Some(at) = start.checked_sub(fetch.origin) else {
                    continue;
                };
                if at >= fetch.total {
                    continue;
                }
                matched += 1;
                // A chunk asked twice -- once, and again after a stall -- is
                // answered twice, and counted once.
                let state = &mut fetch.chunks[at / BUFFER_CHUNK];
                if *state == ChunkState::Landed {
                    continue;
                }
                if *state == ChunkState::Asked {
                    fetch.in_flight = fetch.in_flight.saturating_sub(1);
                }
                *state = ChunkState::Landed;
                let n = (bytes.len() / 4).min(fetch.total.saturating_sub(at));
                for (i, value) in clausters_core::osc::blob_samples(bytes).take(n).enumerate() {
                    fetch.samples[at + i] = value;
                }
                fetch.received += n;
                short |= n < BUFFER_CHUNK.min(fetch.total - at);
            }
            // **A reply that was not ours changes nothing.** It used to end the
            // download -- "a range that carries nothing ends it with what has
            // arrived" -- which is right for a server that declined a read and
            // wrong for a straggler from a fetch that was restarted: the new
            // one would finish on the spot, and what it had not received yet is
            // zeros nobody measured.
            if matched == 0 {
                return FetchStep::None;
            }
            // A chunk that came back shorter than it was asked for is the
            // end of what the buffer holds -- the server clamps a read to it
            // -- and so is one that carries nothing.
            (short && fetch.in_flight == 0) || fetch.received >= fetch.total
        };
        if done {
            let mut fetch = self.fetches.remove(&bufnum).unwrap();
            // **A span is handed over as far as it actually arrived.** The
            // buffer was sized for the whole request and filled from its
            // origin, so what is past `received` is zeros nothing measured --
            // and a view told it holds them draws silence over a stretch it
            // simply has not read. Truncated, the run answers where it
            // reaches and the summary answers everywhere else, which is what
            // a partial read honestly is.
            let frames = fetch.received / fetch.channels.max(1);
            if frames == 0 {
                return FetchStep::None;
            }
            fetch.samples.truncate(frames * fetch.channels.max(1));
            match fetch.window {
                (start_frame, SpanUse::Patch) => FetchStep::Patch {
                    bufnum,
                    start_frame,
                    channels: fetch.channels,
                    samples: fetch.samples,
                },
                (
                    start_frame,
                    SpanUse::Window {
                        def_id,
                        widget_id,
                        key,
                    },
                ) => FetchStep::Window {
                    bufnum,
                    want: WaveWant {
                        def_id,
                        widget_id,
                        shape_only: false,
                    },
                    key,
                    start_frame,
                    channels: fetch.channels,
                    samples: fetch.samples,
                },
            }
        } else {
            FetchStep::of(self.ask())
        }
    }

    /// **Asks for one span of a buffer**, for the view that has zoomed past
    /// what its summary can answer. `start` and `frames` are per channel.
    ///
    /// Returns the first `/buffer_getRange` to send, or `None` when nothing
    /// should be asked: a span of that buffer is already in flight (one per
    /// buffer is the bound this path keeps), the carrier is full, or the
    /// request is empty.
    ///
    /// A window is **one view's**, not a buffer's: two views of one take are at
    /// two zooms over two spans, and what makes this affordable is that each
    /// asks only for what it is showing.
    pub(crate) fn want_span(
        &mut self,
        bufnum: i32,
        start: usize,
        frames: usize,
        channels: usize,
        span: SpanUse,
    ) -> Option<OscMessage> {
        let channels = channels.max(1);
        if frames == 0 {
            return None;
        }
        // Our own traffic is what fills the reply ring, so it is bounded here
        // rather than apologised for afterwards. A patch still queues (it is
        // announced once); a zoom asks again next frame.
        if !self.fetches.contains_key(&bufnum)
            && (self.fetches.len() >= MAX_IN_FLIGHT || self.in_flight() >= CHUNKS_IN_FLIGHT)
        {
            if span == SpanUse::Patch {
                self.queue(bufnum, start, frames, channels);
            }
            return None;
        }
        if self.fetches.contains_key(&bufnum) {
            // **A patch is kept, a zoom is dropped.** A view that could not
            // read its span asks again on the next frame, so losing one costs
            // a frame; an edit is announced *once*, and a picture that misses
            // it stays wrong until something else happens to that buffer. So
            // the announcement waits for the download in flight, merged with
            // whatever else is waiting into the one span that covers them.
            if span == SpanUse::Patch {
                self.queue(bufnum, start, frames, channels);
            }
            return None;
        }
        let (origin, total) = (start * channels, frames * channels);
        self.fetches.insert(
            bufnum,
            BufferFetch::new(channels, origin, total, (start, span)),
        );
        // the first chunk now; the rest as replies make room
        self.fetches.get_mut(&bufnum)?.ask_one(bufnum)
    }

    /// Remembers an announced edit that could not be asked for yet, merged
    /// with whatever else is waiting on that buffer into the one span covering
    /// them.
    fn queue(&mut self, bufnum: i32, start: usize, frames: usize, channels: usize) {
        let pending = self.queued.entry(bufnum).or_insert((start, frames));
        let end = (pending.0 + pending.1).max(start + frames);
        pending.0 = pending.0.min(start);
        pending.1 = end - pending.0;
        self.channels.insert(bufnum, channels);
    }

    /// **Asks for `bufnum`'s summary at one level**, returning the first
    /// `/buffer_peaks` to send -- `None` when that level is already on its way,
    /// or when the request waits for room.
    ///
    /// `bucket` is the level the views of it draw at (`summary_level`), and
    /// the whole grid is handed back at once ([`PeaksDone`]) for whoever
    /// builds the picture from it; `frames` is how long the take is, which is
    /// what says when it is covered. Two levels of one take are two
    /// conversations, so two windows at two widths do not end each other's.
    pub(crate) fn want_summary(
        &mut self,
        bufnum: i32,
        bucket: usize,
        channels: usize,
        frames: usize,
        sample_rate: f64,
    ) -> Option<OscMessage> {
        if bucket == 0 || frames == 0 {
            return None;
        }
        if self.peaks.iter().any(|a| {
            a.bufnum == bufnum
                && a.bucket == bucket
                && a.end == frames
                && a.purpose == PeaksFor::Summary
        }) {
            return None; // that level of that take is already on its way
        }
        // a walk over the take at another length is over the take before an
        // edit, and would end at its length
        self.peaks.retain(|a| {
            !(a.bufnum == bufnum && a.bucket == bucket && a.purpose == PeaksFor::Summary)
        });
        self.start_peaks(PeaksAsk {
            bufnum,
            bucket,
            start: 0,
            end: frames,
            next: 0,
            channels: channels.max(1),
            sample_rate,
            stats: Vec::new(),
            asked: false,
            stalled: 0,
            purpose: PeaksFor::Summary,
        })
    }

    /// **Asks for a finer grid over the span one view is showing**, or `None`
    /// when that exact grid is already on its way for it (or waits its turn).
    ///
    /// `bucket` is finer than the view's own summary, decided where the zoom
    /// is known (`frame::owed`). The ask is keyed by the view, so a new span
    /// for the same view **replaces** the old one: what is wanted is where the
    /// eye is now, and a reply for a span nobody is looking at any more is
    /// ignored when it lands. It takes as many replies as the span needs.
    ///
    /// **Unless the grid on its way still covers what is on screen** (`seen`),
    /// at the same bucket: a pan asks for a span a few frames further on every
    /// frame it moves, and replacing the grid each time kept it from ever
    /// landing while the hand moved.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn want_detail(
        &mut self,
        bufnum: i32,
        def_id: i32,
        widget_id: i32,
        key: SlotKey,
        (start, frames): (usize, usize),
        seen: (usize, usize),
        bucket: usize,
        channels: usize,
    ) -> Option<OscMessage> {
        if bucket == 0 || frames < bucket {
            return None;
        }
        let (start, frames) = align_span(start, frames, bucket);
        let purpose = PeaksFor::Detail {
            def_id,
            widget_id,
            key,
        };
        if let Some(old) = self.peaks.iter().position(|a| a.purpose == purpose) {
            let old = &self.peaks[old];
            if old.bufnum == bufnum
                && old.bucket == bucket
                && ((old.start == start && old.end == start + frames)
                    || (old.start <= seen.0 && old.end >= seen.1))
            {
                return None; // a grid over what is on screen is on its way
            }
            self.peaks.retain(|a| a.purpose != purpose);
        }
        self.start_peaks(PeaksAsk {
            bufnum,
            bucket,
            start,
            end: start + frames,
            next: start,
            channels: channels.max(1),
            sample_rate: 0.0,
            stats: Vec::new(),
            asked: false,
            stalled: 0,
            purpose,
        })
    }

    /// Notes that widget `widget_id` of window `def_id`, a plural asker, is
    /// owed `bufnum`'s summary, to be asked once it has been drawn
    /// ([`Self::take_owed_summaries`]).
    pub(crate) fn owe(
        &mut self,
        def_id: i32,
        widget_id: i32,
        bufnum: i32,
        shape: crate::host::widget::element::OwedSummary,
    ) {
        self.owed
            .retain(|o| !(o.0 == def_id && o.1 == widget_id && o.2 == bufnum));
        self.owed.push((def_id, widget_id, bufnum, shape));
    }

    /// The plural askers of window `def_id` still owed a summary.
    pub(crate) fn owed_widgets(&self, def_id: i32) -> Vec<i32> {
        self.owed
            .iter()
            .filter(|o| o.0 == def_id)
            .map(|o| o.1)
            .collect()
    }

    /// The summaries owed to plural askers of window `def_id`, handed out to
    /// be asked for -- `width_of` answers each asker's drawn width, and one not
    /// drawn yet stays owed.
    pub(crate) fn take_owed_summaries(
        &mut self,
        def_id: i32,
        width_of: impl Fn(i32) -> f32,
    ) -> Vec<(i32, f32, crate::host::widget::element::OwedSummary)> {
        let mut out = Vec::new();
        self.owed.retain(|&(def, widget, bufnum, shape)| {
            if def != def_id {
                return true;
            }
            let width = width_of(widget);
            if width < 1.0 {
                return true;
            }
            out.push((bufnum, width, shape));
            false
        });
        out
    }

    /// Files a conversation and asks its first request if there is room.
    fn start_peaks(&mut self, ask: PeaksAsk) -> Option<OscMessage> {
        self.peaks.push(ask);
        self.ask_peaks().into_iter().next()
    }

    /// The first request of every conversation waiting its turn, while there
    /// is room for one.
    fn ask_peaks(&mut self) -> Vec<OscMessage> {
        let room = PEAKS_IN_FLIGHT.saturating_sub(self.peaks.iter().filter(|a| a.asked).count());
        let mut out = Vec::new();
        for ask in self.peaks.iter_mut().filter(|a| !a.asked).take(room) {
            ask.asked = true;
            ask.stalled = 0;
            out.push(ask.request());
        }
        out
    }

    /// **One `/buffer_peaks.reply` landed**: the conversation it answers, if
    /// any, takes it. Returns that conversation whole when the reply covered
    /// its span, and the requests to send -- its next one, or the first of a
    /// conversation that was waiting for room.
    ///
    /// The reply says where it began and its own length says how many buckets
    /// it carried, so the place is read out of the answer: a reply for a frame
    /// nobody is asking from (a duplicate, or an answer to a grid a view has
    /// moved on from) is nobody's.
    pub(crate) fn on_peaks(
        &mut self,
        bufnum: i32,
        start: u64,
        bucket: usize,
        stats: &[f32],
    ) -> (Option<PeaksDone>, Vec<OscMessage>) {
        let Some(at) = self.peaks.iter().position(|a| {
            a.asked && a.bufnum == bufnum && a.bucket == bucket && a.next as u64 == start
        }) else {
            return (None, Vec::new());
        };
        let ask = &mut self.peaks[at];
        let buckets = stats.len() / (ask.channels * 3);
        ask.stats
            .extend_from_slice(&stats[..buckets * ask.channels * 3]);
        ask.next += buckets * ask.bucket;
        ask.stalled = 0;
        // Covered, or nothing whole was left to answer: either way the grid is
        // what came.
        if buckets == 0 || ask.next >= ask.end {
            let ask = self.peaks.remove(at);
            let done = PeaksDone {
                bufnum: ask.bufnum,
                bucket: ask.bucket,
                start: ask.start,
                end: ask.end.min(ask.next),
                channels: ask.channels,
                sample_rate: ask.sample_rate,
                stats: ask.stats,
                purpose: ask.purpose,
            };
            return (Some(done), self.ask_peaks());
        }
        (None, vec![ask.request()])
    }

    /// **The frame clock of every conversation with the server**: what never
    /// came back is asked for again, and what was owed a slot takes one.
    /// Called once a frame, beside the spans the drawing could not resolve.
    ///
    /// The carrier is allowed to lose a reply -- a full ring drops one rather
    /// than blocking the server -- so *nothing* here may assume that a request
    /// sent is a reply owed. Three conversations answer to this one clock:
    ///
    /// - a **download** that has landed nothing for [`STALLED_ASKS`] frames
    ///   asks for the chunk it is waiting on again, and a *span* is abandoned
    ///   instead, because the view that wanted it asks again every time it
    ///   draws and its span may have moved meanwhile;
    /// - a **summary walk** repeats its request, with nothing to undo: the
    ///   answer is folded where the frame it names says it belongs, so a
    ///   duplicate writes the same buckets twice;
    /// - a **download owed a slot** starts, which is how a multitrack with more
    ///   takes than [`MAX_IN_FLIGHT`] draws all of them rather than the three
    ///   that happened to fit the ring.
    pub(crate) fn tick(&mut self) -> Vec<OscMessage> {
        let mut again = Vec::new();
        self.columns.tick(STALLED_ASKS);
        // **A conversation that has stopped answering.** A reply can be lost
        // with nothing said -- the shared ring drops what does not fit in it,
        // and a page's is 64 KiB -- and the fetch it belonged to would otherwise
        // hold this buffer for the rest of the session: the view frozen at its
        // summary, or empty, and no error anywhere. A download that is
        // *working* lands something on every reply, so no progress across this
        // many frames is the difference that matters.
        let mut abandoned = Vec::new();
        for (bufnum, fetch) in self.fetches.iter_mut() {
            let progressed = fetch.received > fetch.stalled_at;
            fetch.stalled_at = fetch.received;
            fetch.stalled = if progressed { 0 } else { fetch.stalled + 1 };
            if fetch.stalled < STALLED_ASKS {
                continue;
            }
            // A span that went quiet is dropped, and its view asks again on
            // the next frame, for what it shows by then.
            abandoned.push(*bufnum);
        }
        for bufnum in abandoned {
            self.fetches.remove(&bufnum);
        }
        // **A summary conversation that went quiet** asks again from where it
        // was, with nothing to undo: the answer is placed by the frame it
        // names, so a duplicate is nobody's.
        for ask in self.peaks.iter_mut().filter(|a| a.asked) {
            ask.stalled += 1;
            if ask.stalled >= STALLED_ASKS {
                ask.stalled = 0;
                again.push(ask.request());
            }
        }
        again.extend(self.ask_peaks());
        // **What the server said is not there is asked for again**, on the same
        // clock, and let go of when it stays away (see [`ABSENT_ASKS`]).
        self.absent
            .retain(|bufnum, _| self.wants.contains_key(bufnum));
        let mut gone = Vec::new();
        for (bufnum, (frames, asks)) in self.absent.iter_mut() {
            *frames += 1;
            if *frames < STALLED_ASKS {
                continue;
            }
            *frames = 0;
            if *asks >= ABSENT_ASKS {
                gone.push(*bufnum);
                continue;
            }
            *asks += 1;
            again.push(OscMessage {
                addr: "/buffer_query".into(),
                args: vec![OscType::Int(*bufnum)],
            });
        }
        for bufnum in gone {
            self.absent.remove(&bufnum);
            self.wants.remove(&bufnum);
        }
        // room a finished span left
        again.extend(self.ask());
        again
    }

    /// **The announced edit that had to wait**, as the `/buffer_getRange` that
    /// reads it back, or `None` when nothing is waiting on `bufnum`.
    ///
    /// Asked after a step for that buffer lands, which is the moment the one
    /// in-flight download per buffer frees up. A span queued while *this* one
    /// is in flight simply queues again, so an edit is never lost and the
    /// buffer never has two conversations at once.
    pub(crate) fn queued_span(&mut self, bufnum: i32) -> Option<OscMessage> {
        let (start, frames) = self.queued.remove(&bufnum)?;
        let channels = self.channels.remove(&bufnum).unwrap_or(1);
        self.want_span(bufnum, start, frames, channels, SpanUse::Patch)
    }

    /// Forgets every want of a closed (or rebuilt) window, so a finished fetch
    /// does not try to fill it. An orphaned download still completes (and is
    /// discarded with an empty `wants`), which keeps the chunk conversation
    /// stateless for the server.
    /// **Everything on its way about `bufnum`, dropped**: the samples it
    /// changed under, which a view asks for again once it is told to read
    /// them again. The views waiting for its shape keep waiting, for the
    /// shape it has now.
    pub(crate) fn forget(&mut self, bufnum: i32) {
        self.fetches.remove(&bufnum);
        self.queued.remove(&bufnum);
        self.peaks.retain(|a| a.bufnum != bufnum);
        self.columns.forget(bufnum);
    }

    pub(crate) fn drop_def(&mut self, def_id: i32) {
        for wants in self.wants.values_mut() {
            wants.retain(|w| w.def_id != def_id);
        }
        self.wants.retain(|_, wants| !wants.is_empty());
        self.peaks
            .retain(|a| !matches!(a.purpose, PeaksFor::Detail { def_id: d, .. } if d == def_id));
        self.owed.retain(|o| o.0 != def_id);
        self.columns.drop_def(def_id);
        self.absent
            .retain(|bufnum, _| self.wants.contains_key(bufnum));
    }

    /// Hands the finished interleaved buffer to its waiters.
    fn finish(
        &mut self,
        bufnum: i32,
        interleaved: Vec<f32>,
        channels: usize,
        sample_rate: f64,
    ) -> FetchStep {
        FetchStep::Done {
            bufnum,
            samples: interleaved.into(),
            channels,
            sample_rate,
            wants: self.wants.remove(&bufnum).unwrap_or_default(),
        }
    }
}

/// **The level a take is summarized at for a view `width` pixels wide**: the
/// coarsest bucket, `base * 2^k`, that still gives every column of the whole
/// take at least one -- never resolving finer than the screen, so what the
/// summary costs follows the width and not the length. Zoomed past it, a view
/// asks for a finer grid over what it shows (`frame::owed`). `None` while the
/// view has not been drawn, since nothing says how wide it is yet.
pub(crate) fn summary_level(frames: usize, width: f32, base: usize) -> Option<usize> {
    // A width that is not a number is not drawn either.
    if width.is_nan() || width < 1.0 || base == 0 {
        return None;
    }
    let per_column = frames as f64 / width as f64;
    let mut bucket = base;
    while (bucket * 2) as f64 <= per_column {
        bucket *= 2;
    }
    Some(bucket)
}

/// **Widens a span to whole summary buckets.** A run that ends inside a bucket
/// can only patch that bucket from part of it, which would report a peak the
/// samples do not have -- so the request is widened rather than the answer
/// guessed at, and the widening is at most two buckets.
pub(crate) fn align_span(start: usize, frames: usize, bucket: usize) -> (usize, usize) {
    if bucket <= 1 || frames == 0 {
        return (start, frames);
    }
    let first = (start / bucket) * bucket;
    let end = (start + frames).div_ceil(bucket) * bucket;
    (first, end - first)
}

/// **The `/buffer_peaks` that asks for a finer grid over one span** -- the same
/// command the walk uses, with the span named rather than run to the end,
/// because what is wanted is exactly what is on screen.
fn detail_request(bufnum: i32, bucket: usize, start: usize, frames: usize) -> OscMessage {
    OscMessage {
        addr: "/buffer_peaks".into(),
        args: vec![
            OscType::Int(bufnum),
            OscType::Int(bucket as i32),
            index_arg(start),
            index_arg(frames),
        ],
    }
}

/// The `/buffer_getRange` for the next chunk of `bufnum` starting at `start`.
fn request_chunk(bufnum: i32, start: usize, total: usize) -> OscMessage {
    let count = BUFFER_CHUNK.min(total.saturating_sub(start));
    OscMessage {
        addr: "/buffer_getRange".into(),
        args: vec![OscType::Int(bufnum), index_arg(start), index_arg(count)],
    }
}

/// **A frame or a flat sample index as an OSC argument**: an `int32` while
/// it fits, a long past it, as the server's longer replies are. A take of
/// hours outgrows an `int32` -- a mono one at about twelve hours at 48 kHz,
/// a stereo one's flat index at six -- and a count cast down wraps negative,
/// which the answer reads as the start of the take.
pub(crate) fn index_arg(n: usize) -> OscType {
    match i32::try_from(n) {
        Ok(n) => OscType::Int(n),
        Err(_) => OscType::Long(n as i64),
    }
}

/// An index read back in either spelling [`index_arg`] writes; a negative
/// one reads as nothing.
pub(crate) fn index_of(arg: &OscType) -> Option<usize> {
    match arg {
        OscType::Int(n) => usize::try_from(*n).ok(),
        OscType::Long(n) => usize::try_from(*n).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A span asked for by one view, which is what a zoom is.
    fn window(def_id: i32, widget_id: i32) -> SpanUse {
        SpanUse::Window {
            def_id,
            widget_id,
            key: SlotKey::SELF,
        }
    }

    fn ints(msg: &OscMessage) -> Vec<i32> {
        msg.args
            .iter()
            .map(|a| match a {
                OscType::Int(n) => *n,
                other => panic!("expected int args, got {other:?}"),
            })
            .collect()
    }

    /// One `/buffer_getRange.reply` range, in the shape the server sends it:
    /// the samples as a little-endian `f32` blob, never as float arguments.
    fn range_reply(bufnum: i32, start: usize, values: &[f32]) -> Vec<OscType> {
        let blob = clausters_core::osc::sample_blob(values);
        vec![
            OscType::Int(bufnum),
            OscType::Int(start as i32),
            OscType::Blob(blob),
        ]
    }

    /// What a step sends: nothing, one request or several.
    fn sent(step: FetchStep) -> Vec<OscMessage> {
        match step {
            FetchStep::Request(msg) => vec![msg],
            FetchStep::Requests(msgs) => msgs,
            _ => Vec::new(),
        }
    }

    /// **Answers every request in order**, each with `value`s, as a server
    /// would, until a step that is not a request: that step, and every
    /// request answered.
    fn serve(
        f: &mut BufferFetches,
        first: Vec<OscMessage>,
        value: f32,
    ) -> (FetchStep, Vec<OscMessage>) {
        let mut queue: std::collections::VecDeque<OscMessage> = first.into();
        let mut seen = Vec::new();
        while let Some(msg) = queue.pop_front() {
            let a = ints(&msg);
            seen.push(msg);
            match f.on_data(&range_reply(
                a[0],
                a[1] as usize,
                &vec![value; a[2] as usize],
            )) {
                FetchStep::Request(next) => queue.push_back(next),
                FetchStep::Requests(next) => queue.extend(next),
                FetchStep::None => {}
                step => return (step, seen),
            }
        }
        (FetchStep::None, seen)
    }

    /// **One query per buffer, and a shape answers every view waiting on
    /// it** with the summary to build: no samples are downloaded.
    #[test]
    fn query_sent_once_per_buffer_then_a_summary_for_every_view() {
        let mut fetches = BufferFetches::default();
        // Two widgets on the same buffer: one /buffer_query, the second just waits.
        let query = fetches.want(1, 10, 7).expect("first want queries");
        assert_eq!(query.addr, "/buffer_query");
        assert_eq!(ints(&query), vec![7]);
        assert!(fetches.want(1, 11, 7).is_none());
        let FetchStep::Empty {
            bufnum,
            frames,
            channels,
            sample_rate,
            ask_summary,
            wants,
        } = fetches.on_info(7, 3, 2, 48_000.0)
        else {
            panic!("a shape is answered with a summary");
        };
        assert_eq!((bufnum, frames, channels, sample_rate), (7, 3, 2, 48_000.0));
        assert!(ask_summary);
        let ids: Vec<i32> = wants.iter().map(|w| w.widget_id).collect();
        assert_eq!(ids, vec![10, 11]);
        assert!(!fetches.pending());
    }

    /// **A span is read like a buffer and lands as a window**: the chunks walk
    /// the run and nothing outside it, and what comes back is addressed to the
    /// one view that asked.
    #[test]
    fn a_span_walks_its_own_chunks_and_lands_as_a_window() {
        let mut f = BufferFetches::default();
        let (start, frames, channels) = (10_000usize, 6_000usize, 2usize);
        let msg = f
            .want_span(3, start, frames, channels, window(1, 7))
            .expect("a span is asked for");
        // Flat indices: the run starts where the frames start, times channels.
        assert_eq!(
            ints(&msg),
            vec![
                3,
                (start * channels) as i32,
                BUFFER_CHUNK.min(frames * channels) as i32
            ]
        );

        // A second view's span is not asked for while this one is in flight --
        // one download per buffer is the bound.
        assert!(f.want_span(3, 0, 100, channels, window(1, 8)).is_none());

        let total = frames * channels;
        let (step, seen) = serve(&mut f, vec![msg], 0.5);
        let (from, to) = ((start * channels) as i32, (start * channels + total) as i32);
        for asked in &seen {
            let a = ints(asked);
            assert!(
                a[1] >= from && a[1] + a[2] <= to,
                "every chunk inside the run, never past it"
            );
        }
        assert_eq!(
            seen.len(),
            total.div_ceil(BUFFER_CHUNK),
            "each chunk asked once"
        );
        let FetchStep::Window {
            bufnum,
            want,
            start_frame,
            channels: got_channels,
            samples,
            ..
        } = step
        else {
            panic!("a finished span is a window");
        };
        assert_eq!((bufnum, start_frame, got_channels), (3, start, channels));
        assert_eq!((want.def_id, want.widget_id), (1, 7));
        assert_eq!(samples.len(), total);
        assert!(samples.iter().all(|s| *s == 0.5));
    }

    /// A range that comes back empty ends the read. The server clamps a read
    /// to what the buffer holds, so a buffer that shrank between the query
    /// and the read answers with nothing -- and re-asking for the same chunk
    /// would spin forever against a server that is already right.
    #[test]
    fn a_range_that_carries_nothing_ends_the_read() {
        let mut fetches = BufferFetches::default();
        assert!(fetches.want_span(4, 0, 100, 1, window(1, 10)).is_some());
        assert!(matches!(
            fetches.on_data(&range_reply(4, 0, &[])),
            FetchStep::None
        ));
        assert!(!fetches.pending(), "nothing is asked again");
    }

    /// **A span keeps three chunks in flight** once its first answer is in,
    /// asked in order, and each answer asks for the next: what one round trip
    /// per chunk cost was the waiting between them.
    #[test]
    fn a_span_keeps_three_chunks_in_flight() {
        let total = BUFFER_CHUNK * 4 + 100; // mono: five chunks
        let mut fetches = BufferFetches::default();
        let first = fetches
            .want_span(3, 0, total, 1, window(1, 10))
            .expect("the span asks");
        let chunk = BUFFER_CHUNK as i32;
        assert_eq!(ints(&first), vec![3, 0, chunk]);
        let asked = sent(fetches.on_data(&range_reply(3, 0, &vec![1.0; BUFFER_CHUNK])));
        assert_eq!(
            asked.iter().map(ints).collect::<Vec<_>>(),
            vec![
                vec![3, chunk, chunk],
                vec![3, 2 * chunk, chunk],
                vec![3, 3 * chunk, chunk]
            ],
            "three at once, in order"
        );
        let (step, _) = serve(&mut fetches, asked, 2.0);
        let FetchStep::Window { samples, .. } = step else {
            panic!("expected the span");
        };
        assert_eq!(samples.len(), total);
        assert_eq!(samples[0], 1.0);
        assert_eq!(samples[total - 1], 2.0);
    }

    #[test]
    fn empty_buffer_finishes_immediately_and_dropped_def_is_forgotten() {
        let mut fetches = BufferFetches::default();
        fetches.want(1, 10, 5);
        let FetchStep::Done { samples, wants, .. } = fetches.on_info(5, 0, 2, 0.0) else {
            panic!("a buffer of no frames finishes empty");
        };
        assert!(samples.is_empty());
        assert_eq!(wants.len(), 1);

        // A closed window's wants are dropped before the shape comes back.
        fetches.want(2, 20, 6);
        assert!(fetches.want(3, 30, 6).is_none());
        fetches.drop_def(2);
        let FetchStep::Empty { wants, .. } = fetches.on_info(6, 2, 1, 0.0) else {
            panic!("expected the summary");
        };
        let ids: Vec<i32> = wants.iter().map(|w| w.widget_id).collect();
        assert_eq!(ids, vec![30], "only the open window still waits");
    }

    /// **Absent is not empty.** A buffer the server does not have yet -- a join
    /// whose stitch is still on the NRT thread when its box is first drawn --
    /// hands nobody an empty take, and is asked for again until it exists.
    #[test]
    fn an_absent_buffer_is_asked_for_again_and_never_handed_over_empty() {
        let mut fetches = BufferFetches::default();
        fetches.want(1, 10, 8);
        fetches.on_absent(8);
        assert!(fetches.pending(), "the want is kept");
        for _ in 0..STALLED_ASKS - 1 {
            assert!(fetches.tick().is_empty());
        }
        let again = fetches.tick();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].addr, "/buffer_query");
        assert_eq!(ints(&again[0]), vec![8]);

        // Now it exists: the ordinary conversation, for the view that waited.
        let FetchStep::Empty { frames, wants, .. } = fetches.on_info(8, 4, 1, 48_000.0) else {
            panic!("an answer with frames is drawn from its summary");
        };
        assert_eq!(frames, 4);
        assert_eq!(wants.len(), 1);
        assert!(!fetches.pending());

        // A buffer that never comes is let go of, and nobody is told it is empty.
        let mut fetches = BufferFetches::default();
        fetches.want(1, 11, 9);
        fetches.on_absent(9);
        let mut asks = 0;
        for _ in 0..(ABSENT_ASKS + 2) * STALLED_ASKS {
            asks += fetches.tick().len();
        }
        assert_eq!(asks, ABSENT_ASKS, "asked a bounded number of times");
        assert!(!fetches.pending(), "and then let go of");

        // A window closed while it waited takes its absence with it.
        let mut fetches = BufferFetches::default();
        fetches.want(2, 12, 7);
        fetches.on_absent(7);
        fetches.drop_def(2);
        assert!(!fetches.pending());
    }

    /// An announced edit is read back as a span and lands as a patch, which
    /// names no widget: the samples are the buffer's own.
    #[test]
    fn an_announced_edit_reads_its_span_back_as_a_patch() {
        let mut fetches = BufferFetches::default();
        let msg = fetches
            .want_span(4, 8, 4, 2, SpanUse::Patch)
            .expect("a free buffer asks at once");
        assert_eq!(msg.addr, "/buffer_getRange");
        // Flat indices: a stereo span of four frames at frame 8.
        assert_eq!(ints(&msg), vec![4, 16, 8]);
        let step = fetches.on_data(&range_reply(
            4,
            16,
            &[1.0, -1.0, 2.0, -2.0, 3.0, -3.0, 4.0, -4.0],
        ));
        let FetchStep::Patch {
            bufnum,
            start_frame,
            channels,
            samples,
        } = step
        else {
            panic!("expected a patch");
        };
        assert_eq!((bufnum, start_frame, channels), (4, 8, 2));
        assert_eq!(samples.len(), 8, "interleaved, both channels");
    }

    /// A zoom that cannot be asked is dropped (the next frame asks again); an
    /// edit is kept, because it is announced once, and several are merged into
    /// the one span that covers them.
    #[test]
    fn an_edit_announced_while_the_buffer_is_busy_waits_and_merges() {
        let mut fetches = BufferFetches::default();
        fetches
            .want_span(2, 0, 4, 1, window(1, 10))
            .expect("the first span asks");
        assert!(
            fetches.want_span(2, 100, 4, 1, window(1, 11)).is_none(),
            "a zoom asks again next frame"
        );
        assert!(fetches.queued_span(2).is_none(), "nothing waits for a zoom");

        assert!(fetches.want_span(2, 40, 4, 1, SpanUse::Patch).is_none());
        assert!(fetches.want_span(2, 60, 10, 1, SpanUse::Patch).is_none());
        let FetchStep::Window { .. } = fetches.on_data(&range_reply(2, 0, &[0.0; 4])) else {
            panic!("the zoom's own span finishes first");
        };
        let msg = fetches.queued_span(2).expect("both edits waited");
        assert_eq!(
            ints(&msg),
            vec![2, 40, 30],
            "one request covering frames 40..70"
        );
    }

    /// **One route to a picture, whatever the size**: a take is drawn from
    /// its summary, asked for when it stands still and pushed while it is
    /// being written.
    #[test]
    fn every_take_is_drawn_from_its_summary() {
        let mut fetches = BufferFetches::default();
        fetches.want(1, 10, 1);
        let FetchStep::Empty { ask_summary, .. } = fetches.on_info(1, 1_000, 2, 48_000.0) else {
            panic!("a short take is drawn from its summary too");
        };
        assert!(ask_summary);

        // Ten minutes of stereo: 230 MB, which no view downloads at any zoom.
        let mut fetches = BufferFetches::default();
        fetches.want(1, 11, 2);
        let FetchStep::Empty {
            frames,
            ask_summary,
            ..
        } = fetches.on_info(2, 10 * 60 * 48_000, 2, 48_000.0)
        else {
            panic!("a long buffer is drawn from its summary");
        };
        assert_eq!(frames, 10 * 60 * 48_000, "the picture is its whole length");
        assert!(
            ask_summary,
            "and the summary is asked for: nothing writes it"
        );

        // The same length, being recorded into: the same route, and the
        // summary arrives on its own because it does not exist yet.
        let mut fetches = BufferFetches::default();
        fetches.want_shape(1, 12, 3);
        let FetchStep::Empty { ask_summary, .. } = fetches.on_info(3, 10 * 60 * 48_000, 2, 0.0)
        else {
            panic!("a recording is drawn from its shape");
        };
        assert!(
            !ask_summary,
            "a recording's overview is pushed, not asked for"
        );

        // And a *short* take being recorded into still takes it: what `fills`
        // says is that the samples are not there, which no size makes false.
        let mut fetches = BufferFetches::default();
        fetches.want_shape(1, 13, 4);
        let FetchStep::Empty { ask_summary, .. } = fetches.on_info(4, 1_000, 1, 0.0) else {
            panic!("a recording is never downloaded, however short");
        };
        assert!(!ask_summary);
    }

    /// **A conversation that stopped answering does not hold its buffer for
    /// the session.** A reply can be lost with nothing said -- the shared ring
    /// drops what does not fit -- and the picture would sit at its summary
    /// forever. A span is let go of, because the view asks again every time it
    /// draws and its span may have moved meanwhile.
    #[test]
    fn a_conversation_that_stops_answering_is_asked_again() {
        // A span that goes quiet is dropped, which is what lets the next
        // frame ask for the span the view is showing *now*.
        let mut fetches = BufferFetches::default();
        assert!(fetches.want_span(9, 1000, 512, 1, window(1, 10)).is_some());
        for _ in 0..STALLED_ASKS - 1 {
            assert!(fetches.tick().is_empty());
            assert!(
                fetches.want_span(9, 1000, 512, 1, window(1, 10)).is_none(),
                "the conversation in flight is still believed"
            );
        }
        assert!(
            fetches.tick().is_empty(),
            "a span is let go of, not re-asked"
        );
        assert!(
            fetches.want_span(9, 1000, 512, 1, window(1, 10)).is_some(),
            "so the view's next ask starts a conversation of its own"
        );

        // A download that is *working* is never restarted: every reply lands
        // something, which is the difference between slow and lost.
        let mut fetches = BufferFetches::default();
        let chunks = STALLED_ASKS + 8; // more chunks than the stall would allow
        assert!(
            fetches
                .want_span(6, 0, BUFFER_CHUNK * chunks, 1, window(1, 11))
                .is_some()
        );
        for i in 0..STALLED_ASKS + 4 {
            fetches.tick();
            fetches.on_data(&range_reply(6, i * BUFFER_CHUNK, &vec![1.0; BUFFER_CHUNK]));
            assert!(
                fetches.fetches.contains_key(&6),
                "a read that keeps answering is never let go of"
            );
        }
    }

    /// **A multitrack with more takes than the ring holds draws all of them.**
    /// A shape is answered with a summary to build and nothing is downloaded,
    /// so six takes are six summaries at once and none waits for a slot.
    #[test]
    fn more_takes_than_the_carrier_holds_are_all_summarized() {
        let mut fetches = BufferFetches::default();
        for bufnum in 0..6 {
            fetches.want(1, 100 + bufnum, bufnum);
        }
        let summarized = (0..6)
            .filter(|bufnum| {
                matches!(
                    fetches.on_info(*bufnum, 96_000, 1, 48_000.0),
                    FetchStep::Empty { .. }
                )
            })
            .count();
        assert_eq!(summarized, 6);
        assert!(!fetches.pending(), "and none is owed");
    }

    /// **A straggler from a conversation that was restarted must not end the
    /// new one**, and a span is handed over only as far as it arrived. Both
    /// halves of the same failure: a window that claims samples nobody read
    /// draws silence over a stretch of audio.
    #[test]
    fn a_late_reply_from_an_abandoned_fetch_neither_lands_nor_ends_the_new_one() {
        let mut fetches = BufferFetches::default();
        // A window over frames 1000..1512 of a mono take.
        let msg = fetches
            .want_span(7, 1000, 512, 1, window(1, 10))
            .expect("the span asks");
        assert_eq!(ints(&msg), vec![7, 1000, 512]);

        // A reply belonging to *another* span of the same buffer -- the one the
        // stall logic abandoned a moment ago. It is neither read nor believed.
        assert!(matches!(
            fetches.on_data(&range_reply(7, 0, &[1.0; 8])),
            FetchStep::None
        ));

        // The real answer arrives short: half the run, which is the server
        // clamping the read to what the buffer holds rather than a straggler.
        let FetchStep::Window {
            start_frame,
            samples,
            ..
        } = fetches.on_data(&range_reply(7, 1000, &[0.5; 256]))
        else {
            panic!("a short answer ends the read");
        };
        assert_eq!(start_frame, 1000);
        assert_eq!(
            samples.len(),
            256,
            "the window is as long as what arrived, never padded with silence"
        );
        assert!(samples.iter().all(|s| *s == 0.5));
    }

    /// `n` buckets of `channels` channels, each bucket's numbers its index.
    fn grid(n: usize, channels: usize) -> Vec<f32> {
        (0..n)
            .flat_map(|b| std::iter::repeat_n(b as f32, channels * 3))
            .collect()
    }

    /// **A summary comes over several replies and is handed over whole**, and
    /// the conversation survives losing one. The carrier may drop a reply -- a
    /// full ring drops rather than blocks -- so a walk that hears nothing asks
    /// again instead of leaving the view at the picture it had.
    #[test]
    fn a_summary_is_collected_and_handed_over_whole() {
        let mut fetches = BufferFetches::default();
        // A stereo take of 40 buckets of 1024, answered ten buckets at a time.
        let msg = fetches
            .want_summary(3, 1_024, 2, 40 * 1_024, 48_000.0)
            .expect("the first request");
        assert_eq!(msg.addr, "/buffer_peaks");
        assert_eq!(ints(&msg), vec![3, 1_024, 0, 40 * 1_024]);
        assert!(
            fetches
                .want_summary(3, 1_024, 2, 40 * 1_024, 48_000.0)
                .is_none(),
            "that level of that take is already on its way"
        );

        let (done, next) = fetches.on_peaks(3, 0, 1_024, &grid(10, 2));
        assert!(done.is_none(), "nothing is handed over half done");
        assert_eq!(
            ints(&next[0]),
            vec![3, 1_024, 10 * 1_024, 30 * 1_024],
            "it continues where the answer ended"
        );
        // Nothing comes back: the frame ticks and the same request goes again.
        for _ in 0..STALLED_ASKS - 1 {
            assert!(fetches.tick().is_empty(), "a moment's wait is not a loss");
        }
        let again = fetches.tick();
        assert_eq!(ints(&again[0]), vec![3, 1_024, 10 * 1_024, 30 * 1_024]);
        // A duplicate of the first answer is nobody's.
        assert!(fetches.on_peaks(3, 0, 1_024, &grid(10, 2)).0.is_none());

        let (done, next) = fetches.on_peaks(3, 10 * 1_024, 1_024, &grid(30, 2));
        let done = done.expect("the take is covered");
        assert!(next.is_empty());
        assert_eq!((done.bufnum, done.bucket, done.end), (3, 1_024, 40 * 1_024));
        assert_eq!(done.purpose, PeaksFor::Summary);
        assert_eq!(done.stats.len(), 40 * 2 * 3, "every bucket, in order");
        assert_eq!(done.stats[10 * 6], 0.0, "the second answer from its start");
        assert!(fetches.tick().is_empty(), "a finished walk ticks nothing");
    }

    /// **Two levels of one take are two conversations**, so two windows at
    /// two widths do not end each other's; and at most two summary
    /// conversations answer at once, a third waiting for room in the ring.
    #[test]
    fn levels_are_their_own_conversations_and_the_ring_bounds_them() {
        let mut fetches = BufferFetches::default();
        assert!(fetches.want_summary(3, 256, 1, 10_000, 0.0).is_some());
        assert!(fetches.want_summary(3, 4_096, 1, 10_000, 0.0).is_some());
        assert!(
            fetches.want_summary(4, 256, 1, 10_000, 0.0).is_none(),
            "a third waits"
        );
        let (done, next) = fetches.on_peaks(3, 0, 4_096, &grid(3, 1));
        assert!(done.is_some(), "the coarse level is done");
        assert_eq!(
            ints(&next[0]),
            vec![4, 256, 0, 10_000],
            "and the third asks"
        );
    }

    /// **A walk runs to the take's end, its last short bucket too**, and a
    /// take shorter than one bucket is asked for at all.
    #[test]
    fn a_summary_runs_to_the_tail_shorter_than_a_bucket() {
        let mut fetches = BufferFetches::default();
        fetches
            .want_summary(3, 256, 1, 10 * 256 + 100, 0.0)
            .unwrap();
        let (done, next) = fetches.on_peaks(3, 0, 256, &grid(10, 1));
        assert!(done.is_none());
        assert_eq!(ints(&next[0]), vec![3, 256, 10 * 256, 100]);
        let done = fetches.on_peaks(3, 10 * 256, 256, &grid(1, 1)).0;
        assert_eq!(done.expect("covered").stats.len(), 11 * 3);

        assert!(fetches.want_summary(4, 256, 1, 100, 0.0).is_some());
        assert!(
            fetches.want_summary(5, 256, 1, 0, 0.0).is_none(),
            "an empty one has none"
        );
    }

    /// The level a view's summary is asked at: the coarsest that gives every
    /// column a bucket, never finer than the base.
    #[test]
    fn the_level_follows_the_width() {
        let ten_minutes = 10 * 60 * 48_000;
        assert_eq!(summary_level(ten_minutes, 760.0, 256), Some(32_768));
        assert_eq!(summary_level(ten_minutes, 1_520.0, 256), Some(16_384));
        assert_eq!(
            summary_level(96_000, 760.0, 256),
            Some(256),
            "never finer than the base"
        );
        assert_eq!(summary_level(ten_minutes, 0.0, 256), None, "not drawn yet");
    }

    /// A span is widened to whole buckets, so what comes back can replace what
    /// the summary says over it rather than part of a bucket.
    #[test]
    fn a_span_is_widened_to_whole_buckets() {
        assert_eq!(
            align_span(300, 100, 256),
            (256, 256),
            "300..400 is one bucket"
        );
        assert_eq!(align_span(300, 300, 256), (256, 512), "300..600 is two");
        assert_eq!(align_span(512, 256, 256), (512, 256));
        assert_eq!(align_span(7, 3, 1), (7, 3), "no summary, no widening");
        assert_eq!(align_span(9, 0, 256), (9, 0), "an empty span stays empty");
    }

    #[test]
    fn unsolicited_replies_are_ignored() {
        let mut fetches = BufferFetches::default();
        assert!(matches!(fetches.on_info(9, 4, 1, 0.0), FetchStep::None));
        assert!(matches!(
            fetches.on_data(&range_reply(9, 0, &[1.0])),
            FetchStep::None
        ));
    }

    /// **A finer grid is one view's, and it comes over as many replies as it
    /// needs.** The same view asking for the same grid does not ask twice; a
    /// new span replaces the old; the grid is handed over whole; a lost reply
    /// is asked again; and a window closing takes its grids with it.
    #[test]
    fn a_detail_grid_is_one_conversation_per_view() {
        let mut fetches = BufferFetches::default();
        let msg = fetches
            .want_detail(
                7,
                1,
                100,
                SlotKey::SELF,
                (4_096, 8_192),
                (4_096, 12_288),
                32,
                2,
            )
            .expect("the grid asks");
        assert_eq!(msg.addr, "/buffer_peaks");
        assert_eq!(ints(&msg), vec![7, 32, 4_096, 8_192]);
        assert!(
            fetches
                .want_detail(
                    7,
                    1,
                    100,
                    SlotKey::SELF,
                    (4_096, 8_192),
                    (4_096, 12_288),
                    32,
                    2
                )
                .is_none(),
            "the same grid is already on its way"
        );
        // A pan asks a span a little further on: the grid on its way still
        // covers what is on screen, so it is let land rather than replaced.
        assert!(
            fetches
                .want_detail(
                    7,
                    1,
                    100,
                    SlotKey::SELF,
                    (5_120, 8_192),
                    (8_000, 10_000),
                    32,
                    2
                )
                .is_none(),
            "the grid on its way covers the view"
        );
        // Another view of the same take, at its own zoom over its own span.
        assert!(
            fetches
                .want_detail(7, 1, 200, SlotKey::SELF, (0, 8_192), (0, 8_192), 64, 2)
                .is_some()
        );

        // 256 buckets of 32 over the first view's span, in two replies.
        let (done, next) = fetches.on_peaks(7, 4_096, 32, &grid(100, 2));
        assert!(done.is_none(), "half a grid is not handed over");
        assert_eq!(ints(&next[0]), vec![7, 32, 4_096 + 3_200, 8_192 - 3_200]);
        let done = fetches
            .on_peaks(7, 4_096 + 3_200, 32, &grid(156, 2))
            .0
            .expect("the grid is whole");
        assert_eq!(
            done.purpose,
            PeaksFor::Detail {
                def_id: 1,
                widget_id: 100,
                key: SlotKey::SELF,
            }
        );
        assert_eq!(
            (done.start, done.end, done.stats.len()),
            (4_096, 12_288, 256 * 6)
        );

        // The other view's is still waiting, and asked again when nothing came.
        for _ in 0..STALLED_ASKS - 1 {
            assert!(fetches.tick().is_empty());
        }
        let again = fetches.tick();
        assert_eq!(again.len(), 1, "the grid nobody answered");
        assert_eq!(ints(&again[0]), vec![7, 64, 0, 8_192]);

        fetches.drop_def(1);
        assert!(
            fetches.on_peaks(7, 0, 64, &grid(128, 2)).0.is_none(),
            "closed with its window"
        );
    }

    /// **A summary walk over a take an edit changed is not the take's**: the
    /// walk at the old length is replaced by one at the new length rather
    /// than standing in for it, and a list that changed drops what was on
    /// its way about it -- where it walked on to the old length, the rest
    /// of the take was drawn as silence.
    #[test]
    fn a_walk_over_the_take_before_an_edit_is_not_the_takes() {
        let mut fetches = BufferFetches::default();
        assert!(
            fetches
                .want_summary(-2, 1024, 1, 100_000, 48_000.0)
                .is_some()
        );
        assert!(
            fetches
                .want_summary(-2, 1024, 1, 100_000, 48_000.0)
                .is_none(),
            "the same walk is on its way"
        );
        let longer = fetches
            .want_summary(-2, 1024, 1, 150_000, 48_000.0)
            .expect("the walk over the longer take asks");
        assert_eq!(longer.args[2], OscType::Int(0), "from its start");
        assert_eq!(fetches.peaks.len(), 1, "the old walk is gone");
        assert_eq!(fetches.peaks[0].end, 150_000);
        fetches.forget(-2);
        assert!(fetches.peaks.is_empty(), "a list that changed drops it");
    }

    /// **A take of hours is asked for by frames past an `int32`**: the
    /// summary walk and the run of samples name them as longs, and the reply
    /// is read back in either spelling, where a frame cast down wrapped
    /// negative and the walk started over from the take's first frame.
    #[test]
    fn a_span_past_an_int32_is_asked_and_read_as_a_long() {
        let far = 3_000_000_000usize;
        let mut fetches = BufferFetches::default();
        let msg = fetches
            .want_detail(
                -2,
                1,
                10,
                SlotKey::SELF,
                (far, 8_192),
                (far, far + 8_192),
                64,
                1,
            )
            .expect("the grid asks");
        assert_eq!(
            msg.args[2..],
            [OscType::Long(far as i64), OscType::Int(8_192)],
            "{msg:?}"
        );
        let msg = fetches
            .want_span(-2, far, 100, 1, window(1, 10))
            .expect("the span asks");
        assert_eq!(msg.args[1], OscType::Long(far as i64), "{msg:?}");
        let step = fetches.on_data(&[
            OscType::Int(-2),
            OscType::Long(far as i64),
            OscType::Blob(clausters_core::osc::sample_blob(&[0.5; 100])),
        ]);
        assert!(
            matches!(step, FetchStep::Window { start_frame, .. } if start_frame == far),
            "the run lands where it was asked"
        );
    }

    /// A span that does not start on a bucket boundary is widened to one: a
    /// bucket summarized from part of itself would report a peak the samples
    /// do not have, which is the rule the wire already states.
    #[test]
    fn a_detail_grid_is_asked_for_on_whole_buckets() {
        let mut fetches = BufferFetches::default();
        let msg = fetches
            .want_detail(3, 1, 10, SlotKey::SELF, (1_000, 500), (1_000, 1_500), 64, 1)
            .expect("the grid asks");
        assert_eq!(ints(&msg), vec![3, 64, 960, 576]);
        // Nothing to summarize: a span shorter than one bucket.
        assert!(
            fetches
                .want_detail(3, 1, 10, SlotKey::SELF, (0, 32), (0, 32), 64, 1)
                .is_none()
        );
        assert!(
            fetches
                .want_detail(3, 1, 10, SlotKey::SELF, (0, 640), (0, 640), 0, 1)
                .is_none()
        );
    }
}
