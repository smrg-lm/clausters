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

use clausters_core::tempomap::{TempoChange, TempoMap};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::multitrack::{Extra, Multitrack};
use crate::view::View;
use crate::{Document, Lifetime, Opaque, SourceId, SourceRef};

/// The format this file was written in.
///
/// It moves when a reader that does not know the new shape would read the file
/// *wrongly* — never for an added field, which an older reader ignores and a
/// newer one defaults. So far there have been three.
///
/// **2** added [`Location::Segments`]: a source whose samples are spans of
/// other sources. [`Location`] is tagged and has no untagged arm, so a reader
/// that does not know the variant *fails* rather than reading it as something
/// else — which is the case the counter exists for, and the reason an added
/// `Location` moves it where an added field would not.
///
/// **3** measures the multitrack in **seconds**: a region's position, length
/// and fades, every automation point, the markers, the loop and punch spans and
/// a view's visible span and selection, which format 2 wrote in beats; and a
/// tempo entry states `tempo` in beats per second where it stated `bpm`. The
/// numbers keep their fields, so an older reader would read every one of them
/// wrongly — the counter's case exactly. [`migrate`] reads a format-2 file into
/// this one.
pub const FORMAT: u32 = 3;

/// The tempo a format-2 multitrack that stated none was read at, in beats per
/// second: one, the default every reader of that format drew and played it
/// with. Only [`migrate`] uses it, to put those beats in seconds where the file
/// itself said nothing.
pub const FORMAT_2_TEMPO: f64 = 1.0;

/// **A session written in an older format, as this one writes it.**
///
/// Applied to the JSON before it is read, so every reader — the crate's own,
/// the GUI host's, each client's — opens an old file the same way. A session
/// already at [`FORMAT`] (or newer, which [`Session::is_readable`] refuses) is
/// handed back untouched, so calling this on every read is safe.
///
/// From 2 to 3, every beat position of the multitrack is taken to seconds
/// through the tempo map the file saved, or [`FORMAT_2_TEMPO`] where it saved
/// none. A **length** is the difference of two positions, since how long four
/// beats last depends on where they start; a region's own curve is measured
/// from the region's start, and so is its fade in, while its fade out is
/// measured back from its end. The contents of a region — a window's seconds of
/// a recording, a node's beats — are not the multitrack's axis and are left as
/// they were.
pub fn migrate(mut written: Value) -> Value {
    let format = written.get("format").and_then(Value::as_u64).unwrap_or(1);
    // What is not an object is not a session, and reading it says so.
    if format >= 3 || !written.is_object() {
        return written;
    }
    let key = if written.get("multitrack").is_some() {
        "multitrack"
    } else {
        "arrangement"
    };
    let map = format_2_map(written.get(key));
    let secs = |beat: f64| map.secs_at(beat);
    if let Some(multitrack) = written.get_mut(key) {
        to_seconds(multitrack, &secs);
    }
    if let Some(Value::Array(views)) = written.get_mut("views") {
        for view in views {
            for field in ["visible", "selection"] {
                if let Some(span) = view.get_mut(field) {
                    span_to_seconds(span, &secs);
                }
            }
        }
    }
    written["format"] = json!(FORMAT);
    written
}

