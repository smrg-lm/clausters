//! **What each action starts with**: the tasks an action of the editor's
//! door is, and what the transport's answers lead to.

use super::run::*;
use super::*;

/// The verbs a script plays the transport with, which need a file open.
const TRANSPORT: &[&str] = &[
    "playing", "play", "pause", "stop", "locate", "span", "looping",
];

impl Session {
    pub(super) fn start(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        action: &Value,
        ids: &mut IdSpaces,
    ) -> Value {
        let id = self.next;
        self.next += 1;
        let mut run = Run {
            window: action["window"] == true,
            ..Default::default()
        };
        let verb = action["verb"].as_str().unwrap_or_default();
        if TRANSPORT.contains(&verb) && file_id(editing, member).is_none() {
            return finished(id, Raise::Value, NO_FILE);
        }
        let tasks: Vec<Task> = match verb {
            "open" => {
                if self.playback.transport().is_none()
                    && let Err(error) = self.playback.alloc_transport(ids)
                {
                    return finished(id, Raise::Runtime, &error.to_string());
                }
                let mut tasks = Vec::new();
                if action["empty"] != true {
                    tasks.push(Task::Operation {
                        command: Some(json!({"kind": "open"})),
                        prepared: None,
                        repairing: false,
                    });
                }
                tasks.push(Task::Sound);
                tasks.push(Task::Answer(
                    json!({"transport": self.playback.transport()}),
                ));
                tasks
            }
            // **The window's work called off** (its progress panel's button,
            // or Escape): the client stops the renders under way, each before
            // its next piece, and a batch that stops so deletes what it wrote.
            "event" if action["args"][3] == "call_off" => {
                vec![effect("call_off", json!({}))]
            }
            "event" => {
                let turned = door(
                    editing,
                    json!({"verb": "event", "member": member,
                        "addr": action["addr"], "args": action["args"]}),
                );
                self.turned(&mut run, turned, action["lease"] == true)
            }
            "act" => {
                let turned = door(
                    editing,
                    json!({"verb": "act", "member": member, "call": action["call"]}),
                );
                let outcome = turned.get("outcome").cloned().unwrap_or(json!({}));
                run.changed = outcome["changed"] == true;
                let mut tasks = vec![
                    effect("version", json!({"version": turned["version"]})),
                    Task::Take {
                        outcome: outcome.clone(),
                        lease: false,
                    },
                ];
                tasks.extend(release(&turned["freed"]));
                run.reason = Some(outcome["answer"]["reason"].clone());
                tasks.push(Task::Reason(outcome["answer"]["reason"].clone()));
                tasks
            }
            "verb" => {
                let answer = member_call(editing, member, action["call"].clone());
                if answer["error"] == NO_FILE {
                    return finished(id, Raise::Value, NO_FILE);
                }
                run.changed = answer["changed"] == true;
                run.reason = Some(answer["answer"]["reason"].clone());
                vec![
                    Task::Take {
                        outcome: answer.clone(),
                        lease: false,
                    },
                    Task::Reason(answer["answer"]["reason"].clone()),
                ]
            }
            "save" => {
                let mut command = json!({"kind": "save"});
                if let (Some(map), Value::Object(request)) =
                    (command.as_object_mut(), &action["request"])
                {
                    map.extend(request.clone());
                }
                vec![
                    Task::Operation {
                        command: Some(command),
                        prepared: None,
                        repairing: false,
                    },
                    Task::TakeCompleted,
                    Task::Destination,
                ]
            }
            "operation" => vec![Task::Operation {
                command: action.get("command").filter(|c| !c.is_null()).cloned(),
                prepared: action.get("prepared").filter(|p| !p.is_null()).cloned(),
                repairing: action["repairing"] == true,
            }],
            "retry" => {
                let pending = door(
                    editing,
                    json!({"verb": "operation", "action": "pendingRepairs", "member": member}),
                );
                pending["repairs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|repair| {
                        let prepared = door(
                            editing,
                            json!({"verb": "operation", "action": "retryRepair",
                                "ticket": repair["ticket"]}),
                        );
                        Task::Operation {
                            command: None,
                            prepared: Some(prepared),
                            repairing: true,
                        }
                    })
                    .collect()
            }
            "openFile" => {
                let answer = member_call(
                    editing,
                    member,
                    json!({"verb": "open_file", "paths": action["paths"]}),
                );
                if let Some(error) = answer["error"].as_str() {
                    return finished(id, Raise::Value, error);
                }
                vec![Task::Read(paths(&answer["openFile"]["paths"]))]
            }
            "newFile" => vec![Task::Read(VecDeque::from([
                json!({"shape": action["shape"]}),
            ]))],
            "sound" => vec![Task::Sound],
            "syncSound" => vec![Task::SyncSound(action["file"].clone())],
            "close" => vec![Task::Close { closed: false }],
            "closed" => vec![Task::Close { closed: true }],
            "playing" => vec![Task::Rolling(Rolled::Playing {
                was: self.playback.rolling(),
            })],
            "play" => self.script_play(editing, member, action["at"].as_f64()),
            "pause" => {
                let mut tasks = Vec::new();
                if self.focused(editing, member) {
                    tasks.push(Task::Playback(Op::Pause));
                    self.paused = true;
                    tasks.push(Task::ShowTransport);
                }
                tasks
            }
            "stop" => {
                self.paused = false;
                if self.focused(editing, member) {
                    self.script_stop(editing, member)
                } else {
                    vec![Task::Rolling(Rolled::Stop)]
                }
            }
            "locate" => self.script_locate(editing, member, action["at"].as_f64().unwrap_or(0.0)),
            "span" => {
                let span = action["span"]
                    .as_array()
                    .and_then(|s| Some((s.first()?.as_f64()?, s.get(1)?.as_f64()?)));
                let band = member_call(
                    editing,
                    member,
                    json!({"verb": "span", "span": span.map(|(a, b)| json!([a, b]))}),
                );
                let mut tasks = Vec::new();
                if action["show"] != false {
                    tasks.push(effect(
                        "band",
                        json!({"sel_start": band["sel_start"], "sel_len": band["sel_len"]}),
                    ));
                }
                tasks.extend(self.follow(editing, member));
                tasks
            }
            "looping" => {
                let on = action["on"] == true;
                member_call(editing, member, json!({"verb": "looping", "on": on}));
                let mut tasks = self.follow(editing, member);
                tasks.push(effect("looping", json!({"on": on})));
                tasks
            }
            other => return finished(id, Raise::Value, &format!("no audio verb {other:?}")),
        };
        run.tasks.extend(tasks);
        let changed = run.changed;
        let reason = run.reason.take();
        self.runs.insert(id, run);
        let mut answer = self.drive(editing, member, id, ids);
        // known before any await: what a client answers at once
        answer["changed"] = json!(changed);
        if let Some(reason) = reason {
            answer["reason"] = reason;
        }
        answer
    }

    /// What a message to the member came to, as the event handling did.
    pub(super) fn turned(&mut self, run: &mut Run, turned: Value, lease: bool) -> Vec<Task> {
        let outcome = turned.get("outcome").cloned().unwrap_or(json!({}));
        let mut tasks = vec![effect("version", json!({"version": turned["version"]}))];
        if outcome["turn"] == "closed" {
            tasks.push(Task::Close { closed: true });
            tasks.push(Task::Answer(json!(false)));
            return tasks;
        }
        if outcome["turn"] == "step" {
            tasks.push(Task::Stepped(
                turned.get("stepped").cloned().unwrap_or(json!({})),
            ));
            tasks.push(effect("echo", json!({"answer": outcome["answer"]})));
            tasks.extend(release(&turned["freed"]));
            tasks.push(Task::Sound);
            return tasks;
        }
        run.changed = outcome["changed"] == true;
        tasks.push(Task::Take { outcome, lease });
        tasks.extend(release(&turned["freed"]));
        tasks
    }

    pub(super) fn focused(&self, editing: &Editing, member: MemberId) -> bool {
        let file = file_id(editing, member);
        file.is_some() && self.playback.focus() == file.map(|f| f as u64)
    }

    pub(super) fn script_play(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        at: Option<f64>,
    ) -> Vec<Task> {
        if at.is_none() && self.paused && self.focused(editing, member) {
            self.paused = false;
            return vec![Task::Playback(Op::Resume), Task::ShowTransport];
        }
        self.paused = false;
        let mut tasks = Vec::new();
        if let Some(at) = at {
            tasks.extend(self.script_locate(editing, member, at));
        }
        let play = space(editing, member);
        tasks.push(Task::Playback(Op::play(file_id(editing, member), &play)));
        tasks.push(Task::ShowTransport);
        tasks
    }

    pub(super) fn script_stop(&mut self, editing: &mut Editing, member: MemberId) -> Vec<Task> {
        let play = space(editing, member);
        vec![
            Task::Playback(Op::stop(file_id(editing, member), &play)),
            Task::ShowTransport,
        ]
    }

    pub(super) fn script_locate(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        at: f64,
    ) -> Vec<Task> {
        let frame = match editing.member_mut(member) {
            Some(Member::Audio(editor)) => editor.set_cursor(at),
            _ => 0,
        };
        vec![
            effect("cursor", json!({"at": at})),
            Task::Rolling(Rolled::Locate(frame)),
        ]
    }

    /// A pass in progress ends as the span and the switch now say.
    pub(super) fn follow(&mut self, editing: &mut Editing, member: MemberId) -> Vec<Task> {
        if !self.focused(editing, member) {
            return Vec::new();
        }
        let play = space(editing, member);
        vec![Task::Playback(Op::pass(&play["pass"]))]
    }

    /// **The file in focus sounds on the engine**: its list handed to the
    /// engine, and the playback's readers set to the buffers the engine
    /// answers, one per channel.
    pub(super) fn sync_held(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        id: u64,
        file: &Value,
        ids: &mut IdSpaces,
    ) -> Flow {
        let rate = self.rate;
        let Some(wanted) = file["id"].as_u64() else {
            return Flow::Next;
        };
        let take = match editing.member(member) {
            Some(Member::Audio(editor)) => editor
                .held_takes(rate, |file, _| file == wanted)
                .into_iter()
                .next(),
            _ => None,
        };
        let (Some(take), Some(engine)) = (take, editing.engine.as_mut()) else {
            return Flow::Next;
        };
        let buffers = match engine.take(&take, ids) {
            Ok(buffers) => buffers,
            Err(e) => return Flow::Fail(Failure::Own(Raise::Runtime, e)),
        };
        let frames = file["frames"].as_u64().unwrap_or(0);
        let take_rate = file["rate"].as_f64().unwrap_or(0.0);
        if let Err(e) = self
            .playback
            .sync_streams(wanted, buffers, frames, take_rate, rate, ids)
        {
            return Flow::Fail(Failure::Own(Raise::Runtime, e.to_string()));
        }
        self.push(id, vec![Task::RetainPlayback]);
        Flow::Next
    }

    /// What the engine's answer whether the transport rolls leads to.
    pub(super) fn rolled(
        &mut self,
        editing: &mut Editing,
        member: MemberId,
        rolled: Rolled,
        rolling: bool,
    ) -> Vec<Task> {
        let file = file_id(editing, member);
        match rolled {
            Rolled::View { frame } => vec![
                Task::Playback(Op::View {
                    file: file.map_or(0, |f| f as u64),
                    frame,
                }),
                // the meter follows the file now viewed, before the window is drawn
                Task::TellMeters,
            ],
            Rolled::Space(play) => vec![
                // whichever file plays stops, and the one shown comes into focus
                if rolling {
                    Task::Playback(Op::stop(file, &play))
                } else {
                    Task::Playback(Op::play(file, &play))
                },
                Task::ShowTransport,
            ],
            // **A placed cursor moves the music where it plays**: the file in
            // focus rolling goes on from the cursor, its pass ending as the
            // switch says from there; stopped, the transport stands on it.
            Rolled::Cue(frame) if rolling && self.focused(editing, member) => {
                let play = space(editing, member);
                let mut tasks = vec![Task::Playback(Op::Locate(frame))];
                if play["pass"].is_object() {
                    tasks.push(Task::Playback(Op::pass(&play["pass"])));
                }
                tasks.push(Task::ShowTransport);
                tasks
            }
            Rolled::Cue(frame) => vec![Task::Playback(Op::Cue(frame)), Task::ShowTransport],
            Rolled::Playing { was } => {
                let mut tasks = Vec::new();
                if was != rolling {
                    tasks.push(Task::ShowTransport);
                }
                tasks.push(Task::Answer(json!(
                    rolling && self.focused(editing, member)
                )));
                tasks
            }
            Rolled::Stop if rolling => self.script_stop(editing, member),
            Rolled::Stop => Vec::new(),
            Rolled::Locate(frame) => vec![Task::Playback(
                if rolling && self.focused(editing, member) {
                    Op::Locate(frame)
                } else {
                    Op::Cue(frame)
                },
            )],
        }
    }
}
