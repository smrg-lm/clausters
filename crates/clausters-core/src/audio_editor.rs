//! **What the audio editor is as nodes and groups** -- its node system,
//! written once.
//!
//! The multitrack's is [`crate::mixer`]; this is the audio editor's, and the
//! two are kept apart on purpose. What the editor plays is shaped like one
//! clip without the clip's strip: a take read whole at the transport's
//! position, measured, and sent to the hardware. Nothing here is the
//! multitrack's, and a fix one of them needs is a fix of that one.
//!
//! It is here, and not in a client, for the reason the mixer is: every client
//! that plays an audio editor needs the identical wiring. A caller states the
//! facts (how many channels the take has) and gets the specs; the caller
//! sends them, stamps the ids and instantiates.
//!
//! # The shape
//!
//! ```text
//! editor group                       follows the editor's transport: never frozen
//! |- transport group                 governed by it: frozen and thawed
//! |  `- ae.play.N                    the take; private `dry` (N)
//! |     |- [source] slot, per channel  ae.reader: one channel of the take -> dry
//! |     `- ae.pass.N                 dry -> the editor's bus (a mono take on both sides)
//! `- ae.output.W                     reads the editor's bus
//!    |- ae.meter.W                   -> W control buses: the level
//!    `- ae.declick.W                 x TransportFade -> the hardware
//! ```
//!
//! `N` is the take's channel count and `W` the editor bus's width, [`width`]:
//! the take's, and never less than two, so a mono take is heard on both sides
//! as an audio editor plays a mono file. There is no pan law, since there is
//! no strip: the pass writes the one channel to both sides at unity.
//!
//! - **The output is outside the transport group**, because it must not
//!   freeze: a paused meter falls to zero rather than holding what it last
//!   saw, and the declick runs across the moment the readers stop -- which
//!   only works because a transport with a ramp rolls it out before it
//!   freezes (`/transport_fade`).
//! - **The reader's window is the take**: its `span` is the take's length on
//!   the transport, stated by whoever stitches the join, so past the end the
//!   gate is exactly zero and nothing is compared against the buffer per
//!   sample. The gate is a step at both ends: a click in the file is heard as
//!   a click, and what declicks a stop is the output, not the reader.
//! - **The meter reads the take before the declick**, so it shows the take
//!   and not the ramp. The mark that waits is the `meter` widget's own
//!   ballistics, which are the core's (`measure::Ballistics`).
//!
//! **The editor's bus is the caller's**, `W` audio channels it allocates, and
//! it reaches both graphs as **port values**: `out0..` on `ae.play`, `in0..`
//! on `ae.output`. A graph instantiated at the top is handed no bus of its
//! parent's, since it has none, and the two graphs live in different groups --
//! so the bus is a number, the way a meter's control bus is.
//!
//! # What is deliberately not here yet
//!
//! **Effects in preview**: a chain in place on `dry`, between the readers and
//! the pass. It is declared when the first effect is written, since a slot of
//! nothing cannot be heard and so cannot be checked.

use serde_json::{Value, json};

use crate::mixer::meter_spec;

/// The prefix every def here is named with -- the audio editor's, so nothing
/// here is taken for the multitrack's (`mt`).
pub const PREFIX: &str = "ae";

/// The buffer a reader reads.
pub const BUF: &str = "buf";
/// Which channel of the take a reader takes.
pub const CHAN: &str = "chan";
/// How long the take lasts on the transport, in engine samples.
pub const SPAN: &str = "span";
/// The slot a take's readers fill: one per channel.
pub const SOURCE_SLOT: &str = "source";
/// The bus the readers write, private to each `ae.play`.
pub const DRY_BUS: &str = "dry";

/// The most channels a take is played with: a bound rather than a judgement
/// about contents, which keeps a malformed channel count from filling the
/// node tree, and is well past any take a person edits by hand.
pub const MAX_CHANNELS: usize = 32;

