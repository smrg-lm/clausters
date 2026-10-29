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

use super::run::{Await, Came, End, Failure, Said, Wanted};
use super::*;
use crate::editing::{Caller, Raised, Ran, Render, Stepped};

impl Editing {
    /// **Starts `action` on audio member `member`** and carries it as far as
    /// it goes: to its end, or to a render it hands back. `window` says a
    /// window asked it; `lease` is the clipboard lease its event was
    /// prepared with; `ids` are the engine's, which its playback is
    /// allocated from.
    pub(crate) fn audio_act(
        &mut self,
        member: MemberId,
        action: AudioAction,
        window: bool,
        lease: Option<u64>,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
    ) -> Ran {
        let mut ran = Ran::default();
        self.audio_start(member, action, window, lease, false, caller, ids, &mut ran);
        ran
    }

    /// **A render landed**: the run it was for resumed with how it went,
    /// and carried on as far as it goes.
    pub(crate) fn audio_landed(
        &mut self,
        member: MemberId,
        run: u64,
        result: Result<(), String>,
        lease: Option<u64>,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
    ) -> Ran {
        let mut ran = Ran::default();
        let answer = resume(self, member, run, result.map(|()| Came::Done), ids);
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
        action: AudioAction,
        window: bool,
        lease: Option<u64>,
        inline: bool,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Option<Box<Operated>> {
        let answer = begin(self, member, action, window, ids);
        self.audio_drive(member, answer, lease, inline, caller, ids, ran)
    }

    /// Carries a run on from `answer` until it ends or hands a render off.
    /// `inline` carries a render in place (a history step's). Answers the
    /// operation the run completed last, when it ended.
    #[allow(clippy::too_many_arguments)]
    fn audio_drive(
        &mut self,
        member: MemberId,
        mut answer: Result<Answer, String>,
        lease: Option<u64>,
        inline: bool,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Option<Box<Operated>> {
        loop {
            let Answer { run, effects, end } = match answer {
                Ok(answer) => answer,
                Err(message) => {
                    ran.end = Some(Err(Raised {
                        kind: "runtime".into(),
                        message,
                    }));
                    return None;
                }
            };
            for effect in effects {
                self.audio_effect(member, effect, lease, caller, ran);
            }
            let wait = match end {
                End::Done(result, completed) => {
                    ran.end = Some(Ok(result));
                    return completed;
                }
                End::Failed(failure) => {
                    ran.end = Some(Err(raised(failure)));
                    return None;
                }
                End::Waits(wait) => wait,
            };
            let result = match wait {
                Await::Steps(steps) => {
                    let renders = steps.iter().any(|s| matches!(s, Step::Render(_)));
                    if renders && !inline {
                        ran.render = Some(Render { run, steps });
                        return None;
                    }
                    caller.carry(&steps).map(|()| Came::Done)
                }
                Await::Shape(file) => shape(file, caller).map(Came::File),
                Await::Stat(paths) => Ok(Came::Stamps(caller.stamps(&paths))),
                Await::Stepped(stepped) => Ok(Came::Stepped(
                    self.audio_stepped(member, *stepped, caller, ids, ran),
                )),
                Await::Clipboard { document, ready } => self
                    .audio_clipboard(member, document, ready, ran)
                    .map(|()| Came::Done),
            };
            answer = resume(self, member, run, result, ids);
        }
    }

    /// One effect of a run: carried out here when it is the application's,
    /// handed to the window otherwise.
    fn audio_effect(
        &mut self,
        member: MemberId,
        effect: Said,
        lease: Option<u64>,
        caller: &mut dyn Caller,
        ran: &mut Ran,
    ) {
        match effect {
            // the context keeps its own version
            Said::Window(effect) if effect["kind"] == "version" => {}
            Said::Window(effect) => ran.effects.push(effect),
            Said::Release(freed) => {
                for freed in freed {
                    caller.delete(&freed.files);
                }
            }
            Said::Delete(files) => caller.delete(&files),
            Said::CallOff => caller.call_off(member),
            Said::Lease => {
                if let Some(lease) = lease {
                    self.exchange_release(lease);
                }
            }
        }
    }

    /// **A step of the history carried out** -- one the context already took
    /// -- and whether anything moved: the operation it names run to its end,
    /// renders included, the files it changed sounded again, and every
    /// widget drawn again.
    fn audio_stepped(
        &mut self,
        member: MemberId,
        stepped: Stepped,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> bool {
        if !stepped.stepped {
            return false;
        }
        let _ = member;
        let freed = self.audio_carry_step(stepped, caller, ids, ran);
        for freed in freed {
            caller.delete(&freed.files);
        }
        ran.effects.push(json!({"kind": "resync"}));
        true
    }

    /// The step's operation or its files' effects carried out; answers what
    /// it let go of.
    fn audio_carry_step(
        &mut self,
        stepped: Stepped,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Vec<Freed> {
        let mut freed = stepped.freed;
        if let Some(operation) = stepped.operation {
            let at = operation.member;
            let mut nested = Ran::default();
            let completed = self.audio_start(
                at,
                AudioAction::Operation {
                    command: None,
                    prepared: Some(Box::new(Operated::prepared(operation))),
                    repairing: false,
                },
                false,
                None,
                true,
                caller,
                ids,
                &mut nested,
            );
            ran.effects.extend(nested.effects);
            if let (Some(Ok(_)), Some(stepped)) = (nested.end, completed.and_then(|c| c.stepped)) {
                freed.extend(self.audio_carry_step(stepped, caller, ids, ran));
            }
            return freed;
        }
        for effect in stepped.effects {
            let super::super::Effect::Audio { member: at, file } = effect else {
                continue;
            };
            let mut nested = Ran::default();
            self.audio_start(
                at,
                AudioAction::SyncSound { file },
                false,
                None,
                true,
                caller,
                ids,
                &mut nested,
            );
            ran.effects.extend(nested.effects);
        }
        freed
    }

    /// **A copy published**: the clipboard names the spans of files it is
    /// of, and the window's clipboard holds the document.
    fn audio_clipboard(
        &mut self,
        member: MemberId,
        mut document: String,
        ready: bool,
        ran: &mut Ran,
    ) -> Result<(), String> {
        if !ready {
            document = self.exchange_publish(member, &document, Default::default())?;
        }
        ran.effects
            .push(json!({"kind": "clipboard", "document": document}));
        Ok(())
    }
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
/// file is the shape it is asked as), as the editor reads a file it opens.
/// A file a part cannot read in place is decoded when the editor opens it.
fn shape(file: Wanted, caller: &mut dyn Caller) -> Result<Value, String> {
    match file {
        Wanted::Shape(shape) => Ok(json!({"frames": shape.frames, "channels": shape.channels,
            "rate": shape.rate, "path": null, "untitled": true,
            "codec": null, "sample_format": null})),
        Wanted::Path(path) => {
            let info = caller.header(&path)?;
            Ok(json!({
                "frames": info.frames, "channels": info.channels,
                "rate": info.rate, "path": path, "untitled": false,
                "codec": info.codec, "sample_format": info.sample_format,
            }))
        }
    }
}
