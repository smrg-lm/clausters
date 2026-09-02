//! A span of samples, and the inverse the arrangement never had.
//!
//! This is the crate's **third** editable domain, and the one whose absence was
//! visible from outside: [`Intent::WriteSamples`](crate::Intent::WriteSamples)
//! names a node, so writing samples has until now required a
//! [`Document`](crate::Document) to hold one — and even then it does not write
//! them. What that intent applies is a **generation bump**, the signal every
//! reader of those samples needs in order to know its copy is stale; the values
//! themselves are the owner's to write, and
//! [`inverse_of`](crate::log::inverse_of) answers the empty write because the
//! samples are not in the document.
//!
//! So the destructive edit had no inverse anywhere, which is why a host holding
//! the data could draw over a take and a client without it could not. Here the
//! span *is* the structure, and the inverse is what it held read before the edit
//! lands — the same rule [`points`](crate::points) states, over data instead of
//! parameters.
//!
//! # The structure is borrowed, never held
//!
//! [`Samples`] is a view over whoever owns the memory: a client's buffer, a
//! host's mapped file, a test's vector. The crate copies nothing and outlives
//! nothing — a domain that held a second copy of a take would be the largest
//! thing in the process and the one most certainly stale. What it does own is
//! the arithmetic: which frames a channel's span touches, and what was there
//! before.
//!
//! The samples are **interleaved**, so a span is **strided**: one channel's run
//! of frames, `channels` apart. That is how a client holds a stereo take and how
//! the wire already describes an edit to one — a channel is a coordinate of the
//! span, not a fact about the source.
//!
//! # What this decides about samples: nothing
//!
//! No rate, no unit, no reading of what the numbers mean. A frame index is a
//! frame index. The one thing a domain owes the seam is how an edit inverts, and
//! *this span held those values* inverts without knowing any of the rest.

use serde::{Deserialize, Serialize};

use crate::Opaque;
use crate::history::{Applied, Editable};

/// The domain name a span of samples is registered under.
pub const SAMPLES: &str = "samples";

/// An edit to samples. One verb, stating the result — the absolute rule the
/// rest of the crate's vocabularies follow, and for the same reason: an edit
/// that states values is idempotent, and its inverse is the edit stating the
/// values that were there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "lowercase")]
pub enum SamplesIntent {
    /// What a span of one channel holds now.
    Write {
        /// Which channel of the interleaved samples the span belongs to.
        ///
        /// Defaults to 0 when absent, which is what every mono edit means — the
        /// same default [`Intent::WriteSamples`](crate::Intent::WriteSamples)
        /// takes, so the two spell one thing.
        #[serde(default)]
        channel: u32,
        /// First frame of the span, in that channel.
        start: u64,
        /// The values the span now holds.
        values: Vec<f32>,
    },
}

/// A span of samples as a history carries it.
pub fn payload(intent: &SamplesIntent) -> Opaque {
    Opaque(serde_json::to_value(intent).unwrap_or(serde_json::Value::Null))
}

/// Interleaved samples, borrowed from whoever owns them.
///
/// Built per call, for the length of one edit: see the module note on why the
/// crate holds no samples of its own.
#[derive(Debug)]
pub struct Samples<'a> {
    channels: u32,
    data: &'a mut [f32],
}

impl<'a> Samples<'a> {
    /// A view over `data`, read as `channels` interleaved channels.
    ///
    /// `channels` of zero is read as one: a caller that does not count channels
    /// has mono samples, and refusing every edit it makes would be a worse
    /// answer than the one it meant.
    pub fn interleaved(data: &'a mut [f32], channels: u32) -> Self {
        Self {
            channels: channels.max(1),
            data,
        }
    }

    /// How many frames these samples hold, per channel.
    pub fn frames(&self) -> u64 {
        self.data.len() as u64 / u64::from(self.channels)
    }