/// **How wide the editor's bus is for a take of `channels`**: the take's own
/// width, and never less than two -- a mono take is heard on both sides.
pub fn width(channels: usize) -> usize {
    channels.max(2)
}

/// The port channel `channel` of the editor's bus is set on: where
/// `ae.play` writes it (`out0..`) and where `ae.output` reads it (`in0..`).
pub fn bus_port(direction: &str, channel: usize) -> String {
    format!("{direction}{channel}")
}

/// The port the level of channel `channel` is written to, as a control bus.
pub fn meter_port(channel: usize) -> String {
    format!("meter/out{channel}")
}

/// The name of the reader def: one channel of the take.
pub fn reader_name() -> String {
    format!("{PREFIX}.reader")
}

/// The name of the pass for a take of `channels`.
pub fn pass_name(channels: usize) -> String {
    format!("{PREFIX}.pass.{channels}")
}

/// The name of the play graph for a take of `channels`.
pub fn play_name(channels: usize) -> String {
    format!("{PREFIX}.play.{channels}")
}

/// The name of the meter def for an editor bus of `width`.
pub fn meter_name(width: usize) -> String {
    format!("{PREFIX}.meter.{width}")
}

/// The name of the declick def for an editor bus of `width`.
pub fn declick_name(width: usize) -> String {
    format!("{PREFIX}.declick.{width}")
}

/// The name of the output graph for an editor bus of `width`.
pub fn output_name(width: usize) -> String {
    format!("{PREFIX}.output.{width}")
}

/// Refuses a channel count this module does not play.
fn check(channels: usize) -> Result<(), String> {
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(format!(
            "a take of {channels} channels; the audio editor plays 1 to {MAX_CHANNELS}"
        ));
    }
    Ok(())
}

fn control(name: &str, default: f32) -> Value {
    json!({ "name": name, "default": default })
}

/// `in0..` then `out0..`, `n` of each: the controls a per-channel copy
/// declares, in the order its UGens index them.
fn in_out_controls(ins: usize, outs: usize) -> Vec<Value> {
    let mut controls = Vec::new();
    for channel in 0..ins {
        controls.push(control(&format!("in{channel}"), 0.0));
    }
    for channel in 0..outs {
        controls.push(control(&format!("out{channel}"), 0.0));
    }
    controls
}

/// `{"in0": "<bus>:0", ...}` for `n` channels of `bus`, under `prefix`.
fn wiring(prefix: &str, bus: &str, n: usize) -> serde_json::Map<String, Value> {
    (0..n)
        .map(|channel| {
            (
                format!("{prefix}{channel}"),
                json!(format!("{bus}:{channel}")),
            )
        })
        .collect()
}

/// **One channel of the take**, following the transport.
///
/// The multitrack's reader without what a take played whole does not use: no
/// `at` (the take starts at the transport's zero), no `start`, no `loop` (the
/// transport loops) and no `rate` beyond the buffer's own. The phase is the
/// transport's position in engine samples scaled by `BufRateScale`, so a
/// 44.1 kHz take in a 48 kHz session plays at its true pitch.
///
/// The gate is `0 <= position < span`, with no ramp: past the take's end the
/// output is exactly zero, and neither the first nor the last sample is
/// changed. `Out` writes `out + chan`, so each reader lands on its own channel
/// of `dry` whatever the slot hands it as `out`.
pub fn reader_def() -> Value {
    json!({
        "name": reader_name(),
        "controls": [
            control("out", 0.0),
            control(BUF, 0.0),
            control(CHAN, 0.0),
            control(SPAN, 0.0),
        ],
        "ugens": [
            {"kind": "TransportPos", "inputs": [{"const": 0.0}]},
            {"kind": "BinaryOpUGen", "op": "ge", "inputs": [{"ugen": 0}, {"const": 0.0}]},
            {"kind": "BinaryOpUGen", "op": "lt", "inputs": [{"ugen": 0}, {"control": 3}]},
            {"kind": "Mul", "inputs": [{"ugen": 1}, {"ugen": 2}]},
            {"kind": "BufRateScale", "inputs": [{"control": 1}]},
            {"kind": "Mul", "inputs": [{"ugen": 0}, {"ugen": 4}]},
            {"kind": "BufRd", "inputs": [
                {"control": 1}, {"control": 2}, {"ugen": 5}, {"const": 0.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 6}, {"ugen": 3}]},
            {"kind": "Add", "inputs": [{"control": 0}, {"control": 2}]},
            {"kind": "Out", "inputs": [{"ugen": 8}, {"ugen": 7}]},
        ],
    })
}

