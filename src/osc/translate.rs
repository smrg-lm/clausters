//! OSC message → engine command translation, shared between the real-time
//! server ([`crate::osc::server`]) and the NRT renderer
//! ([`crate::server::render`]).
//!
//! [`CmdTranslator`] owns everything that turning a message into fully built
//! [`Cmd`]s requires: the def tables, the node→def mirror that resolves
//! `/n_set` control names, and the auto node-ID counter. It covers the
//! schedulable subset of the protocol (`/s_new`, node/group commands,
//! `/c_set`) plus the synchronous def-table commands (`/d_recv`, `/d_free`).
//! Buffer commands parse into NRT jobs with [`parse_buffer_msg`].

use std::collections::HashMap;
use std::sync::Arc;

use rosc::OscType;

#[cfg(feature = "faust")]
use crate::faust::synth::{FaustDef, FaustSynth};
use crate::dsp::buffer::{Buffer, BufferPool, NUM_BUFFERS};
use crate::node::{AddAction, Group, Place, SynthNode};
use crate::server::engine::Cmd;
use crate::server::nrt::NrtJob;
use crate::synthdef::instance::UGenSynth;
use crate::synthdef::{SynthDef, SynthDefSpec, compile, default_spec};

/// Auto-assigned node IDs (`/s_new` with ID -1) start above this.
const AUTO_NODE_ID_BASE: i32 = 2_000_000;

/// What a live node was built from, mirrored per node ID so `/n_set` can
/// resolve control names off the audio thread.
#[derive(Clone)]
pub enum NodeDef {
    UGen(Arc<SynthDef>),
    #[cfg(feature = "faust")]
    Faust(Arc<FaustDef>),
}

impl NodeDef {
    pub fn control_index(&self, name: &str) -> Option<u32> {
        match self {
            NodeDef::UGen(def) => def.control_index(name),
            #[cfg(feature = "faust")]
            NodeDef::Faust(def) => def.control_index(name),
        }
    }
}

pub struct CmdTranslator {
    /// Faust instances bake the sample rate in at `/s_new` time.
    #[cfg_attr(not(feature = "faust"), allow(dead_code))]
    sample_rate: f32,
    /// Loaded SynthDefs; starts with the built-in "default".
    pub defs: HashMap<String, Arc<SynthDef>>,
    /// Mirror of which def each live node was built from. Maintained from
    /// `/s_new` and from collected garbage (see [`CmdTranslator::forget_node`]).
    pub node_defs: HashMap<i32, NodeDef>,
    next_auto_id: i32,
    /// Compiled Faust defs by name, refcounted (every instance holds a clone).
    #[cfg(feature = "faust")]
    pub faust_defs: HashMap<String, Arc<FaustDef>>,
}

impl CmdTranslator {
    pub fn new(sample_rate: f32) -> Self {
        let mut defs = HashMap::new();
        let default = compile(default_spec()).expect("built-in default def must compile");
        defs.insert(default.name.clone(), Arc::new(default));
        Self {
            sample_rate,
            defs,
            node_defs: HashMap::new(),
            next_auto_id: AUTO_NODE_ID_BASE,
            #[cfg(feature = "faust")]
            faust_defs: HashMap::new(),
        }
    }

    /// Total defs of both families, for `/status.reply`.
    pub fn def_count(&self) -> usize {
        #[allow(unused_mut)]
        let mut n = self.defs.len();
        #[cfg(feature = "faust")]
        {
            n += self.faust_defs.len();
        }
        n
    }

    /// Builds a synth instance from either def table. Faust instantiation
    /// (`createCDSPInstance` + `init`) allocates — fine, this never runs on
    /// the audio thread; the boxed instance reaches it fully built.
    pub fn make_synth(&self, name: &str) -> Result<(Box<dyn SynthNode>, NodeDef), String> {
        if let Some(def) = self.defs.get(name) {
            let synth = Box::new(UGenSynth::new(Arc::clone(def)));
            return Ok((synth, NodeDef::UGen(Arc::clone(def))));
        }
        #[cfg(feature = "faust")]
        if let Some(def) = self.faust_defs.get(name) {
            let synth = FaustSynth::new(Arc::clone(def), self.sample_rate)?;
            return Ok((Box::new(synth), NodeDef::Faust(Arc::clone(def))));
        }
        Err(format!("SynthDef not found: {name}"))
    }

