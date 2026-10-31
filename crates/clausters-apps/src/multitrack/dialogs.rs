//! **The multitrack editor's dialogs**: the path a session is saved under
//! and the session to open -- two forms of the shared mechanism
//! ([`crate::forms`]), the audio editor's Save as and Open for a session.

use crate::forms::{self, Chooser, Spec};

/// **One of the forms.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// The path the session is written to, from now on.
    SaveAs,
    /// The session file to open in this window, chosen in the host's
    /// chooser.
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
            Form::Open => "Open session",
        }
    }

    fn fields(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Form::SaveAs => vec![("path", "File")],
            Form::Open => vec![("path", "Session")],
        }
    }

    /// The session to open is chosen in the host's own chooser, one file.
    fn chooser(self, name: &str) -> Option<Chooser> {
        (self == Form::Open && name == "path").then_some(Chooser {
            filter: SESSIONS,
            multiple: false,
            w: 420.0,
            h: 300.0,
        })
    }
}

/// **The extensions of the files a multitrack opens**: a session is
/// written as JSON.
pub const SESSIONS: &[&str] = &["json"];

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
