//! **Event lanes**: sequences of notes and messages the transport plays, the
//! way a reader plays a take.
//!
//! A take follows the transport because its reader reads the position: a
//! locate, a loop and a stop are the transport's and nothing is sent per
//! pass. A lane is the same thing for events. It holds its notes and messages
//! as data, in samples of one transport's position, and this side keeps the
//! stretch of it just ahead of the position **built** on the engine's lane
//! queue (`Cmd::LaneEntry`), where an entry fires when the position reaches
//! it. Keyed by position, an entry needs nothing re-stamped when the transport
//! jumps: one the position left behind waits for it to come back.
//!
//! The feed is here and not on the audio thread because a note is a node, and
//! a node is built on this one: what reaches the engine is what a timed bundle
//! carries, built by the same translator. Each turn of the loop tops the lane
//! queue up to [`LOOKAHEAD_SECS`] ahead of where the transport stands, in
//! playback order -- through a loop's wrap -- and a locate, a loop set or
//! cleared, a pass that ended on its mark, and new data clear what the lane
//! had queued and feed it again.
//!
//! **Node ids are the server's**, minted per pass from the auto range as a
//! `/synth_new -1` would be, because a lane plays again and again and a note's
//! start and its release must name one node. An entry that leaves unrun comes
//! back through the garbage FIFO with its commands, and the nodes it would have
//! made are forgotten there ([`OscServer::forget_unrun`]).

use std::collections::HashSet;

use super::*;
use crate::server::engine::LaneTag;

/// How far ahead of the position a lane is kept built, in seconds. Far more
/// than a turn of the loop (10 ms while a lane exists), so a turn spent on
/// something slow -- a def compiled, a file read -- does not let the position
/// overtake the feed.
pub(in crate::osc::server) const LOOKAHEAD_SECS: f64 = 0.5;

/// How a note is released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Release {
    /// `/node_set <id> gate 0`: the def's envelope releases it.
    Gate,
    /// `/node_free <id>`.
    Free,
}

/// One event of a lane, at a position of its transport.
#[derive(Clone, Debug, PartialEq)]
enum LaneEvent {
    /// A note: a synth of `def` with `controls`, released `length` samples
    /// after it starts.
    Note {
        def: String,
        controls: Vec<OscType>,
        length: u64,
        release: Release,
    },
    /// A command the server takes in a timed bundle, run as it is written.
    Message(OscMessage),
}

/// One lane, as this side holds it.
pub(in crate::osc::server) struct Lane {
    transport: usize,
    target: i32,
    /// `(position, event)`, sorted by position.
    events: Vec<(u64, LaneEvent)>,
    /// The generation of `events`, bumped by every new data and every clear,
    /// so an entry spent from an older one does not unmark a newer one.
    generation: u32,
    /// The events of this generation that are on the engine's lane queue.
    queued: HashSet<u32>,
}

/// Parses a lane's data: `{"notes": [[start, end, "def", {controls},
/// "gate"|"free"], ...], "messages": [[position, "/addr", args...], ...]}`,
/// every position in samples of the transport.
fn parse_events(json: &[u8]) -> Result<Vec<(u64, LaneEvent)>, String> {
    let value: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("invalid JSON: {e}"))?;
    let sample = |v: &serde_json::Value, what: &str| -> Result<u64, String> {
        v.as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
            .map(|n| n.round() as u64)
            .ok_or_else(|| format!("{what} must be a sample >= 0"))
    };
    let mut events = Vec::new();
    for note in value
        .get("notes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let fields = note.as_array().ok_or("a note is an array")?;
        let [start, end, def, controls, rest @ ..] = fields.as_slice() else {
            return Err("a note is [start, end, def, controls, release]".into());
        };
        let (start, end) = (sample(start, "start")?, sample(end, "end")?);
        let def = def.as_str().ok_or("a note's def is a name")?.to_string();
        let mut pairs = Vec::new();
        for (name, value) in controls.as_object().into_iter().flatten() {
            if let Some(v) = value.as_f64() {
                pairs.push(OscType::String(name.clone()));
                pairs.push(OscType::Float(v as f32));
            }
        }
        let release = match rest.first().and_then(serde_json::Value::as_str) {
            None | Some("gate") => Release::Gate,
            Some("free") => Release::Free,
            Some(other) => return Err(format!("a note is released by gate or free, not {other}")),
        };
        events.push((
            start,
            LaneEvent::Note {
                def,
                controls: pairs,
                length: end.saturating_sub(start),
                release,
            },
        ));
    }
    for message in value
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let fields = message.as_array().ok_or("a message is an array")?;
        let [position, addr, args @ ..] = fields.as_slice() else {
            return Err("a message is [position, address, args...]".into());
        };
        let addr = addr.as_str().ok_or("a message's address is a string")?;
        let args = args
            .iter()
            .map(|a| match a {
                serde_json::Value::Number(n) if n.is_i64() => {
                    OscType::Int(n.as_i64().unwrap_or(0) as i32)
                }
                serde_json::Value::Number(n) => OscType::Float(n.as_f64().unwrap_or(0.0) as f32),
                serde_json::Value::String(s) => OscType::String(s.clone()),
                serde_json::Value::Bool(b) => OscType::Int(i32::from(*b)),
                other => OscType::String(other.to_string()),
            })
            .collect();
        events.push((
            sample(position, "a message's position")?,
            LaneEvent::Message(OscMessage {
                addr: addr.to_string(),
                args,
            }),
        ));
    }
    events.sort_by_key(|(position, _)| *position);
    Ok(events)
}

