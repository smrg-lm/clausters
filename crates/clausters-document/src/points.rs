//! A break-point curve, and the smallest vocabulary a domain can have.
//!
//! This is the crate's **second** editable domain, and it is here for the
//! reason a trait with one implementor is designed wrong: it is what proves
//! that [`history`](crate::history) carries no arrangement in it. Its whole
//! vocabulary is one verb — *the points are now these* — and it names no node,
//! because it has none.
//!
//! It is also the smallest instance of the shape the history exists to serve
//! beyond the arrangement: a structure the client built, edited in a view and
//! read back, with no [`Document`](crate::Document) behind it. A curve drawn in
//! a window is registered in a history like anything else, and an application
//! showing a roll and a curve together undoes across both in one order.
//!
//! # What this decides about curves: nothing
//!
//! A point is a position and a value. There is no interpolation shape here, no
//! unit, no envelope semantics — those belong to whoever renders the curve, and
//! putting a guess at them in the crate would be deciding, on a seam's behalf,
//! a question the client has not asked yet. The one thing a domain must supply
//! is how an edit inverts, and *the points were these* inverts without knowing
//! any of it.

use serde::{Deserialize, Serialize};

use crate::Opaque;
use crate::history::{Applied, Editable};

/// The domain name a curve's structure is registered under.
pub const POINTS: &str = "points";

/// One break point: where it sits, and what it says there.
///
/// `at` is in whatever the curve is drawn against — beats for an automation
/// lane, seconds for an envelope — and the crate does not ask which, for the
/// same reason it does not interpret a leaf's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// Where the point sits.
    pub at: f64,
    /// What the curve says there.
    pub value: f64,
}

/// A break-point curve: its points, in order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Points(pub Vec<Point>);

/// An edit to a curve. One verb, stating the result — the same absolute rule
/// the arrangement's vocabulary follows, and for the same reason: an edit that
/// states a value is idempotent, and its inverse is the edit stating the
/// previous one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "lowercase")]
pub enum PointsIntent {
    /// What the curve holds now, whole.
    SetPoints {
        /// The points, in order.
        points: Vec<Point>,
    },
}

impl Points {
    /// A curve over these points.
    pub fn new(points: Vec<Point>) -> Self {
        Self(points)
    }

    /// The edit stating what this curve holds now — its own inverse, read
    /// before another one lands.
    pub fn state(&self) -> PointsIntent {
        PointsIntent::SetPoints {
            points: self.0.clone(),
        }
    }
}

/// A curve's edit as a history carries it.
pub fn payload(intent: &PointsIntent) -> Opaque {
    Opaque(serde_json::to_value(intent).unwrap_or(serde_json::Value::Null))
}

impl Editable for Points {
    fn apply(&mut self, payload: &Opaque) -> Applied {
        let Ok(PointsIntent::SetPoints { points }) =
            serde_json::from_value::<PointsIntent>(payload.0.clone())
        else {
            return Applied::refused(
                crate::points::payload(&self.state()),
                "not an edit written in this curve's vocabulary",
            );
        };
        if points == self.0 {
            // A resend is not an edit, so it does not become an undo step --
            // the arrangement's rule, and it is the vocabulary's rather than
            // the tree's.
            return Applied {
                effective: crate::points::payload(&self.state()),
                applied: false,
                reason: None,
                stale: false,
            };
        }
        self.0 = points;
        Applied {
            effective: crate::points::payload(&self.state()),
            applied: true,
            reason: None,
            stale: false,
        }
    }

    fn current(&self, _payload: &Opaque) -> Option<Opaque> {
        Some(crate::points::payload(&self.state()))
    }

    fn coalesce_key(&self, payload: &Opaque) -> Option<String> {
        coalesce_key(payload)
    }
}

/// What makes two edits to a curve *the same thing done the same way*.
///
/// One verb over one curve: every edit here is the same thing done the same
/// way, so a stroke's hundred small adjustments are one undo when the caller
/// says the hand did not stop. Free, and reached through
/// [`domain::coalesce_key`](crate::domain::coalesce_key), because a caller
/// across the ABI has to be able to ask without holding the curve.
pub fn coalesce_key(payload: &Opaque) -> Option<String> {
    serde_json::from_value::<PointsIntent>(payload.0.clone()).ok()?;
    Some(POINTS.to_string())
}

#[cfg(test)]
mod tests;
