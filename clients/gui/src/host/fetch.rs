//! The server-buffer fetch state machine, shared by the native and browser
//! fronts.
//!
//! A `waveform` or `spectrogram` that references a server `buffer` pulls its
//! samples over the client leg: `/buffer_query` for the shape, then chunked
//! `/buffer_getRange` requests whose `/buffer_getRange.reply` replies fill a flat interleaved array.
//! The finished download keeps **every channel** (interleaved, with the
//! channel count and sample rate from `/buffer_query.reply`): the waiting front looks up
//! each widget and builds a multichannel waveform or a per-channel STFT from
//! it. The protocol conversation is transport- and platform-independent —
//! only *sending* the returned messages and *placing* the finished samples
//! differ per front — so the whole machine lives here, pure and unit-testable
//! without a GPU or a socket. One chunk is in flight at a time; reassembly is
//! by the reply's explicit `start` offset.

use std::collections::HashMap;
use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscType};

/// Samples per `/buffer_getRange` request when pulling a server buffer: **8 kB
/// of reply**, and the number is the carrier's rather than the buffer's.
///
/// The smallest carrier a reply crosses is the shared ring, 64 KiB, and it is
/// **lossy under pressure by design**: a server whose reply ring is full drops
/// the reply rather than blocking (`osc::server`, "reply ring full"). So the
/// chunk is not a throughput knob, it is what decides how many replies can be
/// in the air at once without any of them being thrown away — four of these
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
/// A view refused here asks again on the next frame — it re-reads the span it
/// cannot draw every time it draws — so this queues rather than fails, and the
/// lanes resolve one after another instead of racing and losing.
const MAX_IN_FLIGHT: usize = 3;

/// The most samples a view will pull **whole** rather than draw from a summary
/// — about five seconds of stereo at 48 kHz, two megabytes on the wire. See
/// [`BufferFetches::whole`] for why the line is drawn by size at all.
///
/// The number is a count of **round trips** as much as of bytes: a whole
/// download is one request per [`BUFFER_CHUNK`], each gated by a frame, so this
/// is the point past which asking for the samples costs more waiting than
/// drawing the summary and reading the run under the eye.
const WHOLE_DOWNLOAD_SAMPLES: usize = 1 << 19;

/// How many asks a buffer's download may refuse **without landing anything**
/// before it is treated as lost and started over. The frame asks once per
/// draw, so this is about half a second at sixty frames — long enough that a
/// working conversation is never restarted between two of its replies, short
/// enough that a dropped one is a hesitation rather than the seconds it used
/// to be.
const STALLED_ASKS: usize = 30;

/// A widget waiting on a server buffer fetch. What to build from the finished
/// samples is read off the widget's kind at completion, so the machine carries
/// no per-kind parameters.
pub(crate) struct WaveWant {
    pub def_id: i32,
    pub widget_id: i32,
    /// Whether this widget asked for the buffer's **shape** rather than its
    /// samples — a take being recorded into, whose picture is filled by the
    /// overview the server streams and whose samples are silence until then.
    pub shape_only: bool,
}

/// An in-progress fetch of a server buffer: the flat interleaved samples
/// filled in as `/buffer_getRange.reply` chunks arrive.
struct BufferFetch {
    channels: usize,
    sample_rate: f64,
    /// Where in the buffer this download starts, as a **flat** index — `0` for
    /// a whole buffer, and the span's own origin for a window.
    origin: usize,
    /// How many samples are being downloaded from `origin` (flat).
    total: usize,
    samples: Vec<f32>,
    received: usize,
    /// How many times a span was refused for this buffer with **no progress**
    /// in between — see [`BufferFetches::want_span`]. Reset by every reply
    /// that lands anything.
    stalled: usize,
    /// What `received` was at the last such refusal, which is what says whether
    /// anything landed since.
    stalled_at: usize,
    /// **What this download is**, when it is a span: the frame it starts at
    /// and who it is for. `None` is the whole buffer, which is for everybody
    /// waiting on it.
    window: Option<(usize, SpanUse)>,
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
    Window { def_id: i32, widget_id: i32 },
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
        start_frame: usize,
        channels: usize,
        samples: Vec<f32>,
    },
    /// **A span another peer wrote** arrived: put it into every view of that
    /// buffer — samples and the summary over them — and redraw.
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
    /// being *written* is `false` — nothing can be asked for what is not there
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
    /// The summary walks under way, by buffer number.
    peaks: HashMap<i32, PeaksWalk>,
    /// **The finer grids asked for**, one per view — `(def_id, widget_id)`,
    /// because two views of one take are at two zooms over two spans and each
    /// gets its own answer.
    details: HashMap<(i32, i32), DetailAsk>,
}

