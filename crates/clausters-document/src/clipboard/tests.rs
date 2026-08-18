//! O7's acceptance: a sample block copied in one window pastes in another, and
//! a block carries its sample rate without being resampled in transit.

use super::*;
use crate::{Beats, Body, Node, NodeId, Opaque};

fn note(id: u64, offset: Beats) -> Member {
    Member {
        offset,
        dur: Some(1.0),
        node: Node::new(
            NodeId(id),
            Body::Clang {
                config: Opaque(serde_json::json!({"midinote": 60 + id})),
                fires: None,
            },
        ),
    }
}

/// Everything that crosses: the JSON, and the blobs beside it.
fn cross(clipboard: &Clipboard, blobs: Vec<Vec<u8>>) -> (Clipboard, Vec<Vec<u8>>) {
    let json = clipboard.to_json();
    (Clipboard::parse(&json), blobs)
}

#[test]
fn a_sample_block_copied_in_one_window_pastes_in_another() {
    // O7's acceptance. Nothing is re-encoded on the way: the structure names a
    // blob by index and the samples travel beside it as the bytes they already
    // were.
    let taken: Vec<f32> = (0..2048).map(|i| (i as f32 / 2048.0) - 0.5).collect();
    let copied = Clipboard::samples(2, 1024, 48_000.0, 0).from_node(NodeId(5));
    assert_eq!(copied.blobs(), 1);
    assert_eq!(copied.values(), Some(2048));

    let (pasted, blobs) = cross(&copied, vec![encode_samples(&taken)]);
    assert_eq!(pasted, copied);
    assert_eq!(pasted.origin, Some(NodeId(5)));
    assert_eq!(blobs.len(), pasted.blobs());
    assert_eq!(decode_samples(&blobs[0]), taken);
}

#[test]
fn a_block_carries_its_rate_and_arrives_at_that_rate() {
    // The other half of the acceptance, and it is a statement about what the
    // crate does *not* have: there is no conversion here to accidentally run.
    // Resampling is an edit -- something an owner performs and logs -- so a
    // paste reads the rate and decides.
    let block: Vec<f32> = vec![0.125, -0.25, 0.5, -1.0];
    let copied = Clipboard::samples(1, 4, 44_100.0, 0);
    let (pasted, blobs) = cross(&copied, vec![encode_samples(&block)]);

    assert_eq!(pasted.sample_rate(), Some(44_100.0));
    assert_eq!(
        decode_samples(&blobs[0]),
        block,
        "the same samples, bit for bit"
    );
}

#[test]
fn a_paste_can_tell_a_rate_mismatch_from_a_match() {
    // What the owner does about it is the owner's; what the clipboard owes is
    // the number, unambiguously.
    let copied = Clipboard::samples(1, 4, 44_100.0, 0);
    assert_ne!(copied.sample_rate(), Some(48_000.0));
    assert_eq!(Clipboard::text("hello").sample_rate(), None);
}

#[test]
fn a_truncated_paste_is_detectable() {
    // A sample block that arrived with no blob is truncated, not empty --
    // pasting silence would be worse than declining.
    let copied = Clipboard::samples(2, 512, 48_000.0, 0);
    let arrived: Vec<Vec<u8>> = Vec::new();
    assert!(arrived.len() < copied.blobs());
    assert!(!copied.is_empty(), "the header says there is something");
}

#[test]
fn a_payload_can_be_checked_against_its_header() {
    let copied = Clipboard::samples(2, 1024, 48_000.0, 0);
    let short = encode_samples(&vec![0.0; 100]);
    assert_ne!(Some(decode_samples(&short).len()), copied.values());

    let whole = encode_samples(&vec![0.0; 2048]);
    assert_eq!(Some(decode_samples(&whole).len()), copied.values());
}

// ---- the kinds ----

#[test]
fn every_kind_survives_the_crossing() {
    let kinds = [
        Clipboard::text("a, b, c"),
        Clipboard::elements([note(1, 0.0), note(2, 1.0)]),
        Clipboard::samples(2, 1024, 48_000.0, 0),
        Clipboard::spectral(64, 513, 2, 256, 1024, 48_000.0, 0),
    ];
    for clipboard in kinds {
        let (crossed, _) = cross(&clipboard, Vec::new());
        assert_eq!(crossed, clipboard, "{}", clipboard.kind());
    }
}

