//! Audio deltas over the common typed application lifecycle.
//!
//! A draft holds only the edited file's metadata while the existing edit
//! vocabulary computes its new parts. Other files, histories and spilled
//! payloads are never cloned, restored or rolled back.
use super::*;
use crate::application::{
    histories::Object,
    operations::{Access, Batch, Operation, Result as OperationResult, Target as OperationTarget},
    resources::{ResourceId, Root, RootKind, ServerInstance, Source},
    services::{Application, Services},
};

/// Typed audio work; the language/host adapter decodes commands before entry.
#[derive(Debug)]
pub enum Command {
    /// Build the initial private samples and join.
    Open,
    /// Prepare a new independent file without publishing it in the list.
    AddFile(Box<AudioEditor>),
    /// A window save decoded on an unpublished draft, including dialog answers.
    WindowSave(Event),
    /// Save an immutable snapshot, confirming its checkpoint only after I/O.
    Save {
        target: Option<Target>,
        format: String,
    },
    /// A sample edit in the existing host gesture vocabulary.
    Edit(Event),
    /// Paste an explicitly leased clipboard source; host offsets are decoded
    /// by the boundary, never by the application lifecycle.
    Paste {
        event: Event,
        clipboard: Option<ResourceId>,
    },
    /// Edit gain automation without generating samples.
    GainEnvelope(Vec<f64>),
    /// Render the current gain envelope into new samples.
    ApplyGain,
    /// Finish removal from the original object after a successful copy.
    FinishCut {
        file: i64,
        revision: u64,
        values: Vec<Value>,
    },
    /// Put a recorded take back; the history cursor is confirmed by its owner.
    Restore(Opaque),
    /// Write history-only samples to scratch storage.
    Spill(i64),
}

/// Explicit routing supplied by the composition, never discovered from focus.
#[derive(Debug)]
pub struct Request {
    /// Editing member executing the command.
    pub member: u32,
    /// Original endpoint incarnation.
    pub server: ServerInstance,
    /// Typed domain command.
    pub command: Command,
    /// Conversation version at preparation, independent of content identity.
    pub version: i64,
    /// Original gesture stamp, retained through delayed work.
    pub seq: i64,
}

#[derive(Debug)]
struct Contents {
    contents: parts::Take,
    revision: u64,
    next_revision: u64,
    frames: BTreeMap<i64, u64>,
    rates: BTreeMap<SourceId, f64>,
    spilled: BTreeSet<i64>,
    take: i64,
    copied: bool,
    landing: Option<u64>,
}

#[derive(Debug)]
enum Change {
    AddFile(Box<AudioFileState>),
    Contents(Contents),
    Save {
        target: Target,
        format: String,
        revision: u64,
        frames: u64,
        channels: u32,
        rate: f64,
    },
    Spill(i64),
}

/// Unpublished change for one file, with no previous composition snapshot.
#[derive(Debug)]
pub struct Commit {
    target: OperationTarget,
    expected_revision: u64,
    binding: u64,
    change: Change,
    outcome: Outcome,
    reserved: Vec<i64>,
    close_dialog: bool,
}

impl Commit {
    /// Whether successful completion changes the document's contents.
    pub fn changes_contents(&self) -> bool {
        self.outcome.changed
    }
}

/// The audio result uses the shared lifecycle completion contract.
pub type Completion = crate::application::services::Completion<Outcome>;

impl AudioEditor {
    // Classify only operation-producing gestures. Navigation and dialog setup
    // continue through the ordinary event path; the composition supplies leases.
    pub(crate) fn command_for(&self, event: &Event) -> Option<Command> {
        if event.addr != "/gui_event" {
            return None;
        }
        let raw_tag = event.args.get(3)?.as_str()?;
        let widget = event.args.first().map_or(0, int);
        let is_window = self.window_id().map(i64::from) == Some(widget);
        // Broadcast events must pass the same ownership boundary as a turn,
        // before preparation can reserve resources or produce an acknowledgement.
        if !is_window && !self.owns(widget, raw_tag) {
            return None;
        }
        let tag = if raw_tag == crate::turn::MENU {
            if !is_window {
                return None;
            }
            event.args.get(4)?.as_str()?
        } else {
            raw_tag
        };
        if (tag != "save" || is_window)
            && self.saves_from(widget, tag, event.args.get(4..).unwrap_or_default())
        {
            return Some(Command::WindowSave(event.clone()));
        }
        match tag {
            "paste" | "mix" => Some(Command::Paste {
                event: event.clone(),
                clipboard: None,
            }),
            "draw" | "sample" | "cut" | "delete" | "copy" | "copy_cut" | "apply_envelope"
            | "apply" | "points" => Some(Command::Edit(event.clone())),
            _ => None,
        }
    }

