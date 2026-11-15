//! The multitrack's session file: what it keeps in the envelope, written and
//! read back as the format has always had it.

use super::*;
use clausters_document::session::FORMAT;
use clausters_document::{Beats, Body, Grouping, Lifetime, Member, Node, NodeId, Range, SourceRef};

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

fn saved() -> MultitrackSession {
    MultitrackSession::new(a_document())
        .with_source(
            SourceId(100),
            Source::file("/home/someone/takes/vocal.wav", Lifetime::External)
                .shaped(1, 48_000, 48_000.0),
        )
        .produced_by(Opaque(serde_json::json!({"script": "song.py"})))
}

fn reopen(session: &MultitrackSession) -> MultitrackSession {
    let written = serde_json::to_string(session).unwrap();
    serde_json::from_str(&written).unwrap()
}

// ---- the multitrack the session carries ----

#[test]
fn a_session_carries_an_arrangement_and_writes_none_when_there_is_none() {
    use clausters_document::multitrack::{Content, Multitrack, Region, Tempo, Track};
    use clausters_document::timebase::{Beat, Second};

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
    track.active_take_lane_mut().unwrap().place(Region::new(
        NodeId(82),
        Second(0.0),
        Second(4.0),
        Content::window(clausters_document::multitrack::Window {
            source: clausters_document::SourceRef {
                source: clausters_document::SourceId(83),
                lifetime: clausters_document::Lifetime::Session,
                generation: 0,
                range: None,
            },
            start: 0.0,
            duration: 4.0,
        }),
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
    use clausters_document::multitrack::Window;
    use clausters_document::multitrack::{Content, Multitrack, Region, Track};
    use clausters_document::timebase::Second;
    use clausters_document::{Lifetime, SourceRef};

    // The table is walked against **both** halves. A reader that checked only
    // the tree would open a session missing exactly what the multitrack plays,
    // and an alternate take counts: it names its source whether or not it is
    // the take lane that plays.
    let window = |source: u64| Window {
        source: SourceRef {
            source: SourceId(source),
            lifetime: Lifetime::Session,
            generation: 0,
            range: None,
        },
        start: 0.0,
        duration: 1.0,
    };
    let mut multitrack = Multitrack::new();
    let mut track = Track::new(NodeId(90), NodeId(91));
    track.active_take_lane_mut().unwrap().place(Region::new(
        NodeId(92),
        Second(0.0),
        Second(4.0),
        Content::window(window(700)),
    ));
    track
        .take_lanes
        .push(clausters_document::multitrack::TakeLane::new(NodeId(93)));
    track.take_lanes[1].place(Region::new(
        NodeId(94),
        Second(0.0),
        Second(4.0),
        Content::window(window(701)),
    ));
    multitrack.tracks.push(track);

    let session = MultitrackSession::new(Document::new(Node::new(
        NodeId(1),
        Body::Clang {
            config: clausters_document::Opaque::none(),
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
    let session: MultitrackSession = serde_json::from_str(json).unwrap();
    assert!(session.contents.contains_key("mixer"));
    let back = serde_json::to_value(&session).unwrap();
    assert_eq!(back["mixer"]["buses"][0]["name"], "reverb");
}

/// **The milestone's acceptance, in one test.** A session with several tracks,
/// alternate take lanes, overlapping layered regions, crossfades and automation
/// round-trips losslessly.
///
/// Written as one multitrack rather than as six assertions because the thing being
/// checked is that they survive *together*: a format can round-trip each of
/// these alone and still lose the layer order when two regions share a beat, or
/// drop the fade on the one that is not on top.
#[test]
fn a_whole_session_round_trips_losslessly() {
    use clausters_document::multitrack::Window;
    use clausters_document::multitrack::{
        Automation, Content, Fade, Marker, Meter, Multitrack, Region, Span, TakeLane, Tempo, Track,
    };
    use clausters_document::timebase::{Beat, Second};
    use clausters_document::{Lifetime, Point, SourceRef};

    let window = |source: u64, from: f64| Window {
        source: SourceRef {
            source: SourceId(source),
            lifetime: Lifetime::Session,
            generation: 0,
            range: None,
        },
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
    vocals.take_lanes[0].name = Some("take 1".into());
    vocals
        .take_lanes
        .push(TakeLane::new(NodeId(12)).named("take 2"));
    vocals
        .take_lanes
        .push(TakeLane::new(NodeId(13)).named("comp"));
    vocals.active = 1;
    for (lane, source) in [(0, 100), (1, 101), (2, 102)] {
        vocals.take_lanes[lane].place(
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
    guitars.take_lanes[0].place(first);
    guitars.take_lanes[0].place(second);
    let mut level = Automation::new(
        NodeId(34),
        clausters_document::Opaque(serde_json::json!({"ctl": "level"})),
    );
    level.points = vec![
        Point {
            at: Second(0.0),
            value: 0.0,
            data: clausters_document::Opaque::none(),
        },
        Point {
            at: Second(16.0),
            value: 1.0,
            data: clausters_document::Opaque(serde_json::json!({"shape": "exp"})),
        },
    ];
    level.visible = true;
    guitars.automation.push(level);
    guitars.soloed = true;

    // A third track, further along the timeline, so the end is its region's.
    let mut sections = Track::new(NodeId(40), NodeId(41)).named("sections");
    sections.take_lanes[0].place(Region::new(
        NodeId(42),
        Second(32.0),
        Second(16.0),
        Content::window(window(202, 0.0)),
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
        a.tracks[0].active_take_lane().unwrap().name.as_deref(),
        Some("take 2")
    );
    assert_eq!(
        a.tracks[0].take_lanes.len(),
        3,
        "the takes nobody chose are kept"
    );
    let guitars = &a.tracks[1].take_lanes[0];
    assert!(guitars.regions[0].overlaps(&guitars.regions[1]));
    assert_eq!(guitars.regions[1].layer, 1, "which one is on top");
    assert_eq!(
        guitars.regions[0].fade_out.as_ref().unwrap().length,
        Second(4.0)
    );
    assert_eq!(a.tracks[1].automation[0].points[1].data.0["shape"], "exp");
    assert_eq!(
        a.tracks[2].take_lanes[0].regions[0].content.source(),
        Some(SourceId(202))
    );
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
    let new = clausters_document::session::migrate(old);
    assert_eq!(new["format"], FORMAT);
    let m = &new["multitrack"];
    assert_eq!(m["tempo"][0]["tempo"], 2.0, "beats per second now");
    assert!(m["tempo"][0].get("bpm").is_none());
    assert_eq!(m["tempo"][1]["tempo"], 1.0);
    assert_eq!(m["markers"][0]["at"], 6.0, "beat 8: two seconds, then four");
    assert_eq!(m["loop_span"]["end"], 6.0);
    assert_eq!(m["tracks"][0]["automation"][0]["points"][0]["at"], 4.0);
    let region = &m["tracks"][0]["take_lanes"][0]["regions"][0];
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

    let session = MultitrackSession::read(new.clone()).expect("it reads");
    assert_eq!(session.multitrack.end(), clausters_document::Second(4.0));
    assert_eq!(
        clausters_document::session::migrate(new.clone()),
        new,
        "a current session is left alone"
    );
}

/// **A session that stated no tempo is migrated at one beat a second**, the
/// default every reader of format 2 drew and played it at.
#[test]
fn a_format_2_session_with_no_tempo_keeps_its_numbers() {
    let old = serde_json::json!({"format": 2, "multitrack": {"markers": [{"id": 1, "at": 3.0}]}});
    let new = clausters_document::session::migrate(old);
    assert_eq!(new["multitrack"]["markers"][0]["at"], 3.0);
    assert_eq!(new["format"], FORMAT);
}

/// **A session says which application it belongs to**: the multitrack's
/// writes its name, a file written before the name reads as the
/// multitrack's, and one another application's metadata names is refused.
#[test]
fn a_session_names_its_application_and_another_s_is_refused() {
    let written = serde_json::to_value(MultitrackSession::new(Document::empty())).unwrap();
    assert_eq!(written["metadata"]["application"], APPLICATION);

    let older = serde_json::json!({"format": FORMAT});
    let session = MultitrackSession::read(older).expect("a file with no metadata reads");
    assert_eq!(session.metadata.application, APPLICATION);

    let other = serde_json::json!({"format": FORMAT, "metadata": {"application": "roll"}});
    let refused = MultitrackSession::read(other).unwrap_err().to_string();
    assert!(refused.contains("roll"), "{refused}");
}
