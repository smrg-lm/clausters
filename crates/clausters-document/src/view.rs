//! The **presentation**: what a window shows of a multitrack, beside the multitrack and
//! never inside it.
//!
//! A [`View`] is one window's picture of one [`Multitrack`]: where it is
//! looking, how far it is zoomed, what the hand is holding, how tall each track
//! is drawn. None of that is what the multitrack *is* — the four-layer rule, and this
//! project has said three times that a selection and a zoom are each window's
//! and never the composition's — and all of it is state a person loses on a
//! reopen unless something writes it down.
//!
//! # Parallel to the model, never a field on it
//!
//! The shape is Live's, and it is deliberate: `Song.View`, `Track.View` and
//! `Application.View` are objects **beside** their model objects rather than
//! children, with presentation on one side and functional data on the other,
//! both readable and writable by a script. So a [`TrackView`] is looked up by
//! the track's id rather than held by the track, and an [`Multitrack`] round
//! trips byte for byte whether or not a view of it exists.
//!
//! Two things follow, and both are the point:
//!
//! - **The model stays clean.** A reader that wants the multitrack reads the multitrack.
//!   Nothing in this module can make a track sound different, and nothing here
//!   is ever consulted by an edit.
//! - **Screen state stops being an anonymous blob.** It was reachable only from
//!   inside the host, so it could not be saved, restored, set from a script or
//!   named in a bug report. Now it is a value with fields.
//!
//! # There is more than one of them
//!
//! A multitrack drawn in two windows has two views, and they disagree on purpose —
//! that is what a second window is *for*. Live holds the same track as a column
//! of slots in one picture and a timeline of clips in another; we hold a list.
//! A format that could carry only one would push the second back to being
//! anonymous, which is the thing this module exists to stop.
//!
//! # What survives, and what goes
//!
//! A view entry for an object the multitrack no longer holds is **dropped**
//! ([`View::prune`]), and that is the same rule the client's screen-state tables
//! were fixed to obey: state goes when the thing goes. Keeping it is worse than
//! losing it — a zoom that survives onto a lane which is not the same lane is a
//! defect that looks like a feature.
//!
//! # Where a file may carry one, and where nothing may
//!
//! A session may hold views ([`crate::Session::views`]), because reopening a
//! multitrack into the window it was left in is what every program in the field
//! does. Nothing here ever reaches the **document** or the **history**: a view
//! is not edited through an intent, an undo never puts a scroll back, and a
//! [`crate::Log`] that recorded a zoom would make the person's own last edit
//! two steps away.

pub mod catalogue;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::NodeId;
use crate::multitrack::{Extra, Multitrack, Span};
use crate::timebase::Beat;

/// One window's picture of one multitrack.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct View {
    /// What the window is called, when a person named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The stretch of the timeline on screen — the zoom and the horizontal
    /// scroll, which are one fact and not two. `None` shows the whole multitrack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<Span>,
    /// How far down the tracks the window is scrolled, in its own units. Not a
    /// beat and not a track index: what a vertical scroll is measured in is the
    /// window's business, and this carries it without reading it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub scroll: f64,
    /// The grid this window snaps to, in beats. Zero snaps nothing.
    ///
    /// A **musical** grid over a multitrack placed in seconds: the window takes
    /// it through the multitrack's tempo map to the seconds an edit snaps to,
    /// so it is the ruler's configuration and not a unit of the placement.
    ///
    /// It is here rather than in the multitrack because two windows over one multitrack
    /// may snap differently — the arranger to a bar, the editor below it to a
    /// sixteenth — which is exactly the case a single grid on the multitrack could
    /// not express.
    #[serde(default, skip_serializing_if = "is_origin")]
    pub quant: Beat,
    /// Whether the window follows its content: `true` refits a window that was
    /// showing the whole multitrack when the multitrack grows. `false` says the window is
    /// the reader's, and nothing moves it — which is what an editor wants,
    /// since a content change is mostly the reader's own edit and a view that
    /// re-frames itself under the hand that edited it is the window starting
    /// over.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub autofit: bool,
    /// The time range the hand swept, when it swept one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<Span>,
    /// What the hand is holding: regions, lanes or tracks, by id.
    ///
    /// One list rather than one per kind, because the multitrack has one id space —
    /// a region's identity is its own and not its source's, and so is a lane's
    /// and a track's. What a selected id *is* is answered by looking it up.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected: Vec<NodeId>,
    /// What a keystroke is aimed at, which is not the same as what is selected:
    /// a hand may hold six regions and be typing at one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused: Option<NodeId>,
    /// The region the detail editor below is showing, when the window has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<NodeId>,
    /// How each track is drawn, by the track's id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tracks: BTreeMap<NodeId, TrackView>,
    /// How each lane is drawn, by the lane's id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lanes: BTreeMap<NodeId, LaneView>,
    /// Fields a newer writer wrote. See [`Extra`].
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// How one track is drawn.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TrackView {
    /// How tall its row is, in the window's own units. `None` is the window's
    /// default, which is what a track nobody resized has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Whether the row is collapsed to its header.
    #[serde(default, skip_serializing_if = "is_no")]
    pub collapsed: bool,
    /// Whether the track's other lanes are shown under the one that plays —
    /// comping open, in a word. Closed by default: a track with six takes on it
    /// is one row until somebody asks to see them.
    #[serde(default, skip_serializing_if = "is_no")]
    pub lanes_shown: bool,
    /// The colour the track is drawn in, as the client writes a colour, carried
    /// and never read. A colour is presentation by every definition this
    /// project uses, and it is saved with the session for the same reason a
    /// height is: the person chose it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Fields a newer writer wrote. See [`Extra`].
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// How one lane is drawn.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LaneView {
    /// How tall its row is when the track's lanes are shown. See
    /// [`TrackView::height`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Fields a newer writer wrote. See [`Extra`].
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

