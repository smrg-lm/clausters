//! **The editor's end of its dialogs**: a form opened holding what the score
//! holds, kept as it is typed, and written to the score on `OK` as one entry.
//!
//! What the forms are and how the window shows one is
//! [`dialogs`](super::super::dialogs); what is here is what each of them does
//! to this editor.

use std::collections::BTreeMap;

use clausters_editing::conversation::Correction;
use clausters_notation::Op;

use crate::forms::{Said, corrections};

use super::super::dialogs::{self, Form, Open};
use super::{Outcome, ScoreEditor};

impl ScoreEditor {
    /// **Open `form`**, holding what the score holds: the reason it cannot
    /// be, or the corrections that show it.
    pub(super) fn open_form(&mut self, form: Form) -> (Option<String>, Vec<Correction>) {
        if !self.dialogs.numbered(dialogs::forms()) {
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
        (None, corrections(self.dialogs.open(form, values)))
    }

    /// **A widget of the dialogs reported**: a field's text is kept, `Cancel`
    /// takes the form down, and `OK` writes what it holds -- one entry -- and
    /// takes it down, or says why it cannot and leaves it up.
    pub(super) fn form_said(
        &mut self,
        name: &str,
        tag: &str,
        values: &[serde_json::Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        let Some(said) = self.dialogs.read(name, tag, values) else {
            return (None, Vec::new());
        };
        match said {
            // what a field holds is kept by the reading, and the score
            // editor's forms have no chooser
            Said::Field(..) | Said::Chosen(..) | Said::Picked(..) | Said::Entered(..) => {
                (None, Vec::new())
            }
            Said::Cancel => {
                self.leaving = None;
                (None, corrections(self.dialogs.hide()))
            }
            // closing without saving: what was not saved is let go
            Said::Discard => {
                crate::file::let_what_waits_go(self, out);
                (None, corrections(self.dialogs.hide()))
            }
            Said::Accept => {
                let Some(open) = self.dialogs.up().cloned() else {
                    return (None, Vec::new());
                };
                if let Some(reason) = self.accept(&open, out) {
                    return (Some(reason), Vec::new());
                }
                // a save the close form asked for, with no file yet, asks for
                // one: the file form is now the one up
                if let Some(up) = self.dialogs.up().filter(|up| up.form != open.form) {
                    return (None, corrections(up.shown(&self.dialogs.ids)));
                }
                let mut shown = self.corrections();
                shown.extend(corrections(self.dialogs.hide()));
                (None, shown)
            }
        }
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
            Form::Close => crate::file::save_then_go(self, out),
            // A file is its holder's to read and write: the outcome names
            // it, and for a save it is the score's file from now on.
            Form::File(file) => {
                let path = open.value("path").trim().to_string();
                match file.format() {
                    Some(_) if path.is_empty() => Some("a file is named by its path".into()),
                    Some(format) => {
                        out.export = Some(serde_json::json!({"path": path, "format": format}));
                        None
                    }
                    // the form goes down, and the rule says whether another
                    // comes up in its place
                    None => crate::file::path_given(self, file == dialogs::File::Open, &path, out),
                }
            }
        }
    }
}
