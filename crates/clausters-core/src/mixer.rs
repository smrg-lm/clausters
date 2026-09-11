//! **What a piece is as nodes and groups** — the multitrack's node system,
//! written once.
//!
//! An automation whose target says `gain` has to drive the *same* `gain` the
//! track's knob shows, and until something says what a track and a clip **are**
//! on the server those are two words that happen to be spelled alike. This
//! module is that something: the defs a piece is made of, and the names their
//! surfaces answer to.
//!
//! It is here, and not in a client, for the reason [`crate::patch`] is here —
//! every client that plays a piece needs the identical wiring, and a wiring
//! written twice is two answers waiting to differ. A caller states the facts
//! (how many channels a track has, which buffer a box reads) and gets the
//! specs; the caller sends them, stamps the ids and instantiates, because ids
//! are the caller's and nothing here knows a node's number.
//!
//! # The shape, and why it is the field's
//!
//! Every fixed-channel DAW has the same channel strip, and the order is not
//! arbitrary — it is what makes gain staging, sends and metering mean what
//! people expect:
//!
//! ```text
//! in -> [pre-fader inserts] -> fader (+ mute) -> pan/width -> [post inserts]
//!    -> sends -> out
//! ```
//!
//! A **clip** is that strip over readers; a **track** is that strip over clips;
//! the **master** is that strip over tracks. Three instances of one idea, which
//! is why they are three GraphDefs over one [`strip_def`] rather than three
//! hand-wired graphs. A clip's gain and a track's gain are *both* real and they
//! are different stages: the first corrects the take, the second mixes it.
//!
//! # What is deliberately not here yet
//!
//! **Effects**: `mt.track` and `mt.clip` declare the slot and leave it empty,
//! because a send to nowhere and an insert of nothing cannot be heard and so
//! cannot be checked. The slot is a nested-graph slot, so an effect may itself
//! be a GraphDef and a compound effect needs no new mechanism.
//!
//! **The meter**: it wants an instant attack, a declared decay in dB/s and a
//! peak hold, and no UGen here does that yet — a `Lag` decays exponentially,
//! which is not the same picture. The def is the meter's, not the strip's, so
//! adding it later does not disturb the strip.
//!
//! **Widths past stereo**: [`strip_def`] is written for 1 and 2 channels, which
//! is what a track declares today. The general N→M downmix (BS.775 and its
//! relatives) is named where the widths are checked and refused.

use serde_json::{Value, json};

/// The prefix every def here is named with. Deliberately not `track` or `clip`
/// on their own: a def name is global, and those two words are the most
/// ambiguous ones available.
pub const PREFIX: &str = "mt";

// ---- the port vocabulary ----
//
// A port is a path. These are the names an `Automation`'s target resolves
// against, the names a header knob writes, and the names a `/node_set` uses --
// one set, which is the whole point of the module.

/// Linear gain. The fader of whatever strip it is on.
pub const GAIN: &str = "gain";
/// Position, `-1..1`. A **pan** over a mono source (an equal-power law that
/// centres at -3 dB) and a **balance** over a stereo one (which attenuates one
/// side): the same word for two laws, because they are the same gesture and
/// confusing them is the classic mixer bug -- so which one applies follows from
/// the source's width and never from a second name.
pub const PAN: &str = "pan";
/// Stereo width, `0..2`. `1` leaves the image alone; `0` collapses it to mono.
pub const WIDTH: &str = "width";
/// Silenced: `0` or `1`, and it is a **control of the fader**, not a state of
/// the group. A mute is automated like every other curve, a hard cut between
/// two samples is a click, and stopping the group would take the tail of an
/// effect and the pre-fader sends with it. Turning a silent group off is a
/// separate question -- one about cost, not about sound.
pub const MUTE: &str = "mute";
/// Where a box starts on the transport, in frames.
pub const AT: &str = "at";
/// How long a box lasts, in frames.
pub const SPAN: &str = "span";
/// The first frame of the source a box reads.
pub const START: &str = "start";
/// The buffer a box reads. Initial-rate: changing it is a new node, which is
/// why a box that is a join is one stitched buffer rather than several readers.
pub const BUF: &str = "buf";
/// Which channel of the source a reader takes.
pub const CHAN: &str = "chan";

