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

impl AudioEditor {
    /// The server buffer currently drawn; also the open file's identity.
    pub fn focused_file(&self) -> i64 {
        self.display
    }

    pub(super) fn binding(&self) -> Value {
        let (start, end) = self.selection.unwrap_or((0, 0));
        json!({"display": self.display, "take": self.opened, "channels": self.channels,
            "rate": self.rate, "frames": self.length(), "cursor": self.cursor.unwrap_or(0),
            "span": if end > start { json!([start as f64 / self.rate, end as f64 / self.rate]) } else { Value::Null },
            "file": self.file(), "layers": self.layers(), "looping": self.looping,
            "dirty": self.dirty})
    }

    pub(super) fn refresh_files(&mut self) {
        if !self.file_open {
            return;
        }
        let describe = |file: &AudioEditor| {
            json!({"id": file.display,
            "label": file.take_name().unwrap_or_else(|| format!("buffer {}", file.opened)),
            "file": file.file()})
        };
        if self.files.is_empty() {
            self.files.push(describe(self));
        }
        let active = describe(self);
        for row in &mut self.files {
            if row["id"].as_i64() == Some(self.display) {
                *row = active.clone();
            } else if let Some(file) = row["id"].as_i64().and_then(|id| self.parked.get(&id)) {
                *row = describe(file);
            }
        }
    }

    /// **Hands `file` what is the window's rather than a file's**: the
    /// widgets, the dialogs and the form that is up, the conversation with the
    /// host, and where a take only the history holds is written.
    fn bind_window(&self, file: &mut AudioEditor) {
        file.window = self.window;
        file.widget = self.widget;
        file.pane = self.pane;
        file.envelope_widget = self.envelope_widget;
        file.lane = self.lane;
        file.files_widget = self.files_widget;
        file.files_visible = self.files_visible;
        file.dialogs = self.dialogs.clone();
        file.title = self.title.clone();
        file.size = self.size;
        file.bare = self.bare;
        file.meters = self.meters;
        file.dialog.clone_from(&self.dialog);
        file.conversation = self.conversation;
        file.scratch.clone_from(&self.scratch);
        file.chunk = self.chunk;
    }

    pub(super) fn add_file(&mut self, mut file: AudioEditor) {
        self.refresh_files();
        self.bind_window(&mut file);
        file.files = std::mem::take(&mut self.files);
        file.parked = std::mem::take(&mut self.parked);
        let was = std::mem::replace(self, file);
        self.parked.insert(was.display, Box::new(was));
        self.files.push(json!({"id": self.display}));
        self.refresh_files();
    }

    pub(super) fn focus_file(&mut self, id: i64) -> Result<(), String> {
        if id == self.display {
            return Ok(());
        }
        let Some(mut file) = self.parked.remove(&id) else {
            return Err("the file is not open in this editor".into());
        };
        self.bind_window(&mut file);
        file.files = std::mem::take(&mut self.files);
        file.parked = std::mem::take(&mut self.parked);
        let was = std::mem::replace(self, *file);
        self.parked.insert(was.display, Box::new(was));
        self.refresh_files();
        Ok(())
    }

    pub(super) fn close_file(&mut self, id: i64) -> Result<(Value, Vec<i64>), String> {
        self.refresh_files();
        if !self.files.iter().any(|f| f["id"].as_i64() == Some(id)) {
            return Err("the file is not open in this editor".into());
        }
        if id == self.display {
            if let Some(next) = self
                .files
                .iter()
                .filter_map(|f| f["id"].as_i64())
                .find(|&i| i != id)
            {
                self.focus_file(next)?;
            } else {
                self.files.clear();
                self.file_open = false;
                return Ok(self.free_buffers());
            }
        }
        let file = self.parked.remove(&id).unwrap();
        self.files.retain(|f| f["id"].as_i64() != Some(id));
        Ok(file.free_buffers())
    }

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

    /// **What the window is corrected with when this file comes into
    /// focus**: its cursor, its selection and its loop switch on every view
    /// of the take. They are the host's screen state, which a definition does
    /// not overwrite, so they travel as an answer -- which also hands the
    /// host the version the switch moved to.
    pub(super) fn focus_corrections(&self) -> Vec<Correction> {
        let (start, end) = self.selection.unwrap_or((0, 0));
        let cursor = self.cursor.unwrap_or(0);
        [self.widget, self.pane, self.lane]
            .into_iter()
            .flatten()
            .map(|id| Correction {
                widget: i64::from(id),
                props: json!({
                    "cursor": cursor,
                    "sel_start": if end > start { start } else { cursor },
                    "sel_len": end.saturating_sub(start),
                    "looping": self.looping,
                }),
            })
            .collect()
    }

