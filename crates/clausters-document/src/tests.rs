//! O1's acceptance, plus the derivations the client already relies on.
//!
//! What is checked here is deliberately narrow: this milestone is the *shape*,
//! so the tests say that the shape survives a round trip, that what this build
//! does not understand survives it too, and that nothing derived was stored by
//! accident.

use super::*;

fn node(id: u64, body: Body) -> Node {
    Node::new(NodeId(id), body)
}

fn event(id: u64) -> Node {
    node(
        id,
        Body::Event {
            config: Opaque::none(),
            fires: None,
        },
    )
}

fn placed(offset: Beats, dur: Option<Beats>, node: Node) -> Member {
    Member { offset, dur, node }
}

fn set(id: u64, members: Vec<Member>) -> Node {
    node(
        id,
        Body::Set {
            grouping: Grouping::Concrete,
            members,
            config: Opaque::none(),
        },
    )
}

/// What a document is after a write and a read: the comparison every round-trip
/// test makes, since equality of *value* is the property, not equality of bytes.
fn reparse(doc: &Document) -> serde_json::Value {
    serde_json::from_str(&serde_json::to_string(doc).unwrap()).unwrap()
}

#[test]
fn a_tree_round_trips_unchanged() {
    let doc = Document::new(set(
        1,
        vec![
            placed(0.0, Some(2.0), event(2)),
            placed(
                2.0,
                None,
                node(
                    3,
                    Body::Buffer {
                        source: SourceRef {
                            source: SourceId(7),
                            lifetime: Lifetime::Session,
                            generation: 3,
                            range: Some(Range { start: 0, end: 480 }),
                        },
                        config: Opaque::none(),
                    },
                ),
            ),
        ],
    ));

    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(doc, back);
    // And writing is deterministic, which is what a saved session needs to be
    // before anyone diffs one: the same document twice is the same bytes.
    assert_eq!(json, serde_json::to_string(&back).unwrap());
}

#[test]
fn a_body_this_build_does_not_know_survives_whole() {
    // A document written by a newer writer: a body kind that does not exist
    // here, carrying fields nobody in this build can name. Losing it would lose
    // the piece, so it is preserved rather than dropped -- the same rule the
    // widget protocol runs on.
    let json =
        r#"{"version":4,"root":{"id":1,"kind":"constellation","spread":0.5,"seeds":[1,2,3]}}"#;
    let doc: Document = serde_json::from_str(json).unwrap();
    assert!(matches!(doc.root.body, Body::Unknown(_)));
    // Lossless rather than byte-identical: `serde_json` sorts an object's keys,
    // so what comes back out is the same *value*, which is the property that
    // matters -- key order in JSON carries no information, and buying its
    // preservation would mean turning on `preserve_order` for every crate in
    // the workspace, since features are additive.
    assert_eq!(
        reparse(&doc),
        serde_json::from_str::<serde_json::Value>(json).unwrap()
    );
}

#[test]
fn a_leaf_config_is_carried_and_never_read() {
    // The generator's own configuration is code in someone else's language. The
    // document transports it byte for byte and has no opinion about it.
    let json =
        r#"{"id":9,"kind":"generator","config":{"pattern":"Pseq","of":[1,2,3],"repeats":null}}"#;
    let node: Node = serde_json::from_str(json).unwrap();
    let Body::Generator { config, .. } = &node.body else {
        panic!("not a generator")
    };
    assert_eq!(config.0["pattern"], "Pseq");
    assert_eq!(config.0["repeats"], serde_json::Value::Null); // a null is content, not an absence
    assert_eq!(
        serde_json::to_value(&node).unwrap(),
        serde_json::from_str::<serde_json::Value>(json).unwrap()
    );
}

#[test]
fn a_generators_output_is_ordinary_tree() {
    // Nothing about being generated makes a subtree a second kind of thing: the
    // events a generator produced are events, placed in a set, indistinguishable
    // from ones a hand wrote.
    let produced = set(10, vec![placed(0.0, Some(1.0), event(11))]);
    let json = serde_json::to_string(&produced).unwrap();
    let back: Node = serde_json::from_str(&json).unwrap();
    assert_eq!(produced, back);
}

