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
    let (opened, silence) = if empty {
        (None, false)
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
///   `tuning`, `colormap`, `normalize`), as an action.
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
    if !editor.open_files.open && !EMPTY_VERBS.contains(&verb) {
        return json!({ "error": NO_FILE_OPEN });
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
                    value
                }
                Err(error) => json!({"error": error}),
            }
        }
        verb @ ("layer" | "layer_take") => {
            let name = if verb == "layer_take" {
                Some("take")
            } else {
                request.get("name").and_then(Value::as_str)
            };
            match name.map(|name| editor.edit_layer(name)).transpose() {
                Ok(corrections) => {
                    json!({"turn": "route", "layer": editor.file.presentation.edit_layer,
                    "answer": conversation::answer(0, version, None, corrections.unwrap_or_default())})
                }
                Err(error) => json!({"error": error}),
            }
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
            editor.window_state.active = false;
            editor.window_state.window = None;
            editor.window_state.binding_generation += 1;
            json!({})
        }
        // the panel's widgets are numbered with the dialogs'
        "dialogs" => {
            let mut names = dialogs::names();
            names.extend(crate::audio::settings::names());
            json!({"dialogs": names})
        }
        "spectrogram" => json!({"spectrogram": editor.spectrogram()}),
        "show_spectrogram_settings"
        | "hide_spectrogram_settings"
        | "toggle_spectrogram_settings" => {
            editor.window_state.settings_visible = match request["verb"].as_str().unwrap() {
                "show_spectrogram_settings" => true,
                "hide_spectrogram_settings" => false,
                _ => !editor.window_state.settings_visible,
            };
            json!({"turn": "route", "restructure": true})
        }
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
        "focus_file" | "close_file" => {
            let verb = request["verb"].as_str().unwrap();
            editor.refresh_files();
            let file = if let Some(index) = request.get("index") {
                match index
                    .as_u64()
                    .and_then(|i| editor.open_files.rows.get(i as usize))
                    .and_then(|f| f["id"].as_i64())
                {
                    Some(id) => id,
                    None => return json!({"error": "the file index is outside the open files"}),
                }
            } else {
                request
                    .get("file")
                    .and_then(Value::as_i64)
                    .unwrap_or(editor.file.id.0)
            };
            let before = editor.file.id.0;
            let result = if verb == "focus_file" {
                editor.focus_file(file).map(|()| Vec::<String>::new())
            } else {
                editor.close_file(file).map(|closed| closed.files)
            };
            match result {
                // The last file closed leaves the editor open with none:
                // `binding` is null and the window is drawn again empty.
                Ok(files) => {
                    let empty = !editor.open_files.open;
                    let moved = before != editor.file.id.0;
                    json!({"turn": "route", "deleteFiles": files, "binding": editor.binding(), "focusChanged": moved || empty,
                    "restructure": true, "fileAction": {"verb": verb, "file": file}, "empty": empty,
                    "answer": moved.then(|| conversation::answer(0, editor.window_state.conversation.applied, None, editor.focus_corrections()))})
                }
                Err(error) => json!({"error": error}),
            }
        }
        "open_file" => {
            let paths = request.get("paths").or_else(|| request.get("path"));
            let paths = match paths {
                Some(Value::String(path)) if !path.is_empty() => vec![json!(path)],
                Some(Value::Array(paths))
                    if paths
                        .iter()
                        .all(|p| p.as_str().is_some_and(|p| !p.is_empty())) =>
                {
                    paths.clone()
                }
                _ => return json!({"error": "open_file needs a path or an array of paths"}),
            };
            json!({"turn": "route", "openFile": {"paths": paths}})
        }
        "playing" => {
            editor.window_state.playing = request.get("file").and_then(Value::as_i64);
            if let Ok(facts) = serde_json::from_value::<Facts>(request.clone())
                && let Some(meters) = facts.meters
            {
                editor.set_meters(meters.map(|m| (m.bus, m.channels)));
            }
            let mut corrections = editor.files_corrections();
            // with no file open the window draws no take, pane or meter
            if !editor.open_files.open {
                return json!({"corrections": corrections});
            }
            let anchor = editor.playhead_anchor();
            let targets: BTreeSet<_> = editor.pictures().chain(editor.window_state.pane).collect();
            corrections.extend(targets.into_iter().map(|widget| Correction {
                widget: i64::from(widget),
                props: json!({"playhead_at": anchor}),
            }));
            corrections.extend(editor.meter_corrections());
            json!({"corrections": corrections})
        }
        "show_files" | "hide_files" | "toggle_files" => {
            editor.window_state.files_visible = match request["verb"].as_str().unwrap() {
                "show_files" => true,
                "hide_files" => false,
                _ => !editor.window_state.files_visible,
            };
            json!({"turn": "route", "restructure": true})
        }
        "envelope" => json!({"points": editor.envelope_quads(false)}),
        "show_envelope" | "setautomation" | "apply_envelope" | "fold_channel" | "solo_channel"
        | "show_all_channels" | "set_spectrogram" => {
            serde_json::to_value(editor.act(&request, version)).unwrap_or_default()
        }
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
        "event" => {
            let event = serde_json::from_value::<Event>(request.clone()).unwrap_or_default();
            serde_json::to_value(editor.event(&event, version)).unwrap_or_default()
        }
        "playback" => editor.playback_file(),
        "parts" => json!({ "parts": editor.list(), "frames": editor.length() }),
        // the list as data, written out: a query, which costs the list
        "segments" => sources::wire_list(
            &editor.list(),
            &editor.file.sources,
            editor.file.channels,
            editor.file.rate,
        ),
        // **What of `files` is this editor's scratch**: the ones in its
        // working folder, which a failed render may take away; nothing else,
        // a file the user named least of all.
        "scratch" => {
            let folder = editor
                .window_state
                .workspace
                .as_deref()
                .map(|w| format!("{}/", w.trim_end_matches('/')));
            let files: Vec<&str> = request["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|f| {
                    folder
                        .as_deref()
                        .is_some_and(|w| f.starts_with(w) && !f[w.len()..].contains(".."))
                })
                .collect();
            json!({"files": files})
        }
        "on_disk" => {
            let reason = editor.on_disk(&request["stamps"]);
            let seq = request["seq"].as_i64().unwrap_or(0);
            json!({"turn": "route",
                "answer": conversation::answer(seq, version, reason, Vec::new())})
        }
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
            editor.file.presentation.looping = get("on").as_bool().unwrap_or(false);
            json!({})
        }
        _ => json!({}),
    }
}