    pub(crate) fn unused_buffers(&self) -> Vec<i64> {
        std::iter::once(self.file.display)
            .chain(self.file.spare.iter().copied())
            .collect()
    }

    pub(crate) fn file_for_source(&self, buffer: i64) -> Option<i64> {
        std::iter::once(&self.file)
            .chain(self.open_files.parked.values())
            .find(|file| file.frames.contains_key(&buffer))
            .map(|file| file.id.0)
    }

    fn draft(&self) -> Self {
        Self {
            file: self.file.clone(),
            window_state: self.window_state.clone(),
            open_files: AudioFiles {
                rows: Vec::new(),
                parked: BTreeMap::new(),
                open: self.open_files.open,
                next_id: self.open_files.next_id,
            },
            planned_allocations: Some(Vec::new()),
        }
    }

    pub(crate) fn sync_resources(
        &self,
        member: u32,
        server: ServerInstance,
        services: &mut Services,
    ) {
        for (buffer, bytes, stored, owned) in self.resource_sources() {
            services.resources.register(Source {
                id: ResourceId { server, buffer },
                member,
                bytes,
                stored,
                owned,
            });
        }
        self.retain_committed(member, server, services);
    }

    fn retain_committed(&self, member: u32, server: ServerInstance, services: &mut Services) {
        let ids = |sources: Vec<SourceId>| {
            sources
                .into_iter()
                .map(|source| ResourceId {
                    server,
                    buffer: source.0 as i64,
                })
                .collect::<Vec<_>>()
        };
        services.resources.retain(
            Root {
                kind: RootKind::Contents,
                owner: u64::from(member),
            },
            ids(self.rooted()),
        );
        services.resources.retain(
            Root {
                kind: RootKind::History,
                owner: u64::from(member),
            },
            ids(services.histories.sources_for(member)),
        );
    }
}

impl Application for AudioEditor {
    type Command = Request;
    type Commit = Commit;
    type Outcome = Outcome;