/// The slot a clip's readers fill: one per channel of the source.
pub const SOURCE_SLOT: &str = "source";
/// The slot a track's clips of a **mono** source fill. There is one clip slot
/// per source width, because a slot names one def and a mono take and a stereo
/// take are not the same wiring: the first is panned into the track and the
/// second is balanced. Which slot a box goes in is the source's width, which is
/// the one fact about the box that decides it.
pub fn clip_slot(inputs: usize) -> String {
    format!("clips.{inputs}")
}
/// The slot a piece's tracks fill.
pub const TRACK_SLOT: &str = "tracks";
/// The slot an effect chain fills. Declared and empty -- see the module docs.
pub const FX_SLOT: &str = "fx";

/// The bus a nested graph is handed by whoever instantiates it.
pub const OUT_BUS: &str = "out";
/// The bus a strip mixes its sources on, private to each instance.
pub const MIX_BUS: &str = "mix";

/// The widths a strip is written for. Past stereo is a downmix table and a
/// decision about which one, and it is refused rather than guessed.
pub const MAX_CHANNELS: usize = 2;

/// The name of the reader def: what one channel of one box sounds as.
pub fn reader_name() -> String {
    format!("{PREFIX}.reader")
}

/// The name of the strip def for `inputs` channels in and `outputs` out.
pub fn strip_name(inputs: usize, outputs: usize) -> String {
    format!("{PREFIX}.strip.{inputs}x{outputs}")
}

/// The name of the clip graph for a source of `inputs` channels on a track of
/// `outputs`.
pub fn clip_name(inputs: usize, outputs: usize) -> String {
    format!("{PREFIX}.clip.{inputs}x{outputs}")
}

/// The name of the track graph for a track of `channels`.
pub fn track_name(channels: usize) -> String {
    format!("{PREFIX}.track.{channels}")
}

/// The name of the piece graph for a piece of `channels`.
pub fn piece_name(channels: usize) -> String {
    format!("{PREFIX}.piece.{channels}")
}

/// Refuses a width this module has no wiring for, saying which it has.
fn check(channels: usize, what: &str) -> Result<(), String> {
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(format!(
            "{what}: {channels} channels; the strip is written for 1 and {MAX_CHANNELS} \
             (a wider one is a downmix table, and which table is a decision)"
        ));
    }
    Ok(())
}

// ---- the defs ----

fn control(name: &str, default: f32) -> Value {
    json!({ "name": name, "default": default })
}

fn ir(name: &str, default: f32) -> Value {
    json!({ "name": name, "default": default, "rate": "ir" })
}

fn lagged(name: &str, default: f32) -> Value {
    // Ten milliseconds: long enough that a jump between two samples is a ramp
    // rather than a click, short enough that a hand does not hear it.
    json!({ "name": name, "default": default, "lag": 0.01 })
}

/// **One channel of one box**: a reader that plays a span of a buffer at the
/// place on the transport the box sits, and silence everywhere else.
///
/// It is *resident*: it is created when the box is, and it stays. Where the
/// piece is playing from is `TransportPos`'s to say, so moving the box is a
/// `/node_set` of `at` and a locate is no message at all -- the reader is
/// wherever the transport says it is. That is the whole reason a piece plays
/// itself rather than being driven.
///
/// The window is a gate rather than a schedule for the same reason: `at` and
/// `span` are read every block, so an edit while it sounds lands on the next
/// block and nothing has to be re-armed.
pub fn reader_def() -> Value {
    json!({
        "name": reader_name(),
        "controls": [
            control(OUT_BUS, 0.0),
            ir(BUF, 0.0),
            ir(CHAN, 0.0),
            control(AT, 0.0),
            control(SPAN, 0.0),
            control(START, 0.0),
            ir("loop", 0.0),
            lagged(GAIN, 1.0),
        ],
        "ugens": [
            // 0: frames since this box began; negative before it starts.
            {"kind": "TransportPos", "inputs": [{"control": 3}]},
            // 1..3: inside the window?
            {"kind": "BinaryOpUGen", "op": "ge", "inputs": [{"ugen": 0}, {"const": 0.0}]},
            {"kind": "BinaryOpUGen", "op": "lt", "inputs": [{"ugen": 0}, {"control": 4}]},
            {"kind": "Mul", "inputs": [{"ugen": 1}, {"ugen": 2}]},
            // 4..5: the frame of the source that is, read interpolated.
            {"kind": "Add", "inputs": [{"ugen": 0}, {"control": 5}]},
            {"kind": "BufRd", "inputs": [
                {"control": 1}, {"control": 2}, {"ugen": 4}, {"control": 6}
            ]},
            // 6..8: gated, levelled, out.
            {"kind": "Mul", "inputs": [{"ugen": 5}, {"ugen": 3}]},
            {"kind": "Mul", "inputs": [{"ugen": 6}, {"control": 7}]},
            {"kind": "Out", "inputs": [{"control": 0}, {"ugen": 7}]}
        ]
    })
}