/// **`dry` onto the editor's bus at unity**: `In` per channel into `Out`, and a mono
/// take's one channel onto both sides. The point after the effects where the
/// take leaves the play graph.
pub fn pass_def(channels: usize) -> Result<Value, String> {
    check(channels)?;
    let outs = width(channels);
    let mut ugens = Vec::new();
    for channel in 0..channels {
        ugens.push(json!({"kind": "In", "inputs": [{"control": channel}]}));
    }
    for out in 0..outs {
        let read = out.min(channels - 1);
        ugens.push(json!({"kind": "Out", "inputs": [
            {"control": channels + out}, {"ugen": read}
        ]}));
    }
    Ok(json!({
        "name": pass_name(channels),
        "controls": in_out_controls(channels, outs),
        "ugens": ugens,
    }))
}

/// **The level meter**: the multitrack's meter, the same algorithm under the
/// editor's name, over the editor's bus.
pub fn meter_def(width: usize) -> Result<Value, String> {
    check(width)?;
    Ok(meter_spec(&meter_name(width), width, width))
}

/// **The declick**: the editor's bus times `TransportFade`, onto the
/// hardware. The transport it reads is the editor's, since the output sits in
/// the group that follows it.
pub fn declick_def(width: usize) -> Result<Value, String> {
    check(width)?;
    let mut ugens = vec![json!({"kind": "TransportFade", "inputs": []})];
    for channel in 0..width {
        let read = ugens.len() as u32;
        ugens.push(json!({"kind": "In", "inputs": [{"control": channel}]}));
        ugens.push(json!({"kind": "Mul", "inputs": [{"ugen": read}, {"ugen": 0}]}));
        ugens.push(json!({"kind": "Out", "inputs": [
            {"control": width + channel}, {"ugen": read + 1}
        ]}));
    }
    Ok(json!({
        "name": declick_name(width),
        "controls": in_out_controls(width, width),
        "ugens": ugens,
    }))
}

/// **The take, as a graph**: its readers onto a private `dry`, and the pass
/// onto the editor's bus, whose channels are the ports `out0..` ([`bus_port`]).
/// The rest of its surface is the readers' own -- `buf`, `chan`, `span` --
/// which a `/graph_addSlot` on [`SOURCE_SLOT`] sets per channel.
pub fn play_graph(channels: usize) -> Result<Value, String> {
    check(channels)?;
    let outs = width(channels);
    let pass = wiring("in", DRY_BUS, channels);
    let mut surface = serde_json::Map::new();
    surface.insert(BUF.into(), json!([{"member": 1, "control": BUF}]));
    surface.insert(CHAN.into(), json!([{"member": 1, "control": CHAN}]));
    surface.insert(SPAN.into(), json!([{"member": 1, "control": SPAN}]));
    for channel in 0..outs {
        let port = bus_port("out", channel);
        surface.insert(port.clone(), json!([{"member": 0, "control": port}]));
    }
    Ok(json!({
        "name": play_name(channels),
        "buses": [{"name": DRY_BUS, "rate": "audio", "channels": channels}],
        "members": [
            {"def": pass_name(channels), "controls": pass},
            {"def": reader_name(), "slot": SOURCE_SLOT, "controls": {"out": DRY_BUS}},
        ],
        "surface": surface,
    }))
}

