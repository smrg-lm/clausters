//! **A take as it is handed over**: what a reader's take is made of when an
//! engine the application holds reads it, and a sound file's header as a
//! caller reads it.
//!
//! These are what an editor says of its takes to whoever plays or reads
//! them, so they sit below every application: an audio editor answers its
//! parts back to back, a multitrack answers a track's regions placed on it,
//! and the editing context (`crate::editing`) carries either to the engine
//! it holds without the editors knowing the context.

use std::collections::BTreeMap;
use std::sync::Arc;

use clausters_document::SourceId;
use clausters_document::multitrack::read::PlacedRegion;
use clausters_document::parts::{FileList, TakeFile};

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

/// A sound file's header, as an editing context's caller reads it
/// (`crate::editing::Caller::header`).
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// Frames per channel.
    pub frames: u64,
    /// Channels.
    pub channels: u32,
    /// The rate it was written at.
    pub rate: f64,
    /// Its container (`wav`, `w64`, `rf64`, `flac`, ...).
    pub codec: String,
    /// Its sample format (`float`, `int24`, `int16`, ...), where it says.
    pub sample_format: Option<String>,
}
