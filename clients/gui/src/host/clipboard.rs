//! The host-wide clipboard: one typed document, plus the bulk it names.
//!
//! It used to be a `String`, which is exactly as much as a notes block or a
//! field's contents needs and one kind short of what an editor needs. A range
//! of audio cannot travel that way: keeping it a string means base64 inside
//! JSON, which is the re-encode the project's bulk rule exists to forbid, at
//! 4/3 the memory, on the one payload that is large by definition.
//!
//! So the clipboard is [`clausters_document::clipboard::Clipboard`] -- the
//! crate's type, not a second definition of it, because the whole point of that
//! format is that one clipboard crosses a window, a def and a process
//! unchanged. The **structure names blobs and does not hold them**, so this
//! holds the payloads beside it, in the order the content indexes.
//!
//! # The text case is not a special case
//!
//! A field still cuts and pastes a string, and it does it through
//! [`Clip::text`]/[`Clip::set_text`] -- `Content::Text` is one of the kinds, and
//! `Clipboard::parse` reads anything that is not a clipboard document as text.
//! That is what lets the browser front keep swapping the *page's* clipboard
//! string in and out around a key: what crosses that boundary is a string
//! either way, and it is a clipboard document when the host wrote one.

//!
//! # The system's clipboard
//!
//! **Text crosses, the rest stays.** A front that can reach the platform's
//! clipboard hands the host a [`SystemClipboard`] ([`Clip::attach`]): every
//! string put here is written there too, and a paste first [`Clip::refresh`]es
//! -- text another program put there after the host's last write is what is
//! pasted, and otherwise the host's own, so a range of samples, which no other
//! program reads, still pastes. The read happens at a paste and never per key,
//! because asking the platform is a round trip to whoever owns the clipboard.
//!
//! The **primary selection** -- what a middle click pastes on a desktop that
//! has one -- rides the same trait, and the host keeps its own beside it, so a
//! front with none (a page) still pastes the last selection made inside it.

use std::sync::{Arc, Mutex};

use clausters_document::clipboard::{Clipboard, Content, decode_samples, encode_samples};

/// **The platform's clipboard**, as a front reaches it: text in and out, and the
/// primary selection where the platform has one. Every method may fail
/// quietly -- a clipboard nobody owns, a platform that refuses -- and the host
/// then keeps working with its own.
pub trait SystemClipboard: Send {
    /// The text on the clipboard, if there is any.
    fn read_text(&mut self) -> Option<String>;
    /// Puts `text` on the clipboard.
    fn write_text(&mut self, text: &str);
    /// The primary selection, on a platform that has one.
    fn read_primary(&mut self) -> Option<String> {
        None
    }
    /// Makes `text` the primary selection, on a platform that has one.
    fn write_primary(&mut self, _text: &str) {}
}

/// The system clipboard a [`Clip`] is attached to, shared by its clones.
#[derive(Clone)]
struct System(Arc<Mutex<Box<dyn SystemClipboard>>>);

impl std::fmt::Debug for System {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.write_str("System")
    }
}

impl System {
    fn with<R>(&self, f: impl FnOnce(&mut dyn SystemClipboard) -> R) -> Option<R> {
        self.0.lock().ok().map(|mut sys| f(sys.as_mut()))
    }
}

/// The clipboard and the payloads it names.
#[derive(Debug, Clone, Default)]
pub struct Clip {
    /// What was copied, or `None` for a clipboard nothing has been put on.
    doc: Option<Clipboard>,
    /// The bulk payloads, in the order the content's `blob` indices name them.
    /// Held as samples rather than as bytes because the host reads them to draw
    /// and writes them to send, and the bytes are the wire's business.
    blobs: Vec<Arc<[f32]>>,
    /// The platform's clipboard, when the front reaches one.
    system: Option<System>,
    /// The last text the host wrote to it or took from it: what tells a copy
    /// another program made since from the host's own.
    mirrored: Option<String>,
    /// The host's own primary selection: the last text selected in it.
    primary: Option<String>,
}

impl Clip {
    /// Reaches the platform's clipboard through `system` from now on.
    pub fn attach(&mut self, system: Box<dyn SystemClipboard>) {
        self.system = Some(System(Arc::new(Mutex::new(system))));
    }

    /// **Before a paste**: takes the text on the platform's clipboard when
    /// another program put it there since the host last wrote or read it.
    /// What the host put there itself, or nothing new, leaves the host's own
    /// -- a block of samples included.
    pub fn refresh(&mut self) {
        let Some(text) = self
            .system
            .as_ref()
            .and_then(|s| s.with(|s| s.read_text()))
            .flatten()
        else {
            return;
        };
        if self.mirrored.as_deref() != Some(text.as_str()) {
            self.doc = Some(Clipboard::parse(&text));
            self.blobs.clear();
            self.mirrored = Some(text);
        }
    }