/// **A finer summary asked for one view**, over the span it is showing.
///
/// It is a single request rather than a walk: the bucket is chosen so the whole
/// span fits one reply, because a detail grid is replaced rather than extended.
/// What it shares with a walk is the reason it is here — a carrier that is
/// allowed to lose a reply — so it is re-asked when nothing comes back.
struct DetailAsk {
    bufnum: i32,
    start: usize,
    frames: usize,
    bucket: usize,
    /// Frames since it was asked, in ticks of [`BufferFetches::tick_peaks`].
    stalled: usize,
}

/// **A walk over a buffer's summary**: what is left to ask for, and how long it
/// has been since anything came back.
///
/// It lives here rather than in a front because it needs what the downloads
/// need — a reply that never arrives must not stop it. A summary is asked for
/// once and answered in pieces; a piece lost on a carrier that is allowed to
/// lose one would otherwise leave a hole in the picture for the rest of the
/// session, with nothing to notice it.
struct PeaksWalk {
    /// The next frame to ask from, and where the take ends.
    next: usize,
    end: usize,
    bucket: usize,
    channels: usize,
    /// Frames since the last answer, in ticks of [`BufferFetches::tick_peaks`].
    stalled: usize,
}

impl BufferFetches {
    /// Registers a widget waiting on `bufnum`. Returns the `/buffer_query` to send
    /// the first time a buffer is wanted (`None` when a query or download for
    /// it is already under way — the widget just joins the wait).
    pub(crate) fn want(&mut self, def_id: i32, widget_id: i32, bufnum: i32) -> Option<OscMessage> {
        self.register(def_id, widget_id, bufnum, false)
    }