/// **The channel strip**: `inputs` channels in, gain and mute, then the image,
/// then `outputs` channels out.
///
/// One def for the clip, the track and the master, because they are one shape.
/// `mute` multiplies rather than stopping anything (see [`MUTE`]), and `pan`
/// is a pan law over a mono input and a balance over a stereo one (see
/// [`PAN`]) -- so a mono take on a stereo track is centred at -3 dB a side,
/// which is what every mixer does and what naming one channel would not.
///
/// Each channel is its own `Out`, because a UGen has one output; the bus
/// indices come in as separate controls (`out0`, `out1`), which is what
/// `"mix:1"` in a GraphDef exists to fill.
pub fn strip_def(inputs: usize, outputs: usize) -> Result<Value, String> {
    check(inputs, "strip inputs")?;
    check(outputs, "strip outputs")?;
    let mut controls = vec![control("in0", 0.0), control("in1", 0.0)];
    controls.push(control("out0", 0.0));
    controls.push(control("out1", 0.0));
    controls.push(lagged(GAIN, 1.0));
    controls.push(lagged(MUTE, 0.0));
    controls.push(lagged(PAN, 0.0));
    controls.push(lagged(WIDTH, 1.0));
    // Control indices, by the order above.
    const IN0: u32 = 0;
    const IN1: u32 = 1;
    const OUT0: u32 = 2;
    const OUT1: u32 = 3;
    const GAIN_C: u32 = 4;
    const MUTE_C: u32 = 5;
    const PAN_C: u32 = 6;
    const WIDTH_C: u32 = 7;

    let mut ugens: Vec<Value> = Vec::new();
    let push = |u: Value, ugens: &mut Vec<Value>| -> u32 {
        ugens.push(u);
        (ugens.len() - 1) as u32
    };

    // The fader: gain, then mute as `1 - mute` so it is a control like any
    // other and rides the same lag.
    let open = push(
        json!({"kind": "BinaryOpUGen", "op": "sub",
               "inputs": [{"const": 1.0}, {"control": MUTE_C}]}),
        &mut ugens,
    );
    let level = push(
        json!({"kind": "Mul", "inputs": [{"ugen": open}, {"control": GAIN_C}]}),
        &mut ugens,
    );

    let read0 = push(
        json!({"kind": "In", "inputs": [{"control": IN0}]}),
        &mut ugens,
    );
    let left_in = push(
        json!({"kind": "Mul", "inputs": [{"ugen": read0}, {"ugen": level}]}),
        &mut ugens,
    );
    let right_in = if inputs == 2 {
        let read1 = push(
            json!({"kind": "In", "inputs": [{"control": IN1}]}),
            &mut ugens,
        );
        Some(push(
            json!({"kind": "Mul", "inputs": [{"ugen": read1}, {"ugen": level}]}),
            &mut ugens,
        ))
    } else {
        None
    };

    let (left, right) = match (inputs, outputs, right_in) {
        // Mono in, stereo out: a **pan**, with the law. Not the same signal
        // twice -- that is 3 dB too loud in the middle and is the mistake this
        // names on purpose.
        (1, 2, _) => {
            let l = push(
                json!({"kind": "Pan2", "inputs": [
                    {"ugen": left_in}, {"control": PAN_C}, {"const": 1.0}, {"const": 0.0}]}),
                &mut ugens,
            );
            let r = push(
                json!({"kind": "Pan2", "inputs": [
                    {"ugen": left_in}, {"control": PAN_C}, {"const": 1.0}, {"const": 1.0}]}),
                &mut ugens,
            );
            (l, Some(r))
        }
        // Stereo in, stereo out: a **balance**, then the width.
        //
        // A balance *attenuates one side* and leaves the centre alone, which is
        // what `Balance2` is not: that applies the equal-power pan law to a
        // stereo pair, so a centred strip comes out 3 dB down and three strips
        // in series -- clip, track, master -- take 9 dB off a piece for
        // nothing. So the law is written here: `min(1, 1 ∓ pan)`, unity at the
        // centre and silence at the far end.
        (2, 2, Some(right_in)) => {
            let lg = push(
                json!({"kind": "BinaryOpUGen", "op": "sub",
                       "inputs": [{"const": 1.0}, {"control": PAN_C}]}),
                &mut ugens,
            );
            let lg = push(
                json!({"kind": "BinaryOpUGen", "op": "min",
                       "inputs": [{"ugen": lg}, {"const": 1.0}]}),
                &mut ugens,
            );
            let rg = push(
                json!({"kind": "Add", "inputs": [{"const": 1.0}, {"control": PAN_C}]}),
                &mut ugens,
            );
            let rg = push(
                json!({"kind": "BinaryOpUGen", "op": "min",
                       "inputs": [{"ugen": rg}, {"const": 1.0}]}),
                &mut ugens,
            );
            let bl = push(
                json!({"kind": "Mul", "inputs": [{"ugen": left_in}, {"ugen": lg}]}),
                &mut ugens,
            );
            let br = push(
                json!({"kind": "Mul", "inputs": [{"ugen": right_in}, {"ugen": rg}]}),
                &mut ugens,
            );
            let wl = push(
                json!({"kind": "StereoWidth", "inputs": [
                    {"ugen": bl}, {"ugen": br}, {"control": WIDTH_C}, {"const": 0.0}]}),
                &mut ugens,
            );
            let wr = push(
                json!({"kind": "StereoWidth", "inputs": [
                    {"ugen": bl}, {"ugen": br}, {"control": WIDTH_C}, {"const": 1.0}]}),
                &mut ugens,
            );
            (wl, Some(wr))
        }
        // Stereo in, mono out: the sum, which is the honest downmix at this
        // width and the only one anybody agrees on.
        (2, 1, Some(right_in)) => {
            let sum = push(
                json!({"kind": "Add", "inputs": [{"ugen": left_in}, {"ugen": right_in}]}),
                &mut ugens,
            );
            (sum, None)
        }
        // Mono in, mono out: the signal.
        (1, 1, _) => (left_in, None),
        _ => unreachable!("check() refused every other width"),
    };

    ugens.push(json!({"kind": "Out", "inputs": [{"control": OUT0}, {"ugen": left}]}));
    if outputs == 2 {
        let right = right.expect("a stereo output has a right channel");
        ugens.push(json!({"kind": "Out", "inputs": [{"control": OUT1}, {"ugen": right}]}));
    }

    Ok(json!({ "name": strip_name(inputs, outputs), "controls": controls, "ugens": ugens }))
}

