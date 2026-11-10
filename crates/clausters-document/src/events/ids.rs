//! **A sequence's ids**: one counter, and how an id from outside is taken in.
//!
//! A sequence's events, its curves and its notes' curves share one id space,
//! and a view names each of them by its id: a roll draws a curve under its
//! id and reports its points under it, an edit names an event by it, a slur
//! and a join name the notes they tie by it. So an id names one thing, and
//! two things under one id are drawn as one and edited into each other.
//!
//! **What the sequence makes, it mints.** Every id the sequence gives out comes
//! from its counter (`EventSequence::mint`), which only climbs and is saved
//! with it, so an id is never handed out twice, not even to something that
//! comes back after its first holder went. Nothing in this crate computes an
//! id from what is there.
//!
//! **What comes from outside is taken in, never trusted.** A file saved by an
//! older writer, a MIDI file, a client's list, a block pasted from another
//! window or another application: each may carry ids that mean something
//! somewhere else, or nothing, or that repeat. They come in through two doors:
//!
//! - **A whole sequence** (read from a file or a client, restored, derived
//!   from a page) is reconciled once, as it comes in ([`EventSequence::repair`]):
//!   an id that names what it named before keeps it, a free one is kept, and
//!   every other is minted again.
//! - **One thing more** (an event added, a curve given) is checked alone
//!   against the space ([`EventSequence::holder_of`]): kept when its id is
//!   free or already its own, minted when it is not, and a curve that names
//!   something of another kind is refused rather than guessed at.
//!
//! So no edit re-reads every id: a whole payload is reconciled because it is
//! whole, and a single one only for itself.
//!
//! **A repeated event id is repaired out loud.** A curve is named only by the
//! views that draw it, so renumbering one changes nothing a reader holds. An
//! event is named by other events (a join, a slur's ends, a selection), and
//! when two share an id a reference to it reads as the first, which is the
//! one that keeps it. The repair says what it renumbered ([`Renumbered`]),
//! and logs it, because that reading is a choice the data did not make.

use std::collections::{HashMap, HashSet};

use super::EventSequence;
use crate::NodeId;

/// **What holds an id** in a sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Holder {
    /// An event.
    Event,
    /// A curve of the sequence.
    Curve,
    /// A curve of the event with this id.
    NoteCurve(u64),
}

impl Holder {
    fn what(self) -> &'static str {
        match self {
            Self::Event => "an event",
            Self::Curve => "a curve of the sequence",
            Self::NoteCurve(_) => "a note's curve",
        }
    }
}

/// **An id taken in that named something else**, and the one it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Renumbered {
    /// The id it came with.
    pub was: u64,
    /// The id it holds now.
    pub now: u64,
    /// What it is.
    pub holder: Holder,
}

/// What a repair says it did, for an answer or a log.
pub fn describe(renumbered: &[Renumbered]) -> Option<String> {
    if renumbered.is_empty() {
        return None;
    }
    let listed: Vec<String> = renumbered
        .iter()
        .map(|r| format!("{} {} -> {}", r.holder.what(), r.was, r.now))
        .collect();
    Some(format!(
        "ids that named something else were given new ones: {}",
        listed.join(", ")
    ))
}

/// Where an id sits, for writing the decision back.
#[derive(Debug, Clone, Copy)]
enum Slot {
    Event(usize),
    Curve(usize),
    NoteCurve(usize, usize),
}

