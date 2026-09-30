//! **The graphs a sequence's notes play in**, when curves shape them.
//!
//! A control acts on a whole channel or on one note, as MIDI 2.0 has it and as
//! a multitrack hangs one automation on a track or on a clip. The two scopes
//! are two graphs, one inside the other:
//!
//! - **A channel** is an instance of [`channel_graph`]: a private control bus
//!   per curve over the channel (a sequence's lane), each written by a reader
//!   of the curve's table on the transport's position ([`curve_def`]), shared
//!   as the channel is. Its notes are its slots.
//! - **A note** is one of those slots, an instance of [`note_graph`]: the def
//!   the note names, beside the readers of the note's own curves on buses of
//!   its own, and -- when either scope bends it -- a node that makes its pitch
//!   ([`pitch_def`]). The voice is marked `ends`, so the slot is freed with it:
//!   the readers do not outlive what they shape.
//!
//! **How the two combine**, one control at a time: a bend is the channel's plus
//! the note's, in semitones, as MIDI 2.0's per-note bend and MPE's master
//! channel add; any other control reads the note's curve when the note has
//! one, else the channel's. A control neither scope names keeps the value the
//! note starts with.
//!
//! A slot's members are fixed by its def, so a channel declares one slot per
//! **shape** of note it holds ([`Shape`]: the def, the controls it is started
//! with, its own curves, the channel's), and the graphs are named by what they
//! hold ([`note_name`], [`channel_name`]) -- the same content is the same def.

use serde_json::{Map, Value, json};

use crate::mixer::{AT, BUF, CURVE_STEP, OUT_BUS};

/// What every name here begins with.
pub const PREFIX: &str = "ev";

/// The control a bend drives: not a control of the voice, but its pitch.
pub const BEND: &str = "bend";

/// The voice's control a bend reaches, through [`pitch_def`].
pub const FREQ: &str = "freq";

/// The voice's control a note is released through, which a note graph always
/// answers: a lane releases a slot by its `gate` port.
pub const GATE: &str = "gate";

/// How long a note curve's reader glides to a new value, in seconds.
///
/// A curve read per block steps once a block, and a lane edited under the play
/// line steps by whatever the edit moved; a voice's control is the user's def
/// and smooths neither, so the reader does -- over about as long as a
/// multitrack strip's controls take.
pub const CURVE_LAG: f32 = 0.01;

/// The name of a note curve's reader.
pub fn curve_name() -> String {
    format!("{PREFIX}.curve")
}

/// **A note curve's reader**: a table read at the transport's position, as a
/// multitrack's automation is (`crate::mixer::curve_def`), onto the control
/// bus `out` -- **held while the transport is stopped**, and glided over
/// [`CURVE_LAG`].
///
/// A multitrack's readers are frozen with the governed group on a stop. A
/// note's cannot be: its voice goes on sounding its release in the group that
/// follows the transport, and the readers are in its graph beside it. So the
/// hold is here: a stop, and the locate back to the cursor after it, would
/// otherwise move every curve a releasing note reads to the value at the
/// cursor -- a step in its level, a jump in its pitch -- where MIDI holds a
/// channel's last value while nothing plays.
pub fn curve_def() -> Value {
    json!({
        "name": curve_name(),
        "controls": [
            {"name": OUT_BUS, "default": 0.0},
            {"name": BUF, "default": 0.0},
            {"name": AT, "default": 0.0},
            {"name": "step", "default": CURVE_STEP},
        ],
        "ugens": [
            // 0..2: the table at the transport's position, as a multitrack reads it.
            {"kind": "TransportPos", "inputs": [{"control": 2}]},
            {"kind": "Div", "inputs": [{"ugen": 0}, {"control": 3}]},
            {"kind": "BufRd", "inputs": [
                {"control": 1}, {"const": 0.0}, {"ugen": 1}, {"const": 0.0}
            ]},
            // 3..4: passed while the transport rolls, held while it is stopped.
            {"kind": "TransportFade", "rate": "kr", "inputs": []},
            {"kind": "Gate", "rate": "kr", "inputs": [{"ugen": 2}, {"ugen": 3}]},
            // 5..6: glided, onto the bus.
            {"kind": "Lag", "rate": "kr", "inputs": [{"ugen": 4}, {"const": CURVE_LAG}]},
            {"kind": "OutCtl", "inputs": [{"control": 0}, {"ugen": 5}]}
        ]
    })
}

