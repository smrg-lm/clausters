//! **A spectrogram's columns over the span a view shows**, asked of a list.
//!
//! A take that is a list of segments over files is never read whole, so its
//! spectrogram is never analyzed whole either: a view asks for the columns of
//! the span it shows, one analysis window every `hop` frames at about a
//! column of the view, and asks again when it moves past what it holds or
//! zooms to another hop. Zoomed out a column is one window over seconds of
//! the take, a preview; zoomed in the hop is fine enough that the windows
//! overlap. What it costs is the view's columns times the window, whatever
//! the take's length.
//!
//! The conversation is the summary walks' (`super::fetch`): one request out
//! at a time per view, each from where the last answer ended, and the columns
//! collected until the span is covered, so a view is handed them whole and
//! keeps the picture it had until then. A view that moved on while its
//! columns were on their way is handed what it asked for, and asks again for
//! where it is: answers land at the pace they come, never cancelled by a
//! scroll that would leave nothing landing at all.

use clausters_core::osc::{OscMessage, OscType};

use crate::spectrogram::Stft;

/// One view's columns on their way.
struct ColumnsAsk {
    key: i32,
    def_id: i32,
    widget_id: i32,
    start: usize,
    hop: usize,
    columns: usize,
    window: usize,
    /// The view's own hop, which a column wider than it is the largest of
    /// the windows on.
    analysis: usize,
    sample_rate: f32,
    /// The take's length, which the transform of a span carries for the
    /// axis that has to know how long the take is.
    total: usize,
    /// What came back, column-major and channel-minor, a byte a magnitude.
    got: Vec<u8>,
    channels: usize,
    /// Columns answered so far.
    answered: usize,
    /// Ticks since the request went with no answer.
    stalled: usize,
}

impl ColumnsAsk {
    fn request(&self) -> OscMessage {
        OscMessage {
            addr: "/buffer_spectrum".into(),
            args: vec![
                OscType::Int(self.key),
                OscType::Long((self.start + self.answered * self.hop) as i64),
                OscType::Int(self.hop as i32),
                OscType::Int((self.columns - self.answered) as i32),
                OscType::Int(self.window as i32),
                OscType::Int(self.analysis as i32),
            ],
        }
    }
}

/// **The columns of one view, whole**: a transform of the span per channel.
pub(crate) struct ColumnsDone {
    pub def_id: i32,
    pub widget_id: i32,
    pub key: i32,
    pub stfts: Vec<Stft>,
}

/// The column conversations, one per view.
#[derive(Default)]
pub(crate) struct ColumnAsks {
    asks: Vec<ColumnsAsk>,
}

/// What a view asks columns for: the span, the hop and the window, the rate
/// and the take's length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ColumnSpan {
    pub start: usize,
    pub hop: usize,
    pub columns: usize,
    pub window: usize,
    /// The view's own hop (`0`: half the window), set by whoever knows the
    /// element's analysis.
    pub analysis: usize,
    pub sample_rate: f32,
    pub total: usize,
}

impl ColumnAsks {
    /// **Asks for a view's columns**, or `None` while that view already has
    /// an ask on its way: what is wanted is asked again once it lands.
    pub(crate) fn want(
        &mut self,
        key: i32,
        def_id: i32,
        widget_id: i32,
        span: ColumnSpan,
    ) -> Option<OscMessage> {
        if span.columns == 0
            || self
                .asks
                .iter()
                .any(|a| a.def_id == def_id && a.widget_id == widget_id)
        {
            return None;
        }
        let ask = ColumnsAsk {
            key,
            def_id,
            widget_id,
            start: span.start,
            hop: span.hop.max(1),
            columns: span.columns,
            window: span.window,
            analysis: span.analysis,
            sample_rate: span.sample_rate,
            total: span.total,
            got: Vec::new(),
            channels: 0,
            answered: 0,
            stalled: 0,
        };
        let msg = ask.request();
        self.asks.push(ask);
        Some(msg)
    }

