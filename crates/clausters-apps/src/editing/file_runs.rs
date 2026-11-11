//! **The file of a window whose document is one** ([`crate::file`]), carried
//! out by the editing context: a save prepared from one revision, stored, and
//! confirmed once the store lands; a change of the window's document read
//! whole first. One run for every such window, the score editor's and the
//! roll's alike, each reached through [`FileMember`].

use clausters_core::ids::IdSpaces;
use serde_json::json;

use super::roll_runs::{Roll, effect};
use super::{Editing, Member, MemberId, Raised, Ran};
use crate::application::operations::{Prepared, Target};
use crate::application::runtime::Runtime;
use crate::application::services::Application;
use crate::closing::{FileChange, Leaving};
use crate::file::{Command, Commit, FileWindow, Request, Saved};
use crate::turn::Converse;

/// **The runtimes of the windows whose document is a file**, one per kind of
/// window, and the stores they wait on, by run.
#[derive(Debug, Default)]
pub(super) struct FileRuntimes {
    #[cfg(feature = "notation")]
    pub(super) score: Runtime<Commit, crate::score::editor::Outcome>,
    pub(super) notes: Runtime<Commit, crate::notes::editor::Outcome>,
    pub(super) waits: std::collections::BTreeMap<u64, Prepared>,
}

impl FileRuntimes {
    /// Every operation over `server` forgotten, with the stores it waited on.
    pub(super) fn invalidate(
        &mut self,
        resources: &mut crate::application::resources::Resources,
        server: crate::application::resources::ServerInstance,
    ) {
        #[cfg(feature = "notation")]
        self.score.invalidate(resources, server);
        self.notes.invalidate(resources, server);
        self.waits
            .retain(|_, work| work.ticket.target.server != server);
    }
}

/// **A window whose document is a file, as the context reaches it**: its
/// runtime, and the window a member is.
pub(crate) trait FileMember:
    FileWindow
    + Application<Command = Request, Commit = Commit, Outcome = <Self as Converse>::Outcome>
    + Sized
{
    /// Its kind's runtime.
    fn runtime(runtimes: &mut FileRuntimes) -> &mut Runtime<Commit, <Self as Converse>::Outcome>;
    /// The window member `member` is, for the runtime's walk.
    fn of(member: &mut Member) -> &mut Self;
    /// The window member `member` is, if it is one of this kind.
    fn get(member: &Member) -> Option<&Self>;
}

#[cfg(feature = "notation")]
impl FileMember for crate::score::editor::ScoreEditor {
    fn runtime(runtimes: &mut FileRuntimes) -> &mut Runtime<Commit, <Self as Converse>::Outcome> {
        &mut runtimes.score
    }
    fn of(member: &mut Member) -> &mut Self {
        match member {
            Member::Score(editor) => editor.as_mut(),
            _ => unreachable!("the operation belongs to a score member"),
        }
    }
    fn get(member: &Member) -> Option<&Self> {
        match member {
            Member::Score(editor) => Some(editor.as_ref()),
            _ => None,
        }
    }
}

impl FileMember for crate::notes::editor::NotesEditor {
    fn runtime(runtimes: &mut FileRuntimes) -> &mut Runtime<Commit, <Self as Converse>::Outcome> {
        &mut runtimes.notes
    }
    fn of(member: &mut Member) -> &mut Self {
        match member {
            Member::Notes(editor) => editor.as_mut(),
            _ => unreachable!("the operation belongs to a notes member"),
        }
    }
    fn get(member: &Member) -> Option<&Self> {
        match member {
            Member::Notes(editor) => Some(editor.as_ref()),
            _ => None,
        }
    }
}