/// The name of the node that makes a note's pitch.
pub fn pitch_name() -> String {
    format!("{PREFIX}.pitch")
}

/// **A note's pitch**, onto the control bus `out`: `base` (the frequency the
/// note is started at) raised by `lane` plus `note` semitones -- the channel's
/// bend and the note's own, each mapped from the bus its reader writes.
pub fn pitch_def() -> Value {
    json!({
        "name": pitch_name(),
        "controls": [
            {"name": OUT_BUS, "default": 0.0},
            {"name": "base", "default": 440.0},
            {"name": "lane", "default": 0.0},
            {"name": "note", "default": 0.0},
        ],
        "ugens": [
            {"kind": "BinaryOpUGen", "op": "add", "rate": "kr",
             "inputs": [{"control": 2}, {"control": 3}]},
            {"kind": "UnaryOpUGen", "op": "midiratio", "rate": "kr",
             "inputs": [{"ugen": 0}]},
            {"kind": "BinaryOpUGen", "op": "mul", "rate": "kr",
             "inputs": [{"control": 1}, {"ugen": 1}]},
            {"kind": "OutCtl", "inputs": [{"control": 0}, {"ugen": 2}]}
        ]
    })
}

/// **What a note's graph is made of**: the def it plays, the controls it is
/// started with (its ports), the controls its own curves drive and the ones
/// the channel's curves drive. [`BEND`] among either is a bend.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Shape {
    pub def: String,
    pub controls: Vec<String>,
    pub own: Vec<String>,
    pub lanes: Vec<String>,
}

impl Shape {
    /// Whether either scope bends it, and the note has a frequency to bend.
    pub fn bends(&self) -> bool {
        self.controls.iter().any(|c| c == FREQ)
            && (self.own.iter().any(|c| c == BEND) || self.lanes.iter().any(|c| c == BEND))
    }
}

/// The bus a channel's curve over `control` writes.
pub fn lane_bus(control: &str) -> String {
    format!("lane/{control}")
}

/// The bus a note's own curve over `control` writes.
fn note_bus(control: &str) -> String {
    format!("note/{control}")
}

/// The port a curve's table buffer is handed through: `<control>/buf`, and
/// `<control>/at` and `<control>/step` beside it.
pub fn curve_port(control: &str, what: &str) -> String {
    format!("{control}/{what}")
}

/// The port of a channel's curve over `control`: `lane/<control>/<what>`.
pub fn lane_port(control: &str, what: &str) -> String {
    format!("lane/{control}/{what}")
}

/// A reader of one curve's table, onto `bus`.
fn reader(bus: &str) -> Value {
    json!({"def": curve_name(), "controls": {OUT_BUS: bus, "step": CURVE_STEP}})
}

/// The three ports of the reader at `member`, named by `port`.
fn reader_ports(surface: &mut Map<String, Value>, member: usize, port: impl Fn(&str) -> String) {
    for control in [BUF, AT, "step"] {
        surface.insert(
            port(control),
            json!([{"member": member, "control": control}]),
        );
    }
}

