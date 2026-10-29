//! **The editor's JSON door**: the facts a caller hands over, an editor
//! built from a request, and one verb of the editor answered.

use super::*;

/// The facts a caller hands over, in [`new_json`] and in `sync`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Facts {
    codec: Option<String>,
    sample_format: Option<String>,
    /// The opened file's shape, read from its header by the client.
    frames: Option<u64>,
    channels: Option<u32>,
    rate: Option<f64>,
    /// The number the file is open under in its editor, which names a file
    /// the editor makes for it: two new files are two files.
    serial: Option<i64>,
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
        if let Some(workspace) = facts.workspace.filter(|w| !w.is_empty()) {
            self.window_state.workspace = Some(workspace);
        }
        // where a save writes from now on, as a save-as left it
        if let Some(path) = facts.path.filter(|p| !p.is_empty()) {
            self.file.target = path;
        } else if facts.untitled == Some(true) {
            self.file.target = String::new();
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

/// The containers a part of a list reads in place; a file of any other is
/// decoded when it is opened. A file whose codec the caller did not say is
/// taken as one of them.
const IN_PLACE: &[&str] = &["wav", "w64", "rf64"];

/// Whether a part reads a file of `codec` in place.
fn reads_in_place(codec: Option<&str>) -> bool {
    codec.is_none_or(|c| IN_PLACE.contains(&c))
}

/// **An editor built from a JSON request** -- the file it opens (`path`,
/// and `frames`, `channels` and `rate` from its header), or `untitled` for a
/// new file of that shape; `workspace`, the working folder its renders are
/// written into and the peaks of what it opened are kept in; `serial`, the
/// number a new file is named after; `chunk`, `name`, `layers`, `title`,
/// `w`, `h`, `chrome` (`false` for a window with no menu bar and no tools)
/// and `version` -- or the reason it cannot be. `"empty": true` opens the
/// editor with no file, whose window offers adding one.
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
    let serial = facts.serial.unwrap_or(1);
    // What the take is a file of: the user's, or a new one this editor makes
    // in its working folder.
    let made = |what: &str| -> Result<String, String> {
        let folder = workspace
            .as_deref()
            .ok_or("an editor that makes its file needs a working folder")?;
        Ok(format!("{folder}/{serial}-{what}.w64"))
    };
    let decoded = path.is_some() && !reads_in_place(facts.codec.as_deref());
    let (opened, silence) = if empty {
        (None, false)
    } else if decoded {
        // **A compressed file is decoded into the working folder**, and the
        // take is that file: a part reads PCM in place and nothing else, and
        // the file was never this editor's to write over, so its first save
        // asks for a path.
        let into = made("decoded")?;
        let file = FileSource {
            peaks: sources::peaks_beside(&into),
            path: into,
            frames,
            channels,
            rate,
            render: true,
        };
        (Some(file), false)
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
        (Some(file), false)
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
        (Some(file), true)
    } else {
        // nothing named: an open editor over an empty take
        (None, false)
    };
    let mut editor = AudioEditor::new(opened, channels, rate, &names, facts.version.unwrap_or(0))?;
    editor.file.silence = silence;

    editor.file.presentation.layers = stack;
    editor.file.codec = facts.codec.clone();
    // without a stamp, the first look takes the file as it is
    editor.file.disk = path.clone().map(|path| (path, facts.stamp));
    editor.file.sample_format = facts.sample_format.clone();
    if decoded {
        // what the take is now is the decoded copy, not written anywhere the
        // user named: no codec of its own, and the opened file's name
        editor.file.decoded_from = path.clone();
        editor.file.codec = None;
        editor.file.sample_format = Some("float".into());
        editor.file.name = path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .map(|n| n.to_string_lossy().into_owned());
    }
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
///   focused file's shape, cursor, selection, layers and loop switch.
///   With no file open, `binding` and `file` answer `{"empty": true}`, and
///   every verb that acts on a file answers `{"error": "no file is open"}`.
/// - `open_file` -- `paths`, a path or array: an outcome asking the caller to
///   read the files' headers. `add_file` -- a file's shape and its path: add
///   it, in this editor's folder, and return the steps and binding.
/// - `focus_file`, `close_file` -- `index` (zero-based) or `file` (open-object id):
///   restore its state or free it. Closing without either names the focused file.
///   Closing the last one leaves the editor open with none (`"empty": true`,
///   `binding` null); the window is drawn again and offers adding a file.
/// - `show_files`, `hide_files`, `toggle_files` -- the independent left list.
/// - `spectrogram`: the spectrogram's settings. `set_spectrogram`: the
///   settings a request names (`window`, `shape`, `overlap`, `scale`,
///   `tuning`, `colormap`, `range`, `gain`, `interpolate`, `normalize`), as
///   an action.
///   `show_spectrogram_settings`, `hide_spectrogram_settings` and
///   `toggle_spectrogram_settings`: their panel beside the take.
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
/// they have been carried out.
///
/// An unknown verb answers `{}`.
pub fn call_json(editor: &mut AudioEditor, request: &str) -> String {
    match serde_json::from_str::<Value>(request) {
        Ok(request) => call_value(editor, request).to_string(),
        Err(_) => "{}".into(),
    }
}

/// **[`call_json`], over a request already read**: what the editing
/// context calls its audio members with, with no text in between.
pub(crate) fn call_value(editor: &mut AudioEditor, request: Value) -> Value {
    let get = |key: &str| request.get(key).cloned().unwrap_or(Value::Null);
    let widget = int(&get("widget")) as i32;
    let version = int(&get("version"));
    let verb = request
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // **A verb that acts on a file needs one open**; the editor's own -- its
    // window, its file list, adding a file, the conversation -- do not.
    if let Some(error) = refusal(editor, verb) {
        return json!({ "error": error });
    }
    if let Some(turned) = editor.turn_verb(&request, version) {
        return match turned {
            Ok(outcome) => {
                let mut value = serde_json::to_value(outcome).unwrap_or_default();
                editor.turn_extras(verb, &mut value);
                value
            }
            Err(error) => json!({"error": error}),
        };
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
            json!({"corrections": editor.meter_corrections()})
        }
        "window" => {
            editor.window_state.files_widget = request
                .get("filesWidget")
                .and_then(Value::as_i64)
                .map(|n| n as i32);
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
            editor.window(
                widget,
                request
                    .get("meter")
                    .and_then(Value::as_i64)
                    .map(|m| m as i32),
            )
        }
        "attach_view" => {
            editor.window_state.active = true;
            editor.window_state.binding_generation += 1;
            json!({})
        }
        "detach_view" => {
            editor.detach_view();
            json!({})
        }
        // the panel's widgets are numbered with the dialogs'
        "dialogs" => {
            let mut names = dialogs::names();
            names.extend(crate::audio::settings::names());
            json!({"dialogs": names})
        }
        "spectrogram" => json!({"spectrogram": editor.spectrogram()}),
        "folded" => json!({"folded": editor.folded()}),
        "file" if !editor.open_files.open => json!({"empty": true}),
        "file" => editor.file(),
        "files" => {
            editor.refresh_files();
            json!({"files": editor.open_files.rows, "visible": editor.window_state.files_visible})
        }
        // with no file open, a binding says so rather than naming one
        "binding" if !editor.open_files.open => json!({"empty": true}),
        "binding" => editor.binding(),
        "add_file" => {
            match new_json(&editor.with_folder(&request).to_string()).and_then(|mut file| {
                let steps = file.open()?;
                editor.add_file(file);
                Ok(steps)
            }) {
                Ok(steps) => {
                    json!({"turn": "route", "steps": steps, "binding": editor.binding(), "focusChanged": true, "restructure": true,
                "answer": conversation::answer(0, editor.window_state.conversation.applied, None, editor.focus_corrections())})
                }
                Err(error) => json!({"error": error}),
            }
        }
        "playing" => {
            let meters = serde_json::from_value::<Facts>(request.clone())
                .ok()
                .and_then(|facts| facts.meters)
                .map(|meters| meters.map(|m| (m.bus, m.channels)));
            json!({"corrections": editor.playing(request.get("file").and_then(Value::as_i64), meters)})
        }
        "envelope" => json!({"points": editor.envelope_quads(false)}),
        "props" if editor.window_state.envelope_widget == Some(widget) => editor
            .envelope_corrections()
            .first()
            .map_or_else(|| json!({}), |c| c.props.clone()),
        // the picture is told the list again, which it draws from its files,
        // and the band a history step may have moved onto the take
        "props" if editor.open_files.open => {
            let mut props = editor.band_of(widget).unwrap_or_default();
            props.insert("segments".into(), editor.segments());
            Value::Object(props)
        }
        "props" => Value::Object(props(widget)),
        "playback" => editor
            .playback_file()
            .map_or_else(|| json!({"active": false}), |file| json!(file)),
        "parts" => json!({ "parts": editor.list(), "frames": editor.length() }),
        // the list as data, written out: a query, which costs the list
        "segments" => sources::wire_list(
            &editor.list(),
            &editor.file.sources,
            editor.file.channels,
            editor.file.rate,
        ),
        "span" => {
            let span = get("span")
                .as_array()
                .and_then(|r| Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?)));
            let (start, len) = editor.set_span(span);
            json!({ "sel_start": start, "sel_len": len })
        }
        "locate" => {
            json!({ "cue": editor.set_cursor(get("at").as_f64().unwrap_or(0.0)) })
        }
        "play" => json!(editor.space()),
        "looping" => {
            editor.set_looping(get("on").as_bool().unwrap_or(false));
            json!({})
        }
        _ => json!({}),
    }
}