/// **The spans of position a transport plays next**, in playback order,
/// `window` samples long in all: straight on from `position`, or up to a
/// loop's end and on from its start -- never more than one pass of the loop,
/// so an event is fed once per pass -- and never past an end mark, which the
/// transport stops on.
fn ahead(
    position: u64,
    window: u64,
    looping: Option<(u64, u64)>,
    end: Option<u64>,
) -> Vec<(u64, u64)> {
    match looping {
        Some((start, stop)) if position < stop && start < stop => {
            let first = (position, stop.min(position + window));
            let left = window.saturating_sub(stop - position);
            // What is left wraps, up to where this pass began, or the loop's
            // end when the position is before the loop.
            let limit = if position >= start { position } else { stop };
            let second = (start, (start + left).min(limit));
            [first, second].into_iter().filter(|(a, b)| a < b).collect()
        }
        _ => {
            let stop = end.map_or(position + window, |e| e.min(position + window));
            if position < stop {
                vec![(position, stop)]
            } else {
                Vec::new()
            }
        }
    }
}

impl OscServer {
    /// `/lane_new <transport:int32> <lane:int32> <target:int32>` -- makes lane
    /// `lane` on transport `transport`, its notes made at the tail of group
    /// `target` (the group that transport governs, for a pause to freeze
    /// them). An existing lane of that id is freed first. Replies `/done`.
    pub(in crate::osc::server) fn handle_lane_new(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let transport = self.transport_arg(&mut args)?;
        let lane = args.int()?;
        let target = args.int()?;
        if self.lanes.contains_key(&lane) {
            self.free_lane(lane)?;
        }
        self.lanes.insert(
            lane,
            Lane {
                transport,
                target,
                events: Vec::new(),
                generation: 0,
                queued: HashSet::new(),
            },
        );
        self.retune_timeout();
        self.done(from, "/lane_new");
        Ok(())
    }

    /// `/lane_set <lane:int32> <json>` -- replaces lane `lane`'s data whole
    /// (the JSON as a string or a blob, like a def's). What it had queued is
    /// cleared and fed again from the position; the notes it is sounding keep
    /// their releases. Replies `/done`.
    pub(in crate::osc::server) fn handle_lane_set(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let id = args.int()?;
        let events = parse_events(crate::osc::args::json_payload(args.rest())?)?;
        if !self.lanes.contains_key(&id) {
            return Err(format!("no lane {id}"));
        }
        self.clear_lane(id, false)?;
        if let Some(lane) = self.lanes.get_mut(&id) {
            lane.events = events;
        }
        self.feed_lane(id);
        self.done(from, "/lane_set");
        Ok(())
    }

    /// `/lane_free <lane:int32>` -- frees lane `lane`: what it queued is
    /// dropped and the notes it is sounding are released now. Replies `/done`.
    pub(in crate::osc::server) fn handle_lane_free(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let id = args.int()?;
        if !self.lanes.contains_key(&id) {
            return Err(format!("no lane {id}"));
        }
        self.free_lane(id)?;
        self.retune_timeout();
        self.done(from, "/lane_free");
        Ok(())
    }

