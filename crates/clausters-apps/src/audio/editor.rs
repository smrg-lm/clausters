//! **The audio editor's turns**: the window over a take made of parts, what one
//! message from the host does to the list, and the steps that make the server
//! draw and play what the list now is.
//!
//! The shape is the samples editor's -- a message read by the conversation, a
//! gesture read in its vocabulary, an acknowledgement, and an entry for the
//! history the caller keeps -- with one difference in what comes back: not a
//! write into the take, but the **steps** that make a new take where a turn
//! needs one and stitch the drawn join over the new list ([`Outcome::steps`]).
//! Each entry states the takes its two lists read, so the history holds them
//! for as long as it can walk to them.
//!
//! **How the window shows the take: views, and the layers of each.** Four
//! words, kept apart because each answers a different question:
//!
//! - A **view** is one picture of the take on a lane of its own: the
//!   waveform or the spectrogram. Views of one take share its time axis, its
//!   selection and its play cursor, never its vertical.
//! - A **visualization layer** is one measure drawn *inside* a view, over the
//!   view's one vertical: peak and RMS in the waveform, the preview of an
//!   unapplied process over them. They are the view's [`Stack`].
//! - An **edit layer** is what a hand edits, superposed on a view in the
//!   take's `pane`: the take itself, or the gain envelope over it. One is in
//!   hand at a time and it rules the vertical (`edit_layer`).
//! - The views are placed in one of two ways, never superposed: as
//!   **alternatives** in the one lane, one shown at a time
//!   ([`Views::Waveform`], [`Views::Spectrogram`]), or **stacked**, one lane
//!   under the other on the shared axis ([`Views::Stacked`]).
//!
//! **These are the defaults the menu offers, not limits of the mechanism.**
//! The menu superposes edit layers (the envelope over the take) and the
//! visualization layers of one quantity (RMS in its peaks), and places two
//! views measuring different quantities as alternatives or lanes. A script
//! still writes any stack through the `layers` verb -- a spectrogram with the
//! traces in boxes over it included -- so long as one quantity claims the
//! vertical.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_core::osc::{OscMessage, OscType};
use clausters_core::tempoclock::{samples_to_secs, secs_to_samples};
use clausters_document::clipboard::{Clipboard, Content, SegmentPart};
use clausters_document::parts::{self, PARTS};
use clausters_document::session::Part;
use clausters_document::view::NOT_AN_EDIT;
use clausters_document::view::layers::Stack;
use clausters_document::{Lifetime, Opaque, Range, SourceId, SourceRef};
use clausters_editing::apply::{Step, steps_json};
use clausters_editing::audio_playback::{Pass, space};
use clausters_editing::conversation::{self, Answer, Conversation, Correction, Message};
use clausters_editing::samples;
use clausters_editing::verbs::{self, ActionState, ActionTarget, PasteTarget};

use crate::application::resources::ServerInstance;
use crate::audio::dialogs;
use crate::audio::renders;
use crate::samples::{MEASURES, MeterAt, Window, props, window};
use crate::turn::{self, Converse, Event, Kind, Leg, Record, int, number, text};
use sources::{FileSource, Sources};

/// **Which views of the take the window shows, and how they are placed**
/// (the module's vocabulary): one of the two alternatives in the take's lane,
/// or both, stacked in two lanes on one axis. A view state, outside the
/// history.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Views {
    /// The waveform alone, in the take's lane.
    #[default]
    Waveform,
    /// The spectrogram alone, in the take's lane.
    Spectrogram,
    /// The waveform in the take's lane and the spectrogram in a lane under
    /// it, on the take's axis.
    Stacked,
}

impl Views {
    /// How many lanes the window holds: what a change between two views has
    /// to redefine the window for.
    fn lanes(self) -> usize {
        match self {
            Views::Stacked => 2,
            _ => 1,
        }
    }
}

/// The most values one write carries when the caller has not said.
pub const DEFAULT_CHUNK: usize = 8192;

/// **Why an edit is refused while its file plays**: a file is not edited
/// while it is heard; stopping it is what makes it editable again.
pub(crate) const PLAYING: &str = "the file is playing: stop it to edit";

/// **Whether `verb` changes what a file holds**: the edits, and the walks
/// through its history. A copy, a selection, a view and a save do not.
pub(crate) fn edits_contents(verb: &str) -> bool {
    matches!(
        verb,
        "undo"
            | "redo"
            | "cut"
            | "paste"
            | "mix"
            | "delete"
            | "draw"
            | "points"
            | "apply"
            | "apply_envelope"
            | "reverse"
    )
}

/// **What one turn came to.**
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// What kind of turn the message was.
    pub turn: Kind,
    /// A view change, also returned when no window is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layers: Option<Value>,
    /// Playback update for the original object, independent of current focus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playback_update: Option<Value>,
    /// This completion's effects were selected by the operation coordinator.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub prepared_complete: bool,
    /// New file binding published only after opening has completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<Value>,
    /// Whether confirmation focused another file.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub focus_changed: bool,
    /// Destination and shape of the successfully written snapshot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved: Option<Value>,
    /// Internal marker used when a draft decodes a window save.
    #[serde(skip)]
    pub(crate) save_requested: bool,
    /// What to send the host, `silent` for nothing.
    pub answer: Option<Answer>,
    /// The stamp a [`Kind::Step`] is answered with.
    pub seq: i64,
    /// Whether a [`Kind::Step`] walks forward.
    pub redo: bool,
    /// The entry to record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<Record>,
    /// Whether the take changed.
    pub changed: bool,
    /// The version after the turn.
    pub version: i64,
    /// **The steps to carry out**, in the JSON a runner walks: a new take made
    /// where the turn needed one, then the drawn join stitched over the list.
    /// The answer asks the window to read the join again, so it is sent once
    /// these have landed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Value>,
    /// Prepared I/O whose delta is still unpublished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<crate::application::operations::Prepared>,
    /// Typed steps retained for application preparation, without a JSON round trip.
    #[serde(skip)]
    pub(crate) planned_steps: Vec<Step>,
    /// Where the position cursor was placed, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locate: Option<f64>,
    /// The selection a sweep left, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<Value>,
    /// **What the space bar asks of the playback**: the caller stops it,
    /// going back to `back`, if it is rolling, and plays `start`..`pass`
    /// otherwise -- the playback is the caller's, and so is whether it rolls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play: Option<Play>,
    /// **What `L` asks of a pass in progress**: how it ends now, read off the
    /// view as a play's is -- the caller changes the pass of what is playing,
    /// and a stopped playback reads the switch on its next play.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pass: Option<Pass>,
    /// **What a selection redrawn asks of a loop in progress**: the span it
    /// repeats over now -- the caller hands it to the playback
    /// (`AudioEditorPlayback::follow`), which answers nothing unless this
    /// take is looping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow: Option<Follow>,
    /// **Where the position cursor now stands**, in frames of the take: the
    /// caller cues the playback there, so the play cursor goes with it while
    /// nothing plays.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cue: Option<u64>,
    /// **Whether to close the window** ([`crate::closing`]): a close asked of
    /// it, with nothing to lose or nothing to ask in.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub close: bool,
    /// **The window holds other widgets now**: the client draws it again
    /// (`window`) and publishes it, and the host keeps what is its own.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub restructure: bool,
    /// **A new file was asked for** in the window (`newFile`): `{channels,
    /// frames, rate}`, for the client to open -- a file is the client's to hold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_file: Option<Value>,
    /// **A file was asked to be opened** in the window (`openFile`):
    /// `{path}`, for the client to open as it opens a new one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_file: Option<Value>,
    /// The file binding the client focuses or closes, chosen by the app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_action: Option<Value>,
    /// **The files to look at on disk** (`checkDisk`), the window having come
    /// back: `{paths, seq}`, the open files read from the disk, for the
    /// client to `stat` and hand to the `on_disk` verb, which answers the
    /// window's `seq`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_disk: Option<Value>,
    /// **A copy made on the server**: the clipboard document naming the
    /// buffer it is in, for the client to put on the host's clipboard
    /// (`/gui_clipboard`). The buffer is the clipboard's from now on, and
    /// the one the previous copy left goes when this one lands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clipboard: Option<String>,
    /// The producing operation has published this document in its composition.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub clipboard_ready: bool,
    /// The buffer that copy is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clipboard_buffer: Option<i64>,
    /// A cut continuation, committed only after copy and publication succeed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clipboard_cut: Option<Value>,
}

turn::turned!(Outcome);

impl Converse for AudioEditor {
    type Outcome = Outcome;

    fn conversation(&mut self) -> &mut Conversation {
        &mut self.window_state.conversation
    }

    fn window_id(&self) -> Option<i32> {
        self.window_state.window
    }

    fn closed(&mut self) {
        self.window_state.window = None;
        self.window_state.active = false;
        self.window_state.binding_generation += 1;
        self.window_state.dialogs.closed();
    }

    fn owns(&self, widget: i64, _tag: &str) -> bool {
        self.window_state
            .widget
            .into_iter()
            .chain(self.window_state.files_widget)
            .chain(self.window_state.pane)
            .chain(self.window_state.envelope_widget)
            .chain(self.window_state.lane)
            .any(|w| i64::from(w) == widget)
            || self.dialog_name(widget).is_some()
    }

    fn resync(&mut self, widget: i64) -> Vec<Correction> {
        if self.window_state.envelope_widget.map(i64::from) == Some(widget) {
            self.envelope_corrections()
        } else if self.pictures().any(|w| i64::from(w) == widget) {
            self.redrawn(widget)
        } else {
            resync(widget)
        }
    }

    fn route(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if let Some(name) = self.dialog_name(widget) {
            return self.form_event(&name, tag, values, out);
        }
        if !self.open_files.open {
            return (Some(NO_FILE_OPEN.into()), Vec::new());
        }
        self.gesture(widget, tag, values, out)
    }

    fn window_verb(&mut self, message: &Message, args: &[Value], out: &mut Outcome) -> bool {
        // **A file chosen in the list is navigation, not an edit**: it names
        // no data a gesture was made against, so it is never stale: a hand
        // clicking down the list faster than the answers come back still
        // lands on the file it clicked last.
        if message.addr == "/gui_event"
            && self.window_state.files_widget.map(i64::from) == Some(message.widget)
        {
            out.turn = Kind::Route;
            if matches!(message.tag.as_str(), "select" | "activate")
                && let Some(file) = args
                    .get(4)
                    .and_then(Value::as_u64)
                    .and_then(|i| self.open_files.rows.get(i as usize))
            {
                out.file_action = Some(json!({"verb": "focus_file", "file": file["id"]}));
            }
            out.answer = Some(conversation::answer(
                message.seq,
                out.version,
                None,
                Vec::new(),
            ));
            return true;
        }
        if message.addr != "/gui_event" || !message.is_window {
            return false;
        }
        // **The window came back**: the files it reads are looked at on
        // disk, as a desktop program does. Nothing changes until the client
        // says what it found; the window put away asks for nothing.
        if message.tag == "activated" {
            out.turn = Kind::Route;
            let paths = if args.get(4).and_then(Value::as_i64) == Some(1) {
                self.disk_paths()
            } else {
                Vec::new()
            };
            if paths.is_empty() {
                out.answer = Some(conversation::answer(
                    message.seq,
                    out.version,
                    None,
                    Vec::new(),
                ));
            } else {
                // answered by `on_disk`, with what was found
                out.check_disk = Some(json!({"paths": paths, "seq": message.seq}));
            }
            return true;
        }
        // **With no file open** the window's own verbs are all it answers:
        // whatever would act on a file is refused, and says why.
        if !self.open_files.open && !WINDOW_LEVEL.contains(&message.tag.as_str()) {
            out.turn = Kind::Route;
            out.answer = Some(conversation::answer(
                message.seq,
                out.version,
                Some(NO_FILE_OPEN.into()),
                Vec::new(),
            ));
            return true;
        }
        if matches!(message.tag.as_str(), "toggle_files" | "close_file") {
            out.turn = Kind::Route;
            if message.tag == "toggle_files" {
                self.window_state.files_visible = !self.window_state.files_visible;
                out.restructure = true;
            } else {
                out.file_action = Some(json!({"verb": "close_file", "file": self.file.id.0}));
            }
            out.answer = Some(conversation::answer(
                message.seq,
                out.version,
                None,
                Vec::new(),
            ));
            return true;
        }
        // A channel's menu entry carries the channel after its verb.
        if let Some((verb, channel)) = message.tag.split_once(':')
            && matches!(verb, "fold_channel" | "solo_channel")
        {
            out.turn = Kind::Route;
            let channel = channel.parse::<u64>().ok().map(Value::from);
            let values: Vec<Value> = channel.into_iter().collect();
            let (reason, corrections) = self.gesture(message.widget, verb, &values, out);
            out.answer = Some(conversation::answer(
                message.seq,
                out.version,
                reason,
                corrections,
            ));
            return true;
        }
        if matches!(
            message.tag.as_str(),
            "view_wave"
                | "view_spectrogram"
                | "view_both"
                | "layer_take"
                | "show_envelope"
                | "apply_envelope"
                | "apply"
                | "reverse"
                | "show_all_channels"
        ) {
            out.turn = Kind::Route;
            let (reason, corrections) = self.gesture(message.widget, &message.tag, &[], out);
            out.answer = Some(conversation::answer(
                message.seq,
                out.version,
                reason,
                corrections,
            ));
            return true;
        }
        // **Ctrl+S over the window is a save**, the window's own verb: it names
        // no widget, changes nothing the history holds, and is answered with
        // the steps that write the file.
        if matches!(
            message.tag.as_str(),
            "save" | "save_as" | "new_file" | "open_file"
        ) {
            out.turn = Kind::Route;
            let (reason, corrections) = match message.tag.as_str() {
                "save" => self.save_from_window(out),
                "save_as" => self.open_form(dialogs::Form::SaveAs),
                "open_file" => self.open_form(dialogs::Form::Open),
                _ => self.open_form(dialogs::Form::New),
            };
            out.answer = Some(conversation::answer(
                message.seq,
                out.version,
                reason,
                corrections,
            ));
            return true;
        }
        // **The space bar over the window is a play**, the window's own verb
        // and the application's to read (the window says `plays`): what it
        // asks of the playback, with the loop switch the host sends beside it.
        if message.tag == "play" || message.tag == "loop" {
            out.turn = Kind::Route;
            let looping = args.get(4).is_some_and(|v| int(v) != 0);
            self.file.presentation.looping = looping;
            // The space bar plays or stops; `L` changes the pass in progress.
            if message.tag == "play" {
                out.play = Some(self.play(looping));
            } else {
                out.pass = Some(self.play(looping).pass);
            }
            out.answer = Some(conversation::answer(
                message.seq,
                out.version,
                None,
                Vec::new(),
            ));
            return true;
        }
        false
    }
}

