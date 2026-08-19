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
const SESSION: &str = include_str!("session_vector.json");

fn vector() -> Document {
    serde_json::from_str(VECTOR).expect("the Python client's document must parse here")
}

fn session() -> Session {
    serde_json::from_str(SESSION).expect("the Python client's session must parse here")
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

    // An aggregate of one of everything, in the order the generator places them.
    assert!(matches!(
        doc.root.body,
        Body::Aggregate {
            grouping: Grouping::Concrete,
            ..
        }
    ));
    assert!(matches!(members[0].node.body, Body::Clang { .. }));
    assert!(matches!(members[1].node.body, Body::Aggregate { .. })); // the track
    assert!(matches!(members[2].node.body, Body::Vector { .. }));
    assert!(matches!(members[3].node.body, Body::Sequence { .. }));
    assert!(matches!(
        members[4].node.body,
        Body::Aggregate {
            grouping: Grouping::Logical,
            ..
        }
    ));
    assert!(matches!(members[5].node.body, Body::Generator { .. }));
}

#[test]
fn a_track_arrives_as_an_aggregate_whose_notes_are_placed_nodes() {
    // Decision A, seen from the other side: a note is an addressable node with
    // an id, which is what will let an intent name it and a log invert it.
    let doc = vector();
    let track = &doc.root.members()[1].node;
    let notes = track.members();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].offset, 0.0);
    assert_eq!(notes[1].offset, 1.5);
    let Body::Clang { config, .. } = &notes[0].node.body else {
        panic!("not a clang")
    };
    assert_eq!(config.0["midinote"], 64);
    assert_ne!(notes[0].node.id, notes[1].node.id);
}

#[test]
fn a_generators_code_arrives_as_a_reference_and_is_never_read() {
    let doc = vector();
    let chain = &doc.root.members()[4].node;
    let Body::Generator { config, .. } = &chain.members()[0].node.body else {
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
fn a_source_arrives_with_its_lifetime() {
    let doc = vector();
    let Body::Vector { source, config } = &doc.root.members()[2].node.body else {
        panic!("not a vector")
    };
    assert_eq!(source.source, SourceId(7));
    assert_eq!(source.lifetime, Lifetime::Session);
    // A vector is data; what plays it is configuration, and configuration is
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

#[test]
fn a_generators_last_rendered_result_crosses_as_ordinary_tree() {
    // O8: what a host with no language attached shows. The generator's own
    // configuration stays opaque; what it produced is tree, so the same reader
    // walks both without a second shape.
    let document = vector();
    let generator = document
        .root
        .members()
        .iter()
        .map(|m| &m.node)
        .find(|n| n.rendered().is_some())
        .expect("a generator carrying its last result");

    let Body::Generator { config, .. } = &generator.body else {
        panic!("a generator");
    };
    assert_eq!(config.0["generator"], "melody");

    let rendered = generator.rendered().unwrap();
    assert_eq!(rendered.duration, Some(2.0));
    assert_eq!(rendered.members().len(), 2);
    assert!(matches!(rendered.body, Body::Aggregate { .. }));

    // And a reader walks into it: an id inside a rendering is reachable and
    // counted, so a client continuing to allocate cannot collide with one.
    let inner = rendered.members()[0].node.id;
    assert_eq!(document.find(inner).map(|n| n.id), Some(inner));
    assert!(document.max_id() >= inner);
}

// ---- the session: the format with two writers ----

#[test]
fn a_session_written_by_the_python_client_opens_here() {
    // O8's acceptance, in the direction a test can actually run: the other
    // writer of this format is a `standalone` host, and a format with two
    // writers in two languages is a format that drifts unless something
    // crosses.
    let session = session();
    assert!(session.is_readable());
    assert_eq!(
        session.provenance.as_ref().unwrap().0["script"],
        "song.py",
        "what produced it, carried and never read"
    );
    assert!(
        session.dangling().is_empty(),
        "every source it names is in the table"
    );
}

#[test]
fn a_source_table_written_there_reads_as_sources_here() {
    let session = session();
    let take = session.source(SourceId(7)).expect("the take");
    assert_eq!(
        take.location,
        Location::File {
            path: "/home/someone/takes/vocal.wav".into()
        }
    );
    assert_eq!(take.lifetime, Lifetime::External);
    assert_eq!(take.sample_rate, Some(48_000.0));
    assert!(take.is_resolvable());
}

#[test]
fn a_session_saved_mid_edit_opens_with_the_edit_still_open() {
    // The scratch was promoted by the save and the decision was left to the
    // person -- so this is what reopening has to say about it.
    let session = session();
    assert_eq!(session.open_edits(), vec![SourceId(8)]);
    let scratch = session.source(SourceId(8)).unwrap();
    assert_eq!(scratch.lifetime, Lifetime::Session, "promoted");
    assert_eq!(scratch.generation, 3, "and edited three times");
    let edit = scratch.editing.as_ref().unwrap();
    assert_eq!(edit.from, SourceId(7));
    assert!(!edit.confirmed);
}

#[test]
fn a_source_never_written_down_crosses_as_volatile() {
    let session = session();
    assert_eq!(session.volatile(), vec![SourceId(9)]);
}
