//! O8's acceptance: the format round-trips whole, a generator's blob survives
//! unread in both directions, and a session saved mid-edit reopens with the
//! edit still open.

use super::*;
use crate::{Beats, Body, Grouping, Member, Node, NodeId, Range, SourceRef};

fn source_ref(id: u64, generation: u64, lifetime: Lifetime) -> SourceRef {
    SourceRef {
        source: SourceId(id),
        lifetime,
        generation,
        range: Some(Range {
            start: 0,
            end: 48_000,
        }),
    }
}

fn placed(offset: Beats, node: Node) -> Member {
    Member {
        offset,
        dur: None,
        node,
    }
}

fn take(id: u64, source: u64) -> Node {
    Node::new(
        NodeId(id),
        Body::Vector {
            source: source_ref(source, 0, Lifetime::External),
            config: Opaque(serde_json::json!({"instrument": "playbuf"})),
        },
    )
}

/// A composition with samples, a generator holding its last rendered result,
/// and a plain clang -- one of everything the format has to carry.
fn composition() -> Document {
    let rendered = Node {
        id: NodeId(20),
        name: None,
        onset: None,
        duration: Some(4.0),
        resident: false,
        body: Body::Sequence {
            config: Opaque::none(),
            members: vec![
                placed(
                    0.0,
                    Node::new(
                        NodeId(21),
                        Body::Clang {
                            config: Opaque(serde_json::json!({"midinote": 60})),
                            fires: None,
                        },
                    ),
                ),
                placed(
                    1.0,
                    Node::new(
                        NodeId(22),
                        Body::Clang {
                            config: Opaque(serde_json::json!({"midinote": 64})),
                            fires: None,
                        },
                    ),
                ),
            ],
        },
    };
    Document::new(Node::new(
        NodeId(1),
        Body::Aggregate {
            grouping: Grouping::Concrete,
            members: vec![
                placed(0.0, take(2, 100)),
                placed(
                    4.0,
                    Node::new(
                        NodeId(3),
                        Body::Generator {
                            config: Opaque(serde_json::json!({
                                "kind": "pbind",
                                "ref": "melody",
                                "seed": 7
                            })),
                            rendered: Some(Box::new(rendered)),
                        },
                    ),
                ),
            ],
            config: Opaque::none(),
        },
    ))
}

fn saved() -> Session {
    Session::new(composition())
        .with_source(
            SourceId(100),
            Source::file("/home/someone/takes/vocal.wav", Lifetime::External)
                .shaped(1, 48_000, 48_000.0),
        )
        .produced_by(Opaque(serde_json::json!({"script": "song.py"})))
}

fn reopen(session: &Session) -> Session {
    let written = serde_json::to_string(session).unwrap();
    serde_json::from_str(&written).unwrap()
}

#[test]
fn a_session_round_trips_whole() {
    let session = saved();
    assert_eq!(reopen(&session), session);
    assert!(session.is_readable());
}

#[test]
fn a_generators_blob_survives_both_directions_unread() {
    // O8's acceptance, and the rule the whole crate runs on: a generator is
    // code in the language of whoever wrote it, so the format's job is to not
    // lose it rather than to understand it.
    let session = saved();
    let opened = reopen(&session);
    let node = opened.document.find(NodeId(3)).unwrap();
    let Body::Generator { config, .. } = &node.body else {
        panic!("a generator");
    };
    assert_eq!(config.0["kind"], "pbind");
    assert_eq!(config.0["seed"], 7);
    assert_eq!(config.0["ref"], "melody");
}

#[test]
fn a_generators_last_rendered_result_is_part_of_the_format() {
    // What a host with no language attached shows. It is in the format rather
    // than in a cache because a cache can be missing, and then there is nothing
    // to draw at all.
    let opened = reopen(&saved());
    let generator = opened.document.find(NodeId(3)).unwrap();
    let rendered = generator.rendered().expect("the frozen result");
    assert_eq!(rendered.duration, Some(4.0));
    assert_eq!(rendered.members().len(), 2);
    assert_eq!(rendered.members()[1].node.id, NodeId(22));
}

#[test]
fn a_rendered_result_is_reachable_to_a_reader_and_not_to_an_edit() {
    // The line the field draws: a reader must see it, and an intent must not
    // -- a rendering is not the composition, and editing one writes over what
    // the next render replaces.
    let document = composition();
    assert!(
        document.find(NodeId(21)).is_some(),
        "a reader walks into it"
    );
    assert_eq!(document.max_id(), NodeId(22), "and counts its ids");

    // `apply` reaches placements only, so a node inside a rendering is not
    // addressable by an edit.
    let mut d = document;
    let outcome = crate::apply(
        &mut d,
        &crate::Intent::Place {
            node: NodeId(21),
            offset: 3.0,
            dur: None,
        },
        &crate::Against::unstated(),
        &crate::Rules::none(),
    );
    assert!(!outcome.applied);
    assert_eq!(outcome.reason.as_deref(), Some("no such node"));
}

