//! **A run carried as far as it goes, in its caller's process**: every await
//! answered in place through what the caller does ([`Services`]), and only a
//! render handed back, since a render is the one thing that takes as long as
//! a take is long.
//!
//! The coordinator's runs stop on an await whenever what follows depends on
//! something outside the context: a file's header, the files on disk, steps
//! carried out, a history step's operation, the clipboard. The application
//! that holds the editor's engine answers all of them at once except a
//! render, which it runs on a thread of its own; this is the loop that
//! answers them, so the application only says how ([`Services`]) and runs
//! the renders it is handed ([`Ran::render`]).

use clausters_editing::apply::{Step, steps_from_json};

use super::*;

/// **What the caller does for a run**, at once: the I/O and the threads are
/// the caller's, never the coordinator's.
pub trait Services {
    /// The header of the sound file at `path`.
    fn header(&mut self, path: &str) -> Result<Header, String>;
    /// **The files at `paths`, as they are on disk**: each path to
    /// `{"modified", "size"}` (milliseconds since the epoch, bytes), or
    /// `null` for one that is not there.
    fn stamps(&mut self, paths: &[String]) -> Value;
    /// **Carries `steps` out** in order, renders included, on the caller's
    /// turn: what a history step does, and a batch with no render.
    fn carry(&mut self, steps: &[Step]) -> Result<(), String>;
    /// Deletes `files`: renders nothing names any more.
    fn delete(&mut self, files: &[String]);
    /// Calls off member `member`'s renders under way, each before its next
    /// step.
    fn call_off(&mut self, member: MemberId);
}

/// A sound file's header, as [`Services::header`] reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    /// Frames per channel.
    pub frames: u64,
    /// Channels.
    pub channels: u32,
    /// The rate it was written at.
    pub rate: f64,
    /// Its container (`wav`, `w64`, `rf64`, `flac`, ...).
    pub codec: String,
    /// Its sample format (`float`, `int24`, `int16`, ...), where it says.
    pub sample_format: Option<String>,
}

/// The containers a part of a list reads in place.
const IN_PLACE: &[&str] = &["wav", "w64", "rf64"];

/// **What an action raised**: its kind, as the coordinator names it
/// (`value`, `command`, `runtime`), and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Raised {
    /// `value` (a request the editor cannot take), `command` (an operation
    /// refused) or `runtime` (a state the editor is not in any more).
    pub kind: String,
    /// Why.
    pub message: String,
}

/// **A render a run waits on**, handed to the caller to run off its turn:
/// the steps, and the run it resumes ([`Editing::audio_landed`]).
#[derive(Debug)]
pub struct Render {
    /// The run it is for.
    pub run: u64,
    /// The steps, a render among them.
    pub steps: Vec<Step>,
}

/// **An action carried as far as it goes**: the effects that are the
/// window's, in order, and its end, or the render it waits on.
#[derive(Debug, Default)]
pub struct Ran {
    /// The window's effects, each `{"kind", ...}`: `echo`, `set`, `publish`,
    /// `band`, `looping`, `bind`, `layers`, `cursor`, `locate`, `changed`,
    /// `closeWindow`, `closedWindow`; and `resync` (every widget drawn
    /// again, after a history step) and `clipboard` (`document`, the
    /// clipboard's).
    pub effects: Vec<Value>,
    /// Its result, or what it raised; `None` while it waits on a render.
    pub end: Option<Result<Value, Raised>>,
    /// The render it waits on, for the caller to run and land.
    pub render: Option<Render>,
}

impl Editing {
    /// **Starts `action` on audio member `member`** and carries it as far as
    /// it goes: to its end, or to a render it hands back. `lease` says the
    /// action holds the clipboard lease its event was prepared with; `ids`
    /// are the engine's, which its playback is allocated from.
    pub fn audio_act(
        &mut self,
        member: MemberId,
        action: &Value,
        lease: bool,
        services: &mut dyn Services,
        ids: &mut IdSpaces,
    ) -> Ran {
        let mut ran = Ran::default();
        self.audio_start(member, action, lease, false, services, ids, &mut ran);
        ran
    }

    /// **A render landed**: the run it was for resumed with how it went,
    /// and carried on as far as it goes.
    pub fn audio_landed(
        &mut self,
        member: MemberId,
        run: u64,
        result: Result<(), String>,
        lease: bool,
        services: &mut dyn Services,
        ids: &mut IdSpaces,
    ) -> Ran {
        let mut ran = Ran::default();
        let answer = call(
            self,
            &json!({"member": member, "run": run, "result": carried(result)}),
            ids,
        );
        self.audio_drive(member, answer, lease, false, services, ids, &mut ran);
        ran
    }

    /// **What a script reads of an audio member**: `transport` (its
    /// transport, span, loop switch and state) or `output` (its meter's
    /// levels and the samples played as silence).
    pub fn audio_query(&mut self, member: MemberId, query: &str, ids: &mut IdSpaces) -> Value {
        call(self, &json!({"member": member, "query": query}), ids)
    }

    #[allow(clippy::too_many_arguments)]
    fn audio_start(
        &mut self,
        member: MemberId,
        action: &Value,
        lease: bool,
        inline: bool,
        services: &mut dyn Services,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) {
        let answer = call(self, &json!({"member": member, "do": action}), ids);
        self.audio_drive(member, answer, lease, inline, services, ids, ran);
    }

