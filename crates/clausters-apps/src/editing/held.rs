//! **An engine the application holds**: what an editing application's
//! members ask of the engine embedded in its own process.
//!
//! A member plays through its playback, which carries its ops out in a
//! [`Medium`]: an engine the application holds builds the engine's commands
//! at once. The take is the other half: the application reads its takes
//! itself and the engine plays what it is handed, so a member gives the
//! engine each **reader**'s take as it holds it -- its tree of parts, shared,
//! and the files they read ([`HeldTake`], `crate::takes`) -- and plays the buffers the engine
//! answers, one per channel. A reader is what the member names it by: an
//! audio editor's file, a multitrack's track. Handing a take over again
//! after an edit is a reference.

use clausters_core::ids::IdSpaces;

use crate::editing::MemberId;
use crate::takes::HeldTake;
use clausters_editing::apply::Medium;

/// **What an engine the application holds gives its members.**
pub trait HeldEngine: std::fmt::Debug + Send {
    /// A medium for one playback's ops, over this engine.
    fn medium(&mut self) -> Box<dyn Medium + Send>;
    /// The engine's sample rate.
    fn rate(&self) -> f64;
    /// **The take of member `member`'s `take.reader` is `take` now**: read
    /// from here on, faded into where a reader plays it. Answers the buffers
    /// its channels' readers play, one per channel, allocated from `ids` the
    /// first time.
    ///
    /// A reader is named by its member and the member's own name for it:
    /// two windows each number their files from one, and keyed by the file
    /// alone the second window's take was read into the first's buffers.
    fn take(
        &mut self,
        member: MemberId,
        take: &HeldTake,
        ids: &mut IdSpaces,
    ) -> Result<Vec<i32>, String>;
    /// The revision the engine reads of member `member`'s `reader`, if it
    /// reads it at all.
    fn revision(&self, member: MemberId, reader: u64) -> Option<u64>;
    /// Member `member`'s `reader` is gone: its reading stops and its buffers
    /// go back to `ids`.
    fn close(&mut self, member: MemberId, reader: u64, ids: &mut IdSpaces);
    /// **Member `member` is gone**: every reading of its stops, as
    /// [`Self::close`] stops one.
    fn close_member(&mut self, member: MemberId, ids: &mut IdSpaces);
    /// Whether `transport` rolls, as the engine says now.
    fn rolling(&self, transport: i32) -> bool;
    /// Control bus `bus` as the engine left it last: what a meter it plays
    /// through reads.
    fn control(&self, bus: usize) -> f32;
    /// **How many samples its readers played as silence** for want of a
    /// frame read in time, since it opened: a dropout, counted.
    fn starved(&self) -> u64;
}
