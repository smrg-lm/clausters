//! **The audio editor's turns**: the window over a take made of parts, what one
//! message from the host does to the list, and the steps that make what the
//! list now names.
//!
//! The shape is the samples editor's -- a message read by the conversation, a
//! gesture read in its vocabulary, an acknowledgement, and an entry for the
//! history the caller keeps -- with one difference in what comes back: not a
//! write into the take, but the **steps** that render a new file where a turn
//! needs one ([`Outcome::steps`]); the application's own reader feeds its
//! engine the new list. The window is told the new list by reference (`segments`, through
//! [`crate::audio::shared`]). Each entry names the takes before and after it
//! by number and states the files each reads, so the history holds them for
//! as long as it can walk to them.
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

use clausters_core::tempoclock::{samples_to_secs, secs_to_samples};
use clausters_document::clipboard::{Clipboard, Content, HeldCopy, SegmentPart};
use clausters_document::parts::{self, PARTS};
use clausters_document::session::Part;
use clausters_document::view::NOT_AN_EDIT;
use clausters_document::view::layers::Stack;
use clausters_document::{Lifetime, Opaque, Range, SourceId, SourceRef};
use clausters_editing::apply::{Step, steps_json};
use clausters_editing::conversation::{self, Answer, Conversation, Correction, Message};
use clausters_editing::pass::{Pass, space};
use clausters_editing::samples;
use clausters_editing::verbs::{self, ActionState, ActionTarget, PasteTarget};

use crate::application::resources::ServerInstance;
use crate::audio::dialogs;
use crate::audio::renders;
use crate::audio::settings;
use crate::audio::shared;
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

/// **The frames one piece of a reverse reads**, 2^23: about three minutes
/// at 48 kHz, 64 MiB of stereo float held while it renders.
const REVERSE_PIECE: u64 = 1 << 23;

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

/// **The file in focus, as its playback sounds it**: its identity, its
/// shape and whether a window shows it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PlaybackFile {
    /// The file, as the editor numbers its open files.
    pub id: i64,
    /// How many channels it is.
    pub channels: u32,
    /// How many frames it comes to, at its own rate.
    pub frames: u64,
    /// Its rate.
    pub rate: f64,
    /// Whether a window shows it.
    pub active: bool,
}

/// **A file played again after an edit**: the file, and whether the window
/// is told so.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PlaybackUpdate {
    /// The file.
    pub file: PlaybackFile,
    /// Whether the edit was the file in view's.
    pub publish: bool,
}

/// What a window asks of a file in the editor's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileVerb {
    /// The file comes into focus.
    FocusFile,
    /// The file is closed.
    CloseFile,
}

/// **A file of the list the window named**: what it asks, and the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FileAction {
    /// What it asks.
    pub verb: FileVerb,
    /// The file.
    pub file: i64,
}

/// Files the window asked to open, each read by its caller.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OpenFiles {
    /// Their paths.
    pub paths: Vec<String>,
}

/// **A new file's shape**, as the New form says it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Shape {
    /// How many channels.
    pub channels: u32,
    /// How many frames.
    pub frames: u64,
    /// Its rate.
    pub rate: f64,
}

/// **The files to look at on disk**, the window having come back: the
/// paths, and the message its answer is for.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CheckDisk {
    /// The open files' paths.
    pub paths: Vec<String>,
    /// The window's message.
    pub seq: i64,
}