/// Where a file verb finds its file: a row of the list, a file by its
/// number, or the file in focus.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FileAt {
    /// A row of the open files' list, from zero.
    Index(usize),
    /// A file, by its number.
    File(i64),
    /// The file in focus.
    Focused,
}

impl FileAt {
    /// Where `request` (`index` or `file`) names.
    pub(crate) fn of(request: &Value) -> FileAt {
        if let Some(index) = request.get("index").filter(|i| !i.is_null()) {
            return FileAt::Index(index.as_u64().map_or(usize::MAX, |i| i as usize));
        }
        request
            .get("file")
            .and_then(Value::as_i64)
            .map_or(FileAt::Focused, FileAt::File)
    }
}

/// The paths an `open_file` request names: `paths`, a path or an array, or
/// why it names none.
pub(crate) fn open_paths(request: &Value) -> Result<Vec<String>, String> {
    let paths = request.get("paths").or_else(|| request.get("path"));
    match paths {
        Some(Value::String(path)) if !path.is_empty() => Ok(vec![path.clone()]),
        Some(Value::Array(paths))
            if paths
                .iter()
                .all(|p| p.as_str().is_some_and(|p| !p.is_empty())) =>
        {
            Ok(paths
                .iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect())
        }
        _ => Err("open_file needs a path or an array of paths".into()),
    }
}