    fn prepare(&self, request: Request, services: &Services) -> Result<Operation<Commit>, String> {
        if let Command::AddFile(file) = &request.command {
            let mut next = file.draft();
            next.file.id = FileId(self.open_files.next_id);
            next.window_state.scratch = self.window_state.scratch.clone();
            next.window_state.scratch_scope = self.window_state.scratch_scope.clone();
            let mut plan = next.prepare(
                Request {
                    command: Command::Open,
                    ..request
                },
                services,
            )?;
            let Change::Contents(contents) = plan.commit.change else {
                unreachable!()
            };
            next.file.contents = contents.contents;
            next.file.take = contents.take;
            next.file.copied = contents.copied;
            next.file.frames = contents.frames;
            next.file.rates = contents.rates;
            next.file.spilled = contents.spilled;
            next.file
                .spare
                .retain(|id| !plan.commit.reserved.contains(id));
            plan.commit.binding = self.window_state.binding_generation;
            plan.commit.change = Change::AddFile(Box::new(next.file));
            return Ok(plan);
        }
        if !self.open_files.open {
            return Err(super::NO_FILE_OPEN.into());
        }
        if let Command::WindowSave(event) = &request.command {
            let mut draft = self.draft();
            let mut outcome = draft.event(event, request.version);
            if !outcome.save_requested {
                return Err("the save request was refused".into());
            }
            let mut plan = self.prepare(
                Request {
                    command: Command::Save {
                        target: Some(draft.file.target.clone()),
                        format: draft
                            .file
                            .written_format
                            .clone()
                            .unwrap_or_else(|| "float".into()),
                    },
                    ..request
                },
                services,
            )?;
            outcome.steps = None;
            outcome.planned_steps.clear();
            plan.commit.outcome = outcome;
            plan.commit.close_dialog = draft.window_state.dialogs.up().is_none();
            return Ok(plan);
        }
        let target = OperationTarget {
            member: request.member,
            object: self.focused_file(),
            server: request.server,
        };
        let clipboard_read = match &request.command {
            Command::Paste { clipboard, .. } => *clipboard,
            _ => None,
        };
        let mut draft = self.draft();
        draft.file.spare.retain(|buffer| {
            services.resources.available(ResourceId {
                server: request.server,
                buffer: *buffer,
            })
        });
        let mut outcome = Outcome {
            turn: Kind::Route,
            version: request.version,
            ..Default::default()
        };
        let mut batches = Vec::new();
        let mut access = Access::Write;
        let change = match request.command {
            Command::Save {
                target: destination,
                format,
            } => {
                if let Some(Target::Buffer(buffer)) = destination.as_ref()
                    && (*buffer == self.file.display || self.file.frames.contains_key(buffer))
                {
                    return Err("the save target is a buffer this editor reads".into());
                }
                let snapshot = draft.next_buffer()?;
                let frames = self.length();
                let mut copy = draft.allocate(snapshot, frames);
                copy.push(send(
                    "/buffer_gen",
                    vec![
                        OscType::Int(snapshot as i32),
                        OscType::String("copy".into()),
                        OscType::Int(0),
                        OscType::Int(self.file.display as i32),
                        OscType::Int(0),
                        OscType::Int((frames * u64::from(self.file.channels)) as i32),
                    ],
                ));
                copy.push(Step::AwaitDone {
                    command: "/buffer_gen".into(),
                    index: Some(snapshot as i32),
                });
                draft.file.display = snapshot;
                let written = draft.save_steps(destination, &format)?;
                batches.push(Batch {
                    member: request.member,
                    server: request.server,
                    steps: copy,
                });
                batches.push(Batch {
                    member: request.member,
                    server: request.server,
                    steps: written,
                });
                access = Access::Snapshot(0);
                Change::Save {
                    target: draft.file.target.clone(),
                    format,
                    revision: self.file.revision,
                    frames: self.length(),
                    channels: self.file.channels,
                    rate: self.file.rate,
                }
            }
            Command::Spill(buffer) => {
                let path = self
                    .path(buffer)
                    .ok_or("the source has no scratch storage")?;
                if !self.file.frames.contains_key(&buffer)
                    || self.file.spilled.contains(&buffer)
                    || self.file.take == buffer
                    || parts::sources(&self.file.contents.parts).contains(&SourceId(buffer as u64))
                {
                    return Err("the source is not history-only resident samples".into());
                }
                // The write is a separate dependency. A failed write cannot free samples.
                batches.push(Batch {
                    member: request.member,
                    server: request.server,
                    steps: vec![
                        send(
                            "/buffer_write",
                            vec![
                                OscType::Int(buffer as i32),
                                OscType::String(path),
                                OscType::String("wav".into()),
                                OscType::String("float".into()),
                            ],
                        ),
                        Step::AwaitDone {
                            command: "/buffer_write".into(),
                            index: Some(buffer as i32),
                        },
                    ],
                });
                batches.push(Batch {
                    member: request.member,
                    server: request.server,
                    steps: vec![
                        send("/buffer_free", vec![OscType::Int(buffer as i32)]),
                        Step::AwaitDone {
                            command: "/buffer_free".into(),
                            index: Some(buffer as i32),
                        },
                    ],
                });
                Change::Spill(buffer)
            }
            command => {
                let steps = match command {
                    Command::AddFile(_) => unreachable!(),
                    Command::Open => draft.open_steps()?,
                    Command::Edit(event) | Command::Paste { event, .. } => {
                        outcome = draft.event(&event, request.version);
                        outcome.planned_steps.clone()
                    }
                    Command::GainEnvelope(points) => {
                        let (reason, corrections) = draft.set_envelope(&points, &mut outcome);
                        outcome.answer = Some(conversation::answer(
                            0,
                            outcome.version,
                            reason,
                            corrections,
                        ));
                        outcome.planned_steps.clone()
                    }
                    Command::ApplyGain => {
                        let (reason, corrections) = draft.apply_envelope_event(&mut outcome);
                        outcome.answer = Some(conversation::answer(
                            0,
                            outcome.version,
                            reason,
                            corrections,
                        ));
                        outcome.planned_steps.clone()
                    }
                    Command::FinishCut {
                        file,
                        revision,
                        values,
                    } => {
                        if file != self.focused_file() || revision != self.file.revision {
                            return Err("the cut target changed while the copy was pending".into());
                        }
                        let (reason, corrections) = draft.gesture(
                            i64::from(draft.window_state.widget.unwrap_or_default()),
                            "cut",
                            &values,
                            &mut outcome,
                        );
                        outcome.answer = Some(conversation::answer(
                            0,
                            outcome.version,
                            reason,
                            corrections,
                        ));
                        outcome.planned_steps.clone()
                    }
                    Command::Restore(payload) => {
                        let mut next = parts::Take::read(&payload)
                            .ok_or("the history payload is not a take")?;
                        self.keep_visibility(&mut next);
                        draft.file.contents = next;
                        draft.file.revision = payload.0["contentRevision"]
                            .as_u64()
                            .unwrap_or(self.file.next_revision + 1);
                        draft.file.next_revision =
                            draft.file.next_revision.max(draft.file.revision);
                        let mut steps = draft.restored();
                        steps.extend(draft.stitched());
                        outcome.changed = true;
                        steps
                    }
                    Command::Save { .. } | Command::Spill(_) | Command::WindowSave(_) => {
                        unreachable!()
                    }
                };
                if !steps.is_empty() {
                    batches.push(Batch {
                        member: request.member,
                        server: request.server,
                        steps,
                    });
                }
                Change::Contents(Contents {
                    contents: draft.file.contents.clone(),
                    revision: draft.file.revision,
                    next_revision: draft.file.next_revision,
                    frames: draft.file.frames.clone(),
                    rates: draft.file.rates.clone(),
                    spilled: draft.file.spilled.clone(),
                    take: draft.file.take,
                    copied: draft.file.copied,
                    landing: outcome.cue,
                })
            }
        };
        let external_target = match &change {
            Change::Save {
                target: Target::Buffer(buffer),
                ..
            } => Some(*buffer),
            _ => None,
        };
        let allocations = draft
            .planned_allocations
            .take()
            .unwrap_or_default()
            .into_iter()
            .filter(|(buffer, _, _)| {
                Some(*buffer) != external_target
                    && services
                        .resources
                        .source(ResourceId {
                            server: request.server,
                            buffer: *buffer,
                        })
                        .is_none()
            })
            .map(|(buffer, frames, _)| Source {
                id: ResourceId {
                    server: request.server,
                    buffer,
                },
                member: request.member,
                bytes: frames * u64::from(self.file.channels) * 4,
                owned: true,
                stored: None,
            })
            .collect::<Vec<_>>();
        let reserved = allocations.iter().map(|source| source.id.buffer).collect();
        let mut read_sources = self.rooted();
        if let Change::Contents(contents) = &change {
            read_sources.extend(parts::sources(&contents.contents.parts));
        }
        if let Change::Spill(buffer) = change {
            read_sources.push(SourceId(buffer as u64));
        }
        let mut reads: Vec<_> = read_sources
            .into_iter()
            .map(|source| ResourceId {
                server: request.server,
                buffer: source.0 as i64,
            })
            .collect();
        if let Some(id) = clipboard_read {
            reads.push(id);
        }
        if let Some(Answer::Ack { seq, .. } | Answer::Push { seq, .. }) = &mut outcome.answer {
            *seq = request.seq;
        }
        if let Some(continuation) = outcome.clipboard_cut.as_mut() {
            continuation["seq"] = json!(request.seq);
        }
        outcome.steps = None;
        outcome.planned_steps.clear();
        let commit = Commit {
            target,
            expected_revision: self.file.revision,
            binding: self.window_state.binding_generation,
            change,
            outcome,
            reserved,
            close_dialog: false,
        };
        Ok(Operation {
            target,
            additional_targets: Vec::new(),
            access,
            batches,
            allocations,
            reads,
            commit,
        })
    }