/// **A cut's second half**, committed once its copy is on the clipboard:
/// the span the copy took from the file, at the revision it took it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "verb", rename = "finish_cut", rename_all = "camelCase")]
pub struct FinishCut {
    /// The file cut.
    pub file: i64,
    /// Its revision when the copy was taken.
    pub before_revision: u64,
    /// The gesture's values.
    pub values: Vec<Value>,
    /// The gesture's message.
    pub seq: i64,
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
    pub playback_update: Option<PlaybackUpdate>,
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
    /// Internal marker: the turn applied the gain curve, which the view puts
    /// away once the change commits.
    #[serde(skip)]
    pub(crate) put_away: bool,
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
    /// **The steps to carry out**, in the JSON a runner walks: the renders
    /// that make the new files a turn's list reads. The answer asks the
    /// window to read the list again, so it is sent once these have landed.
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
    pub new_file: Option<Shape>,
    /// **A file was asked to be opened** in the window (`openFile`):
    /// `{path}`, for the client to open as it opens a new one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_file: Option<OpenFiles>,
    /// The file binding the client focuses or closes, chosen by the app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_action: Option<FileAction>,
    /// **The files a closed file leaves to delete**: its renders, its
    /// history gone with it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub delete_files: Vec<String>,
    /// Whether the editor holds no file now.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub empty: bool,
    /// **The files to look at on disk** (`checkDisk`), the window having come
    /// back: `{paths, seq}`, the open files read from the disk, for the
    /// client to `stat` and hand to the `on_disk` verb, which answers the
    /// window's `seq`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_disk: Option<CheckDisk>,
    /// **A copy**: the clipboard document naming the copy the application
    /// keeps, for the caller to put on the host's clipboard
    /// (`/gui_clipboard`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clipboard: Option<String>,
    /// The producing operation has published this document in its composition.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub clipboard_ready: bool,
    /// A cut continuation, committed only after copy and publication succeed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clipboard_cut: Option<FinishCut>,
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
            if name.starts_with(settings::PREFIX) {
                let value = values.first().cloned().unwrap_or_else(|| json!(tag));
                return self.settings_event(&name, &value);
            }
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
                out.file_action = file["id"].as_i64().map(|file| FileAction {
                    verb: FileVerb::FocusFile,
                    file,
                });
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
                out.check_disk = Some(CheckDisk {
                    paths,
                    seq: message.seq,
                });
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
        if matches!(
            message.tag.as_str(),
            "toggle_files" | "spectrogram_settings" | "close_file"
        ) {
            out.turn = Kind::Route;
            if message.tag == "toggle_files" {
                self.window_state.files_visible = !self.window_state.files_visible;
                out.restructure = true;
            } else if message.tag == "spectrogram_settings" {
                self.window_state.settings_visible = !self.window_state.settings_visible;
                out.restructure = true;
            } else {
                out.file_action = Some(FileAction {
                    verb: FileVerb::CloseFile,
                    file: self.file.id.0,
                });
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
    "spectrogram_settings",
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
    "spectrogram",
    "set_spectrogram",
    "show_spectrogram_settings",
    "hide_spectrogram_settings",
    "toggle_spectrogram_settings",
    "playing",
    "props",
    "event",
];

/// Why a verb that acts on a file is refused when none is open.
const NO_FILE_OPEN: &str = "no file is open";

/// What an audio editor is put back to when a step it took part in is
/// refused elsewhere ([`AudioEditor::before_step`]).
pub(crate) struct Kept(parts::Take, u64);

/// **An audio editor**: the take it opened, the list of parts it is now, and
/// one view's end of the conversation.
#[derive(Clone, Debug)]
pub struct AudioEditor {
    file: AudioFileState,
    window_state: AudioWindowState,
    open_files: AudioFiles,
}

/// An open file's identity in its editor.
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
    // Where a save writes: the file's path, or empty for a file not named
    // yet, whose first save asks for one.
    target: String,
    // The file the take was opened from, one of `sources`; none for a file
    // not written yet.
    opened: Option<SourceId>,
    // The files the list reads, and their rates as the list counts them.
    sources: Sources,
    rates: BTreeMap<SourceId, f64>,
    // A new file, which the open writes as silence before anything is put
    // in it.
    silence: bool,
    // A file a part cannot read in place (a compressed one), which the open
    // decodes into the file the take reads before anything else.
    decoded_from: Option<String>,
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
    /// **The spectrogram's settings**, the window's whatever file is in focus,
    /// and whether their panel stands beside the take.
    settings: settings::Settings,
    settings_visible: bool,
    playing: Option<i64>,
    // What its lists, copies and history entries are kept under: its
    // context's, once a context holds it.
    shared: shared::Shared,
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

impl AudioEditor {
    /// An editor over the file `opened` (none for a file not written yet),
    /// `channels` wide at `rate`, measured by `layers`, whose history is at
    /// `version`; or the reason the stack is refused.
    pub(crate) fn new(
        opened: Option<FileSource>,
        channels: u32,
        rate: f64,
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
                target: String::new(),
                opened: None,
                sources: Sources::new(),
                rates: BTreeMap::new(),
                silence: false,
                decoded_from: None,
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
                settings: settings::Settings::default(),
                settings_visible: false,
                playing: None,
                shared: shared::Shared::default(),
            },
            open_files: AudioFiles {
                rows: Vec::new(),
                parked: BTreeMap::new(),
                open: true,
                next_id: 2,
            },
        };
        if let Some(file) = opened {
            editor.file.target = if file.render {
                String::new()
            } else {
                file.path.clone()
            };
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
        self.file.contents.parts = self.tree_of(&[part(id, 0, frames)]);
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
    fn render_output(&self, what: &str, verb: &str) -> Result<renders::Output, String> {
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
            label: renders::operation(verb),
        })
    }

