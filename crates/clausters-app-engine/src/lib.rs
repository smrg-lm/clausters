//! **The engine an editing application holds.**
//!
//! An editing application (the audio editor, the multitrack) is one program:
//! its windows, its domain and an engine of its own, in one process. This
//! crate is the engine half: the server's engine and command translator taken
//! as a library, spoken to with **built commands** pushed through the
//! engine's lock-free FIFO, never with OSC. There is no socket, no ring of
//! encoded packets and no network thread; the application is the only one
//! that talks to it.
//!
//! - [`AppEngine`] holds the engine's handle, the translator whose typed doors
//!   build the commands (`CmdTranslator::new_synth`, `new_graph`, ...), and,
//!   with the `realtime` feature, the audio device the engine runs on. Its
//!   [`AppEngine::pump`] takes back what the engine hands over (garbage, the
//!   ends of nodes) on the application's own turn.
//! - [`EngineMedium`] is a playback's medium over it
//!   ([`clausters_editing::apply::Medium`]): a playback written once answers
//!   ops, and this carries them out as commands at once, so it answers no
//!   steps to walk. Each playback has its own, with its own table of what its
//!   handles became, over one shared engine.
//!
//! The engine knows no file, no segment and no peaks: what it is handed is
//! nodes, buses, buffers and transport commands.

pub mod audio;
/// What an engine publishes, read without its handle ([`AppEngine::reading`]).
pub use clausters::server::engine::EngineReading;
/// A sound file's header, through the decoder the clients read it with.
pub use clausters::server::nrt::{SoundInfo, probe_audio};
pub mod held;
pub mod steps;
pub mod take;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use clausters::dsp::buffer::Buffer;
use clausters::node::{AddAction, ROOT_NODE_ID};
use clausters::osc::graphdef::GraphDefSpec;
use clausters::osc::translate::{CmdTranslator, Control};
use clausters::server::engine::{Cmd, EndMark, EngineHandle, Garbage, NodeEventKind};
#[cfg(feature = "synth")]
use clausters::synthdef::SynthDefSpec;
use clausters_core::ids::{IdError, IdSpaces, Space};
use clausters_editing::apply::{MULTITRACK_TRANSPORT, Medium, Step};
use clausters_editing::instance::{Op, Port, Ports};

/// **The engine an application holds**: its handle, the translator that
/// builds its commands, and the device it runs on when it has one.
pub struct AppEngine {
    handle: EngineHandle,
    translator: CmdTranslator,
    sample_rate: f32,
    channels: usize,
    /// **What waits for a cue**: the commands pushed after a transport's cue,
    /// held until the engine says the cue is over, so a play rolls once its
    /// readers hold the frames it starts on -- as a client waits for the
    /// cue's `/done` before it rolls a server's transport. The cued
    /// transport, and the commands after it in the order they were pushed.
    held: VecDeque<(usize, Vec<Cmd>)>,
    /// Whether each transport was last told to roll, and has not stopped on
    /// its end mark since.
    rolling: HashMap<usize, bool>,
    /// The audio device the engine runs on; `None` for an engine driven by
    /// hand, block by block.
    #[cfg(feature = "realtime")]
    _backend: Option<clausters::server::backend::AudioBackend>,
}

impl std::fmt::Debug for AppEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppEngine")
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .finish()
    }
}

impl AppEngine {
    /// An engine over `handle`, driven by whoever holds its other half (an
    /// engine a test processes block by block): nothing is opened.
    pub fn over(handle: EngineHandle) -> AppEngine {
        let sample_rate = handle.sample_rate;
        let channels = handle.channels;
        let mut translator = CmdTranslator::with_buses(
            sample_rate,
            handle.audio_buses,
            clausters::server::engine::DEFAULT_CONTROL_BUSES,
        );
        translator.set_meters(Arc::clone(handle.meters()));
        AppEngine {
            handle,
            translator,
            sample_rate,
            channels,
            held: VecDeque::new(),
            rolling: HashMap::new(),
            #[cfg(feature = "realtime")]
            _backend: None,
        }
    }

    /// **An engine with no device**, at `sample_rate` and `channels` wide,
    /// and the engine's half that processes it, for whoever drives it block
    /// by block (a test).
    pub fn detached(
        sample_rate: f32,
        channels: usize,
    ) -> (AppEngine, clausters::server::engine::Engine) {
        let (engine, handle) = clausters::server::engine::engine_pair(sample_rate, channels);
        (AppEngine::over(handle), engine)
    }

