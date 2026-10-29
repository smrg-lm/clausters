//! The file the take is: the new-file, open and save-as dialogs, and a file not
//! written yet, which asks for its path the first time it is saved.

use super::*;
use crate::audio::dialogs::{self, Form};
use crate::forms::{self, Said};

/// Props for widgets, as the corrections a turn answers with.
fn corrected(list: Vec<(i32, Value)>) -> Vec<Correction> {
    list.into_iter()
        .map(|(widget, props)| Correction {
            widget: i64::from(widget),
            props,
        })
        .collect()
}

/// How wide the list's column for the playing mark is.
const FILE_MARK_W: f64 = 18.0;

impl AudioEditor {
    /// **Whether a file is open**: an editor may hold none, before its first
    /// file is added and after its last one is closed.
    pub fn has_file(&self) -> bool {
        self.open_files.open
    }

    /// **Every file it holds, let go of**: their identities, for the
    /// histories to close; from here on it roots none of them, so what only
    /// it named can be released.
    pub(crate) fn let_go_files(&mut self) -> Vec<i64> {
        if !self.open_files.open {
            return Vec::new();
        }
        self.open_files.open = false;
        std::iter::once(self.file.id.0)
            .chain(self.open_files.parked.keys().map(|id| id.0))
            .collect()
    }

