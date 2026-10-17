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

use std::collections::HashMap;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_core::osc::{OscMessage, OscType};
use clausters_core::tempoclock::{samples_to_secs, secs_to_samples};
use clausters_document::clipboard::{Clipboard, Content};
use clausters_document::parts::{self, PARTS};
use clausters_document::session::{Location, Part, Source};
use clausters_document::view::NOT_AN_EDIT;
use clausters_document::view::layers::Stack;
use clausters_document::{Lifetime, Opaque, Range, SourceId, SourceRef};
use clausters_editing::apply::{Step, steps_json};
use clausters_editing::audio_playback::{Pass, space};
use clausters_editing::conversation::{self, Answer, Conversation, Correction, Message};
use clausters_editing::load::stitch_message;
use clausters_editing::samples;
use clausters_editing::sources::{Held, stitch};

use crate::audio::dialogs;
use crate::samples::{MEASURES, MeterAt, Window, props, window};
use crate::turn::{self, Converse, Event, Kind, Leg, Record, int, number, text};

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

/// **What one turn came to.**
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// What kind of turn the message was.
    pub turn: Kind,
    /// A view change, also returned when no window is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layers: Option<Value>,
    /// A preview-chain change, independent of the take and its history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<Value>,
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
}

turn::turned!(Outcome);

impl Converse for AudioEditor {
    type Outcome = Outcome;

    fn conversation(&mut self) -> &mut Conversation {
        &mut self.conversation
    }

    fn window_id(&self) -> Option<i32> {
        self.window
    }

    fn closed(&mut self) {
        self.window = None;
    }

    fn owns(&self, widget: i64, _tag: &str) -> bool {
        self.widget
            .into_iter()
            .chain(self.pane)
            .chain(self.envelope_widget)
            .chain(self.lane)
            .any(|w| i64::from(w) == widget)
            || self.dialog_name(widget).is_some()
    }

