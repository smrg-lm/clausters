//! **The audio editor's dialogs**: a new file's shape, the path a file is
//! saved under, the file to open and the question unsaved work asks -- four
//! forms of the shared mechanism ([`crate::forms`]).

use std::collections::BTreeMap;

use crate::forms::{self, Chooser, FileType, Spec};

/// **One of the forms.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// A file not written yet: how many channels, at what rate, in which
    /// sample format -- the one it is saved in. It has no samples: what it
    /// holds, the edits put there.
    New,
    /// The path the take is written to, from now on.
    SaveAs,
    /// Files to open in this editor, chosen in the host's chooser.
    Open,
    /// **Whether to save the file in focus before it goes**: the window has
    /// it in view, so the question names it by showing it.
    Close,
}

/// What the close form asks.
pub const UNSAVED: &str = "This file has changes that are not saved.";

impl Spec for Form {
    fn prefix(self) -> &'static str {
        match self {
            Form::New => "new",
            Form::SaveAs => "file",
            Form::Open => "open",
            Form::Close => "close",
        }
    }

    fn page(self) -> i32 {
        match self {
            Form::New => 1,
            Form::SaveAs => 2,
            Form::Open => 3,
            Form::Close => 4,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Form::New => "New",
            Form::SaveAs => "Save as",
            Form::Open => "Open",
            Form::Close => "Close",
        }
    }

    fn fields(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Form::New => vec![
                ("channels", "Channels"),
                ("rate", "Sample rate (Hz)"),
                ("format", "Format"),
            ],
            Form::SaveAs => vec![("path", ""), (forms::NAME, "Name")],
            Form::Open => vec![("path", ""), (forms::NAME, "Name")],
            Form::Close => Vec::new(),
        }
    }

    fn closes(self) -> Option<&'static str> {
        (self == Form::Close).then_some(UNSAVED)
    }

    /// A new file's sample format is chosen among those a file is written
    /// in.
    fn options(self, name: &str) -> Option<&'static [&'static str]> {
        (self == Form::New && name == "format").then_some(&FORMAT_NAMES)
    }

    /// The files to open are chosen in the host's own chooser, where Ctrl
    /// and Shift select several, and the file to save as in it too.
    fn chooser(self, name: &str) -> Option<Chooser> {
        match (self, name) {
            (Form::Open, "path") => Some(Chooser::open(AUDIO, true)),
            (Form::SaveAs, "path") => Some(Chooser::save(WRITES)),
            _ => None,
        }
    }

    fn accept(self) -> &'static str {
        match self {
            Form::Open => "Open",
            Form::SaveAs => "Save",
            Form::New | Form::Close => "OK",
        }
    }
}

/// **The kinds of file an audio editor opens**: what a `SoundFile` reads
/// -- WAV, FLAC, Ogg Vorbis, MP3, MP4/AAC (and ALAC in it), AIFF and CAF.
pub const AUDIO: &[FileType] = &[
    WAV,
    FileType::new("FLAC", &["flac"]),
    FileType::new("Ogg Vorbis", &["ogg", "oga"]),
    FileType::new("MP3", &["mp3"]),
    FileType::new("MP4 audio", &["m4a", "mp4", "aac"]),
    FileType::new("AIFF", &["aif", "aiff", "aifc"]),
    FileType::new("CAF", &["caf"]),
];

/// A WAV file: what a take is written as.
pub const WAV: FileType = FileType::new("WAV", &["wav", "wave"]);

/// **The kinds of file an audio editor writes**: a take is saved as WAV.
pub const WRITES: &[FileType] = &[WAV];

/// **The sample formats a file is written in**: what the format field
/// shows, and the format a save writes. A take is held in 32-bit float, so
/// that is the one there is; a file is saved in the format it was made in.
pub const FORMATS: [(&str, &str); 1] = [("32-bit float", FLOAT)];

/// The 32-bit float sample format, as a save names it.
pub const FLOAT: &str = "float";

/// What the format field shows, in the order of [`FORMATS`].
const FORMAT_NAMES: [&str; 1] = [FORMATS[0].0];

/// The forms the window holds, in the order of their pages.
const FORMS: [Form; 4] = [Form::New, Form::SaveAs, Form::Open, Form::Close];

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

/// **A form that is up**: which, and what its fields hold as they are typed.
pub type Open = forms::Open<Form>;

