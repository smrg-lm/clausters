//! **The pass rules every editor plays by**: what the space bar plays, how a
//! pass ends, and what the position cursor does to the music
//! (`docs/app-behavior.md`, "The ends are before the first frame and past
//! the last", "A pass that ends on its own is taken in at once", "The cursor
//! moves the music").
//!
//! Written once and read by both playbacks -- the audio editor's over a take
//! and the multitrack's over its timeline -- so the two windows play, stop,
//! loop and follow a cursor alike. Each playback keeps only its unit: the
//! frame a caller counts in, crossed to the transport's samples by the
//! function it hands in.

use serde::{Deserialize, Serialize};

use crate::instance::Op;

/// **How a pass ends**: it loops over a span, or it runs until an end and
/// goes back. Frames of whatever the caller counts in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Pass {
    /// Over and over, inside the half-open span.
    Loop {
        /// The first frame of the span.
        from: u64,
        /// The first frame past it.
        to: u64,
    },
    /// Until `end`, where the transport stops and is located to `back` --
    /// or on and on, with no end: a multitrack whose passes do not stop
    /// where its contents do.
    Until {
        /// The frame it stops on, if it stops.
        end: Option<u64>,
        /// The frame it goes back to.
        back: u64,
    },
}

/// **What the space bar plays**, read off the view: where the pass starts
/// and how it ends.
///
/// A selection plays from its start, and with none the pass starts at the
/// position cursor, or at the first frame where none is placed. Looping, a
/// selection repeats and so does everything up to `end` with none. Not
/// looping, the pass stops at the end of the selection or at `end` and goes
/// back to the position cursor -- where a selection's play started, when
/// there is no cursor apart from it. With no `end` and no selection a pass
/// runs on, and a loop over nothing has nothing to repeat and runs on too.
pub fn space(
    looping: bool,
    selection: Option<(u64, u64)>,
    cursor: Option<u64>,
    end: Option<u64>,
) -> (u64, Pass) {
    let start = selection.map_or(cursor.unwrap_or(0), |(from, _)| from);
    let back = cursor.unwrap_or(start);
    let span = selection.or_else(|| end.map(|end| (0, end)));
    let pass = match span {
        Some((from, to)) if looping && to > from => Pass::Loop { from, to },
        _ => Pass::Until {
            end: selection.map(|(_, to)| to).or(end),
            back,
        },
    };
    (start, pass)
}

/// **Whether a transport rolls, and whether its pass loops**: the state the
/// rules read, and what each rule answers in transport ops.
///
/// `transport` is `None` while a playback has none yet: the state still
/// moves, and nothing is asked of a transport that is not there. `at`
/// crosses a caller's frame to the transport's sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Passes {
    rolling: bool,
    looping: bool,
    paused: bool,
}

impl Passes {
    /// Whether the transport was last told to roll.
    pub fn rolling(&self) -> bool {
        self.rolling
    }

    /// Whether the pass last asked for loops.
    pub fn looping(&self) -> bool {
        self.looping
    }

    /// **Whether the transport stands where a pause left it**: a play then
    /// resumes the pass in progress rather than starting one. A play, a stop
    /// and a cursor placed where nothing plays end the pause.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// **The paused pass is gone** -- the music it was of no longer plays
    /// there: the next play starts a pass rather than resuming one.
    pub fn end_pause(&mut self) {
        self.paused = false;
    }

    /// Sets whether the transport rolls, as its owner learned it did --
    /// a pass that ended on its own.
    pub fn set_rolling(&mut self, rolling: bool) {
        self.rolling = rolling;
    }

    /// **A pass, from `start`**: the loop or the end mark `pass` asks, the
    /// transport located and **cued** -- its readers read where it stands,
    /// so the first frame played is the take's and not a silence while the
    /// disk catches up -- and rolled.
    pub fn play(
        &mut self,
        transport: Option<i32>,
        start: u64,
        pass: Pass,
        at: &dyn Fn(u64) -> i64,
    ) -> Vec<Op> {
        self.looping = matches!(pass, Pass::Loop { .. });
        let Some(transport) = transport else {
            self.rolling = true;
            self.paused = false;
            return Vec::new();
        };
        let mut ops = pass_ops(transport, pass, at);
        ops.push(Op::TransportLocate {
            transport,
            sample: at(start),
        });
        ops.push(Op::TransportCue { transport });
        ops.extend(self.resume(Some(transport)));
        ops
    }