/// **The output**: the editor's bus, read on the ports `in0..`, measured and
/// sent to the hardware through the declick. Each channel's level lands on the
/// control bus its [`meter_port`] is set to.
pub fn output_graph(width: usize) -> Result<Value, String> {
    check(width)?;
    let mut declick = serde_json::Map::new();
    for channel in 0..width {
        declick.insert(bus_port("out", channel), json!(format!("OUT:{channel}")));
    }
    let mut surface = serde_json::Map::new();
    for channel in 0..width {
        let port = bus_port("in", channel);
        surface.insert(
            port.clone(),
            json!([{"member": 0, "control": port}, {"member": 1, "control": port}]),
        );
        surface.insert(
            meter_port(channel),
            json!([{"member": 0, "control": bus_port("out", channel)}]),
        );
    }
    Ok(json!({
        "name": output_name(width),
        "members": [
            {"def": meter_name(width)},
            {"def": declick_name(width), "controls": declick},
        ],
        "surface": surface,
    }))
}

/// The defs an audio editor playing a take of `channels` needs, split by
/// family and **in the order they must be sent**: the SynthDefs, then the
/// graphs that name them.
pub fn defs_for(channels: usize) -> Result<crate::mixer::Defs, String> {
    check(channels)?;
    let w = width(channels);
    Ok(crate::mixer::Defs {
        synth: vec![
            reader_def(),
            pass_def(channels)?,
            meter_def(w)?,
            declick_def(w)?,
        ],
        graph: vec![play_graph(channels)?, output_graph(w)?],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A mono take is heard on both sides**: its pass writes its one
    /// channel to both of the editor's, and the output is two wide.
    #[test]
    fn a_mono_take_is_passed_to_both_sides() {
        let pass = pass_def(1).unwrap();
        let outs: Vec<&Value> = pass["ugens"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|u| u["kind"] == "Out")
            .collect();
        assert_eq!(outs.len(), 2);
        assert!(outs.iter().all(|u| u["inputs"][1]["ugen"] == 0));
        assert_eq!(width(1), 2);
        assert_eq!(output_graph(width(1)).unwrap()["name"], "ae.output.2");
    }

    /// **The declick reads the transport's level**, per channel, onto the
    /// hardware.
    #[test]
    fn the_declick_scales_every_channel_by_the_transports_level() {
        let declick = declick_def(2).unwrap();
        let ugens = declick["ugens"].as_array().unwrap();
        assert_eq!(ugens[0]["kind"], "TransportFade");
        let muls: Vec<&Value> = ugens.iter().filter(|u| u["kind"] == "Mul").collect();
        assert_eq!(muls.len(), 2);
        assert!(muls.iter().all(|u| u["inputs"][1]["ugen"] == 0));
        let output = output_graph(2).unwrap();
        assert_eq!(output["members"][1]["controls"]["out1"], "OUT:1");
    }

    /// **One port per channel of the editor's bus reaches both readers of
    /// it** -- the meter and the declick -- and each level has a port.
    #[test]
    fn the_editors_bus_is_a_port_on_both_graphs() {
        let output = output_graph(3).unwrap();
        let surface = output["surface"].as_object().unwrap();
        for channel in 0..3 {
            assert_eq!(
                surface[&bus_port("in", channel)].as_array().unwrap().len(),
                2
            );
            assert!(surface.contains_key(&meter_port(channel)));
        }
        let play = play_graph(1).unwrap();
        let surface = play["surface"].as_object().unwrap();
        assert!(surface.contains_key("out0") && surface.contains_key("out1"));
        assert!(!surface.contains_key("out2"));
    }

    /// Nothing past the bound is built, and nothing empty.
    #[test]
    fn a_channel_count_out_of_range_is_refused() {
        assert!(defs_for(0).is_err());
        assert!(defs_for(MAX_CHANNELS + 1).is_err());
        let defs = defs_for(2).unwrap();
        assert_eq!(defs.synth[0]["name"], "ae.reader");
        assert_eq!(defs.graph[1]["name"], "ae.output.2");
    }
}