    /// **An engine on the machine's default output device**, at the device's
    /// own rate.
    #[cfg(feature = "realtime")]
    pub fn open() -> Result<AppEngine, String> {
        let (backend, handle) = clausters::server::backend::start(
            0,
            None,
            None,
            clausters::server::engine::DEFAULT_AUDIO_BUSES,
            clausters::server::engine::DEFAULT_CONTROL_BUSES,
            clausters::dsp::Limits::default(),
            None,
            0,
            &clausters::server::backend::Devices::default(),
        )
        .map_err(|e| e.to_string())?;
        let mut engine = AppEngine::over(handle);
        engine._backend = Some(backend);
        Ok(engine)
    }

    /// The engine's rate.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// How many hardware outputs it has.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The engine's handle, for what reads it directly: the transports'
    /// positions, the meters.
    pub fn handle(&self) -> &EngineHandle {
        &self.handle
    }

    /// **What the engine publishes**, for a window that draws its meters and
    /// playheads from another thread.
    pub fn reading(&self) -> clausters::server::engine::EngineReading {
        self.handle.reading()
    }

    /// The translator, for a command the typed doors build that no op names.
    pub fn translator(&mut self) -> &mut CmdTranslator {
        &mut self.translator
    }

    /// **Pushes `cmds` to the engine**, after the latency plan they move.
    /// Answers whether the FIFO took them; a full one drops them, said in the
    /// log, as a full command ring is on a server.
    ///
    /// Commands after a transport's cue wait for the cue to be over
    /// ([`Self::pump`] sends them), and so does everything pushed meanwhile,
    /// so the engine takes them in the order they were pushed.
    pub fn push(&mut self, cmds: Vec<Cmd>) -> bool {
        if cmds.is_empty() {
            return true;
        }
        if let Some((_, waiting)) = self.held.back_mut() {
            waiting.extend(cmds);
            return true;
        }
        self.send(cmds)
    }

    /// Sends `cmds` up to and including the first cue, and holds the rest
    /// under the transport it cues.
    fn send(&mut self, mut cmds: Vec<Cmd>) -> bool {
        let cue = cmds
            .iter()
            .position(|cmd| matches!(cmd, Cmd::TransportCue { .. }));
        if let Some(at) = cue {
            // what follows waits, pushed with the cue or after it
            let rest = cmds.split_off(at + 1);
            if let Some(Cmd::TransportCue { transport }) = cmds.last() {
                self.held.push_back((*transport, rest));
            }
        }
        for cmd in &cmds {
            if let Cmd::TransportRun { transport, rolling } = cmd {
                self.rolling.insert(*transport, *rolling);
            }
        }
        self.translator.prepare_compensation(&mut cmds, 0);
        if cmds.is_empty() {
            return true;
        }
        match self.handle.send_batch(cmds) {
            Ok(()) => true,
            Err(_) => {
                tracing::warn!("app engine: the command FIFO is full; a batch was dropped");
                false
            }
        }
    }

    /// **Whether transport `transport` rolls**, as the application last told
    /// it -- a play waiting on its cue included -- unless it stopped on its
    /// end mark since.
    pub fn rolling(&self, transport: usize) -> bool {
        let waiting = self.held.iter().rev().find_map(|(_, cmds)| {
            cmds.iter().rev().find_map(|cmd| match cmd {
                Cmd::TransportRun {
                    transport: t,
                    rolling,
                } if *t == transport => Some(*rolling),
                _ => None,
            })
        });
        waiting.unwrap_or_else(|| self.rolling.get(&transport).copied().unwrap_or(false))
    }

    /// **Puts a take's fed streams in the pool**, one mono buffer per
    /// channel, `frames` long, at the buffers `buffers` names (allocated by
    /// [`Self::alloc_take`]), so channel `c`'s reader plays `buffers[c]`. Put
    /// again after an edit that changes the take's length: the readers keep
    /// their rings and refill from where they are.
    pub fn install_take(&mut self, feed: &take::TakeFeed, frames: u64, buffers: &[i32]) -> bool {
        let rate = self.sample_rate as f64;
        let cmds = buffers
            .iter()
            .enumerate()
            .filter_map(|(c, &bufnum)| {
                let stream = feed.stream(c)?;
                Some(Cmd::SetBuffer {
                    index: bufnum as usize,
                    buffer: Some(Arc::new(Buffer::fed_by(
                        Arc::clone(stream),
                        1,
                        frames as usize,
                        rate,
                    ))),
                })
            })
            .collect();
        self.push(cmds)
    }