    pub(super) fn compose_files(&self, tree: &mut Value) {
        let Some(widget) = self.files_widget else {
            return;
        };
        if !self.files_visible {
            return;
        }
        let selected: Vec<usize> = self
            .files
            .iter()
            .position(|f| f["id"].as_i64() == Some(self.display))
            .into_iter()
            .collect();
        let rows: Vec<Value> = self.files.iter().map(|f| json!([f["label"]])).collect();
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
            children.insert(index, json!({"type": "layout", "flow": "row", "margin": 0, "weight": 1,
                "children": [{"type": "table", "id": widget, "name": "files", "label": "Files", "w": 180,
                    "rows": rows, "selected": selected, "multiple": false}, picture]}));
        }
    }
    /// **Whether the take is a file not written yet**: a save has nowhere to
    /// go until a path is given.
    pub fn untitled(&self) -> bool {
        matches!(&self.target, Target::File(path) if path.is_empty())
    }

    /// **What the take is called** in its pane: the name the caller gave it,
    /// else its file's name, `untitled` for a file not written yet, and none
    /// for a take that is a buffer.
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

    /// The name of the dialog widget `widget` is, when it is one.
    pub(super) fn dialog_name(&self, widget: i64) -> Option<String> {
        self.dialogs
            .iter()
            .find(|(_, id)| i64::from(**id) == widget)
            .map(|(name, _)| name.clone())
    }

    /// **Opens `form` over the window**, its fields holding what the editor
    /// has: the shape of the file in the window, or its path.
    pub(super) fn open_form(&mut self, form: Form) -> (Option<String>, Vec<Correction>) {
        if !dialogs::numbered(&self.dialogs) {
            return (Some("this window has no dialogs".into()), Vec::new());
        }
        let values = match form {
            Form::New => dialogs::new_of(self.channels, self.rate),
            Form::SaveAs => {
                let path = match &self.target {
                    Target::File(path) => path.clone(),
                    Target::Buffer(_) => String::new(),
                };
                [("path".to_string(), path)].into()
            }
            Form::Open => [("path".to_string(), ".".to_string())].into(),
        };
        let open = Open { form, values };
        let mut shown = open.shown(&self.dialogs);
        if form == Form::Open
            && let Some(id) = self.dialogs.get("open:path")
        {
            for (widget, props) in &mut shown {
                if widget == id {
                    *props = json!({"path": "."});
                }
            }
        }
        self.dialog = Some(open);
        (None, corrected(shown))
    }

    /// **A save from the window**: over what the take was opened from, or --
    /// for a file not written yet, in a window with dialogs -- the save-as
    /// form, which asks where.
    pub(super) fn save_from_window(
        &mut self,
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if self.untitled() && dialogs::numbered(&self.dialogs) {
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
        if self.dialog.as_ref().is_some_and(|d| d.form == Form::Open) && name == "open:path" {
            if matches!(tag, "pick" | "select") {
                let paths: Vec<Value> = values
                    .iter()
                    .flat_map(|v| v.as_array().cloned().unwrap_or_else(|| vec![v.clone()]))
                    .filter(|v| v.is_string())
                    .collect();
                if tag == "pick" && !paths.is_empty() {
                    out.open_file = Some(json!({"paths": paths}));
                    self.dialog = None;
                    return (None, corrected(forms::hidden(&self.dialogs)));
                }
                if let Some(open) = self.dialog.as_mut() {
                    open.values.insert("paths".into(), json!(paths).to_string());
                }
            }
            return (None, Vec::new());
        }
        let Some(open) = self.dialog.as_mut() else {
            return (None, Vec::new());
        };
        match forms::read(open.form, name, tag, values) {
            None => (None, Vec::new()),
            Some(Said::Field(field, value)) => {
                open.values.insert(field, value);
                (None, Vec::new())
            }
            Some(Said::Cancel | Said::Discard) => {
                self.dialog = None;
                (None, corrected(forms::hidden(&self.dialogs)))
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
                self.dialog = None;
                let mut corrections = corrected(forms::hidden(&self.dialogs));
                // a file saved under a new path is called by it from now on
                if let (Form::SaveAs, Some(pane)) = (open.form, self.pane) {
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
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_file_list_is_an_independent_hideable_widget_on_the_left() {
        let mut editor = super::super::new_json(r#"{"take":1,"frames":1000,"display":2}"#).unwrap();
        editor.bare = true;
        editor.files_widget = Some(20);
        let tree = editor.window(10, None);
        assert_eq!(tree["children"][0]["flow"], "row");
        assert_eq!(tree["children"][0]["children"][0]["id"], 20);
        assert_eq!(tree["children"][0]["children"][0]["type"], "table");
        assert_eq!(tree["children"][0]["children"][1]["id"], 10);
        editor.files_visible = false;
        assert_eq!(editor.window(10, None)["children"][0]["id"], 10);
    }
}
