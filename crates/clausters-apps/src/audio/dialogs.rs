//! **The audio editor's dialogs**: a new file's shape, the path a file is
//! saved under and then its bit depth, the file to open and the question
//! unsaved work asks -- forms of the shared mechanism ([`crate::forms`]).
//! The host's file dialog chooses where and which kind; the kind's
//! parameter, the depth, is asked after it.

use std::collections::BTreeMap;

use crate::forms::{self, FileRequest, FileType, Spec};

/// **One of the forms.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// A file not written yet: at what rate, how many channels, in which
    /// bit depth -- the one it is saved in. It has no samples: what it
    /// holds, the edits put there.
    New,
    /// The path the take is written to, from now on: the host's file
    /// dialog.
    SaveAs,
    /// **The bit depth it is written in**, asked once the path is chosen.
    Depth,
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
            Form::Depth => "depth",
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
            Form::Depth => 5,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Form::New => "New",
            Form::SaveAs => "Save as",
            Form::Depth => "Bit depth",
            Form::Open => "Open",
            Form::Close => "Close",
        }
    }

    fn fields(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Form::New => vec![
                ("rate", "Sample rate (Hz)"),
                ("channels", "Channels"),
                ("format", "Bit depth"),
            ],
            Form::Depth => vec![("format", "Bit depth")],
            Form::SaveAs | Form::Open | Form::Close => Vec::new(),
        }
    }

    fn closes(self) -> Option<&'static str> {
        (self == Form::Close).then_some(UNSAVED)
    }

    /// A file's bit depth is chosen among those a file is written in, as
    /// it is made and as it is saved as another.
    fn options(self, name: &str) -> Option<&'static [&'static str]> {
        (matches!(self, Form::New | Form::Depth) && name == "format").then_some(&FORMAT_NAMES)
    }

    /// The files to open are chosen in the host's file dialog, where Ctrl
    /// and Shift select several, and the file to save as in it too.
    fn file(self) -> Option<FileRequest> {
        match self {
            Form::Open => Some(FileRequest::open(AUDIO, true)),
            Form::SaveAs => Some(FileRequest::save(WRITES)),
            Form::New | Form::Depth | Form::Close => None,
        }
    }

    fn accept(self) -> &'static str {
        match self {
            Form::Open => "Open",
            Form::SaveAs | Form::Depth => "Save",
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

/// **The bit depths a file is written in**: what the bit-depth field
/// shows, and the sample format a save writes -- every depth a file is
/// read in. A take is processed in 32-bit float whatever its file is; a
/// file is saved in the depth it was made in, or opened in, until a Save
/// as chooses another.
pub const FORMATS: [(&str, &str); 6] = [
    ("64-bit float", "double"),
    ("32-bit float", FLOAT),
    ("32-bit", "int32"),
    ("24-bit", "int24"),
    ("16-bit", "int16"),
    ("8-bit", "int8"),
];

/// The 32-bit float sample format, as a save names it.
pub const FLOAT: &str = "float";

/// What the bit-depth field shows for [`FLOAT`].
const FLOAT_NAME: &str = FORMATS[1].0;

/// What the bit-depth field shows, in the order of [`FORMATS`].
const FORMAT_NAMES: [&str; 6] = [
    FORMATS[0].0,
    FORMATS[1].0,
    FORMATS[2].0,
    FORMATS[3].0,
    FORMATS[4].0,
    FORMATS[5].0,
];

/// **Whether a file is written in `format`**, a save's name of it.
#[must_use]
pub fn writes(format: &str) -> bool {
    FORMATS.iter().any(|(_, f)| *f == format)
}

/// **The depth a file stored in `format` is saved in**: its own where a
/// file is written in it; for integers of another width (a 20-bit FLAC),
/// the narrowest integers that hold them; 32-bit float for a file that
/// states none (a lossy one).
#[must_use]
pub fn saved_format(format: Option<&str>) -> &'static str {
    let Some(format) = format else {
        return FLOAT;
    };
    if let Some((_, f)) = FORMATS.iter().find(|(_, f)| *f == format) {
        return f;
    }
    let width = |f: &str| f.strip_prefix("int").and_then(|b| b.parse::<u32>().ok());
    match width(format) {
        Some(bits) => FORMATS
            .iter()
            .rev()
            .map(|(_, f)| *f)
            .find(|f| width(f).is_some_and(|w| w >= bits))
            .unwrap_or(FLOAT),
        None => FLOAT,
    }
}

/// **The words the bit-depth field shows for `format`**, a save's name of
/// it -- 32-bit float for one it does not write.
#[must_use]
pub fn format_name(format: &str) -> &'static str {
    FORMATS
        .iter()
        .find(|(_, f)| *f == format)
        .map_or(FLOAT_NAME, |(name, _)| name)
}

/// **The sample format the bit-depth field of `open` chose**, as a save
/// names it -- or why it holds none a file is written in.
///
/// # Errors
/// When the field holds no depth of [`FORMATS`].
pub fn chosen_format(open: &Open) -> Result<&'static str, String> {
    FORMATS
        .iter()
        .find(|(name, _)| *name == open.value("format").trim())
        .map(|(_, format)| *format)
        .ok_or_else(|| {
            format!(
                "a file is written in {}, not {:?}",
                FORMAT_NAMES.join(", "),
                open.value("format")
            )
        })
}