    /// **The buffers a take of `channels` channels is played from**, one per
    /// channel, allocated from `ids`.
    pub fn alloc_take(channels: usize, ids: &mut IdSpaces) -> Result<Vec<i32>, IdError> {
        (0..channels.max(1))
            .map(|_| ids.alloc(Space::Buffers, 1).map(|b| b as i32))
            .collect()
    }

    /// **Takes back what the engine handed over**: freed nodes, buffers and
    /// bundles are dropped here, off the audio thread, and the translator
    /// forgets the nodes that ended. Called on the application's turn.
    pub fn pump(&mut self) {
        while let Some(garbage) = self.handle.pop_garbage() {
            match garbage {
                Garbage::FreedSynth { id, synth, .. } => {
                    self.translator.forget_freed_synth(id, synth.incarnation())
                }
                Garbage::RejectedSynth { id, why, .. } | Garbage::RejectedGroup { id, why, .. } => {
                    self.translator.release_node_id(id);
                    clausters::server::engine::report_rejected(id, why, "app engine");
                }
                Garbage::TransportEnded { transport } => {
                    self.rolling.insert(transport, false);
                }
                // the cue is over: what waited for it goes
                Garbage::TransportCued { transport, .. } => {
                    if self.held.front().is_some_and(|(t, _)| *t == transport)
                        && let Some((_, cmds)) = self.held.pop_front()
                    {
                        let later = std::mem::take(&mut self.held);
                        self.send(cmds);
                        self.held.extend(later);
                    }
                }
                _ => {}
            }
        }
        while let Some(event) = self.handle.pop_event() {
            if event.kind == NodeEventKind::End {
                self.translator.release_node_id(event.id);
            }
        }
    }
}

/// **A playback's medium over an engine the application holds**: each op
/// becomes the engine's commands at once, so nothing is left to walk.
///
/// The table of what this playback's handles became is its own, as an
/// endpoint's [`clausters_editing::apply::Applier`] is, over the one engine
/// every playback of the application shares.
#[derive(Debug, Clone)]
pub struct EngineMedium {
    engine: Arc<Mutex<AppEngine>>,
    nodes: HashMap<String, i32>,
    buses: HashMap<String, (i32, usize)>,
    audio: HashSet<String>,
    buffers: HashMap<String, i32>,
}

impl EngineMedium {
    /// A medium over `engine` that has made nothing yet.
    pub fn new(engine: Arc<Mutex<AppEngine>>) -> EngineMedium {
        EngineMedium {
            engine,
            nodes: HashMap::new(),
            buses: HashMap::new(),
            audio: HashSet::new(),
            buffers: HashMap::new(),
        }
    }

    /// The node a handle became.
    pub fn node(&self, handle: &str) -> Option<i32> {
        self.nodes.get(handle).copied()
    }

    /// The ports as the controls a typed door writes, with buses and buffers
    /// resolved through the table; a port naming nothing made is left out.
    fn ports(&self, ports: &Ports) -> Vec<(String, f32)> {
        ports
            .iter()
            .filter_map(|(name, port)| {
                let value = match port {
                    Port::Number(value) => *value as f32,
                    Port::Bus { bus, offset } => {
                        let (first, _) = self.buses.get(bus)?;
                        (first + *offset as i32) as f32
                    }
                    Port::Buffer { buffer } => *self.buffers.get(buffer)? as f32,
                };
                Some((name.clone(), value))
            })
            .collect()
    }

