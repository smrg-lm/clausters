//! **What an application's member asks of its caller**, and what one of its
//! runs comes to: the door an application that holds its engine drives a
//! member through ([`Editing::start`], [`Editing::land`],
//! [`Editing::query`]), whatever the member is.
//!
//! A member's action runs in the context as far as it can; what only the
//! caller can do -- the disk, steps carried out on its turn, its threads --
//! it asks through [`Caller`], and a render, the one thing that takes as long
//! as a take is long, is handed back ([`Ran::render`]) for the caller to run
//! on a thread of its own and land. An audio member is driven by its
//! coordinator; a member with no actions of this kind says so.

use clausters_editing::apply::Step;
use serde_json::{Value, json};

use super::{Editing, Member, MemberId};
use clausters_core::ids::IdSpaces;

/// **What the caller does for a member's run**, at once: the I/O and the
/// threads are the caller's, never the application's.
pub trait Caller {
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

/// A sound file's header, as [`Caller::header`] reads it.
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

/// **What an action raised**: its kind (`value`, `command`, `runtime`), and
/// why.
#[derive(Clone, Debug, PartialEq)]
pub struct Raised {
    /// `value` (a request the editor cannot take), `command` (an operation
    /// refused) or `runtime` (a state the editor is not in any more).
    pub kind: String,
    /// Why.
    pub message: String,
}

/// **A render a run waits on**, handed to the caller to run off its turn:
/// the steps, and the run it resumes ([`Editing::land`]).
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
    /// **Starts `action` on member `member`** and carries it as far as it
    /// goes: to its end, or to a render it hands back. `lease` says the
    /// action holds the clipboard lease its event was prepared with; `ids`
    /// are the engine's, which a member's playback is allocated from.
    pub fn start(
        &mut self,
        member: MemberId,
        action: &Value,
        lease: bool,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
    ) -> Ran {
        match self.member(member) {
            Some(Member::Audio(_)) => self.audio_act(member, action, lease, caller, ids),
            _ => refused(),
        }
    }

    /// **A render landed**: the run it was for resumed with how it went,
    /// and carried on as far as it goes.
    pub fn land(
        &mut self,
        member: MemberId,
        run: u64,
        result: Result<(), String>,
        lease: bool,
        caller: &mut dyn Caller,
        ids: &mut IdSpaces,
    ) -> Ran {
        match self.member(member) {
            Some(Member::Audio(_)) => self.audio_landed(member, run, result, lease, caller, ids),
            _ => refused(),
        }
    }

    /// **What a script reads of member `member`**: for an audio member,
    /// `transport` (its transport, span, loop switch and state) or `output`
    /// (its meter's levels and the samples played as silence).
    pub fn query(&mut self, member: MemberId, query: &str) -> Value {
        match self.member(member) {
            Some(Member::Audio(_)) => self.audio_query(member, query),
            _ => json!({"error": "the member answers no queries"}),
        }
    }
}

/// What a member that runs no actions answers one.
fn refused() -> Ran {
    Ran {
        end: Some(Err(Raised {
            kind: "value".into(),
            message: "the member runs no actions".into(),
        })),
        ..Ran::default()
    }
}