    /// Makes `text` the selection a middle click pastes: the host's own, and
    /// the platform's primary selection where there is one. Nothing is written
    /// when it did not change, since a field reports its selection after every
    /// gesture.
    pub fn set_primary(&mut self, text: &str) {
        if text.is_empty() || self.primary.as_deref() == Some(text) {
            return;
        }
        self.primary = Some(text.to_string());
        if let Some(sys) = &self.system {
            sys.with(|s| s.write_primary(text));
        }
    }

    /// What a middle click pastes: the platform's primary selection where it
    /// has one -- which another program may have made -- and the last text
    /// selected in the host otherwise.
    pub fn primary(&mut self) -> Option<String> {
        self.system
            .as_ref()
            .and_then(|s| s.with(|s| s.read_primary()))
            .flatten()
            .or_else(|| self.primary.clone())
    }

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

    /// Whether the payloads that arrived match the header -- the check that
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

    /// Puts a block of interleaved samples on it -- the copy an editor makes.
    pub fn put_samples(&mut self, samples: Arc<[f32]>, channels: u32, sample_rate: f64) {
        let frames = samples.len() as u64 / u64::from(channels.max(1));
        self.put(
            Clipboard::samples(channels.max(1), frames, sample_rate, 0),
            vec![samples],
        );
    }

    /// The text on it: the string a field pastes, which is the content itself
    /// when it is text and its **serialization** when it is anything else -- so
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
    /// ([`Clipboard::parse`] -- a door rather than a guess).
    ///
    /// The bulk is dropped, because a string cannot carry it: a document that
    /// arrives this way with a blob index is a header whose payload is gone,
    /// and [`Clip::is_whole`] is what says so at the paste.
    pub fn set_text(&mut self, raw: &str) {
        self.doc = Some(Clipboard::parse(raw));
        self.blobs.clear();
        // Text crosses: the platform's clipboard holds it too.
        if let Some(sys) = &self.system {
            sys.with(|s| s.write_text(raw));
            self.mirrored = Some(raw.to_string());
        }
    }

    /// The blob at `index` as the bytes that carry it -- little-endian `f32`,
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

    /// A stand-in for a platform's clipboard, shared with the test.
    #[derive(Default, Clone)]
    struct Fake(Arc<Mutex<(Option<String>, Option<String>)>>);

    impl SystemClipboard for Fake {
        fn read_text(&mut self) -> Option<String> {
            self.0.lock().unwrap().0.clone()
        }
        fn write_text(&mut self, text: &str) {
            self.0.lock().unwrap().0 = Some(text.to_string());
        }
        fn read_primary(&mut self) -> Option<String> {
            self.0.lock().unwrap().1.clone()
        }
        fn write_primary(&mut self, text: &str) {
            self.0.lock().unwrap().1 = Some(text.to_string());
        }
    }

    /// **Text crosses and the rest stays**: a string copied in the host is on
    /// the platform's clipboard, one another program copied since is what a
    /// paste takes, and a block of samples copied after it is not replaced by
    /// a refresh that finds nothing new.
    #[test]
    fn text_crosses_to_the_system_and_samples_stay() {
        let fake = Fake::default();
        let mut clip = Clip::default();
        clip.attach(Box::new(fake.clone()));
        clip.set_text("from the host");
        assert_eq!(fake.0.lock().unwrap().0.as_deref(), Some("from the host"));
        // Another program copies.
        fake.0.lock().unwrap().0 = Some("from elsewhere".into());
        clip.refresh();
        assert_eq!(clip.text(), "from elsewhere");
        // A block of samples copied in the host survives the next refresh.
        clip.put_samples(vec![0.5f32; 4].into(), 1, 48_000.0);
        clip.refresh();
        assert_eq!(clip.doc().map(Clipboard::kind), Some("samples"));
        // The primary selection: the platform's when it has one.
        clip.set_primary("selected");
        assert_eq!(fake.0.lock().unwrap().1.as_deref(), Some("selected"));
        assert_eq!(clip.primary().as_deref(), Some("selected"));
    }

    /// With no platform clipboard the primary selection is the host's own.
    #[test]
    fn the_primary_selection_is_the_hosts_own_without_a_platform() {
        let mut clip = Clip::default();
        assert_eq!(clip.primary(), None);
        clip.set_primary("word");
        assert_eq!(clip.primary().as_deref(), Some("word"));
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
