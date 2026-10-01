use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
use serde_json::{Map, json};

use super::*;
use crate::apply::Endpoint;
use crate::notes_playback::Placed;

const RATE: f64 = 1000.0;

fn ids() -> IdSpaces {
    IdSpaces::new(
        ServerShape {
            max_nodes: 1024,
            audio_buses: 1024,
            outputs: 2,
            control_buses: 16384,
            buffers: 1024,
        },
        IdShare::WHOLE,
    )
}

fn point(at: f64, value: f64) -> Point {
    Point {
        at,
        value,
        data: clausters_document::Opaque::none(),
    }
}

fn keys(value: Value) -> Map<String, Value> {
    serde_json::from_value(value).unwrap()
}

fn note(id: &str, start: f64, channel: i64, curves: Vec<PlacedCurve>) -> Placed {
    Placed {
        start,
        end: start + 1.0,
        keys: keys(json!({"midinote": 60, "sustain": 1.0, "channel": channel})),
        id: id.into(),
        scope: String::new(),
        curves,
    }
}

fn curve(target: Value, points: Vec<Point>) -> PlacedCurve {
    PlacedCurve {
        id: String::new(),
        scope: String::new(),
        target,
        points,
    }
}

/// Three notes -- one with a pressure of its own and one plain on channel 0,
/// one on channel 1 -- and two lanes on channel 0: a cutoff and a bend.
fn placement() -> Placement {
    Placement {
        events: vec![
            note(
                "1",
                0.0,
                0,
                vec![curve(
                    json!({"pressure": true}),
                    vec![point(0.0, 0.0), point(1.5, 1.0)],
                )],
            ),
            note("2", 2.0, 0, Vec::new()),
            note("3", 2.0, 1, Vec::new()),
            Placed {
                start: 1.0,
                end: 1.0,
                keys: keys(json!({"type": "osc", "addr": "/cue"})),
                id: "4".into(),
                scope: String::new(),
                curves: Vec::new(),
            },
        ],
        lanes: vec![
            curve(
                json!({"control": "cutoff", "cc": 74, "channel": 0}),
                vec![point(0.0, 200.0), point(3.0, 2000.0)],
            ),
            curve(
                json!({"bend": true, "channel": 0}),
                vec![point(1.0, 0.0), point(2.0, 2.0)],
            ),
        ],
    }
}

/// **The control a curve drives**: a named control wins, a bend is the
/// pitch, pressure and timbre are theirs, and a bare CC drives nothing.
#[test]
fn a_curve_drives_the_control_its_target_names() {
    assert_eq!(
        curve_control(&json!({"cc": 74, "control": "cutoff"})),
        Some("cutoff".into())
    );
    assert_eq!(curve_control(&json!({"bend": true})), Some(BEND.into()));
    assert_eq!(
        curve_control(&json!({"timbre": true})),
        Some("slide".into())
    );
    assert_eq!(
        curve_control(&json!({"pressure": true})),
        Some("press".into())
    );
    assert_eq!(curve_control(&json!({"cc": 74})), None);
}