    /// One op, built into `cmds`. A command the translator refuses is a
    /// defect of the playback that asked for it, said in the log: on a server
    /// it would be a `/fail` nobody waits for.
    fn one(
        &mut self,
        op: Op,
        engine: &mut AppEngine,
        ids: &mut IdSpaces,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), IdError> {
        let t = &mut engine.translator;
        let result: Result<(), String> = match op {
            Op::Def { family, spec } => match family.as_str() {
                #[cfg(feature = "synth")]
                "synth" => serde_json::from_value::<SynthDefSpec>(spec)
                    .map_err(|e| e.to_string())
                    .and_then(|spec| t.define_synth(spec))
                    .map(|_| ()),
                "graph" => serde_json::from_value::<GraphDefSpec>(spec)
                    .map_err(|e| e.to_string())
                    .and_then(|spec| t.define_graph(spec))
                    .map(|_| ()),
                other => Err(format!("no {other} def family in this engine")),
            },
            // The engine is in this process and a def is loaded as it is
            // sent: there is nothing to wait for.
            Op::Barrier => Ok(()),
            Op::Graph {
                handle,
                parent,
                graph,
                ports,
            } => match self.node(&parent) {
                None => Ok(()),
                Some(target) => {
                    let node = alloc_node(ids)?;
                    self.nodes.insert(handle, node);
                    let ports = self.ports(&ports);
                    t.new_graph(&graph, node, AddAction::Tail, target, &ports, cmds)
                        .map(|_| ())
                }
            },
            Op::Transport { handle } => {
                let node = alloc_node(ids)?;
                self.nodes.insert(handle, node);
                t.add_group(node, AddAction::Tail, ROOT_NODE_ID, None, cmds)
                    .map(|()| {
                        cmds.push(Cmd::TransportGroup {
                            transport: MULTITRACK_TRANSPORT as usize,
                            id: node,
                        })
                    })
            }
            Op::Follow { handle, transport } => {
                let node = alloc_node(ids)?;
                self.nodes.insert(handle, node);
                t.add_group(node, AddAction::Tail, ROOT_NODE_ID, None, cmds)
                    .map(|()| {
                        cmds.push(Cmd::TransportFollow {
                            transport: transport as usize,
                            id: node,
                        })
                    })
            }
            Op::Governed {
                handle,
                parent,
                transport,
            } => match self.node(&parent) {
                None => Ok(()),
                Some(target) => {
                    let node = alloc_node(ids)?;
                    self.nodes.insert(handle, node);
                    t.add_group(node, AddAction::Tail, target, None, cmds)
                        .map(|()| {
                            cmds.push(Cmd::TransportGroup {
                                transport: transport as usize,
                                id: node,
                            })
                        })
                }
            },
            Op::Govern { handle, transport } => {
                if let Some(node) = self.node(&handle) {
                    cmds.push(Cmd::TransportGroup {
                        transport: transport as usize,
                        id: node,
                    });
                }
                Ok(())
            }
            Op::AudioBus { handle, channels } => {
                let channels = channels.max(1);
                let first = ids.alloc(Space::AudioBuses, channels)? as i32;
                self.audio.insert(handle.clone());
                self.buses.insert(handle, (first, channels));
                Ok(())
            }
            Op::Bus { handle, channels } => {
                let channels = channels.max(1);
                let first = ids.alloc(Space::ControlBuses, channels)? as i32;
                self.buses.insert(handle, (first, channels));
                Ok(())
            }
            Op::Run { handle, run } => match self.node(&handle) {
                Some(node) => t.run_node(node, run, cmds),
                None => Ok(()),
            },
            Op::Group { handle, before } => match self.node(&before) {
                None => Ok(()),
                Some(target) => {
                    let node = alloc_node(ids)?;
                    self.nodes.insert(handle, node);
                    t.add_group(node, AddAction::Before, target, None, cmds)
                }
            },
            Op::Slot {
                handle,
                target,
                slot,
                ports,
            } => match self.node(&target) {
                None => Ok(()),
                Some(instance) => {
                    let node = alloc_node(ids)?;
                    self.nodes.insert(handle, node);
                    let ports = self.ports(&ports);
                    t.add_slot(instance, &slot, node, &ports, cmds).map(|_| ())
                }
            },
            Op::Move { .. } => {
                Err("a slot moved between graphs is not carried out here yet".into())
            }
            Op::Synth {
                handle,
                def,
                target,
                ports,
            } => match self.node(&target) {
                None => Ok(()),
                Some(group) => {
                    let node = alloc_node(ids)?;
                    self.nodes.insert(handle, node);
                    let ports = self.ports(&ports);
                    let controls: Vec<(Control, f32)> = ports
                        .iter()
                        .map(|(name, value)| (Control::Name(name), *value))
                        .collect();
                    t.new_synth(&def, node, AddAction::Tail, group, &controls, cmds)
                        .map(|_| ())
                }
            },
            Op::Buffer { handle, samples } => {
                let bufnum = ids.alloc(Space::Buffers, 1)? as i32;
                self.buffers.insert(handle, bufnum);
                let frames = samples.len().max(1);
                let mut data = samples;
                data.resize(frames, 0.0);
                let buffer = Buffer::new(data, 1, frames, engine.sample_rate as f64);
                cmds.push(Cmd::SetBuffer {
                    index: bufnum as usize,
                    buffer: Some(Arc::new(buffer)),
                });
                Ok(())
            }
            Op::Set { handle, ports } => match self.node(&handle) {
                None => Ok(()),
                Some(node) => {
                    let ports = self.ports(&ports);
                    let controls: Vec<(Control, f32)> = ports
                        .iter()
                        .map(|(name, value)| (Control::Name(name), *value))
                        .collect();
                    t.set_node(node, &controls, cmds)
                }
            },
            Op::Map { handle, port, bus } => {
                match (self.node(&handle), self.buses.get(&bus).copied()) {
                    (Some(node), Some((index, _))) => t.map_port(node, &port, index, false, cmds),
                    _ => Ok(()),
                }
            }
            Op::Unmap { handle, port } => match self.node(&handle) {
                Some(node) => t.map_port(node, &port, -1, false, cmds),
                None => Ok(()),
            },
            Op::Free { handle, forget } => {
                if let Some(node) = self.nodes.remove(&handle) {
                    t.free_node(node, cmds);
                }
                // Freeing a group frees what is inside it: these only leave
                // the table.
                for handle in forget {
                    self.nodes.remove(&handle);
                }
                Ok(())
            }
            Op::FreeBus { handle } => {
                if let Some((first, channels)) = self.buses.remove(&handle) {
                    let space = if self.audio.remove(&handle) {
                        Space::AudioBuses
                    } else {
                        Space::ControlBuses
                    };
                    let _ = ids.release(space, i64::from(first), channels);
                }
                Ok(())
            }
            Op::FreeBuffer { handle } => {
                if let Some(bufnum) = self.buffers.remove(&handle) {
                    cmds.push(Cmd::SetBuffer {
                        index: bufnum as usize,
                        buffer: None,
                    });
                    let _ = ids.release(Space::Buffers, i64::from(bufnum), 1);
                }
                Ok(())
            }
            Op::FreeDef { names } => {
                t.free_defs(&names);
                Ok(())
            }
            Op::TransportRun { transport, rolling } => {
                cmds.push(Cmd::TransportRun {
                    transport: transport as usize,
                    rolling,
                });
                Ok(())
            }
            Op::TransportLocate { transport, sample } => {
                cmds.push(Cmd::TransportLocate {
                    transport: transport as usize,
                    position: sample.max(0) as u64,
                });
                Ok(())
            }
            Op::TransportLoop { transport, span } => {
                cmds.push(Cmd::TransportLoop {
                    transport: transport as usize,
                    span: span
                        .filter(|(start, end)| start < end)
                        .map(|(start, end)| start.max(0) as u64..end.max(0) as u64),
                });
                Ok(())
            }
            Op::TransportEnd { transport, mark } => {
                cmds.push(Cmd::TransportEnd {
                    transport: transport as usize,
                    mark: mark.map(|(end, back)| EndMark {
                        end: end.max(0) as u64,
                        back: (back >= 0).then_some(back as u64),
                    }),
                });
                Ok(())
            }
            Op::TransportFade { transport, samples } => {
                cmds.push(Cmd::TransportFade {
                    transport: transport as usize,
                    samples: samples.max(0) as u64,
                });
                Ok(())
            }
            Op::TransportCue { transport } => {
                cmds.push(Cmd::TransportCue {
                    transport: transport as usize,
                });
                Ok(())
            }
        };
        if let Err(why) = result {
            tracing::warn!("app engine: {why}");
        }
        Ok(())
    }
}

impl Medium for EngineMedium {
    fn apply(&mut self, ops: Vec<Op>, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        let engine = Arc::clone(&self.engine);
        let mut engine = engine.lock().unwrap_or_else(|e| e.into_inner());
        let mut cmds = Vec::new();
        let mut outcome = Ok(());
        for op in ops {
            if let Err(e) = self.one(op, &mut engine, ids, &mut cmds) {
                outcome = Err(e);
                break;
            }
        }
        // What was built before an allocation ran out goes out all the same,
        // as an endpoint's steps before the failure are already stated.
        engine.push(cmds);
        outcome.map(|()| Vec::new())
    }

    fn bus(&self, handle: &str) -> Option<(i32, usize)> {
        self.buses.get(handle).copied()
    }

    fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

fn alloc_node(ids: &mut IdSpaces) -> Result<i32, IdError> {
    Ok(ids.alloc(Space::Nodes, 1)? as i32)
}