impl EventSequence {
    /// The next id, from the counter. The only place an id is made.
    pub(super) fn mint(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Every id the sequence holds and what holds it: the first holder,
    /// where a sequence not yet repaired has two.
    pub fn holders(&self) -> HashMap<u64, Holder> {
        let mut out = HashMap::new();
        for (id, holder) in self.slots().into_iter().map(|(_, id, h)| (id, h)) {
            if id != 0 {
                out.entry(id).or_insert(holder);
            }
        }
        out
    }

    /// What holds `id`, if anything does. One pass, nothing built: what a
    /// single thing coming in is checked against.
    pub fn holder_of(&self, id: u64) -> Option<Holder> {
        if id == 0 {
            return None;
        }
        if self.events.iter().any(|e| e.id == id) {
            return Some(Holder::Event);
        }
        if self.automation.iter().any(|a| a.id.0 == id) {
            return Some(Holder::Curve);
        }
        self.events
            .iter()
            .find(|e| e.automation.iter().any(|a| a.id.0 == id))
            .map(|e| Holder::NoteCurve(e.id))
    }

    /// Every id in the sequence, in a fixed order: the events, the
    /// sequence's curves, then each event's curves.
    fn slots(&self) -> Vec<(Slot, u64, Holder)> {
        let mut out: Vec<(Slot, u64, Holder)> = self
            .events
            .iter()
            .enumerate()
            .map(|(i, e)| (Slot::Event(i), e.id, Holder::Event))
            .collect();
        out.extend(
            self.automation
                .iter()
                .enumerate()
                .map(|(j, a)| (Slot::Curve(j), a.id.0, Holder::Curve)),
        );
        for (i, event) in self.events.iter().enumerate() {
            out.extend(
                event
                    .automation
                    .iter()
                    .enumerate()
                    .map(|(k, a)| (Slot::NoteCurve(i, k), a.id.0, Holder::NoteCurve(event.id))),
            );
        }
        out
    }

    /// **A whole sequence reconciled as it comes in**: every id unique, none
    /// zero, and the counter past all of them.
    ///
    /// **The events choose first**, because an event is named by other events
    /// (a join, a slur's ends) and a curve only by the views that draw it: an
    /// event keeps its id unless an event before it took it (one that held
    /// it in `prior` first, then in order), and a curve keeps its id only
    /// where no event took it and `prior` held it as nothing else, else it is
    /// minted.
    ///
    /// `prior` is what held each id before the sequence was replaced, when it
    /// replaced one, so what was already here is what stays itself; an id the
    /// counter handed out once and whose holder went is free and may come
    /// back. With no `prior`, the first holder of an id keeps it.
    ///
    /// A note's curve is matched to its event by the id the event came with.
    pub fn repair(&mut self, prior: Option<&HashMap<u64, Holder>>) -> Vec<Renumbered> {
        let slots = self.slots();
        let highest = slots.iter().map(|(_, id, _)| *id).max().unwrap_or(0);
        self.next_id = self.next_id.max(highest);
        let mut claimed: HashSet<u64> = HashSet::with_capacity(slots.len());
        let mut kept = vec![false; slots.len()];
        let was = |id: &u64| prior.and_then(|prior| prior.get(id)).copied();
        let mut claim = |n: usize, id: u64, kept: &mut Vec<bool>| {
            if id != 0 && !kept[n] && claimed.insert(id) {
                kept[n] = true;
            }
        };
        let is_event = |holder: &Holder| *holder == Holder::Event;
        // the events: what was an event here, then in order
        for (n, (_, id, holder)) in slots.iter().enumerate() {
            if is_event(holder) && was(id) == Some(Holder::Event) {
                claim(n, *id, &mut kept);
            }
        }
        for (n, (_, id, holder)) in slots.iter().enumerate() {
            if is_event(holder) {
                claim(n, *id, &mut kept);
            }
        }
        // the curves: what was the same curve here, then what is free
        for (n, (_, id, holder)) in slots.iter().enumerate() {
            if !is_event(holder) && was(id) == Some(*holder) {
                claim(n, *id, &mut kept);
            }
        }
        for (n, (_, id, holder)) in slots.iter().enumerate() {
            if !is_event(holder) && was(id).is_none() {
                claim(n, *id, &mut kept);
            }
        }
        let mut renumbered = Vec::new();
        for (n, (slot, id, holder)) in slots.into_iter().enumerate() {
            if kept[n] {
                continue;
            }
            let now = self.mint();
            if id != 0 {
                renumbered.push(Renumbered {
                    was: id,
                    now,
                    holder,
                });
            }
            match slot {
                Slot::Event(i) => self.events[i].id = now,
                Slot::Curve(j) => self.automation[j].id = NodeId(now),
                Slot::NoteCurve(i, k) => self.events[i].automation[k].id = NodeId(now),
            }
        }
        if let Some(said) = describe(&renumbered) {
            tracing::warn!("a sequence taken in: {said}");
        }
        renumbered
    }

    /// **The events renamed as `renamed` says, and every reference to one
    /// with them**: a join (`joins`), and what the `notation` section names by
    /// event: an item's event (`items`), a spanner's ends (`spanners`), a
    /// control's note (`controls`), a mark's note (`marks`). An id the map
    /// does not name is left as it is. Every name changes at once, so a map
    /// that swaps two ids swaps them.
    #[cfg(feature = "notation")]
    pub(super) fn rename_events(&mut self, renamed: &HashMap<u64, u64>) {
        let to = |id: u64| renamed.get(&id).copied().unwrap_or(id);
        for event in &mut self.events {
            event.id = to(event.id);
            if let serde_json::Value::Object(keys) = &mut event.data.0
                && let Some(joins) = keys.get("joins").and_then(serde_json::Value::as_u64)
            {
                keys.insert("joins".into(), serde_json::json!(to(joins)));
            }
        }
        let Some(serde_json::Value::Object(section)) = self.notation.as_mut() else {
            return;
        };
        let name = |value: &mut serde_json::Value| {
            if let Some(id) = value.as_u64() {
                *value = serde_json::json!(to(id));
            }
        };
        for (part, ends) in [
            ("items", &[][..]),
            ("marks", &[][..]),
            ("spanners", &["from", "to"][..]),
            ("controls", &["on"][..]),
        ] {
            let Some(serde_json::Value::Array(list)) = section.get_mut(part) else {
                continue;
            };
            for entry in list {
                if ends.is_empty() {
                    if let Some(first) = entry.get_mut(0) {
                        name(first);
                    }
                } else {
                    for end in ends {
                        if let Some(value) = entry.get_mut(*end) {
                            name(value);
                        }
                    }
                }
            }
        }
    }

    /// **One event coming in**: its id kept when nothing else holds it, else
    /// minted, and each of its curves the same against the space and against
    /// each other. What [`super::EventsIntent::Add`] takes an event in by.
    pub(super) fn take_in_event(&mut self, event: &mut super::Event) {
        if event.id == 0 || self.holder_of(event.id).is_some() {
            event.id = self.mint();
        } else {
            self.next_id = self.next_id.max(event.id);
        }
        let mut own: HashSet<u64> = HashSet::new();
        for curve in &mut event.automation {
            let id = curve.id.0;
            if id == 0 || self.holder_of(id).is_some() || id == event.id || !own.insert(id) {
                curve.id = NodeId(self.mint());
                own.insert(curve.id.0);
            } else {
                self.next_id = self.next_id.max(id);
            }
        }
    }

    /// **One curve coming in**, to be held by `holder`: its id when that is
    /// free or already this holder's, a new one when it has none, and a
    /// refusal when the id names something of another kind: the caller
    /// meant a thing this sequence holds as something else, and guessing
    /// which would edit the wrong one.
    pub(super) fn take_in_curve(&mut self, id: NodeId, holder: Holder) -> Result<NodeId, String> {
        if id.0 == 0 {
            return Ok(NodeId(self.mint()));
        }
        match self.holder_of(id.0) {
            None => {
                self.next_id = self.next_id.max(id.0);
                Ok(id)
            }
            Some(held) if held == holder => Ok(id),
            Some(held) => Err(format!(
                "id {} is {} here, not {}",
                id.0,
                held.what(),
                holder.what()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::events::{Event, EventsIntent};

    /// Every id the sequence holds, sorted, with its repeats.
    fn ids(s: &EventSequence) -> Vec<u64> {
        let mut out: Vec<u64> = s.slots().into_iter().map(|(_, id, _)| id).collect();
        out.sort_unstable();
        out
    }

    fn unique(s: &EventSequence) -> bool {
        let all = ids(s);
        let mut once = all.clone();
        once.dedup();
        all == once && !all.contains(&0)
    }

    fn curve(id: u64, target: serde_json::Value) -> serde_json::Value {
        json!({"id": id, "target": target, "points": [{"at": 0.0, "value": 0.0}]})
    }

    /// **A file whose ids repeat is repaired as it is read**: the first event
    /// of an id keeps it, a curve gives way to an event, and the counter goes
    /// past everything, so the next id minted is new.
    #[test]
    fn a_sequence_read_with_repeated_ids_is_repaired() {
        let written = json!({
            "events": [
                {"id": 1, "at": 0.0, "data": {"midinote": 60},
                 "automation": [curve(3, json!({"bend": true}))]},
                {"id": 1, "at": 1.0, "data": {"midinote": 62},
                 "automation": [curve(3, json!({"bend": true}))]},
                {"id": 2, "at": 2.0, "data": {"midinote": 64}}
            ],
            "automation": [curve(2, json!({"cc": 7}))],
            "next_id": 1
        });
        let sequence: EventSequence = serde_json::from_value(written).unwrap();
        assert!(unique(&sequence), "{:?}", ids(&sequence));
        assert_eq!(sequence.events[0].id, 1, "the first keeps it");
        assert_eq!(sequence.events[2].id, 2, "an event keeps it from a curve");
        assert_eq!(sequence.events[0].automation[0].id.0, 3);
        assert!(sequence.next_id >= *ids(&sequence).last().unwrap());
    }

    /// **A list that says one event twice** (what a view that copied a note
    /// with its id would send) leaves two events with two ids, the second's
    /// curves renumbered with it, and the answer says so.
    #[test]
    fn a_list_that_says_an_id_twice_leaves_two_ids() {
        let mut sequence = EventSequence::new(vec![Event::new(0.0, json!({"midinote": 60}))]);
        let id = sequence.events[0].id;
        sequence
            .edit(EventsIntent::EventAutomation {
                id,
                automation: serde_json::from_value(curve(0, json!({"pressure": true}))).unwrap(),
            })
            .unwrap();
        let mut copy = sequence.events[0].clone();
        copy.at = crate::timebase::Beat(2.0);
        let events = vec![sequence.events[0].clone(), copy];
        let change = sequence.edit(EventsIntent::SetEvents { events }).unwrap();
        assert_eq!(sequence.events.len(), 2);
        assert!(unique(&sequence), "{:?}", ids(&sequence));
        assert_eq!(sequence.events[0].id, id, "the one that was here stays");
        assert_eq!(change.renumbered.len(), 2, "the copy and its curve");
        assert!(describe(&change.renumbered).is_some());
    }

    /// **A block pasted from somewhere else** carries ids that mean something
    /// there: added here, it takes ids of its own, its curves too, and what
    /// was here keeps every id it had.
    #[test]
    fn a_pasted_event_takes_ids_of_its_own() {
        let mut sequence = EventSequence::new(vec![Event::new(0.0, json!({"midinote": 60}))]);
        let held = sequence.events[0].id;
        sequence
            .edit(EventsIntent::Automation {
                automation: serde_json::from_value(curve(0, json!({"cc": 1}))).unwrap(),
            })
            .unwrap();
        let before = ids(&sequence);
        // from another sequence: its event is 1, its curve is 2, both here
        let mut pasted: Event = serde_json::from_value(json!({
            "id": held, "at": 4.0, "data": {"midinote": 67},
            "automation": [curve(before[1], json!({"bend": true})),
                           curve(before[1], json!({"pressure": true}))]
        }))
        .unwrap();
        pasted.automation[1].id = pasted.automation[0].id;
        let change = sequence.edit(EventsIntent::Add { event: pasted }).unwrap();
        assert!(unique(&sequence), "{:?}", ids(&sequence));
        assert_ne!(change.added, Some(held));
        let kept: Vec<u64> = ids(&sequence)
            .into_iter()
            .filter(|id| before.contains(id))
            .collect();
        assert_eq!(kept, before, "what was here is as it was");
    }

    /// **A curve that names something of another kind is refused**: a
    /// sequence's curve under an event's id, or a note's under the
    /// sequence's, is a caller meaning a thing this sequence holds as
    /// something else.
    #[test]
    fn a_curve_under_another_kind_s_id_is_refused() {
        let mut sequence = EventSequence::new(vec![Event::new(0.0, json!({"midinote": 60}))]);
        let event = sequence.events[0].id;
        let on_event = sequence.edit(EventsIntent::Automation {
            automation: serde_json::from_value(curve(event, json!({"cc": 1}))).unwrap(),
        });
        assert!(on_event.is_err(), "an event's id");
        let made = sequence
            .edit(EventsIntent::Automation {
                automation: serde_json::from_value(curve(0, json!({"cc": 1}))).unwrap(),
            })
            .unwrap()
            .added
            .unwrap();
        let on_curve = sequence.edit(EventsIntent::EventAutomation {
            id: event,
            automation: serde_json::from_value(curve(made, json!({"bend": true}))).unwrap(),
        });
        assert!(on_curve.is_err(), "the sequence's curve");
        assert!(unique(&sequence));
    }

    /// **An id stated that is free is kept, and the counter goes past it**,
    /// so what is minted next is not that id.
    #[test]
    fn a_free_id_is_kept_and_the_counter_passes_it() {
        let mut sequence = EventSequence::new(vec![Event::new(0.0, json!({"midinote": 60}))]);
        let change = sequence
            .edit(EventsIntent::Automation {
                automation: serde_json::from_value(curve(40, json!({"cc": 1}))).unwrap(),
            })
            .unwrap();
        assert_eq!(change.added, Some(40));
        let next = sequence
            .edit(EventsIntent::Add {
                event: Event::new(1.0, json!({"midinote": 62})),
            })
            .unwrap()
            .added
            .unwrap();
        assert!(next > 40);
    }

    /// **A whole sequence handed over is repaired**, and one the sequence
    /// held comes back exactly as it was.
    #[test]
    fn a_sequence_restored_whole_says_each_id_once() {
        let mut sequence = EventSequence::new(vec![
            Event::new(0.0, json!({"midinote": 60})),
            Event::new(1.0, json!({"midinote": 62})),
        ]);
        let held = sequence.clone();
        let mut handed = held.clone();
        handed.events[1].id = handed.events[0].id;
        let change = sequence
            .edit(EventsIntent::Restore {
                sequence: Box::new(handed),
            })
            .unwrap();
        assert!(unique(&sequence));
        assert_eq!(change.renumbered.len(), 1);
        sequence
            .edit(EventsIntent::Restore {
                sequence: Box::new(held.clone()),
            })
            .unwrap();
        assert_eq!(
            ids(&sequence),
            ids(&held),
            "its own state comes back as it was"
        );
    }

    /// **The MIDI files read from outside** go through the same door: what a
    /// file holds has no ids, and every one it gets is unique.
    #[test]
    fn a_midi_file_read_has_every_id_once() {
        let events = vec![
            (0, vec![0x90, 60, 100]),
            (0, vec![0xE0, 0, 0x50]),
            (480, vec![0x80, 60, 0]),
        ];
        let sequence = EventSequence::from_midi(480, &events, &[]).unwrap();
        assert!(unique(&sequence), "{:?}", ids(&sequence));
    }
}
