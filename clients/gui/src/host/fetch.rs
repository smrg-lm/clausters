//! The server-buffer fetch state machine, shared by the native and browser
//! fronts.
//!
//! A `waveform` or `spectrogram` that references a server `buffer` pulls its
//! samples over the client leg: `/b_query` for the shape, then chunked
//! `/b_getn` requests whose `/b_setn` replies fill a flat interleaved array.
//! The finished download keeps **every channel** (interleaved, with the
//! channel count and sample rate from `/b_info`): the waiting front looks up
//! each widget and builds a multichannel waveform or a per-channel STFT from
//! it. The protocol conversation is transport- and platform-independent —
//! only *sending* the returned messages and *placing* the finished samples
//! differ per front — so the whole machine lives here, pure and unit-testable
//! without a GPU or a socket. One chunk is in flight at a time; reassembly is
//! by the reply's explicit `start` offset.

use std::collections::HashMap;
use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscType};

/// Samples per `/b_getn` request when pulling a server buffer (each reply must
/// fit a frame on every transport; a bulk-transfer optimization would replace
/// this path, not grow the chunk).
pub(crate) const BUFFER_CHUNK: usize = 8192;

/// A widget waiting on a server buffer fetch. What to build from the finished
/// samples is read off the widget's kind at completion, so the machine carries
/// no per-kind parameters.
pub(crate) struct WaveWant {
    pub def_id: i32,
    pub widget_id: i32,
}

/// An in-progress fetch of a server buffer: the flat interleaved samples
/// filled in as `/b_setn` chunks arrive.
struct BufferFetch {
    channels: usize,
    sample_rate: f64,
    total: usize,
    samples: Vec<f32>,
    received: usize,
}

/// What one protocol step asks the driving front to do.
pub(crate) enum FetchStep {
    /// Send this message to the audio server (the next `/b_getn`).
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
}

impl BufferFetches {
    /// Registers a widget waiting on `bufnum`. Returns the `/b_query` to send
    /// the first time a buffer is wanted (`None` when a query or download for
    /// it is already under way — the widget just joins the wait).
    pub(crate) fn want(&mut self, def_id: i32, widget_id: i32, bufnum: i32) -> Option<OscMessage> {
        let first = !self.wants.contains_key(&bufnum);
        self.wants
            .entry(bufnum)
            .or_default()
            .push(WaveWant { def_id, widget_id });
        (first && !self.fetches.contains_key(&bufnum)).then(|| OscMessage {
            addr: "/b_query".into(),
            args: vec![OscType::Int(bufnum)],
        })
    }