impl Editing {
    /// **The window's prepared revision stored**, at `path` or at the file it
    /// has, and confirmed once it is: only then is the file the window's and
    /// its changes not what a close asks about, and only then does `then`
    /// go. A write that fails changes nothing.
    pub(super) fn file_save<A: FileMember>(
        &mut self,
        member: MemberId,
        path: Option<String>,
        then: Option<Leaving>,
        ran: &mut Ran,
    ) -> Result<(), String> {
        use crate::application::runtime::Dispatch;
        let seat = self
            .seats
            .get(member as usize)
            .ok_or("the member is absent")?;
        let editor = A::get(&seat.member).ok_or("the member holds no file")?;
        if let Some(why) = editor.no_file() {
            return Err(why.into());
        }
        let path = path
            .or_else(|| editor.file_path())
            .ok_or("the document has no file yet: save it as")?;
        let (contents, revision) = editor.write_file()?;
        let request = Request {
            target: Target {
                member,
                object: editor.binding(),
                server: seat.server,
            },
            command: Command::Save(Box::new(Saved {
                contents,
                revision,
                path,
                then,
            })),
        };
        let runtime = A::runtime(&mut self.files);
        let ticket = runtime.prepare(editor, &mut self.services, request)?;
        let Dispatch::Response(protocol) = runtime.next(ticket) else {
            unreachable!("a save waits on its storage batch");
        };
        self.file_wait(protocol.batch.expect("a storage batch"), ran);
        Ok(())
    }

    /// The store `work` asks for, handed to the caller, its run waited on.
    fn file_wait(&mut self, work: Prepared, ran: &mut Ran) {
        // a new dispatch gets a new run id, so a duplicate landing consumes
        // nothing
        let run = self.services.resources.new_owner();
        ran.render = Some(crate::editing::Render {
            run,
            steps: work.steps.clone(),
        });
        self.files.waits.insert(run, work);
    }

    /// **A store landed**: the operation it was for settled, and what waited
    /// for it done over what it wrote.
    pub(super) fn file_landed<A: FileMember>(
        &mut self,
        member: MemberId,
        run: u64,
        result: Result<(), String>,
        ids: &mut IdSpaces,
    ) -> Ran {
        use crate::application::runtime::{Control, Reply};
        use crate::file::FileOutcome;
        let mut ran = Ran::default();
        if self
            .files
            .waits
            .get(&run)
            .is_none_or(|work| work.member != member)
        {
            return ran;
        }
        let Some(work) = self.files.waits.remove(&run) else {
            return ran;
        };
        let dispatch = A::runtime(&mut self.files).control(
            &mut self.services.resources,
            Control::Complete {
                ticket: work.ticket,
                batch: work.batch,
                result: match result {
                    Ok(()) => Reply::Success,
                    Err(reason) => Reply::Failed(reason),
                },
            },
            self.version,
        );
        let settled = super::settle_operation(
            A::runtime(&mut self.files),
            &mut self.services,
            &mut self.version,
            &mut self.seats,
            A::of,
            dispatch,
        );
        if let Some(work) = settled.protocol.batch {
            self.file_wait(work, &mut ran);
            return ran;
        }
        if let Some(error) = settled.protocol.error {
            ran.end = Some(Err(Raised {
                kind: "command".into(),
                message: error,
            }));
        }
        if let Some(reason) = settled.protocol.reason {
            ran.end = Some(Err(Raised {
                kind: "command".into(),
                message: reason.clone(),
            }));
            ran.effects.push(effect("say", json!({"reason": reason})));
        }
        let committed = settled.protocol.committed == Some(true);
        let saved = settled
            .outcome
            .and_then(|mut outcome| FileOutcome::take_saved(&mut outcome));
        if let Some(saved) = saved {
            ran.end = Some(Ok(json!({"destination": saved.path})));
            let moved = self
                .seats
                .get(member as usize)
                .and_then(|seat| A::get(&seat.member))
                .is_some_and(Converse::unsaved);
            match saved.then {
                // **what waited for the save is done only over what it
                // wrote**: an edit made while it was written is work the save
                // does not hold, and the window keeps it
                Some(_) if moved => ran.effects.push(effect(
                    "say",
                    json!({"reason": "the document changed while it was saved: it stays open"}),
                )),
                Some(Leaving::Window) => {
                    if let Some(mut roll) = self.rolls.remove(&member) {
                        let _ = roll.playback.close(ids);
                    }
                    ran.effects.push(self.roll_closing(member, "closeWindow"));
                    return ran;
                }
                Some(Leaving::File(change)) => {
                    if let Some(mut roll) = self.rolls.remove(&member).or_else(|| Roll::new(self)) {
                        if let Err(why) =
                            self.file_change::<A>(member, &mut roll, change, ids, &mut ran)
                        {
                            ran.effects.push(effect("say", json!({"reason": why})));
                        }
                        self.rolls.insert(member, roll);
                    }
                    return ran;
                }
                None => {}
            }
        }
        if committed {
            ran.effects.push(effect("resync", json!({})));
        }
        ran
    }