    /// **An editor with no file**, holding nothing.
    pub(crate) fn empty() -> AudioEditor {
        new_json(r#"{"empty": true}"#).expect("an empty editor")
    }

    /// The focused open file's identity.
    pub fn focused_file(&self) -> i64 {
        self.file.id.0
    }

    /// Apply object work without changing focus, bindings or conversation state.
    pub(crate) fn over_file<T>(
        &mut self,
        id: i64,
        apply: impl FnOnce(&mut Self) -> T,
    ) -> Option<T> {
        if !self.open_files.open {
            return None;
        }
        if self.file.id.0 == id {
            return Some(apply(self));
        }
        let next = self.open_files.parked.remove(&FileId(id))?;
        let focused = std::mem::replace(&mut self.file, next);
        let answer = apply(self);
        let updated = std::mem::replace(&mut self.file, focused);
        self.open_files.parked.insert(updated.id, updated);
        self.refresh_files();
        Some(answer)
    }

    /// **Each open file's take, as an engine the application holds reads
    /// it**: its parts at `rate`, the engine's, and the revision they are of
    /// -- of the files `wanted` names by id and revision, so a file the
    /// engine reads as it is now costs nothing to look at.
    pub(crate) fn held_takes(
        &self,
        rate: f64,
        wanted: impl Fn(u64, u64) -> bool,
    ) -> Vec<crate::editing::held::HeldTake> {
        if !self.open_files.open {
            return Vec::new();
        }
        std::iter::once(&self.file)
            .chain(self.open_files.parked.values())
            .filter(|file| wanted(file.id.0 as u64, file.revision))
            .map(|file| file.held_take(rate))
            .collect()
    }

    /// The open file `file`, focused or parked.
    fn open_file_state(&self, file: u64) -> Option<&AudioFileState> {
        if !self.open_files.open {
            return None;
        }
        let id = FileId(i64::try_from(file).ok()?);
        if self.file.id == id {
            return Some(&self.file);
        }
        self.open_files.parked.get(&id)
    }

    /// How many frames the open file `file` comes to, at its own rate.
    pub(crate) fn file_length(&self, file: u64) -> Option<u64> {
        self.open_file_state(file).map(AudioFileState::length)
    }

    /// The rate of the open file `file`.
    pub(crate) fn file_rate(&self, file: u64) -> Option<f64> {
        self.open_file_state(file).map(|f| f.rate)
    }

    /// The file in focus as its playback sounds it; `None` with no file
    /// open, which has nothing to sound.
    pub(crate) fn playback_file(&self) -> Option<PlaybackFile> {
        self.open_files.open.then(|| PlaybackFile {
            id: self.file.id.0,
            channels: self.file.channels,
            frames: self.length(),
            rate: self.file.rate,
            active: self.window_state.active,
        })
    }

    /// The file in focus as a client binds it, or `null` with no file open.
    pub(super) fn binding(&self) -> Value {
        if !self.open_files.open {
            return Value::Null;
        }
        let (start, end) = self.file.presentation.selection.unwrap_or((0, 0));
        json!({"id": self.file.id, "generation": self.window_state.binding_generation,
            "take": self.file.opened, "channels": self.file.channels,
            "rate": self.file.rate, "frames": self.length(), "cursor": self.file.presentation.cursor.unwrap_or(0),
            "span": if end > start { json!([start as f64 / self.file.rate, end as f64 / self.file.rate]) } else { Value::Null },
            "file": self.file(), "layers": self.layers(), "looping": self.file.presentation.looping, "dirty": self.file.dirty})
    }

    pub(crate) fn file_at(&self, index: usize) -> Option<i64> {
        self.open_files.rows.get(index)?.get("id")?.as_i64()
    }

    pub(super) fn refresh_files(&mut self) {
        if !self.open_files.open {
            return;
        }
        if self.open_files.rows.is_empty() {
            self.open_files.rows.push(self.file.description());
        }
        for row in &mut self.open_files.rows {
            if row["id"].as_i64() == Some(self.file.id.0) {
                *row = self.file.description();
            } else if let Some(file) = row["id"]
                .as_i64()
                .and_then(|id| self.open_files.parked.get(&FileId(id)))
            {
                *row = file.description();
            }
        }
    }

    pub(super) fn add_file(&mut self, file: AudioEditor) {
        self.refresh_files();
        self.add_file_state(file.file, true);
    }

    pub(super) fn add_file_state(&mut self, mut next: AudioFileState, focus: bool) {
        self.refresh_files();
        next.id = FileId(self.open_files.next_id);
        self.open_files.next_id += 1;
        if !focus && self.open_files.open {
            self.open_files.rows.push(next.description());
            self.open_files.parked.insert(next.id, next);
            self.refresh_files();
            return;
        }
        let was = std::mem::replace(&mut self.file, next);
        if self.open_files.open {
            self.open_files.parked.insert(was.id, was);
        }
        self.open_files.open = true;
        self.open_files.rows.push(self.file.description());
        self.window_state.binding_generation += 1;
        self.refresh_files();
    }

    pub(super) fn focus_file(&mut self, id: i64) -> Result<(), String> {
        if self.open_files.open && id == self.file.id.0 {
            return Ok(());
        }
        let Some(next) = self.open_files.parked.remove(&FileId(id)) else {
            return Err("the file is not open in this editor".into());
        };
        let was = std::mem::replace(&mut self.file, next);
        self.open_files.parked.insert(was.id, was);
        self.window_state.binding_generation += 1;
        self.refresh_files();
        Ok(())
    }

    pub(super) fn close_file(&mut self, id: i64) -> Result<Closed, String> {
        self.refresh_files();
        if !self
            .open_files
            .rows
            .iter()
            .any(|f| f["id"].as_i64() == Some(id))
        {
            return Err("the file is not open in this editor".into());
        }
        if id == self.file.id.0 {
            if let Some(next) = self
                .open_files
                .rows
                .iter()
                .filter_map(|f| f["id"].as_i64())
                .find(|&i| i != id)
            {
                self.focus_file(next)?;
            } else {
                self.open_files.rows.clear();
                self.open_files.open = false;
                self.window_state.binding_generation += 1;
                return Ok(self.file.closed());
            }
        }
        let file = self.open_files.parked.remove(&FileId(id)).unwrap();
        self.open_files
            .rows
            .retain(|f| f["id"].as_i64() != Some(id));
        Ok(file.closed())
    }

    /// **What the window is corrected with when this file comes into
    /// focus**: its cursor, its selection and its loop switch on every view
    /// of the take. They are the host's screen state, which a definition does
    /// not overwrite, so they travel as an answer, which also hands the
    /// host the version the switch moved to.
    pub(super) fn focus_corrections(&self) -> Vec<Correction> {
        let mut corrections = self.band_corrections();
        for correction in &mut corrections {
            correction.props["looping"] = json!(self.file.presentation.looping);
            let (start, len) = self.file.presentation.viewport.unwrap_or((0.0, 0.0));
            correction.props["view_start"] = json!(start);
            correction.props["view_len"] = json!(len);
            let (start, len) = self
                .file
                .presentation
                .vertical
                .get(self.vertical_measure(correction.widget))
                .copied()
                .unwrap_or((0.0, 1.0));
            correction.props["y_start"] = json!(start);
            correction.props["y_len"] = json!(len);
        }
        corrections
    }

    pub(super) fn vertical_measure(&self, widget: i64) -> &'static str {
        if self.window_state.envelope_widget.map(i64::from) == Some(widget) {
            "gain"
        } else if self.window_state.lane.map(i64::from) == Some(widget)
            || self.file.presentation.views == Views::Spectrogram
        {
            "frequency"
        } else {
            "amplitude"
        }
    }

