//! **How a sequence's curves reach its notes**: the graphs they play in, the
//! tables their readers follow, and the slots the lane adds.
//!
//! A curve acts on a channel (a curve of the sequence) or on one note (its own
//! expression), and the two are two graphs one inside the other
//! (`clausters_core::event_graph`): an instance per channel that has a note
//! a curve shapes, reading the channel's curves once, and a slot of it per such
//! note. A note no curve reaches is a plain synth, as it always was.
//!
//! [`plan`] is arithmetic -- which notes play in which graph, and every table,
//! sampled on the transport's frames by the multitrack's own tabulation
//! (`clausters_document::multitrack::nodes::tabulate`). [`NoteCurves`] is what
//! a playback keeps of it: the defs it sent, the instance of each channel and
//! the buffer of each table, and the ops that make the next plan be what is
//! made. A channel's table is given back once its reader has moved to the new
//! one; a note's, once the pass that may sound it is over -- a slot made before
//! an edit reads the table it was made with until its note ends, and a buffer
//! number handed out again at once would give it another curve.
//!
//! A channel's instance outlives an edit the same way. A slot's members are
//! fixed by its def, so a channel that gains or loses a curve, or a shape, is
//! a new graph; the new instance is made beside the old one, which keeps the
//! notes it is sounding -- their releases are the lane's, sent to their own
//! nodes -- and the tables they read, until the next pass gives them back.

use std::collections::{BTreeMap, BTreeSet};

use clausters_core::event::render::Type;
use clausters_core::event_graph::{self, BEND, Shape};
use clausters_core::mixer::{AT, BUF, CURVE_STEP};
use clausters_document::Point;
use clausters_document::multitrack::nodes::{tabulate, value_at};
use serde_json::{Value, json};

use crate::apply::Applier;
use crate::instance::{Handle, Op, Port, Ports};
use crate::notes_playback::{PlacedCurve, Placement, SlotNote, Voice, voice_of};

/// **The control a curve drives**, from its target: `{"control": name}` names
/// it, and wins; `{"bend": ...}` is the pitch ([`BEND`]); `{"pressure": ...}`
/// and `{"timbre": ...}` are `press` and `slide`, the controls an MPE zone
/// drives (and the built-in `default` declares). A bare `{"cc": n}` drives
/// nothing on a synth.
pub fn curve_control(target: &Value) -> Option<String> {
    let target = target.as_object()?;
    if let Some(name) = target.get("control").and_then(Value::as_str) {
        return Some(name.to_string());
    }
    [(BEND, BEND), ("pressure", "press"), ("timbre", "slide")]
        .into_iter()
        .find(|(key, _)| target.contains_key(*key))
        .map(|(_, control)| control.to_string())
}

/// The channel a sequence curve's target names, or `None` for every channel.
fn curve_channel(target: &Value) -> Option<i64> {
    target
        .get("channel")
        .and_then(Value::as_f64)
        .map(|c| c as i64)
}

/// One curve's table, ready for a buffer: `at` is the frame of the transport
/// its first sample is at.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// What it is kept by across plans.
    pub key: String,
    pub at: f64,
    pub table: Vec<f32>,
}

/// One channel's graph: its def, the note graphs its slots are, and the
/// tables of its curves by the control each drives.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelPlan {
    pub key: String,
    pub graph: Value,
    pub notes: Vec<Value>,
    pub curves: Vec<(String, Table)>,
}

/// One note that plays in a graph: its channel (an index into
/// [`CurvePlan::channels`]), its slot, and the tables of its own curves.
#[derive(Clone, Debug, PartialEq)]
pub struct NotePlan {
    pub channel: usize,
    pub slot: String,
    pub curves: Vec<(String, Table)>,
}

/// **Which notes play in which graph**, by the index of their event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurvePlan {
    pub channels: Vec<ChannelPlan>,
    pub notes: Vec<Option<NotePlan>>,
}

