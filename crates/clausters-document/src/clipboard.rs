//! What was copied: one mechanism, whatever the payload.
//!
//! A clipboard that is a `String` can carry a notes block and nothing else —
//! and the moment it has to carry samples, the only way to keep it a string is
//! base64 inside JSON, which is the re-encode the project's bulk rule exists to
//! forbid: 4/3 the memory, on the one payload that is large by definition. So
//! the clipboard becomes a **typed document with a kind**, and the split it
//! draws is the one `/buffer_set` and `/buffer_setRange` already draw: a payload
//! whose size follows the *parameters* stays structure, and a payload whose size
//! follows the *audio* rides beside it as little-endian `f32`.
//!
//! # The structure names blobs; it does not hold them
//!
//! [`Clipboard`] serializes whole as JSON, and a bulk payload appears in it as
//! an **index** into the blobs travelling alongside — the same convention a
//! GuiDef's `"blob": <index>` prop already uses. That is what lets one
//! clipboard cross a wire, a window boundary and a process without anything
//! being re-encoded on the way. [`Clipboard::blobs`] says how many must
//! accompany it, so a receiver can tell a truncated paste from an empty one.
//!
//! # A block carries its sample rate and is never resampled here
//!
//! Resampling is an **edit**, and an edit is something an owner performs and
//! logs. A paste that quietly resampled would change data nobody asked it to
//! change, in a step nothing records. So the rate travels with the block, the
//! crate never touches it, and [`Clipboard::sample_rate`] is what a paste reads
//! before deciding whether to convert, refuse or ask.
//!
//! # A string is still one of the kinds
//!
//! The notes block that travels as a string today keeps working:
//! [`Clipboard::parse`] takes what is on the clipboard and gives back a typed
//! one either way, reading anything that is not a clipboard document as
//! [`Content::Text`]. The compatibility is an explicit door rather than an
//! untagged guess, because a guess would read a *stored string that happens to
//! look like JSON* as a structure.

use serde::{Deserialize, Serialize};

use crate::{Member, NodeId};

/// What was copied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Content {
    /// Text, which is what the host-wide clipboard was before it had kinds —
    /// a field's contents, and the flat notes block that still travels this
    /// way.
    Text {
        /// The string.
        text: String,
    },
    /// A piece of the tree: notes, clips, whole subtrees. They are **placed
    /// members**, so a copied selection keeps the relative offsets it had and
    /// the recursion is the tree's own rather than a second shape.
    Elements {
        /// What was copied, in the order it was taken.
        members: Vec<Member>,
    },
    /// Data at constant rate: an audio block, a control block.
    Samples {
        /// How many channels are interleaved in the blob.
        channels: u32,
        /// Frames per channel.
        frames: u64,
        /// The rate this was taken at. **Carried, never acted on** — see the
        /// module docs.
        sample_rate: f64,
        /// Which of the accompanying blobs holds it: interleaved
        /// little-endian `f32`.
        blob: usize,
    },
    /// A spectral region: an analysis, not a picture of one.
    Spectral {
        /// Analysis frames in the block.
        frames: u32,
        /// Bins per frame.
        bins: u32,
        /// Values per bin: 1 for magnitudes, 2 for interleaved real and
        /// imaginary. A region that has to be resynthesized keeps its phase,
        /// and one that only has to be measured need not.
        values_per_bin: u32,
        /// The analysis hop, in frames of the source.
        hop: u32,
        /// The analysis window length, in frames of the source.
        window: u32,
        /// The rate the source was analyzed at. Carried, never acted on.
        sample_rate: f64,
        /// Which blob holds it: frame-major little-endian `f32`.
        blob: usize,
    },
}

impl Content {
    /// The kind's name on the wire — what a reader dispatches on, and what a
    /// reader that does not understand this kind reports.
    pub fn kind(&self) -> &'static str {
        match self {
            Content::Text { .. } => "text",
            Content::Elements { .. } => "elements",
            Content::Samples { .. } => "samples",
            Content::Spectral { .. } => "spectral",
        }
    }
}

/// The clipboard: one typed document, plus where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clipboard {
    /// What was copied.
    pub content: Content,
    /// The element it was taken from, when that is known. A paste reads it to
    /// decide whether it is pasting into the same kind of place; nothing here
    /// requires it, because a clipboard outlives the document it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<NodeId>,
}