    /// The cursor and the selection, as every view of the take is corrected
    /// with them.
    pub(super) fn band_corrections(&self) -> Vec<Correction> {
        let (start, end) = self.file.presentation.selection.unwrap_or((0, 0));
        let cursor = self.file.presentation.cursor.unwrap_or(0);
        [
            self.window_state.widget,
            self.window_state.pane,
            self.window_state
                .lane
                .filter(|_| self.file.presentation.views == Views::Stacked),
        ]
        .into_iter()
        .flatten()
        .map(|id| Correction {
            widget: i64::from(id),
            props: json!({
                "cursor": cursor,
                "sel_start": if end > start { start } else { cursor },
                "sel_len": end.saturating_sub(start),
            }),
        })
        .collect()
    }

    /// **An edit landed at frame `at`**: nothing is selected, the position
    /// cursor stands there and the playback is cued to it. Answers the
    /// corrections that tell the window.
    /// **What the hand placed stays on the take**: an undo that shortens the
    /// take under the position cursor or a selection puts them at its end,
    /// where a paste at a cursor left past it used to land at the end while
    /// the view it opened was measured from past it.
    pub(super) fn keep_on_take(&mut self) {
        let length = self.length();
        let presentation = &mut self.file.presentation;
        presentation.cursor = presentation.cursor.map(|c| c.min(length));
        presentation.selection = presentation
            .selection
            .map(|(a, b)| (a.min(length), b.min(length)))
            .filter(|(a, b)| b > a);
    }

    /// The band props of take widget `widget`, if it is one a band is set
    /// on: its cursor and its selection as the editor holds them.
    pub(super) fn band_of(&self, widget: i32) -> Option<serde_json::Map<String, Value>> {
        self.band_corrections()
            .into_iter()
            .find(|c| c.widget == i64::from(widget))
            .and_then(|c| match c.props {
                Value::Object(props) => Some(props),
                _ => None,
            })
    }

    pub(super) fn land_at(&mut self, at: u64, out: &mut Outcome) -> Vec<Correction> {
        let at = at.min(self.length());
        self.file.presentation.selection = None;
        self.file.presentation.cursor = Some(at);
        let secs = self.secs_at(at as f64);
        out.selection = Some(json!({ "start": secs, "len": 0.0 }));
        out.locate = Some(secs);
        out.cue = Some(at);
        out.follow = self.follow(false);
        self.band_corrections()
    }

    /// **The list's rows and the row in focus**: each open file's name, the
    /// one a pass is playing marked with the transport's play sign.
    fn file_rows(&self) -> (Vec<Value>, Vec<usize>) {
        let selected = self
            .open_files
            .rows
            .iter()
            .position(|f| f["id"].as_i64() == Some(self.file.id.0))
            .into_iter()
            .collect();
        let rows = self
            .open_files
            .rows
            .iter()
            .map(|f| {
                let mark = if f["id"].as_i64().is_some()
                    && f["id"].as_i64() == self.window_state.playing
                {
                    crate::chrome::PLAY
                } else {
                    ""
                };
                json!([mark, f["label"]])
            })
            .collect();
        (rows, selected)
    }