    fn commit(&mut self, mut delta: Commit, services: &mut Services) -> Completion {
        if matches!(delta.change, Change::AddFile(_)) {
            if self.open_files.next_id != delta.target.object {
                return self.abort(
                    delta,
                    OperationResult::Failed("the file identity was superseded".into()),
                    0,
                    services,
                );
            }
            let Change::AddFile(file) = std::mem::replace(&mut delta.change, Change::Spill(-1))
            else {
                unreachable!()
            };
            let focus =
                self.window_state.active && self.window_state.binding_generation == delta.binding;
            self.add_file_state(*file, focus);
            delta.outcome.changed = true;
            delta.outcome.prepared_complete = true;
            delta.outcome.restructure = focus;
            delta.outcome.focus_changed = focus;
            delta.outcome.binding = focus.then(|| self.binding());
            if focus {
                delta.outcome.answer =
                    Some(conversation::answer(0, 0, None, self.focus_corrections()));
                delta.outcome.playback_update =
                    Some(json!({"file":self.playback_file(),"publish":true}));
            }
            self.retain_committed(delta.target.member, delta.target.server, services);
            return Completion {
                committed: true,
                outcome: delta.outcome,
                recovery: Vec::new(),
                returned: Vec::new(),
            };
        }
        let current = self.over_file(delta.target.object, |editor| editor.file.revision);
        if current.is_none()
            || (!matches!(delta.change, Change::Save { .. })
                && current != Some(delta.expected_revision))
        {
            return self.abort(
                delta,
                OperationResult::Failed("the object changed before confirmation".into()),
                0,
                services,
            );
        }
        let visible = self.focused_file() == delta.target.object
            && self.window_state.binding_generation == delta.binding;
        let applied = self
            .over_file(delta.target.object, |editor| {
                if !matches!(delta.change, Change::Save { .. })
                    && editor.file.revision != delta.expected_revision
                {
                    return false;
                }
                editor
                    .file
                    .spare
                    .retain(|buffer| !delta.reserved.contains(buffer));
                match delta.change {
                    Change::AddFile(_) => unreachable!(),
                    Change::Contents(mut contents) => {
                        editor.keep_visibility(&mut contents.contents);
                        editor.file.contents = contents.contents;
                        editor.file.revision = contents.revision;
                        editor.file.next_revision = contents.next_revision;
                        editor.file.frames = contents.frames;
                        editor.file.rates = contents.rates;
                        editor.file.spilled = contents.spilled;
                        editor.file.take = contents.take;
                        editor.file.copied = contents.copied;
                        if let Some(at) = contents.landing {
                            editor.file.presentation.cursor = Some(at);
                            editor.file.presentation.selection = None;
                        }
                        editor
                            .file
                            .spare
                            .retain(|buffer| !delta.reserved.contains(buffer));
                        if let Some(record) = &delta.outcome.record {
                            let structure = services.histories.structure_of(Object {
                                member: delta.target.member,
                                id: delta.target.object,
                            });
                            if let Some(structure) = structure {
                                record_into(&mut services.histories, structure, record);
                            }
                        }
                    }
                    Change::Save {
                        target,
                        format,
                        revision,
                        frames,
                        channels,
                        rate,
                    } => {
                        let mut saved = match &target {
                            Target::File(path) => json!({"path":path}),
                            Target::Buffer(buffer) => json!({"buffer":buffer}),
                        };
                        saved["frames"] = json!(frames);
                        saved["channels"] = json!(channels);
                        saved["rate"] = json!(rate);
                        delta.outcome.saved = Some(saved);
                        editor.file.target = target;
                        editor.file.written_format = Some(format);
                        editor.file.saved_revision = revision;
                    }
                    Change::Spill(buffer) => {
                        editor.file.spilled.insert(buffer);
                    }
                }
                editor.file.dirty = editor.file.revision != editor.file.saved_revision;
                true
            })
            .unwrap_or(false);
        if visible && applied {
            self.window_state
                .conversation
                .applied(delta.outcome.version);
        }
        if visible && applied && delta.close_dialog {
            self.window_state.dialogs.hide();
        }
        if !visible {
            delta.outcome.answer = None;
            delta.outcome.restructure = false;
            delta.outcome.locate = None;
            delta.outcome.selection = None;
            delta.outcome.cue = None;
            delta.outcome.follow = None;
            delta.outcome.layers = None;
            delta.outcome.preview = None;
        }
        if applied && let Some(raw) = delta.outcome.clipboard.as_ref() {
            let doc = Clipboard::parse(raw);
            match services.clipboard.publish(
                &mut services.resources,
                doc,
                delta.target.server,
                delta.target.member,
            ) {
                Ok(document) => {
                    delta.outcome.clipboard = Some(document);
                    delta.outcome.clipboard_ready = true;
                }
                Err(reason) => {
                    delta.outcome.answer =
                        Some(conversation::answer(0, 0, Some(reason), Vec::new()))
                }
            }
        }
        let current_view = self.focused_file() == delta.target.object && self.window_state.active;
        if applied && current_view {
            // Project current state into the currently composed widgets. Even
            // the original binding may have changed its optional lanes while
            // I/O was pending. A recreated binding gets a new state push.
            let seq = if visible {
                match delta.outcome.answer {
                    Some(Answer::Ack { seq, .. } | Answer::Push { seq, .. }) => seq,
                    _ => 0,
                }
            } else {
                0
            };
            let reason = match &delta.outcome.answer {
                Some(Answer::Ack { reason, .. } | Answer::Push { reason, .. }) if visible => {
                    reason.clone()
                }
                _ => None,
            };
            let mut corrections = self.reread();
            corrections.extend(self.envelope_corrections());
            if delta.close_dialog {
                corrections.extend(
                    crate::forms::hidden(&self.window_state.dialogs.ids)
                        .into_iter()
                        .map(|(widget, props)| Correction {
                            widget: i64::from(widget),
                            props,
                        }),
                );
            }
            corrections.extend(self.focus_corrections());
            if let Some(pane) = self.window_state.pane {
                corrections.push(Correction {
                    widget: i64::from(pane),
                    props: json!({"label":self.pane_label()}),
                });
            }
            delta.outcome.answer = Some(conversation::answer(seq, 0, reason, corrections));
        }
        delta.outcome.prepared_complete = true;
        if applied && delta.outcome.changed && self.window_state.active {
            delta.outcome.playback_update = self.over_file(
                delta.target.object,
                |editor| json!({"file":editor.playback_file(),"publish":current_view}),
            );
        }
        self.retain_committed(delta.target.member, delta.target.server, services);
        Completion {
            committed: applied,
            outcome: delta.outcome,
            recovery: Vec::new(),
            returned: Vec::new(),
        }
    }

