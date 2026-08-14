//! The document this crate defines is the one the Python client writes.
//!
//! The crate's own suite proves the shape round-trips against itself, which
//! says nothing about whether anyone else agrees with it. This does: the vector
//! beside it is a composition built with `clausters.form` and converted through
//! `clausters.form.document` (`gen-form-vector.py` writes it, and it is
//! committed). Nothing in CI runs the Python client's call sites, so without a
//! crossing like this one the two halves of O1 could drift until a user found
//! out with a document that would not open.
//!
//! When the format changes on purpose: re-run the generator and commit whatever
//! moved. When it changes by accident, this fails first.

use clausters_document::*;

const VECTOR: &str = include_str!("form_vector.json");

fn vector() -> Document {
    serde_json::from_str(VECTOR).expect("the Python client's document must parse here")
}

#[test]
fn the_clients_document_parses_and_survives_a_round_trip() {
    let doc = vector();
    assert_eq!(doc.version, 1);

    // Lossless rather than byte-identical, for the reason the crate's own suite
    // states: key order in JSON carries no information.
    let out: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
    let original: serde_json::Value = serde_json::from_str(VECTOR).unwrap();
    assert_eq!(out, original);
}

#[test]
fn the_arrangements_bodies_all_arrive_as_themselves() {
    let doc = vector();
    let members = doc.root.members();

    // A set of one of everything, in the order the generator places them.
    assert!(matches!(
        doc.root.body,
        Body::Set {
            grouping: Grouping::Concrete,
            ..
        }
    ));
    assert!(matches!(members[0].node.body, Body::Event { .. }));
    assert!(matches!(members[1].node.body, Body::Set { .. })); // the track
    assert!(matches!(members[2].node.body, Body::Buffer { .. }));
    assert!(matches!(members[3].node.body, Body::Sequence { .. }));
    assert!(matches!(
        members[4].node.body,
        Body::Set {
            grouping: Grouping::Logical,
            ..
        }
    ));
    assert!(matches!(members[5].node.body, Body::Generator { .. }));
}

#[test]
fn a_track_arrives_as_a_set_whose_notes_are_placed_nodes() {
    // Decision A, seen from the other side: a note is an addressable node with
    // an id, which is what will let an intent name it and a log invert it.
    let doc = vector();
    let track = &doc.root.members()[1].node;
    let notes = track.members();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].offset, 0.0);
    assert_eq!(notes[1].offset, 1.5);
    let Body::Event { config, .. } = &notes[0].node.body else {
        panic!("not an event")
    };
    assert_eq!(config.0["midinote"], 64);
    assert_ne!(notes[0].node.id, notes[1].node.id);
}

#[test]
fn a_generators_code_arrives_as_a_reference_and_is_never_read() {
    let doc = vector();
    let chain = &doc.root.members()[4].node;
    let Body::Generator { config } = &chain.members()[0].node.body else {
        panic!("not a generator")
    };
    // The crate has no idea what `rlpf` is, and that is the point.
    assert_eq!(config.0["generator"], "rlpf");
    assert_eq!(config.0["controls"]["cutoff"], 900.0);
}

#[test]
fn a_resident_generator_arrives_unlocatable() {
    let doc = vector();
    let resident = &doc.root.members()[5].node;
    assert!(!resident.locatable());
    assert!(doc.root.members()[0].node.locatable());
}

#[test]
fn material_arrives_with_its_lifetime() {
    let doc = vector();
    let Body::Buffer { source, config } = &doc.root.members()[2].node.body else {
        panic!("not a buffer")
    };
    assert_eq!(source.source, SourceId(7));
    assert_eq!(source.lifetime, Lifetime::Session);
    // A buffer is data; what plays it is configuration, and configuration is
    // the client's to interpret.
    assert_eq!(config.0["instrument"], "take");
}

#[test]
fn the_ids_are_unique_across_the_whole_tree() {
    // An intent names a node by id, so a collision would silently address the
    // wrong element -- and ids are minted on the client, where nothing but this
    // checks them.
    let doc = vector();
    let mut ids = Vec::new();
    doc.root.walk(&mut |node| ids.push(node.id));
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "duplicate node id in {ids:?}");
    assert_eq!(doc.max_id(), *sorted.last().unwrap());
}