/// The tempo map a format-2 multitrack states: entries in beats per minute.
fn format_2_map(multitrack: Option<&Value>) -> TempoMap {
    let changes: Vec<TempoChange> = multitrack
        .and_then(|m| m.get("tempo"))
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|t| TempoChange {
                    beats: number(t.get("at")),
                    tempo: t.get("bpm").and_then(Value::as_f64).unwrap_or(60.0) / 60.0,
                    ramp: t.get("ramp").and_then(Value::as_bool).unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    TempoMap::from_changes(&changes, FORMAT_2_TEMPO)
        .unwrap_or_else(|_| TempoMap::new(FORMAT_2_TEMPO))
}

fn number(value: Option<&Value>) -> f64 {
    value.and_then(Value::as_f64).unwrap_or(0.0)
}

/// Every beat position of one multitrack, in seconds.
fn to_seconds(multitrack: &mut Value, secs: &dyn Fn(f64) -> f64) {
    if let Some(Value::Array(entries)) = multitrack.get_mut("tempo") {
        for entry in entries {
            if let Some(object) = entry.as_object_mut()
                && let Some(bpm) = object.remove("bpm")
            {
                object.insert("tempo".into(), json!(bpm.as_f64().unwrap_or(60.0) / 60.0));
            }
        }
    }
    if let Some(Value::Array(markers)) = multitrack.get_mut("markers") {
        for marker in markers {
            position_to_seconds(marker, "at", secs);
        }
    }
    for field in ["loop_span", "punch"] {
        if let Some(span) = multitrack.get_mut(field) {
            span_to_seconds(span, secs);
        }
    }
    let Some(Value::Array(tracks)) = multitrack.get_mut("tracks") else {
        return;
    };
    for track in tracks {
        if let Some(Value::Array(curves)) = track.get_mut("automation") {
            for curve in curves {
                points_to_seconds(curve, 0.0, secs);
            }
        }
        let Some(Value::Array(lanes)) = track.get_mut("lanes") else {
            continue;
        };
        for lane in lanes {
            let Some(Value::Array(regions)) = lane.get_mut("regions") else {
                continue;
            };
            for region in regions {
                region_to_seconds(region, secs);
            }
        }
    }
}

fn region_to_seconds(region: &mut Value, secs: &dyn Fn(f64) -> f64) {
    let position = number(region.get("position"));
    let end = position + number(region.get("length"));
    let (start, stop) = (secs(position), secs(end));
    region["position"] = json!(start);
    region["length"] = json!(stop - start);
    if let Some(fade) = region.get_mut("fade_in")
        && fade.is_object()
    {
        let length = number(fade.get("length"));
        fade["length"] = json!(secs(position + length) - start);
    }
    if let Some(fade) = region.get_mut("fade_out")
        && fade.is_object()
    {
        let length = number(fade.get("length"));
        fade["length"] = json!(stop - secs(end - length));
    }
    if let Some(Value::Array(curves)) = region.get_mut("automation") {
        for curve in curves {
            points_to_seconds(curve, position, secs);
        }
    }
}

/// A curve's points, measured from `origin` beats before and from
/// `secs(origin)` after.
fn points_to_seconds(curve: &mut Value, origin: f64, secs: &dyn Fn(f64) -> f64) {
    let base = secs(origin);
    if let Some(Value::Array(points)) = curve.get_mut("points") {
        for point in points {
            let at = number(point.get("at"));
            point["at"] = json!(secs(origin + at) - base);
        }
    }
}

fn position_to_seconds(value: &mut Value, field: &str, secs: &dyn Fn(f64) -> f64) {
    if let Some(at) = value.get(field).and_then(Value::as_f64) {
        value[field] = json!(secs(at));
    }
}

fn span_to_seconds(span: &mut Value, secs: &dyn Fn(f64) -> f64) {
    if span.is_object() {
        position_to_seconds(span, "start", secs);
        position_to_seconds(span, "end", secs);
    }
}

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
    /// **Spans of other sources, read back to back as one thing** — what a
    /// join over fragments makes, and what the user named a *pseudobuffer*
    /// when the segments were designed: something a reader reads like a
    /// recording, which owns no samples.
    ///
    /// It is in the source table rather than in the multitrack because that is what
    /// the table is: the document says what plays when and deliberately not
    /// where a source's samples are, since inside a running system a source is
    /// a server buffer, a mapped file or a rendered result. A source whose
    /// samples *are* spans of other sources is that same sentence one level
    /// in — and it keeps every reader downstream reading one shape, since a
    /// region over a join is a plain window onto a plain source.
    ///
    /// **The parts are arbitrary**: the same source or several, any valid
    /// range of each, in any order. Order is the reading order, which is the
    /// whole point — fragments put back in an order their source does not have
    /// is exactly what cannot be said as a window.
    ///
    /// A realized join is [`crate::multitrack::picture`]'s source like any
    /// other; what realizes it is the caller's (`/buffer_stitch` over pool
    /// buffers today, a prebuffered stream once a part may name a file — root
    /// `PLAN.md`). **This statement does not change when that does**, which is
    /// why the recipe is here and not in a call a client remembers making.
    Segments {
        /// The spans, in reading order.
        parts: Vec<Part>,
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

/// One span of a [`Location::Segments`]: which source, which frames of it, and
/// the fade at each end.
///
/// **Frames, not seconds**, unlike [`crate::SegmentRef`]: a part of a join is a
/// statement about samples — the unit `/buffer_stitch` takes it in and the unit
/// [`crate::SourceRef::range`] already speaks. A `SegmentRef` measures a
/// *window a multitrack places*, which is the musical side of the same fact and is
/// where seconds belong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    /// The source and the frames of it this part contributes, in its `range`.
    /// A part with no range contributes the whole of it.
    pub source: SourceRef,
    /// Frames of linear fade in at this part's head.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade_in: u64,
    /// Frames of linear fade out at its tail.
    ///
    /// The two are what an editor puts on a cut: a seam between spans that do
    /// not continue each other is a step, and a step is a click however well
    /// the frames are read.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fade_out: u64,
    /// Which channel of the source each channel of the join reads, or the
    /// identity mapping when absent — which is the ordinary case and the reason
    /// it is an option rather than a list every part spells.
    ///
    /// A negative entry is silence, as on the wire. Stated when the parts are
    /// not all the same width, which "any source, any range" allows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<i32>>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
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
    /// Fields a newer writer wrote. See [`Extra`].
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
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
            extra: Extra::new(),
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

/// A composition, saved: the multitrack, and where its samples are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The format this was written in. See [`FORMAT`].
    pub format: u32,
    /// **The multitrack**: its tracks, and the timeline they are placed on.
    #[serde(
        default,
        skip_serializing_if = "is_empty_multitrack",
        alias = "arrangement"
    )]
    pub multitrack: Multitrack,
    /// The general tree, for what is not an multitrack.
    ///
    /// **Absent means empty, not invalid** — a session that is only an
    /// arrangement is the shape this milestone is walking towards, and it has
    /// to be writable before the leg comes off rather than after.
    ///
    /// **This is the leg that is being walked off, and saying so is part of
    /// the design rather than an apology.** It is what every current reader
    /// opens - the standalone host, the clients' save and reopen - so it stays
    /// until the host binds the arrangement instead, which is a milestone of
    /// its own. What replaces it is already here: a
    /// [`Content::Composite`](crate::multitrack::Content::Composite) region
    /// carries this same tree, placed, so nothing the general model can say is
    /// lost by the move - it gains a position.
    #[serde(
        default = "Document::empty",
        skip_serializing_if = "Document::is_empty"
    )]
    pub document: Document,
    /// How the multitrack was being **looked at**: one entry per window.
    ///
    /// Presentation, parallel to the model and never inside it
    /// ([`crate::view`]). A session carries it for the reason every program in
    /// the field does — reopening a multitrack into the window it was left in is
    /// what a person expects — and a reader that ignores the field opens the
    /// same multitrack, since nothing here can change what plays.
    ///
    /// A **list** because a multitrack drawn in two windows has two views, and they
    /// disagree on purpose. Empty is the ordinary case: nothing was saved, so
    /// the window opens on its own defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<View>,
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
    /// Fields a newer writer wrote. See [`Extra`].
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Whether an arrangement says nothing at all, so an empty one stays out of the
/// file rather than writing an empty object into every session ever saved.
fn is_empty_multitrack(multitrack: &Multitrack) -> bool {
    *multitrack == Multitrack::new()
}