/// **The verbs that come to an outcome** ([`AudioEditor::turn_verb`]).
const TURN_VERBS: &[&str] = &[
    "layers",
    "show_layer",
    "hide_layer",
    "solo_layer",
    "layer_alpha",
    "view_wave",
    "view_spectrogram",
    "view_both",
    "layer",
    "layer_take",
    "show_spectrogram_settings",
    "hide_spectrogram_settings",
    "toggle_spectrogram_settings",
    "show_files",
    "hide_files",
    "toggle_files",
    "focus_file",
    "close_file",
    "open_file",
    "show_envelope",
    "setautomation",
    "apply_envelope",
    "fold_channel",
    "solo_channel",
    "show_all_channels",
    "set_spectrogram",
    "event",
    "on_disk",
];

/// Whether `verb` comes to an outcome.
pub(crate) fn turns(verb: &str) -> bool {
    TURN_VERBS.contains(&verb)
}

/// **Why `verb` is refused by an editor**, if it is: a verb that acts on a
/// file, with none open.
pub(crate) fn refusal(editor: &AudioEditor, verb: &str) -> Option<&'static str> {
    (!editor.open_files.open && !EMPTY_VERBS.contains(&verb)).then_some(NO_FILE_OPEN)
}

impl AudioEditor {
    /// **A verb that comes to an outcome**, as a script or the window's
    /// list calls it -- a layer or a view, a file focused or closed, files
    /// asked open, a panel shown or put away, an edit of the gain curve or
    /// the channels, a gesture -- or `None` for one that reads or sets
    /// without a turn.
    pub(crate) fn turn_verb(
        &mut self,
        request: &Value,
        version: i64,
    ) -> Option<Result<Outcome, String>> {
        let verb = request
            .get("verb")
            .and_then(Value::as_str)
            .filter(|verb| turns(verb))?;
        let route = |restructure| Outcome {
            turn: Kind::Route,
            restructure,
            ..Outcome::default()
        };
        Some(match verb {
            "layers" | "show_layer" | "hide_layer" | "solo_layer" | "layer_alpha" | "view_wave"
            | "view_spectrogram" | "view_both" => {
                self.layer_verb(verb, request).map(|mut outcome| {
                    outcome.version = version;
                    outcome.answer = Some(conversation::answer(
                        0,
                        version,
                        None,
                        self.layer_corrections(),
                    ));
                    outcome
                })
            }
            "layer" | "layer_take" => {
                let name = if verb == "layer_take" {
                    Some("take")
                } else {
                    request.get("name").and_then(Value::as_str)
                };
                name.map(|name| self.edit_layer(name))
                    .transpose()
                    .map(|corrections| Outcome {
                        answer: Some(conversation::answer(
                            0,
                            version,
                            None,
                            corrections.unwrap_or_default(),
                        )),
                        ..route(false)
                    })
            }
            "show_spectrogram_settings"
            | "hide_spectrogram_settings"
            | "toggle_spectrogram_settings" => {
                self.window_state.settings_visible = match verb {
                    "show_spectrogram_settings" => true,
                    "hide_spectrogram_settings" => false,
                    _ => !self.window_state.settings_visible,
                };
                Ok(route(true))
            }
            "show_files" | "hide_files" | "toggle_files" => {
                self.window_state.files_visible = match verb {
                    "show_files" => true,
                    "hide_files" => false,
                    _ => !self.window_state.files_visible,
                };
                Ok(route(true))
            }
            "focus_file" => self.file_verb(FileVerb::FocusFile, FileAt::of(request)),
            "close_file" => self.file_verb(FileVerb::CloseFile, FileAt::of(request)),
            "open_file" => open_paths(request).map(|paths| Outcome {
                open_file: Some(OpenFiles { paths }),
                ..route(false)
            }),
            "show_envelope" | "setautomation" | "apply_envelope" | "fold_channel"
            | "solo_channel" | "show_all_channels" | "set_spectrogram" => {
                Ok(self.act(request, version))
            }
            "event" => {
                let event = serde_json::from_value::<Event>(request.clone()).unwrap_or_default();
                Ok(self.event(&event, version))
            }
            "on_disk" => Ok(self.disk_answer(
                &request["stamps"],
                request["seq"].as_i64().unwrap_or(0),
                version,
            )),
            _ => return None,
        })
    }