    /// What the list is corrected with when the file a pass plays changes.
    pub(super) fn files_corrections(&mut self) -> Vec<Correction> {
        self.refresh_files();
        let (Some(widget), true) = (
            self.window_state.files_widget,
            self.window_state.files_visible,
        ) else {
            return Vec::new();
        };
        let (rows, selected) = self.file_rows();
        vec![Correction {
            widget: i64::from(widget),
            props: json!({"rows": rows, "selected": selected}),
        }]
    }

    pub(super) fn compose_files(&self, tree: &mut Value) {
        let Some(widget) = self.window_state.files_widget else {
            return;
        };
        if !self.window_state.files_visible {
            return;
        }
        let (rows, selected) = self.file_rows();
        // Chrome and dialogs stay outside the editing row. The independent
        // list is a table; its selection is the application's focused file.
        let Some(children) = tree.get_mut("children").and_then(Value::as_array_mut) else {
            return;
        };
        let index = children
            .iter()
            .position(|c| c["weight"].as_f64().is_some_and(|w| w > 0.0));
        if let Some(index) = index {
            let picture = children.remove(index);
            children.insert(
                index,
                json!({"type": "layout", "flow": "row", "margin": 0, "weight": 1,
                "children": [{"type": "table", "id": widget, "name": "files", "w": 180,
                    "columns": [{"title": "", "w": FILE_MARK_W}, "Files"],
                    "rows": rows, "selected": selected, "multiple": false}, picture]}),
            );
        }
    }
    /// **Whether the take is a file not written yet**: a save has nowhere to
    /// go until a path is given.
    pub fn untitled(&self) -> bool {
        self.file.untitled()
    }

    /// **What the take is called** in its pane: the name the caller gave it,
    /// else its file's name, `untitled` for a file not written yet, and none
    /// for a take that is a buffer.
    pub(super) fn take_name(&self) -> Option<String> {
        self.file.take_name()
    }

    /// The name of the dialog widget `widget` is, when it is one.
    pub(super) fn dialog_name(&self, widget: i64) -> Option<String> {
        self.window_state.dialogs.name(widget).map(str::to_string)
    }

    /// Whether the window numbered the widgets of its dialogs.
    fn has_dialogs(&self) -> bool {
        self.window_state.dialogs.numbered(dialogs::forms())
    }

    /// **Opens `form` over the window**, its fields holding what the editor
    /// has: the shape of the file in the window, or its path. The open form's
    /// chooser opens where the hand last left it.
    pub(super) fn open_form(&mut self, form: Form) -> (Option<String>, Vec<Correction>) {
        if !self.has_dialogs() {
            return (Some("this window has no dialogs".into()), Vec::new());
        }
        let values = match form {
            Form::New => dialogs::new_of(self.file.channels, self.file.rate),
            Form::SaveAs => [("path".to_string(), self.file.target.clone())].into(),
            Form::Open => BTreeMap::new(),
        };
        (
            None,
            corrected(self.window_state.dialogs.open(form, values)),
        )
    }

    pub(crate) fn saves_from(&self, widget: i64, tag: &str, values: &[Value]) -> bool {
        if tag == "save" {
            return !(self.untitled() && self.has_dialogs());
        }
        let Some(open) = self
            .window_state
            .dialogs
            .up()
            .filter(|open| open.form == Form::SaveAs)
        else {
            return false;
        };
        self.dialog_name(widget).is_some_and(|name| {
            matches!(
                forms::read(open.form, &name, tag, values),
                Some(Said::Accept)
            )
        })
    }

    /// **A save from the window**: over what the take was opened from, or --
    /// for a file not written yet, in a window with dialogs -- the save-as
    /// form, which asks where.
    pub(super) fn save_from_window(
        &mut self,
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if self.untitled() && self.has_dialogs() {
            return self.open_form(Form::SaveAs);
        }
        match self.save_target(None, "float") {
            Ok(_) => {
                out.save_requested = true;
                (None, Vec::new())
            }
            Err(why) => (Some(why), Vec::new()),
        }
    }

