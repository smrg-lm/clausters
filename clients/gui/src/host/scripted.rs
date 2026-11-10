//! **A script's steps on a window an application holds, one at a time.**
//!
//! A script drives a window with verbs, a hand, keys and questions, and the
//! window's application answers what each one asked of it from its own
//! thread, as corrections that come back later. A step performed the moment
//! it arrived could read the window from before the step ahead of it was
//! answered: a selection by pitch, then a transposition of what was selected,
//! moved what was selected before. So on a window an application holds, each
//! step of a script waits in that window's queue until the application has
//! answered the one before:
//!
//! - **a verb, a hand or keys** is handed to the front, which performs it --
//!   what it asks of the application goes there as the window's turns -- and
//!   then says so ([`Host::performed`]), and the host asks the application to
//!   send word back behind those turns (`ToApp::After`);
//! - **a question** (`/gui_query`, the editor's `selected`) goes round the
//!   application's thread the same way and is answered when it is back;
//!
//! and the next step is released when that word comes back. A window no
//! application holds is a client's, whose owner answers on the wire, and its
//! steps are performed when they arrive, as always.

use std::collections::{HashMap, VecDeque};

use serde_json::Value;

use super::{ClientId, HostEffect};

/// One step of a script, waiting for its turn.
#[derive(Debug)]
pub(super) enum Step {
    /// A verb, a hand or keys, which the front performs.
    Effect(HostEffect),
    /// `/gui_query <id>` of a widget of the window.
    Query { id: i32, from: ClientId },
    /// A question of the window's application door, answered by the host
    /// (the editor's `selected`).
    Ask {
        call: Value,
        from: ClientId,
        seq: i32,
    },
    /// `/gui_set` of a widget of the window (a client's selection).
    Set {
        args: Vec<clausters_core::osc::OscType>,
        from: ClientId,
    },
}

/// Each window's queue.
#[derive(Debug, Default)]
pub(super) struct Scripted {
    windows: HashMap<i32, Queue>,
}

#[derive(Debug, Default)]
struct Queue {
    /// Whether a step is out, waiting for the application's word.
    busy: bool,
    steps: VecDeque<Step>,
}

impl Scripted {
    /// `step` queued behind the window's others.
    pub(super) fn push(&mut self, window: i32, step: Step) {
        self.windows
            .entry(window)
            .or_default()
            .steps
            .push_back(step);
    }

    /// **The next step of `window` to release**, or none while one is out
    /// or none is left; the window is then busy until [`Self::done`].
    pub(super) fn next(&mut self, window: i32) -> Option<Step> {
        let queue = self.windows.get_mut(&window)?;
        if queue.busy {
            return None;
        }
        let step = queue.steps.pop_front()?;
        queue.busy = true;
        Some(step)
    }

    /// Whether `window` has a step out.
    pub(super) fn busy(&self, window: i32) -> bool {
        self.windows.get(&window).is_some_and(|q| q.busy)
    }

    /// The step out on `window` is answered; a window with nothing left is
    /// forgotten.
    pub(super) fn done(&mut self, window: i32) {
        if let Some(queue) = self.windows.get_mut(&window) {
            queue.busy = false;
            if queue.steps.is_empty() {
                self.windows.remove(&window);
            }
        }
    }

    /// A window that went takes its steps with it.
    pub(super) fn forget(&mut self, window: i32) {
        self.windows.remove(&window);
    }
}

/// A host with no applications performs every step when it comes.
#[cfg(not(any(not(target_arch = "wasm32"), feature = "apps")))]
impl super::Host {
    /// Whether a script's step on `window` waits its turn: never here.
    pub(super) fn scripts(&self, _window: i32) -> bool {
        false
    }

    /// A step performed now.
    pub(super) fn script(&mut self, _window: i32, step: Step, effects: &mut Vec<HostEffect>) {
        match step {
            Step::Effect(effect) => effects.push(effect),
            Step::Query { id, from } => effects.push(HostEffect::Reply(self.info_of(id, from))),
            Step::Set { args, from } => self.set_now(&args, from, effects),
            Step::Ask { .. } => {}
        }
    }

    /// Nothing waits here.
    pub fn performed(&mut self, _window: i32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **One step out at a time, in their order**: the second waits for the
    /// first's answer, and a window with nothing left is forgotten.
    #[test]
    fn a_window_releases_one_step_at_a_time_in_order() {
        let mut s = Scripted::default();
        s.push(
            1,
            Step::Query {
                id: 10,
                from: ClientId::Web,
            },
        );
        s.push(
            1,
            Step::Query {
                id: 11,
                from: ClientId::Web,
            },
        );
        assert!(matches!(s.next(1), Some(Step::Query { id: 10, .. })));
        assert!(s.next(1).is_none(), "the first is out");
        assert!(s.busy(1));
        s.done(1);
        assert!(matches!(s.next(1), Some(Step::Query { id: 11, .. })));
        s.done(1);
        assert!(!s.busy(1));
        assert!(s.windows.is_empty());
    }
}