#[test]
fn a_punctual_event_can_reference_the_generator_it_fires() {
    // Structure resolved at run time rather than at render time: no flattening
    // pass expands this, and the reference has to survive the format.
    let mut trigger = event(5);
    trigger.onset = Some(4.0);
    trigger.body = Body::Event {
        config: Opaque::none(),
        fires: Some(NodeId(9)),
    };

    assert_eq!(trigger.character(), Character::Punctual);
    let back: Node = serde_json::from_str(&serde_json::to_string(&trigger).unwrap()).unwrap();
    assert_eq!(trigger, back);
}

#[test]
fn the_temporal_character_is_derived_from_what_is_present() {
    let mut n = event(1);
    assert_eq!(n.character(), Character::Abstract);
    n.duration = Some(2.0);
    assert_eq!(n.character(), Character::Relative);
    n.onset = Some(1.0);
    assert_eq!(n.character(), Character::Segment);
    n.duration = None;
    assert_eq!(n.character(), Character::Punctual);
}

#[test]
fn a_resident_generator_is_not_locatable() {
    let mut generator = node(
        1,
        Body::Generator {
            config: Opaque::none(),
            rendered: None,
        },
    );
    assert!(generator.locatable());
    generator.resident = true;
    assert!(!generator.locatable());
}

#[test]
fn the_temporal_relation_reads_the_placements_and_nothing_else() {
    // Simultaneous: shared start and shared end -- the container that can be
    // reinterpreted, which is what the recursion rests on.
    let simultaneous = set(
        1,
        vec![
            placed(0.0, Some(4.0), event(2)),
            placed(0.0, Some(4.0), event(3)),
        ],
    );
    assert_eq!(simultaneous.body.relation(), Some(Relation::Simultaneous));

    // A single member is simultaneous with itself.
    let one = set(1, vec![placed(1.0, Some(2.0), event(2))]);
    assert_eq!(one.body.relation(), Some(Relation::Simultaneous));

    // Successive: tiling contiguously, in any order of declaration.
    let successive = set(
        1,
        vec![
            placed(2.0, Some(2.0), event(3)),
            placed(0.0, Some(2.0), event(2)),
        ],
    );
    assert_eq!(successive.body.relation(), Some(Relation::Successive));

    // A gap makes it mixed -- silence between two members is a relation, not a
    // rounding error.
    let gap = set(
        1,
        vec![
            placed(0.0, Some(1.0), event(2)),
            placed(2.0, Some(1.0), event(3)),
        ],
    );
    assert_eq!(gap.body.relation(), Some(Relation::Mixed));

    // And a body that holds no members has no relation to report.
    assert_eq!(event(1).body.relation(), None);
    assert_eq!(set(1, vec![]).body.relation(), None);
}

#[test]
fn a_members_duration_falls_back_to_the_elements_own() {
    // The placement's `dur` trims; without one the element's own length is what
    // the relation reads, or the two would disagree about the same tiling.
    let mut first = event(2);
    first.duration = Some(2.0);
    let mut second = event(3);
    second.duration = Some(2.0);
    let s = set(1, vec![placed(0.0, None, first), placed(2.0, None, second)]);
    assert_eq!(s.body.relation(), Some(Relation::Successive));
}

#[test]
fn placements_that_round_tripped_through_floats_still_read_as_simultaneous() {
    // The wire is f32 in places and beats are computed; an exact comparison
    // would call this mixed, which is the kind of thing that is only ever found
    // by a user wondering why a group stopped being one.
    let drift = 0.1 + 0.2 - 0.3; // not exactly zero
    let s = set(
        1,
        vec![
            placed(0.0, Some(4.0), event(2)),
            placed(drift, Some(4.0 - drift), event(3)),
        ],
    );
    assert_eq!(s.body.relation(), Some(Relation::Simultaneous));
}

#[test]
fn a_node_is_found_and_the_ids_in_use_are_known() {
    let doc = Document::new(set(
        1,
        vec![placed(
            0.0,
            None,
            set(2, vec![placed(0.0, None, event(30))]),
        )],
    ));
    assert_eq!(doc.find(NodeId(30)).map(|n| n.id), Some(NodeId(30)));
    assert_eq!(doc.find(NodeId(99)), None);
    // A client continuing a document it did not author needs this to allocate
    // past what is already there.
    assert_eq!(doc.max_id(), NodeId(30));
}

