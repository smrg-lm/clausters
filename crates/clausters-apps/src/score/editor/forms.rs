//! **The editor's end of its dialogs**: a form opened holding what the score
//! holds, kept as it is typed, and written to the score on `OK` as one entry.
//!
//! What the forms are and how the window shows one is
//! [`dialogs`](super::super::dialogs); what is here is what each of them does
//! to this editor.

use std::collections::BTreeMap;

use serde_json::Value;

use clausters_core::notation::Op;
use clausters_editing::conversation::Correction;

use super::super::dialogs::{self, Form, Open, Said};
use super::{Outcome, ScoreEditor};

/// `(widget, props)` pairs as the corrections an answer carries.
fn corrected(pairs: Vec<(i32, Value)>) -> Vec<Correction> {
    pairs
        .into_iter()
        .map(|(widget, props)| Correction {
            widget: i64::from(widget),
            props,
        })
        .collect()
}

impl ScoreEditor {
    /// The widget of the dialogs `widget` is, by name.
    pub(super) fn dialog_widget(&self, widget: i64) -> Option<&str> {
        self.dialogs
            .iter()
            .find(|(_, id)| i64::from(**id) == widget)
            .map(|(name, _)| name.as_str())
    }

    /// **Open `form`**, holding what the score holds: the reason it cannot
    /// be, or the corrections that show it.
    pub(super) fn open_form(&mut self, form: Form) -> (Option<String>, Vec<Correction>) {
        if !dialogs::numbered(&self.dialogs) {
            return (Some("this window has no dialogs".into()), Vec::new());
        }
        let values = match form {
            Form::Text => {
                let held = self.held();
                match held.sheet() {
                    Some(sheet) => dialogs::text_of(&sheet.header),
                    None => {
                        return (
                            Some("this document has no model to edit".into()),
                            Vec::new(),
                        );
                    }
                }
            }
            Form::Param(param) => [("value".to_string(), param.initial().to_string())].into(),
            Form::Page => dialogs::page_of(&self.setup()),
            Form::Close => BTreeMap::new(),
            // an export is a file of its own: it opens asking, where a save
            // offers the score's
            Form::File(file) => {
                let offered = match file.format() {
                    Some(_) => String::new(),
                    None => self.path.clone().unwrap_or_default(),
                };
                [("path".to_string(), offered)].into()
            }
        };
        let open = Open { form, values };
        let shown = corrected(open.shown(&self.dialogs));
        self.dialog = Some(open);
        (None, shown)
    }

    /// **A widget of the dialogs reported**: a field's text is kept, `Cancel`
    /// takes the form down, and `OK` writes what it holds -- one entry -- and
    /// takes it down, or says why it cannot and leaves it up.
    pub(super) fn form_said(
        &mut self,
        name: &str,
        tag: &str,
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        let Some(open) = self.dialog.as_mut() else {
            return (None, Vec::new());
        };
        match dialogs::read(open.form, name, tag) {
            None => (None, Vec::new()),
            Some(Said::Field(field, text)) => {
                open.values.insert(field, text);
                (None, Vec::new())
            }
            Some(Said::Cancel) => {
                self.dialog = None;
                self.closing = false;
                (None, corrected(dialogs::hidden(&self.dialogs)))
            }
            // closing without saving: what was not saved is let go
            Some(Said::Discard) => {
                self.dialog = None;
                out.close = true;
                (None, corrected(dialogs::hidden(&self.dialogs)))
            }
            Some(Said::Accept) => {
                let open = open.clone();
                if let Some(reason) = self.accept(&open, out) {
                    return (Some(reason), Vec::new());
                }
                // a save the close form asked for, with no file yet, asks for
                // one: the file form is now the one up
                if self.dialog.as_ref().is_some_and(|up| up.form != open.form) {
                    return (None, corrected(self.dialog_shown()));
                }
                self.dialog = None;
                let mut corrections = self.corrections();
                corrections.extend(corrected(dialogs::hidden(&self.dialogs)));
                (None, corrections)
            }
        }
    }

    /// What the window is corrected with to show the form that is up.
    fn dialog_shown(&self) -> Vec<(i32, Value)> {
        self.dialog
            .as_ref()
            .map(|up| up.shown(&self.dialogs))
            .unwrap_or_default()
    }

    /// Write what `open` holds to the score; the answer is why it was not.
    fn accept(&mut self, open: &Open, out: &mut Outcome) -> Option<String> {
        match open.form {
            Form::Text => {
                let header = {
                    let held = self.held();
                    let sheet = held.sheet()?;
                    dialogs::header_from(&sheet.header, open)
                };
                self.edit(&[Op::SetHeader { header }], "page text", out)
                    .err()
            }
            Form::Param(param) => match param.action(open.value("value")) {
                Ok(action) => self.perform(&action, out),
                Err(why) => Some(why),
            },
            Form::Page => match dialogs::page_action(open) {
                Ok(action) => self.perform(&action, out),
                Err(why) => Some(why),
            },
            // Save, and close once it is written; a score with no file is
            // asked for one first
            Form::Close => {
                self.closing = true;
                let (reason, _) = self.save(out);
                if out.save.is_some() {
                    self.closing = false;
                    out.close = true;
                }
                reason
            }
            // A file is its holder's to read and write: the outcome names
            // it, and for a save it is the score's file from now on.
            Form::File(file) => {
                let path = open.value("path").trim().to_string();
                if path.is_empty() {
                    return Some("a file is named by its path".into());
                }
                match (file, file.format()) {
                    (_, Some(format)) => {
                        out.export = Some(serde_json::json!({"path": path, "format": format}));
                    }
                    (dialogs::File::Open, _) => {
                        self.opening = Some(path.clone());
                        out.open = Some(path);
                    }
                    (_, None) => {
                        self.path = Some(path.clone());
                        out.save = Some(path);
                        out.close = std::mem::take(&mut self.closing);
                    }
                }
                None
            }
        }
    }
}
