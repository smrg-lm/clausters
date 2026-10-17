//! **The audio editor's dialogs**: a new file's shape, the path a file is
//! saved under and the file to open -- three forms of the shared mechanism ([`crate::forms`]).

use std::collections::BTreeMap;

use serde_json::Value;

use crate::forms::{self, Spec};

/// The caller's widget id for each widget of the dialogs it numbered.
pub type Ids = forms::Ids;

/// **One of the forms.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// A file not written yet: how many channels, how long, at what rate.
    New,
    /// The path the take is written to, from now on.
    SaveAs,
    /// A file to open, in a window of its own.
    Open,
}

impl Spec for Form {
    fn prefix(self) -> &'static str {
        match self {
            Form::New => "new",
            Form::SaveAs => "file",
            Form::Open => "open",
        }
    }

    fn page(self) -> i32 {
        match self {
            Form::New => 1,
            Form::SaveAs => 2,
            Form::Open => 3,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Form::New => "New file",
            Form::SaveAs => "Save as",
            Form::Open => "Open file",
        }
    }

    fn fields(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Form::New => vec![
                ("channels", "Channels"),
                ("seconds", "Length (seconds)"),
                ("rate", "Sample rate (Hz)"),
            ],
            Form::SaveAs | Form::Open => vec![("path", "File")],
        }
    }
}

/// The forms the window holds, in the order of their pages.
const FORMS: [Form; 3] = [Form::New, Form::SaveAs, Form::Open];

/// **Every widget of the dialogs, by name**, for a caller to number.
#[must_use]
pub fn names() -> Vec<String> {
    forms::names(&FORMS)
}

/// Whether `ids` numbers every widget of the dialogs.
#[must_use]
pub fn numbered(ids: &Ids) -> bool {
    forms::numbered(&FORMS, ids)
}

/// **The dialogs, as the stack the window holds**, or `None` when the caller
/// did not number them.
#[must_use]
pub fn stack(ids: &Ids) -> Option<Value> {
    forms::stack(&FORMS, ids)
}

/// **A form that is up**: which, and what its fields hold as they are typed.
pub type Open = forms::Open<Form>;

/// The form `name` belongs to, by its prefix.
#[must_use]
pub fn form_of(name: &str) -> Option<Form> {
    let prefix = name.split(':').next()?;
    FORMS.into_iter().find(|form| form.prefix() == prefix)
}

/// **The new-file form, opening on the shape of the file in the window**:
/// its channels, ten seconds and its rate.
#[must_use]
pub fn new_of(channels: u32, rate: f64) -> BTreeMap<String, String> {
    [
        ("channels", channels.to_string()),
        ("seconds", "10".to_string()),
        ("rate", format!("{}", rate.round() as i64)),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

/// **The shape a new file is**, read from what the form holds: channels,
/// frames and rate -- or why a field is not a number it can be.
///
/// # Errors
/// When the channels are not a whole number from 1, the length is not a
/// non-negative number of seconds, or the rate is not a positive number.
pub fn new_shape(open: &Open) -> Result<(u32, u64, f64), String> {
    let number = |name: &str| open.value(name).trim().replace(',', ".");
    let channels = number("channels")
        .parse::<u32>()
        .ok()
        .filter(|c| *c >= 1)
        .ok_or_else(|| {
            format!(
                "channels are a whole number from 1, not {:?}",
                number("channels")
            )
        })?;
    let seconds = number("seconds")
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.0)
        .ok_or_else(|| {
            format!(
                "a length is a number of seconds, not {:?}",
                number("seconds")
            )
        })?;
    let rate = number("rate")
        .parse::<f64>()
        .ok()
        .filter(|r| r.is_finite() && *r > 0.0)
        .ok_or_else(|| {
            format!(
                "a rate is a positive number of hertz, not {:?}",
                number("rate")
            )
        })?;
    Ok((channels, (seconds * rate).round() as u64, rate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids() -> Ids {
        names()
            .into_iter()
            .enumerate()
            .map(|(i, name)| (name, 300 + i as i32))
            .collect()
    }

    #[test]
    fn the_two_forms_are_pages_of_a_stack() {
        let ids = ids();
        let stack = stack(&ids).expect("numbered");
        let pages = stack["children"].as_array().unwrap();
        assert_eq!(pages.len(), 4, "no dialog, a new file, a save and an open");
        assert_eq!(pages[1]["children"][0]["title"], "New file");
        assert_eq!(pages[2]["children"][0]["title"], "Save as");
        assert_eq!(pages[3]["children"][0]["title"], "Open file");
        assert_eq!(form_of("open:path"), Some(Form::Open));
        assert_eq!(form_of("new:rate"), Some(Form::New));
        assert_eq!(form_of("file:ok"), Some(Form::SaveAs));
        assert_eq!(form_of("stack"), None);
        let open = Open {
            form: Form::SaveAs,
            values: BTreeMap::new(),
        };
        assert_eq!(
            open.shown(&ids).last(),
            Some(&(ids["stack"], json!({"index": 2})))
        );
    }

    #[test]
    fn a_new_file_is_read_from_its_three_fields() {
        let mut open = Open {
            form: Form::New,
            values: new_of(2, 44_100.0),
        };
        assert_eq!(new_shape(&open), Ok((2, 441_000, 44_100.0)));
        open.values.insert("seconds".into(), "0,5".into());
        assert_eq!(new_shape(&open), Ok((2, 22_050, 44_100.0)));
        open.values.insert("channels".into(), "0".into());
        assert!(new_shape(&open).is_err());
        open.values.insert("channels".into(), "1".into());
        open.values.insert("rate".into(), "fast".into());
        assert!(new_shape(&open).is_err());
    }
}