/// **A press of the space bar over the take**, read off the view: where a
/// pass starts, how it ends, and where a stop goes back to -- all frames of
/// the take.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Play {
    /// The frame the pass starts on.
    pub start: u64,
    /// How it ends: a loop, or an end mark and its return.
    pub pass: Pass,
    /// Where a stop goes back to: the position cursor.
    pub back: u64,
}

/// **The span a loop in progress repeats over**, after a selection was
/// redrawn: frames of the take, half-open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Follow {
    /// The first frame of the span.
    pub from: u64,
    /// The first frame past it.
    pub to: u64,
    /// Whether the head goes to `from`: once, when the sweep is let go, and
    /// not while it is drawn -- a locate per move is a retrigger per move.
    pub place: bool,
}

/// **The verbs that need no file**: the window's own, which an editor with
/// no file open still answers -- adding a file, its keys, its file list and
/// closing it.
const WINDOW_LEVEL: &[&str] = &[
    "new_file",
    "open_file",
    "keys",
    "toggle_files",
    "close",
    "activated",
];

/// The JSON verbs an editor with no file open answers.
const EMPTY_VERBS: &[&str] = &[
    "sync",
    "window",
    "attach_view",
    "detach_view",
    "dialogs",
    "file",
    "files",
    "binding",
    "playback",
    "add_file",
    "open_file",
    "focus_file",
    "close_file",
    "show_files",
    "hide_files",
    "toggle_files",
    "playing",
    "props",
    "event",
];

/// Why a verb that acts on a file is refused when none is open.
const NO_FILE_OPEN: &str = "no file is open";

/// What an audio editor is put back to when a step it took part in is
/// refused elsewhere ([`AudioEditor::before_step`]).
pub(crate) struct Kept(parts::Take, u64);

/// **An audio editor**: the take it opened, the list of parts it is now, the
/// join that list is drawn from, and one view's end of the conversation.
#[derive(Clone, Debug)]
pub struct AudioEditor {
    file: AudioFileState,
    window_state: AudioWindowState,
    open_files: AudioFiles,
}

/// **What a file the editor makes is written from** by the open.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Written {
    /// A server buffer the editor was opened over, edited in the working
    /// folder and written back by a save.
    Buffer(i64),
    /// Nothing: a new file is silence until something is put in it.
    Silence,
}

/// An open object's identity, independent of its current server buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
struct FileId(i64);

#[derive(Clone, Debug)]
// Contents and sample-storage facts for one open object. No widget or
// conversation belongs here, so this state can occupy the active slot or the
// inactive collection without carrying a second window.
struct AudioFileState {
    id: FileId,
    presentation: AudioFileViewState,
    channels: u32,
    rate: f64,
    // The join the server plays, distinct from the object's identity.
    display: i64,
    target: Target,
    // The file the take was opened from, one of `sources`; none for a file
    // not written yet.
    opened: Option<SourceId>,
    // The files the list reads, and their rates as the list counts them.
    sources: Sources,
    rates: BTreeMap<SourceId, f64>,
    // What the open writes the file from, for a file the editor makes:
    // a server buffer it was opened over, or silence for a new file.
    written_from: Option<Written>,
    contents: parts::Take,
    dirty: bool,
    revision: u64,
    next_revision: u64,
    saved_revision: u64,
    written_format: Option<String>,
    codec: Option<String>,
    sample_format: Option<String>,
    name: Option<String>,
    // The file on disk the take came from, as it was when the editor last
    // read it: what a window coming back checks it against. No stamp for a
    // file the editor has just saved over, which the next look adopts; none
    // at all for a file the editor made.
    disk: Option<(String, Option<sources::DiskStamp>)>,
}

#[derive(Clone, Debug)]
// Presentation restored with a file, outside the content history. Host-local
// navigation is projected through the existing window and navigation groups.
struct AudioFileViewState {
    looping: bool,
    layers: Stack,
    views: Views,
    edit_layer: String,
    cursor: Option<u64>,
    selection: Option<(u64, u64)>,
    viewport: Option<(f64, f64)>,
    vertical: BTreeMap<String, (f64, f64)>,
    folded: BTreeSet<u32>,
}

#[derive(Clone, Debug)]
// One view's conversation and component identities survive every file switch.
struct AudioWindowState {
    active: bool,
    // Diagnostic binding identity; content-event validation uses Editing's
    // version and Conversation, not a second wire staleness mechanism.
    binding_generation: u64,
    meter_widget: Option<i32>,
    envelope_widget: Option<i32>,
    // The working folder the editor's renders and the peaks of what it
    // opened are written to; the client's, made when the editor opens.
    workspace: Option<String>,
    // The engine's rate, which the join it plays is stitched at.
    engine_rate: f64,
    // Whether the engine is the application's own, which reads each file's
    // list as it is and plays no join: nothing is stitched.
    held: bool,
    /// The endpoint incarnation the editor's buffers are on.
    server: crate::application::resources::ServerInstance,
    chunk: usize,
    lane: Option<i32>,
    title: String,
    size: (i64, i64),
    bare: bool,
    window: Option<i32>,
    widget: Option<i32>,
    pane: Option<i32>,
    meters: Option<(i32, usize)>,
    dialogs: dialogs::Dialogs,
    conversation: Conversation,
    files_widget: Option<i32>,
    files_visible: bool,
    playing: Option<i64>,
}

#[derive(Clone, Debug)]
// Ordered list projection and inactive file states. The editor's `file` is
// the active slot; these states never recursively hold another AudioEditor.
struct AudioFiles {
    rows: Vec<Value>,
    parked: BTreeMap<FileId, AudioFileState>,
    open: bool,
    next_id: i64,
}

/// A part reading frames `start..end` of `buffer`.
fn part(source: SourceId, start: u64, end: u64) -> Part {
    Part {
        source: SourceRef {
            source,
            lifetime: Lifetime::Temporary,
            generation: 0,
            range: Some(Range { start, end }),
        },
        fade_in: 0,
        fade_out: 0,
        fade_in_shape: Default::default(),
        fade_out_shape: Default::default(),
        channels: None,
    }
}

fn send(addr: &str, args: Vec<OscType>) -> Step {
    Step::Send(OscMessage {
        addr: addr.into(),
        args,
    })
}

/// What a paste or a mix carries in: the parts it puts in, and the steps
/// that make the file they read when the clipboard held samples.
type Carried = (Vec<Part>, Vec<Step>);

impl AudioEditor {
    /// **The window's actions as they read now**, by verb: what each
    /// addresses, whether it is available and why not, a switch's state and a
    /// choice's alternative in force. `undo`/`redo` and `pending` are the
    /// focused file's, which the editing context supplies.
    pub(crate) fn actions(
        &self,
        member: u32,
        server: ServerInstance,
        undo: bool,
        redo: bool,
        pending: bool,
        playing: bool,
    ) -> BTreeMap<&'static str, ActionState> {
        let view = &self.file.presentation;
        let views = match view.views {
            Views::Waveform => "view_wave",
            Views::Spectrogram => "view_spectrogram",
            Views::Stacked => "view_both",
        };
        let layer = if view.edit_layer.starts_with("points") {
            "show_envelope"
        } else {
            "layer_take"
        };
        let selected = view.selection.is_some_and(|(a, b)| b > a);
        let document = ActionTarget::Document {
            member,
            object: self.focused_file(),
        };
        // The window's verbs a take has no meaning for: a take has no grid,
        // and its spans are not boxes to split, join or quantize. With no
        // reading the host neither performs them nor lists their keys.
        const NOT_A_TAKE_S: &[&str] = &["split", "join", "quantize", "snap"];
        verbs::WINDOW
            .iter()
            .chain(verbs::AUDIO)
            .filter(|verb| !NOT_A_TAKE_S.contains(&verb.name))
            .map(|verb| {
                let name = verb.name;
                let window = WINDOW_LEVEL.contains(&name);
                let target = if window {
                    ActionTarget::Window
                } else {
                    document.clone()
                };
                let mut state = ActionState::new(verb, target);
                state.value = match verb.group {
                    Some("views") => Some(views.into()),
                    Some("layer") => Some(layer.into()),
                    _ => None,
                };
                state.checked = match name {
                    "loop" => Some(view.looping),
                    "toggle_files" => Some(self.window_state.files_visible),
                    _ => state.value.as_deref().map(|on| on == name),
                };
                if !window && !self.open_files.open {
                    state.refuse("no file is open");
                }
                match name {
                    "undo" if !undo => state.refuse("the file in focus has nothing to undo"),
                    "redo" if !redo => state.refuse("the file in focus has nothing to redo"),
                    "show_envelope" if self.window_state.envelope_widget.is_none() => {
                        state.refuse("this window has no envelope body");
                    }
                    _ => {}
                }
                // Work on the file in focus holds back what would edit, save or
                // close it; what only looks at it stays available.
                if matches!(
                    name,
                    "undo"
                        | "redo"
                        | "cut"
                        | "paste"
                        | "mix"
                        | "delete"
                        | "save"
                        | "save_as"
                        | "close_file"
                        | "apply"
                        | "apply_envelope"
                        | "reverse"
                ) {
                    state.pending = pending;
                    if pending {
                        state.refuse("the file has pending work");
                    }
                }
                // A file is not edited while it plays.
                if playing && edits_contents(name) {
                    state.refuse(PLAYING);
                }
                // Last: an empty selection is the one objection a host reads
                // for itself, so it must not hide another.
                if matches!(name, "cut" | "copy" | "delete" | "reverse") && !selected {
                    state.refuse_unselected();
                }
                if matches!(name, "paste" | "mix") {
                    state.paste = Some(PasteTarget::Samples {
                        channels: self.file.channels,
                        rate: self.file.rate,
                        connection: server.connection,
                        generation: server.generation,
                    });
                }
                (name, state)
            })
            .collect()
    }
}

