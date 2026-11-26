//! **The multitrack editor's dialogs**: the path a session is saved under
//! and the session to open -- two forms of the shared mechanism
//! ([`crate::forms`]), the audio editor's Save as and Open for a session.

use crate::forms::{self, FileRequest, FileType, Spec};

/// **One of the forms.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// The path the session is written to, from now on.
    SaveAs,
    /// The session file to open in this window, chosen in the host's
    /// file dialog.
    Open,
}

impl Spec for Form {
    fn prefix(self) -> &'static str {
        match self {
            Form::SaveAs => "file",
            Form::Open => "open",
        }
    }

    fn page(self) -> i32 {
        match self {
            Form::SaveAs => 1,
            Form::Open => 2,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Form::SaveAs => "Save as",
            Form::Open => "Open",
        }
    }

    fn fields(self) -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }

    /// The session to open is chosen in the host's file dialog, one file,
    /// and the file to save as in it too.
    fn file(self) -> Option<FileRequest> {
        Some(match self {
            Form::Open => FileRequest::open(SESSIONS, false),
            Form::SaveAs => FileRequest::save(SESSIONS),
        })
    }

    fn accept(self) -> &'static str {
        match self {
            Form::Open => "Open",
            Form::SaveAs => "Save",
        }
    }
}

/// **The kind of file a multitrack opens and saves**: a session, written
/// as JSON.
pub const SESSIONS: &[FileType] = &[FileType::new("Multitrack session", &["json"])];

/// The forms the window holds, in the order of their pages.
const FORMS: [Form; 2] = [Form::SaveAs, Form::Open];

/// **Every widget of the dialogs, by name**, for a caller to number.
#[must_use]
pub fn names() -> Vec<String> {
    forms::names(&FORMS)
}

/// The forms the window holds, in the order of their pages.
#[must_use]
pub fn forms() -> &'static [Form] {
    &FORMS
}

/// **The window's dialogs**, held whole ([`forms::Dialogs`]).
pub type Dialogs = forms::Dialogs<Form>;