    /// What the JSON door adds to a [`Self::turn_verb`]'s outcome: the
    /// stack a layer verb left, and the layer an edit goes to.
    pub(crate) fn turn_extras(&self, verb: &str, value: &mut Value) {
        match verb {
            "layers" | "show_layer" | "hide_layer" | "solo_layer" | "layer_alpha" | "view_wave"
            | "view_spectrogram" | "view_both" => {
                value["measure"] = json!(self.file.presentation.layers.to_wire());
            }
            "layer" | "layer_take" => value["layer"] = json!(self.file.presentation.edit_layer),
            _ => {}
        }
    }

    /// **A file of the list focused or closed**, `at` where the request
    /// names it. The last file closed leaves the editor open with none:
    /// `binding` is null, `empty` true, and the window is drawn again.
    pub(crate) fn file_verb(&mut self, verb: FileVerb, at: FileAt) -> Result<Outcome, String> {
        self.refresh_files();
        let file = match at {
            FileAt::Index(index) => self
                .open_files
                .rows
                .get(index)
                .and_then(|f| f["id"].as_i64())
                .ok_or("the file index is outside the open files")?,
            FileAt::File(file) => file,
            FileAt::Focused => self.file.id.0,
        };
        let before = self.file.id.0;
        let delete_files = match verb {
            FileVerb::FocusFile => {
                self.focus_file(file)?;
                Vec::new()
            }
            FileVerb::CloseFile => self.close_file(file)?.files,
        };
        let empty = !self.open_files.open;
        let moved = before != self.file.id.0;
        Ok(Outcome {
            turn: Kind::Route,
            delete_files,
            binding: Some(self.binding()),
            focus_changed: moved || empty,
            restructure: true,
            file_action: Some(FileAction { verb, file }),
            empty,
            answer: moved.then(|| {
                conversation::answer(
                    0,
                    self.window_state.conversation.applied,
                    None,
                    self.focus_corrections(),
                )
            }),
            ..Outcome::default()
        })
    }