impl AudioEditor {
    /// An editor over the file `opened` (none for a file not written yet),
    /// `channels` wide at `rate`, played through the join `display`,
    /// measured by `layers`, whose history is at `version`; or the reason
    /// the stack is refused.
    pub(crate) fn new(
        opened: Option<FileSource>,
        channels: u32,
        rate: f64,
        display: i64,
        layers: &[String],
        version: i64,
    ) -> Result<Self, String> {
        let mut editor = Self {
            file: AudioFileState {
                id: FileId(1),
                presentation: AudioFileViewState {
                    looping: false,
                    layers: Stack::checked(&json!(layers))?,
                    views: Views::Waveform,
                    edit_layer: "take".into(),
                    cursor: None,
                    selection: None,
                    viewport: None,
                    vertical: BTreeMap::new(),
                    folded: BTreeSet::new(),
                },
                channels: channels.max(1),
                rate: if rate > 0.0 { rate } else { 48_000.0 },
                display,
                target: Target::File(String::new()),
                opened: None,
                sources: Sources::new(),
                rates: BTreeMap::new(),
                written_from: None,
                contents: parts::Take::default(),
                dirty: false,
                revision: 1,
                next_revision: 1,
                saved_revision: 1,
                written_format: None,
                codec: None,
                sample_format: None,
                name: None,
                disk: None,
            },
            window_state: AudioWindowState {
                active: true,
                binding_generation: 0,
                meter_widget: None,
                envelope_widget: None,
                workspace: None,
                engine_rate: 0.0,
                held: false,
                server: Default::default(),
                chunk: DEFAULT_CHUNK,
                lane: None,
                title: "Audio".into(),
                size: (1000, 520),
                bare: false,
                window: None,
                widget: None,
                pane: None,
                meters: None,
                dialogs: dialogs::Dialogs::default(),
                conversation: Conversation::new(version),
                files_widget: None,
                files_visible: true,
                playing: None,
            },
            open_files: AudioFiles {
                rows: Vec::new(),
                parked: BTreeMap::new(),
                open: true,
                next_id: 2,
            },
        };
        if let Some(file) = opened {
            editor.file.target = Target::File(if file.render {
                String::new()
            } else {
                file.path.clone()
            });
            editor.reopen(file);
        }
        editor.ensure_envelope();
        Ok(editor)
    }

    /// Starts over on `file`: the list is the whole of it.
    fn reopen(&mut self, file: FileSource) {
        let id = self.add_source(file);
        let frames = self.file.sources[&id].frames;
        self.file.opened = Some(id);
        self.file.contents.parts = if frames > 0 {
            vec![part(id, 0, frames)]
        } else {
            Vec::new()
        };
    }

    /// **A file the list may read**, added to the table under its id: what
    /// a render the editor wrote, a file it opened or a file a paste names
    /// becomes. Answers the id.
    pub(crate) fn add_source(&mut self, file: FileSource) -> SourceId {
        let id = sources::id_of(&file.path);
        if file.rate != self.file.rate {
            self.file.rates.insert(id, file.rate);
        }
        self.file.sources.entry(id).or_insert(file);
        id
    }

    /// **The working folder**, or why an edit that writes a file cannot.
    fn workspace(&self) -> Result<&str, String> {
        self.window_state
            .workspace
            .as_deref()
            .ok_or_else(|| "the editor has no working folder to write a render into".into())
    }

    /// **A new file in the working folder**, for what this edit renders:
    /// named after the file, its next revision and `what`, so two edits never
    /// write one file.
    fn render_output(&self, what: &str) -> Result<renders::Output, String> {
        let path = format!(
            "{}/{}-{}-{what}.w64",
            self.workspace()?,
            self.file.id.0,
            self.file.next_revision + 1
        );
        Ok(renders::Output {
            peaks: sources::peaks_beside(&path),
            path,
            format: "float".into(),
        })
    }

    /// The list a render reads: the span `from..to` of the take, at its rate.
    fn render_list(&self, from: u64, to: u64) -> Result<renders::List, String> {
        let span = parts::span(&self.file.contents.parts, from, to, &self.rates_of())?;
        Ok(sources::list(
            &span,
            &self.file.sources,
            self.file.channels,
            self.file.rate,
        ))
    }

    /// **A render, as the source it makes**: the file `out` names, `frames`
    /// long at `rate`, written by this editor.
    fn rendered(&mut self, out: &renders::Output, frames: u64, rate: f64) -> SourceId {
        self.add_source(FileSource {
            path: out.path.clone(),
            peaks: out.peaks.clone(),
            frames,
            channels: self.file.channels,
            rate,
            render: true,
        })
    }

    /// The list the take is now.
    pub fn list(&self) -> &[Part] {
        &self.file.contents.parts
    }