    fn abort(
        &mut self,
        delta: Commit,
        result: OperationResult,
        completed_batches: usize,
        services: &mut Services,
    ) -> Completion {
        let _ = self.over_file(delta.target.object, |editor| {
            editor
                .file
                .spare
                .retain(|buffer| !delta.reserved.contains(buffer))
        });
        let reason = match result {
            OperationResult::Failed(reason) => reason,
            OperationResult::Cancelled => "the operation was cancelled".into(),
            OperationResult::Success => unreachable!(),
        };
        let mut outcome = Outcome::default();
        if self.focused_file() == delta.target.object
            && self.window_state.binding_generation == delta.binding
        {
            let seq = match delta.outcome.answer {
                Some(Answer::Ack { seq, .. } | Answer::Push { seq, .. }) => seq,
                _ => 0,
            };
            outcome.answer = Some(conversation::answer(seq, 0, Some(reason), self.reread()));
        }
        if let Change::Spill(buffer) = &delta.change {
            let buffer = *buffer;
            if completed_batches > 0 {
                self.over_file(delta.target.object, |editor| {
                    editor.file.spilled.insert(buffer);
                    services.resources.location(
                        ResourceId {
                            server: delta.target.server,
                            buffer,
                        },
                        editor.path(buffer),
                    );
                });
            }
        }
        let returned = if let Change::AddFile(file) = &delta.change {
            std::iter::once(file.display)
                .chain(file.spare.iter().copied())
                .collect()
        } else {
            Vec::new()
        };
        let recovery = match delta.change {
            Change::AddFile(file) => vec![
                send("/buffer_free", vec![OscType::Int(file.display as i32)]),
                Step::AwaitDone {
                    command: "/buffer_free".into(),
                    index: Some(file.display as i32),
                },
            ],
            Change::Save { .. } => Vec::new(),
            Change::Spill(buffer) if completed_batches > 0 => vec![
                send("/buffer_free", vec![OscType::Int(buffer as i32)]),
                Step::AwaitDone {
                    command: "/buffer_free".into(),
                    index: Some(buffer as i32),
                },
            ],
            Change::Spill(_) => Vec::new(),
            Change::Contents(ref contents) => self
                .over_file(delta.target.object, |editor| {
                    let restored = editor
                        .file
                        .spilled
                        .difference(&contents.spilled)
                        .copied()
                        .collect::<Vec<_>>();
                    let mut steps = Vec::new();
                    for buffer in restored {
                        steps.push(send("/buffer_free", vec![OscType::Int(buffer as i32)]));
                        steps.push(Step::AwaitDone {
                            command: "/buffer_free".into(),
                            index: Some(buffer as i32),
                        });
                    }
                    if editor.file.copied {
                        steps.extend(editor.stitched());
                    } else {
                        steps.push(send(
                            "/buffer_free",
                            vec![OscType::Int(editor.file.display as i32)],
                        ));
                        steps.push(Step::AwaitDone {
                            command: "/buffer_free".into(),
                            index: Some(editor.file.display as i32),
                        });
                    }
                    steps
                })
                .unwrap_or_default(),
        };
        Completion {
            committed: false,
            outcome,
            recovery,
            returned,
        }
    }
}