    /// **What the window's look on disk found**: `stamps`, each open file's,
    /// taken in, and the window's message `seq` answered.
    pub(crate) fn disk_answer(&mut self, stamps: &Value, seq: i64, version: i64) -> Outcome {
        let reason = self.on_disk(stamps);
        Outcome {
            turn: Kind::Route,
            answer: Some(conversation::answer(seq, version, reason, Vec::new())),
            ..Outcome::default()
        }
    }

    /// The span the space bar plays in the file in focus, in seconds, or
    /// `None` with no selection or no file.
    pub(crate) fn span_seconds(&self) -> Option<(f64, f64)> {
        let (start, end) = self.file.presentation.selection?;
        (self.open_files.open && end > start)
            .then(|| (start as f64 / self.file.rate, end as f64 / self.file.rate))
    }

    /// The file in focus's loop switch.
    pub(crate) fn looping(&self) -> bool {
        self.open_files.open && self.file.presentation.looping
    }

    /// The file in focus's loop switch, as a script sets it.
    pub(crate) fn set_looping(&mut self, on: bool) {
        self.file.presentation.looping = on;
    }

    /// **Where the playback's meter writes**, as it says once it has its
    /// buses: the window's meter corrected to read there.
    pub(crate) fn sync_meters(&mut self, meters: Option<(i32, usize)>) -> Vec<Correction> {
        self.window_state.meters = meters;
        self.meter_corrections()
    }

    /// **What plays now**: `file` (or none), its meter `meters` when the
    /// playback said; the list's play mark, the play cursor and the meter
    /// corrected to it.
    pub(crate) fn playing(
        &mut self,
        file: Option<i64>,
        meters: Option<Option<(i32, usize)>>,
    ) -> Vec<Correction> {
        self.window_state.playing = file;
        if let Some(meters) = meters {
            self.set_meters(meters);
        }
        let mut corrections = self.files_corrections();
        // with no file open the window draws no take, pane or meter
        if !self.open_files.open {
            return corrections;
        }
        let anchor = self.playhead_anchor();
        let targets: BTreeSet<_> = self.pictures().chain(self.window_state.pane).collect();
        corrections.extend(targets.into_iter().map(|widget| Correction {
            widget: i64::from(widget),
            props: json!({"playhead_at": anchor}),
        }));
        corrections.extend(self.meter_corrections());
        corrections
    }

    /// The window is gone: the editor draws nowhere.
    pub(crate) fn detach_view(&mut self) {
        self.window_state.active = false;
        self.window_state.window = None;
        self.window_state.binding_generation += 1;
    }

    /// **What of `files` is this editor's scratch**: the ones in its
    /// working folder, which a failed render may take away; nothing else, a
    /// file the user named least of all.
    pub(crate) fn scratch(&self, files: &[String]) -> Vec<String> {
        let folder = self
            .window_state
            .workspace
            .as_deref()
            .map(|w| format!("{}/", w.trim_end_matches('/')));
        files
            .iter()
            .filter(|f| {
                folder
                    .as_deref()
                    .is_some_and(|w| f.starts_with(w) && !f[w.len()..].contains(".."))
            })
            .cloned()
            .collect()
    }
}