/// The buses a strip's `in`/`out` controls are wired to, given a bus name and a
/// width. Channel 1 of a mono bus does not exist, so it is left at 0 -- the def
/// never reads it.
fn strip_wiring(input: &str, inputs: usize, output: &str, outputs: usize) -> Value {
    json!({
        "in0": format!("{input}:0"),
        "in1": if inputs == 2 { format!("{input}:1") } else { format!("{input}:0") },
        "out0": format!("{output}:0"),
        "out1": if outputs == 2 { format!("{output}:1") } else { format!("{output}:0") },
    })
}

/// **A clip**: readers onto a private bus, then a strip onto the bus the track
/// hands it.
///
/// The readers are a **slot** because a source has as many channels as it has
/// and a box may be re-cut while it sounds; the effect chain is a slot for the
/// same reason and is empty today. The clip does not decide where it goes: `out`
/// is external, so one clip def is every clip on every track.
pub fn clip_graph(inputs: usize, outputs: usize) -> Result<Value, String> {
    check(inputs, "clip inputs")?;
    check(outputs, "clip outputs")?;
    Ok(json!({
        "name": clip_name(inputs, outputs),
        "buses": [
            {"name": "src", "rate": "audio", "channels": inputs},
            {"name": OUT_BUS, "rate": "audio", "channels": outputs, "external": true},
        ],
        "members": [
            // 0: the strip. Shared: one per clip, however many readers there are.
            {"def": strip_name(inputs, outputs),
             "controls": strip_wiring("src", inputs, OUT_BUS, outputs)},
            // 1: one reader per channel of the source.
            {"def": reader_name(), "slot": SOURCE_SLOT,
             "controls": {OUT_BUS: "src"}},
        ],
        "surface": {
            GAIN:  [{"member": 0, "control": GAIN}],
            PAN:   [{"member": 0, "control": PAN}],
            WIDTH: [{"member": 0, "control": WIDTH}],
            MUTE:  [{"member": 0, "control": MUTE}],
            // The reader's own, which is the slot's surface rather than the
            // clip's: a box with two channels has two of each.
            AT:    [{"member": 1, "control": AT}],
            SPAN:  [{"member": 1, "control": SPAN}],
            START: [{"member": 1, "control": START}],
            "source/gain": [{"member": 1, "control": GAIN}],
        },
        "defaults": { GAIN: 1.0, WIDTH: 1.0 }
    }))
}