    /// **What a widget of the dialogs said**: a field typed into, a chooser's
    /// selection or the directory it went into, a form cancelled, or `OK` --
    /// a new file's shape or files to open, which the outcome names for the
    /// client to open, or a path the take is saved under from now on. A pick
    /// in the chooser (a double click or Enter on a file) opens what is
    /// selected as `OK` does.
    pub(super) fn form_event(
        &mut self,
        name: &str,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        let Some(said) = self.window_state.dialogs.read(name, tag, values) else {
            return (None, Vec::new());
        };
        let Some(open) = self.window_state.dialogs.up().cloned() else {
            return (None, Vec::new());
        };
        let reason = match said {
            Said::Field(..) | Said::Chosen(..) | Said::Entered(..) => return (None, Vec::new()),
            Said::Cancel | Said::Discard => {
                return (None, corrected(self.window_state.dialogs.hide()));
            }
            Said::Picked(_, paths) if open.form == Form::Open => open_paths(&paths, out),
            Said::Picked(..) => return (None, Vec::new()),
            Said::Accept => match open.form {
                Form::New => match dialogs::new_shape(&open) {
                    Ok((channels, frames, rate)) => {
                        out.new_file = Some(Shape {
                            channels,
                            frames,
                            rate,
                        });
                        None
                    }
                    Err(why) => Some(why),
                },
                Form::Open => {
                    let paths = self.window_state.dialogs.chosen("path").to_vec();
                    open_paths(&paths, out)
                }
                Form::SaveAs => {
                    let path = open.value("path").trim().to_string();
                    if path.is_empty() {
                        Some("a file is saved under a path".into())
                    } else {
                        match self.save_target(Some(path), "float") {
                            Ok(target) => {
                                // the operation the request becomes writes here
                                self.file.target = target;
                                out.save_requested = true;
                                None
                            }
                            Err(why) => Some(why),
                        }
                    }
                }
            },
        };
        // a refused form stays up, holding what was typed and chosen
        if reason.is_some() {
            return (reason, Vec::new());
        }
        let mut corrections = corrected(self.window_state.dialogs.hide());
        // a file saved under a new path is called by it from now on
        if let (Form::SaveAs, Some(pane)) = (open.form, self.window_state.pane) {
            corrections.push(Correction {
                widget: i64::from(pane),
                props: json!({"label": self.pane_label()}),
            });
        }
        (None, corrections)
    }

    /// **The file the take is**, as a client reads it: its path (`null` for
    /// one not written yet, or a take saved into a buffer), its shape and its
    /// rate.
    pub fn file(&self) -> Value {
        if !self.open_files.open {
            return Value::Null;
        }
        self.file.file()
    }
}

/// **What closing a file lets go of**: the steps that free the join it
/// played through, that buffer, and the renders to delete.
pub(crate) struct Closed {
    pub files: Vec<String>,
}

