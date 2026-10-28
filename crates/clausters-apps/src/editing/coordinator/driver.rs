//! **An audio member's run carried as far as it goes, in its caller's
//! process**: every await answered in place through what the caller does
//! ([`Caller`]), and only a render handed back, since a render is the one
//! thing that takes as long as a take is long.
//!
//! The coordinator's runs stop on an await whenever what follows depends on
//! something outside the context: a file's header, the files on disk, steps
//! carried out, a history step's operation, the clipboard. The application
//! that holds the editor's engine answers all of them at once except a
//! render, which it runs on a thread of its own; this is the loop that
//! answers them, so the application only says how ([`Caller`]) and runs
//! the renders it is handed ([`Ran::render`]).

use clausters_editing::apply::{Step, steps_from_json};

use super::run::{Await, End, Failure};
use super::*;
use crate::editing::{Caller, Raised, Ran, Render};

impl Editing {
    /// **Starts `action` on audio member `member`** and carries it as far as
    /// it goes: to its end, or to a render it hands back. `lease` says the
    /// action holds the clipboard lease its event was prepared with; `ids`
    /// are the engine's, which its playback is allocated from.
    pub(crate) fn audio_act(
        &mut self,
        member: MemberId,
        action: &Value,
        lease: bool,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
    ) -> Ran {
        let mut ran = Ran::default();
        self.audio_start(member, action, lease, false, caller, ids, &mut ran);
        ran
    }

    /// **A render landed**: the run it was for resumed with how it went,
    /// and carried on as far as it goes.
    pub(crate) fn audio_landed(
        &mut self,
        member: MemberId,
        run: u64,
        result: Result<(), String>,
        lease: bool,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
    ) -> Ran {
        let mut ran = Ran::default();
        let answer = resume(self, member, run, result.map(|()| json!({})), ids);
        self.audio_drive(member, answer, lease, false, caller, ids, &mut ran);
        ran
    }

    /// **What a script reads of an audio member**: `transport` (its
    /// transport, span, loop switch and state) or `output` (its meter's
    /// levels and the samples played as silence).
    pub(crate) fn audio_query(&mut self, member: MemberId, query: &str) -> Value {
        super::query(self, member, query)
    }

    #[allow(clippy::too_many_arguments)]
    fn audio_start(
        &mut self,
        member: MemberId,
        action: &Value,
        lease: bool,
        inline: bool,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) {
        let answer = begin(self, member, action, ids);
        self.audio_drive(member, answer, lease, inline, caller, ids, ran);
    }

    /// Carries a run on from `answer` until it ends or hands a render off.
    /// `inline` carries a render in place (a history step's).
    #[allow(clippy::too_many_arguments)]
    fn audio_drive(
        &mut self,
        member: MemberId,
        mut answer: Result<Answer, String>,
        lease: bool,
        inline: bool,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) {
        loop {
            let Answer { run, effects, end } = match answer {
                Ok(answer) => answer,
                Err(message) => {
                    ran.end = Some(Err(Raised {
                        kind: "runtime".into(),
                        message,
                    }));
                    return;
                }
            };
            for effect in effects {
                self.audio_effect(member, effect, lease, caller, ran);
            }
            let wait = match end {
                End::Done(result) => {
                    ran.end = Some(Ok(result));
                    return;
                }
                End::Failed(failure) => {
                    ran.end = Some(Err(raised(failure)));
                    return;
                }
                End::Waits(wait) => wait,
            };
            let result = match wait {
                Await::Steps(steps) => {
                    let steps = steps_from_json(&steps);
                    let renders = steps.iter().any(|s| matches!(s, Step::Render(_)));
                    if renders && !inline {
                        ran.render = Some(Render { run, steps });
                        return;
                    }
                    caller.carry(&steps).map(|()| json!({}))
                }
                Await::Shape(file) => shape(&file, caller),
                Await::Stat(paths) => Ok(json!({"stamps": caller.stamps(&strings(&paths))})),
                Await::Stepped(stepped) => {
                    Ok(self.audio_stepped(member, &stepped, caller, ids, ran))
                }
                Await::Clipboard { document, ready } => {
                    self.audio_clipboard(member, document, ready, ran)
                }
            };
            answer = resume(self, member, run, result, ids);
        }
    }

