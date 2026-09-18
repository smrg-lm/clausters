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

/// A document with samples, a generator holding its last rendered result,
/// and a plain clang -- one of everything the format has to carry.
fn a_document() -> Document {
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
    Session::new(a_document())
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
    // -- a rendering is not the document, and editing one writes over what
    // the next render replaces.
    let document = a_document();
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
    use crate::multitrack::{Content, Multitrack, Region, Tempo, Track};
    use crate::timebase::{Beat, Second};

    // Nothing said, nothing written: every session saved before this existed
    // reads back identical, which is what makes the field an addition rather
    // than a format change.
    let plain = saved();
    let json = serde_json::to_string(&plain).unwrap();
    assert!(!json.contains("arrangement"), "{json}");
    assert_eq!(reopen(&plain), plain);

    let mut multitrack = Multitrack::new();
    multitrack.set_tempo(Tempo::at(Beat(0.0), 1.6));
    let mut track = Track::new(NodeId(80), NodeId(81)).named("drums");
    track.active_lane_mut().unwrap().place(Region::new(
        NodeId(82),
        Second(0.0),
        Second(4.0),
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
    multitrack.tracks.push(track);
    let session = plain.with_multitrack(multitrack);
    let opened = reopen(&session);
    assert_eq!(opened.multitrack.end(), Second(4.0));
    assert_eq!(opened.multitrack.tempo_at(Beat(2.0)).unwrap().tempo, 1.6);
    assert_eq!(opened, session);
}

#[test]
fn a_source_only_a_region_names_is_still_reported_missing() {
    use crate::multitrack::{Content, Multitrack, Region, Track};
    use crate::timebase::Second;
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
    let mut multitrack = Multitrack::new();
    let mut track = Track::new(NodeId(90), NodeId(91));
    track.active_lane_mut().unwrap().place(Region::new(
        NodeId(92),
        Second(0.0),
        Second(4.0),
        Content::window(window(700)),
    ));
    track.lanes.push(crate::multitrack::Lane::new(NodeId(93)));
    track.lanes[1].place(Region::new(
        NodeId(94),
        Second(0.0),
        Second(4.0),
        Content::window(window(701)),
    ));
    multitrack.tracks.push(track);

    let session = Session::new(Document::new(Node::new(
        NodeId(1),
        Body::Clang {
            config: crate::Opaque::none(),
            fires: None,
        },
    )))
    .with_multitrack(multitrack);
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

/// **The milestone's acceptance, in one test.** A session with several tracks,
/// alternate lanes, overlapping layered regions, crossfades and automation
/// round-trips losslessly.
///
/// Written as one multitrack rather than as six assertions because the thing being
/// checked is that they survive *together*: a format can round-trip each of
/// these alone and still lose the layer order when two regions share a beat, or
/// drop the fade on the one that is not on top.
#[test]
fn a_whole_session_round_trips_losslessly() {
    use crate::multitrack::{
        Automation, Content, Fade, Lane, Marker, Meter, Multitrack, Region, Span, Tempo, Track,
    };
    use crate::timebase::{Beat, Second};
    use crate::{Lifetime, Point, SegmentRef, SegmentSource, SourceRef};

    let window = |source: u64, from: f64| SegmentRef {
        source: SegmentSource::Samples(SourceRef {
            source: SourceId(source),
            lifetime: Lifetime::Session,
            generation: 0,
            range: None,
        }),
        start: from,
        duration: 4.0,
    };

    let mut multitrack = Multitrack::new();
    multitrack.set_tempo(Tempo::at(Beat(0.0), 1.6));
    multitrack.set_tempo(Tempo::at(Beat(32.0), 2.0).ramping());
    multitrack.set_meter(Meter::at(Beat(0.0), 4, 4));
    multitrack.set_meter(Meter::at(Beat(32.0), 7, 8));
    multitrack.add_marker(Marker::new(NodeId(1), Second(0.0)).named("intro"));
    multitrack.add_marker(Marker::new(NodeId(2), Second(32.0)).named("B"));
    multitrack.loop_span = Some(Span::new(Second(0.0), Second(32.0)));
    multitrack.punch = Some(Span::new(Second(8.0), Second(16.0)));

    // A track comped from three takes, playing the second.
    let mut vocals = Track::new(NodeId(10), NodeId(11)).named("vocals");
    vocals.lanes[0].name = Some("take 1".into());
    vocals.lanes.push(Lane::new(NodeId(12)).named("take 2"));
    vocals.lanes.push(Lane::new(NodeId(13)).named("comp"));
    vocals.active = 1;
    for (lane, source) in [(0, 100), (1, 101), (2, 102)] {
        vocals.lanes[lane].place(
            Region::new(
                NodeId(20 + lane as u64),
                Second(0.0),
                Second(16.0),
                Content::window(window(source, 0.0)),
            )
            .named(format!("vox {lane}")),
        );
    }

    // A track whose two regions overlap, crossfaded, with the layer order
    // saying which is on top.
    let mut guitars = Track::new(NodeId(30), NodeId(31)).named("guitars");
    let mut first = Region::new(
        NodeId(32),
        Second(0.0),
        Second(20.0),
        Content::window(window(200, 0.0)),
    );
    first.fade_out = Some(Fade::of(Second(4.0)));
    let mut second = Region::new(
        NodeId(33),
        Second(16.0),
        Second(16.0),
        Content::window(window(201, 2.0)),
    );
    second.fade_in = Some(Fade::of(Second(4.0)));
    second.layer = 1;
    second.muted = true;
    guitars.lanes[0].place(first);
    guitars.lanes[0].place(second);
    let mut level = Automation::new(
        NodeId(34),
        crate::Opaque(serde_json::json!({"ctl": "level"})),
    );
    level.points = vec![
        Point {
            at: 0.0,
            value: 0.0,
            data: crate::Opaque::none(),
        },
        Point {
            at: 16.0,
            value: 1.0,
            data: crate::Opaque(serde_json::json!({"shape": "exp"})),
        },
    ];
    level.visible = true;
    guitars.automation.push(level);
    guitars.soloed = true;

    // A track placing the general tree, which is what a composite region is
    // for: everything the five primitives can build, given a position.
    let mut sections = Track::new(NodeId(40), NodeId(41)).named("sections");
    sections.lanes[0].place(Region::new(
        NodeId(42),
        Second(32.0),
        Second(16.0),
        Content::Composite {
            node: Box::new(Node::new(
                NodeId(43),
                Body::Aggregate {
                    grouping: Grouping::Concrete,
                    members: vec![placed(
                        0.0,
                        Node::new(
                            NodeId(44),
                            Body::Clang {
                                config: crate::Opaque::none(),
                                fires: None,
                            },
                        ),
                    )],
                    config: crate::Opaque::none(),
                },
            )),
        },
    ));

    multitrack.tracks.push(vocals);
    multitrack.tracks.push(guitars);
    multitrack.tracks.push(sections);

    let session = saved().with_multitrack(multitrack);
    let opened = reopen(&session);
    assert_eq!(opened, session, "the whole session, unchanged");

    // ...and the details a whole-value comparison would not name if it failed.
    let a = &opened.multitrack;
    assert_eq!(a.tracks.len(), 3);
    assert_eq!(a.end(), Second(48.0));
    assert_eq!(
        a.tracks[0].active_lane().unwrap().name.as_deref(),
        Some("take 2")
    );
    assert_eq!(
        a.tracks[0].lanes.len(),
        3,
        "the takes nobody chose are kept"
    );
    let guitars = &a.tracks[1].lanes[0];
    assert!(guitars.regions[0].overlaps(&guitars.regions[1]));
    assert_eq!(guitars.regions[1].layer, 1, "which one is on top");
    assert_eq!(
        guitars.regions[0].fade_out.as_ref().unwrap().length,
        Second(4.0)
    );
    assert_eq!(a.tracks[1].automation[0].points[1].data.0["shape"], "exp");
    assert!(a.tracks[2].lanes[0].regions[0].content.as_node().is_some());
    assert_eq!(a.tempo_at(Beat(40.0)).unwrap().tempo, 2.0);
    assert_eq!(a.meter_at(Beat(40.0)).unwrap().beats, 7);
}

/// **A format-2 session opens in seconds**, every beat position taken through
/// the tempo map it saved: two beats a second up to beat 4, one after.
#[test]
fn a_format_2_session_is_migrated_through_its_own_tempo_map() {
    use serde_json::json;
    let old = json!({
        "format": 2,
        "multitrack": {
            "tempo": [{"at": 0.0, "bpm": 120.0}, {"at": 4.0, "bpm": 60.0}],
            "markers": [{"id": 9, "at": 8.0}],
            "loop_span": {"start": 0.0, "end": 8.0},
            "tracks": [{
                "id": 1,
                "automation": [{"id": 7, "points": [{"at": 6.0, "value": 1.0}]}],
                "lanes": [{"id": 2, "regions": [{
                    "id": 3, "position": 2.0, "length": 4.0,
                    "fade_in": {"length": 1.0}, "fade_out": {"length": 1.0},
                    "automation": [{"id": 8, "points": [
                        {"at": 0.0, "value": 0.0}, {"at": 4.0, "value": 1.0}]}],
                    "content": {"fill": "window", "window": {
                        "source": {"source": 1, "lifetime": "session"},
                        "start": 0.5, "duration": 3.0}}
                }]}]
            }]
        },
        "views": [{"visible": {"start": 0.0, "end": 8.0}, "quant": 1.0}],
        "sources": {}
    });
    let new = migrate(old);
    assert_eq!(new["format"], FORMAT);
    let m = &new["multitrack"];
    assert_eq!(m["tempo"][0]["tempo"], 2.0, "beats per second now");
    assert!(m["tempo"][0].get("bpm").is_none());
    assert_eq!(m["tempo"][1]["tempo"], 1.0);
    assert_eq!(m["markers"][0]["at"], 6.0, "beat 8: two seconds, then four");
    assert_eq!(m["loop_span"]["end"], 6.0);
    assert_eq!(m["tracks"][0]["automation"][0]["points"][0]["at"], 4.0);
    let region = &m["tracks"][0]["lanes"][0]["regions"][0];
    assert_eq!(region["position"], 1.0);
    assert_eq!(
        region["length"], 3.0,
        "a length is the difference of two positions"
    );
    assert_eq!(region["fade_in"]["length"], 0.5);
    assert_eq!(region["fade_out"]["length"], 1.0);
    assert_eq!(
        region["automation"][0]["points"][1]["at"], 3.0,
        "a region's curve is measured from its own start"
    );
    assert_eq!(
        region["content"]["window"]["start"], 0.5,
        "what fills a region is not the multitrack's axis"
    );
    assert_eq!(new["views"][0]["visible"]["end"], 6.0);
    assert_eq!(new["views"][0]["quant"], 1.0, "a view's grid stays musical");

    let session = Session::read(new.clone()).expect("it reads");
    assert_eq!(session.multitrack.end(), crate::Second(4.0));
    assert_eq!(migrate(new.clone()), new, "a current session is left alone");
}

/// **A session that stated no tempo is migrated at one beat a second**, the
/// default every reader of format 2 drew and played it at.
#[test]
fn a_format_2_session_with_no_tempo_keeps_its_numbers() {
    let old = serde_json::json!({"format": 2, "multitrack": {"markers": [{"id": 1, "at": 3.0}]}});
    let new = migrate(old);
    assert_eq!(new["multitrack"]["markers"][0]["at"], 3.0);
    assert_eq!(new["format"], FORMAT);
}