impl CurvePlan {
    /// **The plan with its graphs named for one sender**: every note and
    /// channel graph renamed under `scope` ([`event_graph::scoped`]) and every
    /// channel graph's slots pointed at the renamed notes. `scope` is the
    /// node id of the lane the notes play from, which the server holds unique,
    /// so a graph this sender gives back is one no other sender plays.
    pub fn scoped(mut self, scope: i32) -> Self {
        for channel in &mut self.channels {
            for note in &mut channel.notes {
                let name = event_graph::scoped(note["name"].as_str().unwrap_or_default(), scope);
                note["name"] = json!(name);
            }
            if let Some(members) = channel.graph["members"].as_array_mut() {
                for member in members {
                    if let Some(def) = member["def"].as_str() {
                        member["def"] = json!(event_graph::scoped(def, scope));
                    }
                }
            }
            let name =
                event_graph::scoped(channel.graph["name"].as_str().unwrap_or_default(), scope);
            channel.graph["name"] = json!(name);
        }
        self
    }
}

/// The curves `curves` drive, one per control (the first that names it), with
/// points.
fn by_control<'a>(curves: impl Iterator<Item = &'a PlacedCurve>) -> Vec<(String, &'a PlacedCurve)> {
    let mut out: Vec<(String, &PlacedCurve)> = Vec::new();
    for curve in curves {
        if curve.points.is_empty() {
            continue;
        }
        if let Some(control) = curve_control(&curve.target)
            && !out.iter().any(|(c, _)| *c == control)
        {
            out.push((control, curve));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A table from frame `first` to frame `last` of points placed from second
/// `origin`.
fn table(key: String, points: &[Point], first: f64, last: f64, origin: f64, rate: f64) -> Table {
    Table {
        key,
        at: first,
        table: tabulate(first, last, CURVE_STEP, |frame| {
            value_at(points, frame / rate - origin)
        }),
    }
}

/// **The graphs a placement's curves need**, at `rate` frames a second.
///
/// A note plays in a graph when it has a curve of its own or its channel has
/// one: its channel is `scope` and the note's `channel` key, and a curve of
/// the sequence reaches it when it is of the same scope and names that channel
/// or none. A
/// note's own table is in the note's own time, from its start to the later of
/// its release and its last point -- a curve may run into the release -- and a
/// sequence's on the transport's, from its first point to its last; past either
/// end a reader holds.
pub fn plan(placement: &Placement, rate: f64) -> CurvePlan {
    struct Channel {
        curves: Vec<(String, Table)>,
        shapes: Vec<Shape>,
    }
    let mut channels: BTreeMap<String, Channel> = BTreeMap::new();
    // Each note in a graph: its channel's key, its shape's index, its tables.
    type Planned = (String, usize, Vec<(String, Table)>);
    let mut notes: Vec<Option<Planned>> = vec![None; placement.events.len()];
    for (i, event) in placement.events.iter().enumerate() {
        if Type::of(&event.keys) != Type::Note {
            continue;
        }
        let Some(Voice { def, controls, .. }) = voice_of(&event.keys) else {
            continue;
        };
        let channel = event
            .keys
            .get("channel")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as i64;
        let key = format!("{}#{channel}", event.scope);
        let own = by_control(event.curves.iter());
        if !channels.contains_key(&key) {
            let curves = by_control(placement.curves.iter().filter(|curve| {
                curve.scope == event.scope
                    && curve_channel(&curve.target).is_none_or(|c| c == channel)
            }));
            let curves = curves
                .into_iter()
                .map(|(control, curve)| {
                    let points = &curve.points;
                    let first = points[0].at * rate;
                    let last = points[points.len() - 1].at * rate;
                    let key = format!("channel/{key}/{control}");
                    (control, table(key, points, first, last, 0.0, rate))
                })
                .collect();
            channels.insert(
                key.clone(),
                Channel {
                    curves,
                    shapes: Vec::new(),
                },
            );
        }
        let entry = channels.get_mut(&key).expect("inserted above");
        if own.is_empty() && entry.curves.is_empty() {
            continue;
        }
        let mut names: Vec<String> = controls.into_iter().map(|(name, _)| name).collect();
        names.sort();
        names.dedup();
        let shape = Shape {
            def,
            controls: names,
            own: own.iter().map(|(c, _)| c.clone()).collect(),
            channel: entry.curves.iter().map(|(c, _)| c.clone()).collect(),
        };
        let slot = match entry.shapes.iter().position(|s| *s == shape) {
            Some(slot) => slot,
            None => {
                entry.shapes.push(shape);
                entry.shapes.len() - 1
            }
        };
        let id = if event.id.is_empty() {
            i.to_string()
        } else {
            event.id.clone()
        };
        let curves = own
            .into_iter()
            .map(|(control, curve)| {
                let points = &curve.points;
                let reach = event.end.max(event.start + points[points.len() - 1].at);
                let key = format!("note/{id}/{control}");
                // In the note's own time: its reader counts from the sample
                // the note starts on, so the table does too, and a note moved
                // is the same table.
                let t = table(key, points, 0.0, (reach - event.start) * rate, 0.0, rate);
                (control, t)
            })
            .collect();
        notes[i] = Some((key, slot, curves));
    }
    let mut out = CurvePlan::default();
    let mut index = BTreeMap::new();
    for (key, channel) in channels {
        if channel.shapes.is_empty() {
            continue;
        }
        let graphs: Vec<Value> = channel.shapes.iter().map(event_graph::note_graph).collect();
        let controls: Vec<String> = channel.curves.iter().map(|(c, _)| c.clone()).collect();
        index.insert(key.clone(), out.channels.len());
        out.channels.push(ChannelPlan {
            graph: event_graph::channel_graph(&controls, &graphs),
            key,
            notes: graphs,
            curves: channel.curves,
        });
    }
    out.notes = notes
        .into_iter()
        .map(|note| {
            let (key, slot, curves) = note?;
            Some(NotePlan {
                channel: index[&key],
                slot: event_graph::slot_name(slot),
                curves,
            })
        })
        .collect();
    out
}

/// **What a playback keeps of its notes' curves**: the defs it sent, the
/// instance of each channel and the buffer of each table, under handles that
/// begin with its own prefix.
#[derive(Clone, Debug, Default)]
pub struct NoteCurves {
    prefix: String,
    sent: BTreeSet<String>,
    /// The note and channel graphs sent, which are given back once nothing
    /// plays them: as many as the shapes a session ever had, where the
    /// readers in `sent` beside them are four.
    graphs: BTreeSet<String>,
    /// The graphs each instance alive plays: its channel's and its notes'.
    uses: BTreeMap<Handle, Vec<String>>,
    /// Each channel's key, the graph its instance is, and the instance's
    /// generation.
    channels: BTreeMap<String, (String, u32)>,
    /// Instances a channel's graph moved away from, kept for the notes they
    /// are sounding until a new pass starts.
    outlived: Vec<Handle>,
    /// Each table's key, its samples and first frame, and its generation.
    tables: BTreeMap<String, (Vec<f32>, f64, u32)>,
    /// A channel's buffers to give back on the next plan: its reader moved to
    /// the new one when this plan was made.
    retired: Vec<Handle>,
    /// A note's buffers to give back when a new pass starts: a note sounding
    /// now may be reading one until it ends.
    lingering: Vec<Handle>,
}

impl NoteCurves {
    /// Nothing made yet, its handles beginning with `prefix`.
    pub fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
            ..Self::default()
        }
    }

    fn instance(&self, key: &str, generation: u32) -> Handle {
        format!("{}/channel/{key}/{generation}", self.prefix)
    }

    /// A channel's instance now.
    fn current_instance(&self, key: &str) -> Option<Handle> {
        self.channels
            .get(key)
            .map(|(_, generation)| self.instance(key, *generation))
    }

    /// A channel's instance moved away from: kept, with every table it may
    /// read, until a new pass.
    fn outlive(&mut self, key: &str) {
        if let Some(handle) = self.current_instance(key) {
            self.outlived.push(handle);
        }
    }

    fn buffer(&self, key: &str, generation: u32) -> Handle {
        format!("{}/table/{key}/{generation}", self.prefix)
    }

    /// The buffer a table is in now.
    fn current(&self, key: &str) -> Option<Handle> {
        self.tables.get(key).map(|(_, _, g)| self.buffer(key, *g))
    }

    /// A table as `plan` has it: a new buffer when it is new or moved, the one
    /// before it retired. Whether it changed, and its buffer.
    fn table(&mut self, t: &Table, ops: &mut Vec<Op>) -> (bool, Handle) {
        let generation = match self.tables.get(&t.key) {
            Some((samples, at, g)) if *samples == t.table && *at == t.at => {
                return (false, self.buffer(&t.key, *g));
            }
            Some((_, _, g)) => {
                let g = *g;
                let old = self.buffer(&t.key, g);
                self.retire(&t.key, old);
                g + 1
            }
            None => 0,
        };
        let handle = self.buffer(&t.key, generation);
        ops.push(Op::Buffer {
            handle: handle.clone(),
            samples: t.table.clone(),
        });
        self.tables
            .insert(t.key.clone(), (t.table.clone(), t.at, generation));
        (true, handle)
    }

    /// A buffer no table is in any more, kept as long as something may read it
    /// -- a note's until a new pass, and a channel's too while an instance it
    /// was moved away from may still be reading it.
    fn retire(&mut self, key: &str, handle: Handle) {
        if key.starts_with("note/") || !self.outlived.is_empty() {
            self.lingering.push(handle);
        } else {
            self.retired.push(handle);
        }
    }

    /// **The ops that make `plan` be what is made**, the channels' instances
    /// at the tail of `parent`: the defs not sent yet, an instance per channel
    /// (made again when its graph changed), each table's buffer and the ports
    /// of a channel's readers, and what is no longer planned given back --
    /// a note's tables, and a channel's instance its graph moved away from,
    /// only when `pass` says a new pass starts, since until then a note
    /// sounding from before an edit may be in one, or reading one.
    pub fn ops(&mut self, plan: &CurvePlan, parent: &str, pass: bool) -> Vec<Op> {
        let mut ops: Vec<Op> = Vec::new();
        if pass {
            for handle in std::mem::take(&mut self.outlived) {
                self.uses.remove(&handle);
                ops.push(Op::Free {
                    handle,
                    forget: Vec::new(),
                });
            }
        }
        ops.extend(
            self.retired
                .drain(..)
                .map(|handle| Op::FreeBuffer { handle }),
        );
        if pass {
            ops.extend(
                self.lingering
                    .drain(..)
                    .map(|handle| Op::FreeBuffer { handle }),
            );
        }
        let mut defs = Vec::new();
        if !plan.channels.is_empty() {
            for spec in [
                event_graph::curve_def(),
                event_graph::local_def(),
                event_graph::hold_def(),
                event_graph::pitch_def(),
            ] {
                let name = spec["name"].as_str().unwrap_or_default().to_string();
                if self.sent.insert(name) {
                    defs.push(Op::Def {
                        family: "synth".into(),
                        spec,
                    });
                }
            }
        }
        for channel in &plan.channels {
            for spec in channel.notes.iter().chain([&channel.graph]) {
                let name = spec["name"].as_str().unwrap_or_default().to_string();
                if self.sent.insert(name.clone()) {
                    self.graphs.insert(name);
                    defs.push(Op::Def {
                        family: "graph".into(),
                        spec: spec.clone(),
                    });
                }
            }
        }
        if !defs.is_empty() {
            ops.extend(defs);
            ops.push(Op::Barrier);
        }
        let mut fresh = BTreeSet::new();
        for channel in &plan.channels {
            let name = channel.graph["name"].as_str().unwrap_or_default();
            let generation = match self.channels.get(&channel.key) {
                Some((graph, _)) if graph == name => continue,
                // A channel whose graph changed is made again beside the old
                // instance, which keeps sounding what it holds.
                Some((_, generation)) => {
                    let next = generation + 1;
                    self.outlive(&channel.key);
                    next
                }
                None => 0,
            };
            let handle = self.instance(&channel.key, generation);
            let played = std::iter::once(name.to_string())
                .chain(
                    channel
                        .notes
                        .iter()
                        .filter_map(|n| n["name"].as_str().map(str::to_string)),
                )
                .collect();
            self.uses.insert(handle.clone(), played);
            ops.push(Op::Graph {
                handle,
                parent: parent.to_string(),
                graph: name.to_string(),
                ports: Ports::new(),
            });
            self.channels
                .insert(channel.key.clone(), (name.to_string(), generation));
            fresh.insert(channel.key.clone());
        }
        let planned: BTreeSet<&str> = plan.channels.iter().map(|c| c.key.as_str()).collect();
        let gone: Vec<String> = self
            .channels
            .keys()
            .filter(|k| !planned.contains(k.as_str()))
            .cloned()
            .collect();
        // A channel no curve reaches any more: its notes play as plain synths
        // from now on, and the ones it is sounding finish in it.
        for key in gone {
            self.outlive(&key);
            self.channels.remove(&key);
        }
        let mut alive = BTreeSet::new();
        for channel in &plan.channels {
            let mut ports = Ports::new();
            for (control, t) in &channel.curves {
                alive.insert(t.key.clone());
                let (changed, handle) = self.table(t, &mut ops);
                if changed || fresh.contains(&channel.key) {
                    ports.insert(
                        event_graph::channel_port(control, BUF),
                        Port::Buffer { buffer: handle },
                    );
                    ports.insert(event_graph::channel_port(control, AT), Port::Number(t.at));
                    ports.insert(
                        event_graph::channel_port(control, "step"),
                        Port::Number(CURVE_STEP),
                    );
                }
            }
            if !ports.is_empty() {
                ops.push(Op::Set {
                    handle: self.instance(&channel.key, self.channels[&channel.key].1),
                    ports,
                });
            }
        }
        for note in plan.notes.iter().flatten() {
            for (_, t) in &note.curves {
                alive.insert(t.key.clone());
                self.table(t, &mut ops);
            }
        }
        let dead: Vec<String> = self
            .tables
            .keys()
            .filter(|k| !alive.contains(*k))
            .cloned()
            .collect();
        for key in dead {
            if let Some(handle) = self.current(&key) {
                self.retire(&key, handle);
            }
            self.tables.remove(&key);
        }
        // **A graph nothing plays is given back**, once a pass has freed the
        // instances that outlived their channel's last change: what the plan
        // names and what an instance still sounding plays stay.
        if pass {
            let played: BTreeSet<&String> = self.uses.values().flatten().collect();
            let unplayed: Vec<String> = self
                .graphs
                .iter()
                .filter(|name| !played.contains(name))
                .cloned()
                .collect();
            self.give_back(unplayed, &mut ops);
        }
        ops
    }

    /// Frees the graphs `names` and forgets having sent them, so a plan that
    /// names one again sends it again.
    fn give_back(&mut self, names: Vec<String>, ops: &mut Vec<Op>) {
        if names.is_empty() {
            return;
        }
        for name in &names {
            self.graphs.remove(name);
            self.sent.remove(name);
        }
        ops.push(Op::FreeDef { names });
    }

    /// **The slot each note plays in**, once [`Self::ops`] is applied: its
    /// channel's instance, its slot, and its readers' ports -- a table's
    /// buffer, the frame it starts at, the frames a sample covers.
    pub fn slots(&self, plan: &CurvePlan, applier: &Applier) -> Vec<Option<SlotNote>> {
        plan.notes
            .iter()
            .map(|note| {
                let note = note.as_ref()?;
                let channel = plan.channels.get(note.channel)?;
                let graph = applier.node(&self.current_instance(&channel.key)?)?;
                let mut ports = Vec::new();
                for (control, t) in &note.curves {
                    let buffer = applier.buffer(&self.current(&t.key)?)?;
                    ports.push((event_graph::curve_port(control, BUF), f64::from(buffer)));
                    ports.push((event_graph::curve_port(control, "step"), CURVE_STEP));
                }
                Some(SlotNote {
                    graph,
                    slot: note.slot.clone(),
                    ports,
                })
            })
            .collect()
    }

    /// **Everything given back**, for a playback that goes away: each
    /// channel's instance freed (and its notes with it), the ones outlived
    /// too, the graphs they played, then every buffer.
    pub fn teardown(&mut self) -> Vec<Op> {
        let current: Vec<Handle> = self
            .channels
            .keys()
            .filter_map(|key| self.current_instance(key))
            .collect();
        let mut ops: Vec<Op> = current
            .into_iter()
            .chain(self.outlived.drain(..))
            .map(|handle| Op::Free {
                handle,
                forget: Vec::new(),
            })
            .collect();
        self.channels.clear();
        self.uses.clear();
        let graphs: Vec<String> = self.graphs.iter().cloned().collect();
        self.give_back(graphs, &mut ops);
        ops.extend(
            self.retired
                .drain(..)
                .chain(self.lingering.drain(..))
                .map(|handle| Op::FreeBuffer { handle }),
        );
        let keys: Vec<String> = self.tables.keys().cloned().collect();
        for key in keys {
            if let Some(handle) = self.current(&key) {
                ops.push(Op::FreeBuffer { handle });
            }
        }
        self.tables.clear();
        ops
    }
}

#[cfg(test)]
mod tests;
