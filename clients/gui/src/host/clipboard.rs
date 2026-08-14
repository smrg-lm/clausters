//! The host-wide clipboard: one typed document, plus the bulk it names.
//!
//! It used to be a `String`, which is exactly as much as a notes block or a
//! field's contents needs and one kind short of what an editor needs. A range
//! of audio cannot travel that way: keeping it a string means base64 inside
//! JSON, which is the re-encode the project's bulk rule exists to forbid, at
//! 4/3 the memory, on the one payload that is large by definition.
//!
//! So the clipboard is [`clausters_document::clipboard::Clipboard`] — the
//! crate's type, not a second definition of it, because the whole point of that
//! format is that one clipboard crosses a window, a def and a process
//! unchanged. The **structure names blobs and does not hold them**, so this
//! holds the payloads beside it, in the order the content indexes.
//!
//! # The text case is not a special case
//!
//! A field still cuts and pastes a string, and it does it through
//! [`Clip::text`]/[`Clip::set_text`] — `Content::Text` is one of the kinds, and
//! `Clipboard::parse` reads anything that is not a clipboard document as text.
//! That is what lets the browser front keep swapping the *page's* clipboard
//! string in and out around a key: what crosses that boundary is a string
//! either way, and it is a clipboard document when the host wrote one.

use std::sync::Arc;

use clausters_document::clipboard::{Clipboard, Content, decode_samples, encode_samples};

/// The clipboard and the payloads it names.
#[derive(Debug, Clone, Default)]
pub struct Clip {
    /// What was copied, or `None` for a clipboard nothing has been put on.
    doc: Option<Clipboard>,
    /// The bulk payloads, in the order the content's `blob` indices name them.
    /// Held as samples rather than as bytes because the host reads them to draw
    /// and writes them to send, and the bytes are the wire's business.
    blobs: Vec<Arc<[f32]>>,
}

impl Clip {
    /// Whether there is anything to paste.
    pub fn is_empty(&self) -> bool {
        self.doc.as_ref().is_none_or(Clipboard::is_empty)
    }

    /// What is on it, if anything.
    pub fn doc(&self) -> Option<&Clipboard> {
        self.doc.as_ref()
    }

    /// The bulk payloads, in index order.
    pub fn blobs(&self) -> &[Arc<[f32]>] {
        &self.blobs
    }

    /// Whether the payloads that arrived match the header — the check that
    /// tells a **truncated** paste from an empty one, since pasting silence
    /// would be worse than declining.
    pub fn is_whole(&self) -> bool {
        let Some(doc) = &self.doc else {
            return false;
        };
        if self.blobs.len() < doc.blobs() {
            return false;
        }
        match doc.values() {
            Some(values) => self.blobs.last().is_some_and(|b| b.len() >= values),
            None => true,
        }
    }

    /// Puts a typed document on the clipboard, with the payloads it names.
    pub fn put(&mut self, doc: Clipboard, blobs: Vec<Arc<[f32]>>) {
        self.doc = Some(doc);
        self.blobs = blobs;
    }

    /// Puts a block of interleaved samples on it — the copy an editor makes.
    pub fn put_samples(&mut self, samples: Arc<[f32]>, channels: u32, sample_rate: f64) {
        let frames = samples.len() as u64 / u64::from(channels.max(1));
        self.put(
            Clipboard::samples(channels.max(1), frames, sample_rate, 0),
            vec![samples],
        );
    }

    /// The text on it: the string a field pastes, which is the content itself
    /// when it is text and its **serialization** when it is anything else — so
    /// a structured clipboard read as text is the document rather than nothing,
    /// and a field that pastes it gets something it can see.
    pub fn text(&self) -> String {
        match self.doc.as_ref() {
            Some(Clipboard {
                content: Content::Text { text },
                ..
            }) => text.clone(),
            Some(doc) => doc.to_json(),
            None => String::new(),
        }
    }

    /// Puts a string on it, reading a clipboard document if that is what it is
    /// ([`Clipboard::parse`] — a door rather than a guess).
    ///
    /// The bulk is dropped, because a string cannot carry it: a document that
    /// arrives this way with a blob index is a header whose payload is gone,
    /// and [`Clip::is_whole`] is what says so at the paste.
    pub fn set_text(&mut self, raw: &str) {
        self.doc = Some(Clipboard::parse(raw));
        self.blobs.clear();
    }

    /// The blob at `index` as the bytes that carry it — little-endian `f32`,
    /// the one encoding bulk uses everywhere here.
    pub fn blob_bytes(&self, index: usize) -> Option<Vec<u8>> {
        self.blobs.get(index).map(|b| encode_samples(b))
    }

    /// Takes a payload that arrived as bytes.
    pub fn push_blob_bytes(&mut self, bytes: &[u8]) {
        self.blobs.push(decode_samples(bytes).into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The text case still works, and it works through the kinds.** A string
    /// put on the clipboard comes back as the same string; a clipboard document
    /// put on it as a string comes back as the document, because `parse` is a
    /// door and not a guess.
    #[test]
    fn a_string_round_trips_and_a_document_is_recognized() {
        let mut clip = Clip::default();
        assert!(clip.is_empty());
        clip.set_text("a, b");
        assert_eq!(clip.text(), "a, b");
        assert_eq!(clip.doc().map(Clipboard::kind), Some("text"));

        let block = Clipboard::samples(2, 4, 48_000.0, 0);
        clip.set_text(&block.to_json());
        assert_eq!(clip.doc().map(Clipboard::kind), Some("samples"));
        // ...and its payload did not travel with the string, which is exactly
        // what a paste has to notice.
        assert!(!clip.is_whole(), "a header with no payload is not whole");
    }

    /// A copied block is whole, and a truncated one says so rather than
    /// pasting silence.
    #[test]
    fn a_block_knows_whether_its_payload_arrived() {
        let mut clip = Clip::default();
        let samples: Arc<[f32]> = vec![0.5f32; 8].into();
        clip.put_samples(samples.clone(), 2, 48_000.0);
        assert!(clip.is_whole());
        assert!(!clip.is_empty());
        assert_eq!(clip.blobs().len(), 1);
        // The header says four frames of two channels; the payload holds them.
        assert_eq!(clip.doc().unwrap().values(), Some(8));

        // The same header with a short payload is a truncated paste.
        let mut short = Clip::default();
        short.put(
            Clipboard::samples(2, 4, 48_000.0, 0),
            vec![vec![0.5; 3].into()],
        );
        assert!(!short.is_whole());
    }

    /// The bytes are the crate's little-endian `f32`, both ways.
    #[test]
    fn a_payload_crosses_as_little_endian_f32() {
        let mut clip = Clip::default();
        clip.put_samples(vec![1.0f32, -0.5, 0.25].into(), 1, 44_100.0);
        let bytes = clip.blob_bytes(0).expect("the payload is there");
        assert_eq!(bytes.len(), 12);
        let mut back = Clip::default();
        back.put(Clipboard::samples(1, 3, 44_100.0, 0), vec![]);
        back.push_blob_bytes(&bytes);
        assert!(back.is_whole());
        assert_eq!(&back.blobs()[0][..], &[1.0, -0.5, 0.25]);
    }
}