    /// **The loop switch changed while it plays**: the pass in progress ends
    /// as `pass` now says, from where the transport stands -- nothing
    /// located, nothing restarted. Stopped, nothing: the next play reads the
    /// switch.
    pub fn set_pass(
        &mut self,
        transport: Option<i32>,
        pass: Pass,
        at: &dyn Fn(u64) -> i64,
    ) -> Vec<Op> {
        if !self.rolling {
            return Vec::new();
        }
        self.looping = matches!(pass, Pass::Loop { .. });
        transport.map_or_else(Vec::new, |t| pass_ops(t, pass, at))
    }

    /// **The selection was redrawn while the pass loops**: the loop is now
    /// over `[from, to)`, from where the transport stands, and with `place`
    /// the transport goes to `from`. A sweep asks the span on every move and
    /// the place once, when it is let go: a transport past its loop's end
    /// runs on and never wraps, so a span drawn behind it is one it has to be
    /// put into. Nothing unless it rolls on a pass that loops.
    pub fn follow(
        &mut self,
        transport: Option<i32>,
        (from, to): (u64, u64),
        place: bool,
        at: &dyn Fn(u64) -> i64,
    ) -> Vec<Op> {
        if !self.rolling || !self.looping || from >= to {
            return Vec::new();
        }
        let Some(transport) = transport else {
            return Vec::new();
        };
        let mut ops = pass_ops(transport, Pass::Loop { from, to }, at);
        if place {
            ops.extend(locate(Some(transport), from, at));
        }
        ops
    }

    /// Rolls the transport again from where it stands.
    pub fn resume(&mut self, transport: Option<i32>) -> Vec<Op> {
        self.rolling = true;
        self.paused = false;
        transport
            .map(|transport| Op::TransportRun {
                transport,
                rolling: true,
            })
            .into_iter()
            .collect()
    }

    /// **Pauses where it stands**: a resume continues from there.
    pub fn pause(&mut self, transport: Option<i32>) -> Vec<Op> {
        self.rolling = false;
        self.paused = true;
        transport
            .map(|transport| Op::TransportRun {
                transport,
                rolling: false,
            })
            .into_iter()
            .collect()
    }

    /// **Stops and goes back to `back`**: the next play starts from the
    /// position cursor, not from where the pass ended.
    pub fn stop(&mut self, transport: Option<i32>, back: u64, at: &dyn Fn(u64) -> i64) -> Vec<Op> {
        let mut ops = self.pause(transport);
        self.paused = false;
        ops.extend(locate(transport, back, at));
        ops
    }

    /// **The position cursor moved to `frame` where nothing plays**: a
    /// stopped transport stands on it, so a play starts there, and a rolling
    /// one -- playing something else -- is left alone. A cursor placed in
    /// what plays moves the music ([`locate`]), which the caller decides.
    pub fn cue(&mut self, transport: Option<i32>, frame: u64, at: &dyn Fn(u64) -> i64) -> Vec<Op> {
        if self.rolling {
            return Vec::new();
        }
        self.paused = false;
        locate(transport, frame, at)
    }

    /// **The position cursor was placed at `frame` in what plays**: rolling,
    /// the music goes on from it, its pass ending as `pass` says from there;
    /// stopped, the transport stands on it ([`Self::cue`]).
    pub fn place(
        &mut self,
        transport: Option<i32>,
        frame: u64,
        pass: Pass,
        at: &dyn Fn(u64) -> i64,
    ) -> Vec<Op> {
        if !self.rolling {
            return self.cue(transport, frame, at);
        }
        let mut ops = locate(transport, frame, at);
        ops.extend(self.set_pass(transport, pass, at));
        ops
    }
}

/// Puts the transport at frame `frame`.
pub fn locate(transport: Option<i32>, frame: u64, at: &dyn Fn(u64) -> i64) -> Vec<Op> {
    transport
        .map(|transport| Op::TransportLocate {
            transport,
            sample: at(frame),
        })
        .into_iter()
        .collect()
}

