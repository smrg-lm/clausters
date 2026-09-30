//! `/midi_query`: what the MIDI bindings are, read back.
//!
//! The bindings themselves are the translator's (`/midi_bind`,
//! `/midi_bindZone`, `/midi_unbind`, `/midi_map` go through
//! [`OscServer::handle_via_translate`]); this reads them.

use clausters_midi::mpe::Side;

use super::super::*;
use crate::midi::MidiBinding;

impl OscServer {
    /// `/midi_query [channel...]` -- one `/midi_query.reply` per binding, then
    /// `/done "/midi_query"`. A reply is `kind channel members instrument
    /// target addAction gate` and then the control map as `selector name`
    /// pairs: `kind` is `"channel"` for a per-channel binding and `"zone"` for
    /// an MPE zone (`channel` its master, `members` as the device last set
    /// them). With no argument every binding is listed; a channel asked for
    /// and bound to nothing comes back as `"none" channel`.
    pub(in crate::osc::server) fn handle_midi_query(
        &mut self,
        args: Args,
        from: ClientId,
    ) -> Answer {
        let midi = &self.translator.midi;
        let asked: Vec<u8> = args
            .rest()
            .iter()
            .filter_map(|a| match a {
                OscType::Int(c) => u8::try_from(*c).ok(),
                _ => None,
            })
            .collect();
        let channels: Vec<u8> = if asked.is_empty() {
            let mut all: Vec<u8> = midi
                .channels
                .keys()
                .chain(midi.zones.keys())
                .copied()
                .collect();
            all.sort_unstable();
            all
        } else {
            asked
        };
        let mut replies = Vec::new();
        for channel in channels {
            let reply = if let Some(zone) = midi.zones.get(&channel) {
                let members = Side::of_master(channel)
                    .and_then(|side| midi.decoder.zone(side))
                    .map_or(zone.config.members, |z| z.members);
                let mut reply = vec![
                    OscType::String("zone".into()),
                    OscType::Int(i32::from(channel)),
                    OscType::Int(i32::from(members)),
                ];
                reply.extend(binding_args(&zone.binding, Some(zone.config.timbre_cc)));
                reply
            } else if let Some(binding) = midi.channels.get(&channel) {
                let mut reply = vec![
                    OscType::String("channel".into()),
                    OscType::Int(i32::from(channel)),
                    OscType::Int(0),
                ];
                reply.extend(binding_args(binding, None));
                reply
            } else {
                vec![
                    OscType::String("none".into()),
                    OscType::Int(i32::from(channel)),
                ]
            };
            replies.push(reply);
        }
        for reply in replies {
            self.reply(from, "/midi_query.reply", reply);
        }
        self.done(from, "/midi_query");
        Ok(())
    }
}

/// A binding as a reply carries it: instrument, target, addAction, gate, then
/// every selector that names a control, with the zone's timbre controller
/// number after its `timbre` pair's name as `timbreCc`.
fn binding_args(b: &MidiBinding, timbre_cc: Option<u8>) -> Vec<OscType> {
    let mut out = vec![
        OscType::String(b.instrument.clone()),
        OscType::Int(b.target),
        OscType::Int(b.action),
        OscType::Int(i32::from(b.gate)),
    ];
    let mut pair = |selector: String, name: &str| {
        out.push(OscType::String(selector));
        out.push(OscType::String(name.to_string()));
    };
    pair("note".into(), &b.freq_control);
    pair("vel".into(), &b.amp_control);
    pair("gate".into(), &b.gate_control);
    for (selector, name) in [
        ("bend", &b.bend_control),
        ("pressure", &b.pressure_control),
        ("poly", &b.poly_control),
        ("timbre", &b.timbre_control),
        ("lift", &b.lift_control),
    ] {
        if let Some(name) = name {
            pair(selector.into(), name);
        }
    }
    let mut cc: Vec<_> = b.cc.iter().collect();
    cc.sort_unstable();
    for (n, name) in cc {
        pair(format!("cc{n}"), name);
    }
    let mut programs: Vec<_> = b.programs.iter().collect();
    programs.sort_unstable();
    for (n, name) in programs {
        pair(format!("prog{n}"), name);
    }
    if let Some(n) = timbre_cc {
        out.push(OscType::String("timbreCc".into()));
        out.push(OscType::Int(i32::from(n)));
    }
    out
}