    /// One effect of a run: carried out here when it is the application's,
    /// handed to the window otherwise.
    fn audio_effect(
        &mut self,
        member: MemberId,
        effect: Value,
        lease: bool,
        caller: &mut dyn Caller,
        ran: &mut Ran,
    ) {
        match effect["kind"].as_str().unwrap_or_default() {
            // the context keeps its own version
            "version" => {}
            "release" => {
                for freed in effect["freed"].as_array().into_iter().flatten() {
                    caller.delete(&strings(&freed["files"]));
                }
            }
            "delete" => caller.delete(&strings(&effect["files"])),
            "call_off" => caller.call_off(member),
            "operationEcho" => ran.effects.push(json!({"kind": "echo",
                "answer": effect["echo"]["answer"]})),
            "lease" => {
                if lease {
                    self.exchange_json(&json!({"action": "release", "lease": effect["lease"]}));
                }
            }
            _ => ran.effects.push(effect),
        }
    }

    /// **A step of the history carried out** -- one the context already took
    /// -- and whether anything moved: the operation it names run to its end,
    /// renders included, the files it changed sounded again, and every
    /// widget drawn again.
    fn audio_stepped(
        &mut self,
        member: MemberId,
        stepped: &Value,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Value {
        if stepped["stepped"] != true {
            return json!({"changed": false});
        }
        self.audio_carry_step(member, stepped, caller, ids, ran);
        for freed in stepped["freed"].as_array().into_iter().flatten() {
            caller.delete(&strings(&freed["files"]));
        }
        ran.effects.push(json!({"kind": "resync"}));
        json!({"changed": true})
    }

    fn audio_carry_step(
        &mut self,
        member: MemberId,
        stepped: &Value,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) {
        if let Some(operation) = stepped.get("operation").filter(|o| !o.is_null()) {
            let at = member_of(operation, member);
            let mut nested = Ran::default();
            self.audio_start(
                at,
                &json!({"verb": "operation", "prepared": operation}),
                false,
                true,
                caller,
                ids,
                &mut nested,
            );
            ran.effects.extend(nested.effects);
            if let Some(Ok(completed)) = nested.end {
                self.audio_carry_step(at, &completed["stepped"], caller, ids, ran);
            }
            return;
        }
        for effect in stepped["effects"].as_array().into_iter().flatten() {
            if effect["kind"] != "audio" || effect["file"].is_null() {
                continue;
            }
            let at = member_of(effect, member);
            let mut nested = Ran::default();
            self.audio_start(
                at,
                &json!({"verb": "syncSound", "file": effect["file"]}),
                false,
                true,
                caller,
                ids,
                &mut nested,
            );
            ran.effects.extend(nested.effects);
        }
    }

    /// **A copy published**: the clipboard names the spans of files it is
    /// of, and the window's clipboard holds the document.
    fn audio_clipboard(
        &mut self,
        member: MemberId,
        mut document: Value,
        ready: bool,
        ran: &mut Ran,
    ) -> Result<Value, String> {
        if !ready {
            let published = self.exchange_json(
                &json!({"action": "publish", "member": member, "document": document}),
            );
            if let Some(error) = published["error"].as_str() {
                return Err(error.into());
            }
            document = published["document"].clone();
        }
        ran.effects
            .push(json!({"kind": "clipboard", "document": document}));
        Ok(json!({}))
    }
}

/// The member a step's part names, or `member`.
fn member_of(value: &Value, member: MemberId) -> MemberId {
    value["member"]
        .as_u64()
        .and_then(|m| MemberId::try_from(m).ok())
        .unwrap_or(member)
}

/// What a run that failed raises: a failure of its caller's as an
/// operation refused, one of the coordinator's as it names it.
fn raised(failure: Failure) -> Raised {
    match failure {
        Failure::Client(message) => Raised {
            kind: "command".into(),
            message,
        },
        Failure::Own(raise, message) => Raised {
            kind: raise.name().into(),
            message,
        },
    }
}

/// **A file asked for**, by the window or a script: its header read (a new
/// file is the shape it is asked as). A file a part cannot read in place is
/// decoded when the editor opens it.
fn shape(file: &Value, caller: &mut dyn Caller) -> Result<Value, String> {
    if let Some(shape) = file.get("shape").filter(|s| !s.is_null()) {
        return Ok(
            json!({"frames": shape["frames"], "channels": shape["channels"],
            "rate": shape["rate"], "path": null, "untitled": true,
            "codec": null, "sample_format": null}),
        );
    }
    let path = file["path"].as_str().unwrap_or_default();
    let info = caller.header(path)?;
    Ok(json!({
        "frames": info.frames, "channels": info.channels,
        "rate": info.rate, "path": path, "untitled": false,
        "codec": info.codec, "sample_format": info.sample_format,
    }))
}

/// The strings of a JSON array.
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}
