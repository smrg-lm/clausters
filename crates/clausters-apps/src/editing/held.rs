//! **An engine the application holds**: what the audio editor's coordinator
//! asks of an engine embedded in the application's own process, in place of
//! the steps it hands a client to carry out on a server.
//!
//! The coordinator plays a file through its playback, which carries its ops
//! out in a [`Medium`]: an endpoint's applier answers OSC steps for a client
//! to send, and an engine the application holds builds the engine's commands
//! at once and answers none. The take is the other half. Over a server the
//! editor stitches its list into a join the server reads from disk; over an
//! engine it holds, the application reads its takes itself and the engine
//! plays what it is handed, so the coordinator gives the engine each file's
//! list as the document says it ([`HeldTake`]) and plays the buffers the
//! engine answers, one per channel.

use clausters_core::ids::IdSpaces;
use clausters_document::clipboard::SegmentPart;
use clausters_editing::apply::Medium;

/// **A file's take, as an engine the application holds reads it**: its
/// parts over files, at the engine's rate, and the revision they are of.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldTake {
    /// The file's identity in the editor, as its playback names it.
    pub file: u64,
    /// The file's contents revision: the same number is the same list.
    pub revision: u64,
    /// The parts, each a span of a file, its frames the engine's.
    pub parts: Vec<SegmentPart>,
    /// How many channels the take is.
    pub channels: u32,
    /// How many frames it comes to at `rate`.
    pub frames: u64,
    /// The rate it is read at: the engine's.
    pub rate: f64,
}

/// **What an engine the application holds gives the audio editor.**
pub trait HeldEngine: std::fmt::Debug + Send {
    /// A medium for one playback's ops, over this engine.
    fn medium(&mut self) -> Box<dyn Medium + Send>;
    /// The engine's sample rate.
    fn rate(&self) -> f64;
    /// **The take of `take.file` is `take` now**: read from here on, faded
    /// into where a reader plays it. Answers the buffers its channels'
    /// readers play, one per channel, allocated from `ids` the first time.
    fn take(&mut self, take: &HeldTake, ids: &mut IdSpaces) -> Result<Vec<i32>, String>;
    /// The revision the engine reads of `file`, if it reads it at all.
    fn revision(&self, file: u64) -> Option<u64>;
    /// `file` is closed: its reading stops and its buffers go back to `ids`.
    fn close(&mut self, file: u64, ids: &mut IdSpaces);
    /// Whether `transport` rolls, as the engine says now.
    fn rolling(&self, transport: i32) -> bool;
}
