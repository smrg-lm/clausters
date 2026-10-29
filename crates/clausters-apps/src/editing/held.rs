//! **An engine the application holds**: what an editing application's
//! members ask of the engine embedded in its own process.
//!
//! A member plays through its playback, which carries its ops out in a
//! [`Medium`]: an engine the application holds builds the engine's commands
//! at once. The take is the other half: the application reads its takes
//! itself and the engine plays what it is handed, so a member gives the
//! engine each **reader**'s take as it holds it -- its tree of parts, shared,
//! and the files they read ([`HeldTake`]) -- and plays the buffers the engine
//! answers, one per channel. A reader is what the member names it by: an
//! audio editor's file, a multitrack's track. Handing a take over again
//! after an edit is a reference.

use std::collections::BTreeMap;
use std::sync::Arc;

use clausters_core::ids::IdSpaces;

use crate::editing::MemberId;
use clausters_document::SourceId;
use clausters_document::multitrack::read::PlacedRegion;
use clausters_document::parts::{FileList, TakeFile};
use clausters_editing::apply::Medium;

/// **What a reader's take is made of**: an audio editor's parts back to
/// back, or a multitrack's track, its regions placed on it.
#[derive(Clone, Debug)]
pub enum HeldList {
    /// The parts of a take over files, shared with the editor's take and
    /// counted at the list's rate.
    Parts(FileList),
    /// A track's regions, each a window onto a source, summed where they
    /// overlap.
    Placed(PlacedList),
}

/// **A track as an engine the application holds reads it**: its regions
/// placed on its frames at the engine's rate, and what each source is.
#[derive(Clone, Debug)]
pub struct PlacedList {
    /// The regions, by where they start.
    pub regions: Arc<[PlacedRegion]>,
    /// What each source the regions name is.
    pub sources: Arc<BTreeMap<SourceId, PlacedSource>>,
    /// One past the last frame any region sounds on.
    pub frames: u64,
}

/// **What a region's source is**, as its reader reads it.
#[derive(Clone, Debug)]
pub enum PlacedSource {
    /// A sound file.
    File(TakeFile),
    /// A join: the parts of other sources, read back to back.
    Parts(FileList),
}

/// **A reader's take, as an engine the application holds reads it**: its
/// parts or its regions, at the engine's rate, and the revision they are of.
#[derive(Clone, Debug)]
pub struct HeldTake {
    /// What reads it, as its member names it: an audio editor's file, a
    /// multitrack's track, as its playback names it.
    pub reader: u64,
    /// The take's revision: the same number is the same list.
    pub revision: u64,
    /// What the take is made of.
    pub list: HeldList,
    /// How many channels the take is.
    pub channels: u32,
    /// The rate it is read at: the engine's.
    pub rate: f64,
}

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