// ---- an edit that is still open ----

fn mid_edit() -> Session {
    let mut session = saved();
    session.sources.insert(
        SourceId(101),
        Source::file("scratch/vocal-edit.wav", Lifetime::Temporary)
            .shaped(1, 48_000, 48_000.0)
            .editing(SourceId(100)),
    );
    session
}

#[test]
fn a_session_saved_mid_edit_reopens_with_the_edit_still_open() {
    // O8's acceptance. A save never blocks on a confirmation, so the format has
    // to be able to say *this is a working copy of that, and nobody has decided
    // yet*.
    let mut session = mid_edit();
    assert_eq!(session.open_edits(), vec![SourceId(101)]);

    // Saving promotes the scratch and leaves the edit open.
    assert!(session.promote(SourceId(101)));
    let opened = reopen(&session);

    let scratch = opened.source(SourceId(101)).unwrap();
    assert_eq!(scratch.lifetime, Lifetime::Session, "promoted by the save");
    assert!(scratch.is_being_edited(), "and still undecided");
    assert_eq!(scratch.editing.as_ref().unwrap().from, SourceId(100));
    assert_eq!(opened.open_edits(), vec![SourceId(101)]);
}

#[test]
fn saving_is_not_an_edit_and_confirming_is() {
    // The two rejected alternatives, stated as behavior: promoting does not
    // confirm, and confirming is a separate act.
    let mut session = mid_edit();
    session.promote(SourceId(101));
    assert!(session.source(SourceId(101)).unwrap().is_being_edited());

    assert!(session.confirm(SourceId(101)));
    assert!(!session.source(SourceId(101)).unwrap().is_being_edited());
    assert!(session.open_edits().is_empty());
}

#[test]
fn only_a_working_copy_is_promoted() {
    // A save must not relabel the user's own file, which is read-only and never
    // touched, nor re-promote what is already saved.
    let mut session = mid_edit();
    assert!(!session.promote(SourceId(100)), "the user's file");
    assert_eq!(
        session.source(SourceId(100)).unwrap().lifetime,
        Lifetime::External
    );
    session.promote(SourceId(101));
    assert!(!session.promote(SourceId(101)), "already promoted");
}

// ---- what a save and an open have to report ----

#[test]
fn a_source_never_written_down_is_named_rather_than_pretended_about() {
    // A server buffer never exported. Saving is not blocked by it -- that would
    // block the safest habit in the program -- but the file cannot claim to be
    // complete either.
    let mut session = saved();
    session
        .sources
        .insert(SourceId(102), Source::volatile(Lifetime::Session));
    assert_eq!(session.volatile(), vec![SourceId(102)]);
    assert!(session.source(SourceId(100)).unwrap().is_resolvable());

    let opened = reopen(&session);
    assert_eq!(opened.volatile(), vec![SourceId(102)]);
}

#[test]
fn a_source_the_tree_names_and_the_table_lacks_is_reported_once() {
    // What an opening reader says up front, rather than discovering it one
    // element at a time halfway through drawing.
    let mut session = saved();
    session.sources.remove(&SourceId(100));
    assert_eq!(session.dangling(), vec![SourceId(100)]);

    let whole = saved();
    assert!(whole.dangling().is_empty());
}

#[test]
fn provenance_is_carried_and_never_read() {
    // What makes re-generating possible without the document knowing how: the
    // recipe stays in the language that wrote it.
    let session = saved().with_source(
        SourceId(103),
        Source::file("renders/bounce.wav", Lifetime::Session).produced_by(Opaque(
            serde_json::json!({"rendered_from": 3, "at": "2026-08-14T10:00:00Z"}),
        )),
    );
    let opened = reopen(&session);
    assert_eq!(
        opened.provenance.as_ref().unwrap().0["script"],
        "song.py",
        "the session's own"
    );
    assert_eq!(
        opened
            .source(SourceId(103))
            .unwrap()
            .provenance
            .as_ref()
            .unwrap()
            .0["rendered_from"],
        3
    );
}