    /// **The file each part of the list reads**, by the stem of its path:
    /// what a test reads of the list without knowing the ids.
    #[cfg(test)]
    pub(crate) fn part_files(&self) -> Vec<String> {
        self.file
            .contents
            .parts
            .iter()
            .filter_map(|part| self.file.sources.get(&part.source.source))
            .map(|source| {
                std::path::Path::new(&source.path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect()
    }

    /// **The paths of the open files read from the disk**, the focused one
    /// first: what a window coming back looks at.
    pub(crate) fn disk_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = Vec::new();
        if self.open_files.open {
            paths.extend(self.file.disk.as_ref().map(|(path, _)| path.clone()));
        }
        for state in self.open_files.parked.values() {
            if let Some((path, _)) = &state.disk
                && !paths.contains(path)
            {
                paths.push(path.clone());
            }
        }
        paths
    }

    /// **What the client found on disk** (`stamps`, a path to its stamp, or
    /// `null` for one that is gone): a file that changed since the editor
    /// read it is said on the window's status bar, once -- the stamp is
    /// taken as the file's from then on. The take reads what is there now:
    /// nothing was kept of what it read before.
    pub(crate) fn on_disk(&mut self, stamps: &Value) -> Option<String> {
        let mut changed = Vec::new();
        let mut gone = Vec::new();
        let mut look = |disk: &mut Option<(String, Option<sources::DiskStamp>)>| {
            let Some((path, held)) = disk.as_mut() else {
                return;
            };
            let Some(found) = stamps.get(path.as_str()) else {
                return;
            };
            // a file this editor saved over is taken as it is now
            let Some(stamp) = held.as_mut() else {
                *held = serde_json::from_value(found.clone()).ok();
                return;
            };
            let name = std::path::Path::new(path.as_str())
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            match serde_json::from_value::<sources::DiskStamp>(found.clone()) {
                Ok(now) if now != *stamp => {
                    *stamp = now;
                    changed.push(name);
                }
                Ok(_) => {}
                Err(_) if found.is_null() => {
                    gone.push(name);
                    *disk = None;
                }
                Err(_) => {}
            }
        };
        look(&mut self.file.disk);
        for state in self.open_files.parked.values_mut() {
            look(&mut state.disk);
        }
        let said = |names: &[String], what: &str| match names {
            [] => None,
            [one] => Some(format!("{one} {what}")),
            many => Some(format!("{} {what}", many.join(", "))),
        };
        let lines: Vec<String> = [
            said(&changed, "changed on disk"),
            said(&gone, "is no longer on disk"),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!lines.is_empty()).then(|| lines.join("; "))
    }

    /// **A request for a file to add, in this editor's folder**: a file
    /// added writes its renders where the editor's do, at the engine's rate
    /// it plays at, unless the request names its own.
    pub(crate) fn with_folder(&self, request: &Value) -> Value {
        let mut request = request.clone();
        if request.get("workspace").is_none_or(Value::is_null) {
            request["workspace"] = json!(self.window_state.workspace);
        }
        if request.get("engine_rate").is_none_or(Value::is_null) {
            request["engine_rate"] = json!(self.window_state.engine_rate);
        }
        request
    }

    /// How many frames the take is now.
    pub fn length(&self) -> u64 {
        parts::length(&self.file.contents.parts, &self.rates_of()).unwrap_or(0)
    }

    /// How the list counts its takes: at the editor's rate, and each take
    /// at another rate at its own.
    fn rates_of(&self) -> parts::Rates<'_> {
        parts::Rates {
            join: self.file.rate,
            of: &self.file.rates,
        }
    }

    /// The canonical array of layers, back to front.
    pub fn layers(&self) -> Value {
        self.file.presentation.layers.to_value()
    }

    /// Set a stack of names, through the shared validator.
    pub fn set_layers(&mut self, stack: &[String]) -> Result<(), String> {
        self.file.presentation.layers = Stack::checked(&json!(stack))?;
        Ok(())
    }

    fn edit_layer(&mut self, name: &str) -> Result<Vec<Correction>, String> {
        if !matches!(name, "take" | "placement")
            && !(matches!(name, "points" | "points:0")
                && self.window_state.envelope_widget.is_some())
        {
            return Err(format!("this pane has no '{name}' body"));
        }
        let relabel =
            self.file.presentation.edit_layer.starts_with("points") || name.starts_with("points");
        self.file.presentation.edit_layer = name.into();
        Ok(self
            .window_state
            .pane
            .into_iter()
            .map(|id| Correction {
                widget: i64::from(id),
                props: if relabel {
                    json!({"layer": name, "label": self.pane_label()})
                } else {
                    json!({"layer": name})
                },
            })
            .collect())
    }

    fn layer_corrections(&self) -> Vec<Correction> {
        self.window_state
            .widget
            .into_iter()
            .map(|widget| Correction {
                widget: i64::from(widget),
                props: json!({"layers": self.file.presentation.layers.to_wire()}),
            })
            .collect()
    }

    fn layer_verb(&mut self, verb: &str, request: &Value) -> Result<Outcome, String> {
        let next = match verb {
            "layers" => match request.get("stack") {
                Some(stack) => Stack::checked(stack)?,
                None => self.file.presentation.layers.clone(),
            },
            // The two alternatives in the take's lane, and both stacked: the
            // waveform in the take's lane over the spectrogram's own.
            "view_wave" | "view_both" => Stack::checked(&json!(["peak", "rms"]))?,
            "view_spectrogram" => Stack::checked(&json!(["spectrogram"]))?,
            _ => self.file.presentation.layers.edit(
                request
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                verb,
                request.get("alpha").and_then(Value::as_f64),
            )?,
        };
        self.file.presentation.layers = next;
        let before = self.file.presentation.views;
        self.file.presentation.views = match verb {
            "view_wave" => Views::Waveform,
            "view_spectrogram" => Views::Spectrogram,
            "view_both" => Views::Stacked,
            _ => self.file.presentation.views,
        };
        Ok(Outcome {
            turn: Kind::Route,
            restructure: before.lanes() != self.file.presentation.views.lanes(),
            layers: Some(self.layers()),
            answer: Some(conversation::answer(0, 0, None, self.layer_corrections())),
            ..Outcome::default()
        })
    }

    /// **The takes this editor cannot let go of**: the one it opened, which is
    /// the caller's, and every take the list reads now.
    pub fn rooted(&self) -> Vec<SourceId> {
        if !self.open_files.open {
            return Vec::new();
        }
        let mut out = self.file.rooted();
        out.extend(
            self.open_files
                .parked
                .values()
                .flat_map(|file| file.rooted()),
        );
        out
    }

    /// Scope remote references to this endpoint incarnation.
    pub(crate) fn set_server(&mut self, server: crate::application::resources::ServerInstance) {
        self.window_state.server = server;
    }

    /// **The files to delete now that nothing names `source`**: a render of
    /// this editor's that no open file's list reads any more (the history
    /// that let go of it said so), and its peaks. Nothing for a file the user
    /// owns, or one a list still reads.
    pub(crate) fn release(&mut self, source: SourceId) -> Vec<String> {
        if self.rooted().contains(&source) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for file in std::iter::once(&mut self.file).chain(self.open_files.parked.values_mut()) {
            if let Some(gone) = file.sources.remove(&source) {
                file.rates.remove(&source);
                if gone.render && !out.contains(&gone.path) {
                    out.push(gone.path);
                    out.push(gone.peaks);
                }
            }
        }
        out
    }

    /// **Saving: the take as the edits have left it, written where it belongs.**
    ///
    /// With no `to` it is written **over what the take was opened from** --
    /// the file, or the server buffer it was opened over. With a `to` it is a
    /// save-as, which a later save then writes over. The list is rendered
    /// into the file through its parts, so what lands is exactly what is
    /// heard (`renders::consolidate`).
    ///
    /// **A file the list reads is never written while it is read**: the
    /// take is rendered into the working folder, the file it replaces is
    /// moved there and the list goes on naming it where it now is, and the
    /// render is moved where the file was (two `Step::Move`s). So an undo
    /// past the save still has the frames it reads. **A buffer** is filled
    /// from the rendered file, at the take's length.
    ///
    /// These are a save operation's steps, built over its draft, which they
    /// leave naming the destination and any file moved.
    fn save_steps(&mut self, to: Option<Target>, format: &str) -> Result<Vec<Step>, String> {
        let to = self.save_target(to, format)?;
        let frames = self.length();
        let list = self.render_list(0, frames)?;
        let mut moves: Vec<Step> = Vec::new();
        let mut steps = match &to {
            Target::File(path) => {
                let read = self.file.sources.values().any(|s| s.path == *path);
                if read {
                    let held = format!(
                        "{}/{}-{}-replaced.w64",
                        self.workspace()?,
                        self.file.id.0,
                        self.file.next_revision + 1
                    );
                    let fresh = format!("{held}.saving");
                    let out = renders::Output {
                        path: fresh.clone(),
                        peaks: sources::peaks_beside(&fresh),
                        format: format.into(),
                    };
                    moves.push(Step::Move {
                        from: path.clone(),
                        to: held.clone(),
                    });
                    moves.push(Step::Move {
                        from: fresh,
                        to: path.clone(),
                    });
                    // The list reads the file it replaces where it now is: the
                    // source keeps its id, so every list the history holds
                    // reads the moved file too, and it is a render of this
                    // editor's from now on, which goes when nothing names it.
                    for source in self.file.sources.values_mut() {
                        if source.path == *path {
                            source.path = held.clone();
                            source.peaks = sources::peaks_beside(&held);
                            source.render = true;
                        }
                    }
                    let mut steps = vec![renders::consolidate(&list, &out)];
                    steps.append(&mut moves);
                    steps
                } else {
                    let out = renders::Output {
                        path: path.clone(),
                        peaks: String::new(),
                        format: format.into(),
                    };
                    vec![renders::consolidate(&list, &out)]
                }
            }
            Target::Buffer(buffer) => {
                let out = self.render_output("saved")?;
                vec![
                    renders::consolidate(&list, &out),
                    send(
                        "/buffer_allocRead",
                        vec![OscType::Int(*buffer as i32), OscType::String(out.path)],
                    ),
                    Step::AwaitDone {
                        command: "/buffer_allocRead".into(),
                        index: Some(*buffer as i32),
                    },
                ]
            }
        };
        self.file.target = to;
        // The list names the moved file now, so the join is stitched again.
        steps.extend(self.stitched());
        Ok(steps)
    }

    /// **Where a save writes**, `to` or what the take was opened from,
    /// checked: a known format, a file with a path, a buffer that is not the
    /// one the editor plays.
    pub(super) fn save_target(&self, to: Option<Target>, format: &str) -> Result<Target, String> {
        let to = to.unwrap_or_else(|| self.file.target.clone());
        if !matches!(format, "float" | "int24" | "int16") {
            return Err(format!(
                "'{format}' is not a sample format (float, int24, int16)"
            ));
        }
        match &to {
            Target::File(path) if path.is_empty() => Err("a file is saved under a path".into()),
            Target::Buffer(buffer) if *buffer == self.file.display => Err(format!(
                "buffer {buffer} is the one this editor plays: save into another"
            )),
            _ => Ok(to),
        }
    }

    /// What a save writes over.
    pub fn target(&self) -> &Target {
        &self.file.target
    }

    /// **Opening: the file read for its peaks, and the join the server plays
    /// stitched over it.**
    ///
    /// Nothing of the file is read but its header (the client's, before the
    /// editor exists) and, once, its samples into its peaks, which are kept in
    /// the working folder: a render of the file into a `PeaksOut`. A server
    /// buffer the editor was opened over is written into the working folder
    /// first and edited there; the buffer is written again by a save and by
    /// nothing else.
    pub fn open(&mut self) -> Result<Value, String> {
        self.open_steps().map(|steps| steps_json(&steps))
    }

    fn open_steps(&mut self) -> Result<Vec<Step>, String> {
        let mut steps = Vec::new();
        let opened = self
            .file
            .opened
            .and_then(|id| self.file.sources.get(&id).cloned());
        let written = self.file.written_from.take();
        if let (Some(Written::Silence), Some(file)) = (written, opened.as_ref()) {
            let out = renders::Output {
                path: file.path.clone(),
                peaks: file.peaks.clone(),
                format: "float".into(),
            };
            steps.push(renders::silence(
                file.frames,
                file.channels,
                file.rate,
                &out,
            ));
            steps.extend(self.stitched());
            return Ok(steps);
        }
        if let (Some(Written::Buffer(buffer)), Some(file)) = (written, opened.as_ref()) {
            steps.push(send(
                "/buffer_write",
                vec![
                    OscType::Int(buffer as i32),
                    OscType::String(file.path.clone()),
                    OscType::String("w64".into()),
                    OscType::String("float".into()),
                ],
            ));
            steps.push(Step::AwaitDone {
                command: "/buffer_write".into(),
                index: Some(buffer as i32),
            });
        }
        if let Some(file) = opened.filter(|f| f.frames > 0) {
            let whole = SegmentPart {
                path: file.path.clone(),
                start: 0,
                frames: file.frames,
                fade_in: 0,
                fade_out: 0,
                fade_in_shape: Default::default(),
                fade_out_shape: Default::default(),
                channels: None,
                source_channels: file.channels,
                source_frames: file.frames,
                source_rate: file.rate,
            };
            steps.push(renders::measure(&whole, &file.peaks));
        }
        steps.extend(self.stitched());
        Ok(steps)
    }

    /// **The join the server plays**, stitched over the list's files at the
    /// engine's rate -- so a file at another rate plays at its pitch -- and
    /// the `/done` awaited. Nothing for an empty list.
    fn stitched(&self) -> Vec<Step> {
        // an engine the application holds reads the list itself
        if self.window_state.held {
            return Vec::new();
        }
        let rate = if self.window_state.engine_rate > 0.0 {
            self.window_state.engine_rate
        } else {
            self.file.rate
        };
        let list = sources::list(
            &self.file.contents.parts,
            &self.file.sources,
            self.file.channels,
            rate,
        );
        if list.parts.is_empty() {
            return Vec::new();
        }
        let display = self.file.display as i32;
        vec![
            Step::Send(sources::stitch(display, &list)),
            Step::AwaitDone {
                command: "/buffer_stitch".into(),
                index: Some(display),
            },
        ]
    }

    /// **The list the window draws**: the parts by location, at the file's
    /// rate, each with its file's peaks.
    pub(crate) fn segments(&self) -> Value {
        sources::wire_list(
            &self.file.contents.parts,
            &self.file.sources,
            self.file.channels,
            self.file.rate,
        )
    }

    /// **The window**, numbered with the take widget's id, drawing the join.
    ///
    /// `meter` is the id the level meter is drawn under; the meter is there
    /// once a playback has said where it writes ([`Self::set_meters`]).
    pub fn window(&mut self, widget: i32, meter: Option<i32>) -> Value {
        self.window_state.widget = Some(widget);
        let meter = meter
            .zip(self.window_state.meters)
            .filter(|_| self.open_files.open)
            .map(|(id, (bus, channels))| MeterAt {
                widget: id,
                bus,
                channels,
            });
        // only a meter the window draws is corrected afterwards
        self.window_state.meter_widget = meter.map(|m| m.widget);
        let label = self.pane_label();
        let segments = self.segments();
        let mut tree = window(&Window {
            buffer: self.file.display,
            channels: self.file.channels,
            name: Some(&label),
            layers: &self.file.presentation.layers,
            rate: self.file.rate,
            widget,
            pane: self.window_state.pane,
            playhead_at: self.playhead_anchor(),
            layer: &self.file.presentation.edit_layer,
            title: &self.window_state.title,
            size: self.window_state.size,
            meter,
            bare: self.window_state.bare,
            dialogs: self.window_state.dialogs.stack(dialogs::forms()),
            folded: &self.folded(),
            lane: self
                .window_state
                .lane
                .filter(|_| self.file.presentation.views == Views::Stacked),
            empty: !self.open_files.open,
            segments: Some(&segments),
        });
        self.compose_envelope(&mut tree);
        self.refresh_files();
        self.compose_files(&mut tree);
        tree
    }

    fn playhead_anchor(&self) -> f64 {
        if self
            .window_state
            .playing
            .is_none_or(|file| file == self.file.id.0)
        {
            0.0
        } else {
            -1.0
        }
    }

    /// **Where the level is read from**: the first control bus the
    /// playback's meter writes and how many there are, or `None`.
    pub fn set_meters(&mut self, meters: Option<(i32, usize)>) {
        self.window_state.meters = meters;
    }

    /// **What the space bar plays**, with the loop switch `looping`: from the
    /// selection or the position cursor, to the end of the selection or the
    /// take, and back to the cursor -- or over and over.
    pub fn play(&self, looping: bool) -> Play {
        let (start, pass) = space(
            looping,
            self.file.presentation.selection,
            self.file.presentation.cursor,
            self.length(),
        );
        Play {
            start,
            pass,
            back: self.file.presentation.cursor.unwrap_or(start),
        }
    }

    /// The span a loop repeats over, as the view stands: the selection, or
    /// the take with none -- what [`space`] says a looping pass covers.
    fn follow(&self, place: bool) -> Option<Follow> {
        match space(
            true,
            self.file.presentation.selection,
            self.file.presentation.cursor,
            self.length(),
        )
        .1
        {
            Pass::Loop { from, to } => Some(Follow { from, to, place }),
            Pass::Until { .. } => None,
        }
    }

    /// **What a history step is put back to** when another structure of the
    /// same step refuses it: the list, and which takes are on disk.
    pub(crate) fn before_step(&self) -> Kept {
        Kept(self.file.contents.clone(), self.file.revision)
    }

    /// Puts back what [`before_step`](Self::before_step) kept.
    pub(crate) fn put_back(&mut self, kept: Kept) {
        self.file.contents = kept.0;
        self.file.revision = kept.1;
        self.file.dirty = self.file.revision != self.file.saved_revision;
    }

    fn keep_visibility(&self, next: &mut parts::Take) {
        // Visibility is view state: walking the edit order restores the curve,
        // not which body the user has chosen to look at since the edit.
        for curve in &mut next.automation {
            if let Some(held) = self
                .file
                .contents
                .automation
                .iter()
                .find(|a| a.id == curve.id)
            {
                curve.visible = held.visible;
            }
        }
    }

    /// **A step of the history, applied**: the list a payload states becomes
    /// the take, and the answer is the steps that stitch it. Nothing when the
    /// payload is not a list.
    pub fn apply(&mut self, payload: &Value) -> Option<Value> {
        let mut next = parts::Take::read(&Opaque(payload.clone()))?;
        self.keep_visibility(&mut next);
        self.file.contents = next;
        if let Some(revision) = payload["contentRevision"].as_u64() {
            self.file.revision = revision;
        } else {
            self.file.next_revision += 1;
            self.file.revision = self.file.next_revision;
        }
        self.file.dirty = self.file.revision != self.file.saved_revision;
        self.ensure_envelope();
        Some(steps_json(&self.stitched()))
    }

    /// **One message from the host**, read and answered ([`turn::turn`]).
    pub fn event(&mut self, event: &Event, version: i64) -> Outcome {
        let outcome = turn::turn(self, event, version);
        if outcome.changed {
            self.file.dirty = self.file.revision != self.file.saved_revision;
        }
        outcome
    }

    /// **The widgets that draw the take**: the view in its lane and, stacked,
    /// the spectrogram's lane under it.
    pub(super) fn pictures(&self) -> impl Iterator<Item = i32> + '_ {
        self.window_state.widget.into_iter().chain(
            self.window_state
                .lane
                .filter(|_| self.file.presentation.views == Views::Stacked),
        )
    }

    /// **Every picture of the take, read again** ([`resync`]).
    pub(super) fn reread(&self) -> Vec<Correction> {
        self.pictures()
            .flat_map(|w| self.redrawn(i64::from(w)))
            .collect()
    }

    /// **A picture of the take, told the list it is now**: the host draws
    /// the new list from its files, keeping what it drew until the new one
    /// has come.
    fn redrawn(&self, widget: i64) -> Vec<Correction> {
        vec![Correction {
            widget,
            props: json!({"segments": self.segments()}),
        }]
    }

    /// **Every widget of the window, corrected** -- what a history step leaves
    /// behind.
    pub fn resync_all(&self, version: i64) -> Answer {
        let mut corrections = self.reread();
        corrections.extend(self.envelope_corrections());
        conversation::answer(0, version, None, corrections)
    }

    /// Answers the stamp a [`Kind::Step`] carried, once the caller has walked.
    pub fn acknowledge(&self, seq: i64, version: i64, reason: Option<String>) -> Answer {
        conversation::answer(seq, version, reason, Vec::new())
    }