/// **The new-file form, opening on the shape of the file in the window**:
/// its channels and its rate, in 32-bit float.
#[must_use]
pub fn new_of(channels: u32, rate: f64) -> BTreeMap<String, String> {
    [
        ("channels", channels.to_string()),
        ("rate", format!("{}", rate.round() as i64)),
        ("format", FORMATS[0].0.to_string()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

/// **The shape a new file is**, read from what the form holds: channels,
/// rate and the sample format it is saved in -- or why a field is not what
/// it can be. It has no frames: a new file holds no samples until an edit
/// puts them there.
///
/// # Errors
/// When the channels are not a whole number from 1, the rate is not a
/// positive number, or the format is none a file is written in.
pub fn new_shape(open: &Open) -> Result<(u32, f64, &'static str), String> {
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
    let format = FORMATS
        .iter()
        .find(|(name, _)| *name == open.value("format").trim())
        .map(|(_, format)| *format)
        .ok_or_else(|| {
            format!(
                "a file is written in 32-bit float, not {:?}",
                open.value("format")
            )
        })?;
    Ok((channels, rate, format))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids() -> forms::Ids {
        names()
            .into_iter()
            .enumerate()
            .map(|(i, name)| (name, 300 + i as i32))
            .collect()
    }

    /// **The verbs that open a form end in an ellipsis, and only they**: a
    /// menu entry says it asks for more before it acts, and one that acts at
    /// once (a panel shown, a view chosen) does not.
    #[test]
    fn the_verbs_that_open_a_form_end_in_an_ellipsis() {
        use clausters_editing::verbs::{AUDIO, WINDOW};
        let asks: Vec<&str> = WINDOW
            .iter()
            .chain(AUDIO)
            .filter_map(|v| v.label.strip_suffix("..."))
            .collect();
        // the close form asks, and no verb opens it
        let titles: Vec<&str> = FORMS
            .iter()
            .filter(|f| **f != Form::Close)
            .map(|f| f.title())
            .collect();
        assert_eq!(asks.len(), titles.len(), "{asks:?}");
        for title in titles {
            assert!(asks.contains(&title), "{title} opens a form");
        }
    }

    #[test]
    fn the_forms_are_pages_of_a_stack() {
        let ids = ids();
        let stack = forms::stack(&FORMS, &ids).expect("numbered");
        let pages = stack["children"].as_array().unwrap();
        assert_eq!(
            pages.len(),
            5,
            "no dialog, a new file, a save, an open and the close form"
        );
        assert_eq!(pages[1]["children"][0]["title"], "New");
        assert_eq!(pages[2]["children"][0]["title"], "Save as");
        assert_eq!(pages[3]["children"][0]["title"], "Open");
        // a chooser has no caption: the dialog's title says what it chooses
        let chooser = &pages[3]["children"][0]["children"][0]["children"][0]["children"][0];
        assert_eq!(chooser["type"], "files", "the host's chooser");
        assert_eq!(chooser["id"], ids["open:path"]);
        assert_eq!(chooser["multiple"], true);
        assert!(
            chooser["filter"]
                .as_array()
                .unwrap()
                .contains(&json!("flac"))
        );
        let open = Open {
            form: Form::SaveAs,
            values: BTreeMap::new(),
        };
        assert_eq!(
            open.shown(&ids).last(),
            Some(&(ids["stack"], json!({"index": 2})))
        );
    }

    /// **A new file is read from its two fields**, and has no length to
    /// ask: it holds no samples until an edit puts them there.
    #[test]
    fn a_new_file_is_read_from_its_two_fields() {
        assert_eq!(
            Form::New.fields().iter().map(|f| f.0).collect::<Vec<_>>(),
            ["channels", "rate", "format"]
        );
        // the format is chosen, and 32-bit float is the one there is
        assert_eq!(Form::New.options("format"), Some(&["32-bit float"][..]));
        let mut open = Open {
            form: Form::New,
            values: new_of(2, 44_100.0),
        };
        assert_eq!(new_shape(&open), Ok((2, 44_100.0, "float")));
        open.values.insert("rate".into(), "22050,5".into());
        assert_eq!(new_shape(&open), Ok((2, 22_050.5, "float")));
        open.values.insert("format".into(), "16-bit".into());
        assert!(new_shape(&open).is_err());
        open.values.insert("format".into(), "32-bit float".into());
        open.values.insert("channels".into(), "0".into());
        assert!(new_shape(&open).is_err());
        open.values.insert("channels".into(), "1".into());
        open.values.insert("rate".into(), "fast".into());
        assert!(new_shape(&open).is_err());
    }
}
