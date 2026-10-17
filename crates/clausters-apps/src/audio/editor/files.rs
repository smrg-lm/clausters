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
            Form::Open => [("path".to_string(), String::new())].into(),
        };
        let open = Open { form, values };
        let shown = open.shown(&self.dialogs);
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
                        let path = open.value("path").trim().to_string();
                        if path.is_empty() {
                            Some("a file is opened by its path".into())
                        } else {
                            out.open_file = Some(json!({ "path": path }));
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
            "untitled": self.untitled(),
        })
    }
}