fn record_into(
    histories: &mut crate::application::histories::Histories,
    structure: clausters_document::history::StructureId,
    record: &Record,
) {
    use clausters_document::history::{Entry, Step as HistoryStep};
    let mut entry: Option<Entry> = None;
    for leg in &record.legs {
        let Ok(forward) = serde_json::from_value::<HistoryStep>(leg.forward.clone()) else {
            continue;
        };
        let next = match entry.take() {
            Some(entry) => entry.and(structure, forward, Opaque(leg.backward.clone())),
            None => Entry::new(
                record.label.clone(),
                structure,
                forward,
                Opaque(leg.backward.clone()),
            ),
        };
        let next = if leg.key.is_empty() {
            next
        } else {
            next.keyed(leg.key.clone())
        };
        entry = Some(next.holding(
            leg.holds_forward.iter().copied().map(SourceId),
            leg.holds_backward.iter().copied().map(SourceId),
        ));
    }
    if let Some(entry) = entry {
        histories.record(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{histories::Object, operations::Ticket};
    #[derive(Default)]
    struct Harness {
        shared: crate::application::services::Services,
        operations: crate::application::operations::Operations<Commit>,
    }
    fn endpoint() -> ServerInstance {
        ServerInstance {
            connection: 1,
            generation: 1,
        }
    }
    fn request(command: Command) -> Request {
        Request {
            member: 0,
            server: endpoint(),
            command,
            version: 1,
            seq: 0,
        }
    }
    fn editor() -> AudioEditor {
        let mut editor=super::super::new_json(r#"{"take":1,"frames":100,"channels":1,"rate":48000,"display":2,"buffers":[10,11,12,13,14,15,16],"scratch":"scratch"}"#).unwrap();
        editor.window(17, None);
        editor
    }
    fn begin(editor: &mut AudioEditor, services: &mut Harness, command: Command) -> Ticket {
        editor.sync_resources(0, endpoint(), &mut services.shared);
        let operation = editor.prepare(request(command), &services.shared).unwrap();
        services
            .operations
            .prepare(&mut services.shared.resources, operation)
            .unwrap()
    }
    fn finish(
        editor: &mut AudioEditor,
        services: &mut Harness,
        ticket: Ticket,
        result: OperationResult,
    ) -> Completion {
        loop {
            let (batch, _) = services.operations.next(ticket).unwrap();
            if let Some(finished) = services.operations.complete(
                &services.shared.resources,
                ticket,
                batch,
                result.clone(),
            ) {
                let completion = services
                    .operations
                    .finish(editor, &mut services.shared, finished);
                if !completion.committed {
                    assert!(
                        services
                            .operations
                            .finish_abort(&mut services.shared.resources, ticket)
                    );
                }
                return completion;
            }
        }
    }
    fn opened() -> (AudioEditor, Harness) {
        let mut editor = editor();
        let mut services = Harness::default();
        services.shared.histories.join(
            Object {
                member: 0,
                id: editor.focused_file(),
            },
            "take",
            "samples",
        );
        let ticket = begin(&mut editor, &mut services, Command::Open);
        assert!(
            !editor.file.copied,
            "preparation does not publish the private copy"
        );
        assert!(finish(&mut editor, &mut services, ticket, OperationResult::Success).committed);
        (editor, services)
    }
    fn stroke() -> Command {
        Command::Edit(Event {
            addr: "/gui_event".into(),
            args: vec![
                json!(17),
                json!(1),
                json!(0),
                json!("draw"),
                json!(0),
                json!(10),
                json!([0.5]),
                json!([0.0]),
            ],
        })
    }

    #[test]
    fn gesture_commands_leave_navigation_and_dialog_setup_out_of_operations() {
        let mut editor = editor();
        editor.window_state.window = Some(17);
        let Command::Edit(mut event) = stroke() else {
            unreachable!()
        };
        assert!(matches!(editor.command_for(&event), Some(Command::Edit(_))));
        event.args[3] = json!(crate::turn::MENU);
        event.args[4] = json!("apply_envelope");
        assert!(matches!(editor.command_for(&event), Some(Command::Edit(_))));
        event.args[4] = json!("save");
        assert!(matches!(
            editor.command_for(&event),
            Some(Command::WindowSave(_))
        ));
        for tag in ["locate", "selection", "view_both", "save_as", "unknown"] {
            event.args[4] = json!(tag);
            assert!(editor.command_for(&event).is_none(), "{tag}");
        }
        event.args.clear();
        assert!(editor.command_for(&event).is_none());
    }

    #[test]
    fn pending_edits_and_restores_keep_current_curve_visibility() {
        let (mut editor, mut services) = opened();
        editor.ensure_envelope();
        for visible in [true, false] {
            let previous = editor.file.contents.payload();
            let ticket = begin(&mut editor, &mut services, stroke());
            editor.file.contents.automation[0].visible = visible;
            assert!(finish(&mut editor, &mut services, ticket, OperationResult::Success).committed);
            assert_eq!(editor.file.contents.automation[0].visible, visible);
            let ticket = begin(&mut editor, &mut services, Command::Restore(previous));
            editor.file.contents.automation[0].visible = !visible;
            assert!(finish(&mut editor, &mut services, ticket, OperationResult::Success).committed);
            assert_eq!(editor.file.contents.automation[0].visible, !visible);
        }
    }

    #[test]
    fn sample_edits_publish_contents_and_history_only_after_success() {
        let (mut editor, mut services) = opened();
        let before = editor.file.contents.clone();
        let ticket = begin(&mut editor, &mut services, stroke());
        assert_eq!(editor.file.contents, before);
        assert!(!services.shared.histories.can_undo());
        assert!(services.shared.resources.collect().is_empty());
        let failed = finish(
            &mut editor,
            &mut services,
            ticket,
            OperationResult::Failed("allocation refused".into()),
        );
        assert!(!failed.committed);
        assert_eq!(editor.file.contents, before);
        assert!(!services.shared.histories.can_undo());
        assert_eq!(
            services.shared.resources.collect().len(),
            1,
            "only the failed operation's reservation is released"
        );
        let ticket = begin(&mut editor, &mut services, stroke());
        assert!(finish(&mut editor, &mut services, ticket, OperationResult::Success).committed);
        assert_ne!(editor.file.contents, before);
        assert!(services.shared.histories.can_undo());
    }

    #[test]
    fn save_checkpoint_waits_for_io_and_an_edit_during_save_stays_dirty() {
        let (mut editor, mut services) = opened();
        let edit = begin(&mut editor, &mut services, stroke());
        finish(&mut editor, &mut services, edit, OperationResult::Success);
        assert!(editor.file.dirty);
        let ticket = begin(
            &mut editor,
            &mut services,
            Command::Save {
                target: Some(Target::File("saved.wav".into())),
                format: "float".into(),
            },
        );
        assert!(matches!(editor.target(), Target::Buffer(1)));
        let (batch, _) = services.operations.next(ticket).unwrap();
        assert!(
            services
                .operations
                .complete(
                    &services.shared.resources,
                    ticket,
                    batch,
                    OperationResult::Success
                )
                .is_none()
        );
        let edit = begin(&mut editor, &mut services, stroke());
        finish(&mut editor, &mut services, edit, OperationResult::Success);
        assert!(finish(&mut editor, &mut services, ticket, OperationResult::Success).committed);
        assert_eq!(editor.target(), &Target::File("saved.wav".into()));
        assert!(editor.file.dirty);
        let undo = services
            .shared
            .histories
            .peek_walk(clausters_document::history::Direction::Undo)
            .unwrap();
        let payload = undo.legs[0].1.last().unwrap().clone();
        let restore = begin(&mut editor, &mut services, Command::Restore(payload));
        finish(
            &mut editor,
            &mut services,
            restore,
            OperationResult::Success,
        );
        services
            .shared
            .histories
            .walk(clausters_document::history::Direction::Undo)
            .unwrap();
        assert!(
            !editor.file.dirty,
            "undo to the saved content identity is clean"
        );
        let edit = begin(&mut editor, &mut services, stroke());
        finish(&mut editor, &mut services, edit, OperationResult::Success);
        let ticket = begin(
            &mut editor,
            &mut services,
            Command::Save {
                target: Some(Target::File("failed.wav".into())),
                format: "float".into(),
            },
        );
        assert!(
            !finish(
                &mut editor,
                &mut services,
                ticket,
                OperationResult::Failed("quota".into())
            )
            .committed
        );
        assert_eq!(editor.target(), &Target::File("saved.wav".into()));
        assert!(editor.file.dirty);
    }

    #[test]
    fn a_recreated_view_gets_a_fresh_push_without_the_original_gesture_receipt() {
        let (mut editor, mut services) = opened();
        editor.window_state.lane = Some(18);
        editor.file.presentation.views = Views::Stacked;
        editor.sync_resources(0, endpoint(), &mut services.shared);
        let mut request = request(stroke());
        request.seq = 77;
        let plan = editor.prepare(request, &services.shared).unwrap();
        let ticket = services
            .operations
            .prepare(&mut services.shared.resources, plan)
            .unwrap();
        editor.window_state.binding_generation += 1;
        editor.file.presentation.views = Views::Waveform;
        let completion = finish(&mut editor, &mut services, ticket, OperationResult::Success);
        let Some(Answer::Push {
            seq, corrections, ..
        }) = completion.outcome.answer
        else {
            panic!("fresh state push");
        };
        assert_eq!(seq, 0);
        assert!(corrections.iter().all(|correction| correction.widget != 18));
        assert_eq!(completion.outcome.playback_update.unwrap()["publish"], true);
        assert!(!completion.outcome.restructure);
    }

    #[test]
    fn a_delayed_save_reports_the_original_snapshots_destination_and_shape() {
        let (mut editor, mut services) = opened();
        let original = editor.focused_file();
        let ticket = begin(
            &mut editor,
            &mut services,
            Command::Save {
                target: Some(Target::Buffer(99)),
                format: "float".into(),
            },
        );
        let (batch, _) = services.operations.next(ticket).unwrap();
        assert!(
            services
                .operations
                .complete(
                    &services.shared.resources,
                    ticket,
                    batch,
                    OperationResult::Success
                )
                .is_none()
        );
        editor.add_file(super::super::new_json(r#"{"take":50,"frames":200,"channels":2,"rate":44100,"display":51,"buffers":[52,53]}"#).unwrap());
        let focused = editor.focused_file();
        let completion = finish(&mut editor, &mut services, ticket, OperationResult::Success);
        assert!(completion.committed);
        assert_eq!(
            completion.outcome.saved.unwrap(),
            json!({"buffer":99,"frames":100,"channels":1,"rate":48000.0})
        );
        assert_eq!(editor.focused_file(), focused);
        assert_eq!(editor.target(), &Target::Buffer(50));
        editor.focus_file(original).unwrap();
        assert_eq!(editor.target(), &Target::Buffer(99));
    }

    #[test]
    fn late_commit_targets_the_original_file_without_rebinding_the_view() {
        let (mut editor, mut services) = opened();
        let original = editor.focused_file();
        let ticket = begin(&mut editor, &mut services, stroke());
        editor.add_file(
            super::super::new_json(r#"{"take":50,"frames":200,"display":51,"buffers":[52,53]}"#)
                .unwrap(),
        );
        let focused = editor.focused_file();
        let completion = finish(&mut editor, &mut services, ticket, OperationResult::Success);
        assert!(completion.committed);
        assert!(completion.outcome.answer.is_none());
        assert_eq!(editor.focused_file(), focused);
        assert_eq!(editor.length(), 200);
        editor.focus_file(original).unwrap();
        assert!(editor.file.dirty);
        assert!(services.shared.histories.can_undo());
    }
}