    /// `/buffer_spectrum.reply key start hop window channels blob`: the next
    /// request of the view it answers, and its columns once they are all
    /// there. A reply nobody is waiting for is dropped.
    pub(crate) fn on_reply(
        &mut self,
        args: &[OscType],
    ) -> (Option<OscMessage>, Option<ColumnsDone>) {
        let int = |i: usize| match args.get(i) {
            Some(OscType::Int(n)) => *n as i64,
            Some(OscType::Long(n)) => *n,
            _ => -1,
        };
        let (key, start, hop, window, channels) = (int(0), int(1), int(2), int(3), int(4));
        let Some(OscType::Blob(blob)) = args.get(5) else {
            return (None, None);
        };
        let Some(i) = self.asks.iter().position(|a| {
            i64::from(a.key) == key
                && (a.start + a.answered * a.hop) as i64 == start
                && a.hop as i64 == hop
                && a.window as i64 == window
        }) else {
            return (None, None);
        };
        let ask = &mut self.asks[i];
        let channels = channels.max(1) as usize;
        let per_column = channels * (ask.window / 2);
        let came = blob.len() / per_column.max(1);
        if came == 0 {
            // Nothing came: the span is past the take, or the file is gone.
            // What came before stands; the view asks again if it still wants.
            self.asks.remove(i);
            return (None, None);
        }
        ask.channels = channels;
        ask.got.extend_from_slice(&blob[..came * per_column]);
        ask.answered += came;
        ask.stalled = 0;
        if ask.answered < ask.columns {
            return (Some(ask.request()), None);
        }
        let ask = self.asks.remove(i);
        let bins = ask.window / 2;
        let stfts = (0..ask.channels)
            .filter_map(|ch| {
                let mags: Vec<f32> = ask
                    .got
                    .chunks_exact(per_column)
                    .flat_map(|column| &column[ch * bins..(ch + 1) * bins])
                    .map(|q| f32::from(*q) / 255.0)
                    .collect();
                Stft::of_span(
                    mags,
                    bins,
                    ask.start,
                    ask.hop,
                    ask.window,
                    ask.sample_rate,
                    ask.total,
                )
            })
            .collect();
        (
            None,
            Some(ColumnsDone {
                def_id: ask.def_id,
                widget_id: ask.widget_id,
                key: ask.key,
                stfts,
            }),
        )
    }

    /// One tick of the frame clock: an ask that stopped answering is dropped,
    /// and its view asks again for what it shows by then.
    pub(crate) fn tick(&mut self, stalled_asks: usize) {
        for ask in &mut self.asks {
            ask.stalled += 1;
        }
        self.asks.retain(|a| a.stalled < stalled_asks);
    }

    /// Forgets the asks over take `key`, whose list changed.
    pub(crate) fn forget(&mut self, key: i32) {
        self.asks.retain(|a| a.key != key);
    }

    /// Forgets the asks of a closed window.
    pub(crate) fn drop_def(&mut self, def_id: i32) {
        self.asks.retain(|a| a.def_id != def_id);
    }

    /// Whether anything is on its way.
    pub(crate) fn pending(&self) -> bool {
        !self.asks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, columns: usize) -> ColumnSpan {
        ColumnSpan {
            start,
            hop: 100,
            columns,
            window: 8,
            analysis: 0,
            sample_rate: 48_000.0,
            total: 10_000,
        }
    }

    fn reply(start: i64, columns: usize, value: u8) -> Vec<OscType> {
        vec![
            OscType::Int(-2),
            OscType::Long(start),
            OscType::Int(100),
            OscType::Int(8),
            OscType::Int(2),
            OscType::Blob(vec![value; columns * 2 * 4]),
        ]
    }

    #[test]
    fn a_views_columns_are_handed_over_whole_after_as_many_replies_as_they_take() {
        let mut asks = ColumnAsks::default();
        assert!(asks.want(-2, 1, 7, span(500, 5)).is_some());
        // A second ask for the same view waits for the first to land.
        assert!(asks.want(-2, 1, 7, span(900, 5)).is_none());
        let (next, done) = asks.on_reply(&reply(500, 3, 255));
        let next = next.expect("the rest is asked for");
        assert_eq!(next.args[1], OscType::Long(800));
        assert_eq!(next.args[3], OscType::Int(2));
        assert!(done.is_none());
        let (next, done) = asks.on_reply(&reply(800, 2, 0));
        assert!(next.is_none());
        let done = done.expect("the columns, whole");
        assert_eq!((done.def_id, done.widget_id, done.stfts.len()), (1, 7, 2));
        let stft = &done.stfts[0];
        assert_eq!((stft.n_frames(), stft.n_bins(), stft.origin()), (5, 4, 500));
        assert_eq!(stft.total_samples(), 10_000);
        assert_eq!(stft.column(0)[0], 1.0);
        assert_eq!(stft.column(4)[0], 0.0);
        // Landed, the view may ask again.
        assert!(asks.want(-2, 1, 7, span(900, 5)).is_some());
    }

    #[test]
    fn a_reply_nobody_waits_for_and_an_ask_gone_quiet_are_let_go() {
        let mut asks = ColumnAsks::default();
        assert_eq!(asks.on_reply(&reply(0, 1, 1)).1.map(|d| d.key), None);
        asks.want(-2, 1, 7, span(0, 5));
        asks.tick(3);
        asks.tick(3);
        assert!(asks.pending());
        asks.tick(3);
        assert!(!asks.pending());
    }
}
