//! **The notes editor's dialogs**: forms of the shared mechanism
//! ([`crate::forms`]) -- how a sequence written as no notation is read into a
//! page, when a copy of it is converted into one; the file to open and the
//! path to save as, for a roll that has a file; and the question a window
//! closed with changes not saved asks.
//!
//! The form opens holding the reading the sequence stores
//! (`notation.reading`) and the writing its first part says, so `OK` with
//! nothing changed is the conversion a window with no dialog makes; and
//! nothing is converted until `OK`, which is then **one entry** of the
//! history.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use clausters_document::EventSequence;
use clausters_document::events::reading::Transcription;
use clausters_document::events::{Polyphony, Writing};

use crate::forms::{self, Spec};

/// **One of the forms.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// The meter, the shortest value and the writing a sequence is read
    /// into a page with.
    Notation,
    /// The file to open in this window, chosen in the host's chooser.
    Open,
    /// The path the sequence is written to, from now on.
    SaveAs,
    /// **Whether to save before closing**: the sequence has changes its file
    /// does not hold. `OK` is Save, and a third button closes without.
    Close,
}

/// What the close form asks.
pub const UNSAVED: &str = "The sequence has changes that are not saved.";

/// The shortest values an onset lands on, each with the denominator it is
/// written with.
const DIVISIONS: [(&str, i64); 4] = [
    ("Quarter", 4),
    ("Eighth", 8),
    ("Sixteenth", 16),
    ("Thirty-second", 32),
];

/// How the polyphony is written, each with the writing every part takes.
const WRITINGS: [(&str, Polyphony, u8); 3] = [
    ("Two voices a staff", Polyphony::Voices, 2),
    ("One voice a staff", Polyphony::Voices, 1),
    ("One voice, with ties", Polyphony::Ties, 1),
];

const DIVISION_NAMES: [&str; 4] = [
    DIVISIONS[0].0,
    DIVISIONS[1].0,
    DIVISIONS[2].0,
    DIVISIONS[3].0,
];

const WRITING_NAMES: [&str; 3] = [WRITINGS[0].0, WRITINGS[1].0, WRITINGS[2].0];

impl Spec for Form {
    fn prefix(self) -> &'static str {
        match self {
            Form::Notation => "notation",
            Form::Open => "open",
            Form::SaveAs => "file",
            Form::Close => "close",
        }
    }

    fn page(self) -> i32 {
        match self {
            Form::Notation => 1,
            Form::Open => 2,
            Form::SaveAs => 3,
            Form::Close => 4,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Form::Notation => "Write as notation",
            Form::Open => "Open",
            Form::SaveAs => "Save as",
            Form::Close => "Close",
        }
    }

    fn fields(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Form::Notation => vec![
                ("meter", "Meter"),
                ("division", "Shortest value"),
                ("writing", "Polyphony"),
            ],
            Form::Open => vec![("path", "File")],
            Form::SaveAs => vec![("path", "File")],
            Form::Close => Vec::new(),
        }
    }

    fn options(self, name: &str) -> Option<&'static [&'static str]> {
        match (self, name) {
            (Form::Notation, "division") => Some(&DIVISION_NAMES),
            (Form::Notation, "writing") => Some(&WRITING_NAMES),
            _ => None,
        }
    }

    fn closes(self) -> Option<&'static str> {
        (self == Form::Close).then_some(UNSAVED)
    }

    /// The file to open is chosen in the host's own chooser, one file a roll
    /// reads.
    fn chooser(self, name: &str) -> Option<forms::Chooser> {
        (self == Form::Open && name == "path").then_some(forms::Chooser {
            filter: super::file::OPENS,
            multiple: false,
            w: 420.0,
            h: 300.0,
        })
    }
}

/// The forms the window holds, in the order of their pages.
const FORMS: [Form; 4] = [Form::Notation, Form::Open, Form::SaveAs, Form::Close];

/// **Every widget of the dialog, by name**, for a caller to number.
#[must_use]
pub fn names() -> Vec<String> {
    forms::names(&FORMS)
}

/// The forms the window holds.
#[must_use]
pub fn forms() -> &'static [Form] {
    &FORMS
}

/// **The window's dialogs**, held whole ([`forms::Dialogs`]).
pub type Dialogs = forms::Dialogs<Form>;

/// **A form that is up**: which, and what its fields hold as they are typed.
pub type Open = forms::Open<Form>;