    /// `/b_info` for a buffer we are waiting on: start its download (or finish
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
        self.fetches.insert(
            bufnum,
            BufferFetch {
                channels,
                sample_rate,
                total,
                samples: vec![0.0; total],
                received: 0,
            },
        );
        FetchStep::Request(request_chunk(bufnum, 0, total))
    }

    /// `/b_setn bufnum start count value...`: store a chunk, then request the
    /// next one or finish when the whole buffer has arrived.
    pub(crate) fn on_data(&mut self, args: &[OscType]) -> FetchStep {
        let [
            OscType::Int(bufnum),
            OscType::Int(start),
            OscType::Int(count),
            rest @ ..,
        ] = args
        else {
            return FetchStep::None;
        };
        let (bufnum, start) = (*bufnum, (*start).max(0) as usize);
        let count = (*count).max(0) as usize;
        let (done, total) = {
            let Some(fetch) = self.fetches.get_mut(&bufnum) else {
                return FetchStep::None;
            };
            let end = start.saturating_add(count).min(fetch.total);
            let n = end.saturating_sub(start);
            for (i, arg) in rest.iter().take(n).enumerate() {
                if let OscType::Float(v) = arg {
                    fetch.samples[start + i] = *v;
                }
            }
            fetch.received += n;
            (fetch.received >= fetch.total, fetch.total)
        };
        if done {
            let fetch = self.fetches.remove(&bufnum).unwrap();
            self.finish(bufnum, fetch.samples, fetch.channels, fetch.sample_rate)
        } else {
            FetchStep::Request(request_chunk(bufnum, start + count, total))
        }
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

/// The `/b_getn` for the next chunk of `bufnum` starting at `start`.
fn request_chunk(bufnum: i32, start: usize, total: usize) -> OscMessage {
    let count = BUFFER_CHUNK.min(total.saturating_sub(start));
    OscMessage {
        addr: "/b_getn".into(),
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

    fn ints(msg: &OscMessage) -> Vec<i32> {
        msg.args
            .iter()
            .map(|a| match a {
                OscType::Int(n) => *n,
                other => panic!("expected int args, got {other:?}"),
            })
            .collect()
    }

    fn setn_args(bufnum: i32, start: usize, values: &[f32]) -> Vec<OscType> {
        let mut args = vec![
            OscType::Int(bufnum),
            OscType::Int(start as i32),
            OscType::Int(values.len() as i32),
        ];
        args.extend(values.iter().map(|v| OscType::Float(*v)));
        args
    }

    #[test]
    fn query_sent_once_per_buffer_then_chunks_keeping_all_channels() {
        let mut fetches = BufferFetches::default();
        // Two widgets on the same buffer: one /b_query, the second just waits.
        let query = fetches.want(1, 10, 7).expect("first want queries");
        assert_eq!(query.addr, "/b_query");
        assert_eq!(ints(&query), vec![7]);
        assert!(fetches.want(1, 11, 7).is_none());

        // Stereo, 3 frames: one chunk covers it; both channels come out.
        let FetchStep::Request(msg) = fetches.on_info(7, 3, 2, 48_000.0) else {
            panic!("expected the first /b_getn");
        };
        assert_eq!(msg.addr, "/b_getn");
        assert_eq!(ints(&msg), vec![7, 0, 6]);
        let step = fetches.on_data(&setn_args(7, 0, &[0.0, 9.0, 1.0, 9.0, 2.0, 9.0]));
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

    #[test]
    fn large_buffer_walks_sequential_chunks() {
        let total = BUFFER_CHUNK * 2 + 100; // mono: three chunks
        let mut fetches = BufferFetches::default();
        fetches.want(1, 10, 3);
        let FetchStep::Request(msg) = fetches.on_info(3, total, 1, 44_100.0) else {
            panic!("expected the first /b_getn");
        };
        assert_eq!(ints(&msg), vec![3, 0, BUFFER_CHUNK as i32]);
        let FetchStep::Request(msg) = fetches.on_data(&setn_args(3, 0, &vec![1.0; BUFFER_CHUNK]))
        else {
            panic!("expected the second chunk request");
        };
        assert_eq!(
            ints(&msg),
            vec![3, BUFFER_CHUNK as i32, BUFFER_CHUNK as i32]
        );
        let FetchStep::Request(msg) =
            fetches.on_data(&setn_args(3, BUFFER_CHUNK, &vec![2.0; BUFFER_CHUNK]))
        else {
            panic!("expected the third chunk request");
        };
        assert_eq!(ints(&msg), vec![3, (BUFFER_CHUNK * 2) as i32, 100]);
        let FetchStep::Done { samples, .. } =
            fetches.on_data(&setn_args(3, BUFFER_CHUNK * 2, &vec![3.0; 100]))
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
        let FetchStep::Done { wants, .. } = fetches.on_data(&setn_args(6, 0, &[0.5, 0.5])) else {
            panic!("expected completion");
        };
        let ids: Vec<i32> = wants.iter().map(|w| w.widget_id).collect();
        assert_eq!(ids, vec![30], "only the open window still waits");
    }

    #[test]
    fn unsolicited_replies_are_ignored() {
        let mut fetches = BufferFetches::default();
        assert!(matches!(fetches.on_info(9, 4, 1, 0.0), FetchStep::None));
        assert!(matches!(
            fetches.on_data(&setn_args(9, 0, &[1.0])),
            FetchStep::None
        ));
    }
}