    fn secs_at(&self, units: f64) -> f64 {
        samples_to_secs(units.round() as i64, self.file.rate)
    }

    /// The frame of the take at `secs`, clamped to it.
    fn frame_at(&self, secs: f64) -> u64 {
        (secs_to_samples(secs, self.file.rate).max(0) as u64).min(self.length())
    }

    /// **The time range, set by the caller** -- `[start, end]` in seconds, or
    /// `None` -- as a sweep leaves it: what the space bar plays. Answers the
    /// band the window draws, `(sel_start, sel_len)` in frames.
    pub fn set_span(&mut self, span: Option<(f64, f64)>) -> (u64, u64) {
        self.file.presentation.selection = span
            .map(|(a, b)| (self.frame_at(a), self.frame_at(b)))
            .filter(|(a, b)| b > a);
        self.file
            .presentation
            .selection
            .map_or((0, 0), |(a, b)| (a, b - a))
    }

    /// **The position cursor, placed by the caller** at `secs`, as a click on
    /// the ruler places it. Answers its frame, where a stopped playback cues.
    pub fn set_cursor(&mut self, secs: f64) -> u64 {
        let frame = self.frame_at(secs);
        self.file.presentation.cursor = Some(frame);
        frame
    }

    /// One gesture onto the take. Answers the reason and the corrections the
    /// acknowledgement carries.
    fn gesture(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if tag == "show_envelope" {
            return self.show_envelope();
        }
        let channel = values.first().and_then(Value::as_u64);
        match tag {
            "unfold" => return self.unfold_channel(channel),
            "fold_channel" => return self.fold_channel(channel),
            "solo_channel" => return self.solo_channel(channel),
            "show_all_channels" => return self.show_all_channels(),
            _ => {}
        }
        if tag == "points" && self.window_state.envelope_widget.map(i64::from) == Some(widget) {
            return self.set_envelope_event(values, out);
        }
        if matches!(tag, "apply_envelope" | "apply") {
            return self.apply_envelope_event(out);
        }
        if tag == "layer" {
            return match values
                .first()
                .and_then(Value::as_str)
                .map(|name| self.edit_layer(name))
            {
                Some(Ok(corrections)) => (None, corrections),
                Some(Err(reason)) => (Some(reason), Vec::new()),
                None => (Some("layer needs a body name".into()), Vec::new()),
            };
        }
        if tag == "layer_take" {
            return match self.edit_layer("take") {
                Ok(corrections) => (None, corrections),
                Err(reason) => (Some(reason), Vec::new()),
            };
        }
        if matches!(tag, "view_wave" | "view_spectrogram" | "view_both") {
            return match self.layer_verb(tag, &Value::Null) {
                Ok(view) => {
                    out.layers = view.layers;
                    out.restructure = view.restructure;
                    (None, self.layer_corrections())
                }
                Err(reason) => (Some(reason), Vec::new()),
            };
        }
        if NOT_AN_EDIT.contains(&tag) {
            self.observe(widget, tag, values, out);
            return (None, Vec::new());
        }
        if matches!(tag, "copy" | "copy_cut") {
            if tag == "copy_cut" {
                match self.cut("cut", values) {
                    Ok(Some(_)) => {}
                    Ok(None) => return (None, Vec::new()),
                    Err(reason) => return (Some(reason), Vec::new()),
                }
            }
            return match self.copy_span(values, out) {
                Ok(()) => {
                    if tag == "copy_cut" && out.clipboard.is_some() {
                        out.clipboard_cut =
                            Some(json!({"verb":"finish_cut", "file":self.focused_file(),
                            "beforeRevision":self.file.revision, "values":values}));
                    }
                    (None, Vec::new())
                }
                Err(why) => (Some(why), Vec::new()),
            };
        }
        let edited = match tag {
            "draw" | "sample" => self.stroke(tag, values),
            "cut" | "delete" => self.cut(tag, values),
            "paste" => self.paste(values),
            "mix" => self.mix(values),
            "reverse" => self.reverse(values),
            _ => return (None, Vec::new()),
        };
        match edited {
            Ok(Some((label, list, mut steps))) => {
                let before = self.file.contents.clone();
                self.file.contents.parts = list;
                steps.extend(self.stitched());
                out.record = Some(self.record(&label, &before));
                out.planned_steps = steps.clone();
                out.steps = Some(steps_json(&steps));
                out.changed = true;
                out.version += 1;
                // **The picture is read again**, unlike a stroke over a buffer
                // the host holds: the join it draws is replaced whole by the
                // stitch, and only the answer can say so. A caller sends the
                // answer once the steps have landed.
                let mut corrections = self.reread();
                // **A cut or a paste leaves nothing selected**: the frames a
                // selection named have moved, so it would stand over other
                // samples. The position cursor stays where a cut landed, and
                // goes to the end of what a paste put in -- shown whole, the
                // view opened out to it where it ran past what is seen.
                if matches!(tag, "cut" | "delete" | "paste") {
                    let at = values.first().map_or(0.0, number).round().max(0.0) as u64;
                    if tag == "paste" {
                        let was = parts::length(&before.parts, &self.rates_of()).unwrap_or(0);
                        let pasted = self.length().saturating_sub(was);
                        corrections.extend(self.land_at(at + pasted, out));
                        corrections.extend(self.show_span(at, at + pasted, was));
                    } else {
                        corrections.extend(self.land_at(at, out));
                    }
                }
                (None, corrections)
            }
            Ok(None) => (None, Vec::new()),
            Err(why) => (Some(why), self.reread()),
        }
    }

    /// The entry a turn leaves: the list after, the list before, and the takes
    /// each reads.
    fn record(&mut self, label: &str, before: &parts::Take) -> Record {
        let previous = self.file.revision;
        self.file.next_revision += 1;
        self.file.revision = self.file.next_revision;
        self.file.dirty = self.file.revision != self.file.saved_revision;
        let mut forward = self.file.contents.payload().0;
        forward["contentRevision"] = json!(self.file.revision);
        let mut backward = before.payload().0;
        backward["contentRevision"] = json!(previous);
        let ids = |list: &[Part]| parts::sources(list).iter().map(|s| s.0).collect();
        Record {
            label: label.into(),
            legs: vec![Leg {
                forward: json!({ "edit": forward }),
                backward,
                key: String::new(),
                holds_forward: ids(&self.file.contents.parts),
                holds_backward: ids(&before.parts),
            }],
        }
    }

    /// **A pencil stroke is a new take the size of the stroke**: a render of
    /// the frames it was drawn over, the drawn channel written in and the
    /// others what the list holds there, spliced over those frames.
    fn stroke(&mut self, tag: &str, values: &[Value]) -> Result<Edited, String> {
        let taken = samples::intake(tag, values);
        let Some(payload) = taken.payloads.first() else {
            return match taken.refusal {
                Some(why) => Err(why),
                None => Ok(None),
            };
        };
        let start = number(payload.get("start").unwrap_or(&Value::Null)).max(0.0) as u64;
        let channel = number(payload.get("channel").unwrap_or(&Value::Null)).max(0.0) as u32;
        let mut drawn: Vec<f32> = payload
            .get("values")
            .and_then(Value::as_array)
            .map(|v| v.iter().map(|x| number(x) as f32).collect())
            .unwrap_or_default();
        let total = self.length();
        if start >= total || channel >= self.file.channels {
            return Err("the stroke is outside the take".into());
        }
        drawn.truncate((total - start) as usize);
        let frames = drawn.len() as u64;
        if frames == 0 {
            return Ok(None);
        }
        let over = self.render_list(start, start + frames)?;
        let out = self.render_output("stroke")?;
        let step = renders::stroke(&over, channel, &drawn, &out);
        let id = self.rendered(&out, frames, self.file.rate);
        let list = parts::replace(
            &self.file.contents.parts,
            start,
            start + frames,
            &[part(id, 0, frames)],
            &self.rates_of(),
        )?;
        Ok(Some((taken.label, list, vec![step])))
    }

    /// **A copy is the span's parts, by location**: a clipboard of segments,
    /// each naming its file, so a copy of an hour is a few parts and moves no
    /// samples, into this window or any other.
    fn copy_span(&mut self, values: &[Value], out: &mut Outcome) -> Result<(), String> {
        let at = |i: usize| values.get(i).map_or(0.0, number).round().max(0.0) as u64;
        let (start, len) = (at(0), at(1).min(self.length().saturating_sub(at(0))));
        if len == 0 {
            return Ok(());
        }
        let span = parts::span(
            &self.file.contents.parts,
            start,
            start + len,
            &self.rates_of(),
        )?;
        let clip = Clipboard::segments(
            self.file.channels,
            self.file.rate,
            sources::segment_parts(&span, &self.file.sources),
        );
        out.clipboard = Some(clip.to_json());
        Ok(())
    }

    /// **A cut takes the span out of the list**, and moves no samples. A
    /// delete is the same removal with nothing put on the clipboard: what
    /// reaches here as a cut was copied first (`copy_cut`), so the two differ
    /// only in the history's label (`tag`).
    fn cut(&mut self, tag: &str, values: &[Value]) -> Result<Edited, String> {
        let at = |i: usize| values.get(i).map_or(0.0, number).round().max(0.0) as u64;
        let (start, len) = (at(0), at(1));
        if len == 0 {
            return Ok(None);
        }
        let list = parts::remove(
            &self.file.contents.parts,
            start,
            start + len,
            &self.rates_of(),
        )?;
        if list.is_empty() {
            return Err("a take cannot be cut down to nothing".into());
        }
        Ok(Some((tag.into(), list, Vec::new())))
    }

    /// **What a paste or a mix carries**, as parts of a list this editor can
    /// read: a clipboard of segments by its files, which nothing has to read
    /// to put in; a block the host carried, rendered into a file of its own;
    /// a block a server holds, written into one. The steps make what has to
    /// be made, and the frames are the block's at the take's rate.
    fn carried(&mut self, values: &[Value]) -> Result<Option<Carried>, String> {
        let doc = values.get(2).map(text).unwrap_or_default();
        let Ok(clip) = serde_json::from_str::<Clipboard>(&doc) else {
            return Err("the clipboard did not travel with the paste".into());
        };
        let fits = |channels: u32, ours: u32| {
            if channels == ours {
                Ok(())
            } else {
                Err(format!(
                    "a block of {channels} channels does not paste into a take of {ours}"
                ))
            }
        };
        match clip.content {
            Content::Segments {
                channels, parts, ..
            } => {
                fits(channels, self.file.channels)?;
                let mut list = Vec::with_capacity(parts.len());
                for segment in parts {
                    let peaks = self
                        .file
                        .sources
                        .get(&sources::id_of(&segment.path))
                        .map(|s| s.peaks.clone())
                        .unwrap_or_else(|| sources::peaks_beside(&segment.path));
                    let id = self.add_source(FileSource {
                        path: segment.path.clone(),
                        peaks,
                        frames: segment.source_frames,
                        channels: segment.source_channels,
                        rate: segment.source_rate,
                        // a file another editor wrote is that editor's to delete
                        render: false,
                    });
                    let mut piece = part(id, segment.start, segment.start + segment.frames);
                    piece.fade_in = segment.fade_in;
                    piece.fade_out = segment.fade_out;
                    piece.fade_in_shape = segment.fade_in_shape;
                    piece.fade_out_shape = segment.fade_out_shape;
                    piece.channels = segment.channels.clone();
                    list.push(piece);
                }
                Ok((!list.is_empty()).then_some((list, Vec::new())))
            }
            Content::Buffer {
                channels,
                frames,
                sample_rate,
                buffer,
                location,
            } => {
                self.check_location(location)?;
                fits(channels, self.file.channels)?;
                if frames == 0 {
                    return Ok(None);
                }
                if !sample_rate.is_finite() || sample_rate <= 0.0 {
                    return Err("the clipboard sample rate must be finite and positive".into());
                }
                let out = self.render_output("pasted")?;
                let written = vec![
                    send(
                        "/buffer_write",
                        vec![
                            OscType::Int(buffer),
                            OscType::String(out.path.clone()),
                            OscType::String("w64".into()),
                            OscType::String("float".into()),
                        ],
                    ),
                    Step::AwaitDone {
                        command: "/buffer_write".into(),
                        index: Some(buffer),
                    },
                ];
                let id = self.rendered(&out, frames, sample_rate);
                let whole = SegmentPart {
                    path: out.path.clone(),
                    start: 0,
                    frames,
                    fade_in: 0,
                    fade_out: 0,
                    fade_in_shape: Default::default(),
                    fade_out_shape: Default::default(),
                    channels: None,
                    source_channels: channels,
                    source_frames: frames,
                    source_rate: sample_rate,
                };
                let mut steps = written;
                steps.push(renders::measure(&whole, &out.peaks));
                Ok(Some((vec![part(id, 0, frames)], steps)))
            }
            Content::Samples { .. } => {
                let (_, samples, rate) = self.block(values)?;
                let frames = samples.len() as u64 / u64::from(self.file.channels);
                if frames == 0 {
                    return Ok(None);
                }
                let out = self.render_output("pasted")?;
                let step = renders::block_file(&samples, self.file.channels, rate, &out);
                let id = self.rendered(&out, frames, rate);
                Ok(Some((vec![part(id, 0, frames)], vec![step])))
            }
            other => Err(format!(
                "a take takes samples, and the clipboard holds {}",
                other.kind()
            )),
        }
    }