/// **The form, holding what `sequence` would be read with**: the meter its
/// reading states (`4/4` where it states none), its shortest value, and how
/// its first part writes its polyphony.
#[must_use]
pub fn notation_of(sequence: &EventSequence) -> BTreeMap<String, String> {
    let reading = Transcription::of(sequence);
    let division = DIVISIONS
        .iter()
        .find(|(_, d)| *d == reading.division)
        .map_or(DIVISIONS[2].0, |(name, _)| *name);
    let writing = sequence
        .parts_read()
        .first()
        .map(|part| part.writing)
        .unwrap_or_default();
    let written = WRITINGS
        .iter()
        .find(|(_, polyphony, per)| {
            *polyphony == writing.polyphony
                && (*polyphony == Polyphony::Ties || *per == writing.voices_per_staff)
        })
        .map_or(WRITINGS[0].0, |(name, ..)| *name);
    [
        ("meter", reading.meter.unwrap_or_else(|| "4/4".into())),
        ("division", division.to_string()),
        ("writing", written.to_string()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

/// **The reading and the writing the form holds**, as the conversion takes
/// them, or why the meter is not one.
///
/// # Errors
/// When the meter is not written as `3/4`.
pub fn conversion(form: &Open) -> Result<(Value, Writing), String> {
    let typed = form.value("meter").trim();
    let meter = typed
        .split_once('/')
        .and_then(|(n, d)| Some((n.trim().parse::<i64>().ok()?, d.trim().parse::<i64>().ok()?)))
        .filter(|(count, unit)| *count > 0 && *unit > 0 && (*unit as u64).is_power_of_two())
        .ok_or_else(|| format!("a meter is written as 3/4, not {typed:?}"))?;
    let division = DIVISIONS
        .iter()
        .find(|(name, _)| *name == form.value("division"))
        .map_or(16, |(_, d)| *d);
    let (polyphony, voices_per_staff) = WRITINGS
        .iter()
        .find(|(name, ..)| *name == form.value("writing"))
        .map_or((Polyphony::Voices, 2), |(_, p, v)| (*p, *v));
    Ok((
        json!({"meter": format!("{}/{}", meter.0, meter.1), "division": division}),
        Writing {
            polyphony,
            voices_per_staff,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::forms::{Said, stack};

    fn dialogs() -> Dialogs {
        let mut d = Dialogs::default();
        d.ids = names()
            .into_iter()
            .enumerate()
            .map(|(i, name)| (name, 300 + i as i32))
            .collect();
        d
    }

    fn find(tree: &Value, id: i32) -> Option<&Value> {
        if tree["id"] == id {
            return Some(tree);
        }
        tree["children"]
            .as_array()?
            .iter()
            .find_map(|child| find(child, id))
    }

    #[test]
    fn the_form_opens_holding_what_the_sequence_would_be_read_with() {
        let plain = EventSequence::default();
        let held = notation_of(&plain);
        assert_eq!(held["meter"], "4/4");
        assert_eq!(held["division"], "Sixteenth");
        assert_eq!(held["writing"], "Two voices a staff");
        let mut stated = EventSequence::default();
        stated
            .edit(clausters_document::events::EventsIntent::SetReading {
                reading: Some(json!({"meter": "3/4", "division": 8})),
            })
            .unwrap();
        let held = notation_of(&stated);
        assert_eq!(
            (held["meter"].as_str(), held["division"].as_str()),
            ("3/4", "Eighth")
        );
    }

    #[test]
    fn the_choices_are_the_host_s_choice_and_report_the_option_chosen() {
        let mut d = dialogs();
        let tree = stack(&FORMS, &d.ids).unwrap();
        let division = find(&tree, d.ids["notation:division"]).unwrap();
        assert_eq!(division["type"], "choice");
        assert_eq!(division["options"], json!(DIVISION_NAMES));
        assert_eq!(
            find(&tree, d.ids["notation:meter"]).unwrap()["type"],
            "text"
        );
        let shown = d.open(Form::Notation, notation_of(&EventSequence::default()));
        assert!(shown.contains(&(d.ids["notation:division"], json!({"index": 2}))));
        assert_eq!(
            d.read("notation:writing", "2.0", &[]),
            Some(Said::Field("writing".into(), "One voice, with ties".into()))
        );
        assert_eq!(d.read("notation:writing", "9", &[]), None);
        d.read("notation:meter", "6/8", &[]);
        let (reading, writing) = conversion(d.up().unwrap()).unwrap();
        assert_eq!(reading, json!({"meter": "6/8", "division": 16}));
        assert_eq!(
            (writing.polyphony, writing.voices_per_staff),
            (Polyphony::Ties, 1)
        );
        d.read("notation:meter", "six", &[]);
        assert!(conversion(d.up().unwrap()).is_err());
        d.read("notation:meter", "3/5", &[]);
        assert!(conversion(d.up().unwrap()).is_err());
    }
}
