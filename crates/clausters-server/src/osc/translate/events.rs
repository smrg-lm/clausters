//! Events written by a client: on an event bus, or to one node.
//!
//! `/bus_event` writes notes and channel messages on an event bus, where
//! whatever reads it plays them; `/node_event` gives them to one node that
//! reads notes, as though they came from its bus, and adds what only an
//! event addressed to one node can say -- a control by name. Both read the
//! JSON the shared core reads (`clausters_core::event_bus::read_all`), and
//! both run at the sample their bundle names, as any command does.

use super::*;
use clausters_core::event_bus::{Written, read_all};

impl CmdTranslator {
    /// `/bus_event bus json`: the events, written on event bus `bus`.
    pub(super) fn bus_event(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let Some(bus) = msg.args.first().and_then(int_value) else {
            return Err("expected: event bus, events JSON".into());
        };
        let bus = u32::try_from(bus)
            .ok()
            .filter(|b| (*b as usize) < self.limits.event_buses)
            .ok_or_else(|| format!("event bus must be in range 0..{}", self.limits.event_buses))?;
        let events = read_all(crate::osc::args::json_payload(&msg.args[1..])?)?;
        if events.iter().any(|w| w.control.is_some()) {
            return Err(
                "a control is one node's: send a control event to the node, with /node_event"
                    .into(),
            );
        }
        cmds.extend(events.into_iter().map(|w| Cmd::BusEvent {
            bus,
            event: w.event,
        }));
        Ok(())
    }

    /// `/node_event node json`: the events, given to node `node` -- a plugin
    /// or a `Voices` node -- as though read from its bus. A `control` event
    /// names one of the node's controls.
    pub(super) fn node_event(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let Some(id) = msg.args.first().and_then(int_value) else {
            return Err("expected: node, events JSON".into());
        };
        let events = read_all(crate::osc::args::json_payload(&msg.args[1..])?)?;
        let def = self
            .node_defs
            .get(&id)
            .ok_or_else(|| format!("node {id} not found"))?;
        if !def.reads_notes() {
            return Err(format!(
                "node {id} reads no notes: events go to a plugin or a Voices node"
            ));
        }
        let mut resolved = Vec::with_capacity(events.len());
        for Written { mut event, control } in events {
            if let Some(name) = control {
                if !def.takes_note_controls() {
                    return Err(
                        "a plugin's controls are the node's: set them with /node_set".into(),
                    );
                }
                event.index = def
                    .control_index(&name)
                    .ok_or_else(|| format!("node {id} has no control {name:?}"))?;
            }
            resolved.push(Cmd::NodeEvent { id, event });
        }
        cmds.extend(resolved);
        Ok(())
    }
}
