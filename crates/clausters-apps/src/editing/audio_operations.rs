//! Boundary decoding for the typed audio lifecycle. No recursive dispatch.
use super::*;
use crate::application::runtime::{Control, Dispatch, PendingWalk};
use crate::application::{
    operations::{Result as OperationResult, Ticket},
    services::Application,
};
use audio::operations::{Command, Request};
use clausters_editing::apply::steps_json;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
enum OperationCall {
    Prepare {
        member: MemberId,
        command: AudioCommand,
    },
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum AudioCommand {
    Open,
    Save {
        path: Option<String>,
        buffer: Option<i64>,
        #[serde(default = "float_format")]
        format: String,
    },
    Edit {
        event: Event,
    },
    Restore {
        payload: Value,
    },
}
fn float_format() -> String {
    "float".into()
}

impl Editing {
    pub(super) fn prepare_audio(
        &mut self,
        member: MemberId,
        command: Command,
        seq: i64,
    ) -> Result<Ticket, String> {
        self.select_member(member);
        self.sync_resources();
        let seat = self
            .seats
            .get_mut(member as usize)
            .ok_or("the operation member is not open")?;
        let Member::Audio(editor) = &mut seat.member else {
            return Err("the member is not an audio application".into());
        };
        let object = match &command {
            Command::FinishCut { file, .. } => Some(*file),
            _ => None,
        };
        let request = Request {
            member,
            server: seat.server,
            command,
            version: self.version,
            seq,
        };
        let operation = if let Some(object) = object {
            editor
                .over_file(object, |editor| editor.prepare(request, &self.services))
                .ok_or("the original operation object is closed")??
        } else {
            editor.prepare(request, &self.services)?
        };
        let ticket = self
            .audio_runtime
            .operations
            .prepare(&mut self.services.resources, operation)
            .map_err(str::to_string)?;
        Ok(ticket)
    }

    pub(super) fn prepare_add_file(&mut self, member: MemberId, request: &Value) -> Value {
        let request = match self.member(member) {
            Some(Member::Audio(editor)) => editor.with_folder(request),
            _ => request.clone(),
        };
        let file = match audio::new_json(&request.to_string()) {
            Ok(file) => file,
            Err(error) => return json!({"error":error}),
        };
        let unused = file.unused_buffers();
        match self.prepare_audio(member, Command::AddFile(Box::new(file)), 0) {
            Ok(ticket) => json!({"turn":"route", "operation":self.audio_next(ticket)}),
            Err(error) => {
                let freed = self.release();
                json!({"error":error,"freed":freed,"returned":{
                    "member":member,"server":self.seats[member as usize].server,"buffers":unused}})
            }
        }
    }

