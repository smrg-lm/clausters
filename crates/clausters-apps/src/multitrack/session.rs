//! **The multitrack's session file**: the envelope every application's file
//! shares ([`SessionFile`]), with what the multitrack keeps in it.
//!
//! What is the multitrack's is three things: the [`Multitrack`] itself, how
//! each window was looking at it ([`MultitrackView`]) and where a pass over it
//! ends ([`End`]). They are read out of the envelope's
//! [`SessionFile::contents`] and written back into it, so the JSON is the one
//! the format has always had -- `multitrack`, `views` and `end` beside
//! `sources` at the top level -- and a file another application writes over
//! the same envelope is read by that application, not by this one.
//!
//! A [`MultitrackSession`] reads as its envelope too (it dereferences to the
//! [`SessionFile`]), so `session.sources` and `session.volatile()` are the
//! envelope's whichever application's file it is.

use std::ops::{Deref, DerefMut};

use clausters_document::multitrack::Multitrack;
use clausters_document::session::{End, SessionFile};
use clausters_document::{Document, Opaque, Source, SourceId};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::view::MultitrackView;

/// **The application a multitrack's session file belongs to**, as its
/// metadata names it.
pub const APPLICATION: &str = "multitrack";

/// A multitrack, saved: its envelope, its tracks, its windows' views and
/// where a pass ends.
#[derive(Debug, Clone, PartialEq)]
pub struct MultitrackSession {
    /// What every application's file holds: the format, the metadata, the
    /// general tree, the source table and the provenance, and the fields a
    /// newer writer wrote.
    pub file: SessionFile,
    /// **The multitrack**: its tracks, and the timeline they are placed on.
    pub multitrack: Multitrack,
    /// How the multitrack was being **looked at**: one entry per window.
    ///
    /// Presentation, parallel to the model and never inside it
    /// ([`super::view`]). A session carries it because reopening a multitrack
    /// into the window it was left in is what a person expects, and a reader
    /// that ignores the field opens the same multitrack, since nothing here
    /// can change what plays.
    ///
    /// A **list** because a multitrack drawn in two windows has two views, and
    /// they disagree on purpose. Empty is the ordinary case: nothing was
    /// saved, so the window opens on its own defaults.
    pub views: Vec<MultitrackView>,
    /// **Where a pass over the multitrack ends** ([`End`]): the transport's
    /// own switch, kept so a session opened again stops where it stopped
    /// before. Not in a view, since it changes what is heard and a view
    /// changes nothing that plays; and not in the multitrack, since it is no
    /// edit and no undo puts it back.
    pub end: End,
}

impl Deref for MultitrackSession {
    type Target = SessionFile;

    fn deref(&self) -> &SessionFile {
        &self.file
    }
}

impl DerefMut for MultitrackSession {
    fn deref_mut(&mut self) -> &mut SessionFile {
        &mut self.file
    }
}

/// The keys the multitrack keeps in its envelope. `arrangement` is what the
/// multitrack's key was called before 2026-09-08, read so a session written
/// then still opens.
const MULTITRACK: &str = "multitrack";
const ARRANGEMENT: &str = "arrangement";
const VIEWS: &str = "views";
const END: &str = "end";

impl MultitrackSession {
    /// A session over this document, with an empty multitrack and no sources
    /// yet.
    pub fn new(document: Document) -> Self {
        Self {
            file: SessionFile::new(APPLICATION, document),
            multitrack: Multitrack::new(),
            views: Vec::new(),
            end: End::Open,
        }
    }

    /// Carries this multitrack.
    pub fn with_multitrack(mut self, multitrack: Multitrack) -> Self {
        self.multitrack = multitrack;
        self
    }

    /// Carries how the multitrack was being looked at. See
    /// [`MultitrackSession::views`].
    pub fn with_view(mut self, view: MultitrackView) -> Self {
        self.views.push(view);
        self
    }

    /// Records where a source is.
    pub fn with_source(mut self, id: SourceId, source: Source) -> Self {
        self.file.sources.insert(id, source);
        self
    }

    /// Records what produced the session.
    pub fn produced_by(mut self, provenance: Opaque) -> Self {
        self.file.provenance = Some(provenance);
        self
    }

    /// Sources the session names but its table does not hold -- what an
    /// opening reader reports rather than discovering one element at a time.
    ///
    /// Both halves are walked: the multitrack's regions and the general tree.
    /// A reader that checked only one would open a session missing exactly the
    /// samples the other half plays.
    pub fn dangling(&self) -> Vec<SourceId> {
        let mut missing = Vec::new();
        for region in self.multitrack.regions() {
            if let Some(source) = region.content.source()
                && !self.file.sources.contains_key(&source)
                && !missing.contains(&source)
            {
                missing.push(source);
            }
        }
        for source in self.file.dangling() {
            if !missing.contains(&source) {
                missing.push(source);
            }
        }
        missing
    }

    /// The multitrack's session out of an envelope: what the multitrack keeps
    /// is taken out of its contents, and the rest stays there, carried.
    ///
    /// **A file another application's metadata names is refused** -- it is
    /// not a multitrack's, whatever its JSON happens to hold.
    pub fn from_file(mut file: SessionFile) -> Result<Self, serde_json::Error> {
        if !file.belongs_to(APPLICATION) {
            return Err(serde_json::Error::custom(format!(
                "the file belongs to the {} application, not to the multitrack",
                file.metadata.application
            )));
        }
        let multitrack = match file
            .contents
            .remove(MULTITRACK)
            .or_else(|| file.contents.remove(ARRANGEMENT))
        {
            Some(written) => serde_json::from_value(written)?,
            None => Multitrack::new(),
        };
        let views = match file.contents.remove(VIEWS) {
            Some(written) => serde_json::from_value(written)?,
            None => Vec::new(),
        };
        let end = match file.contents.remove(END) {
            Some(written) => serde_json::from_value(written)?,
            None => End::Open,
        };
        Ok(Self {
            file,
            multitrack,
            views,
            end,
        })
    }

    /// The envelope with what the multitrack keeps written into its contents:
    /// what a save writes. An empty multitrack, no views and an open end
    /// write nothing, as they always have.
    pub fn to_file(&self) -> Result<SessionFile, serde_json::Error> {
        let mut file = self.file.clone();
        if self.multitrack != Multitrack::new() {
            file.contents.insert(
                MULTITRACK.to_string(),
                serde_json::to_value(&self.multitrack)?,
            );
        }
        if !self.views.is_empty() {
            file.contents
                .insert(VIEWS.to_string(), serde_json::to_value(&self.views)?);
        }
        if !self.end.is_open() {
            file.contents.insert(END.to_string(), self.end.to_json());
        }
        Ok(file)
    }

    /// A multitrack's session read from its JSON, **migrated first** when it
    /// was written in an older format ([`SessionFile::read`]), and refused
    /// when it belongs to another application ([`MultitrackSession::from_file`]).
    pub fn read(written: Value) -> Result<Self, serde_json::Error> {
        Self::from_file(SessionFile::read(written)?)
    }

    /// [`MultitrackSession::read`] over the file's text.
    pub fn read_str(text: &str) -> Result<Self, serde_json::Error> {
        Self::read(serde_json::from_str(text)?)
    }

    /// [`MultitrackSession::read`] over the file's bytes.
    pub fn read_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        Self::read(serde_json::from_slice(bytes)?)
    }
}

impl Serialize for MultitrackSession {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_file()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MultitrackSession {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let file = SessionFile::deserialize(deserializer)?;
        Self::from_file(file).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests;