#[test]
fn a_newer_format_is_refused_rather_than_half_read() {
    // An added *field* is not a version change -- an older reader ignores it,
    // the way an unknown body is carried rather than dropped. A format number
    // moves only when reading it wrongly is the alternative.
    let mut session = saved();
    assert!(session.is_readable());
    session.format = FORMAT + 1;
    assert!(!session.is_readable());
}

#[test]
fn an_unknown_body_survives_a_save_and_an_open() {
    // The forward-compatibility door, exercised through the file rather than
    // only through the tree: a session written by a newer client opens here
    // with what this build does not understand still intact.
    let future = serde_json::json!({
        "id": 9,
        "kind": "constellation",
        "spread": 0.5
    });
    let node: Node = serde_json::from_value(future.clone()).unwrap();
    let session = Session::new(Document::new(node));
    let opened = reopen(&session);
    let Body::Unknown(carried) = &opened.document.root.body else {
        panic!("preserved whole");
    };
    assert_eq!(carried["kind"], "constellation");
    assert_eq!(carried["spread"], 0.5);
}

// ---- the arrangement the session now carries ----

#[test]
fn a_session_carries_an_arrangement_and_writes_none_when_there_is_none() {
    use crate::arrangement::{Arrangement, Content, Region, Tempo, Track};
    use crate::timebase::Beat;

    // Nothing said, nothing written: every session saved before this existed
    // reads back identical, which is what makes the field an addition rather
    // than a format change.
    let plain = saved();
    let json = serde_json::to_string(&plain).unwrap();
    assert!(!json.contains("arrangement"), "{json}");
    assert_eq!(reopen(&plain), plain);

    let mut arrangement = Arrangement::new();
    arrangement.set_tempo(Tempo::at(Beat(0.0), 96.0));
    let mut track = Track::new(NodeId(80), NodeId(81)).named("drums");
    track.active_lane_mut().unwrap().place(Region::new(
        NodeId(82),
        Beat(0.0),
        Beat(4.0),
        Content::Composite {
            node: Box::new(Node::new(
                NodeId(83),
                Body::Clang {
                    config: crate::Opaque::none(),
                    fires: None,
                },
            )),
        },
    ));
    arrangement.tracks.push(track);
    let session = plain.with_arrangement(arrangement);
    let opened = reopen(&session);
    assert_eq!(opened.arrangement.end(), Beat(4.0));
    assert_eq!(opened.arrangement.tempo_at(Beat(2.0)).unwrap().bpm, 96.0);
    assert_eq!(opened, session);
}

#[test]
fn a_source_only_a_region_names_is_still_reported_missing() {
    use crate::arrangement::{Arrangement, Content, Region, Track};
    use crate::timebase::Beat;
    use crate::{Lifetime, SegmentRef, SegmentSource, SourceRef};

    // The table is walked against **both** halves. A reader that checked only
    // the tree would open a session missing exactly what the arrangement plays,
    // and an alternate take counts: it names its source whether or not it is
    // the lane that plays.
    let window = |source: u64| SegmentRef {
        source: SegmentSource::Samples(SourceRef {
            source: SourceId(source),
            lifetime: Lifetime::Session,
            generation: 0,
            range: None,
        }),
        start: 0.0,
        duration: 1.0,
    };
    let mut arrangement = Arrangement::new();
    let mut track = Track::new(NodeId(90), NodeId(91));
    track.active_lane_mut().unwrap().place(Region::new(
        NodeId(92),
        Beat(0.0),
        Beat(4.0),
        Content::window(window(700)),
    ));
    track.lanes.push(crate::arrangement::Lane::new(NodeId(93)));
    track.lanes[1].place(Region::new(
        NodeId(94),
        Beat(0.0),
        Beat(4.0),
        Content::window(window(701)),
    ));
    arrangement.tracks.push(track);

    let session = Session::new(Document::new(Node::new(
        NodeId(1),
        Body::Clang {
            config: crate::Opaque::none(),
            fires: None,
        },
    )))
    .with_arrangement(arrangement);
    assert_eq!(session.dangling(), vec![SourceId(700), SourceId(701)]);
}

#[test]
fn a_top_level_field_a_newer_writer_added_survives_a_save() {
    let json = r#"{"format":1,"document":{"version":1,"root":{"id":1,"kind":"clang"}},
                   "mixer":{"buses":[{"id":1,"name":"reverb"}]}}"#;
    let session: Session = serde_json::from_str(json).unwrap();
    assert!(session.extra.contains_key("mixer"));
    let back = serde_json::to_value(&session).unwrap();
    assert_eq!(back["mixer"]["buses"][0]["name"], "reverb");
}