/// **A note plays in its channel's graph when a curve reaches it**, its own
/// or the channel's, as a slot of its shape; a note on a channel no curve is
/// on stays a plain synth, and so does everything that is not a note.
#[test]
fn a_note_a_curve_reaches_is_a_slot_of_its_channel() {
    let plan = plan(&placement(), RATE);
    assert_eq!(plan.channels.len(), 1, "channel 1 has no curve");
    let channel = &plan.channels[0];
    assert_eq!(channel.key, "#0");
    let controls: Vec<&str> = channel.lanes.iter().map(|(c, _)| c.as_str()).collect();
    assert_eq!(controls, [BEND, "cutoff"]);
    assert_eq!(
        channel.notes.len(),
        2,
        "one shape with a pressure, one without"
    );

    let first = plan.notes[0].as_ref().unwrap();
    let second = plan.notes[1].as_ref().unwrap();
    assert_ne!(first.slot, second.slot);
    assert!(plan.notes[2].is_none(), "channel 1");
    assert!(plan.notes[3].is_none(), "a marker");

    // The pressure runs from the note's start past its release to its last
    // point, at 1.5 s: 1500 frames, a sample every 64.
    let (control, table) = &first.curves[0];
    assert_eq!(control, "press");
    assert_eq!(table.at, 0.0);
    assert_eq!(
        table.table.len(),
        (1500.0f64 / CURVE_STEP).ceil() as usize + 1
    );
    assert_eq!(*table.table.last().unwrap(), 1.0);

    // The bend lane runs from its first point to its last, on the axis.
    let (_, bend) = &channel.lanes[0];
    assert_eq!(bend.at, 1000.0);
    assert_eq!(*bend.table.last().unwrap(), 2.0);
}

/// **A lane reaches the notes of its own scope**: two boxes over one
/// sequence are two scopes, and a lane of one is not the other's.
#[test]
fn a_lane_reaches_the_notes_of_its_scope_alone() {
    let mut placement = placement();
    for event in &mut placement.events {
        event.scope = "a".into();
        event.curves.clear();
    }
    for lane in &mut placement.lanes {
        lane.scope = "b".into();
    }
    assert!(plan(&placement, RATE).channels.is_empty());
}

/// **What is made follows the plan**: the defs once, an instance per
/// channel, a buffer per table and the channel's readers' ports; the same
/// plan again makes nothing, and a moved curve is a new table whose old one
/// is given back on the next plan.
#[test]
fn the_ops_make_what_the_plan_says_and_no_more() {
    let mut curves = NoteCurves::new("t");
    let first = curves.ops(&plan(&placement(), RATE), "parent", true);
    let defs = first
        .iter()
        .filter(|op| matches!(op, Op::Def { .. }))
        .count();
    assert_eq!(
        defs, 7,
        "the two readers, the hold, the pitch, two notes and the channel"
    );
    assert!(
        first.iter().any(
            |op| matches!(op, Op::Def { spec, .. } if spec["name"] == event_graph::curve_name())
        )
    );
    assert!(first.iter().any(|op| matches!(op, Op::Barrier)));
    assert_eq!(
        first
            .iter()
            .filter(|op| matches!(op, Op::Graph { .. }))
            .count(),
        1
    );
    assert_eq!(
        first
            .iter()
            .filter(|op| matches!(op, Op::Buffer { .. }))
            .count(),
        3,
        "two lanes and a pressure"
    );
    assert!(
        curves
            .ops(&plan(&placement(), RATE), "parent", true)
            .is_empty()
    );

    let mut moved = placement();
    moved.lanes[0].points[1].value = 1000.0;
    let second = curves.ops(&plan(&moved, RATE), "parent", false);
    assert!(matches!(&second[..], [Op::Buffer { .. }, Op::Set { .. }]));
    let third = curves.ops(&plan(&moved, RATE), "parent", false);
    assert!(
        matches!(&third[..], [Op::FreeBuffer { .. }]),
        "given back a plan later"
    );
}

/// **A slot note names its instance, its slot and its readers' ports**, once
/// the ops are applied; a teardown frees the instance and every buffer.
#[test]
fn a_slot_note_carries_its_readers_ports() {
    let mut curves = NoteCurves::new("t");
    let mut applier = Applier::new(Endpoint::default());
    let mut ids = ids();
    let plan = plan(&placement(), RATE);
    let mut ops = vec![Op::Follow {
        handle: "parent".into(),
        transport: 3,
    }];
    ops.extend(curves.ops(&plan, "parent", true));
    applier.apply(ops, &mut ids).unwrap();
    let slots = curves.slots(&plan, &applier);
    let first = slots[0].as_ref().unwrap();
    assert_eq!(Some(first.graph), applier.node("t/channel/#0/0"));
    assert_eq!(first.slot, plan.notes[0].as_ref().unwrap().slot);
    let names: Vec<&str> = first.ports.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["press/buf", "press/step"]);
    assert!(slots[1].as_ref().unwrap().ports.is_empty());
    assert!(slots[2].is_none());

    let down = curves.teardown();
    assert!(matches!(down[0], Op::Free { .. }));
    assert_eq!(
        down.iter()
            .filter(|op| matches!(op, Op::FreeBuffer { .. }))
            .count(),
        3
    );
}

