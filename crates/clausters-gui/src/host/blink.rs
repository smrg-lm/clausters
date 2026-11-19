//! **The caret blinks**: while the keyboard is on something that takes
//! typed text (a field, the code), its caret shows and hides at a steady
//! pace, and it holds still while the hand works -- a key or a press shows
//! it at once and starts the pace again from there.
//!
//! The pace is the host's and applies to every window, as the platform's
//! does: [`Blink::DEFAULT_MS`] for a whole cycle, the same the webview editor
//! and the desktop's own text fields keep, and `0` for a caret that never
//! blinks. Nothing ticks for it: a front draws the window whose caret
//! changed phase and sleeps until the next change ([`Host::caret_due`]).

use std::cell::Cell;

use super::Host;

/// The caret's pace, and when it last held still.
#[derive(Debug, Clone, Copy)]
pub struct Blink {
    /// A whole cycle, shown then hidden, in milliseconds; `0` for none.
    pub cycle_ms: f64,
    /// When the hand last moved it, on the front's clock.
    since: f64,
}

impl Blink {
    /// The whole cycle a caret blinks with unless told otherwise.
    pub const DEFAULT_MS: f64 = 1200.0;
}

impl Default for Blink {
    fn default() -> Self {
        Self {
            cycle_ms: Self::DEFAULT_MS,
            since: 0.0,
        }
    }
}

impl Host {
    /// **Whether the caret of window `def_id` shows at `now_ms`**: always
    /// with no blinking, else in the first half of each cycle since the hand
    /// last moved it.
    pub fn caret_shown(&self, def_id: i32, now_ms: f64) -> bool {
        let half = self.blink.cycle_ms * 0.5;
        if half <= 0.0 || !self.focus_takes_text(def_id) {
            return true;
        }
        (((now_ms - self.blink.since).max(0.0) / half) as u64).is_multiple_of(2)
    }

    /// **When the caret next changes**, on the front's clock: the window
    /// that holds a caret, and the time; `None` with no caret or no
    /// blinking, when nothing has to wake.
    pub fn caret_due(&self, now_ms: f64) -> Option<(i32, f64)> {
        let half = self.blink.cycle_ms * 0.5;
        let (def_id, _) = self.focused?;
        if half <= 0.0 || !self.focus_takes_text(def_id) {
            return None;
        }
        let gone = (now_ms - self.blink.since).max(0.0);
        let next = ((gone / half).floor() + 1.0) * half;
        Some((def_id, self.blink.since + next))
    }

    /// **The hand moved the caret** at `now_ms` (a key, a press): it shows,
    /// and holds still for half a cycle.
    pub fn caret_moved(&mut self, now_ms: f64) {
        self.blink.since = now_ms;
    }

    /// The caret's whole cycle, in milliseconds; `0` stops it blinking.
    pub fn set_caret_blink(&mut self, cycle_ms: f64) {
        self.blink.cycle_ms = cycle_ms.max(0.0);
    }
}

thread_local! {
    /// The phase of the frame being drawn: whether a caret drawn now shows.
    static SHOWN: Cell<bool> = const { Cell::new(true) };
}

/// **Sets the caret's phase for the frame about to be drawn**, which every
/// element drawing a caret reads ([`shown`]): one window is drawn at a time,
/// on one thread.
pub(crate) fn set_phase(shown: bool) {
    SHOWN.with(|s| s.set(shown));
}

/// Whether a caret drawn in this frame shows.
pub(crate) fn shown() -> bool {
    SHOWN.with(Cell::get)
}