#[test]
fn the_tree_carries_no_lane_and_no_vertical_position() {
    // The restriction belongs to the view: a multitrack lane is a projection.
    // If this ever fails it is because the model grew track-ness to make a view
    // easier to write, which is the one thing the shape is meant to refuse.
    let json = serde_json::to_string(&set(1, vec![placed(0.0, None, event(2))])).unwrap();
    for forbidden in ["track", "lane", "\"y\"", "row", "height"] {
        assert!(
            !json.contains(forbidden),
            "{forbidden} reached the document: {json}"
        );
    }
}

#[test]
fn an_id_that_names_two_different_nodes_is_refused_at_the_door() {
    // The failure this rules out: an intent naming node 2 reaches whichever the
    // lookup finds first while the client that sent it keeps the other, so one
    // gesture writes two places and the picture springs back.
    let doc = Document::new(set(
        1,
        vec![
            placed(0.0, None, event(2)),
            placed(4.0, None, set(2, vec![])),
        ],
    ));
    let message = doc.duplicate_id().expect("a duplicate id");
    assert!(message.contains("node id 2"), "{message}");
    assert!(message.contains("event"), "{message}");
    assert!(message.contains("set"), "{message}");

    // And it is refused on the way in, which is the door every writer passes
    // through -- a client, a host, a file written by either.
    let json = serde_json::to_string(&doc).unwrap();
    let err = serde_json::from_str::<Document>(&json).expect_err("refused");
    assert!(err.to_string().contains("node id 2"), "{err}");
}

#[test]
fn one_element_placed_twice_is_carried_rather_than_refused() {
    // Ambiguous and consistent: which placement an intent names is an open
    // question with three answers, and refusing here would pick the one that
    // forbids it -- from inside a check about something else.
    let doc = Document::new(set(
        1,
        vec![placed(0.0, None, event(2)), placed(4.0, None, event(2))],
    ));
    assert_eq!(doc.duplicate_id(), None);
    let json = serde_json::to_string(&doc).unwrap();
    assert!(serde_json::from_str::<Document>(&json).is_ok());
}

#[test]
fn a_rendering_is_walked_for_ids_like_the_rest_of_the_tree() {
    // A generator's last rendered result is ordinary tree, so its nodes take
    // ids like any other and collide like any other.
    let doc = Document::new(set(
        1,
        vec![placed(
            0.0,
            None,
            node(
                2,
                Body::Generator {
                    config: Opaque::none(),
                    rendered: Some(Box::new(set(3, vec![placed(0.0, None, event(2))]))),
                },
            ),
        )],
    ));
    let message = doc.duplicate_id().expect("a duplicate id");
    assert!(message.contains("node id 2"), "{message}");
}

#[test]
fn a_name_is_a_label_that_survives_a_round_trip_and_addresses_nothing() {
    // The server's own rule for a group's name, taken rather than invented: the
    // id stays what an intent addresses, and the name says what the node is.
    let doc = Document::new(set(1, vec![placed(0.0, None, event(2).named("kick"))]).named("drums"));
    let back: Document = serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
    assert_eq!(back.root.name.as_deref(), Some("drums"));
    assert_eq!(back.find(NodeId(2)).unwrap().name.as_deref(), Some("kick"));
    // Nothing addresses by name, so an anonymous node is reachable exactly as
    // before -- and an anonymous one writes no key at all.
    let anonymous = Document::new(set(1, vec![]));
    assert!(!serde_json::to_string(&anonymous).unwrap().contains("name"));
}

#[test]
fn a_sets_own_restrictions_are_carried_and_never_read() {
    // One set kind, and a view's restrictions are the writer's business: a
    // multitrack's track is a set with restrictions, and this is how it gets
    // back to the writer that had it without the tree growing track-ness.
    let node = Node::new(
        NodeId(1),
        Body::Set {
            grouping: Grouping::Concrete,
            members: vec![placed(0.0, None, event(2))],
            config: Opaque(serde_json::json!({"form": "track"})),
        },
    );
    let doc = Document::new(node);
    let json = serde_json::to_string(&doc).unwrap();
    let back: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(back, doc, "carried whole, byte for byte");
    // And it is carried, not read: the typed tree still says only what a set
    // is, and the relation is derived from the placements as it always was.
    assert_eq!(back.root.body.members().len(), 1);
    assert!(matches!(back.root.body, Body::Set { .. }));
}