    /// Carries a run on from `answer` until it ends or hands a render off.
    /// `inline` carries a render in place (a history step's).
    #[allow(clippy::too_many_arguments)]
    fn audio_drive(
        &mut self,
        member: MemberId,
        mut answer: Value,
        lease: bool,
        inline: bool,
        services: &mut dyn Services,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) {
        loop {
            let Some(run) = answer["run"].as_u64() else {
                ran.end = Some(Err(Raised {
                    kind: "runtime".into(),
                    message: answer["error"]
                        .as_str()
                        .unwrap_or("the audio editor did not answer")
                        .into(),
                }));
                return;
            };
            for effect in answer["effects"].as_array().cloned().unwrap_or_default() {
                self.audio_effect(member, effect, lease, services, ran);
            }
            if answer["done"] == true {
                ran.end = Some(end_of(&answer));
                return;
            }
            let wait = answer["await"].clone();
            let result = match wait["kind"].as_str().unwrap_or_default() {
                "steps" | "batch" => {
                    let steps = steps_from_json(&wait["steps"]);
                    let renders = steps.iter().any(|s| matches!(s, Step::Render(_)));
                    if renders && !inline {
                        ran.render = Some(Render { run, steps });
                        return;
                    }
                    carried(services.carry(&steps))
                }
                "shape" => shape(&wait, services),
                "stat" => {
                    let paths: Vec<String> = wait["paths"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| p.as_str().map(str::to_string))
                        .collect();
                    json!({"stamps": services.stamps(&paths)})
                }
                "stepped" => self.audio_stepped(member, &wait["stepped"], services, ids, ran),
                "clipboard" => self.audio_clipboard(member, &wait, ran),
                other => json!({"failed": format!("the audio editor cannot wait on {other:?}")}),
            };
            answer = call(
                self,
                &json!({"member": member, "run": run, "result": result}),
                ids,
            );
        }
    }

    /// One effect of a run: carried out here when it is the application's,
    /// handed to the window otherwise.
    fn audio_effect(
        &mut self,
        member: MemberId,
        effect: Value,
        lease: bool,
        services: &mut dyn Services,
        ran: &mut Ran,
    ) {
        match effect["kind"].as_str().unwrap_or_default() {
            // the context keeps its own version
            "version" => {}
            "release" => {
                for freed in effect["freed"].as_array().into_iter().flatten() {
                    services.delete(&strings(&freed["files"]));
                }
            }
            "delete" => services.delete(&strings(&effect["files"])),
            "call_off" => services.call_off(member),
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
        services: &mut dyn Services,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Value {
        if stepped["stepped"] != true {
            return json!({"changed": false});
        }
        self.audio_carry_step(member, stepped, services, ids, ran);
        for freed in stepped["freed"].as_array().into_iter().flatten() {
            services.delete(&strings(&freed["files"]));
        }
        ran.effects.push(json!({"kind": "resync"}));
        json!({"changed": true})
    }

    fn audio_carry_step(
        &mut self,
        member: MemberId,
        stepped: &Value,
        services: &mut dyn Services,
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
                services,
                ids,
                &mut nested,
            );
            ran.effects.extend(nested.effects);
            if let Some(Ok(completed)) = nested.end {
                self.audio_carry_step(at, &completed["stepped"], services, ids, ran);
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
                services,
                ids,
                &mut nested,
            );
            ran.effects.extend(nested.effects);
        }
    }

    /// **A copy published**: the clipboard names the spans of files it is
    /// of, and the window's clipboard holds the document.
    fn audio_clipboard(&mut self, member: MemberId, wait: &Value, ran: &mut Ran) -> Value {
        let mut document = wait["document"].clone();
        if wait["ready"] != true {
            let published = self.exchange_json(
                &json!({"action": "publish", "member": member, "document": document}),
            );
            if let Some(error) = published["error"].as_str() {
                return json!({ "failed": error });
            }
            document = published["document"].clone();
        }
        ran.effects
            .push(json!({"kind": "clipboard", "document": document}));
        json!({})
    }
}

/// The member a step's part names, or `member`.
fn member_of(value: &Value, member: MemberId) -> MemberId {
    value["member"]
        .as_u64()
        .and_then(|m| MemberId::try_from(m).ok())
        .unwrap_or(member)
}

/// How a run ended: its result, or what it raised.
fn end_of(answer: &Value) -> Result<Value, Raised> {
    if answer["rethrow"] == true {
        return Err(Raised {
            kind: "command".into(),
            message: answer["reason"].as_str().unwrap_or_default().into(),
        });
    }
    match answer.get("error").filter(|e| !e.is_null()) {
        Some(error) => Err(Raised {
            kind: error["type"].as_str().unwrap_or("runtime").into(),
            message: error["message"].as_str().unwrap_or_default().into(),
        }),
        None => Ok(answer.get("result").cloned().unwrap_or(Value::Null)),
    }
}

/// Carried steps as an await's result.
fn carried(outcome: Result<(), String>) -> Value {
    match outcome {
        Ok(()) => json!({}),
        Err(failed) => json!({ "failed": failed }),
    }
}

/// **A file asked for**, by the window or a script: its header read (a new
/// file is the shape it is asked as), and refused where a part could not
/// read it in place.
fn shape(wait: &Value, services: &mut dyn Services) -> Value {
    if let Some(shape) = wait.get("shape").filter(|s| !s.is_null()) {
        return json!({"frames": shape["frames"], "channels": shape["channels"],
            "rate": shape["rate"], "path": null, "untitled": true,
            "codec": null, "sample_format": null});
    }
    let path = wait["path"].as_str().unwrap_or_default();
    match services.header(path) {
        Ok(info) if IN_PLACE.contains(&info.codec.as_str()) => json!({
            "frames": info.frames, "channels": info.channels,
            "rate": info.rate, "path": path, "untitled": false,
            "codec": info.codec, "sample_format": info.sample_format,
        }),
        Ok(info) => json!({"failed": format!(
            "{path} is a {} file, which the audio editor does not read in place", info.codec)}),
        Err(e) => json!({ "failed": e }),
    }
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