/// The forms the window holds, in the order of their pages.
const FORMS: [Form; 5] = [
    Form::New,
    Form::SaveAs,
    Form::Open,
    Form::Close,
    Form::Depth,
];

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
/// its rate, its channels and its bit depth (`format`, as a save names it).
#[must_use]
pub fn new_of(channels: u32, rate: f64, format: &str) -> BTreeMap<String, String> {
    [
        ("rate", format!("{}", rate.round() as i64)),
        ("channels", channels.to_string()),
        ("format", format_name(format).to_string()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

/// **The shape a new file is**, read from what the form holds: channels,
/// rate and the bit depth it is saved in -- or why a field is not what it
/// can be. It has no frames: a new file holds no samples until an edit
/// puts them there.
///
/// # Errors
/// When the channels or the rate are not a whole number from 1, or the depth is none a file is written in.
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
    // a file states its rate in whole hertz, any of them
    let rate = number("rate")
        .parse::<u32>()
        .ok()
        .filter(|r| *r >= 1)
        .map(f64::from)
        .ok_or_else(|| {
            format!(
                "a rate is a whole number of hertz from 1, not {:?}",
                number("rate")
            )
        })?;
    let format = chosen_format(open)?;
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
        // the close form asks, and no verb opens it; the depth is asked
        // after a Save as
        let titles: Vec<&str> = FORMS
            .iter()
            .filter(|f| !matches!(**f, Form::Close | Form::Depth))
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
            6,
            "the file dialogs, a new file, a save, an open, the close form and the depth"
        );
        assert_eq!(pages[1]["children"][0]["title"], "New");
        assert_eq!(pages[5]["children"][0]["title"], "Bit depth");
        // the open and the save are the host's file dialog, on page 0
        let files = pages[0]["children"].as_array().unwrap();
        let open = files.iter().find(|f| f["id"] == ids["open"]).unwrap();
        assert_eq!(open["type"], "file_dialog");
        assert_eq!(open["mode"], "open");
        assert_eq!(open["multiple"], true);
        assert!(open["kinds"].to_string().contains("flac"));
        let save = files.iter().find(|f| f["id"] == ids["file"]).unwrap();
        assert_eq!(save["mode"], "save");
        assert_eq!(save["kinds"], json!([["WAV", ["wav", "wave"]]]));
        let open = Open::new(Form::Depth, BTreeMap::new());
        assert_eq!(
            open.shown(&ids).last(),
            Some(&(ids["stack"], json!({"index": 5})))
        );
    }

    /// **A new file is read from its three fields** -- its rate, its
    /// channels and its bit depth -- and has no length to ask: it holds no
    /// samples until an edit puts them there.
    #[test]
    fn a_new_file_is_read_from_its_three_fields() {
        assert_eq!(
            Form::New.fields().iter().map(|f| f.0).collect::<Vec<_>>(),
            ["rate", "channels", "format"]
        );
        // the bit depth is chosen, as it is after a Save as
        let depths = Some(
            &[
                "64-bit float",
                "32-bit float",
                "32-bit",
                "24-bit",
                "16-bit",
                "8-bit",
            ][..],
        );
        assert_eq!(Form::New.options("format"), depths);
        assert_eq!(Form::Depth.options("format"), depths);
        let mut open = Open::new(Form::New, new_of(2, 44_100.0, "int24"));
        assert_eq!(new_shape(&open), Ok((2, 44_100.0, "int24")));
        // any whole rate, and only a whole one: a file states it in hertz
        open.values.insert("rate".into(), "551".into());
        open.values.insert("format".into(), "16-bit".into());
        assert_eq!(new_shape(&open), Ok((2, 551.0, "int16")));
        open.values.insert("format".into(), "8-bit".into());
        assert_eq!(new_shape(&open), Ok((2, 551.0, "int8")));
        open.values.insert("rate".into(), "22050,5".into());
        assert!(new_shape(&open).is_err());
        open.values.insert("rate".into(), "551".into());
        open.values.insert("format".into(), "12-bit".into());
        assert!(new_shape(&open).is_err());
        open.values.insert("format".into(), "32-bit float".into());
        open.values.insert("channels".into(), "0".into());
        assert!(new_shape(&open).is_err());
        open.values.insert("channels".into(), "1".into());
        open.values.insert("rate".into(), "fast".into());
        assert!(new_shape(&open).is_err());
    }

    /// **A file is saved in the depth it stores**, every depth a file is
    /// read in being one it is written in: integers of a width no file is
    /// written in go to the narrowest that hold them, and a file that
    /// states no depth (a lossy one) to 32-bit float.
    #[test]
    fn a_file_is_saved_in_the_depth_it_stores() {
        for (_, format) in FORMATS {
            assert_eq!(saved_format(Some(format)), format);
            assert!(writes(format));
        }
        assert_eq!(saved_format(Some("int20")), "int24");
        assert_eq!(saved_format(Some("int12")), "int16");
        assert_eq!(saved_format(Some("int4")), "int8");
        assert_eq!(saved_format(Some("int48")), FLOAT);
        assert_eq!(saved_format(None), FLOAT);
        assert_eq!(format_name("ulaw"), "32-bit float");
    }
}