/// **A note's graph**, as [`Shape`] says: the readers of its own curves, the
/// pitch node when it is bent, then the voice (`ends`), each of whose controls
/// reads the note's curve, else the channel's, else keeps its port's value.
///
/// The voice comes **last**: a control bus is written by whatever runs first in
/// the block, and a group sorts by audio buses alone, so the order the members
/// are listed in is the order they run -- the readers first, and the voice
/// reading what they wrote in the same block.
pub fn note_graph(shape: &Shape) -> Value {
    let mut buses: Vec<Value> = shape
        .lanes
        .iter()
        .map(|c| json!({"name": lane_bus(c), "rate": "control", "external": true}))
        .collect();
    let mut members = Vec::new();
    let mut surface = Map::new();
    for control in &shape.own {
        buses.push(json!({"name": note_bus(control), "rate": "control"}));
        reader_ports(&mut surface, members.len(), |what| {
            curve_port(control, what)
        });
        members.push(reader(&note_bus(control)));
    }
    let mut maps = Map::new();
    let bends = shape.bends();
    if bends {
        buses.push(json!({"name": "pitch", "rate": "control"}));
        let mut pitch_maps = Map::new();
        if shape.lanes.iter().any(|c| c == BEND) {
            pitch_maps.insert("lane".into(), json!(lane_bus(BEND)));
        }
        if shape.own.iter().any(|c| c == BEND) {
            pitch_maps.insert("note".into(), json!(note_bus(BEND)));
        }
        surface.insert(
            FREQ.into(),
            json!([{"member": members.len(), "control": "base"}]),
        );
        members.push(json!({
            "def": pitch_name(),
            "controls": {OUT_BUS: "pitch"},
            "maps": pitch_maps,
        }));
        maps.insert(FREQ.into(), json!("pitch"));
    }
    for control in shape.lanes.iter().filter(|c| *c != BEND) {
        maps.insert(control.clone(), json!(lane_bus(control)));
    }
    for control in shape.own.iter().filter(|c| *c != BEND) {
        maps.insert(control.clone(), json!(note_bus(control)));
    }
    let voice = members.len();
    // **The release is a port too.** A note is started with the controls its
    // keys name, and `gate` is seldom one of them -- a def's own default opens
    // it -- yet the lane closes it through the slot's surface, where a port
    // the surface does not have is ignored and the note would hang.
    surface.insert(GATE.into(), json!([{"member": voice, "control": GATE}]));
    // **A control a curve drives is not a port.** Setting a mapped control
    // unmaps it, so the value a note starts with -- its `amp`, written by
    // its author -- would take the control back from the curve the moment the
    // slot is made. The curve is what the control is while the note sounds.
    for control in &shape.controls {
        if bends && control == FREQ {
            continue;
        }
        if !maps.contains_key(control) {
            surface.insert(
                control.clone(),
                json!([{"member": voice, "control": control}]),
            );
        }
    }
    members.push(json!({"def": shape.def, "maps": maps, "ends": true}));
    let mut graph = json!({
        "buses": buses,
        "members": members,
        "surface": surface,
    });
    graph["name"] = json!(note_name(&graph));
    graph
}

/// **A channel's graph**: a reader per curve over the channel, onto a bus of
/// its own, and a slot per shape of note -- each a [`note_graph`], handed the
/// channel's buses. The slots are named `note.0`, `note.1`, ... in the order of
/// `notes`.
pub fn channel_graph(lanes: &[String], notes: &[Value]) -> Value {
    let buses: Vec<Value> = lanes
        .iter()
        .map(|c| json!({"name": lane_bus(c), "rate": "control"}))
        .collect();
    let mut members = Vec::new();
    let mut surface = Map::new();
    for control in lanes {
        reader_ports(&mut surface, members.len(), |what| lane_port(control, what));
        members.push(reader(&lane_bus(control)));
    }
    let handed: Map<String, Value> = lanes
        .iter()
        .map(|c| (lane_bus(c), json!(lane_bus(c))))
        .collect();
    for (i, note) in notes.iter().enumerate() {
        members.push(json!({
            "def": note["name"],
            "kind": "graph",
            "slot": slot_name(i),
            "controls": handed,
        }));
    }
    let mut graph = json!({
        "buses": buses,
        "members": members,
        "surface": surface,
    });
    graph["name"] = json!(channel_name(&graph));
    graph
}