/// **A track**: clips onto a private mix bus, then a strip onto the bus the
/// master hands it.
///
/// The clips are a slot of **nested graphs**, which is what a clip being a
/// thing with its own gain, its own image and its own chain amounts to; the
/// track's own effects are the same slot shape and are empty today.
pub fn track_graph(channels: usize) -> Result<Value, String> {
    check(channels, "track")?;
    Ok(json!({
        "name": track_name(channels),
        "buses": [
            {"name": MIX_BUS, "rate": "audio", "channels": channels},
            {"name": OUT_BUS, "rate": "audio", "channels": channels, "external": true},
        ],
        "members": [
            {"def": strip_name(channels, channels),
             "controls": strip_wiring(MIX_BUS, channels, OUT_BUS, channels)},
            {"def": clip_name(1, channels), "kind": "graph", "slot": clip_slot(1),
             "controls": {OUT_BUS: MIX_BUS}},
            {"def": clip_name(2, channels), "kind": "graph", "slot": clip_slot(2),
             "controls": {OUT_BUS: MIX_BUS}},
        ],
        "surface": {
            GAIN:  [{"member": 0, "control": GAIN}],
            PAN:   [{"member": 0, "control": PAN}],
            WIDTH: [{"member": 0, "control": WIDTH}],
            MUTE:  [{"member": 0, "control": MUTE}],
        },
        "defaults": { GAIN: 1.0, WIDTH: 1.0 }
    }))
}

/// **The piece**: tracks onto the master bus, then the master strip onto the
/// hardware.
///
/// The master is the same strip as everything else, which is the point -- the
/// last fader in the chain is not a different kind of thing.
///
/// **The tracks are a slot of this** rather than instances beside it, and that
/// is not a nicety: a track's output is a bus, the master's mix bus is private
/// to the master's instance, and a graph instantiated on its own could never
/// name it. Containment is what lets the master hand each track the bus it
/// writes to -- so a whole piece is *one* `/graph_new`, and every track, clip
/// and reader after it is a slot added to what is already sounding.
pub fn piece_graph(channels: usize) -> Result<Value, String> {
    check(channels, "piece")?;
    Ok(json!({
        "name": piece_name(channels),
        "buses": [
            {"name": MIX_BUS, "rate": "audio", "channels": channels},
        ],
        "members": [
            {"def": strip_name(channels, channels),
             "controls": strip_wiring(MIX_BUS, channels, "OUT", channels)},
            {"def": track_name(channels), "kind": "graph", "slot": TRACK_SLOT,
             "controls": {OUT_BUS: MIX_BUS}},
        ],
        "surface": {
            GAIN:  [{"member": 0, "control": GAIN}],
            PAN:   [{"member": 0, "control": PAN}],
            WIDTH: [{"member": 0, "control": WIDTH}],
            MUTE:  [{"member": 0, "control": MUTE}],
        },
        "defaults": { GAIN: 1.0, WIDTH: 1.0 }
    }))
}

