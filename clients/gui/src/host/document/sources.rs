//! The session's source table, resolved to **server buffers**.
//!
//! A document says what plays when and deliberately never says where its
//! samples are; a session's `sources` table is that other half, and this is
//! where a host turns it into something it can draw and edit. The answer is a
//! server buffer per source, loaded with `/buffer_allocRead`. **The load itself
//! is the shared crate's** ([`clausters_editing::load`]), which both clients
//! walk too; what is this host's is where the numbers come from and whether a
//! file is there.
//!
//! # Why a buffer and not a mapped file
//!
//! The host can already map a file of raw `f32` and draw it with no server at
//! all (the `path` bulk route), which is cheaper and needs nothing running. It
//! is the wrong route here for two reasons, and neither is about drawing.
//!
//! A session's samples are **audio the user brought** — a WAV, a FLAC, an
//! MP3 — and decoding those is the server's job and nobody else's here (it
//! decodes by content, through hound and symphonia). Mapping one as raw floats
//! draws the header as a click.
//!
//! And what is drawn has to be what is **edited**: a destructive edit is
//! `/buffer_gain`, `/buffer_setRange`, `/buffer_reverse` — commands addressed
//! to a buffer, run on the server's own NRT thread. A host drawing a mapped
//! file and editing a buffer would be showing one copy and writing another,
//! which is the two-owner problem again with the samples in place of the tree.
//!
//! So the picture and the samples are one thing: a source becomes a buffer,
//! the clip draws that buffer, and an edit writes the very samples on screen.

use std::path::Path;

use clausters_core::ids::{IdSpaces, Space};
use clausters_document::session::Session;

pub use clausters_editing::load::{Load, Take, Takes, held_takes, stitch_message};

/// Plans the load of every source the document actually names.
///
/// `beside` is the session file's own folder, which is what a relative path is
/// resolved against.
///
/// The buffers come from `ids`, the host's one buffer space, so what goes on
/// allocating after the load -- a curve's table, a join made by a hand -- never
/// writes over a take. And a file is looked for here, because this host's
/// server is in its own process and reads the same disk.
pub fn plan(session: &Session, beside: &Path, ids: &mut IdSpaces) -> Load {
    clausters_editing::load::plan(session, beside, &|path| path.exists(), &mut || {
        ids.alloc(Space::Buffers, 1)
            .map(|bufnum| bufnum as i32)
            .map_err(|e| e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::session::Source;
    use clausters_document::{
        Body, Document, Lifetime, Member, Node, NodeId, Opaque, SourceId, SourceRef,
    };
    use std::path::PathBuf;

    fn take_node(id: u64, source: u64) -> Node {
        Node::new(
            NodeId(id),
            Body::Vector {
                source: SourceRef {
                    source: SourceId(source),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                },
                config: Default::default(),
            },
        )
    }

    fn aggregate(id: u64, members: Vec<Member>) -> Node {
        Node::new(
            NodeId(id),
            Body::Aggregate {
                grouping: clausters_document::Grouping::Concrete,
                members,
                config: Opaque::none(),
            },
        )
    }

    fn at(offset: f64, node: Node) -> Member {
        Member {
            offset,
            dur: None,
            node,
        }
    }

    fn spaces() -> IdSpaces {
        IdSpaces::new(
            clausters_core::ids::ServerShape::DEFAULT,
            clausters_core::ids::IdShare::WHOLE,
        )
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "clausters_gui_sources_{tag}_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// Buffers come from the host's one space, past what it already holds, in
    /// source order, so the same session loads the same way twice.
    #[test]
    fn buffers_are_allocated_from_the_host_s_space_in_order() {
        let dir = tmp("order");
        std::fs::write(dir.join("a.wav"), b"a").expect("write");
        std::fs::write(dir.join("b.wav"), b"b").expect("write");
        let session = Session::new(Document::new(aggregate(
            1,
            // Named out of order in the tree, on purpose.
            vec![at(0.0, take_node(2, 9)), at(1.0, take_node(3, 4))],
        )))
        .with_source(SourceId(9), Source::file("a.wav", Lifetime::Session))
        .with_source(SourceId(4), Source::file("b.wav", Lifetime::Session));
        let mut ids = spaces();
        let held = ids.alloc(Space::Buffers, 100).unwrap();
        let load = plan(&session, &dir, &mut ids);
        assert_eq!(load.takes.get(SourceId(4)).map(|t| t.bufnum), Some(100));
        assert_eq!(load.takes.get(SourceId(9)).map(|t| t.bufnum), Some(101));
        assert_eq!(held, 0, "past what the host already held");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The host looks for the file**, where a client's door cannot: its
    /// server reads the same disk.
    #[test]
    fn a_file_that_is_not_there_is_named_before_anything_is_sent() {
        let dir = tmp("gone");
        let session = Session::new(Document::new(aggregate(1, vec![at(0.0, take_node(2, 1))])))
            .with_source(SourceId(1), Source::file("gone.wav", Lifetime::Session));
        let load = plan(&session, &dir, &mut spaces());
        assert!(load.messages.is_empty());
        assert!(load.unresolved[0].1.contains("is not there"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