    fn decode_audio_event(&mut self, member: MemberId, event: &Event) -> Option<Command> {
        let Member::Audio(editor) = self.member(member)? else {
            return None;
        };
        let mut command = editor.command_for(event)?;
        if let Command::Paste { clipboard, .. } = &mut command {
            let doc = clausters_document::clipboard::Clipboard::parse(
                event
                    .args
                    .get(6)
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            *clipboard = if let clausters_document::clipboard::Content::Buffer {
                buffer,
                frames,
                channels,
                ..
            } = doc.content
            {
                let server = self.seats.get(member as usize)?.server;
                let id = ResourceId {
                    server,
                    buffer: i64::from(buffer),
                };
                if self.services.resources.source(id).is_none() {
                    self.services.resources.register(Source {
                        id,
                        member,
                        bytes: frames.saturating_mul(u64::from(channels)).saturating_mul(4),
                        owned: false,
                    });
                }
                Some(id)
            } else {
                None
            };
        }
        Some(command)
    }

    pub(super) fn prepare_audio_event(
        &mut self,
        member: MemberId,
        event: &Event,
    ) -> Option<Turned> {
        let command = self.decode_audio_event(member, event)?;
        Some(self.prepare_audio_turn(member, command, event.args.get(1).map_or(0, int)))
    }

    pub(super) fn prepare_audio_action(
        &mut self,
        member: MemberId,
        request: &Value,
    ) -> Option<Turned> {
        if !matches!(self.member(member), Some(Member::Audio(_))) {
            return None;
        }
        let command = match request["verb"].as_str()? {
            "setautomation" => Command::GainEnvelope(
                request["points"]
                    .as_array()?
                    .iter()
                    .map(|point| point.as_f64().unwrap_or(f64::NAN))
                    .collect(),
            ),
            "apply_envelope" => Command::ApplyGain,
            "finish_cut" => Command::FinishCut {
                file: request["file"].as_i64()?,
                revision: request["beforeRevision"].as_u64()?,
                values: request["values"].as_array()?.clone(),
            },
            _ => return None,
        };
        Some(self.prepare_audio_turn(member, command, request["seq"].as_i64().unwrap_or(0)))
    }

    fn prepare_audio_turn(&mut self, member: MemberId, command: Command, seq: i64) -> Turned {
        let version = self.version;
        let outcome = match self.prepare_audio(member, command, seq) {
            Ok(ticket) => {
                if let Some((batch, work)) = self.audio_runtime.operations.next(ticket) {
                    let changed = self
                        .audio_runtime
                        .operations
                        .operation(ticket)
                        .is_some_and(|operation| operation.commit.changes_contents());
                    audio::Outcome {
                        turn: Kind::Route,
                        changed,
                        version,
                        operation: Some(crate::application::operations::Prepared {
                            member: work.member,
                            ticket,
                            batch,
                            server: work.server,
                            steps: work.steps,
                        }),
                        ..Default::default()
                    }
                } else if let Some(finished) = self.audio_runtime.operations.finish_ready(ticket) {
                    let Member::Audio(editor) = &mut self.seats[member as usize].member else {
                        unreachable!()
                    };
                    self.audio_runtime
                        .finish_turn(
                            editor.as_mut(),
                            &mut self.services,
                            finished,
                            &mut self.version,
                        )
                        .outcome
                } else {
                    unreachable!("a new operation has a batch or an immediate result");
                }
            }
            Err(reason) => audio::Outcome {
                turn: Kind::Route,
                version,
                answer: Some(clausters_editing::conversation::answer(
                    seq,
                    version,
                    Some(reason),
                    Vec::new(),
                )),
                ..Default::default()
            },
        };
        let version = self.version;
        let freed = if outcome.operation.is_none() {
            self.release()
        } else {
            Vec::new()
        };
        Turned {
            outcome: Outcome::Audio(Box::new(outcome)),
            stepped: None,
            corrections: Vec::new(),
            freed,
            version,
        }
    }

    fn audio_next(&mut self, ticket: Ticket) -> Value {
        let dispatch = self.audio_runtime.next(ticket);
        self.dispatch_audio(dispatch)
    }

    fn dispatch_audio(
        &mut self,
        dispatch: Dispatch<audio::operations::Commit, audio::Outcome>,
    ) -> Value {
        match dispatch {
            Dispatch::Finish(finished) => self.finish_audio(finished),
            Dispatch::Response(response) => response,
            Dispatch::Repaired(repair) => {
                let freed = self.release();
                let echo = repair.echo(self.version);
                json!({"complete":true,"committed":false,"repaired":true,"outcome":repair.outcome,"echo":echo,"returned":{"member":repair.ticket.target.member,"server":repair.ticket.target.server,"buffers":repair.returned},
                    "reason":repair.reason,"freed":freed,"version":self.version})
            }
        }
    }

    fn finish_audio(
        &mut self,
        finished: crate::application::operations::Finished<audio::operations::Commit>,
    ) -> Value {
        if let Some(walk) = self.audio_runtime.take_walk(finished.ticket) {
            return self.finish_audio_walk(finished, walk);
        }
        let target = finished.ticket.target;
        let Some(seat) = self.seats.get_mut(target.member as usize) else {
            return json!({"error":"the operation owner is absent"});
        };
        let Member::Audio(editor) = &mut seat.member else {
            return json!({"error":"the operation owner is absent"});
        };
        let ticket = finished.ticket;
        let failure = finished.result.clone();
        let completion = self.audio_runtime.finish_turn(
            editor.as_mut(),
            &mut self.services,
            finished,
            &mut self.version,
        );
        if !completion.committed {
            return self.audio_runtime.refuse_turn(
                ticket,
                failure,
                completion,
                &self.services.resources,
                self.version,
            );
        }
        let mut outcome = completion.outcome;
        let recovery = if self.services.resources.valid(target.server) {
            steps_json(&completion.recovery)
        } else {
            json!([])
        };
        let destination = outcome.saved.clone().unwrap_or_else(|| {
            editor
                .over_file(target.object, |editor| match editor.target() {
                    audio::Target::File(path) => json!({"path":path}),
                    audio::Target::Buffer(buffer) => json!({"buffer":buffer}),
                })
                .unwrap_or(Value::Null)
        });
        if outcome.binding.is_some() {
            self.select_member(target.member);
        }
        if let Some(binding) = outcome.binding.as_mut() {
            binding["structure"] = json!(self.structure(target.member));
            binding["version"] = json!(self.version);
        }
        let freed = self.release();
        json!({"complete":true,"committed":completion.committed,"destination":destination,"outcome":outcome,"recovery":{"member":target.member,"server":target.server,"steps":recovery},"freed":freed,"version":self.version})
    }

    pub(super) fn prepare_audio_walk(&mut self, direction: Direction) -> Option<Stepped> {
        let scope = self.services.histories.selected();
        let expected = self.services.histories.peek_walk(direction)?;
        self.sync_resources();
        let mut plans = Vec::new();
        let mut previews = Vec::new();
        for (structure, payloads) in &expected.legs {
            let payload = payloads.last()?.clone();
            for (owner, seat) in self.seats.iter_mut().enumerate() {
                let member = owner as MemberId;
                let object = self
                    .services
                    .histories
                    .object_for(member, *structure)
                    .map(|object| object.id);
                if seat.structure != *structure && object.is_none() {
                    continue;
                }
                if let Member::Audio(editor) = &mut seat.member {
                    let object = object.unwrap_or(editor.focused_file());
                    let request = Request {
                        member,
                        server: seat.server,
                        version: self.version,
                        seq: 0,
                        command: Command::Restore(payload.clone()),
                    };
                    let prepared =
                        editor.over_file(object, |editor| editor.prepare(request, &self.services));
                    match prepared {
                        Some(Ok(plan)) => {
                            let file = editor
                                .over_file(object, |editor| editor.playback_file())
                                .unwrap();
                            previews.push(Effect::Audio {
                                member,
                                steps: json!([]),
                                file,
                            });
                            plans.push((member, plan));
                        }
                        _ => {
                            return Some(self.refused_audio_walk(
                                "the recorded object cannot prepare this history step",
                            ));
                        }
                    }
                }
            }
        }
        if plans.is_empty() {
            return None;
        }
        let ticket = match self.audio_runtime.prepare_walk(
            &mut self.services.resources,
            scope,
            direction,
            expected,
            plans,
        ) {
            Ok(ticket) => ticket,
            Err(reason) => return Some(self.refused_audio_walk(reason)),
        };
        let (batch, work) = self
            .audio_runtime
            .operations
            .next(ticket)
            .expect("audio restore has a stitch batch");
        Some(Stepped {
            stepped: true,
            reason: None,
            operation: Some(crate::application::operations::Prepared {
                member: work.member,
                ticket,
                batch,
                server: work.server,
                steps: work.steps,
            }),
            effects: previews,
            corrections: Vec::new(),
            freed: Vec::new(),
            version: self.version,
        })
    }

    fn refused_audio_walk(&self, reason: &str) -> Stepped {
        Stepped {
            stepped: false,
            operation: None,
            reason: Some(reason.into()),
            effects: Vec::new(),
            corrections: Vec::new(),
            freed: Vec::new(),
            version: self.version,
        }
    }

    fn finish_audio_walk(
        &mut self,
        finished: crate::application::operations::Finished<audio::operations::Commit>,
        walk: PendingWalk<audio::operations::Commit>,
    ) -> Value {
        let decision = crate::application::walks::finish(self, finished, walk);
        let (ticket, receipt) = (decision.ticket, decision.receipt);
        let mut stepped = match decision.result {
            Ok(stepped) => stepped,
            Err(refused) => {
                return self.audio_runtime.refuse_walk(
                    ticket,
                    decision.additional_targets,
                    receipt,
                    refused,
                    &self.services.resources,
                    self.version,
                );
            }
        };
        for effect in &mut stepped.effects {
            if let Effect::Audio { steps, .. } = effect {
                *steps = json!([]);
            }
        }
        let echo = self.audio_runtime.finish_walk(
            &mut self.services.resources,
            ticket,
            receipt,
            stepped.reason.clone(),
            self.version,
        );
        let mut freed = std::mem::take(&mut stepped.freed);
        freed.extend(self.release());
        json!({"complete":true,"committed":stepped.stepped,"stepped":stepped,"echo":echo,"recoveries":[],"freed":freed,"version":self.version})
    }

    pub(super) fn audio_operation_json(&mut self, request: &Value) -> Value {
        if request["action"] != "prepare" {
            let control = match serde_json::from_value::<Control>(request.clone()) {
                Ok(control) => control,
                Err(error) => return json!({"error":error.to_string()}),
            };
            let dispatch =
                self.audio_runtime
                    .control(&mut self.services.resources, control, self.version);
            return self.dispatch_audio(dispatch);
        }
        let request = match serde_json::from_value::<OperationCall>(request.clone()) {
            Ok(request) => request,
            Err(error) => return json!({"error":error.to_string()}),
        };
        match request {
            OperationCall::Prepare { member, command } => {
                let command = match command {
                    AudioCommand::Open => Command::Open,
                    AudioCommand::Save {
                        path,
                        buffer,
                        format,
                    } => Command::Save {
                        target: path
                            .map(audio::Target::File)
                            .or_else(|| buffer.map(audio::Target::Buffer)),
                        format,
                    },
                    AudioCommand::Edit { event } => match self.decode_audio_event(member, &event) {
                        Some(command) => command,
                        None => return json!({"error":"the event is not a sample operation"}),
                    },
                    AudioCommand::Restore { payload } => Command::Restore(Opaque(payload)),
                };
                match self.prepare_audio(member, command, 0) {
                    Ok(ticket) => self.audio_next(ticket),
                    Err(error) => json!({"error":error}),
                }
            }
        }
    }
}

impl crate::application::walks::WalkHost<audio::operations::Commit> for Editing {
    type Stepped = Stepped;
    type Outcome = audio::Outcome;
    fn confirm_walk(
        &mut self,
        walk: &PendingWalk<audio::operations::Commit>,
    ) -> Result<Stepped, String> {
        self.services.histories.select(walk.scope);
        if self.services.histories.peek_walk(walk.direction).as_ref() != Some(&walk.expected) {
            return Err("the editing order changed while the step was pending".into());
        }
        let stepped = self.step(walk.direction);
        if stepped.stepped {
            Ok(stepped)
        } else {
            Err(stepped
                .reason
                .unwrap_or_else(|| "the history step was refused".into()))
        }
    }
    fn abort_walk(
        &mut self,
        member: u32,
        delta: audio::operations::Commit,
        result: OperationResult,
        completed: usize,
    ) -> audio::operations::Completion {
        let Member::Audio(editor) = &mut self.seats[member as usize].member else {
            unreachable!()
        };
        editor.abort(delta, result, completed, &mut self.services)
    }
    fn walk_server(&self, member: u32) -> ServerInstance {
        self.seats[member as usize].server
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_gesture_decoding_supplies_endpoint_scoped_clipboard_reads() {
        let mut editing = Editing::new();
        let opened = call_json(
            &mut editing,
            &json!({
                "verb":"openAudio", "key":"take:3", "path":"/in/take3.wav",
                "workspace":"/work", "frames":100, "display":9
            })
            .to_string(),
        );
        let member = serde_json::from_str::<Value>(&opened).unwrap()["member"]
            .as_u64()
            .unwrap() as MemberId;
        call_json(
            &mut editing,
            &json!({"verb":"member", "member":member,
            "call":{"verb":"window", "widget":12}})
            .to_string(),
        );
        let document = clausters_document::clipboard::Clipboard {
            content: clausters_document::clipboard::Content::Buffer {
                buffer: 77,
                frames: 10,
                channels: 2,
                sample_rate: 48000.0,
                location: None,
            },
            origin: None,
        }
        .to_json();
        let id = ResourceId {
            server: editing.seats[member as usize].server,
            buffer: 77,
        };
        let foreign = Event {
            addr: "/gui_event".into(),
            args: vec![
                json!(999),
                json!(1),
                json!(0),
                json!("paste"),
                json!(0),
                json!(0),
                json!(document),
            ],
        };
        assert!(editing.decode_audio_event(member, &foreign).is_none());
        assert!(
            editing.services.resources.source(id).is_none(),
            "foreign gestures cannot register clipboard resources"
        );
        for tag in ["paste", "mix"] {
            let event = Event {
                addr: "/gui_event".into(),
                args: vec![
                    json!(12),
                    json!(1),
                    json!(0),
                    json!(tag),
                    json!(0),
                    json!(0),
                    json!(document),
                ],
            };
            let Some(audio::operations::Command::Paste {
                clipboard,
                event: decoded,
            }) = editing.decode_audio_event(member, &event)
            else {
                panic!("paste command expected")
            };
            assert_eq!(clipboard, Some(id));
            assert_eq!(decoded.args, event.args);
            let source = editing.services.resources.source(id).unwrap();
            assert_eq!(source.bytes, 80);
            assert!(!source.owned);
        }
        let event = Event {
            addr: "/gui_event".into(),
            args: vec![json!(12), json!(1), json!(0), json!("locate")],
        };
        assert!(editing.prepare_audio_event(member, &event).is_none());
    }
}