/// The slot a channel's `i`th shape of note is.
pub fn slot_name(i: usize) -> String {
    format!("note.{i}")
}

/// A note graph's name: what it holds, hashed.
fn note_name(graph: &Value) -> String {
    format!("{PREFIX}.note.{:016x}", fnv(&graph.to_string()))
}

/// A channel graph's name: what it holds, hashed.
fn channel_name(graph: &Value) -> String {
    format!("{PREFIX}.channel.{:016x}", fnv(&graph.to_string()))
}

/// FNV-1a over the text: a name that is the same for the same content in every
/// client and every run, which is all a def's name has to be here.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(own: &[&str], lanes: &[&str]) -> Shape {
        Shape {
            def: "default".into(),
            controls: vec!["amp".into(), FREQ.into(), "pressure".into()],
            own: own.iter().map(|s| s.to_string()).collect(),
            lanes: lanes.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// **The voice reads the note's curve, else the channel's**, and comes
    /// after everything that writes what it reads.
    #[test]
    fn a_note_reads_its_own_curve_before_the_channels() {
        let graph = note_graph(&shape(&["pressure"], &["pressure", "cutoff"]));
        let members = graph["members"].as_array().unwrap();
        let voice = members.last().unwrap();
        assert_eq!(voice["def"], "default");
        assert_eq!(voice["ends"], true);
        assert_eq!(voice["maps"]["pressure"], "note/pressure");
        assert_eq!(voice["maps"]["cutoff"], "lane/cutoff");
        assert!(
            graph["surface"]["pressure"].is_null(),
            "a port would unmap the curve"
        );
        assert_eq!(members[0]["def"], curve_name());
        assert_eq!(graph["surface"]["pressure/buf"][0]["member"], 0);
        assert_eq!(graph["surface"]["amp"][0]["member"], members.len() - 1);
        assert_eq!(
            graph["surface"][GATE][0]["member"],
            members.len() - 1,
            "the lane releases the voice through it"
        );
    }

    /// **A bend is the channel's plus the note's**, made into the voice's
    /// frequency by the pitch node, which the `freq` port starts.
    #[test]
    fn a_bend_in_either_scope_makes_the_pitch() {
        let graph = note_graph(&shape(&[BEND], &[BEND]));
        let members = graph["members"].as_array().unwrap();
        let pitch = &members[1];
        assert_eq!(pitch["def"], pitch_name());
        assert_eq!(pitch["maps"]["lane"], "lane/bend");
        assert_eq!(pitch["maps"]["note"], "note/bend");
        assert_eq!(members[2]["maps"][FREQ], "pitch");
        assert_eq!(graph["surface"][FREQ][0]["member"], 1);
        assert_eq!(graph["surface"][FREQ][0]["control"], "base");
        assert!(!note_graph(&shape(&[], &[]))["surface"][FREQ].is_null());
    }

    /// **A channel reads each of its curves once**, and hands their buses to
    /// every slot; the same content is the same name.
    #[test]
    fn a_channel_shares_its_curves_with_its_slots() {
        let lanes = vec![BEND.to_string()];
        let note = note_graph(&shape(
            &[],
            &lanes.iter().map(String::as_str).collect::<Vec<_>>(),
        ));
        let graph = channel_graph(&lanes, std::slice::from_ref(&note));
        let members = graph["members"].as_array().unwrap();
        assert_eq!(members[0]["controls"][OUT_BUS], "lane/bend");
        assert_eq!(members[1]["slot"], "note.0");
        assert_eq!(members[1]["def"], note["name"]);
        assert_eq!(members[1]["controls"]["lane/bend"], "lane/bend");
        assert!(graph["surface"]["lane/bend/buf"].is_array());
        assert_eq!(graph["name"], channel_graph(&lanes, &[note])["name"]);
    }
}