    /// Registers a widget waiting on `bufnum`'s **shape** — a take being
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
    /// Native-only because that is who asks: a page is woken by its own events
    /// (a socket message, a frame callback) and never chooses when to sleep, so
    /// the browser front has no wake-up to schedule and this would read as dead
    /// code there — which is exactly what the wasm build reported.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn pending(&self) -> bool {
        !self.wants.is_empty() || !self.fetches.is_empty()
    }

    /// `/buffer_query.reply` for a buffer we are waiting on: start its download (or finish
    /// immediately when it is empty/unallocated).
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
        let channels = channels.max(1);
        let total = frames * channels;
        if total == 0 {
            return self.finish(bufnum, Vec::new(), channels, sample_rate);
        }
        // **Which of the two routes this take arrives by**, decided here
        // because this is where the shape is first known — see [`Self::whole`].
        let being_written = self
            .wants
            .get(&bufnum)
            .is_some_and(|w| w.iter().all(|want| want.shape_only));
        if being_written || !Self::whole(total) {
            return FetchStep::Empty {
                bufnum,
                frames,
                channels,
                sample_rate,
                ask_summary: !being_written,
                wants: self.wants.remove(&bufnum).unwrap_or_default(),
            };
        }
        self.fetches.insert(
            bufnum,
            BufferFetch {
                channels,
                sample_rate,
                origin: 0,
                total,
                samples: vec![0.0; total],
                received: 0,
                stalled: 0,
                stalled_at: 0,
                window: None,
            },
        );
        FetchStep::Request(request_chunk(bufnum, 0, total))
    }

    /// **Whether a buffer of `total` samples is worth downloading whole.**
    ///
    /// There are two routes to a picture of a server buffer, and keeping both
    /// is the point rather than a wart: a short buffer is one conversation and
    /// then no latency at all, since every zoom is already answered out of the
    /// samples in hand; a long one cannot be downloaded at any zoom (a
    /// ten-minute stereo take is 230 MB) and is drawn from its summary, with
    /// the run under the eye read back as the eye moves.
    ///
    /// What was wrong was not the fork but the **criterion**: it used to be
    /// `fills` — *is this being recorded?* — so two views of one finished take,
    /// one opened while it recorded and one after, took different routes and
    /// behaved differently under the same hand. The question is a cost, so the
    /// answer is the size, and `fills` is back to meaning the one thing it
    /// says: these samples are being written, so they are not there to fetch.
    ///
    /// The threshold is about five seconds of stereo at 48 kHz — a buffer a
    /// player, a wavetable or a short take fits in, against the samples of a
    /// session, which do not.
    fn whole(total: usize) -> bool {
        total <= WHOLE_DOWNLOAD_SAMPLES
    }

    /// `/buffer_getRange.reply bufnum [start blob]...`: store each range, then
    /// request the next chunk or finish when the whole buffer has arrived.
    ///
    /// **The samples come back as a little-endian `f32` blob**, not as float
    /// arguments — the reply's own length is what actually came back, so no
    /// declared count can disagree with it. A range that carries nothing (an
    /// unallocated buffer, or a read past a buffer shorter than
    /// `/buffer_query.reply` said) ends the download with what has arrived
    /// rather than re-asking for a chunk the server has already declined.
    pub(crate) fn on_data(&mut self, args: &[OscType]) -> FetchStep {
        let [OscType::Int(bufnum), ranges @ ..] = args else {
            return FetchStep::None;
        };
        let bufnum = *bufnum;
        let (done, total, next) = {
            let Some(fetch) = self.fetches.get_mut(&bufnum) else {
                return FetchStep::None;
            };
            let mut next = 0usize;
            let mut landed = 0usize;
            let mut matched = 0usize;
            for range in ranges.chunks(2) {
                let [OscType::Int(start), OscType::Blob(bytes)] = range else {
                    continue;
                };
                let start = (*start).max(0) as usize;
                // The reply's `start` is the buffer's own flat index; a window
                // stores it where the window begins. A range that falls outside
                // this fetch's span belongs to **another conversation** — an
                // abandoned one whose reply arrived late — and is not this
                // fetch's to read or to be ended by.
                let Some(at) = start.checked_sub(fetch.origin) else {
                    continue;
                };
                if at >= fetch.total {
                    continue;
                }
                matched += 1;
                let n = (bytes.len() / 4).min(fetch.total.saturating_sub(at));
                for (i, word) in bytes.as_chunks::<4>().0.iter().take(n).enumerate() {
                    fetch.samples[at + i] = f32::from_le_bytes(*word);
                }
                fetch.received += n;
                landed += n;
                next = next.max(start + n);
            }
            // **A reply that was not ours changes nothing.** It used to end the
            // download — "a range that carries nothing ends it with what has
            // arrived" — which is right for a server that declined a read and
            // wrong for a straggler from a fetch that was restarted: the new
            // one would finish on the spot, and what it had not received yet is
            // zeros nobody measured.
            if matched == 0 {
                return FetchStep::None;
            }
            (
                landed == 0 || fetch.received >= fetch.total,
                fetch.origin + fetch.total,
                next,
            )
        };
        if done {
            let mut fetch = self.fetches.remove(&bufnum).unwrap();
            // **A span is handed over as far as it actually arrived.** The
            // buffer was sized for the whole request and filled from its
            // origin, so what is past `received` is zeros nothing measured —
            // and a view told it holds them draws silence over a stretch it
            // simply has not read. Truncated, the run answers where it
            // reaches and the summary answers everywhere else, which is what
            // a partial read honestly is.
            if fetch.window.is_some() {
                let frames = fetch.received / fetch.channels.max(1);
                if frames == 0 {
                    return FetchStep::None;
                }
                fetch.samples.truncate(frames * fetch.channels.max(1));
            }
            match fetch.window {
                Some((start_frame, SpanUse::Patch)) => {
                    return FetchStep::Patch {
                        bufnum,
                        start_frame,
                        channels: fetch.channels,
                        samples: fetch.samples,
                    };
                }
                Some((start_frame, SpanUse::Window { def_id, widget_id })) => {
                    return FetchStep::Window {
                        bufnum,
                        want: WaveWant {
                            def_id,
                            widget_id,
                            shape_only: false,
                        },
                        start_frame,
                        channels: fetch.channels,
                        samples: fetch.samples,
                    };
                }
                None => {}
            }
            self.finish(bufnum, fetch.samples, fetch.channels, fetch.sample_rate)
        } else {
            FetchStep::Request(request_chunk(bufnum, next, total))
        }
    }

    /// **Asks for one span of a buffer**, for the view that has zoomed past
    /// what its summary can answer. `start` and `frames` are per channel.
    ///
    /// Returns the first `/buffer_getRange` to send, or `None` when nothing
    /// should be asked: a download for that buffer is already in flight (a
    /// whole one will cover this span; another window's is one in flight per
    /// buffer, which is the bound this path keeps), or the request is empty.
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
        if !self.fetches.contains_key(&bufnum) && self.fetches.len() >= MAX_IN_FLIGHT {
            if span == SpanUse::Patch {
                self.queue(bufnum, start, frames, channels);
            }
            return None;
        }
        if let Some(fetch) = self.fetches.get_mut(&bufnum) {
            // **A conversation that has stopped answering is abandoned.** A
            // reply can be lost with nothing said — the shared ring drops what
            // does not fit in it, and a page's is 64 KiB — and the fetch it
            // belonged to would otherwise hold this buffer for the rest of the
            // session: every later span refused, the view frozen at its
            // summary, and no error anywhere. A download that is *working*
            // lands something on every reply, so no progress across this many
            // asks is the difference that matters. The frame asks again every
            // time it draws a span it cannot resolve, so this counts frames
            // without needing a clock.
            let progressed = fetch.received > fetch.stalled_at;
            fetch.stalled_at = fetch.received;
            fetch.stalled = if progressed { 0 } else { fetch.stalled + 1 };
            if fetch.stalled >= STALLED_ASKS {
                self.fetches.remove(&bufnum);
            }
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
            BufferFetch {
                channels,
                sample_rate: 0.0,
                origin,
                total,
                samples: vec![0.0; total],
                received: 0,
                stalled: 0,
                stalled_at: 0,
                window: Some((start, span)),
            },
        );
        Some(request_chunk(bufnum, origin, origin + total))
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

    /// **Starts a walk over `bufnum`'s summary**, returning the first
    /// `/buffer_peaks` to send.
    ///
    /// `bucket` must be the one the asking pyramid is built at, or what comes
    /// back cannot be folded into it; `frames` is how long the take is, which
    /// is what says when the walk is done.
    pub(crate) fn want_peaks(
        &mut self,
        bufnum: i32,
        bucket: usize,
        channels: usize,
        frames: usize,
    ) -> Option<OscMessage> {
        if bucket == 0 || frames < bucket {
            return None;
        }
        let walk = PeaksWalk {
            next: 0,
            end: frames,
            bucket,
            channels: channels.max(1),
            stalled: 0,
        };
        let msg = peaks_request(bufnum, walk.bucket, walk.next);
        self.peaks.insert(bufnum, walk);
        Some(msg)
    }

    /// One `/buffer_peaks.reply` landed: advance the walk and return the next
    /// request, or `None` when the summary is covered (or nothing is walking
    /// that buffer).
    ///
    /// The reply says where it began and its own length says how many buckets
    /// it carried, so the walk reads its place out of the answer rather than
    /// assuming the server answered the request it made.
    pub(crate) fn on_peaks(&mut self, bufnum: i32, start: u64, stats: usize) -> Option<OscMessage> {
        let walk = self.peaks.get_mut(&bufnum)?;
        let buckets = stats / (walk.channels * 3);
        if buckets == 0 {
            self.peaks.remove(&bufnum); // no whole bucket left: it is covered
            return None;
        }
        walk.stalled = 0;
        walk.next = (start as usize + buckets * walk.bucket).max(walk.next);
        if walk.next + walk.bucket > walk.end {
            self.peaks.remove(&bufnum);
            return None;
        }
        Some(peaks_request(bufnum, walk.bucket, walk.next))
    }

    /// **Asks for a finer grid over the span one view is showing**, or `None`
    /// when that exact grid is already in flight for it.
    ///
    /// `bucket` is finer than the view's own summary and the span is sized to
    /// one reply, both decided where the zoom is known (`frame::owed`). The ask
    /// is keyed by the view, so a new span for the same view **replaces** the
    /// old one: what is wanted is where the eye is now, and a reply for a span
    /// nobody is looking at any more is ignored when it lands.
    pub(crate) fn want_detail(
        &mut self,
        bufnum: i32,
        def_id: i32,
        widget_id: i32,
        start: usize,
        frames: usize,
        bucket: usize,
    ) -> Option<OscMessage> {
        if bucket == 0 || frames < bucket {
            return None;
        }
        let (start, frames) = align_span(start, frames, bucket);
        let ask = DetailAsk {
            bufnum,
            start,
            frames,
            bucket,
            stalled: 0,
        };
        if let Some(old) = self.details.get(&(def_id, widget_id))
            && old.bufnum == bufnum
            && old.start == start
            && old.frames == frames
            && old.bucket == bucket
        {
            return None; // the same grid is already on its way
        }
        let msg = detail_request(bufnum, bucket, start, frames);
        self.details.insert((def_id, widget_id), ask);
        Some(msg)
    }

    /// **One `/buffer_peaks.reply` that answers a view's detail grid**, or
    /// `None` when it answers the summary walk instead (or nobody asked).
    ///
    /// The two are told apart by the **bucket**: a detail grid is asked for
    /// finer than the view's own summary, which is the bucket a walk asks at,
    /// so a reply carrying a bucket some view asked for at that start is that
    /// view's. Consumed, because one reply is the whole grid.
    pub(crate) fn detail_reply(
        &mut self,
        bufnum: i32,
        start: u64,
        bucket: usize,
    ) -> Option<(i32, i32)> {
        let key = *self
            .details
            .iter()
            .find(|(_, ask)| {
                ask.bufnum == bufnum && ask.bucket == bucket && ask.start as u64 == start
            })
            .map(|(key, _)| key)?;
        self.details.remove(&key);
        Some(key)
    }

    /// **Asks again for a piece of a summary that never came back.** Called
    /// once a frame, beside the spans the drawing could not resolve.
    ///
    /// The carrier is allowed to lose a reply — a full ring drops one rather
    /// than blocking the server — so a walk that has heard nothing for
    /// [`STALLED_ASKS`] frames repeats its request. There is nothing to undo:
    /// the answer is folded where the frame it names says it belongs, so a
    /// duplicate writes the same buckets twice.
    pub(crate) fn tick_peaks(&mut self) -> Vec<OscMessage> {
        let mut again = Vec::new();
        for (bufnum, walk) in self.peaks.iter_mut() {
            walk.stalled += 1;
            if walk.stalled >= STALLED_ASKS {
                walk.stalled = 0;
                again.push(peaks_request(*bufnum, walk.bucket, walk.next));
            }
        }
        // A detail grid is one request and one reply, so a lost reply is the
        // whole of it: the view stays at the bucket it had with nothing to say
        // so. Same clock, same reason.
        for ask in self.details.values_mut() {
            ask.stalled += 1;
            if ask.stalled >= STALLED_ASKS {
                ask.stalled = 0;
                again.push(detail_request(
                    ask.bufnum, ask.bucket, ask.start, ask.frames,
                ));
            }
        }
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
    pub(crate) fn drop_def(&mut self, def_id: i32) {
        for wants in self.wants.values_mut() {
            wants.retain(|w| w.def_id != def_id);
        }
        self.wants.retain(|_, wants| !wants.is_empty());
        self.details.retain(|(def, _), _| *def != def_id);
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

/// **Widens a span to whole summary buckets.** A run that ends inside a bucket
/// can only patch that bucket from part of it, which would report a peak the
/// samples do not have — so the request is widened rather than the answer
/// guessed at, and the widening is at most two buckets.
pub(crate) fn align_span(start: usize, frames: usize, bucket: usize) -> (usize, usize) {
    if bucket <= 1 || frames == 0 {
        return (start, frames);
    }
    let first = (start / bucket) * bucket;
    let end = (start + frames).div_ceil(bucket) * bucket;
    (first, end - first)
}

/// **The `/buffer_peaks` that asks for a take's overview** from `first_frame`
/// on, at the bucket the asking summary is built at — so what comes back folds
/// into it with nothing converted.
///
/// `frames` is left at "to the end": the server answers as much as one reply
/// holds and says where it ended, which is what the walk reads.
pub(crate) fn peaks_request(bufnum: i32, bucket: usize, first_frame: usize) -> OscMessage {
    OscMessage {
        addr: "/buffer_peaks".into(),
        args: vec![
            OscType::Int(bufnum),
            OscType::Int(bucket as i32),
            OscType::Int(first_frame as i32),
            OscType::Int(-1),
        ],
    }
}

/// **The `/buffer_peaks` that asks for a finer grid over one span** — the same
/// command the walk uses, with the span named rather than run to the end,
/// because what is wanted is exactly what is on screen.
fn detail_request(bufnum: i32, bucket: usize, start: usize, frames: usize) -> OscMessage {
    OscMessage {
        addr: "/buffer_peaks".into(),
        args: vec![
            OscType::Int(bufnum),
            OscType::Int(bucket as i32),
            OscType::Int(start as i32),
            OscType::Int(frames as i32),
        ],
    }
}

/// The `/buffer_getRange` for the next chunk of `bufnum` starting at `start`.
fn request_chunk(bufnum: i32, start: usize, total: usize) -> OscMessage {
    let count = BUFFER_CHUNK.min(total.saturating_sub(start));
    OscMessage {
        addr: "/buffer_getRange".into(),
        args: vec![
            OscType::Int(bufnum),
            OscType::Int(start as i32),
            OscType::Int(count as i32),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A span asked for by one view, which is what a zoom is.
    fn window(def_id: i32, widget_id: i32) -> SpanUse {
        SpanUse::Window { def_id, widget_id }
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
        let mut blob = Vec::with_capacity(values.len() * 4);
        for v in values {
            blob.extend_from_slice(&v.to_le_bytes());
        }
        vec![
            OscType::Int(bufnum),
            OscType::Int(start as i32),
            OscType::Blob(blob),
        ]
    }

    #[test]
    fn query_sent_once_per_buffer_then_chunks_keeping_all_channels() {
        let mut fetches = BufferFetches::default();
        // Two widgets on the same buffer: one /buffer_query, the second just waits.
        let query = fetches.want(1, 10, 7).expect("first want queries");
        assert_eq!(query.addr, "/buffer_query");
        assert_eq!(ints(&query), vec![7]);
        assert!(fetches.want(1, 11, 7).is_none());

        // Stereo, 3 frames: one chunk covers it; both channels come out.
        let FetchStep::Request(msg) = fetches.on_info(7, 3, 2, 48_000.0) else {
            panic!("expected the first /buffer_getRange");
        };
        assert_eq!(msg.addr, "/buffer_getRange");
        assert_eq!(ints(&msg), vec![7, 0, 6]);
        let step = fetches.on_data(&range_reply(7, 0, &[0.0, 9.0, 1.0, 9.0, 2.0, 9.0]));
        let FetchStep::Done {
            bufnum,
            samples,
            channels,
            sample_rate,
            wants,
        } = step
        else {
            panic!("expected completion");
        };
        assert_eq!(bufnum, 7);
        assert_eq!(channels, 2);
        assert_eq!(sample_rate, 48_000.0);
        assert_eq!(
            &samples[..],
            &[0.0, 9.0, 1.0, 9.0, 2.0, 9.0],
            "interleaved, every channel kept"
        );
        let ids: Vec<i32> = wants.iter().map(|w| w.widget_id).collect();
        assert_eq!(ids, vec![10, 11]);
    }

    /// A range that comes back empty ends the download. The server clamps a
    /// read to what the buffer holds, so a buffer that shrank between the
    /// query and the read answers with nothing — and re-asking for the same
    /// chunk would spin forever against a server that is already right.
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

        // A second view's span is not asked for while this one is in flight —
        // one download per buffer is the bound.
        assert!(f.want_span(3, 0, 100, channels, window(1, 8)).is_none());

        let total = frames * channels;
        let mut at = start * channels;
        let mut step = f.on_data(&range_reply(3, at, &vec![0.5; BUFFER_CHUNK]));
        at += BUFFER_CHUNK;
        while let FetchStep::Request(msg) = step {
            let args = ints(&msg);
            assert_eq!(args[1], at as i32, "the next chunk continues the run");
            let n = args[2] as usize;
            assert!(at + n <= start * channels + total, "and never past its end");
            step = f.on_data(&range_reply(3, at, &vec![0.5; n]));
            at += n;
        }
        let FetchStep::Window {
            bufnum,
            want,
            start_frame,
            channels: got_channels,
            samples,
        } = step
        else {
            panic!("a finished span is a window");
        };
        assert_eq!((bufnum, start_frame, got_channels), (3, start, channels));
        assert_eq!((want.def_id, want.widget_id), (1, 7));
        assert_eq!(samples.len(), total);
        assert!(samples.iter().all(|s| *s == 0.5));
    }

    #[test]
    fn a_range_that_carries_nothing_finishes_the_download() {
        let mut fetches = BufferFetches::default();
        fetches.want(1, 10, 4);
        let FetchStep::Request(_) = fetches.on_info(4, 100, 1, 48_000.0) else {
            panic!("expected the first /buffer_getRange");
        };
        let step = fetches.on_data(&range_reply(4, 0, &[]));
        let FetchStep::Done { samples, .. } = step else {
            panic!("an empty range ends it rather than re-asking");
        };
        assert_eq!(samples.len(), 100, "what was declared, zero-filled");
    }

    #[test]
    fn large_buffer_walks_sequential_chunks() {
        let total = BUFFER_CHUNK * 2 + 100; // mono: three chunks
        let mut fetches = BufferFetches::default();
        fetches.want(1, 10, 3);
        let FetchStep::Request(msg) = fetches.on_info(3, total, 1, 44_100.0) else {
            panic!("expected the first /buffer_getRange");
        };
        assert_eq!(ints(&msg), vec![3, 0, BUFFER_CHUNK as i32]);
        let FetchStep::Request(msg) = fetches.on_data(&range_reply(3, 0, &vec![1.0; BUFFER_CHUNK]))
        else {
            panic!("expected the second chunk request");
        };
        assert_eq!(
            ints(&msg),
            vec![3, BUFFER_CHUNK as i32, BUFFER_CHUNK as i32]
        );
        let FetchStep::Request(msg) =
            fetches.on_data(&range_reply(3, BUFFER_CHUNK, &vec![2.0; BUFFER_CHUNK]))
        else {
            panic!("expected the third chunk request");
        };
        assert_eq!(ints(&msg), vec![3, (BUFFER_CHUNK * 2) as i32, 100]);
        let FetchStep::Done { samples, .. } =
            fetches.on_data(&range_reply(3, BUFFER_CHUNK * 2, &vec![3.0; 100]))
        else {
            panic!("expected completion");
        };
        assert_eq!(samples.len(), total);
        assert_eq!(samples[0], 1.0);
        assert_eq!(samples[total - 1], 3.0);
    }

    #[test]
    fn empty_buffer_finishes_immediately_and_dropped_def_is_forgotten() {
        let mut fetches = BufferFetches::default();
        fetches.want(1, 10, 5);
        let FetchStep::Done { samples, wants, .. } = fetches.on_info(5, 0, 2, 0.0) else {
            panic!("an unallocated buffer should finish empty");
        };
        assert!(samples.is_empty());
        assert_eq!(wants.len(), 1);

        // A closed window's wants are dropped mid-download; the fetch still
        // completes for the remaining waiter only.
        fetches.want(2, 20, 6);
        assert!(fetches.want(3, 30, 6).is_none());
        let FetchStep::Request(_) = fetches.on_info(6, 2, 1, 0.0) else {
            panic!("expected a chunk request");
        };
        fetches.drop_def(2);
        let FetchStep::Done { wants, .. } = fetches.on_data(&range_reply(6, 0, &[0.5, 0.5])) else {
            panic!("expected completion");
        };
        let ids: Vec<i32> = wants.iter().map(|w| w.widget_id).collect();
        assert_eq!(ids, vec![30], "only the open window still waits");
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

    /// **The two routes to a picture, and what chooses between them**: the
    /// size of the take, not whether something is recording into it.
    #[test]
    fn a_short_buffer_is_downloaded_and_a_long_one_is_summarized() {
        let mut fetches = BufferFetches::default();
        fetches.want(1, 10, 1);
        let FetchStep::Request(msg) = fetches.on_info(1, 1_000, 2, 48_000.0) else {
            panic!("a short buffer is one conversation and then no latency");
        };
        assert_eq!(msg.addr, "/buffer_getRange");

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
    /// the session.** A reply can be lost with nothing said — the shared ring
    /// drops what does not fit — and the view would sit at its summary forever,
    /// every later ask refused by a download that will never finish.
    #[test]
    fn a_download_that_stops_answering_is_started_over() {
        let mut fetches = BufferFetches::default();
        fetches.want(1, 10, 5);
        let FetchStep::Request(_) = fetches.on_info(5, 100_000, 1, 48_000.0) else {
            panic!("a short buffer downloads");
        };
        // Nothing comes back. Every frame asks for the span it cannot draw and
        // is refused -- until the ask that gives up on the silence.
        for _ in 0..STALLED_ASKS - 1 {
            assert!(
                fetches.want_span(5, 0, 512, 1, window(1, 10)).is_none(),
                "a download in flight is still believed"
            );
        }
        assert!(
            fetches.want_span(5, 0, 512, 1, window(1, 10)).is_some(),
            "and then the span is asked for again"
        );

        // A download that is *working* is never restarted: every reply lands
        // something, which is the difference between slow and lost.
        let mut fetches = BufferFetches::default();
        fetches.want(1, 11, 6);
        let chunks = STALLED_ASKS + 8; // more chunks than the stall would allow
        let FetchStep::Request(_) = fetches.on_info(6, BUFFER_CHUNK * chunks, 1, 48_000.0) else {
            panic!("expected the first chunk");
        };
        for i in 0..STALLED_ASKS + 4 {
            assert!(fetches.want_span(6, 0, 512, 1, window(1, 11)).is_none());
            let FetchStep::Request(_) =
                fetches.on_data(&range_reply(6, i * BUFFER_CHUNK, &vec![1.0; BUFFER_CHUNK]))
            else {
                panic!("a download that keeps answering is never restarted");
            };
        }
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

        // The real answer arrives short: half the run, and then a range that
        // carries nothing, which is a server declining rather than a straggler.
        let FetchStep::Request(_) = fetches.on_data(&range_reply(7, 1000, &[0.5; 256])) else {
            panic!("a partial answer asks for the rest");
        };
        let FetchStep::Window {
            start_frame,
            samples,
            ..
        } = fetches.on_data(&range_reply(7, 1256, &[]))
        else {
            panic!("a range that carries nothing ends the read");
        };
        assert_eq!(start_frame, 1000);
        assert_eq!(
            samples.len(),
            256,
            "the window is as long as what arrived, never padded with silence"
        );
        assert!(samples.iter().all(|s| *s == 0.5));
    }

    /// **A summary arrives in pieces and the walk survives losing one.** The
    /// carrier may drop a reply — a full ring drops rather than blocks — so a
    /// walk that hears nothing asks again instead of leaving a hole in the
    /// picture that nothing would ever notice.
    #[test]
    fn a_summary_walk_advances_on_each_answer_and_asks_again_when_one_is_lost() {
        let mut fetches = BufferFetches::default();
        // A stereo take of 40 buckets, answered ten buckets at a time.
        let msg = fetches
            .want_peaks(3, 256, 2, 40 * 256)
            .expect("a take longer than a bucket is walked");
        assert_eq!(msg.addr, "/buffer_peaks");
        assert_eq!(ints(&msg), vec![3, 256, 0, -1]);

        let next = fetches
            .on_peaks(3, 0, 10 * 2 * 3)
            .expect("more of the take is left");
        assert_eq!(
            ints(&next),
            vec![3, 256, 10 * 256, -1],
            "it continues where the answer ended, not where the request did"
        );

        // Nothing comes back. The frame ticks, and the same piece is asked for
        // again once the silence has gone on long enough.
        for _ in 0..STALLED_ASKS - 1 {
            assert!(
                fetches.tick_peaks().is_empty(),
                "a moment's wait is not a loss"
            );
        }
        let again = fetches.tick_peaks();
        assert_eq!(again.len(), 1);
        assert_eq!(
            ints(&again[0]),
            vec![3, 256, 10 * 256, -1],
            "the same piece"
        );

        // The rest lands, and the walk ends rather than asking past the take.
        assert!(fetches.on_peaks(3, 10 * 256, 30 * 2 * 3).is_none());
        assert!(
            fetches.tick_peaks().is_empty(),
            "a finished walk ticks nothing"
        );
        assert!(
            fetches.on_peaks(3, 0, 6).is_none(),
            "and a late answer to a walk that ended is nobody's"
        );
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

    /// **A finer grid is one request and one reply, and it is one view's.**
    /// The same view asking for the same grid does not ask twice; a new span
    /// replaces the old; and a reply is routed by the bucket it was measured
    /// at, so the walk's own replies are never taken for a detail.
    #[test]
    fn a_detail_grid_is_one_conversation_per_view_and_is_told_by_its_bucket() {
        let mut fetches = BufferFetches::default();
        let msg = fetches
            .want_detail(7, 1, 100, 4_096, 8_192, 32)
            .expect("the grid asks");
        assert_eq!(msg.addr, "/buffer_peaks");
        assert_eq!(ints(&msg), vec![7, 32, 4_096, 8_192]);
        assert!(
            fetches.want_detail(7, 1, 100, 4_096, 8_192, 32).is_none(),
            "the same grid is already on its way"
        );
        // Another view of the same take, at its own zoom over its own span.
        assert!(fetches.want_detail(7, 1, 200, 0, 8_192, 64).is_some());

        // A reply at a bucket nobody asked for is not a detail -- it is the
        // summary walk's, and goes to the pyramid itself.
        assert_eq!(fetches.detail_reply(7, 4_096, 256), None);
        assert_eq!(fetches.detail_reply(9, 4_096, 32), None, "another buffer");
        assert_eq!(fetches.detail_reply(7, 4_096, 32), Some((1, 100)));
        assert_eq!(
            fetches.detail_reply(7, 4_096, 32),
            None,
            "one reply is the whole grid: it is consumed"
        );

        // The other view's is still waiting, and a lost reply is asked for
        // again on the same clock a walk is.
        for _ in 0..STALLED_ASKS - 1 {
            assert!(fetches.tick_peaks().is_empty());
        }
        let again = fetches.tick_peaks();
        assert_eq!(again.len(), 1, "the grid nobody answered");
        assert_eq!(ints(&again[0]), vec![7, 64, 0, 8_192]);

        // A window closing takes its grids with it.
        fetches.drop_def(1);
        assert_eq!(fetches.detail_reply(7, 0, 64), None);
    }

    /// A span that does not start on a bucket boundary is widened to one: a
    /// bucket summarized from part of itself would report a peak the samples
    /// do not have, which is the rule the wire already states.
    #[test]
    fn a_detail_grid_is_asked_for_on_whole_buckets() {
        let mut fetches = BufferFetches::default();
        let msg = fetches
            .want_detail(3, 1, 10, 1_000, 500, 64)
            .expect("the grid asks");
        assert_eq!(ints(&msg), vec![3, 64, 960, 576]);
        assert_eq!(fetches.detail_reply(3, 960, 64), Some((1, 10)));
        // Nothing to summarize: a span shorter than one bucket.
        assert!(fetches.want_detail(3, 1, 10, 0, 32, 64).is_none());
        assert!(fetches.want_detail(3, 1, 10, 0, 640, 0).is_none());
    }
}