#[test]
fn copied_elements_keep_the_offsets_they_had() {
    // A copied selection of notes is placed members, so the recursion is the
    // tree's own and a paste needs no second shape to interpret.
    let copied = Clipboard::elements([note(1, 0.0), note(2, 2.5)]);
    let (pasted, _) = cross(&copied, Vec::new());
    let Content::Elements { members } = pasted.content else {
        panic!("elements");
    };
    assert_eq!(members[0].offset, 0.0);
    assert_eq!(members[1].offset, 2.5);
    assert_eq!(members[1].node.id, NodeId(2));
    assert_eq!(members[1].dur, Some(1.0));
}

#[test]
fn a_generators_configuration_crosses_the_clipboard_unread() {
    // The same rule the document itself runs on: a leaf is opaque, so copying
    // one carries its configuration whole rather than dropping what this build
    // does not understand.
    let config = serde_json::json!({"kind": "some-future-pattern", "seed": 42});
    let member = Member {
        offset: 0.0,
        dur: None,
        node: Node::new(
            NodeId(3),
            Body::Generator {
                config: Opaque(config.clone()),
                rendered: None,
            },
        ),
    };
    let (pasted, _) = cross(&Clipboard::elements([member]), Vec::new());
    let Content::Elements { members } = pasted.content else {
        panic!("elements");
    };
    let Body::Generator {
        config: carried, ..
    } = &members[0].node.body
    else {
        panic!("a generator");
    };
    assert_eq!(carried.0, config);
}

// ---- what was there before kinds ----

#[test]
fn a_plain_string_still_reads_as_a_clipboard() {
    // K6's host-wide clipboard was a `String`, and the flat notes block still
    // travels that way. It is a kind now, not a special case.
    let clipboard = Clipboard::parse("0.0 1.0 60 100 0");
    assert_eq!(clipboard.kind(), "text");
    assert_eq!(clipboard, Clipboard::text("0.0 1.0 60 100 0"));
}

#[test]
fn the_fallback_is_a_door_and_not_a_guess() {
    // A stored string that happens to be JSON is still a stored string. An
    // untagged fallback would paste a document where the person copied a line,
    // which is the silent kind of wrong.
    let looks_like_json = r#"{"note": "a text a person copied"}"#;
    let clipboard = Clipboard::parse(looks_like_json);
    assert_eq!(clipboard.kind(), "text");

    // And a real clipboard document reads as itself.
    let real = Clipboard::samples(1, 8, 48_000.0, 0);
    assert_eq!(Clipboard::parse(&real.to_json()), real);
}

#[test]
fn an_empty_clipboard_says_so_in_every_kind() {
    assert!(Clipboard::text("").is_empty());
    assert!(Clipboard::elements([]).is_empty());
    assert!(Clipboard::samples(2, 0, 48_000.0, 0).is_empty());
    assert!(Clipboard::spectral(0, 513, 1, 256, 1024, 48_000.0, 0).is_empty());
    assert!(!Clipboard::text(" ").is_empty());
}

// ---- the encoding ----

#[test]
fn the_bulk_encoding_is_little_endian_f32_and_round_trips() {
    // One implementation, because three languages writing the same byte order
    // three times is three places for it to be wrong -- and the wrong one
    // sounds like noise rather than failing.
    assert_eq!(encode_samples(&[1.0]), 1.0f32.to_le_bytes().to_vec());
    let values = vec![0.0, -1.0, 1.0, 0.333_333_34, f32::MIN, f32::MAX];
    assert_eq!(decode_samples(&encode_samples(&values)), values);
}

#[test]
fn a_trailing_partial_value_is_dropped_rather_than_guessed_at() {
    let mut bytes = encode_samples(&[0.5, 0.25]);
    bytes.push(0xff);
    assert_eq!(decode_samples(&bytes), vec![0.5, 0.25]);
}