    fn free_lane(&mut self, id: i32) -> Answer {
        self.clear_lane(id, true)?;
        self.lanes.remove(&id);
        Ok(())
    }

    /// Drops what lane `id` has queued, and with `release` the releases of
    /// the notes it is sounding run now. Its generation moves, so what comes
    /// back spent from before is not taken for what is queued after.
    fn clear_lane(&mut self, id: i32, release: bool) -> Answer {
        let Some(lane) = self.lanes.get_mut(&id) else {
            return Ok(());
        };
        lane.generation = lane.generation.wrapping_add(1);
        lane.queued.clear();
        let transport = lane.transport;
        self.handle
            .send(Cmd::ClearLane {
                transport,
                lane: id,
                release,
            })
            .map_err(|_| "command FIFO full".to_string())
    }

    /// **Transport `k` jumped** -- a locate, a loop set or cleared, a pass
    /// that went back to its mark: every lane on it is cleared and fed again
    /// from where it now stands. The engine releases the sounding notes on a
    /// jump itself, so the clear leaves releases alone.
    pub(in crate::osc::server) fn lanes_jumped(&mut self, k: usize) {
        let ids: Vec<i32> = self
            .lanes
            .iter()
            .filter(|(_, lane)| lane.transport == k)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if self.clear_lane(id, false).is_ok() {
                self.feed_lane(id);
            }
        }
    }

    /// Where transport `k` stands, as a reply would say: a locate sent and not
    /// yet applied counts as done.
    pub(in crate::osc::server) fn transport_position(&self, k: usize) -> u64 {
        let clock = self.handle.current_transport_samples(k);
        match self.transports[k].pending_locate {
            Some((position, sent_at)) if clock <= sent_at => position,
            _ => self.handle.current_transport_position(k),
        }
    }

    /// One turn's feed: every lane topped up ahead of its transport.
    pub(in crate::osc::server) fn feed_lanes(&mut self) {
        let ids: Vec<i32> = self.lanes.keys().copied().collect();
        for id in ids {
            self.feed_lane(id);
        }
    }

    /// Tops lane `id` up: every event within the lookahead of its transport's
    /// position, in playback order, that is not on the lane queue yet is built
    /// and sent there.
    fn feed_lane(&mut self, id: i32) {
        let Some(lane) = self.lanes.get(&id) else {
            return;
        };
        let k = lane.transport;
        let t = self.transports[k];
        let window = (LOOKAHEAD_SECS * self.info.actual_sample_rate) as u64;
        let looping = t.loop_span.map(|(a, b)| (a.max(0) as u64, b.max(0) as u64));
        let end = t.end_mark.map(|(e, _)| e.max(0) as u64);
        let spans = ahead(self.transport_position(k), window, looping, end);
        let mut due: Vec<u32> = Vec::new();
        for (from, to) in spans {
            let first = lane.events.partition_point(|(p, _)| *p < from);
            for (i, (position, _)) in lane.events.iter().enumerate().skip(first) {
                if *position >= to {
                    break;
                }
                if !lane.queued.contains(&(i as u32)) {
                    due.push(i as u32);
                }
            }
        }
        for i in due {
            match self.lane_entry(id, i) {
                Ok(Some(cmd)) => {
                    if self.handle.send(cmd).is_err() {
                        // A full FIFO: the next turn offers it again.
                        break;
                    }
                    if let Some(lane) = self.lanes.get_mut(&id) {
                        lane.queued.insert(i);
                    }
                }
                Ok(None) => {}
                Err(why) => {
                    warn!("lane {id}: event {i} was not built: {why}");
                    // Marked all the same, so a broken event is not rebuilt
                    // and warned about on every turn.
                    if let Some(lane) = self.lanes.get_mut(&id) {
                        lane.queued.insert(i);
                    }
                }
            }
        }
    }

    /// Builds lane `id`'s event `i` as the entry the engine takes: a note's
    /// `/synth_new` with a fresh id and its release, or a message.
    fn lane_entry(&mut self, id: i32, i: u32) -> Result<Option<Cmd>, String> {
        let Some(lane) = self.lanes.get(&id) else {
            return Ok(None);
        };
        let Some((position, event)) = lane.events.get(i as usize).cloned() else {
            return Ok(None);
        };
        let (transport, target) = (lane.transport, lane.target);
        let tag = LaneTag {
            lane: id,
            generation: lane.generation,
            event: i,
        };
        let mut start = Vec::new();
        let mut release = Vec::new();
        let mut length = 0;
        match event {
            LaneEvent::Note {
                def,
                controls,
                length: held,
                release: how,
            } => {
                let mut args = vec![
                    OscType::String(def),
                    OscType::Int(-1),
                    OscType::Int(1),
                    OscType::Int(target),
                ];
                args.extend(controls);
                self.translator.translate(
                    &OscMessage {
                        addr: "/synth_new".into(),
                        args,
                    },
                    &mut start,
                )?;
                let Some(node) = start.iter().find_map(|cmd| match cmd {
                    Cmd::AddSynth { id, .. } => Some(*id),
                    _ => None,
                }) else {
                    return Ok(None);
                };
                let message = match how {
                    Release::Gate => OscMessage {
                        addr: "/node_set".into(),
                        args: vec![
                            OscType::Int(node),
                            OscType::String("gate".into()),
                            OscType::Float(0.0),
                        ],
                    },
                    Release::Free => OscMessage {
                        addr: "/node_free".into(),
                        args: vec![OscType::Int(node)],
                    },
                };
                if let Err(why) = self.translator.translate(&message, &mut release) {
                    self.forget_unrun(&start);
                    return Err(why);
                }
                length = held;
            }
            LaneEvent::Message(message) => {
                self.translator.translate(&message, &mut start)?;
            }
        }
        Ok(Some(Cmd::LaneEntry {
            transport,
            position,
            tag,
            start,
            release,
            length,
        }))
    }

    /// **A lane entry left the engine**: it is no longer queued, and when it
    /// never ran, the nodes it would have made are forgotten.
    pub(in crate::osc::server) fn lane_spent(&mut self, tag: LaneTag, start: &[Cmd], fired: bool) {
        if !fired {
            self.forget_unrun(start);
        }
        if let Some(lane) = self.lanes.get_mut(&tag.lane)
            && lane.generation == tag.generation
        {
            lane.queued.remove(&tag.event);
        }
    }

    /// **Commands that were built and never ran** -- a cleared timed bundle, an
    /// unrun lane entry: the nodes they would have made never existed, so the
    /// mirror forgets them and a server-minted id goes back to its range. A
    /// shell that ran is empty and has nothing here.
    pub(in crate::osc::server) fn forget_unrun(&mut self, cmds: &[Cmd]) {
        for cmd in cmds {
            if let Cmd::AddSynth { id, .. } | Cmd::AddGroup { id, .. } = cmd {
                self.translator.forget_node(*id);
                self.translator.release_node_id(*id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_runs_straight_on_and_stops_at_the_end_mark() {
        assert_eq!(ahead(100, 50, None, None), vec![(100, 150)]);
        assert_eq!(ahead(100, 50, None, Some(120)), vec![(100, 120)]);
        assert!(ahead(130, 50, None, Some(120)).is_empty());
    }

    #[test]
    fn a_window_wraps_through_a_loop_and_covers_one_pass() {
        // 20 to the loop's end, and 30 more from its start.
        assert_eq!(
            ahead(80, 50, Some((0, 100)), None),
            vec![(80, 100), (0, 30)]
        );
        // A loop shorter than the window: one pass, never an event twice.
        assert_eq!(
            ahead(15, 50, Some((10, 30)), None),
            vec![(15, 30), (10, 15)]
        );
        // Past the loop's end the position runs on, as the engine's does.
        assert_eq!(ahead(120, 50, Some((0, 100)), None), vec![(120, 170)]);
    }

    #[test]
    fn a_lanes_data_reads_notes_and_messages_in_position_order() {
        let events = parse_events(
            br#"{"notes": [[480, 960, "sine", {"freq": 440, "amp": 0.1}, "free"]],
                 "messages": [[0, "/mark", 1, 2.5, "cue"]]}"#,
        )
        .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, 0);
        let LaneEvent::Message(m) = &events[0].1 else {
            panic!("the message first");
        };
        assert_eq!(
            m.args,
            vec![
                OscType::Int(1),
                OscType::Float(2.5),
                OscType::String("cue".into())
            ]
        );
        let LaneEvent::Note {
            length, release, ..
        } = &events[1].1
        else {
            panic!("then the note");
        };
        assert_eq!((*length, *release), (480, Release::Free));
        assert!(parse_events(br#"{"notes": [[0, 1, "x", {}, "slowly"]]}"#).is_err());
    }
}