    /// The list a render reads: the span `from..to` of the take, at its rate.
    fn render_list(&self, from: u64, to: u64) -> Result<renders::List, String> {
        let span = self.file.contents.parts.span(from, to, &self.rates_of())?;
        Ok(sources::list(
            &span.to_vec(),
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

    /// The list the take is now, flat.
    pub fn list(&self) -> Vec<Part> {
        self.file.contents.parts.to_vec()
    }

    /// **What its lists, copies and history entries are kept under**: its
    /// context's ([`shared::Shared`]).
    pub fn shared(&self) -> &shared::Shared {
        &self.window_state.shared
    }

    /// Keeps what it hands on by reference under `shared`: its context's.
    pub(crate) fn share(&mut self, shared: shared::Shared) {
        self.window_state.shared = shared;
    }

    /// The take's parts as they are held: a tree, shared.
    pub(crate) fn tree_of(&self, parts: &[Part]) -> parts::Tree {
        parts::Tree::from_parts(parts, &self.rates_of()).unwrap_or_default()
    }

    /// **The file each part of the list reads**, by the stem of its path:
    /// what a test reads of the list without knowing the ids.
    #[cfg(test)]
    pub(crate) fn part_files(&self) -> Vec<String> {
        self.file
            .contents
            .parts
            .iter()
            .filter_map(|placed| self.file.sources.get(&placed.part.source.source))
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
    /// added writes its renders where the editor's do, unless the request
    /// names its own, and a file it makes is named after the number it will
    /// be open under.
    pub(crate) fn with_folder(&self, request: &Value) -> Value {
        let mut request = request.clone();
        if request.get("workspace").is_none_or(Value::is_null) {
            request["workspace"] = json!(self.window_state.workspace);
        }
        // the number the file will be open under, which names a file it makes
        request["serial"] = json!(self.open_files.next_id);
        request
    }

    /// How many frames the take is now.
    pub fn length(&self) -> u64 {
        self.file.contents.parts.frames()
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
    /// that let go of it said so), its peaks and the spectral summary beside
    /// them. Nothing for a file the user
    /// owns, or one a list still reads.
    pub(crate) fn release(&mut self, source: SourceId) -> Vec<String> {
        // a take a history entry named, kept for it
        if self.window_state.shared.let_go_take(source.0) {
            return Vec::new();
        }
        if self.rooted().contains(&source) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for file in std::iter::once(&mut self.file).chain(self.open_files.parked.values_mut()) {
            if let Some(gone) = file.sources.remove(&source) {
                file.rates.remove(&source);
                if gone.render && !out.contains(&gone.path) {
                    let spectra = sources::spectra_beside(&gone.peaks);
                    out.push(gone.path);
                    out.push(gone.peaks);
                    out.push(spectra);
                }
            }
        }
        out
    }

    /// **Saving: the take as the edits have left it, written where it belongs.**
    ///
    /// With no `to` it is written **over the file the take was opened
    /// from**. With a `to` it is a save-as, which a later save then writes
    /// over. The list is rendered
    /// into the file through its parts, so what lands is exactly what is
    /// heard (`renders::consolidate`).
    ///
    /// **A file the list reads is never written while it is read**: the
    /// take is rendered into the working folder, the file it replaces is
    /// moved there and the list goes on naming it where it now is, and the
    /// render is moved where the file was (two `Step::Move`s). So an undo
    /// past the save still has the frames it reads.
    ///
    /// These are a save operation's steps, built over its draft, which they
    /// leave naming the destination and any file moved.
    fn save_steps(&mut self, to: Option<String>, format: &str) -> Result<Vec<Step>, String> {
        let path = self.save_target(to, format)?;
        let frames = self.length();
        let list = self.render_list(0, frames)?;
        let read = self.file.sources.values().any(|s| s.path == path);
        let steps = if read {
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
                label: renders::operation("save"),
            };
            // The list reads the file it replaces where it now is: the
            // source keeps its id, so every list the history holds reads the
            // moved file too, and it is a render of this editor's from now
            // on, which goes when nothing names it.
            for source in self.file.sources.values_mut() {
                if source.path == path {
                    source.path = held.clone();
                    source.peaks = sources::peaks_beside(&held);
                    source.render = true;
                }
            }
            vec![
                renders::consolidate(&list, &out),
                Step::Move {
                    from: path.clone(),
                    to: held,
                },
                Step::Move {
                    from: fresh,
                    to: path.clone(),
                },
            ]
        } else {
            let out = renders::Output {
                path: path.clone(),
                peaks: String::new(),
                format: format.into(),
                label: renders::operation("save"),
            };
            vec![renders::consolidate(&list, &out)]
        };
        self.file.target = path;
        Ok(steps)
    }

    /// **Where a save writes**, `to` or the file the take was opened from,
    /// checked: a known format, and a path.
    pub(super) fn save_target(&self, to: Option<String>, format: &str) -> Result<String, String> {
        let to = to.unwrap_or_else(|| self.file.target.clone());
        if !matches!(format, "float" | "int24" | "int16") {
            return Err(format!(
                "'{format}' is not a sample format (float, int24, int16)"
            ));
        }
        if to.is_empty() {
            return Err("a file is saved under a path".into());
        }
        Ok(to)
    }

    /// The file a save writes over; empty for a file not named yet.
    pub fn target(&self) -> &str {
        &self.file.target
    }

    /// **Opening: the file read for its peaks.**
    ///
    /// Nothing of the file is read but its header (the caller's, before the
    /// editor exists) and, once, its samples into its peaks, which are kept in
    /// the working folder: the file measured in blocks ([`Step::Measure`]). A new file
    /// is written as silence first.
    pub fn open(&mut self) -> Result<Value, String> {
        self.open_steps().map(|steps| steps_json(&steps))
    }

    fn open_steps(&mut self) -> Result<Vec<Step>, String> {
        let mut steps = Vec::new();
        let opened = self
            .file
            .opened
            .and_then(|id| self.file.sources.get(&id).cloned());
        let silence = std::mem::take(&mut self.file.silence);
        if let (Some(from), Some(file)) = (self.file.decoded_from.take(), opened.as_ref()) {
            let out = renders::Output {
                path: file.path.clone(),
                peaks: file.peaks.clone(),
                format: "float".into(),
                label: renders::operation("open_file"),
            };
            steps.push(renders::decode(
                &from,
                file.channels,
                file.frames,
                file.rate,
                &out,
            ));
            return Ok(steps);
        }
        if let (true, Some(file)) = (silence, opened.as_ref()) {
            let out = renders::Output {
                path: file.path.clone(),
                peaks: file.peaks.clone(),
                format: "float".into(),
                label: renders::operation("new_file"),
            };
            steps.push(renders::silence(
                file.frames,
                file.channels,
                file.rate,
                &out,
            ));
            return Ok(steps);
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
            steps.push(renders::measure(
                &whole,
                &file.peaks,
                &renders::operation("open_file"),
            ));
        }
        Ok(steps)
    }

    /// **The list the window draws**: the take's parts as they are held,
    /// handed to the host by reference ([`shared::Shared::say`]) and named here by
    /// the number it is kept under, with its shape -- so what an edit tells
    /// the window is the same few bytes whatever the take's length.
    pub(crate) fn segments(&self) -> Value {
        let list = self.file_list(&self.file.contents.parts);
        let frames = list.tree.frames();
        json!({"shared": self.window_state.shared.say(list), "channels": self.file.channels,
               "sample_rate": self.file.rate, "frames": frames})
    }

    /// **`tree` as its readers read it**, over the files of this file's
    /// table it names.
    pub(crate) fn file_list(&self, tree: &parts::Tree) -> parts::FileList {
        sources::file_list(tree, &self.file.sources, self.file.channels, self.file.rate)
    }

    /// **The window**, numbered with the take widget's id, drawing the list.
    ///
    /// `meter` is the id the level meter is drawn under. The meter is part
    /// of the window from the start, whatever the file's work: until a
    /// playback says where it writes ([`Self::set_meters`]) it reads no bus,
    /// and the answer that says so corrects it (`Self::meter_corrections`).
    pub fn window(&mut self, widget: i32, meter: Option<i32>) -> Value {
        self.window_state.widget = Some(widget);
        let (bus, channels) = self
            .window_state
            .meters
            .unwrap_or((-1, self.file.channels as usize));
        let meter = meter.filter(|_| self.open_files.open).map(|id| MeterAt {
            widget: id,
            bus,
            channels,
        });
        // only a meter the window draws is corrected afterwards
        self.window_state.meter_widget = meter.map(|m| m.widget);
        let label = self.pane_label();
        let segments = self.segments();
        let spectrogram = self.window_state.settings.props();
        let mut tree = window(&Window {
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
            segments: &segments,
            spectrogram: Some(&spectrogram),
        });
        self.compose_envelope(&mut tree);
        self.compose_settings(&mut tree);
        self.refresh_files();
        self.compose_files(&mut tree);
        tree
    }

    /// **The spectrogram's settings panel, beside the take**, when it is
    /// shown and the window numbered its widgets.
    fn compose_settings(&self, tree: &mut Value) {
        if !self.window_state.settings_visible || !self.open_files.open {
            return;
        }
        let Some(panel) = self
            .window_state
            .settings
            .panel(&self.window_state.dialogs.ids)
        else {
            return;
        };
        let Some(children) = tree.get_mut("children").and_then(Value::as_array_mut) else {
            return;
        };
        if let Some(index) = children
            .iter()
            .position(|c| c["weight"].as_f64().is_some_and(|w| w > 0.0))
        {
            let work = children.remove(index);
            children.insert(
                index,
                crate::panels::beside(work, panel, crate::panels::Side::Right),
            );
        }
    }

    /// The spectrogram's settings.
    pub fn spectrogram(&self) -> &settings::Settings {
        &self.window_state.settings
    }

    /// **Changes the spectrogram's settings** a request names: the pictures
    /// of the take are told their props, and the panel, when it is up, what
    /// it shows.
    pub(super) fn set_spectrogram(&mut self, request: &Value) -> (Option<String>, Vec<Correction>) {
        if let Err(why) = self.window_state.settings.set(request) {
            return (Some(why), Vec::new());
        }
        (None, self.settings_corrections(true))
    }

    /// **A report from the settings panel**: the setting it changes, and the
    /// pictures told. The panel already shows what the hand did.
    fn settings_event(&mut self, name: &str, value: &Value) -> (Option<String>, Vec<Correction>) {
        let Some(request) = settings::Settings::read(name, value) else {
            return (None, Vec::new());
        };
        if let Err(why) = self.window_state.settings.set(&request) {
            return (Some(why), self.settings_corrections(true));
        }
        (None, self.settings_corrections(false))
    }

    /// What the take's pictures, and the panel when `panel`, are told of the
    /// settings.
    fn settings_corrections(&self, panel: bool) -> Vec<Correction> {
        let props = self.window_state.settings.props();
        let mut corrections: Vec<Correction> = self
            .pictures()
            .map(|widget| Correction {
                widget: i64::from(widget),
                props: props.clone(),
            })
            .collect();
        if panel
            && self.window_state.settings_visible
            && let Some(column) = self
                .window_state
                .settings
                .panel(&self.window_state.dialogs.ids)
        {
            let mut stack = vec![column];
            while let Some(node) = stack.pop() {
                if let Some(children) = node["children"].as_array() {
                    stack.extend(children.iter().cloned());
                }
                let Some(id) = node["id"].as_i64() else {
                    continue;
                };
                let shown = ["index", "value"]
                    .iter()
                    .filter_map(|k| node.get(*k).map(|v| (k.to_string(), v.clone())))
                    .collect::<serde_json::Map<_, _>>();
                corrections.push(Correction {
                    widget: id,
                    props: Value::Object(shown),
                });
            }
        }
        corrections
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

    /// **The meter where the playback writes**: the correction of the
    /// window's meter to the buses it reads, once the playback has said, and
    /// nothing for a window that draws none.
    pub(super) fn meter_corrections(&self) -> Vec<Correction> {
        let (Some(widget), Some((bus, channels))) =
            (self.window_state.meter_widget, self.window_state.meters)
        else {
            return Vec::new();
        };
        vec![Correction {
            widget: i64::from(widget),
            props: json!({"bus": bus, "channels": channels}),
        }]
    }

    /// **What the space bar plays**, with the loop switch `looping`: from the
    /// selection or the position cursor, to the end of the selection or the
    /// take, and back to the cursor -- or over and over.
    pub fn play(&self, looping: bool) -> Play {
        let (start, pass) = space(
            looping,
            self.file.presentation.selection,
            self.file.presentation.cursor,
            Some(self.length()),
        );
        Play {
            start,
            pass,
            back: self.file.presentation.cursor.unwrap_or(start),
        }
    }

    /// [`Self::play`] under the file's own loop switch.
    pub fn space(&self) -> Play {
        self.play(self.file.presentation.looping)
    }

    /// The span a loop repeats over, as the view stands: the selection, or
    /// the take with none -- what [`space`] says a looping pass covers.
    fn follow(&self, place: bool) -> Option<Follow> {
        match space(
            true,
            self.file.presentation.selection,
            self.file.presentation.cursor,
            Some(self.length()),
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

    /// **The take a history payload names**: by its revision, kept by the
    /// file; or, from a payload that carries its parts, read from them.
    pub(crate) fn take_named(&self, payload: &Value) -> Option<parts::Take> {
        if let Some(take) = payload.get("take").and_then(Value::as_u64) {
            return self.window_state.shared.take(take);
        }
        parts::Take::read(&Opaque(payload.clone()), &self.rates_of())
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
    /// the take. Answers whether the payload is a list.
    pub fn apply(&mut self, payload: &Value) -> bool {
        let Some(mut next) = self.take_named(payload) else {
            return false;
        };
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
        true
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
}

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

/// The domain an audio editor's structure is registered under.
pub const DOMAIN: &str = PARTS;

mod actions;
mod channels;
mod door;
mod envelope;
mod gestures;

mod files;
/// Typed two-phase audio operations.
pub mod operations;
pub(crate) mod sources;
pub(crate) use door::call_value;
pub(crate) use door::{FileAt, open_paths, refusal, turns};
pub use door::{call_json, new_json};
pub(crate) use sources::{peaks_beside, peaks_for_opened};

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