    /// **A paste puts what the clipboard carries in at the position**: a
    /// copy of segments as its parts, and a block as a file of its own, at
    /// its own rate -- the list reads it at the ratio, so nothing is
    /// converted on the way in.
    fn paste(&mut self, values: &[Value]) -> Result<Edited, String> {
        let position = values.first().map_or(0.0, number).round().max(0.0) as u64;
        let Some((carried, steps)) = self.carried(values)? else {
            return Ok(None);
        };
        let list = parts::insert(
            &self.file.contents.parts,
            position,
            &carried,
            &self.rates_of(),
        )?;
        Ok(Some(("paste".into(), list, steps)))
    }

    /// **A mix adds what the clipboard carries onto the frames it lands
    /// on**: a render of those frames plus it, as long as the frames under
    /// it (a block that runs past the take is mixed as far as the take
    /// goes), spliced over them. A sum is taken sample for sample, so the
    /// block has to be at the take's rate.
    fn mix(&mut self, values: &[Value]) -> Result<Edited, String> {
        let position = values.first().map_or(0.0, number).round().max(0.0) as u64;
        let total = self.length();
        if position >= total {
            return Err("the mix lands past the end of the take".into());
        }
        let doc = values.get(2).map(text).unwrap_or_default();
        let clip = serde_json::from_str::<Clipboard>(&doc)
            .map_err(|_| "the clipboard did not travel with the mix".to_string())?;
        if let Some(rate) = clip.sample_rate()
            && rate != self.file.rate
        {
            return Err(format!(
                "a block at {rate} Hz does not mix into a take at {} Hz",
                self.file.rate
            ));
        }
        let out = self.render_output("mixed")?;
        let (frames, made) = if let Content::Samples { .. } = clip.content {
            let (_, mut block, _) = self.block(values)?;
            let width = u64::from(self.file.channels);
            let frames = (block.len() as u64 / width).min(total - position);
            block.truncate((frames * width) as usize);
            if frames == 0 {
                return Ok(None);
            }
            let onto = self.render_list(position, position + frames)?;
            (frames, vec![renders::mix_block(&onto, &block, &out)])
        } else {
            let Some((carried, mut made)) = self.carried(values)? else {
                return Ok(None);
            };
            let rates = self.rates_of();
            let frames = parts::length(&carried, &rates)?.min(total - position);
            if frames == 0 {
                return Ok(None);
            }
            let added = sources::list(
                &parts::span(&carried, 0, frames, &rates)?,
                &self.file.sources,
                self.file.channels,
                self.file.rate,
            );
            let onto = self.render_list(position, position + frames)?;
            // what the block needed made goes first
            made.push(renders::mix_lists(&onto, &added, &out));
            (frames, made)
        };
        let id = self.rendered(&out, frames, self.file.rate);
        let list = parts::replace(
            &self.file.contents.parts,
            position,
            position + frames,
            &[part(id, 0, frames)],
            &self.rates_of(),
        )?;
        Ok(Some(("mix".into(), list, made)))
    }

    /// **A span reversed**: a render of the frames played backwards, over
    /// them.
    fn reverse(&mut self, values: &[Value]) -> Result<Edited, String> {
        let at = |i: usize| values.get(i).map_or(0.0, number).round().max(0.0) as u64;
        // From the menu it names nothing, and is the selection's.
        let (start, len) = match (values.is_empty(), self.file.presentation.selection) {
            (true, Some((a, b))) => (a, b - a),
            (true, None) => return Err("select the span to reverse".into()),
            _ => (at(0), at(1)),
        };
        let len = len.min(self.length().saturating_sub(start));
        if len == 0 {
            return Ok(None);
        }
        let span = self.render_list(start, start + len)?;
        let out = self.render_output("reversed")?;
        let step = renders::reverse(&span, &out);
        let id = self.rendered(&out, len, self.file.rate);
        let list = parts::replace(
            &self.file.contents.parts,
            start,
            start + len,
            &[part(id, 0, len)],
            &self.rates_of(),
        )?;
        Ok(Some(("reverse".into(), list, vec![step])))
    }

    /// **The block a paste or a mix carries**: where it goes, its samples
    /// interleaved at the take's width and the rate they were taken at; or
    /// why it does not fit this take.
    fn block(&self, values: &[Value]) -> Result<(u64, Vec<f32>, f64), String> {
        let position = values.first().map_or(0.0, number).round().max(0.0) as u64;
        let doc = values.get(2).map(text).unwrap_or_default();
        let Ok(clip) = serde_json::from_str::<Clipboard>(&doc) else {
            return Err("the clipboard did not travel with the paste".into());
        };
        let Content::Samples {
            channels,
            frames,
            sample_rate,
            blob,
        } = clip.content
        else {
            return Err(format!(
                "a take takes samples, and the clipboard holds {}",
                clip.content.kind()
            ));
        };
        if channels != self.file.channels {
            return Err(format!(
                "a block of {channels} channels does not paste into a take of {}",
                self.file.channels
            ));
        }
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("the clipboard sample rate must be finite and positive".into());
        }
        let rate = sample_rate;
        let samples: Vec<f32> = match values.get(3 + blob) {
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| {
                    v.as_f64()
                        .filter(|v| v.is_finite() && (*v as f32).is_finite())
                        .map(|v| v as f32)
                })
                .collect::<Option<Vec<_>>>()
                .ok_or("the clipboard contains invalid samples")?,
            _ => return Err("the clipboard's samples did not travel with it".into()),
        };
        let width = u64::from(self.file.channels);
        if frames.checked_mul(width) != Some(samples.len() as u64) {
            return Err("the clipboard sample payload does not match its shape".into());
        }
        Ok((position, samples, rate))
    }

    fn check_location(
        &self,
        location: Option<clausters_document::clipboard::BufferLocation>,
    ) -> Result<(), String> {
        let server = self.window_state.server;
        if let Some(location) = location {
            if location.connection != server.connection || location.generation != server.generation
            {
                return Err("the clipboard buffer belongs to another server instance".into());
            }
        } else if server != Default::default() {
            return Err("the clipboard buffer has no owning exchange".into());
        }
        Ok(())
    }

    /// **Frames `from..to` shown whole**: where they run past what the view
    /// shows (the take's `was` frames when it has said nothing of its view),
    /// it opens out until they are inside; a span already seen moves nothing.
    fn show_span(&mut self, from: u64, to: u64, was: u64) -> Vec<Correction> {
        let Some(widget) = self.window_state.widget else {
            return Vec::new();
        };
        let (start, len) = self
            .file
            .presentation
            .viewport
            .unwrap_or((0.0, was.max(1) as f64));
        let (from, to) = (from as f64, to as f64);
        if from >= start && to <= start + len {
            return Vec::new();
        }
        let begin = start.min(from);
        let len = (start + len).max(to) - begin;
        self.file.presentation.viewport = Some((begin, len));
        vec![Correction {
            widget: i64::from(widget),
            props: json!({"view_start": begin, "view_len": len}),
        }]
    }

    /// A tag that says what the view is looking at rather than what changed.
    fn observe(&mut self, widget: i64, tag: &str, values: &[Value], out: &mut Outcome) {
        match tag {
            "view" if values.len() >= 2 => {
                let start = number(&values[0]);
                let len = number(&values[1]);
                if start.is_finite() && len.is_finite() && len > 0.0 {
                    self.file.presentation.viewport = Some((start.max(0.0), len));
                }
            }
            "view_y" if values.len() >= 2 => {
                let start = number(&values[0]);
                let len = number(&values[1]);
                if start.is_finite() && len.is_finite() && len > 0.0 {
                    let measure = self.vertical_measure(widget).to_owned();
                    self.file
                        .presentation
                        .vertical
                        .insert(measure, (start, len));
                }
            }
            "locate" if !values.is_empty() => {
                let frame = number(&values[0]);
                let cursor = frame.max(0.0).round() as u64;
                self.file.presentation.cursor = Some(cursor);
                out.cue = self.file.presentation.cursor;
                out.locate = Some(self.secs_at(frame));
            }
            "selection" => {
                let frame = |i: usize| values.get(i).map_or(0.0, number).max(0.0);
                let (from, len) = (frame(0), frame(1));
                self.file.presentation.selection =
                    (len > 0.0).then(|| (from.round() as u64, (from + len).round() as u64));
                let at = |i: usize| values.get(i).map_or(0.0, |v| self.secs_at(number(v)));
                out.selection = Some(json!({ "start": at(0), "len": at(1) }));
                // The release confirms the loop span without moving the
                // independent position cursor used by paste and stop.
                out.follow = self.follow(values.last().and_then(Value::as_bool).unwrap_or(false));
            }
            _ => {}
        }
    }
}

/// **Where a save writes**: a file, or a server buffer.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A file on the server's filesystem -- the page's own storage, in a tab.
    File(String),
    /// A server buffer, rewritten whole at the take's length.
    Buffer(i64),
}

/// What a gesture made: its label, the list after it and the steps that make
/// its new takes -- or nothing at all.
type Edited = Option<(String, Vec<Part>, Vec<Step>)>;

/// What `widget` should be drawing: read the take again.
fn resync(widget: i64) -> Vec<Correction> {
    let Ok(id) = i32::try_from(widget) else {
        return Vec::new();
    };
    vec![Correction {
        widget,
        props: Value::Object(props(id)),
    }]
}

/// The facts a caller hands over, in [`new_json`] and in `sync`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Facts {
    codec: Option<String>,
    sample_format: Option<String>,
    /// A server buffer the editor is opened over, which it edits as a file
    /// of its working folder.
    take: Option<i64>,
    /// The opened file's shape, read from its header by the client.
    frames: Option<u64>,
    channels: Option<u32>,
    rate: Option<f64>,
    display: Option<i64>,
    chunk: Option<usize>,
    /// The file the take was opened from.
    path: Option<String>,
    /// **A file not written yet**: its first save asks for a path.
    untitled: Option<bool>,
    /// The working folder the editor writes its renders into.
    workspace: Option<String>,
    /// The opened file's stamp on disk, when the caller has it; otherwise
    /// the window's first look takes the file as it is.
    stamp: Option<sources::DiskStamp>,
    /// The engine's rate, which the join it plays is stitched at.
    engine_rate: Option<f64>,
    /// Whether the engine is the application's own, which reads the lists
    /// itself: nothing is stitched from then on.
    held: Option<bool>,
    #[serde(deserialize_with = "present")]
    name: Option<Option<String>>,
    layers: Option<Value>,
    title: Option<String>,
    w: Option<i64>,
    h: Option<i64>,
    /// `false` for a window with no menu bar and no tools.
    chrome: Option<bool>,
    #[serde(deserialize_with = "present")]
    window: Option<Option<i32>>,
    /// Where the level meter is read from: `{"bus", "channels"}`, or `null`.
    #[serde(deserialize_with = "present")]
    meters: Option<Option<MeterFact>>,
    version: Option<i64>,
    /// **An editor with no file open**: the application on its own, whose
    /// File menu adds the first file.
    empty: Option<bool>,
}

/// A playback's meter, as a caller hands it over.
#[derive(Deserialize, Clone, Copy)]
struct MeterFact {
    bus: i32,
    channels: usize,
}

/// A key that is present, whatever its value -- `null` included.
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

