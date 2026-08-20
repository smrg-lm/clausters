//! The document written to a file, and the table that says where its samples
//! is.
//!
//! A [`crate::Document`] describes *what plays when*; it deliberately does not
//! say where a source lives, because inside a running system a source is a
//! server buffer, a mapped file or a rendered result and the tree has no
//! business knowing which. A **session** is the document plus exactly that
//! missing half: a source table, so the thing can be closed and opened again.
//!
//! # Why the format is here and not in a client
//!
//! It has two writers in two languages — a language client, and a `standalone`
//! host with no language attached — and a format with two writers in two
//! languages is a format that drifts. So the shape lives once, beside the tree
//! it carries.
//!
//! # A source is named, located and dated
//!
//! [`Source`] carries where the samples are ([`Location`]), whether it outlives
//! the session ([`crate::Lifetime`]), which generation of its content this is,
//! and its shape. Two fields are the ones a naive format leaves out and then
//! cannot add:
//!
//! - **Provenance** — a reference to whatever produced it, carried opaquely.
//!   It is what makes re-generating possible *without the document knowing
//!   how*: the recipe is in the language that wrote it, and the session only
//!   has to not lose it.
//! - **An open edit** ([`OpenEdit`]) — a destructive edit session over this
//!   samples that has not been confirmed. A save never blocks on a
//!   confirmation, so a saved session can and must be able to say *this is a
//!   working copy of that, and the person has not decided yet*. Without the
//!   field, saving mid-edit either silently confirms the edit or refuses to
//!   save, and both make saving mean something it should not.
//!
//! # What a host with no language shows
//!
//! A generator's *code* is opaque, so a host that embeds no interpreter cannot
//! run it. What it can show is what the generator **last produced**, which is
//! ordinary tree and lives on the generator node itself
//! ([`crate::Node::rendered`]). That is the frozen floor the standalone
//! decision rests on, and it is why it is part of the format rather than a
//! cache: a cache can be missing, and then there is nothing to draw.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Document, Lifetime, Opaque, SourceId};

/// The format this file was written in.
///
/// It moves when a reader that does not know the new shape would read the file
/// *wrongly* — never for an added field, which an older reader ignores and a
/// newer one defaults. So far there has been one.
pub const FORMAT: u32 = 1;

/// Where samples actually is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "at", rename_all = "lowercase")]
pub enum Location {
    /// A file. Relative paths are resolved against the session's own folder,
    /// which is what makes a session directory movable; an absolute one names
    /// the user's own file, which a session must never copy or rewrite.
    File {
        /// The path as written.
        path: String,
    },
    /// Samples that exist only in the running system — a server buffer never
    /// exported, a result never written down.
    ///
    /// A session may hold one, because saving must not be blocked by it, but a
    /// reader that finds one knows the samples are not there: it opens with
    /// that element unresolved rather than pretending. [`Session::volatile`]
    /// is what a save consults before promising the file is complete.
    Volatile,
}

/// A destructive edit session over one source, and whether it has been
/// confirmed.
///
/// See the module docs: this exists so a save mid-edit can be honest instead of
/// blocking or deciding for the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenEdit {
    /// The samples this working copy was made from. Untouched — the original
    /// is never written — so discarding is dropping the copy.
    pub from: SourceId,
    /// Whether the person has confirmed the edit. `false` in a saved session
    /// means the edit is still open and reopens that way.
    #[serde(default)]
    pub confirmed: bool,
}

/// One entry in the session's source table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// Where it is.
    pub location: Location,
    /// Whether it outlives the session.
    pub lifetime: Lifetime,
    /// Which generation of its content this is — the source half of the two
    /// counters, so a reader holding an older copy knows to re-read.
    #[serde(default)]
    pub generation: u64,
    /// Channels, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u32>,
    /// Frames per channel, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<u64>,
    /// The rate it was recorded or rendered at, when known. Carried, never
    /// acted on — resampling is an edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<f64>,
    /// What produced it, carried opaquely and never interpreted. Absent for
    /// samples the user imported, which was produced by nothing here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Opaque>,
    /// The destructive edit open over it, if one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editing: Option<OpenEdit>,
}

impl Source {
    /// Samples in a file.
    pub fn file(path: impl Into<String>, lifetime: Lifetime) -> Self {
        Self {
            location: Location::File { path: path.into() },
            lifetime,
            generation: 0,
            channels: None,
            frames: None,
            sample_rate: None,
            provenance: None,
            editing: None,
        }
    }

    /// Samples that have not been written down.
    pub fn volatile(lifetime: Lifetime) -> Self {
        Self {
            location: Location::Volatile,
            ..Self::file("", lifetime)
        }
    }