/// **The loop and the end mark `pass` asks of the transport**: one of the
/// two, and the other cleared.
pub fn pass_ops(transport: i32, pass: Pass, at: &dyn Fn(u64) -> i64) -> Vec<Op> {
    let (span, mark) = match pass {
        Pass::Loop { from, to } => (Some((at(from), at(to))), None),
        Pass::Until { end, back } => (None, end.map(|end| (at(end), at(back)))),
    };
    vec![
        Op::TransportLoop { transport, span },
        Op::TransportEnd { transport, mark },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(frame: u64) -> i64 {
        frame as i64
    }

    /// **The space bar's pass, read off the view**, for a take and for a
    /// multitrack whose passes run on.
    #[test]
    fn the_space_bar_plays_the_selection_or_from_the_cursor() {
        let end = Some(1000);
        // a selection plays from its start to its end and back to the cursor
        assert_eq!(
            space(false, Some((100, 200)), Some(50), end),
            (
                100,
                Pass::Until {
                    end: Some(200),
                    back: 50
                }
            )
        );
        // with none, from the cursor to the end
        assert_eq!(
            space(false, None, Some(50), end),
            (
                50,
                Pass::Until {
                    end: Some(1000),
                    back: 50
                }
            )
        );
        // looping, the selection, or everything
        assert_eq!(
            space(true, Some((100, 200)), None, end).1,
            Pass::Loop { from: 100, to: 200 }
        );
        assert_eq!(
            space(true, None, Some(50), end),
            (50, Pass::Loop { from: 0, to: 1000 })
        );
        // a pass with no end runs on, and a loop over nothing does too
        assert_eq!(
            space(false, None, Some(50), None).1,
            Pass::Until {
                end: None,
                back: 50
            }
        );
        assert_eq!(
            space(true, None, None, None).1,
            Pass::Until { end: None, back: 0 }
        );
        assert_eq!(
            space(true, None, None, Some(0)).1,
            Pass::Until {
                end: Some(0),
                back: 0
            }
        );
    }

    /// **A pass that runs on clears the end mark**, so the transport does
    /// not stop on the one a pass before it set.
    #[test]
    fn a_pass_with_no_end_clears_the_mark() {
        let ops = pass_ops(3, Pass::Until { end: None, back: 0 }, &same);
        assert_eq!(
            ops,
            vec![
                Op::TransportLoop {
                    transport: 3,
                    span: None
                },
                Op::TransportEnd {
                    transport: 3,
                    mark: None
                },
            ]
        );
    }

    /// **The cursor moves the music only where it plays**: a cue stands a
    /// stopped transport on it and leaves a rolling one alone, and a loop
    /// follows a selection only while it rolls and loops.
    #[test]
    fn a_cue_moves_a_stopped_transport_and_a_loop_follows_while_rolling() {
        let mut passes = Passes::default();
        assert_eq!(
            passes.cue(Some(1), 40, &same),
            vec![Op::TransportLocate {
                transport: 1,
                sample: 40
            }]
        );
        assert!(passes.follow(Some(1), (0, 10), true, &same).is_empty());
        let played = passes.play(Some(1), 0, Pass::Loop { from: 0, to: 100 }, &same);
        assert!(matches!(
            played.last(),
            Some(Op::TransportRun { rolling: true, .. })
        ));
        assert!(passes.cue(Some(1), 40, &same).is_empty(), "it rolls");
        let followed = passes.follow(Some(1), (20, 40), true, &same);
        assert_eq!(
            followed.last(),
            Some(&Op::TransportLocate {
                transport: 1,
                sample: 20
            })
        );
        assert!(
            passes
                .set_pass(Some(1), Pass::Until { end: None, back: 0 }, &same)
                .len()
                == 2
        );
        assert!(
            passes.follow(Some(1), (20, 40), true, &same).is_empty(),
            "the pass no longer loops"
        );
        passes.stop(Some(1), 0, &same);
        assert!(!passes.rolling());
        assert!(
            passes
                .set_pass(Some(1), Pass::Loop { from: 0, to: 1 }, &same)
                .is_empty(),
            "stopped, the next play reads the switch"
        );
    }

    /// **A placed cursor moves the music where it plays**, and a pause is
    /// what a play resumes: a stop, a cue and a pause whose pass is gone end
    /// it.
    #[test]
    fn a_placed_cursor_moves_the_music_and_a_pause_is_resumed() {
        let mut passes = Passes::default();
        let until = Pass::Until {
            end: Some(100),
            back: 30,
        };
        assert_eq!(
            passes.place(Some(1), 30, until, &same),
            locate(Some(1), 30, &same),
            "stopped: it stands there"
        );
        passes.play(Some(1), 0, until, &same);
        let placed = passes.place(Some(1), 30, until, &same);
        assert_eq!(placed[0], locate(Some(1), 30, &same)[0]);
        assert_eq!(&placed[1..], &pass_ops(1, until, &same)[..]);
        passes.pause(Some(1));
        assert!(passes.paused());
        passes.resume(Some(1));
        assert!(!passes.paused());
        passes.pause(Some(1));
        passes.cue(Some(1), 10, &same);
        assert!(!passes.paused(), "a cursor placed starts a pass afresh");
        passes.pause(Some(1));
        passes.end_pause();
        assert!(!passes.paused());
        passes.pause(Some(1));
        passes.stop(Some(1), 0, &same);
        assert!(!passes.paused());
    }
}