impl AudioEditor {
    fn adopt(&mut self, facts: Facts) {
        if let Some(channels) = facts.channels {
            self.file.channels = channels.max(1);
        }
        if let Some(rate) = facts.rate.filter(|r| *r > 0.0) {
            self.file.rate = rate;
        }
        if let Some(display) = facts.display {
            self.file.display = display;
        }
        if let Some(workspace) = facts.workspace.filter(|w| !w.is_empty()) {
            self.window_state.workspace = Some(workspace);
        }
        // where a save writes from now on, as a save-as left it
        if let Some(path) = facts.path.filter(|p| !p.is_empty()) {
            self.file.target = Target::File(path);
        } else if facts.untitled == Some(true) {
            self.file.target = Target::File(String::new());
        }
        if let Some(rate) = facts.engine_rate.filter(|r| *r > 0.0) {
            self.window_state.engine_rate = rate;
        }
        if let Some(held) = facts.held {
            self.window_state.held = held;
        }
        if let Some(chunk) = facts.chunk {
            self.window_state.chunk = chunk.max(1);
        }
        if let Some(name) = facts.name {
            self.file.name = name;
        }
        if let Some(title) = facts.title {
            self.window_state.title = title;
        }
        if let Some(chrome) = facts.chrome {
            self.window_state.bare = !chrome;
        }
        if let Some(w) = facts.w {
            self.window_state.size.0 = w;
        }
        if let Some(h) = facts.h {
            self.window_state.size.1 = h;
        }
        if let Some(window) = facts.window {
            self.window_state.window = window;
        }
        if let Some(meters) = facts.meters {
            self.window_state.meters = meters.map(|m| (m.bus, m.channels));
        }
    }
}

/// **An editor built from a JSON request** -- the file it opens (`path`,
/// and `frames`, `channels` and `rate` from its header), or a server buffer
/// (`take`) it edits as a file of its own, or `untitled` for a new file of
/// that shape; `workspace`, the working folder its renders are written into
/// and the peaks of what it opened are kept in; `display`, the buffer of the
/// join it plays; `engine_rate`, `chunk`, `name`, `layers`, `title`, `w`,
/// `h`, `chrome` (`false` for a window with no menu bar and no tools) and
/// `version` -- or the reason it cannot be. `"empty": true` opens the editor
/// with no file, whose window offers adding one.
pub fn new_json(request: &str) -> Result<AudioEditor, String> {
    let facts: Facts =
        serde_json::from_str(request).map_err(|e| format!("not an audio editor request: {e}"))?;
    let layers = facts.layers.clone().unwrap_or_else(|| json!(MEASURES));
    let stack = Stack::checked(&layers)?;
    let names: Vec<String> = MEASURES.iter().map(|name| name.to_string()).collect();
    let empty = facts.empty == Some(true);
    let channels = facts.channels.unwrap_or(1).max(1);
    let rate = facts.rate.filter(|r| *r > 0.0).unwrap_or(48_000.0);
    let frames = facts.frames.unwrap_or(0);
    let workspace = facts.workspace.clone().filter(|w| !w.is_empty());
    let path = facts.path.clone().filter(|p| !p.is_empty());
    let display = facts.display.unwrap_or(0);
    // What the take is a file of: the user's, or one this editor makes in
    // its working folder -- a buffer's, or silence.
    let made = |what: &str| -> Result<String, String> {
        let folder = workspace
            .as_deref()
            .ok_or("an editor that makes its file needs a working folder")?;
        Ok(format!("{folder}/{display}-{what}.w64"))
    };
    let (opened, written) = if empty {
        (None, None)
    } else if let (Some(_), Some(buffer)) = (path.as_ref(), facts.take) {
        // **A file a part cannot read** (a compressed one): the client
        // decoded it into a buffer, which the editor writes to its folder,
        // and a save still goes to the file.
        let copy = made("opened")?;
        let file = FileSource {
            peaks: sources::peaks_beside(&copy),
            path: copy,
            frames,
            channels,
            rate,
            render: true,
        };
        (Some(file), Some(Written::Buffer(buffer)))
    } else if let Some(path) = path.as_ref() {
        let peaks = match workspace.as_deref() {
            Some(folder) => sources::peaks_for_opened(folder, path),
            None => sources::peaks_beside(path),
        };
        let file = FileSource {
            path: path.clone(),
            peaks,
            frames,
            channels,
            rate,
            render: false,
        };
        (Some(file), None)
    } else if let Some(buffer) = facts.take {
        let path = made("opened")?;
        let file = FileSource {
            peaks: sources::peaks_beside(&path),
            path,
            frames,
            channels,
            rate,
            render: true,
        };
        (Some(file), Some(Written::Buffer(buffer)))
    } else if facts.untitled == Some(true) {
        let path = made("new")?;
        let file = FileSource {
            peaks: sources::peaks_beside(&path),
            path,
            frames: frames.max(1),
            channels,
            rate,
            render: true,
        };
        (Some(file), Some(Written::Silence))
    } else {
        // nothing named: an open editor over an empty take
        (None, None)
    };
    let mut editor = AudioEditor::new(
        opened,
        channels,
        rate,
        display,
        &names,
        facts.version.unwrap_or(0),
    )?;
    editor.file.written_from = written;
    if let Some(Written::Buffer(buffer)) = written {
        // a decoded file saves to its path, a buffer of a file not written
        // yet asks for one, and a buffer is saved over
        editor.file.target = match path.as_ref() {
            Some(path) => Target::File(path.clone()),
            None if facts.untitled == Some(true) => Target::File(String::new()),
            None => Target::Buffer(buffer),
        };
    }
    editor.file.presentation.layers = stack;
    editor.file.codec = facts.codec.clone();
    // without a stamp, the first look takes the file as it is
    editor.file.disk = path.clone().map(|path| (path, facts.stamp));
    editor.file.sample_format = facts.sample_format.clone();
    editor.adopt(Facts {
        layers: None,
        channels: None,
        rate: None,
        path: None,
        untitled: None,
        ..facts
    });
    if empty {
        editor.open_files.open = false;
    }
    Ok(editor)
}

/// **One verb of an editor, over JSON** -- the door both clients bind.
///
/// - `sync` -- any of the facts [`new_json`] reads: handed over before the
///   verbs that read them; `meters` (`{"bus", "channels"}` or `null`) says
///   where the playback's meter writes. Answers `{}`.
/// - `layers` -- `stack`, optional: a layer array or word list. Answers an
///   outcome with the canonical `layers` and a correction, or `{"error"}`.
/// - `show_layer`, `hide_layer`, `solo_layer` -- `name`: change a layer;
///   solo toggles without changing visibility. `layer_alpha` also takes `alpha`.
/// - `view_wave`, `view_spectrogram`: the view in the take's lane, one or the
///   other; `view_both`: both, stacked in two lanes ([`Views`]). An outcome
///   with `restructure` asks the client to draw the window again.
/// - `layer` -- optional `name`: the active edit body and its correction.
/// - `envelope` -- seconds-based gain quads. `show_envelope` shows the body.
/// - `setautomation` -- `points`, gain quads; `apply_envelope` renders them.
///   Scripted edits go through `Editing::act` so their history is recorded.
/// - `window` -- `widget`, optional `pane`, `envelope`, `lane` and `meter`, the
///   ids of the axis container, the gain curve, the spectrogram's lane and
///   the level meter: the window, as a GuiDef.
/// - `props` -- `widget`: the correction.
/// - `event` -- `addr`, `args`, `version`: an [`Outcome`].
/// - `parts` -- `{"parts", "frames"}`: the list, and how long it is.
/// - `files` -- the ordered open files and list visibility; `binding` -- the
///   focused file's buffer facts, cursor, selection, layers and loop switch.
///   With no file open, `binding` and `file` answer `{"empty": true}`, and
///   every verb that acts on a file answers `{"error": "no file is open"}`.
/// - `open_file` -- `paths`, a path or array: an outcome asking the client to
///   read the files' headers. `add_file` -- a file's shape, its path and its
///   display buffer (a `take` for a file the client decoded): add it, in this
///   editor's folder, and return the server steps and binding.
/// - `focus_file`, `close_file` -- `index` (zero-based) or `file` (open-object id):
///   restore its state or free it. Closing without either names the focused file.
///   Closing the last one leaves the editor open with none (`"empty": true`,
///   `binding` null); the window is drawn again and offers adding a file.
/// - `show_files`, `hide_files`, `toggle_files` -- the independent left list.
/// - `span` -- `span`: `[start, end]` in seconds, or `null`: the time range
///   the space bar plays, as a sweep leaves it. Answers `{"sel_start",
///   "sel_len"}`, the band the window draws, in frames.
/// - `locate` -- `at`, in seconds: the position cursor. Answers `{"cue"}`,
///   its frame.
/// - `play` -- the [`Play`] the space bar would ask for, under the file's
///   loop switch. `looping` -- `on`: that switch, as a script sets it.
///
/// Opening, saving and a history step are not verbs here: each is
/// an operation of the context (`operation`), whose steps commit only once
/// the server has carried them out.
///
/// An unknown verb answers `{}`.
pub fn call_json(editor: &mut AudioEditor, request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return "{}".into();
    };
    let get = |key: &str| request.get(key).cloned().unwrap_or(Value::Null);
    let widget = int(&get("widget")) as i32;
    let version = int(&get("version"));
    let verb = request
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // **A verb that acts on a file needs one open**; the editor's own -- its
    // window, its file list, adding a file, the conversation -- do not.
    if !editor.open_files.open && !EMPTY_VERBS.contains(&verb) {
        return json!({ "error": NO_FILE_OPEN }).to_string();
    }
    match verb {
        "sync" => {
            if let Some(visible) = request.get("filesVisible").and_then(Value::as_bool) {
                editor.window_state.files_visible = visible;
            }
            if let Ok(facts) = serde_json::from_value::<Facts>(request.clone()) {
                editor.adopt(Facts {
                    layers: None,
                    ..facts
                });
            }
            "{}".into()
        }
        verb @ ("layers" | "show_layer" | "hide_layer" | "solo_layer" | "layer_alpha"
        | "view_wave" | "view_spectrogram" | "view_both") => {
            match editor.layer_verb(verb, &request) {
                Ok(mut outcome) => {
                    outcome.version = version;
                    outcome.answer = Some(conversation::answer(
                        0,
                        version,
                        None,
                        editor.layer_corrections(),
                    ));
                    let mut value = serde_json::to_value(outcome).unwrap();
                    value["measure"] = json!(editor.file.presentation.layers.to_wire());
                    value.to_string()
                }
                Err(error) => json!({"error": error}).to_string(),
            }
        }
        verb @ ("layer" | "layer_take") => {
            let name = if verb == "layer_take" {
                Some("take")
            } else {
                request.get("name").and_then(Value::as_str)
            };
            match name.map(|name| editor.edit_layer(name)).transpose() {
                Ok(corrections) => json!({"turn": "route", "layer": editor.file.presentation.edit_layer,
                    "answer": conversation::answer(0, version, None, corrections.unwrap_or_default())}).to_string(),
                Err(error) => json!({"error": error}).to_string(),
            }
        }
        "window" => {
            editor.window_state.files_widget = request.get("filesWidget").and_then(Value::as_i64).map(|n| n as i32);
            if let Some(ids) = request.get("dialogs").and_then(Value::as_object) {
                editor.window_state.dialogs.ids = ids
                    .iter()
                    .filter_map(|(name, id)| Some((name.clone(), id.as_i64()? as i32)))
                    .collect();
            }
            editor.window_state.envelope_widget = request
                .get("envelope")
                .and_then(Value::as_i64)
                .map(|n| n as i32);
            editor.ensure_envelope();
            editor.window_state.pane = request
                .get("pane")
                .and_then(Value::as_i64)
                .map(|n| n as i32);
            editor.window_state.lane = request
                .get("lane")
                .and_then(Value::as_i64)
                .map(|n| n as i32);
            editor
                .window(
                    widget,
                    request
                        .get("meter")
                        .and_then(Value::as_i64)
                        .map(|m| m as i32),
                )
                .to_string()
        }
        "attach_view"=>{editor.window_state.active=true;editor.window_state.binding_generation+=1;"{}".into()},
        "detach_view"=>{editor.window_state.active=false;editor.window_state.window=None;editor.window_state.binding_generation+=1;"{}".into()},
        "dialogs" => json!({"dialogs": dialogs::names()}).to_string(),
        "folded" => json!({"folded": editor.folded()}).to_string(),
        "file" if !editor.open_files.open => json!({"empty": true}).to_string(),
        "file" => editor.file().to_string(),
        "files" => { editor.refresh_files(); json!({"files": editor.open_files.rows, "visible": editor.window_state.files_visible}).to_string() },
        // with no file open, a binding says so rather than naming one
        "binding" if !editor.open_files.open => json!({"empty": true}).to_string(),
        "binding" => editor.binding().to_string(),
        "add_file" => match new_json(&editor.with_folder(&request).to_string())
        .and_then(|mut file| {
            let steps = file.open()?;
            editor.add_file(file);
            Ok(steps)
        }) {
            Ok(steps) => json!({"turn": "route", "steps": steps, "binding": editor.binding(), "focusChanged": true, "restructure": true,
                "answer": conversation::answer(0, editor.window_state.conversation.applied, None, editor.focus_corrections())}).to_string(),
            Err(error) => json!({"error": error}).to_string(),
        },
        "focus_file" | "close_file" => {
            let verb = request["verb"].as_str().unwrap();
            editor.refresh_files();
            let file = if let Some(index) = request.get("index") {
                match index.as_u64().and_then(|i| editor.open_files.rows.get(i as usize)).and_then(|f| f["id"].as_i64()) {
                    Some(id) => id,
                    None => return json!({"error": "the file index is outside the open files"}).to_string(),
                }
            } else { request.get("file").and_then(Value::as_i64).unwrap_or(editor.file.id.0) };
            let before = editor.file.id.0;
            let result = if verb == "focus_file" {
                editor.focus_file(file).map(|()| (json!([]), Vec::<i64>::new(), Vec::<String>::new()))
            } else { editor.close_file(file).map(|closed| (closed.steps, closed.buffers, closed.files)) };
            match result {
                // The last file closed leaves the editor open with none:
                // `binding` is null and the window is drawn again empty.
                Ok((steps, freed, files)) => {
                    let empty = !editor.open_files.open;
                    let moved = before != editor.file.id.0;
                    json!({"turn": "route", "steps": steps, "freedBuffers": freed, "deleteFiles": files, "binding": editor.binding(), "focusChanged": moved || empty,
                    "restructure": true, "fileAction": {"verb": verb, "file": file}, "empty": empty,
                    "answer": moved.then(|| conversation::answer(0, editor.window_state.conversation.applied, None, editor.focus_corrections()))}).to_string()
                }
                Err(error) => json!({"error": error}).to_string(),
            }
        }
        "open_file" => {
            let paths = request.get("paths").or_else(|| request.get("path"));
            let paths = match paths {
                Some(Value::String(path)) if !path.is_empty() => vec![json!(path)],
                Some(Value::Array(paths)) if paths.iter().all(|p| p.as_str().is_some_and(|p| !p.is_empty())) => paths.clone(),
                _ => return json!({"error": "open_file needs a path or an array of paths"}).to_string(),
            };
            json!({"turn": "route", "openFile": {"paths": paths}}).to_string()
        }
        "playing" => {
            editor.window_state.playing = request.get("file").and_then(Value::as_i64);
            if let Ok(facts) = serde_json::from_value::<Facts>(request.clone())
                && let Some(meters) = facts.meters {
                editor.set_meters(meters.map(|m| (m.bus, m.channels)));
            }
            let mut corrections = editor.files_corrections();
            // with no file open the window draws no take, pane or meter
            if !editor.open_files.open {
                return json!({"corrections": corrections}).to_string();
            }
            let anchor = editor.playhead_anchor();
            let targets: BTreeSet<_> = editor.pictures().chain(editor.window_state.pane).collect();
            corrections.extend(targets.into_iter().map(|widget| Correction {
                widget: i64::from(widget), props: json!({"playhead_at": anchor})
            }));
            if let (Some(widget), Some((bus, channels))) = (editor.window_state.meter_widget, editor.window_state.meters) {
                corrections.push(Correction { widget: i64::from(widget), props: json!({"bus": bus, "channels": channels}) });
            }
            json!({"corrections": corrections}).to_string()
        }
        "show_files" | "hide_files" | "toggle_files" => {
            editor.window_state.files_visible = match request["verb"].as_str().unwrap() {
                "show_files" => true, "hide_files" => false, _ => !editor.window_state.files_visible,
            };
            json!({"turn": "route", "restructure": true}).to_string()
        }
        "envelope" => json!({"points": editor.envelope_quads(false)}).to_string(),
        "show_envelope" | "setautomation" | "apply_envelope" | "fold_channel" | "solo_channel"
        | "show_all_channels" => {
            serde_json::to_string(&editor.act(&request, version)).unwrap()
        }
        "props" if editor.window_state.envelope_widget == Some(widget) => editor
            .envelope_corrections()
            .first()
            .map_or_else(|| "{}".into(), |c| c.props.to_string()),
        // the picture is told the list again: it draws it from its files
        "props" if editor.open_files.open => json!({"segments": editor.segments()}).to_string(),
        "props" => Value::Object(props(widget)).to_string(),
        "event" => {
            let event = serde_json::from_value::<Event>(request.clone()).unwrap_or_default();
            serde_json::to_string(&editor.event(&event, version)).unwrap_or_else(|_| "{}".into())
        }
        "playback" => editor.playback_file().to_string(),
        "parts" => json!({ "parts": editor.list(), "frames": editor.length() }).to_string(),
        "segments" => editor.segments().to_string(),
        "on_disk" => {
            let reason = editor.on_disk(&request["stamps"]);
            let seq = request["seq"].as_i64().unwrap_or(0);
            json!({"turn": "route",
                "answer": conversation::answer(seq, version, reason, Vec::new())})
            .to_string()
        }
        "span" => {
            let span = get("span")
                .as_array()
                .and_then(|r| Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?)));
            let (start, len) = editor.set_span(span);
            json!({ "sel_start": start, "sel_len": len }).to_string()
        }
        "locate" => {
            json!({ "cue": editor.set_cursor(get("at").as_f64().unwrap_or(0.0)) }).to_string()
        }
        "play" => json!(editor.play(editor.file.presentation.looping)).to_string(),
        "looping" => {
            editor.file.presentation.looping = get("on").as_bool().unwrap_or(false);
            "{}".into()
        }
        _ => "{}".into(),
    }
}