/// **A note's old table outlives the edit until the next pass**: a note
/// sounding from before the edit reads it until it ends, and a buffer number
/// handed out again at once would give it another curve.
#[test]
fn a_notes_old_table_is_given_back_on_the_next_pass() {
    let mut curves = NoteCurves::new("t");
    curves.ops(&plan(&placement(), RATE), "parent", true);
    let mut moved = placement();
    moved.events[0].curves[0].points[1].value = 0.5;
    let edit = curves.ops(&plan(&moved, RATE), "parent", false);
    assert!(
        matches!(&edit[..], [Op::Buffer { .. }]),
        "the new table only"
    );
    assert!(curves.ops(&plan(&moved, RATE), "parent", false).is_empty());
    let pass = curves.ops(&plan(&moved, RATE), "parent", true);
    assert!(
        matches!(&pass[..], [Op::FreeBuffer { .. }]),
        "given back on a pass"
    );
}

/// **A curve added while the notes sound does not cut them**: the channel's
/// graph changes, so its new instance is made beside the old one, which keeps
/// the notes it is sounding and the tables they read -- a lane's moved table
/// waits with them -- and both are given back on the next pass.
#[test]
fn a_channel_made_again_keeps_its_old_instance_until_the_next_pass() {
    let mut curves = NoteCurves::new("t");
    curves.ops(&plan(&placement(), RATE), "parent", true);
    let mut grown = placement();
    grown.lanes[0].points[1].value = 1000.0;
    grown.lanes.push(curve(
        json!({"timbre": true, "channel": 0}),
        vec![point(0.0, 0.0), point(1.0, 1.0)],
    ));
    let edit = curves.ops(&plan(&grown, RATE), "parent", false);
    assert!(
        !edit.iter().any(|op| matches!(op, Op::Free { .. })),
        "the old instance is not freed"
    );
    assert!(
        edit.iter()
            .any(|op| matches!(op, Op::Graph { handle, .. } if handle == "t/channel/#0/1"))
    );
    let next = curves.ops(&plan(&grown, RATE), "parent", false);
    assert!(
        next.is_empty(),
        "the cutoff's old table waits with the instance that reads it"
    );
    let pass = curves.ops(&plan(&grown, RATE), "parent", true);
    assert!(matches!(
        &pass[..],
        [Op::Free { handle, .. }, Op::FreeBuffer { .. }] if handle == "t/channel/#0/0"
    ));
}

/// **A channel no curve reaches any more outlives its curves too**: its notes
/// play as plain synths from then on, and the ones it was sounding finish in
/// it until the next pass.
#[test]
fn a_channel_whose_curves_go_finishes_its_notes() {
    let mut curves = NoteCurves::new("t");
    curves.ops(&plan(&placement(), RATE), "parent", true);
    let mut bare = placement();
    bare.lanes.clear();
    bare.events[0].curves.clear();
    let edit = curves.ops(&plan(&bare, RATE), "parent", false);
    assert!(edit.is_empty(), "nothing freed under the notes");
    let pass = curves.ops(&plan(&bare, RATE), "parent", true);
    assert!(matches!(&pass[0], Op::Free { handle, .. } if handle == "t/channel/#0/0"));
    assert_eq!(
        pass.iter()
            .filter(|op| matches!(op, Op::FreeBuffer { .. }))
            .count(),
        3
    );
}
