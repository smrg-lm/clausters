//! UGens that act on **their own node** rather than on a signal (U4):
//! `FreeSelf`, `PauseSelf`, `FreeSelfWhenDone` and `Done`.
//!
//! All four pass their input straight through, so they drop into a chain
//! without a `Mul` or a spare wire — the graph keeps working if you delete
//! them. What they add is the other half of the done-action mechanism: an
//! `EnvGen` decides *when* to free from the inside, and these decide it from a
//! trigger or from another UGen's finishing.
//!
//! The two flavours are genuinely different and the split is not cosmetic:
//!
//! - [`SelfControl`] (`FreeSelf`, `PauseSelf`) watches a **signal** and reports
//!   its action while that signal is positive. It does not latch: a paused node
//!   resumed by `/node_run 1` re-pauses only if its input is still up, which is
//!   what makes `PauseSelf` usable as a gate rather than a one-way door.
//! - [`WhenDone`] (`FreeSelfWhenDone`, `Done`) watches **another UGen's done
//!   flag**, which is not a signal at all and cannot be read off a wire. The
//!   synth resolves it (see `ExecMode::DoneQuery`) and hands it over before
//!   `process`.

use crate::dsp::{DoneAction, ProcessCtx, UGen, at};

/// `FreeSelf(in)` and `PauseSelf(in)`: pass `in` through, and ask for the
/// node to be freed or paused while `in` is greater than zero.
///
/// The action is reported for the block just processed and **not latched**.
/// For `FreeSelf` that is indistinguishable (the node is gone at the end of
/// the block either way), but for `PauseSelf` it is the whole behaviour: a
/// latched one would re-pause the instant `/node_run 1` resumed it, forever.
pub struct SelfControl {
    action: DoneAction,
    active: bool,
}

impl SelfControl {
    pub fn new(action: DoneAction) -> Self {
        Self {
            action,
            active: false,
        }
    }
}

impl UGen for SelfControl {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        self.active = false;
        for (i, out) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            self.active |= x > 0.0;
            *out = x;
        }
    }

    fn done(&self) -> DoneAction {
        if self.active {
            self.action
        } else {
            DoneAction::None
        }
    }
}

/// What a [`WhenDone`] does with the flag it watches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhenDoneMode {
    /// `Done(src)`: report the flag as a signal, 1 or 0. Reads as "has that
    /// envelope finished?", which is a trigger for anything else in the graph.
    Report,
    /// `FreeSelfWhenDone(src)`: pass `src` through and free the node once it
    /// finishes. The idiom for a voice whose envelope has `doneAction` 0
    /// because something else in the graph still needs it.
    Free,
}

/// `Done(src)` and `FreeSelfWhenDone(src)`: watch the **done flag** of the
/// UGen wired into input 0.
///
/// The flag is not the input's value — an envelope that has played out sits at
/// its final level, which may be any number, including the one it started at.
/// It arrives through [`UGen::set_done_flag`], resolved by the synth from the
/// wire's UGen index, so input 0 must be a wire to a kind that *has* a done
/// flag; the compiler rejects anything else by name rather than letting it read
/// zero forever.
///
/// **The flag has block resolution.** It is one bool per UGen, read once when
/// the watcher runs, so a watcher reports it for the whole block in which it
/// was raised — even at `ar`, where the source may have finished part-way
/// through. That is inherent to a flag rather than a signal, and at `kr` (these
/// two default there) it is exactly the resolution on offer anyway.
pub struct WhenDone {
    mode: WhenDoneMode,
    flag: bool,
}

impl WhenDone {
    pub fn new(mode: WhenDoneMode) -> Self {
        Self { mode, flag: false }
    }
}

impl UGen for WhenDone {
    fn set_done_flag(&mut self, done: bool) {
        self.flag = done;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        match self.mode {
            WhenDoneMode::Report => output.fill(if self.flag { 1.0 } else { 0.0 }),
            WhenDoneMode::Free => {
                for (i, out) in output.iter_mut().enumerate() {
                    *out = at(inputs[0], i);
                }
            }
        }
    }

    fn done(&self) -> DoneAction {
        match self.mode {
            WhenDoneMode::Free if self.flag => DoneAction::FreeSelf,
            _ => DoneAction::None,
        }
    }
}