fn yes() -> bool {
    true
}

fn is_yes(b: &bool) -> bool {
    *b
}

fn is_no(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &f64) -> bool {
    *n == 0.0
}

fn is_origin(beat: &Beat) -> bool {
    *beat == Beat::ZERO
}

impl View {
    /// A window that says nothing: the whole multitrack, no grid, nothing held.
    pub fn new() -> Self {
        Self {
            autofit: true,
            ..Self::default()
        }
    }

    /// Names it.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// How this track is drawn, or the default when nobody has touched it.
    pub fn track(&self, id: NodeId) -> TrackView {
        self.tracks.get(&id).cloned().unwrap_or_default()
    }

    /// How this track is drawn, to be edited — created on first use, which is
    /// what makes "nobody has touched it" cost nothing to store.
    pub fn track_mut(&mut self, id: NodeId) -> &mut TrackView {
        self.tracks.entry(id).or_default()
    }

    /// How this lane is drawn, or the default.
    pub fn lane(&self, id: NodeId) -> LaneView {
        self.lanes.get(&id).cloned().unwrap_or_default()
    }

    /// How this lane is drawn, to be edited. See [`View::track_mut`].
    pub fn lane_mut(&mut self, id: NodeId) -> &mut LaneView {
        self.lanes.entry(id).or_default()
    }

    /// Drops everything this view says about objects the multitrack no longer holds,
    /// and answers whether anything went.
    ///
    /// **State goes when the thing goes**, which is the rule the client's
    /// screen-state tables were fixed to obey after one of them handed a freed
    /// object's expansion to whatever was allocated next. Here the failure would
    /// be quieter and worse: an id is reused by a client that mints them, and a
    /// zoom kept for a lane that is not the same lane is a defect that looks
    /// like a feature.
    pub fn prune(&mut self, multitrack: &Multitrack) -> bool {
        let mut held: Vec<NodeId> = Vec::new();
        for track in &multitrack.tracks {
            held.push(track.id);
            for lane in &track.lanes {
                held.push(lane.id);
                held.extend(lane.regions.iter().map(|r| r.id));
            }
            held.extend(track.automation.iter().map(|a| a.id));
        }
        let before = (
            self.tracks.len(),
            self.lanes.len(),
            self.selected.len(),
            self.focused,
            self.detail,
        );
        self.tracks.retain(|id, _| held.contains(id));
        self.lanes.retain(|id, _| held.contains(id));
        self.selected.retain(|id| held.contains(id));
        self.focused = self.focused.filter(|id| held.contains(id));
        self.detail = self.detail.filter(|id| held.contains(id));
        before
            != (
                self.tracks.len(),
                self.lanes.len(),
                self.selected.len(),
                self.focused,
                self.detail,
            )
    }
}

/// The `/gui_event` tags that are **not** edits of the structure: what a view
/// is looking at, and where the hand is.
///
/// This module says the rule in prose — a selection and a zoom are each
/// window's and never the composition's — and the routing table is that rule as
/// a value, so a client can obey it without restating it. It is here rather
/// than in a client because it was written twice, once per language, and two
/// copies of a list of words drift the way every duplicated table drifts: a tag
/// added to one of them makes that client answer a gesture the other one edits
/// with.
///
/// What a client does with the tags is still the client's: it writes them into
/// its own screen state, which is not this crate's to hold. What is settled
/// here is only *which ones* those are.
pub const NOT_AN_EDIT: [&str; 8] = [
    "selection",
    "view",
    "view_x",
    "view_y",
    "layer",
    "focus",
    "locate",
    "height",
];

/// Whether `tag` names screen state rather than an edit — [`NOT_AN_EDIT`] asked
/// of one tag.
#[must_use]
pub fn is_screen_state(tag: &str) -> bool {
    NOT_AN_EDIT.contains(&tag)
}

#[cfg(test)]
mod tests;