    /// Where in `data` frame `frame` of `channel` sits, or `None` when it is
    /// outside them. The one piece of arithmetic the domain owns.
    fn index(&self, channel: u32, frame: u64) -> Option<usize> {
        if channel >= self.channels {
            return None;
        }
        let at = frame
            .checked_mul(u64::from(self.channels))?
            .checked_add(u64::from(channel))?;
        (at < self.data.len() as u64).then_some(at as usize)
    }

    /// The values a span holds now, or `None` when it is not inside these
    /// samples.
    pub fn read(&self, channel: u32, start: u64, len: usize) -> Option<Vec<f32>> {
        let mut out = Vec::with_capacity(len);
        for n in 0..len {
            out.push(self.data[self.index(channel, start + n as u64)?]);
        }
        Some(out)
    }

    /// Writes `values` into a span the caller has already checked.
    fn write(&mut self, channel: u32, start: u64, values: &[f32]) {
        for (n, value) in values.iter().enumerate() {
            if let Some(at) = self.index(channel, start + n as u64) {
                self.data[at] = *value;
            }
        }
    }
}

impl Editable for Samples<'_> {
    fn apply(&mut self, payload: &Opaque) -> Applied {
        let Ok(SamplesIntent::Write {
            channel,
            start,
            values,
        }) = serde_json::from_value::<SamplesIntent>(payload.0.clone())
        else {
            return Applied::refused(
                empty(0, 0),
                "not an edit written in this samples vocabulary",
            );
        };
        if values.is_empty() {
            // Nothing said is not an edit, and it is not a refusal either --
            // the same answer `WriteSamples` gives an empty write.
            return unchanged(channel, start);
        }
        let Some(previous) = self.read(channel, start, values.len()) else {
            return Applied::refused(
                empty(channel, start),
                "that span is not inside these samples",
            );
        };
        if previous == values {
            // A resend is not an edit, so it does not become an undo step --
            // `points`' rule, and it earns more here: a stroke crossing a run
            // that already holds those values would otherwise record an entry
            // that changes nothing.
            return unchanged(channel, start);
        }
        self.write(channel, start, &values);
        Applied {
            effective: payload_of(channel, start, values),
            applied: true,
            reason: None,
            stale: false,
        }
    }

    fn current(&self, payload: &Opaque) -> Option<Opaque> {
        let SamplesIntent::Write {
            channel,
            start,
            values,
        } = serde_json::from_value::<SamplesIntent>(payload.0.clone()).ok()?;
        // The inverse of an empty write is an empty write: it is the one edit
        // that describes itself in both directions.
        if values.is_empty() {
            return Some(empty(channel, start));
        }
        let previous = self.read(channel, start, values.len())?;
        Some(payload_of(channel, start, previous))
    }

    fn coalesce_key(&self, payload: &Opaque) -> Option<String> {
        let SamplesIntent::Write {
            channel,
            start,
            values,
        } = serde_json::from_value::<SamplesIntent>(payload.0.clone()).ok()?;
        // **The span is part of the sentence**, unlike a curve's, where one verb
        // over one structure says everything. A sample dragged up and then
        // further up is the same thing done the same way and is one undo; two
        // strokes over different runs are two, and merging them would make an
        // undo take back a stroke the hand had already finished.
        Some(format!("{SAMPLES}:{channel}:{start}:{}", values.len()))
    }
}

/// The payload for a span stating these values.
fn payload_of(channel: u32, start: u64, values: Vec<f32>) -> Opaque {
    payload(&SamplesIntent::Write {
        channel,
        start,
        values,
    })
}

/// The empty write at a position — what an unloggable or refused edit answers
/// with, since it is the one payload that states no values.
fn empty(channel: u32, start: u64) -> Opaque {
    payload_of(channel, start, Vec::new())
}

/// An edit that changed nothing, answering with the empty write at its own
/// position.
fn unchanged(channel: u32, start: u64) -> Applied {
    Applied {
        effective: empty(channel, start),
        applied: false,
        reason: None,
        stale: false,
    }
}

#[cfg(test)]
mod tests;