    /// **The window's document changes** (File > New, Open, Close file, or the
    /// door's verbs): what plays stopped and one transition through the
    /// runtime, which takes the old document's order with it. A file is read
    /// whole first, so one that does not read leaves everything as it was, and
    /// a file another window holds brings that window forward instead. Whoever
    /// opened the window is handed what it left, and the window is drawn again.
    pub(super) fn file_change<A: FileMember>(
        &mut self,
        member: MemberId,
        roll: &mut Roll,
        change: FileChange,
        ids: &mut IdSpaces,
        ran: &mut Ran,
    ) -> Result<(), String> {
        let here = self
            .seats
            .get(member as usize)
            .and_then(|seat| A::get(&seat.member))
            .ok_or("the member holds no file")?
            .window_id();
        if let FileChange::Open(path) = &change
            && let Some(window) = self
                .window_holding_file(path)
                .filter(|window| Some(*window) != here)
        {
            ran.effects.push(effect("front", json!({"window": window})));
            return Err(format!("{path} is open in another window"));
        }
        let seat = self
            .seats
            .get(member as usize)
            .ok_or("the member is absent")?;
        let server = seat.server;
        let editor = A::get(&seat.member).ok_or("the member holds no file")?;
        if let Some(why) = editor.no_file() {
            return Err(why.into());
        }
        let command = match &change {
            FileChange::New => Command::Replace {
                contents: editor.blank()?,
                path: None,
            },
            FileChange::Open(path) => {
                let bytes =
                    clausters_core::files::read(path).map_err(|e| format!("{path}: {e}"))?;
                Command::Replace {
                    contents: editor
                        .read_file(&bytes)
                        .map_err(|e| format!("{path}: {e}"))?,
                    path: Some(path.clone()),
                }
            }
            // none already: nothing to let go of
            FileChange::Close if !editor.has_document() => return Ok(()),
            FileChange::Close => Command::Unbind,
        };
        let request = Request {
            target: Target {
                member,
                object: editor.binding(),
                server,
            },
            command,
        };
        let ticket = A::runtime(&mut self.files).prepare(editor, &mut self.services, request)?;
        self.handler_verb(member, roll, "stop", &json!({"mark": 0.0}), ids, ran);
        let finished = A::runtime(&mut self.files)
            .operations
            .finish_ready(ticket)
            .expect("changing a window's document has no pending I/O");
        let editor = A::of(&mut self.seats[member as usize].member);
        let settled = A::runtime(&mut self.files).settle_turn(
            editor,
            &mut self.services,
            finished,
            &mut self.version,
        );
        if settled.protocol.committed != Some(true) {
            return Err("the document changed before confirmation".into());
        }
        if let Some(left) = settled
            .outcome
            .and_then(|mut outcome| crate::file::FileOutcome::take_left(&mut outcome))
        {
            ran.effects
                .push(effect("released", json!({"snapshot": left})));
        }
        self.version += 1;
        // the next document's order is its own
        self.select_member(member);
        self.roll_update(member, roll, ids);
        self.sound_rolls(ids);
        ran.effects.push(effect("publish", json!({})));
        ran.effects.push(effect("resync", json!({})));
        // a gesture made against the document it let go of is refused, and one
        // made after is not
        ran.effects.push(self.told_version());
        Ok(())
    }
}