/// Every def a piece of these widths needs, **in the order they must be sent**:
/// the SynthDefs first, then the graphs that name them, then the graphs that
/// name those.
///
/// One call rather than a list a caller assembles, because the order is a rule
/// and a caller that got it wrong would find out at instantiation, in another
/// process, as a missing member.
pub fn defs_for(widths: &[(usize, usize)], master: usize) -> Result<Defs, String> {
    check(master, "piece")?;
    let mut strips: Vec<(usize, usize)> = widths.to_vec();
    strips.push((master, master));
    for &(_, outputs) in widths {
        strips.push((outputs, outputs));
        for inputs in 1..=MAX_CHANNELS {
            strips.push((inputs, outputs));
        }
    }
    for inputs in 1..=MAX_CHANNELS {
        strips.push((inputs, master));
    }
    strips.sort_unstable();
    strips.dedup();

    let mut synth = vec![reader_def()];
    for &(inputs, outputs) in &strips {
        synth.push(strip_def(inputs, outputs)?);
    }
    let mut graph = Vec::new();
    // A track declares a clip slot per source width, so every one of those clip
    // graphs has to exist before it -- not only the widths a piece happens to
    // use today, since a take of the other width is one import away.
    let mut tracks: Vec<usize> = widths.iter().map(|&(_, out)| out).collect();
    tracks.push(master);
    tracks.sort_unstable();
    tracks.dedup();
    for &channels in &tracks {
        for inputs in 1..=MAX_CHANNELS {
            graph.push(clip_graph(inputs, channels)?);
        }
    }
    for &channels in &tracks {
        graph.push(track_graph(channels)?);
    }
    graph.push(piece_graph(master)?);
    Ok(Defs { synth, graph })
}

/// The defs a piece needs, split by the family they are sent as.
pub struct Defs {
    /// `/def_send synth`, in order.
    pub synth: Vec<Value>,
    /// `/def_send graph`, in order: a graph never names one that comes after it.
    pub graph: Vec<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The port vocabulary is the same set everywhere it is spoken.** A knob,
    /// a curve and a `/node_set` write the same name, and this is what says so
    /// before an automation resolves against nothing.
    #[test]
    fn every_strip_answers_the_same_four_ports() {
        for graph in [
            clip_graph(1, 2).unwrap(),
            track_graph(2).unwrap(),
            piece_graph(2).unwrap(),
        ] {
            let surface = graph["surface"].as_object().expect("a surface");
            for port in [GAIN, PAN, WIDTH, MUTE] {
                assert!(surface.contains_key(port), "{} lacks {port}", graph["name"]);
            }
        }
    }

    /// **A width nobody wrote a downmix for is refused, not guessed.**
    #[test]
    fn a_width_past_stereo_is_refused() {
        let err = strip_def(6, 2).unwrap_err();
        assert!(
            err.contains("downmix"),
            "it says what the decision is: {err}"
        );
        assert!(
            strip_def(0, 2).is_err(),
            "and a strip of no channels is not one"
        );
    }

    /// **A mono source on a stereo strip is panned, not copied.** The same
    /// signal on both sides is 3 dB too loud in the middle, and it is the
    /// mistake worth a test rather than a comment.
    #[test]
    fn a_mono_strip_pans_into_stereo() {
        let def = strip_def(1, 2).unwrap();
        let kinds: Vec<&str> = def["ugens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds.iter().filter(|k| **k == "Pan2").count(), 2);
        assert_eq!(
            kinds.iter().filter(|k| **k == "In").count(),
            1,
            "one source channel"
        );
        assert_eq!(
            kinds.iter().filter(|k| **k == "Out").count(),
            2,
            "two sides"
        );
    }

    /// **A stereo source is balanced and given a width**, which is a different
    /// law from a pan and is why one word covers both — and the balance leaves
    /// the centre alone, which is the whole difference and what three strips in
    /// series would otherwise cost 9 dB for.
    #[test]
    fn a_stereo_strip_balances_and_widens() {
        let def = strip_def(2, 2).unwrap();
        let kinds: Vec<&str> = def["ugens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["kind"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"StereoWidth"));
        assert!(!kinds.contains(&"Pan2"), "a balance is not a pan");
        assert!(
            !kinds.contains(&"Balance2"),
            "`Balance2` is the pan law over a pair, which is 3 dB down at centre"
        );
    }

    /// **The send order is a rule, not a caller's guess**: a graph never names
    /// one that has not been sent.
    #[test]
    fn the_defs_come_back_in_an_order_that_resolves() {
        let defs = defs_for(&[(1, 2), (2, 2)], 2).unwrap();
        let synth: Vec<String> = defs
            .synth
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_string())
            .collect();
        assert!(synth.contains(&reader_name()));
        assert!(synth.contains(&strip_name(1, 2)));

        let mut sent: Vec<String> = synth;
        for graph in &defs.graph {
            for member in graph["members"].as_array().unwrap() {
                let named = member["def"].as_str().unwrap().to_string();
                assert!(sent.contains(&named), "{named} is named before it is sent");
            }
            sent.push(graph["name"].as_str().unwrap().to_string());
        }
        assert!(sent.contains(&piece_name(2)));
    }
}
