//! **The pass a transport is on**, as the engine publishes it beside its
//! position every block: whether it rolls, the span its position wraps inside
//! and the mark it stops on.
//!
//! A reader that carries a position on between two readings -- a play cursor
//! drawn between the blocks that publish it -- needs this, because only the
//! transport knows where its position turns back or stops. Carried without
//! it, a cursor over a loop shorter than a block ran past the loop's end by
//! up to a block before the next reading put it back.

/// What transport's position does next: see the module.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransportPass {
    /// Whether it rolls. Stopped, its position holds.
    pub rolling: bool,
    /// The span `[start, end)` its position wraps inside, when it loops.
    pub looping: Option<(u64, u64)>,
    /// The position it stops on when rolling with no loop, when it has an end
    /// mark.
    pub end: Option<u64>,
}

/// **One reading of a transport**, as one block published it: its clock (the
/// samples elapsed under it, which never jumps), its position and its pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransportReading {
    pub clock: u64,
    pub position: u64,
    pub pass: TransportPass,
}

impl TransportReading {
    /// **Where its position stands once its clock reads `clock`**: the
    /// position run on by what the clock advanced since this reading, on its
    /// pass. A reader carries the clock on between blocks, since it only ever
    /// advances, and places the position with this -- where a position read
    /// alone cannot tell a block that wrapped a loop back onto the same sample
    /// from a transport that stopped.
    pub fn position_at(&self, clock: f64) -> f64 {
        let from = self.position as f64;
        self.pass
            .carried(from, from + (clock - self.clock as f64).max(0.0))
    }
}

/// The word an absent end mark is published as.
const NO_END: u64 = u64::MAX;

impl TransportPass {
    /// **Where a position read as `from` stands once it has run on to
    /// `ahead`** (`ahead >= from`) on this pass: held where it is stopped,
    /// wrapped inside the loop it has not yet run past, and stopped on the end
    /// mark it has not yet passed. A loop wins over the mark, as it does in
    /// the engine, since the position wraps before it could reach it.
    pub fn carried(&self, from: f64, ahead: f64) -> f64 {
        if !self.rolling {
            return from;
        }
        if let Some((start, end)) = self.looping {
            let (start, end) = (start as f64, end as f64);
            // a position already past the loop's end runs on and never wraps
            return if from < end && ahead >= end && end > start {
                start + (ahead - end).rem_euclid(end - start)
            } else {
                ahead
            };
        }
        match self.end.map(|end| end as f64) {
            Some(end) if from <= end => ahead.min(end),
            _ => ahead,
        }
    }

    /// The four words it is published as: rolling (0 or 1), the loop's start
    /// and end (both 0 when nothing loops) and the end mark (`u64::MAX` for
    /// none).
    pub fn to_words(self) -> [u64; 4] {
        let (start, end) = self.looping.unwrap_or((0, 0));
        [
            u64::from(self.rolling),
            start,
            end,
            self.end.unwrap_or(NO_END),
        ]
    }

    /// The pass four published words say: [`TransportPass::to_words`] read
    /// back.
    pub fn from_words([rolling, start, end, mark]: [u64; 4]) -> Self {
        Self {
            rolling: rolling != 0,
            looping: (end > start).then_some((start, end)),
            end: (mark != NO_END).then_some(mark),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rolling() -> TransportPass {
        TransportPass {
            rolling: true,
            ..TransportPass::default()
        }
    }

    #[test]
    fn a_straight_pass_runs_on() {
        assert_eq!(rolling().carried(100.0, 1124.0), 1124.0);
    }

    #[test]
    fn a_stopped_transport_holds() {
        assert_eq!(TransportPass::default().carried(100.0, 1124.0), 100.0);
    }

    #[test]
    fn a_loop_shorter_than_the_run_wraps_inside_it() {
        // one cycle of 109 frames, carried a block of 1024 past a reading
        let pass = TransportPass {
            looping: Some((1000, 1109)),
            ..rolling()
        };
        let at = pass.carried(1050.0, 2074.0);
        assert!((1000.0..1109.0).contains(&at), "{at}");
        assert_eq!(at, 1000.0 + (2074.0f64 - 1109.0).rem_euclid(109.0));
        // short of the end it runs straight
        assert_eq!(pass.carried(1050.0, 1100.0), 1100.0);
        // a position past the loop's end runs on, as the engine's does
        assert_eq!(pass.carried(1200.0, 1300.0), 1300.0);
    }

    #[test]
    fn an_end_mark_stops_the_run_and_a_loop_wins_over_it() {
        let pass = TransportPass {
            end: Some(500),
            ..rolling()
        };
        assert_eq!(pass.carried(400.0, 900.0), 500.0);
        assert_eq!(pass.carried(600.0, 700.0), 700.0);
        let looped = TransportPass {
            looping: Some((0, 1000)),
            ..pass
        };
        assert_eq!(looped.carried(400.0, 900.0), 900.0);
    }

    #[test]
    fn a_reading_places_its_position_by_its_clock() {
        // a loop of 512 frames read every block of 1024: the same position
        // each block, while the clock runs on
        let reading = TransportReading {
            clock: 10_240,
            position: 100,
            pass: TransportPass {
                looping: Some((0, 512)),
                ..rolling()
            },
        };
        assert_eq!(reading.position_at(10_240.0), 100.0);
        assert_eq!(reading.position_at(10_752.0), 100.0, "one loop on");
        assert_eq!(reading.position_at(10_540.0), 400.0);
        assert_eq!(reading.position_at(10_700.0), 48.0, "wrapped");
        // a clock behind the reading does not move it back
        assert_eq!(reading.position_at(10_000.0), 100.0);
    }

    #[test]
    fn the_words_read_back() {
        for pass in [
            TransportPass::default(),
            rolling(),
            TransportPass {
                looping: Some((0, 48_000)),
                end: Some(0),
                ..rolling()
            },
            TransportPass {
                end: Some(1 << 40),
                ..TransportPass::default()
            },
        ] {
            assert_eq!(TransportPass::from_words(pass.to_words()), pass);
        }
    }
}