    /// Drops the node→def mirror entry of a freed node.
    pub fn forget_node(&mut self, id: i32) {
        self.node_defs.remove(&id);
    }

    /// `/d_recv`: compile a SynthDef JSON blob into the def table.
    pub fn d_recv(&mut self, args: &[OscType]) -> Result<(), String> {
        let bytes: &[u8] = match args.first() {
            Some(OscType::Blob(b)) => b,
            Some(OscType::String(s)) => s.as_bytes(),
            _ => return Err("expected a JSON blob or string".into()),
        };
        let spec: SynthDefSpec =
            serde_json::from_slice(bytes).map_err(|e| format!("invalid JSON: {e}"))?;
        let def = compile(spec)?;
        self.defs.insert(def.name.clone(), Arc::new(def));
        Ok(())
    }

    /// `/d_free name...`. Live synths keep their `Arc<SynthDef>`: scsynth
    /// semantics. Same for Faust factories (instances refcount them).
    pub fn d_free(&mut self, args: &[OscType]) -> Result<(), String> {
        for arg in args {
            let OscType::String(name) = arg else {
                return Err("expected synthdef names".into());
            };
            self.defs.remove(name);
            #[cfg(feature = "faust")]
            self.faust_defs.remove(name);
        }
        Ok(())
    }

    /// Translates one schedulable message into commands, appending to `cmds`.
    /// Everything allocating (boxed synths, name resolution) happens now;
    /// nothing reaches the engine until the caller ships the batch.
    pub fn translate(&mut self, msg: &rosc::OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        match msg.addr.as_str() {
            "/s_new" => {
                let [
                    OscType::String(name),
                    OscType::Int(id),
                    OscType::Int(action),
                    OscType::Int(target),
                    rest @ ..,
                ] = msg.args.as_slice()
                else {
                    return Err("expected: name, id, addAction, targetID".into());
                };
                let (mut synth, def) = self.make_synth(name)?;
                let action = AddAction::from_i32(*action).ok_or("add action must be 0-4")?;
                let id = if *id == -1 {
                    self.next_auto_id += 1;
                    self.next_auto_id
                } else if *id > 0 {
                    *id
                } else {
                    return Err("node ID must be positive or -1".into());
                };
                for pair in rest.chunks(2) {
                    if let (Some(index), Some(value)) = (
                        control_key(&pair[0], &def),
                        pair.get(1).and_then(float_value),
                    ) {
                        synth.set_control(index, value);
                    }
                }
                self.node_defs.insert(id, def);
                cmds.push(Cmd::AddSynth {
                    id,
                    target: *target,
                    action,
                    synth,
                });
                Ok(())
            }
            "/n_set" => {
                let Some(OscType::Int(id)) = msg.args.first() else {
                    return Err("expected: id, then control/value pairs".into());
                };
                let def = self
                    .node_defs
                    .get(id)
                    .cloned()
                    .ok_or_else(|| format!("node {id} not found"))?;
                for pair in msg.args[1..].chunks(2) {
                    if let (Some(index), Some(value)) = (
                        control_key(&pair[0], &def),
                        pair.get(1).and_then(float_value),
                    ) {
                        cmds.push(Cmd::SetControl {
                            id: *id,
                            index,
                            value,
                        });
                    }
                }
                Ok(())
            }
            "/n_free" => {
                for arg in &msg.args {
                    let OscType::Int(id) = arg else {
                        return Err("expected int node IDs".into());
                    };
                    cmds.push(Cmd::FreeNode { id: *id });
                }
                Ok(())
            }
            "/n_before" | "/n_after" => {
                let place = if msg.addr == "/n_before" {
                    Place::Before
                } else {
                    Place::After
                };
                for pair in msg.args.chunks(2) {
                    let [OscType::Int(id), OscType::Int(target)] = pair else {
                        return Err("expected int (nodeID, targetID) pairs".into());
                    };
                    cmds.push(Cmd::MoveNode {
                        id: *id,
                        target: *target,
                        place,
                    });
                }
                Ok(())
            }
            "/g_new" => {
                for triple in msg.args.chunks(3) {
                    let [OscType::Int(id), OscType::Int(action), OscType::Int(target)] = triple
                    else {
                        return Err("expected int (id, addAction, targetID) triples".into());
                    };
                    let action = AddAction::from_i32(*action).ok_or("add action must be 0-4")?;
                    cmds.push(Cmd::AddGroup {
                        id: *id,
                        target: *target,
                        action,
                        group: Group::new(),
                    });
                }
                Ok(())
            }
            "/g_freeAll" | "/g_deepFree" => {
                for arg in &msg.args {
                    let OscType::Int(id) = arg else {
                        return Err("expected int group IDs".into());
                    };
                    cmds.push(if msg.addr == "/g_freeAll" {
                        Cmd::FreeAllInGroup { id: *id }
                    } else {
                        Cmd::DeepFreeGroup { id: *id }
                    });
                }
                Ok(())
            }
            // The immediate form writes the shared atomics on the network
            // thread, but a scheduled write must land at its exact sample on
            // the engine.
            "/c_set" => {
                for pair in msg.args.chunks(2) {
                    let (OscType::Int(index), Some(value)) = (&pair[0], float_value(&pair[1]))
                    else {
                        return Err("expected (busIndex, value) pairs".into());
                    };
                    if *index < 0 {
                        return Err("bus index must be non-negative".into());
                    }
                    cmds.push(Cmd::SetControlBus {
                        index: *index as usize,
                        value,
                    });
                }
                Ok(())
            }
            other => Err(format!("{other} cannot be scheduled in a timed bundle")),
        }
    }
}