    /// Its shape.
    pub fn shaped(mut self, channels: u32, frames: u64, sample_rate: f64) -> Self {
        self.channels = Some(channels);
        self.frames = Some(frames);
        self.sample_rate = Some(sample_rate);
        self
    }

    /// What produced it.
    pub fn produced_by(mut self, provenance: Opaque) -> Self {
        self.provenance = Some(provenance);
        self
    }

    /// Marks it as an unconfirmed working copy of `from`.
    pub fn editing(mut self, from: SourceId) -> Self {
        self.editing = Some(OpenEdit {
            from,
            confirmed: false,
        });
        self
    }

    /// Whether a destructive edit is open and undecided over these samples.
    pub fn is_being_edited(&self) -> bool {
        self.editing.as_ref().is_some_and(|e| !e.confirmed)
    }

    /// Whether the samples are somewhere a reader could find them.
    pub fn is_resolvable(&self) -> bool {
        matches!(&self.location, Location::File { path } if !path.is_empty())
    }
}

/// A composition, saved: the document, and where its samples are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The format this was written in. See [`FORMAT`].
    pub format: u32,
    /// The composition.
    pub document: Document,
    /// Where each source is. A `BTreeMap`, so a written session is stable
    /// under re-saving and a diff of two saves is the edits and not the
    /// iteration order.
    #[serde(default)]
    pub sources: BTreeMap<SourceId, Source>,
    /// What produced the session as a whole — the scripts behind it — carried
    /// opaquely. The document never knows how to re-run them; it only has to
    /// not lose the reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Opaque>,
}

impl Session {
    /// A session over this document, with no sources yet.
    pub fn new(document: Document) -> Self {
        Self {
            format: FORMAT,
            document,
            sources: BTreeMap::new(),
            provenance: None,
        }
    }

    /// Records where a source is.
    pub fn with_source(mut self, id: SourceId, source: Source) -> Self {
        self.sources.insert(id, source);
        self
    }

    /// Records what produced the session.
    pub fn produced_by(mut self, provenance: Opaque) -> Self {
        self.provenance = Some(provenance);
        self
    }

    /// The source a reference names, if the table has it.
    pub fn source(&self, id: SourceId) -> Option<&Source> {
        self.sources.get(&id)
    }

    /// Sources whose samples are not written down anywhere — what a save
    /// consults before promising the file is complete.
    pub fn volatile(&self) -> Vec<SourceId> {
        self.sources
            .iter()
            .filter(|(_, s)| !s.is_resolvable())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Sources with a destructive edit still open and undecided.
    pub fn open_edits(&self) -> Vec<SourceId> {
        self.sources
            .iter()
            .filter(|(_, s)| s.is_being_edited())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Sources the tree names but the table does not hold — what an opening
    /// reader reports rather than discovering one element at a time.
    pub fn dangling(&self) -> Vec<SourceId> {
        let mut missing = Vec::new();
        self.document.root.walk(&mut |node| {
            let named: Vec<crate::SourceId> = match &node.body {
                crate::Body::Vector { source, .. } => vec![source.source],
                // Assembled samples names one source per window, and a table
                // covering only the first would reopen with the rest missing.
                crate::Body::Segments { segments, .. } => {
                    segments.iter().map(|s| s.source.source).collect()
                }
                _ => Vec::new(),
            };
            for source in named {
                if !self.sources.contains_key(&source) && !missing.contains(&source) {
                    missing.push(source);
                }
            }
        });
        missing
    }

    /// Promotes a temporary working copy to one that is saved beside the
    /// document, **leaving the edit open**.
    ///
    /// What a save mid-edit does. The two alternatives both make saving mean
    /// something it should not: auto-confirming turns a save into an edit, and
    /// refusing until the edit is settled makes the safest habit in the program
    /// the one that is blocked. So the lifetime changes, the document keeps
    /// naming the same source, and the log is untouched.
    pub fn promote(&mut self, id: SourceId) -> bool {
        let Some(source) = self.sources.get_mut(&id) else {
            return false;
        };
        if source.lifetime != Lifetime::Temporary {
            return false;
        }
        source.lifetime = Lifetime::Session;
        true
    }

    /// Confirms the edit open over a source: the working copy becomes the
    /// samples, and there is nothing left undecided about it.
    pub fn confirm(&mut self, id: SourceId) -> bool {
        let Some(source) = self.sources.get_mut(&id) else {
            return false;
        };
        let Some(edit) = &mut source.editing else {
            return false;
        };
        edit.confirmed = true;
        true
    }

    /// Whether this build can read the file at all.
    ///
    /// A newer *format* is refused rather than half-read; a newer field inside
    /// a format this build knows is not a version change, and is ignored on
    /// the way through — the same rule [`crate::Body::Unknown`] follows.
    pub fn is_readable(&self) -> bool {
        self.format <= FORMAT
    }
}

#[cfg(test)]
mod tests;
