//! A timeline of events, and the second structure a roll can be opened over.
//!
//! The crate's **fourth** editable domain, and the one that finishes the set the
//! clients' views ask for: a roll draws events, and until now the only way to
//! write one back was [`Intent::SetMembers`](crate::Intent::SetMembers) — an
//! aggregate's members, which needs a tree to be a member *of*. A timeline the
//! caller built has no aggregate and no document, and it is edited by the same
//! gesture in the same view.
//!
//! The vocabulary is one verb, *the events are now these*, for the reason
//! [`points`](crate::points) has one: a roll's edit is already whole-list —
//! notes added, moved and removed arrive as the list that resulted — so the
//! edit stating the previous list is its inverse and nothing else is needed.
//!
//! # What this decides about events: nothing
//!
//! An event is a position and a payload the crate never reads. No pitch, no
//! duration, no channel, no unit for `at` — beats for a roll on the musical
//! grid, seconds for a lane of markers, and the crate does not ask which, the
//! same way it does not ask what a leaf's configuration means. What would be
//! decided by giving an event fields is exactly the question
//! `clients/python/PLAN.md` holds open under "`Track` wraps a `Timeline`, so the
//! tree has two ways of placing things", and it is not this domain's to answer.
//!
//! # Why the identity is the position and not an id
//!
//! An event carries none. The arrangement's members do, because a placement has
//! to survive its siblings moving; a whole-list edit does not need one, and
//! minting ids for notes here would decide the model question above from
//! underneath. When an event does gain an identity it will be because the
//! arrangement gave it one, and this domain will carry it in the payload it
//! already does not read.

use serde::{Deserialize, Serialize};

use crate::Opaque;
use crate::history::{Applied, Editable};

/// The domain name a timeline of events is registered under.
pub const EVENTS: &str = "events";

/// One event: where it sits, and what it says there.
///
/// `at` is in whatever the timeline is drawn against, and `data` is the event
/// itself — carried and never interpreted, like every other [`Opaque`] here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Where the event sits.
    pub at: f64,
    /// What it says there.
    #[serde(default, skip_serializing_if = "Opaque::is_empty")]
    pub data: Opaque,
}

/// A timeline: its events, in order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Events(pub Vec<Event>);

/// An edit to a timeline. One verb, stating the result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "lowercase")]
pub enum EventsIntent {
    /// What the timeline holds now, whole.
    SetEvents {
        /// The events, in order.
        events: Vec<Event>,
    },
}

impl Events {
    /// A timeline over these events.
    pub fn new(events: Vec<Event>) -> Self {
        Self(events)
    }

    /// The edit stating what this timeline holds now — its own inverse, read
    /// before another one lands.
    pub fn state(&self) -> EventsIntent {
        EventsIntent::SetEvents {
            events: self.0.clone(),
        }
    }
}

/// A timeline's edit as a history carries it.
pub fn payload(intent: &EventsIntent) -> Opaque {
    Opaque(serde_json::to_value(intent).unwrap_or(serde_json::Value::Null))
}

impl Editable for Events {
    fn apply(&mut self, payload: &Opaque) -> Applied {
        let Ok(EventsIntent::SetEvents { events }) =
            serde_json::from_value::<EventsIntent>(payload.0.clone())
        else {
            return Applied::refused(
                crate::events::payload(&self.state()),
                "not an edit written in this timeline's vocabulary",
            );
        };
        if events == self.0 {
            // A resend is not an edit, so it does not become an undo step.
            return Applied {
                effective: crate::events::payload(&self.state()),
                applied: false,
                reason: None,
                stale: false,
            };
        }
        self.0 = events;
        Applied {
            effective: crate::events::payload(&self.state()),
            applied: true,
            reason: None,
            stale: false,
        }
    }

    fn current(&self, _payload: &Opaque) -> Option<Opaque> {
        Some(crate::events::payload(&self.state()))
    }

    fn coalesce_key(&self, _payload: &Opaque) -> Option<String> {
        // One verb over one timeline, like a curve's: every edit here is the
        // same thing done the same way, so a note dragged across the grid is one
        // undo when the caller says the hand did not stop. The span that makes a
        // samples key more than the domain name has no counterpart here -- a
        // whole-list edit names no span.
        Some(EVENTS.to_string())
    }
}

#[cfg(test)]
mod tests;