    fn resync(&mut self, widget: i64) -> Vec<Correction> {
        if self.envelope_widget.map(i64::from) == Some(widget) {
            self.envelope_corrections()
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
        self.gesture(widget, tag, values, out)
    }

    fn window_verb(&mut self, message: &Message, args: &[Value], out: &mut Outcome) -> bool {
        if message.addr != "/gui_event" || !message.is_window {
            return false;
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
                | "preview_on"
                | "preview_off"
                | "remove_effect"
                | "apply"
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

/// What an audio editor is put back to when a step it took part in is
/// refused elsewhere ([`AudioEditor::before_step`]).
pub(crate) struct Kept(parts::Take, BTreeSet<i64>);

/// **An audio editor**: the take it opened, the list of parts it is now, the
/// join that list is drawn from, and one view's end of the conversation.
#[derive(Clone, Debug)]
pub struct AudioEditor {
    /// The buffer the take was opened from. Never written, never freed here.
    take: i64,
    channels: u32,
    rate: f64,
    /// The join the window draws: the list, stitched.
    display: i64,
    /// **What a save writes**: the file the take was read from, or the server
    /// buffer it was opened from -- or whichever a save-as last named.
    target: Target,
    /// The buffer the editor was opened over. Written by a save and by
    /// nothing else: the editor edits a private copy of it ([`Self::open`]).
    opened: i64,
    /// Whether the private copy has been made.
    copied: bool,
    contents: parts::Take,
    envelope_widget: Option<i32>,
    preview_on: bool,
    effect_installed: bool,
    /// Frames of every take this editor knows -- the one it opened and each it
    /// made -- by buffer.
    frames: BTreeMap<i64, u64>,
    /// Buffers the caller handed over for new takes, next first.
    spare: Vec<i64>,
    /// Where a take only the history holds is written when it leaves memory,
    /// or `None` when none may.
    scratch: Option<String>,
    /// The takes that are on disk rather than in a buffer: each keeps its
    /// buffer number as its identity, and is read back into it when a list
    /// that reads it is stitched again.
    spilled: BTreeSet<i64>,
    chunk: usize,
    name: Option<String>,
    /// The visualization layers of the view in the take's lane.
    layers: Stack,
    /// Which views the window shows ([`Views`]).
    views: Views,
    /// The id the client numbered the spectrogram's lane with, for the
    /// stacked views.
    lane: Option<i32>,
    title: String,
    size: (i64, i64),
    /// Whether the window is composed with no chrome ([`Window::bare`]).
    bare: bool,
    window: Option<i32>,
    widget: Option<i32>,
    pane: Option<i32>,
    edit_layer: String,
    /// Where the position cursor stands, in frames of the take, once placed.
    cursor: Option<u64>,
    /// The selection, as a half-open span of frames, while there is one.
    selection: Option<(u64, u64)>,
    /// Where the level meter is read from, once a playback measures the take.
    meters: Option<(i32, usize)>,
    /// The channels drawn folded, a view state outside the history.
    folded: BTreeSet<u32>,
    /// The dialogs' widgets, by name ([`dialogs::names`]).
    dialogs: dialogs::Ids,
    /// The form that is up, when one is.
    dialog: Option<dialogs::Open>,
    conversation: Conversation,
}

/// A part reading frames `start..end` of `buffer`.
fn part(buffer: i64, start: u64, end: u64) -> Part {
    Part {
        source: SourceRef {
            source: SourceId(buffer as u64),
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

impl AudioEditor {
    /// An editor over the take in `take` -- `frames` long, `channels` wide,
    /// at `rate` -- drawn through the join `display`, measured by `layers`,
    /// whose history is at `version`; or the reason the stack is refused.
    pub fn new(
        take: i64,
        frames: u64,
        channels: u32,
        rate: f64,
        display: i64,
        layers: &[String],
        version: i64,
    ) -> Result<Self, String> {
        let mut editor = Self {
            take,
            channels: channels.max(1),
            rate: if rate > 0.0 { rate } else { 48_000.0 },
            display,
            target: Target::Buffer(take),
            opened: take,
            copied: false,
            contents: parts::Take::default(),
            envelope_widget: None,
            preview_on: false,
            effect_installed: false,
            frames: BTreeMap::new(),
            spare: Vec::new(),
            scratch: None,
            spilled: BTreeSet::new(),
            chunk: DEFAULT_CHUNK,
            name: None,
            layers: Stack::checked(&json!(layers))?,
            views: Views::Waveform,
            lane: None,
            title: "Audio".into(),
            size: (1000, 520),
            bare: false,
            window: None,
            widget: None,
            pane: None,
            edit_layer: "take".into(),
            cursor: None,
            selection: None,
            meters: None,
            folded: BTreeSet::new(),
            dialogs: dialogs::Ids::new(),
            dialog: None,
            conversation: Conversation::new(version),
        };
        editor.reopen(take, frames);
        editor.ensure_envelope();
        Ok(editor)
    }

    /// Starts over on the take in `take`: the list is the whole of it.
    fn reopen(&mut self, take: i64, frames: u64) {
        self.take = take;
        self.frames.insert(take, frames);
        self.contents.parts = if frames > 0 {
            vec![part(take, 0, frames)]
        } else {
            Vec::new()
        };
    }

    /// The list the take is now.
    pub fn list(&self) -> &[Part] {
        &self.contents.parts
    }

    /// How many frames the take is now.
    pub fn length(&self) -> u64 {
        parts::length(&self.contents.parts).unwrap_or(0)
    }

    /// The canonical array of layers, back to front.
    pub fn layers(&self) -> Value {
        self.layers.to_value()
    }

    /// Set a stack of names, through the shared validator.
    pub fn set_layers(&mut self, stack: &[String]) -> Result<(), String> {
        self.layers = Stack::checked(&json!(stack))?;
        Ok(())
    }

    fn edit_layer(&mut self, name: &str) -> Result<Vec<Correction>, String> {
        if !matches!(name, "take" | "placement")
            && !(matches!(name, "points" | "points:0") && self.envelope_widget.is_some())
        {
            return Err(format!("this pane has no '{name}' body"));
        }
        let relabel = self.edit_layer.starts_with("points") || name.starts_with("points");
        self.edit_layer = name.into();
        Ok(self
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
        self.widget
            .into_iter()
            .map(|widget| Correction {
                widget: i64::from(widget),
                props: json!({"layers": self.layers.to_wire()}),
            })
            .collect()
    }

    fn layer_verb(&mut self, verb: &str, request: &Value) -> Result<Outcome, String> {
        let next = match verb {
            "layers" => match request.get("stack") {
                Some(stack) => Stack::checked(stack)?,
                None => self.layers.clone(),
            },
            // The two alternatives in the take's lane, and both stacked: the
            // waveform in the take's lane over the spectrogram's own.
            "view_wave" | "view_both" => Stack::checked(&json!(["peak", "rms"]))?,
            "view_spectrogram" => Stack::checked(&json!(["spectrogram"]))?,
            _ => self.layers.edit(
                request
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                verb,
                request.get("alpha").and_then(Value::as_f64),
            )?,
        };
        self.layers = next;
        let before = self.views;
        self.views = match verb {
            "view_wave" => Views::Waveform,
            "view_spectrogram" => Views::Spectrogram,
            "view_both" => Views::Stacked,
            _ => self.views,
        };
        Ok(Outcome {
            turn: Kind::Route,
            restructure: before.lanes() != self.views.lanes(),
            layers: Some(self.layers()),
            answer: Some(conversation::answer(0, 0, None, self.layer_corrections())),
            ..Outcome::default()
        })
    }

    /// **The takes this editor cannot let go of**: the one it opened, which is
    /// the caller's, and every take the list reads now.
    pub fn rooted(&self) -> Vec<SourceId> {
        let mut out = parts::sources(&self.contents.parts);
        let opened = SourceId(self.take as u64);
        if !out.contains(&opened) {
            out.push(opened);
        }
        out
    }

    /// How many bytes the take in `buffer` holds, or `None` for one this editor
    /// never knew.
    pub fn bytes(&self, buffer: i64) -> Option<u64> {
        self.frames
            .get(&buffer)
            .map(|frames| frames * u64::from(self.channels) * 4)
    }

    /// Forgets a take the caller is about to free, so nothing here names it.
    pub fn forget(&mut self, buffer: i64) {
        if buffer != self.take {
            self.frames.remove(&buffer);
            self.spilled.remove(&buffer);
        }
    }

    /// Whether the take in `buffer` is on disk rather than in its buffer --
    /// so freeing it gives back a number and frees no buffer.
    pub fn is_spilled(&self, buffer: i64) -> bool {
        self.spilled.contains(&buffer)
    }

    /// Where the take in `buffer` is written when it leaves memory.
    fn path(&self, buffer: i64) -> Option<String> {
        let scratch = self.scratch.as_deref()?;
        Some(format!(
            "{}/take-{buffer}.wav",
            scratch.trim_end_matches('/')
        ))
    }

    /// **Takes a take out of memory**: the steps that write it to the scratch
    /// path and free its buffer, or `None` for one that cannot go -- no
    /// scratch, the take this editor opened, one the list reads now, one
    /// already on disk, or one it never made. Written as float, so what is
    /// read back is what was written.
    pub fn spill(&mut self, buffer: i64) -> Option<Value> {
        let path = self.path(buffer)?;
        let reads = parts::sources(&self.contents.parts).contains(&SourceId(buffer as u64));
        if buffer == self.take || reads || self.spilled.contains(&buffer) {
            return None;
        }
        self.frames.get(&buffer)?;
        self.spilled.insert(buffer);
        let index = buffer as i32;
        Some(steps_json(&[
            send(
                "/buffer_write",
                vec![
                    OscType::Int(index),
                    OscType::String(path),
                    OscType::String("wav".into()),
                    OscType::String("float".into()),
                ],
            ),
            Step::AwaitDone {
                command: "/buffer_write".into(),
                index: Some(index),
            },
            send("/buffer_free", vec![OscType::Int(index)]),
            Step::AwaitDone {
                command: "/buffer_free".into(),
                index: Some(index),
            },
        ]))
    }

    /// **Saving: the take as the edits have left it, written where it belongs.**
    ///
    /// With no `to` it is written **over what the take was opened from** --
    /// the file it was read from, or the server buffer it was opened over. An
    /// audio editor's save is the user's own act on the user's own file or
    /// buffer, the one place the program writes one. With a `to` it is a
    /// save-as: the edit goes to that file or that buffer, which becomes the
    /// one a later save writes. The join is read through its parts, so what
    /// lands is exactly what is heard.
    ///
    /// **A file** is written as a WAV in `format` (`"float"`, `"int24"` or
    /// `"int16"`). **A buffer** is rewritten whole, at the take's length -- a
    /// cut made it shorter, a paste longer -- so whatever reads it hears the
    /// edit from then on; saving into a buffer that is sounding is heard as the
    /// glitch it is, and it is the user's to avoid rather than the editor's to
    /// refuse. A new buffer is the caller's to hand over, as every buffer
    /// number here is.
    pub fn save(&mut self, to: Option<Target>, format: &str) -> Result<Value, String> {
        let to = to.unwrap_or_else(|| self.target.clone());
        if !matches!(format, "float" | "int24" | "int16") {
            return Err(format!(
                "'{format}' is not a sample format (float, int24, int16)"
            ));
        }
        let display = self.display as i32;
        let steps = match &to {
            Target::File(path) if path.is_empty() => {
                return Err("a file is saved under a path".into());
            }
            Target::File(path) => vec![
                send(
                    "/buffer_write",
                    vec![
                        OscType::Int(display),
                        OscType::String(path.clone()),
                        OscType::String("wav".into()),
                        OscType::String(format.into()),
                    ],
                ),
                Step::AwaitDone {
                    command: "/buffer_write".into(),
                    index: Some(display),
                },
            ],
            Target::Buffer(buffer) => {
                if *buffer == self.display || self.frames.contains_key(buffer) {
                    return Err(format!(
                        "buffer {buffer} is one this editor reads: save into another"
                    ));
                }
                let frames = self.length();
                let width = u64::from(self.channels);
                let mut steps = self.allocate(*buffer, frames);
                steps.push(send(
                    "/buffer_gen",
                    vec![
                        OscType::Int(*buffer as i32),
                        OscType::String("copy".into()),
                        OscType::Int(0),
                        OscType::Int(display),
                        OscType::Int(0),
                        OscType::Int((frames * width) as i32),
                    ],
                ));
                steps.push(Step::AwaitDone {
                    command: "/buffer_gen".into(),
                    index: Some(*buffer as i32),
                });
                steps
            }
        };
        self.target = to;
        Ok(steps_json(&steps))
    }

    /// What a save writes over.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// A spill the caller could not carry out -- the write was refused, a
    /// quota full -- so the take is still in its buffer.
    pub fn kept(&mut self, buffer: i64) {
        self.spilled.remove(&buffer);
    }

    /// The steps that read back every take on disk the list now reads, before
    /// the join over it is stitched.
    fn restored(&mut self) -> Vec<Step> {
        let wanted: Vec<i64> = parts::sources(&self.contents.parts)
            .iter()
            .map(|s| s.0 as i64)
            .filter(|b| self.spilled.contains(b))
            .collect();
        let mut steps = Vec::new();
        for buffer in wanted {
            let Some(path) = self.path(buffer) else {
                continue;
            };
            self.spilled.remove(&buffer);
            steps.push(send(
                "/buffer_allocRead",
                vec![OscType::Int(buffer as i32), OscType::String(path)],
            ));
            steps.push(Step::AwaitDone {
                command: "/buffer_allocRead".into(),
                index: Some(buffer as i32),
            });
        }
        steps
    }

    /// **Opening: a private copy of what the editor was opened over, and the
    /// window's join stitched over it.**
    ///
    /// The buffer the editor was handed is never named by the history, so a
    /// save can rewrite it -- the user's buffer, or the one a file was read
    /// into -- without taking the frames an undo needs away from under it.
    /// The copy is a buffer the caller handed over, as every new take is;
    /// with none, the editor cannot open.
    pub fn open(&mut self) -> Result<Value, String> {
        let mut steps = Vec::new();
        if !self.copied {
            let frames = self.frames.get(&self.opened).copied().unwrap_or(0);
            let copy = self.next_buffer()?;
            steps.extend(self.allocate(copy, frames));
            steps.push(send(
                "/buffer_gen",
                vec![
                    OscType::Int(copy as i32),
                    OscType::String("copy".into()),
                    OscType::Int(0),
                    OscType::Int(self.opened as i32),
                    OscType::Int(0),
                    OscType::Int((frames * u64::from(self.channels)) as i32),
                ],
            ));
            steps.push(Step::AwaitDone {
                command: "/buffer_gen".into(),
                index: Some(copy as i32),
            });
            self.frames.remove(&self.opened);
            self.reopen(copy, frames);
            self.copied = true;
        }
        steps.extend(self.stitched());
        Ok(steps_json(&steps))
    }

    /// The list, stitched into the display buffer, and the `/done` awaited.
    fn stitched(&self) -> Vec<Step> {
        let held: HashMap<SourceId, Held> = self
            .frames
            .iter()
            .map(|(buffer, frames)| {
                (
                    SourceId(*buffer as u64),
                    Held {
                        buffer: *buffer as i32,
                        channels: self.channels as usize,
                        frames: *frames,
                        rate: self.rate,
                    },
                )
            })
            .collect();
        let mut source =
            Source::volatile(Lifetime::Temporary).shaped(self.channels, self.length(), self.rate);
        source.location = Location::Segments {
            parts: self.contents.parts.clone(),
        };
        let Some(made) = stitch(&source, &held) else {
            return Vec::new();
        };
        let display = self.display as i32;
        vec![
            Step::Send(stitch_message(display, &made)),
            Step::AwaitDone {
                command: "/buffer_stitch".into(),
                index: Some(display),
            },
        ]
    }

    /// **The window**, numbered with the take widget's id, drawing the join.
    ///
    /// `meter` is the id the level meter is drawn under; the meter is there
    /// once a playback has said where it writes ([`Self::set_meters`]).
    pub fn window(&mut self, widget: i32, meter: Option<i32>) -> Value {
        self.widget = Some(widget);
        let meter = meter.zip(self.meters).map(|(id, (bus, channels))| MeterAt {
            widget: id,
            bus,
            channels,
        });
        let label = self.pane_label();
        let mut tree = window(&Window {
            buffer: self.display,
            channels: self.channels,
            name: Some(&label),
            layers: &self.layers,
            rate: self.rate,
            widget,
            pane: self.pane,
            layer: &self.edit_layer,
            title: &self.title,
            size: self.size,
            meter,
            bare: self.bare,
            dialogs: dialogs::stack(&self.dialogs),
            folded: &self.folded(),
            lane: self.lane.filter(|_| self.views == Views::Stacked),
        });
        self.compose_envelope(&mut tree);
        tree
    }

    /// **Where the level is read from**: the first control bus the
    /// playback's meter writes and how many there are, or `None`.
    pub fn set_meters(&mut self, meters: Option<(i32, usize)>) {
        self.meters = meters;
    }

    /// **What the space bar plays**, with the loop switch `looping`: from the
    /// selection or the position cursor, to the end of the selection or the
    /// take, and back to the cursor -- or over and over.
    pub fn play(&self, looping: bool) -> Play {
        let (start, pass) = space(looping, self.selection, self.cursor, self.length());
        Play {
            start,
            pass,
            back: self.cursor.unwrap_or(start),
        }
    }

    /// The span a loop repeats over, as the view stands: the selection, or
    /// the take with none -- what [`space`] says a looping pass covers.
    fn follow(&self, place: bool) -> Option<Follow> {
        match space(true, self.selection, self.cursor, self.length()).1 {
            Pass::Loop { from, to } => Some(Follow { from, to, place }),
            Pass::Until { .. } => None,
        }
    }

    /// **What a history step is put back to** when another structure of the
    /// same step refuses it: the list, and which takes are on disk.
    pub(crate) fn before_step(&self) -> Kept {
        Kept(self.contents.clone(), self.spilled.clone())
    }

    /// Puts back what [`before_step`](Self::before_step) kept.
    pub(crate) fn put_back(&mut self, kept: Kept) {
        self.contents = kept.0;
        self.spilled = kept.1;
    }

    /// **A step of the history, applied**: the list a payload states becomes
    /// the take, and the answer is the steps that stitch it. Nothing when the
    /// payload is not a list.
    pub fn apply(&mut self, payload: &Value) -> Option<Value> {
        let mut next = parts::Take::read(&Opaque(payload.clone()))?;
        // Visibility is view state: walking the edit order restores the curve,
        // not which body the user has chosen to look at since the edit.
        for curve in &mut next.automation {
            if let Some(held) = self.contents.automation.iter().find(|a| a.id == curve.id) {
                curve.visible = held.visible;
            }
        }
        self.contents = next;
        self.ensure_envelope();
        let mut steps = self.restored();
        steps.extend(self.stitched());
        Some(steps_json(&steps))
    }

    /// **One message from the host**, read and answered ([`turn::turn`]).
    pub fn event(&mut self, event: &Event, version: i64) -> Outcome {
        turn::turn(self, event, version)
    }

    /// **The widgets that draw the take**: the view in its lane and, stacked,
    /// the spectrogram's lane under it.
    pub(super) fn pictures(&self) -> impl Iterator<Item = i32> + '_ {
        self.widget
            .into_iter()
            .chain(self.lane.filter(|_| self.views == Views::Stacked))
    }

    /// **Every picture of the take, read again** ([`resync`]).
    pub(super) fn reread(&self) -> Vec<Correction> {
        self.pictures().flat_map(|w| resync(i64::from(w))).collect()
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
        samples_to_secs(units.round() as i64, self.rate)
    }

    /// The frame of the take at `secs`, clamped to it.
    fn frame_at(&self, secs: f64) -> u64 {
        (secs_to_samples(secs, self.rate).max(0) as u64).min(self.length())
    }

    /// **The time range, set by the caller** -- `[start, end]` in seconds, or
    /// `None` -- as a sweep leaves it: what the space bar plays. Answers the
    /// band the window draws, `(sel_start, sel_len)` in frames.
    pub fn set_span(&mut self, span: Option<(f64, f64)>) -> (u64, u64) {
        self.selection = span
            .map(|(a, b)| (self.frame_at(a), self.frame_at(b)))
            .filter(|(a, b)| b > a);
        self.selection.map_or((0, 0), |(a, b)| (a, b - a))
    }

    /// **The position cursor, placed by the caller** at `secs`, as a click on
    /// the ruler places it. Answers its frame, where a stopped playback cues.
    pub fn set_cursor(&mut self, secs: f64) -> u64 {
        let frame = self.frame_at(secs);
        self.cursor = Some(frame);
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
        if tag == "points" && self.envelope_widget.map(i64::from) == Some(widget) {
            return self.set_envelope_event(values, out);
        }
        if matches!(tag, "preview_on" | "preview_off" | "remove_effect") {
            return self.preview_event(tag, out);
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
            self.observe(tag, values, out);
            return (None, Vec::new());
        }
        let edited = match tag {
            "draw" | "sample" => self.stroke(tag, values),
            "cut" => self.cut(values),
            "paste" => self.paste(values),
            "mix" => self.mix(values),
            _ => return (None, Vec::new()),
        };
        match edited {
            Ok(Some((label, list, mut steps))) => {
                let before = self.contents.clone();
                self.contents.parts = list;
                steps.extend(self.restored());
                steps.extend(self.stitched());
                out.record = Some(self.record(&label, &before));
                out.steps = Some(steps_json(&steps));
                out.changed = true;
                out.version += 1;
                // **The picture is read again**, unlike a stroke over a buffer
                // the host holds: the join it draws is replaced whole by the
                // stitch, and only the answer can say so. A caller sends the
                // answer once the steps have landed.
                (None, self.reread())
            }
            Ok(None) => (None, Vec::new()),
            Err(why) => (Some(why), self.reread()),
        }
    }

    /// The entry a turn leaves: the list after, the list before, and the takes
    /// each reads.
    fn record(&self, label: &str, before: &parts::Take) -> Record {
        let ids = |list: &[Part]| parts::sources(list).iter().map(|s| s.0).collect();
        Record {
            label: label.into(),
            legs: vec![Leg {
                forward: json!({ "edit": self.contents.payload().0 }),
                backward: before.payload().0,
                key: String::new(),
                holds_forward: ids(&self.contents.parts),
                holds_backward: ids(&before.parts),
            }],
        }
    }

    /// A buffer for a new take, or the reason there is none.
    fn next_buffer(&mut self) -> Result<i64, String> {
        if self.spare.is_empty() {
            return Err("no buffer was handed over for the take this edit writes".into());
        }
        Ok(self.spare.remove(0))
    }

    /// The steps that make `buffer` a take `frames` long at the editor's rate.
    fn allocate(&self, buffer: i64, frames: u64) -> Vec<Step> {
        vec![
            send(
                "/buffer_alloc",
                vec![
                    OscType::Int(buffer as i32),
                    OscType::Int(frames as i32),
                    OscType::Int(self.channels as i32),
                    OscType::Float(self.rate as f32),
                ],
            ),
            Step::Sync(buffer as i32),
        ]
    }

    /// **A pencil stroke is a new take the size of the stroke**, made of the
    /// frames it was drawn over with the drawn channel written in, and spliced
    /// over those frames.
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
        let mut drawn: Vec<Value> = payload
            .get("values")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let total = self.length();
        if start >= total || channel >= self.channels {
            return Err("the stroke is outside the take".into());
        }
        drawn.truncate((total - start) as usize);
        let frames = drawn.len() as u64;
        let buffer = self.next_buffer()?;
        let mut steps = self.allocate(buffer, frames);
        if self.channels > 1 {
            // The frames it was drawn over, every channel: the other channels
            // keep what they held, and the server is what copies them.
            let width = i64::from(self.channels);
            steps.push(send(
                "/buffer_gen",
                vec![
                    OscType::Int(buffer as i32),
                    OscType::String("copy".into()),
                    OscType::Int(0),
                    OscType::Int(self.display as i32),
                    OscType::Int((start as i64 * width) as i32),
                    OscType::Int((frames as i64 * width) as i32),
                ],
            ));
            steps.push(Step::AwaitDone {
                command: "/buffer_gen".into(),
                index: Some(buffer as i32),
            });
        }
        steps.extend(samples::write_steps(
            buffer as i32,
            self.channels,
            &json!({ "channel": channel, "start": 0, "values": drawn }),
            self.chunk,
        ));
        self.frames.insert(buffer, frames);
        let list = parts::replace(
            &self.contents.parts,
            start,
            start + frames,
            &[part(buffer, 0, frames)],
        )?;
        Ok(Some((taken.label, list, steps)))
    }

    /// **A cut takes the span out of the list**, and moves no samples.
    fn cut(&mut self, values: &[Value]) -> Result<Edited, String> {
        let at = |i: usize| values.get(i).map_or(0.0, number).round().max(0.0) as u64;
        let (start, len) = (at(0), at(1));
        if len == 0 {
            return Ok(None);
        }
        let list = parts::remove(&self.contents.parts, start, start + len)?;
        if list.is_empty() {
            return Err("a take cannot be cut down to nothing".into());
        }
        Ok(Some(("cut".into(), list, Vec::new())))
    }

    /// **A paste is a new take holding the block**, put in at the position.
    ///
    /// The block travels with the gesture -- the clipboard is the host's -- so
    /// its samples are written into a take of their own; it is refused when
    /// its width or its rate is not the take's, since fitting either would be
    /// an edit nobody asked for.
    fn paste(&mut self, values: &[Value]) -> Result<Edited, String> {
        let (position, samples) = self.block(values)?;
        let frames = samples.len() as u64 / u64::from(self.channels);
        if frames == 0 {
            return Ok(None);
        }
        let buffer = self.next_buffer()?;
        let mut steps = self.allocate(buffer, frames);
        steps.extend(self.fill(buffer, &samples));
        self.frames.insert(buffer, frames);
        let list = parts::insert(&self.contents.parts, position, &[part(buffer, 0, frames)])?;
        Ok(Some(("paste".into(), list, steps)))
    }

    /// **A mix adds the block onto the frames it lands on**, and the result is
    /// a new take over those frames.
    ///
    /// The frames under the block are copied out of the join into a take of
    /// their own, the block is written into a scratch buffer, and the server
    /// adds one into the other (`/buffer_mix`) -- the sum is an operation over
    /// samples, so it is the server's. The scratch buffer is freed by the same
    /// steps and goes back to the ones this editor holds. A block that runs
    /// past the end of the take is mixed as far as the take goes.
    fn mix(&mut self, values: &[Value]) -> Result<Edited, String> {
        let (position, mut samples) = self.block(values)?;
        let width = u64::from(self.channels);
        let total = self.length();
        if position >= total {
            return Err("the mix lands past the end of the take".into());
        }
        let frames = (samples.len() as u64 / width).min(total - position);
        samples.truncate((frames * width) as usize);
        if frames == 0 {
            return Ok(None);
        }
        if self.spare.len() < 2 {
            return Err("a mix needs two buffers handed over: the take and a scratch one".into());
        }
        let (take, scratch) = (self.spare.remove(0), self.spare.remove(0));
        let mut steps = self.allocate(take, frames);
        steps.push(send(
            "/buffer_gen",
            vec![
                OscType::Int(take as i32),
                OscType::String("copy".into()),
                OscType::Int(0),
                OscType::Int(self.display as i32),
                OscType::Int((position * width) as i32),
                OscType::Int((frames * width) as i32),
            ],
        ));
        steps.push(Step::AwaitDone {
            command: "/buffer_gen".into(),
            index: Some(take as i32),
        });
        steps.extend(self.allocate(scratch, frames));
        steps.extend(self.fill(scratch, &samples));
        steps.push(send(
            "/buffer_mix",
            vec![
                OscType::Int(take as i32),
                OscType::Int(0),
                OscType::Int(scratch as i32),
                OscType::Int(0),
                OscType::Int(frames as i32),
                OscType::Float(1.0),
            ],
        ));
        steps.push(Step::AwaitDone {
            command: "/buffer_mix".into(),
            index: Some(take as i32),
        });
        steps.push(send("/buffer_free", vec![OscType::Int(scratch as i32)]));
        steps.push(Step::AwaitDone {
            command: "/buffer_free".into(),
            index: Some(scratch as i32),
        });
        self.spare.push(scratch);
        self.frames.insert(take, frames);
        let list = parts::replace(
            &self.contents.parts,
            position,
            position + frames,
            &[part(take, 0, frames)],
        )?;
        Ok(Some(("mix".into(), list, steps)))
    }

    /// **The block a paste or a mix carries**: where it goes, and its samples
    /// interleaved at the take's width -- or why it does not fit this take.
    fn block(&self, values: &[Value]) -> Result<(u64, Vec<f32>), String> {
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
        if channels != self.channels {
            return Err(format!(
                "a block of {channels} channels does not paste into a take of {}",
                self.channels
            ));
        }
        if sample_rate > 0.0 && (sample_rate - self.rate).abs() > f64::EPSILON {
            return Err(format!(
                "a block at {sample_rate} Hz does not paste into a take at {} Hz",
                self.rate
            ));
        }
        let mut samples: Vec<f32> = match values.get(3 + blob) {
            Some(Value::Array(items)) => items.iter().map(|v| number(v) as f32).collect(),
            _ => return Err("the clipboard's samples did not travel with it".into()),
        };
        let width = u64::from(self.channels);
        let frames = frames.min(samples.len() as u64 / width);
        samples.truncate((frames * width) as usize);
        Ok((position, samples))
    }

    /// The steps that write `samples`, interleaved, into `buffer` from its
    /// first frame, and wait for the last write.
    fn fill(&self, buffer: i64, samples: &[f32]) -> Vec<Step> {
        let chunk = self.chunk.max(1);
        let mut steps: Vec<Step> = samples
            .chunks(chunk)
            .enumerate()
            .map(|(i, run)| {
                send(
                    "/buffer_setRange",
                    vec![
                        OscType::Int(buffer as i32),
                        OscType::Int((i * chunk) as i32),
                        OscType::Blob(clausters_core::osc::sample_blob(run)),
                    ],
                )
            })
            .collect();
        steps.push(Step::AwaitDone {
            command: "/buffer_setRange".into(),
            index: Some(buffer as i32),
        });
        steps
    }

    /// A tag that says what the view is looking at rather than what changed.
    fn observe(&mut self, tag: &str, values: &[Value], out: &mut Outcome) {
        match tag {
            "locate" if !values.is_empty() => {
                let frame = number(&values[0]);
                let cursor = frame.max(0.0).round() as u64;
                self.cursor = Some(cursor);
                out.cue = self.cursor;
                out.locate = Some(self.secs_at(frame));
                // **A sweep let go puts the cursor on the selection's start**
                // (a click empties the selection at its press), and that is
                // when a loop in progress is put into the span it was moved
                // to: a head past the loop's end would run on and never wrap.
                if self.selection.is_some_and(|(from, _)| from == cursor) {
                    out.follow = self.follow(true);
                }
            }
            "selection" => {
                let frame = |i: usize| values.get(i).map_or(0.0, number).max(0.0);
                let (from, len) = (frame(0), frame(1));
                self.selection =
                    (len > 0.0).then(|| (from.round() as u64, (from + len).round() as u64));
                let at = |i: usize| values.get(i).map_or(0.0, |v| self.secs_at(number(v)));
                out.selection = Some(json!({ "start": at(0), "len": at(1) }));
                // The span follows the hand while a loop plays; the head does
                // not, until the sweep is let go.
                out.follow = self.follow(false);
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
    take: Option<i64>,
    frames: Option<u64>,
    channels: Option<u32>,
    rate: Option<f64>,
    display: Option<i64>,
    /// Buffers for new takes, appended to what the editor already holds.
    buffers: Option<Vec<i64>>,
    chunk: Option<usize>,
    /// The file the take was read from.
    path: Option<String>,
    /// **A file not written yet**: its first save asks for a path.
    untitled: Option<bool>,
    /// The directory a take leaves memory for.
    scratch: Option<String>,
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
            self.channels = channels.max(1);
        }
        if let Some(rate) = facts.rate.filter(|r| *r > 0.0) {
            self.rate = rate;
        }
        if let Some(take) = facts.take {
            let frames = facts
                .frames
                .unwrap_or_else(|| self.frames.get(&take).copied().unwrap_or(0));
            self.reopen(take, frames);
        } else if let Some(frames) = facts.frames {
            self.reopen(self.take, frames);
        }
        if let Some(display) = facts.display {
            self.display = display;
        }
        if let Some(buffers) = facts.buffers {
            self.spare.extend(buffers);
        }
        if let Some(path) = facts.path.filter(|p| !p.is_empty()) {
            self.target = Target::File(path);
        } else if facts.untitled == Some(true) {
            self.target = Target::File(String::new());
        }
        if let Some(scratch) = facts.scratch.filter(|s| !s.is_empty()) {
            self.scratch = Some(scratch);
        }
        if let Some(chunk) = facts.chunk {
            self.chunk = chunk.max(1);
        }
        if let Some(name) = facts.name {
            self.name = name;
        }
        if let Some(title) = facts.title {
            self.title = title;
        }
        if let Some(chrome) = facts.chrome {
            self.bare = !chrome;
        }
        if let Some(w) = facts.w {
            self.size.0 = w;
        }
        if let Some(h) = facts.h {
            self.size.1 = h;
        }
        if let Some(window) = facts.window {
            self.window = window;
        }
        if let Some(meters) = facts.meters {
            self.meters = meters.map(|m| (m.bus, m.channels));
        }
    }
}

/// **An editor built from a JSON request** -- `take`, `frames`, `channels`,
/// `rate`, `display`, `buffers`, `chunk`, `path` (the file the take was read
/// from, which a save writes over), `scratch` (the directory a take only
/// the history holds is written to when it leaves memory), `name`, `layers`,
/// `title`, `w`, `h`, `chrome` (`false` for a window with no menu bar and no
/// tools) and `version` -- or the reason it cannot be.
pub fn new_json(request: &str) -> Result<AudioEditor, String> {
    let facts: Facts =
        serde_json::from_str(request).map_err(|e| format!("not an audio editor request: {e}"))?;
    let layers = facts.layers.clone().unwrap_or_else(|| json!(MEASURES));
    let stack = Stack::checked(&layers)?;
    let names: Vec<String> = MEASURES.iter().map(|name| name.to_string()).collect();
    let mut editor = AudioEditor::new(
        facts.take.unwrap_or(0),
        facts.frames.unwrap_or(0),
        facts.channels.unwrap_or(1),
        facts.rate.unwrap_or(0.0),
        facts.display.unwrap_or(0),
        &names,
        facts.version.unwrap_or(0),
    )?;
    editor.layers = stack;
    editor.adopt(Facts {
        layers: None,
        take: None,
        frames: None,
        ..facts
    });
    Ok(editor)
}

/// **One verb of an editor, over JSON** -- the door both clients bind.
///
/// - `sync` -- any of the facts [`new_json`] reads: handed over before the
///   verbs that read them; `buffers` are added to the ones the editor holds,
///   and `meters` (`{"bus", "channels"}` or `null`) says where the playback's
///   meter writes.
///   Answers `{"spare"}`, how many it holds now -- a turn takes one for a
///   stroke or a paste and two for a mix, so a caller keeps it topped up.
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
/// - `apply` -- `payload`: `{"steps"}` for a list the history handed back, or
///   `{}` for a payload that is not one.
/// - `parts` -- `{"parts", "frames"}`: the list, and how long it is.
/// - `open` -- `{"steps"}`: the private copy of what the editor was opened
///   over, and the join stitched over it; or `{"error"}` with no buffer
///   handed over for the copy.
/// - `save` -- `path` or `buffer`, and `format` (`"float"` when absent), all
///   optional: `{"steps", "path"}` or `{"steps", "buffer"}`, what writes the
///   take over what it was opened from or, given one, into that file or that
///   buffer; or `{"error"}`.
/// - `kept` -- `buffer`: a spill the caller could not carry out, so that take
///   is still in its buffer. Answers `{}`.
/// - `span` -- `span`: `[start, end]` in seconds, or `null`: the time range
///   the space bar plays, as a sweep leaves it. Answers `{"sel_start",
///   "sel_len"}`, the band the window draws, in frames.
/// - `locate` -- `at`, in seconds: the position cursor. Answers `{"cue"}`,
///   its frame.
/// - `play` -- `looping`: the [`Play`] the space bar would ask for.
/// - `acknowledge` -- `seq`, `version`, `reason`: an [`Answer`].
///
/// An unknown verb answers `{}`.
pub fn call_json(editor: &mut AudioEditor, request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return "{}".into();
    };
    let get = |key: &str| request.get(key).cloned().unwrap_or(Value::Null);
    let widget = int(&get("widget")) as i32;
    let version = int(&get("version"));
    match request
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "sync" => {
            if let Ok(facts) = serde_json::from_value::<Facts>(request.clone()) {
                editor.adopt(Facts {
                    layers: None,
                    ..facts
                });
            }
            json!({ "spare": editor.spare.len() }).to_string()
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
                    value["measure"] = json!(editor.layers.to_wire());
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
                Ok(corrections) => json!({"turn": "route", "layer": editor.edit_layer,
                    "answer": conversation::answer(0, version, None, corrections.unwrap_or_default())}).to_string(),
                Err(error) => json!({"error": error}).to_string(),
            }
        }
        "window" => {
            if let Some(ids) = request.get("dialogs").and_then(Value::as_object) {
                editor.dialogs = ids
                    .iter()
                    .filter_map(|(name, id)| Some((name.clone(), id.as_i64()? as i32)))
                    .collect();
            }
            editor.envelope_widget = request
                .get("envelope")
                .and_then(Value::as_i64)
                .map(|n| n as i32);
            editor.ensure_envelope();
            editor.pane = request
                .get("pane")
                .and_then(Value::as_i64)
                .map(|n| n as i32);
            editor.lane = request
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
        "preview" => editor.preview_state().to_string(),
        "dialogs" => json!({"dialogs": dialogs::names()}).to_string(),
        "folded" => json!({"folded": editor.folded()}).to_string(),
        "file" => editor.file().to_string(),
        "envelope" => json!({"points": editor.envelope_quads(false)}).to_string(),
        "show_envelope" | "setautomation" | "apply_envelope" | "preview_on" | "preview_off"
        | "remove_effect" | "fold_channel" | "solo_channel" | "show_all_channels" => {
            serde_json::to_string(&editor.act(&request, version)).unwrap()
        }
        "props" if editor.envelope_widget == Some(widget) => editor
            .envelope_corrections()
            .first()
            .map_or_else(|| "{}".into(), |c| c.props.to_string()),
        "props" => Value::Object(props(widget)).to_string(),
        "open" => match editor.open() {
            Ok(steps) => json!({ "steps": steps }).to_string(),
            Err(error) => json!({ "error": error }).to_string(),
        },
        "event" => {
            let event = serde_json::from_value::<Event>(request.clone()).unwrap_or_default();
            serde_json::to_string(&editor.event(&event, version)).unwrap_or_else(|_| "{}".into())
        }
        "apply" => match editor.apply(&get("payload")) {
            Some(steps) => json!({ "steps": steps }).to_string(),
            None => "{}".into(),
        },
        "parts" => json!({ "parts": editor.list(), "frames": editor.length() }).to_string(),
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
        "play" => json!(editor.play(get("looping").as_bool().unwrap_or(false))).to_string(),
        "save" => {
            let format = get("format").as_str().unwrap_or("float").to_string();
            let to = match (get("path").as_str(), get("buffer").as_i64()) {
                (Some(path), _) => Some(Target::File(path.to_string())),
                (None, Some(buffer)) => Some(Target::Buffer(buffer)),
                (None, None) => None,
            };
            match editor.save(to, &format) {
                Ok(steps) => {
                    let mut answer = json!({ "steps": steps });
                    match editor.target() {
                        Target::File(path) => answer["path"] = json!(path),
                        Target::Buffer(buffer) => answer["buffer"] = json!(buffer),
                    }
                    answer.to_string()
                }
                Err(error) => json!({ "error": error }).to_string(),
            }
        }
        "kept" => {
            editor.kept(int(&get("buffer")));
            "{}".into()
        }
        "acknowledge" => serde_json::to_string(&editor.acknowledge(
            int(&get("seq")),
            version,
            get("reason").as_str().map(str::to_string),
        ))
        .unwrap_or_else(|_| "{}".into()),
        _ => "{}".into(),
    }
}

/// The domain an audio editor's structure is registered under.
pub const DOMAIN: &str = PARTS;

mod channels;
mod envelope;
mod files;

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
        let mut editor = new_json(r#"{"take":1,"frames":1000}"#).unwrap();
        editor.bare = true;
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
        let mut editor = new_json(r#"{"take":1,"frames":1000}"#).unwrap();
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
        let mut editor = new_json(r#"{"take":1,"frames":1000,"window":90}"#).unwrap();
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
        let mut editor = new_json(r#"{"take":1,"frames":1000}"#).unwrap();
        editor.bare = true;
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
