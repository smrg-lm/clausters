//! **What the host says to itself**, in one door and in both builds.
//!
//! The status bar ([`super::status`]) is what the host says to the *person at
//! the window*: what the hand asked for, and what came back. This is the other
//! half -- what the host says about its own working, which nobody asked for and
//! which is the only thing there is to read when a seam whose halves are each
//! correct does not work.
//!
//! # Why it is a door and not `tracing`
//!
//! The host is one program compiled twice, and `tracing` is only half of it: a
//! native build installs a subscriber (`bin/clausters-gui.rs`) and a browser
//! build installs nothing, because a wasm shim was out of scope when the web
//! front was written. So a hundred lines of diagnostics printed in a window and
//! **none of them in a tab** -- a defect diagnosable by reading the log on one
//! platform and not diagnosable at all on the other, which the crate's own rule
//! forbids: a behaviour that differs between native and browser is a defect,
//! never a platform's quirk.
//!
//! One door fixes that: the macros here format once and hand the line to
//! [`emit`], which is `tracing` natively and the browser console in a page.
//! Nothing else in `host` names `tracing` -- the two fronts keep their own
//! platform logs, which is what a front is for.
//!
//! # The four levels, and which two survive a release
//!
//! - [`warn!`] and [`info!`] are **always compiled**. A warning is about the
//!   world -- an unreadable font, a dropped client, a bundle that is not a
//!   manifest -- and a build that drops it is a build that cannot be supported.
//! - [`debug!`] and [`note!`] are **compiled only in a debug build**
//!   (`debug_assertions`), arguments and all: the format call is inside the
//!   `cfg`, so a release pays nothing, not even the formatting of a line
//!   nothing would read. A release build that is being *instrumented* turns
//!   them back on with the `diagnostics` feature -- which exists because the
//!   binaries the Python launcher stages are release ones, so without it the
//!   notes are invisible in exactly the path a manual test takes.
//!
//! [`note!`] is the one with a second destination. It writes a
//! [`status::Kind::Note`](super::status::Kind::Note) line onto a **window's own
//! status bar**, which is where a person debugging is already looking: the bar
//! opens into a log of the last several lines, so a note sits in the same
//! column as the gesture that produced it and the refusal that answered it.
//! That is the whole reason it is a separate level rather than a `debug!` with
//! a window id -- the destination differs, not the severity.
//!
//! # What it is not
//!
//! It is not a transcript and not an event log a program reads back: a note is
//! addressed to whoever is looking at the window now. Nothing here crosses the
//! wire, and no client can ask for it -- the same decision the status bar makes,
//! for the same reason.

// Only the note channel reaches the host, and only a debug build has one.
#[cfg(any(debug_assertions, feature = "diagnostics"))]
use super::Host;

/// How loud a line is, which is the only thing that decides where it lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Something is wrong with the world and the host carried on.
    Warn,
    /// Something happened that an operator would want in the log.
    Info,
    /// Something happened that only somebody debugging wants.
    Debug,
}

/// Puts one line where this build's platform log is.
///
/// The whole of the platform seam for diagnostics: `tracing` natively, the
/// browser console in a page. It takes a formatted string rather than
/// `tracing`'s structured fields because the other half of the seam has no
/// structure to give them to -- a console takes a line -- and a door whose two
/// sides carry different things is two doors.
pub fn emit(level: Level, line: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    match level {
        Level::Warn => tracing::warn!("{line}"),
        Level::Info => tracing::info!("{line}"),
        Level::Debug => tracing::debug!("{line}"),
    }
    #[cfg(target_arch = "wasm32")]
    {
        let prefix = match level {
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
        };
        web_sys::console::log_1(&format!("{prefix}: {line}").into());
    }
}

/// Says one note on a window's status bar, and in the platform log with it.
///
/// Called only through [`note!`], which is what keeps the formatting out of a
/// release build; it is `pub` because the macro expands at the call site.
#[cfg(any(debug_assertions, feature = "diagnostics"))]
pub fn note_on(host: &Host, def_id: i32, verb: &str, text: String) {
    emit(Level::Debug, &text);
    host.say(def_id, super::status::Line::of_note(verb, text));
}

/// A line about the world, kept in every build.
macro_rules! warn_ {
    ($($arg:tt)*) => {
        $crate::host::diag::emit($crate::host::diag::Level::Warn, &format!($($arg)*))
    };
}

/// A line an operator would want, kept in every build.
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::host::diag::emit($crate::host::diag::Level::Info, &format!($($arg)*))
    };
}

/// A line only somebody debugging wants. Compiled out of a release build,
/// arguments included -- unless that build asked for the `diagnostics` feature.
///
/// *Arguments included* is the part with a consequence: a binding whose **only**
/// reader is a `debug!` is unused in a release build and warns there. Spell it
/// `_name` at the binding and `{_name}` in the format string, which reads in
/// both builds; the alternative -- compiling the call in and branching on
/// `cfg!` -- leaves the line's text in the shipped binary, which is the thing
/// this level exists to avoid.
macro_rules! debug {
    ($($arg:tt)*) => {{
        #[cfg(any(debug_assertions, feature = "diagnostics"))]
        $crate::host::diag::emit($crate::host::diag::Level::Debug, &format!($($arg)*));
    }};
}

/// A line only somebody debugging wants, **on a window's status bar** as well
/// as in the platform log. Compiled out of a release build, arguments included
/// -- unless that build asked for the `diagnostics` feature.
///
/// `verb` is the first word of the line and the key the bar collapses on, so a
/// note repeated by every motion of one drag stays one line -- the same rule an
/// event's verb follows there.
macro_rules! note {
    ($host:expr, $def_id:expr, $verb:expr, $($arg:tt)*) => {{
        #[cfg(any(debug_assertions, feature = "diagnostics"))]
        $crate::host::diag::note_on($host, $def_id, $verb, format!($($arg)*));
    }};
}

// `warn` is spelled `warn_` where it is defined and re-exported under the name
// it is called by: a `macro_rules!` called `warn` cannot be re-exported at all,
// because the name is a built-in attribute's as well. Call sites say
// `diag::warn!` like every other level.
pub(crate) use warn_ as warn;
pub(crate) use {debug, info, note};