impl Clipboard {
    /// Text — a field's contents, or the flat notes block.
    pub fn text(text: impl Into<String>) -> Self {
        Self::of(Content::Text { text: text.into() })
    }

    /// A piece of the tree.
    pub fn elements(members: impl IntoIterator<Item = Member>) -> Self {
        Self::of(Content::Elements {
            members: members.into_iter().collect(),
        })
    }

    /// An audio or control block. `blob` names which accompanying payload
    /// holds the interleaved samples.
    pub fn samples(channels: u32, frames: u64, sample_rate: f64, blob: usize) -> Self {
        Self::of(Content::Samples {
            channels,
            frames,
            sample_rate,
            blob,
        })
    }

    /// A spectral region. See [`Content::Spectral`] for what each number means.
    pub fn spectral(
        frames: u32,
        bins: u32,
        values_per_bin: u32,
        hop: u32,
        window: u32,
        sample_rate: f64,
        blob: usize,
    ) -> Self {
        Self::of(Content::Spectral {
            frames,
            bins,
            values_per_bin,
            hop,
            window,
            sample_rate,
            blob,
        })
    }

    fn of(content: Content) -> Self {
        Self {
            content,
            origin: None,
        }
    }

    /// Records which element this was taken from.
    pub fn from_node(mut self, node: NodeId) -> Self {
        self.origin = Some(node);
        self
    }

    /// The kind's name — see [`Content::kind`].
    pub fn kind(&self) -> &'static str {
        self.content.kind()
    }

    /// How many bulk payloads must travel alongside this clipboard.
    ///
    /// What a receiver checks: a sample block that arrived with no blob is a
    /// **truncated** paste, and pasting silence would be worse than declining.
    pub fn blobs(&self) -> usize {
        match &self.content {
            Content::Samples { blob, .. } | Content::Spectral { blob, .. } => blob + 1,
            Content::Text { .. } | Content::Elements { .. } => 0,
        }
    }

    /// The rate this block was taken at, for the kinds that have one.
    ///
    /// What a paste reads before deciding. The crate offers no conversion at
    /// all, deliberately: resampling is an edit, and an edit is something an
    /// owner performs and logs.
    pub fn sample_rate(&self) -> Option<f64> {
        match &self.content {
            Content::Samples { sample_rate, .. } | Content::Spectral { sample_rate, .. } => {
                Some(*sample_rate)
            }
            Content::Text { .. } | Content::Elements { .. } => None,
        }
    }

    /// How many `f32` values the accompanying blob should hold, for the kinds
    /// that have one — what validates a payload against its header.
    pub fn values(&self) -> Option<usize> {
        match &self.content {
            Content::Samples {
                channels, frames, ..
            } => Some(*channels as usize * *frames as usize),
            Content::Spectral {
                frames,
                bins,
                values_per_bin,
                ..
            } => Some(*frames as usize * *bins as usize * *values_per_bin as usize),
            Content::Text { .. } | Content::Elements { .. } => None,
        }
    }

    /// Whether there is nothing to paste.
    pub fn is_empty(&self) -> bool {
        match &self.content {
            Content::Text { text } => text.is_empty(),
            Content::Elements { members } => members.is_empty(),
            Content::Samples {
                channels, frames, ..
            } => *channels == 0 || *frames == 0,
            Content::Spectral { frames, bins, .. } => *frames == 0 || *bins == 0,
        }
    }

    /// Reads whatever is on the clipboard.
    ///
    /// A clipboard document if it is one, and [`Content::Text`] otherwise —
    /// which is what keeps everything that travelled as a string working. The
    /// fallback is an explicit door rather than an untagged guess: a guess
    /// would read a *stored string that happens to be JSON* as a structure, and
    /// silently paste a document where the person copied a line of text.
    pub fn parse(raw: &str) -> Self {
        serde_json::from_str::<Clipboard>(raw).unwrap_or_else(|_| Self::text(raw))
    }

    /// The clipboard as the string that carries it.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Samples as the bytes that carry them: little-endian `f32`, the one encoding
/// bulk uses everywhere in this project.
///
/// Here rather than in each client for the ordinary reason: three languages
/// writing the same byte order three times is three places for it to be wrong,
/// and the one that is wrong sounds like noise rather than failing.
pub fn encode_samples(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// The inverse of [`encode_samples`]. Trailing bytes that do not make a whole
/// `f32` are dropped rather than guessed at.
pub fn decode_samples(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

#[cfg(test)]
mod tests;