impl Session {
    /// A session over this document, with no arrangement and no sources yet.
    pub fn new(document: Document) -> Self {
        Self {
            format: FORMAT,
            multitrack: Multitrack::new(),
            views: Vec::new(),
            document,
            sources: BTreeMap::new(),
            provenance: None,
            extra: Extra::new(),
        }
    }

    /// Carries this multitrack.
    pub fn with_multitrack(mut self, multitrack: Multitrack) -> Self {
        self.multitrack = multitrack;
        self
    }

    /// Carries how the multitrack was being looked at. See [`Session::views`].
    pub fn with_view(mut self, view: View) -> Self {
        self.views.push(view);
        self
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

    /// Sources the multitrack names but the table does not hold — what an opening
    /// reader reports rather than discovering one element at a time.
    ///
    /// Both halves are walked: the arrangement's regions and the general tree.
    /// A reader that checked only one would open a session missing exactly the
    /// material the other half plays.
    pub fn dangling(&self) -> Vec<SourceId> {
        let mut missing = Vec::new();
        for region in self.multitrack.regions() {
            if let Some(window) = region.content.as_window()
                && let Some(source) = window.source.samples()
                && !self.sources.contains_key(&source.source)
                && !missing.contains(&source.source)
            {
                missing.push(source.source);
            }
        }
        self.document.walk(&mut |node| {
            let named: Vec<crate::SourceId> = match &node.body {
                crate::Body::Vector { source, .. } => vec![source.source],
                // Assembled samples names one source per window, and a table
                // covering only the first would reopen with the rest missing.
                crate::Body::Segments { segments, .. } => segments
                    .iter()
                    .filter_map(|s| s.source.samples())
                    .map(|source| source.source)
                    .collect(),
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

    /// A session read from its JSON, **migrated first** when it was written in
    /// an older format ([`migrate`]). The door every reader of a file goes
    /// through, so an old session opens the same everywhere.
    pub fn read(written: Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(migrate(written))
    }

    /// [`Session::read`] over the file's text.
    pub fn read_str(text: &str) -> Result<Self, serde_json::Error> {
        Self::read(serde_json::from_str(text)?)
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