/// Parses one `/b_*` command (except the synchronous `/b_query`) into the
/// buffer index and the NRT job that performs it. `mirror` is the
/// network-side pool: commands that keep or reuse the current contents
/// (`/b_read`, `/b_write`, `/b_zero`) read shape and data from it.
pub fn parse_buffer_msg(
    addr: &str,
    args: &[OscType],
    mirror: &BufferPool,
    default_sample_rate: f64,
) -> Result<(i32, NrtJob), String> {
    let (index, job) = match addr {
        "/b_alloc" => {
            let (index, frames) = match args {
                [OscType::Int(index), OscType::Int(frames), ..] => (*index, *frames),
                _ => return Err("expected: bufnum, frames [, channels]".into()),
            };
            let channels = int_arg(args, 2).unwrap_or(1);
            if frames <= 0 || channels <= 0 {
                return Err("frames and channels must be positive".into());
            }
            (
                index,
                NrtJob::Alloc {
                    frames: frames as usize,
                    channels: channels as usize,
                    sample_rate: default_sample_rate,
                },
            )
        }
        "/b_allocRead" => {
            let (index, path) = match args {
                [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
                _ => return Err("expected: bufnum, path [, fileStart, numFrames]".into()),
            };
            (
                index,
                NrtJob::AllocRead {
                    path,
                    file_start: int_arg(args, 2).unwrap_or(0).max(0) as usize,
                    num_frames: int_arg(args, 3).unwrap_or(0) as i64,
                },
            )
        }
        // `leaveOpen` is accepted and ignored (no streaming yet). The buffer
        // must already exist; its shape is kept.
        "/b_read" => {
            let (index, path) = match args {
                [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
                _ => return Err("expected: bufnum, path [, fileStart, numFrames, bufStart]".into()),
            };
            let Some(current) = mirror_buffer(mirror, index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            (
                index,
                NrtJob::Read {
                    path,
                    file_start: int_arg(args, 2).unwrap_or(0).max(0) as usize,
                    num_frames: int_arg(args, 3).unwrap_or(-1) as i64,
                    buf_start: int_arg(args, 4).unwrap_or(0).max(0) as usize,
                    current,
                },
            )
        }
        // WAV only in v1.
        "/b_write" => {
            let (index, path) = match args {
                [OscType::Int(index), OscType::String(path), ..] => (*index, path.clone()),
                _ => {
                    return Err(
                        "expected: bufnum, path [, headerFormat, sampleFormat, numFrames, startFrame]"
                            .into(),
                    );
                }
            };
            let header = string_arg(args, 2).unwrap_or("wav");
            if !header.eq_ignore_ascii_case("wav") && !header.eq_ignore_ascii_case("wave") {
                return Err(format!("unsupported header format {header:?}"));
            }
            let Some(buffer) = mirror_buffer(mirror, index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            (
                index,
                NrtJob::Write {
                    path,
                    sample_format: string_arg(args, 3).unwrap_or("int16").to_string(),
                    num_frames: int_arg(args, 4).unwrap_or(-1) as i64,
                    buf_start: int_arg(args, 5).unwrap_or(0).max(0) as usize,
                    buffer,
                },
            )
        }
        // Buffers are immutable: zeroing builds a same-shape replacement.
        "/b_zero" => {
            let Some(OscType::Int(index)) = args.first() else {
                return Err("expected a buffer index".into());
            };
            let Some(current) = mirror_buffer(mirror, *index) else {
                return Err(format!("no buffer allocated at {index}"));
            };
            (
                *index,
                NrtJob::Alloc {
                    frames: current.frames(),
                    channels: current.channels(),
                    sample_rate: current.sample_rate(),
                },
            )
        }
        "/b_free" => {
            let Some(OscType::Int(index)) = args.first() else {
                return Err("expected a buffer index".into());
            };
            (*index, NrtJob::Free)
        }
        other => return Err(format!("{other} is not a buffer command")),
    };
    if index < 0 || index as usize >= NUM_BUFFERS {
        return Err(format!("buffer index out of range: {index}"));
    }
    Ok((index, job))
}

/// `/d_faust name payload` arguments: the payload string is Faust source or
/// a JSON box tree (the caller sniffs the leading `{`).
pub fn parse_d_faust(args: &[OscType]) -> Result<(String, String), String> {
    let (name, def) = match args {
        [OscType::String(name), OscType::String(src), ..] => (name.clone(), src.clone()),
        [OscType::String(name), OscType::Blob(src), ..] => (
            name.clone(),
            String::from_utf8(src.clone()).map_err(|_| "def blob is not UTF-8".to_string())?,
        ),
        _ => return Err("expected: name, JSON or Faust source".into()),
    };
    if name.is_empty() {
        return Err("empty def name".into());
    }
    Ok((name, def))
}

fn mirror_buffer(mirror: &BufferPool, index: i32) -> Option<Arc<Buffer>> {
    usize::try_from(index)
        .ok()
        .and_then(|i| mirror.get(i))
        .and_then(|b| b.as_ref().map(Arc::clone))
}

/// Control reference: by name (resolved against the def) or by index.
pub fn control_key(arg: &OscType, def: &NodeDef) -> Option<u32> {
    match arg {
        OscType::String(name) => def.control_index(name),
        OscType::Int(i) if *i >= 0 => Some(*i as u32),
        _ => None,
    }
}

/// Optional trailing int argument (scsynth buffer commands have several).
pub fn int_arg(args: &[OscType], n: usize) -> Option<i32> {
    match args.get(n) {
        Some(OscType::Int(i)) => Some(*i),
        _ => None,
    }
}

pub fn string_arg(args: &[OscType], n: usize) -> Option<&str> {
    match args.get(n) {
        Some(OscType::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

pub fn float_value(arg: &OscType) -> Option<f32> {
    match arg {
        OscType::Float(f) => Some(*f),
        OscType::Int(i) => Some(*i as f32),
        OscType::Double(d) => Some(*d as f32),
        _ => None,
    }
}