/// The domain an audio editor's structure is registered under.
pub const DOMAIN: &str = PARTS;

mod channels;
mod envelope;

mod files;
/// Typed two-phase audio operations.
pub mod operations;
mod sources;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod layer_tests {
    use super::*;

    fn call(editor: &mut AudioEditor, value: Value) -> Value {
        serde_json::from_str(&call_json(editor, &value.to_string())).unwrap()
    }

    #[test]
    fn edit_body_calls_and_menu_verbs_correct_the_pane_without_editing_samples() {
        let mut editor =
            new_json(r#"{"path":"/in/take1.wav","workspace":"/work","frames":1000}"#).unwrap();
        editor.window_state.bare = true;
        let tree = call(&mut editor, json!({"verb":"window","widget":17,"pane":18}));
        assert_eq!(tree["children"][0]["type"], "pane");
        let out = call(
            &mut editor,
            json!({"verb":"layer","name":"placement","version":4}),
        );
        assert_eq!(out["answer"]["corrections"][0]["widget"], 18);
        assert_eq!(
            out["answer"]["corrections"][0]["props"]["layer"],
            "placement"
        );
        assert_eq!(out["answer"]["docVersion"], 4);
        assert_eq!(
            call(&mut editor, json!({"verb":"layer_take"}))["layer"],
            "take"
        );
        assert!(
            call(&mut editor, json!({"verb":"layer","name":"points"}))
                .get("error")
                .is_some()
        );
        assert_eq!(call(&mut editor, json!({"verb":"layer"}))["layer"], "take");
        assert_eq!(editor.length(), 1000);
        assert_eq!(editor.layers(), json!(["peak", "rms"]));
    }

    #[test]
    fn presets_correct_the_open_picture_without_editing_the_take() {
        let mut editor =
            new_json(r#"{"path":"/in/take1.wav","workspace":"/work","frames":1000}"#).unwrap();
        editor.window(17, None);
        for verb in ["view_spectrogram", "view_both", "view_wave"] {
            let out = call(&mut editor, json!({"verb":verb,"version":8}));
            assert_eq!(out["turn"], "route");
            assert_eq!(out["changed"], false);
            assert_eq!(out["answer"]["docVersion"], 8);
            assert_eq!(out["answer"]["corrections"][0]["widget"], 17);
            let stack = Stack::checked(&out["layers"]).unwrap();
            assert_eq!(
                out["answer"]["corrections"][0]["props"]["layers"],
                stack.to_wire()
            );
            assert!(stack.axis_domain().is_ok());
        }
        assert_eq!(editor.length(), 1000);
    }

    #[test]
    fn object_layers_and_their_settings_survive_changes_and_refusals() {
        let mut editor =
            new_json(r#"{"layers":["spectrogram",{"draw":"peak","y":"box","alpha":0.4}]}"#)
                .unwrap();
        call(&mut editor, json!({"verb":"hide_layer","name":"peak"}));
        assert_eq!(editor.layers()[1]["visible"], false);
        call(&mut editor, json!({"verb":"show_layer","name":"peak"}));
        assert_eq!(editor.layers()[1]["alpha"], 0.4);
        assert_eq!(editor.layers()[1]["y"], "box");
        let before = editor.layers();
        let out = call(
            &mut editor,
            json!({"verb":"layers","stack":["spectrogram","peak"]}),
        );
        assert!(out["error"].as_str().unwrap().contains("different domains"));
        assert_eq!(editor.layers(), before);
        assert!(
            call(
                &mut editor,
                json!({"verb":"layer_alpha","name":"peak","alpha":2})
            )
            .get("error")
            .is_some()
        );
        assert_eq!(editor.layers(), before);
        call(&mut editor, json!({"verb":"solo_layer","name":"peak"}));
        assert_eq!(editor.layers()[1]["solo"], true);
        call(&mut editor, json!({"verb":"solo_layer","name":"peak"}));
        assert_eq!(editor.layers(), before);
    }

    #[test]
    fn menu_events_use_the_same_preset_and_keep_the_stamp() {
        let mut editor =
            new_json(r#"{"path":"/in/take1.wav","workspace":"/work","frames":1000,"window":90}"#)
                .unwrap();
        editor.window(17, None);
        let event = Event {
            addr: "/gui_event".into(),
            args: vec![json!(17), json!(7), json!(0), json!("view_both")],
        };
        let out = editor.event(&event, 3);
        assert_eq!(out.layers, Some(json!(["peak", "rms"])));
        assert!(out.restructure, "the stacked views add a lane");
        let answer = serde_json::to_value(out.answer.unwrap()).unwrap();
        assert_eq!(answer["seq"], 7);
        assert_eq!(answer["docVersion"], 3);
    }

    /// **The two views are alternatives in one lane, or stacked in two**,
    /// never superposed: the stacked window holds the take's lane, with its
    /// pane and the gain curve in it, over the spectrogram's own lane on the
    /// same axis, which carries the time ruler. Moving between one lane and
    /// two redefines the window; between the alternatives, a prop does.
    #[test]
    fn the_views_are_alternatives_in_a_lane_or_stacked_lanes() {
        let mut editor =
            new_json(r#"{"path":"/in/take1.wav","workspace":"/work","frames":1000}"#).unwrap();
        editor.window_state.bare = true;
        let window = |editor: &mut AudioEditor| {
            call(
                editor,
                json!({"verb":"window","widget":17,"pane":18,"envelope":19,"lane":20}),
            )
        };
        let one = window(&mut editor);
        assert_eq!(one["children"][0]["type"], "pane", "one lane");
        let out = call(&mut editor, json!({"verb":"view_spectrogram"}));
        assert_eq!(out["restructure"], Value::Null, "an alternative is a prop");
        let out = call(&mut editor, json!({"verb":"view_both"}));
        assert_eq!(out["restructure"], true);
        let corrected: Vec<_> = editor.reread().iter().map(|c| c.widget).collect();
        assert_eq!(corrected, [17, 20], "both pictures read the take again");
        let two = window(&mut editor);
        let lanes = &two["children"][0];
        assert_eq!(lanes["type"], "layout");
        let (pane, lane) = (&lanes["children"][0], &lanes["children"][1]);
        assert_eq!(pane["id"], 18);
        assert_eq!(
            pane["axes"]["x"]["ruler"], false,
            "the ruler is the bottom lane's"
        );
        assert_eq!(pane["children"][0]["measure"], "peak rms");
        assert_eq!(
            pane["children"][1]["id"], 19,
            "the envelope stays over the take"
        );
        assert_eq!(lane["id"], 20);
        assert_eq!(lane["link"], 18, "on the take's axis");
        assert_eq!(lane["measure"], "spectrogram");
        assert_eq!(lane["editable"], false);
        let out = call(&mut editor, json!({"verb":"view_wave"}));
        assert_eq!(out["restructure"], true, "back to one lane");
        assert_eq!(window(&mut editor)["children"][0]["type"], "pane");
    }
}