/// **The application's decision over chosen paths**: they are files for the
/// client to open, as it opens a new one -- reading them is the client's,
/// through `SoundFile` -- or, with none chosen, the reason the form stays up.
fn open_paths(paths: &[String], out: &mut Outcome) -> Option<String> {
    if paths.is_empty() {
        return Some("select one or more files to open".into());
    }
    out.open_file = Some(OpenFiles {
        paths: paths.to_vec(),
    });
    None
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_file_list_is_an_independent_hideable_widget_on_the_left() {
        let mut editor =
            super::super::new_json(r#"{"path":"/in/take1.wav","workspace":"/work","frames":1000}"#)
                .unwrap();
        editor.window_state.bare = true;
        editor.window_state.files_widget = Some(20);
        let tree = editor.window(10, None);
        assert_eq!(tree["children"][0]["flow"], "row");
        assert_eq!(tree["children"][0]["children"][0]["id"], 20);
        assert_eq!(tree["children"][0]["children"][0]["type"], "table");
        assert_eq!(tree["children"][0]["children"][1]["id"], 10);
        editor.window_state.files_visible = false;
        assert_eq!(editor.window(10, None)["children"][0]["id"], 10);
    }

    /// **The list marks the file a pass plays**, which need not be the one in
    /// focus, and keeps the row in focus selected.
    #[test]
    fn the_list_marks_the_file_that_plays() {
        let mut editor =
            super::super::new_json(r#"{"path":"/in/take1.wav","workspace":"/work","frames":1000}"#)
                .unwrap();
        editor.window_state.files_widget = Some(20);
        let answer: serde_json::Value = serde_json::from_str(&super::super::call_json(
            &mut editor,
            r#"{"verb":"playing","file":1}"#,
        ))
        .unwrap();
        let props = &answer["corrections"][0]["props"];
        assert_eq!(answer["corrections"][0]["widget"], 20);
        assert_eq!(props["rows"][0][0], crate::chrome::PLAY);
        assert_eq!(props["selected"], serde_json::json!([0]));
        let stopped: serde_json::Value = serde_json::from_str(&super::super::call_json(
            &mut editor,
            r#"{"verb":"playing","file":null}"#,
        ))
        .unwrap();
        assert_eq!(stopped["corrections"][0]["props"]["rows"][0][0], "");
    }
}

impl AudioFileState {
    /// **What closing this file lets go of**: the renders its list was made
    /// of, to delete -- the history that could walk back to them goes with
    /// the file.
    fn closed(&self) -> Closed {
        // its renders, and the peaks the editor measured of a file it opened
        // with the spectral summary beside them
        let files = self
            .sources
            .values()
            .flat_map(|s| {
                let measured = s.peaks != sources::peaks_beside(&s.path);
                let path = s.render.then(|| s.path.clone());
                let peaks = (s.render || measured)
                    .then(|| [s.peaks.clone(), sources::spectra_beside(&s.peaks)]);
                path.into_iter().chain(peaks.into_iter().flatten())
            })
            .collect();
        Closed { files }
    }
    pub(super) fn take_name(&self) -> Option<String> {
        if let Some(name) = self.name.as_ref().filter(|n| !n.is_empty()) {
            return Some(name.clone());
        }
        if self.target.is_empty() {
            return Some("untitled".into());
        }
        std::path::Path::new(&self.target)
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
    }
    pub fn file(&self) -> Value {
        let path = if self.target.is_empty() {
            Value::Null
        } else {
            json!(self.target)
        };
        json!({
            "path": path,
            "frames": self.length(),
            "channels": self.channels,
            "rate": self.rate,
            "format": self.written_format,
            "codec": if self.written_format.is_some() { Some("wav") } else { self.codec.as_deref() },
            "sample_format": self.written_format.as_ref().or(self.sample_format.as_ref()),
            "untitled": self.untitled(),
        })
    }
    pub fn untitled(&self) -> bool {
        self.target.is_empty()
    }
    fn held_take(&self, rate: f64) -> crate::editing::held::HeldTake {
        let rate = if rate > 0.0 { rate } else { self.rate };
        let list = sources::file_list(
            &self.contents.parts,
            &self.sources,
            self.channels,
            self.rate,
        );
        crate::editing::held::HeldTake {
            reader: self.id.0 as u64,
            revision: self.revision,
            list: crate::editing::held::HeldList::Parts(list),
            channels: self.channels,
            rate,
        }
    }
    fn length(&self) -> u64 {
        self.contents.parts.frames()
    }
    fn description(&self) -> Value {
        json!({"id": self.id, "label": crate::samples::label(self.take_name().as_deref()), "file": self.file()})
    }
    /// The files this one cannot let go of: the one it was opened from, and
    /// every file its list reads now.
    pub(super) fn rooted(&self) -> Vec<SourceId> {
        let mut roots = self.contents.parts.sources().to_vec();
        if let Some(opened) = self.opened
            && !roots.contains(&opened)
        {
            roots.push(opened);
        }
        roots
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    fn call(editor: &mut AudioEditor, request: Value) -> Value {
        serde_json::from_str(&call_json(editor, &request.to_string())).unwrap()
    }

    #[test]
    fn file_navigation_restores_the_observed_viewport_and_selection() {
        let mut editor =
            new_json(r#"{"path":"/in/take3.wav","workspace":"/work","frames":10000}"#).unwrap();
        editor.window(10, None);
        for (tag, values) in [
            ("view", vec![json!(1200), json!(2000)]),
            ("view_y", vec![json!(0.25), json!(0.5)]),
            ("locate", vec![json!(100)]),
            ("selection", vec![json!(300), json!(50)]),
        ] {
            let mut args = vec![json!(10), json!(1), json!(0), json!(tag)];
            args.extend(values);
            editor.event(
                &Event {
                    addr: "/gui_event".into(),
                    args,
                },
                0,
            );
        }
        editor.add_file(
            new_json(r#"{"path":"/in/take50.wav","workspace":"/work","frames":20000}"#).unwrap(),
        );
        let fresh = editor.focus_corrections();
        assert_eq!(
            fresh[0].props["view_len"], 0.0,
            "a new file fits its own extent"
        );
        assert_eq!(fresh[0].props["sel_len"], 0);
        assert_eq!(fresh[0].props["y_start"], 0.0);
        assert_eq!(fresh[0].props["y_len"], 1.0);
        editor.focus_file(1).unwrap();
        let restored = editor.focus_corrections();
        assert_eq!(restored[0].props["view_start"], 1200.0);
        assert_eq!(restored[0].props["view_len"], 2000.0);
        assert_eq!(restored[0].props["y_start"], 0.25);
        assert_eq!(restored[0].props["y_len"], 0.5);
        assert_eq!(restored[0].props["cursor"], 100);
        assert_eq!(restored[0].props["sel_start"], 300);
        assert_eq!(restored[0].props["sel_len"], 50);
    }

    #[test]
    fn focus_rebinds_file_state_without_copying_the_window_or_its_conversation() {
        let mut editor =
            new_json(r#"{"path":"/in/take3.wav","workspace":"/work","frames":100}"#).unwrap();
        editor.window_state.files_widget = Some(40);
        editor.window_state.pane = Some(11);
        editor.window_state.lane = Some(13);
        editor.window_state.conversation = Conversation::new(17);
        editor.window_state.workspace = Some("/work".into());
        call(
            &mut editor,
            json!({"verb":"window", "widget":10, "pane":11, "lane":13, "meter":12, "filesWidget":40}),
        );
        editor.file.presentation.cursor = Some(31);
        editor.file.presentation.selection = Some((10, 40));
        editor.file.presentation.folded.insert(0);
        editor.file.presentation.views = Views::Stacked;
        let contents = editor.file.contents.clone();
        let first = editor.focused_file();
        editor.add_file(
            new_json(r#"{"path":"/in/take50.wav","workspace":"/work","frames":200,"channels":6}"#)
                .unwrap(),
        );
        let second = editor.focused_file();
        assert_ne!(first, second);
        assert_eq!(editor.window_state.conversation.applied, 17);
        assert_eq!(editor.window_state.files_widget, Some(40));
        assert_eq!(editor.window_state.workspace.as_deref(), Some("/work"));
        assert_eq!(editor.window_state.binding_generation, 1);
        editor.focus_file(first).unwrap();
        assert_eq!(editor.file.contents, contents);
        assert_eq!(editor.file.presentation.cursor, Some(31));
        assert_eq!(editor.file.presentation.selection, Some((10, 40)));
        assert!(editor.file.presentation.folded.contains(&0));
        assert_eq!(editor.file.presentation.views, Views::Stacked);
        assert_eq!(editor.window_state.binding_generation, 2);
        editor.window(10, Some(12));
        assert_eq!(
            editor.window_state.binding_generation, 2,
            "redraw retains the binding"
        );
        editor.focus_file(second).unwrap();
        let third_generation = editor.window_state.binding_generation;
        editor.close_file(first).unwrap();
        assert_eq!(editor.window_state.binding_generation, third_generation);
        assert!(editor.focus_file(first).is_err());
    }

    #[test]
    fn redraw_keeps_the_playhead_bound_to_the_playing_file() {
        fn anchor(tree: &Value, widget: i64) -> Option<f64> {
            if tree["id"] == widget {
                return tree["axes"]["x"]["playhead_at"].as_f64();
            }
            tree["children"]
                .as_array()?
                .iter()
                .find_map(|child| anchor(child, widget))
        }
        let mut editor =
            new_json(r#"{"path":"/in/take3.wav","workspace":"/work","frames":100}"#).unwrap();
        call(
            &mut editor,
            json!({"verb":"window", "widget":10, "pane":11, "lane":13}),
        );
        let first = editor.focused_file();
        editor.add_file(
            new_json(r#"{"path":"/in/take50.wav","workspace":"/work","frames":200}"#).unwrap(),
        );
        let second = editor.focused_file();
        call(&mut editor, json!({"verb":"playing", "file":first}));
        for (file, expected) in [(second, -1.0), (first, 0.0), (second, -1.0)] {
            editor.focus_file(file).unwrap();
            for views in [Views::Waveform, Views::Stacked] {
                editor.file.presentation.views = views;
                let tree = editor.window(10, None);
                assert_eq!(anchor(&tree, 11), Some(expected));
                // Repeated structural redraws must preserve the same association.
                let tree = editor.window(10, None);
                assert_eq!(anchor(&tree, 11), Some(expected));
            }
        }
        call(&mut editor, json!({"verb":"playing", "file":null}));
        let tree = editor.window(10, None);
        assert_eq!(anchor(&tree, 11), Some(0.0));
    }

    #[test]
    fn transport_projects_only_its_file_and_resizes_the_existing_meter() {
        let mut editor =
            new_json(r#"{"path":"/in/take3.wav","workspace":"/work","frames":100}"#).unwrap();
        call(
            &mut editor,
            json!({"verb":"sync", "meters":{"bus":-1,"channels":1}}),
        );
        call(
            &mut editor,
            json!({"verb":"window", "widget":10, "pane":11, "lane":13, "meter":12, "filesWidget":40}),
        );
        editor.file.presentation.views = Views::Stacked;
        let first = editor.focused_file();
        editor.add_file(
            new_json(r#"{"path":"/in/take50.wav","workspace":"/work","frames":200}"#).unwrap(),
        );
        editor.file.presentation.views = Views::Stacked;
        let out = call(
            &mut editor,
            json!({"verb":"playing", "file":first, "meters":{"bus":-1,"channels":1}}),
        );
        for widget in [10, 11, 13] {
            let correction = out["corrections"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["widget"] == widget)
                .unwrap();
            assert_eq!(correction["props"]["playhead_at"], -1.0);
        }
        let meter = out["corrections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["widget"] == 12)
            .unwrap();
        assert_eq!(meter["props"], json!({"bus":-1,"channels":1}));
        let stopped = call(
            &mut editor,
            json!({"verb":"playing", "file":null, "meters":{"bus":-1,"channels":1}}),
        );
        let meter = stopped["corrections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["widget"] == 12)
            .unwrap();
        assert_eq!(meter["props"], json!({"bus":-1,"channels":1}));
    }

    /// **The meter is drawn before the playback says where it writes**: the
    /// window holds it from the start, reading no bus, and the answer that
    /// says where points it there. A window with no file draws no meter, and
    /// nothing corrects one.
    #[test]
    fn a_meter_is_drawn_before_the_playback_says_where_it_writes() {
        let mut editor =
            new_json(r#"{"path":"/in/take3.wav","workspace":"/work","frames":100}"#).unwrap();
        let tree = call(
            &mut editor,
            json!({"verb":"window", "widget":10, "meter":12}),
        );
        assert!(
            tree.to_string()
                .contains(r#""bus":-1,"channels":1,"id":12,"rate":"control""#),
            "{tree}"
        );
        let out = call(
            &mut editor,
            json!({"verb":"sync", "meters":{"bus":7,"channels":1}}),
        );
        assert_eq!(
            out["corrections"],
            json!([{"widget":12, "props":{"bus":7,"channels":1}}])
        );
        let mut empty = new_json(r#"{"empty": true}"#).unwrap();
        let tree = call(
            &mut empty,
            json!({"verb":"window", "widget":10, "meter":12}),
        );
        assert!(!tree.to_string().contains(r#""type":"meter""#), "{tree}");
        let out = call(
            &mut empty,
            json!({"verb":"playing", "file":null, "meters":{"bus":7,"channels":1}}),
        );
        assert!(
            out["corrections"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["widget"] != 12),
            "{out}"
        );
    }
}
