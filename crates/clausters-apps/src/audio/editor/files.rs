//! The file the take is: the new-file, open and save-as dialogs, and a file not
//! written yet, which asks for its path the first time it is saved.

use super::*;
use crate::audio::dialogs::{self, Form, Open};
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
    /// The focused open object's stable identity, not its display buffer.
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

    pub(crate) fn playback_file(&self) -> Value {
        json!({"id":self.file.id, "display":self.file.display, "channels":self.file.channels,
            "frames":self.length(), "rate":self.file.rate, "preview":self.preview_state()})
    }

    pub(super) fn binding(&self) -> Value {
        let (start, end) = self.file.presentation.selection.unwrap_or((0, 0));
        json!({"id": self.file.id, "generation": self.window_state.binding_generation,
            "display": self.file.display, "take": self.file.opened, "channels": self.file.channels,
            "rate": self.file.rate, "frames": self.length(), "cursor": self.file.presentation.cursor.unwrap_or(0),
            "span": if end > start { json!([start as f64 / self.file.rate, end as f64 / self.file.rate]) } else { Value::Null },
            "file": self.file(), "layers": self.layers(), "looping": self.file.presentation.looping, "dirty": self.file.dirty})
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
        let mut next = file.file;
        next.id = FileId(self.open_files.next_id);
        self.open_files.next_id += 1;
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

    pub(super) fn close_file(&mut self, id: i64) -> Result<(Value, Vec<i64>), String> {
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
                return Ok(self.file.free_buffers());
            }
        }
        let file = self.open_files.parked.remove(&FileId(id)).unwrap();
        self.open_files
            .rows
            .retain(|f| f["id"].as_i64() != Some(id));
        Ok(file.free_buffers())
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
        }
        corrections
    }

    /// The cursor and the selection, as every view of the take is corrected
    /// with them.
    fn band_corrections(&self) -> Vec<Correction> {
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
        self.window_state
            .dialogs
            .iter()
            .find(|(_, id)| i64::from(**id) == widget)
            .map(|(name, _)| name.clone())
    }

    /// **Opens `form` over the window**, its fields holding what the editor
    /// has: the shape of the file in the window, or its path.
    pub(super) fn open_form(&mut self, form: Form) -> (Option<String>, Vec<Correction>) {
        if !dialogs::numbered(&self.window_state.dialogs) {
            return (Some("this window has no dialogs".into()), Vec::new());
        }
        let values = match form {
            Form::New => dialogs::new_of(self.file.channels, self.file.rate),
            Form::SaveAs => {
                let path = match &self.file.target {
                    Target::File(path) => path.clone(),
                    Target::Buffer(_) => String::new(),
                };
                [("path".to_string(), path)].into()
            }
            Form::Open => [("path".to_string(), ".".to_string())].into(),
        };
        let open = Open { form, values };
        let mut shown = open.shown(&self.window_state.dialogs);
        if form == Form::Open
            && let Some(id) = self.window_state.dialogs.get("open:path")
        {
            for (widget, props) in &mut shown {
                if widget == id {
                    *props = json!({"path": "."});
                }
            }
        }
        self.window_state.dialog = Some(open);
        (None, corrected(shown))
    }

    /// **A save from the window**: over what the take was opened from, or --
    /// for a file not written yet, in a window with dialogs -- the save-as
    /// form, which asks where.
    pub(super) fn save_from_window(
        &mut self,
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if self.untitled() && dialogs::numbered(&self.window_state.dialogs) {
            return self.open_form(Form::SaveAs);
        }
        match self.save(None, "float") {
            Ok(steps) => {
                out.steps = Some(steps);
                (None, Vec::new())
            }
            Err(why) => (Some(why), Vec::new()),
        }
    }

    /// **What a widget of the dialogs said**: a field typed into, a form
    /// cancelled, or `OK` -- a new file's shape, which the outcome names for
    /// the client to open, or a path the take is saved under from now on.
    pub(super) fn form_event(
        &mut self,
        name: &str,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if self
            .window_state
            .dialog
            .as_ref()
            .is_some_and(|d| d.form == Form::Open)
            && name == "open:path"
        {
            if matches!(tag, "pick" | "select") {
                let paths: Vec<Value> = values
                    .iter()
                    .flat_map(|v| v.as_array().cloned().unwrap_or_else(|| vec![v.clone()]))
                    .filter(|v| v.is_string())
                    .collect();
                if tag == "pick" && !paths.is_empty() {
                    out.open_file = Some(json!({"paths": paths}));
                    self.window_state.dialog = None;
                    return (None, corrected(forms::hidden(&self.window_state.dialogs)));
                }
                if let Some(open) = self.window_state.dialog.as_mut() {
                    open.values.insert("paths".into(), json!(paths).to_string());
                }
            }
            return (None, Vec::new());
        }
        let Some(open) = self.window_state.dialog.as_mut() else {
            return (None, Vec::new());
        };
        match forms::read(open.form, name, tag, values) {
            None => (None, Vec::new()),
            Some(Said::Field(field, value)) => {
                open.values.insert(field, value);
                (None, Vec::new())
            }
            Some(Said::Cancel | Said::Discard) => {
                self.window_state.dialog = None;
                (None, corrected(forms::hidden(&self.window_state.dialogs)))
            }
            Some(Said::Accept) => {
                let open = open.clone();
                let reason = match open.form {
                    Form::New => match dialogs::new_shape(&open) {
                        Ok((channels, frames, rate)) => {
                            out.new_file = Some(json!({
                                "channels": channels, "frames": frames, "rate": rate,
                            }));
                            None
                        }
                        Err(why) => Some(why),
                    },
                    Form::Open => {
                        let paths = serde_json::from_str::<Vec<Value>>(open.value("paths"))
                            .unwrap_or_default();
                        if paths.is_empty() {
                            Some("select one or more files to open".into())
                        } else {
                            out.open_file = Some(json!({ "paths": paths }));
                            None
                        }
                    }
                    Form::SaveAs => {
                        let path = open.value("path").trim().to_string();
                        if path.is_empty() {
                            Some("a file is saved under a path".into())
                        } else {
                            match self.save(Some(Target::File(path)), "float") {
                                Ok(steps) => {
                                    out.steps = Some(steps);
                                    None
                                }
                                Err(why) => Some(why),
                            }
                        }
                    }
                };
                // a refused form stays up, holding what was typed
                if reason.is_some() {
                    return (reason, Vec::new());
                }
                self.window_state.dialog = None;
                let mut corrections = corrected(forms::hidden(&self.window_state.dialogs));
                // a file saved under a new path is called by it from now on
                if let (Form::SaveAs, Some(pane)) = (open.form, self.window_state.pane) {
                    corrections.push(Correction {
                        widget: i64::from(pane),
                        props: json!({"label": self.pane_label()}),
                    });
                }
                (None, corrections)
            }
        }
    }

    /// **The file the take is**, as a client reads it: its path (`null` for
    /// one not written yet, or a take saved into a buffer), its shape and its
    /// rate.
    pub fn file(&self) -> Value {
        self.file.file()
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_file_list_is_an_independent_hideable_widget_on_the_left() {
        let mut editor = super::super::new_json(r#"{"take":1,"frames":1000,"display":2}"#).unwrap();
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
        let mut editor = super::super::new_json(r#"{"take":1,"frames":1000,"display":2}"#).unwrap();
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
    fn free_buffers(&self) -> (Value, Vec<i64>) {
        let mut buffers: std::collections::BTreeSet<i64> = self
            .frames
            .keys()
            .copied()
            .filter(|b| !self.spilled.contains(b))
            .chain(self.spare.iter().copied())
            .chain([self.display])
            .collect();
        if self.owns_opened {
            buffers.insert(self.opened);
        }
        let resident: Vec<i64> = buffers.iter().copied().collect();
        buffers.extend(self.spilled.iter().copied());
        let buffers: Vec<i64> = buffers.into_iter().collect();
        let steps = steps_json(
            &resident
                .iter()
                .copied()
                .flat_map(|buffer| {
                    [
                        send(
                            "/buffer_free",
                            vec![clausters_core::osc::OscType::Int(buffer as i32)],
                        ),
                        clausters_editing::apply::Step::AwaitDone {
                            command: "/buffer_free".into(),
                            index: Some(buffer as i32),
                        },
                    ]
                })
                .collect::<Vec<_>>(),
        );
        (steps, buffers)
    }
    pub(super) fn take_name(&self) -> Option<String> {
        if let Some(name) = self.name.as_ref().filter(|n| !n.is_empty()) {
            return Some(name.clone());
        }
        match &self.target {
            Target::File(path) if path.is_empty() => Some("untitled".into()),
            Target::File(path) => std::path::Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string),
            Target::Buffer(_) => None,
        }
    }
    pub fn file(&self) -> Value {
        let path = match &self.target {
            Target::File(path) if !path.is_empty() => json!(path),
            _ => Value::Null,
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
        matches!(&self.target, Target::File(path) if path.is_empty())
    }
    fn length(&self) -> u64 {
        parts::length(
            &self.contents.parts,
            &parts::Rates {
                join: self.rate,
                of: &self.rates,
            },
        )
        .unwrap_or(0)
    }
    fn description(&self) -> Value {
        json!({"id": self.id, "label": self.take_name().unwrap_or_else(|| format!("buffer {}", self.opened)), "file": self.file()})
    }
    pub(super) fn rooted(&self) -> Vec<SourceId> {
        let mut roots = parts::sources(&self.contents.parts);
        let opened = SourceId(self.take as u64);
        if !roots.contains(&opened) {
            roots.push(opened);
        }
        roots
    }
    pub(super) fn bytes(&self, buffer: i64) -> Option<u64> {
        self.frames
            .get(&buffer)
            .map(|frames| frames * u64::from(self.channels) * 4)
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    fn call(editor: &mut AudioEditor, request: Value) -> Value {
        serde_json::from_str(&call_json(editor, &request.to_string())).unwrap()
    }

    #[test]
    fn focus_rebinds_file_state_without_copying_the_window_or_its_conversation() {
        let mut editor = new_json(r#"{"take":3,"frames":100,"display":9}"#).unwrap();
        editor.window_state.files_widget = Some(40);
        editor.window_state.pane = Some(11);
        editor.window_state.lane = Some(13);
        editor.window_state.conversation = Conversation::new(17);
        editor.window_state.scratch = Some("history".into());
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
        editor.add_file(new_json(r#"{"take":50,"frames":200,"channels":6,"display":51}"#).unwrap());
        let second = editor.focused_file();
        assert_ne!(first, second);
        assert_ne!(second, editor.file.display);
        assert_eq!(editor.window_state.conversation.applied, 17);
        assert_eq!(editor.window_state.files_widget, Some(40));
        assert_eq!(editor.window_state.scratch.as_deref(), Some("history"));
        assert_eq!(editor.window_state.binding_generation, 1);
        // Changing a display binding does not change the open object's identity.
        call(&mut editor, json!({"verb":"sync", "display":999}));
        assert_eq!(editor.focused_file(), second);
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
        assert_eq!(editor.file.display, 999);
        let third_generation = editor.window_state.binding_generation;
        editor.close_file(first).unwrap();
        assert_eq!(editor.window_state.binding_generation, third_generation);
        assert!(editor.focus_file(first).is_err());
    }

    #[test]
    fn transport_projects_only_its_file_and_resizes_the_existing_meter() {
        let mut editor = new_json(r#"{"take":3,"frames":100,"display":9}"#).unwrap();
        call(
            &mut editor,
            json!({"verb":"window", "widget":10, "pane":11, "lane":13, "meter":12, "filesWidget":40}),
        );
        editor.file.presentation.views = Views::Stacked;
        let first = editor.focused_file();
        editor.add_file(new_json(r#"{"take":50,"frames":200,"display":51}"#).unwrap());
        editor.file.presentation.views = Views::Stacked;
        let out = call(
            &mut editor,
            json!({"verb":"playing", "file":first, "meters":{"bus":100,"channels":6}}),
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
        assert_eq!(meter["props"], json!({"bus":100,"channels":6}));
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
}
