//! **The score editor's dialogs**: the forms a menu entry opens over the
//! window while it is up.
//!
//! Five of them -- the page's text, a transformation's parameter, the page's
//! margins and staff, a file's path, and the question a window closed with
//! changes not saved asks -- and one rule for how a window comes to show one.
//! A
//! dialog is a node of the tree and an editor answers with props, never with
//! nodes, so every dialog is **in the window from the start, on a page of a
//! `stack`**: a hidden page is not placed, which is a dialog that is not up,
//! and opening one is the stack's `index` set to its page ([`Form::page`]),
//! with its fields corrected to what the score holds. Nothing is defined and
//! nothing is freed, so the standalone host and every client open the same
//! dialog by the same correction.
//!
//! The widgets are named here ([`names`]) and numbered by the caller, as the
//! tools are; a window composed without them has no dialogs, and the menu
//! entries that would open one are disabled.
//!
//! A field reports its text as it is typed, which the editor keeps
//! ([`Open`]); nothing reaches the score until `OK`, which is then **one
//! entry** of the history however many fields changed.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use clausters_core::notation::{FIELDS, Header, PageSetup};

/// The caller's widget id for each widget of the dialogs it numbered.
pub type Ids = crate::forms::Ids;

/// The field of the text form holding the footnotes, one to a line.
pub const NOTES: &str = "notes";

/// The stack the dialogs are pages of.
pub const STACK: &str = crate::forms::STACK;

/// **One of the forms.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// The page's text: every field of the header, and the footnotes.
    Text,
    /// One number a transformation takes.
    Param(Param),
    /// The page's margins and the staff's height.
    Page,
    /// The path of a file: to open, or to save as.
    File(File),
    /// **Whether to save before closing**: the score has changes its file
    /// does not hold. `OK` is Save, and a third button closes without.
    Close,
}

/// **What a path is asked for.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum File {
    /// The file to read into the editor.
    Open,
    /// The file to write the score to, from now on.
    SaveAs,
    /// The file to write the score's render to, as a Standard MIDI File.
    ExportMidi,
    /// The same, as a MIDI 2.0 Clip File.
    ExportClip,
}

impl File {
    /// The format an export writes, as the outcome names it; `None` for what
    /// is no export.
    #[must_use]
    pub fn format(self) -> Option<&'static str> {
        match self {
            File::ExportMidi => Some("smf"),
            File::ExportClip => Some("clip"),
            File::Open | File::SaveAs => None,
        }
    }
}

/// **The transformations that ask for a number.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Param {
    /// By how many semitones.
    Transpose,
    /// How many times in all.
    Repeat,
    /// By what factor the values are scaled: `3/2`, `2`.
    Stretch,
    /// A tempo mark: its words, and its speed in quarter notes a minute.
    Tempo,
    /// A direction: words written at a point.
    Direction,
    /// A rehearsal mark: a letter or a number.
    Rehearsal,
    /// A fingering over the note.
    Fingering,
    /// A chord symbol over the note.
    Harmony,
    /// A syllable of the lyrics under the note.
    Lyric,
    /// What a staff is called.
    StaffName,
    /// How far a staff sounds from what it writes.
    Transposition,
    /// The passes an ending is played in.
    Ending,
}

impl Param {
    /// What the field is labelled, which is the question it asks.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Param::Transpose => "Semitones",
            Param::Repeat => "Times",
            Param::Stretch => "Factor",
            Param::Tempo => "Tempo (words, then speed)",
            Param::Direction | Param::Rehearsal => "Text",
            Param::Fingering => "Finger",
            Param::Harmony => "Chord",
            Param::Lyric => "Syllable (2: for verse 2)",
            Param::StaffName => "Name, short name",
            Param::Transposition => "Semitones",
            Param::Ending => "Passes",
        }
    }

    /// What the field opens holding.
    #[must_use]
    pub fn initial(self) -> &'static str {
        match self {
            Param::Transpose => "2",
            Param::Repeat => "2",
            Param::Stretch => "3/2",
            Param::Tempo => "Allegro 120",
            Param::Direction => "dolce",
            Param::Rehearsal => "A",
            Param::Fingering => "1",
            Param::Harmony => "C",
            Param::Lyric => "",
            Param::StaffName => "",
            Param::Transposition => "-2",
            Param::Ending => "1",
        }
    }

    /// **The editor's verb for what was typed**, or why it is not a
    /// parameter.
    ///
    /// # Errors
    /// When the text is not the number the transformation takes.
    pub fn action(self, typed: &str) -> Result<Value, String> {
        let typed = typed.trim();
        match self {
            Param::Transpose => typed
                .parse::<i32>()
                .map(|semitones| json!({"action": "transform", "name": "transpose", "semitones": semitones}))
                .map_err(|_| format!("semitones are a whole number, not {typed:?}")),
            Param::Repeat => typed
                .parse::<u32>()
                .ok()
                .filter(|count| *count >= 1)
                .map(|count| json!({"action": "transform", "name": "repeat", "count": count}))
                .ok_or_else(|| format!("a repetition is counted from 1, not {typed:?}")),
            Param::Stretch => {
                let (n, d) = typed.split_once('/').unwrap_or((typed, "1"));
                match (n.trim().parse::<i64>(), d.trim().parse::<i64>()) {
                    (Ok(n), Ok(d)) if n > 0 && d > 0 => {
                        Ok(json!({"action": "transform", "name": "stretch", "factor": [n, d]}))
                    }
                    _ => Err(format!(
                        "a factor is a positive number or a fraction such as 3/2, not {typed:?}"
                    )),
                }
            }
            // the speed is the number it ends with, the words what is before
            Param::Tempo => {
                let (words, last) = typed.rsplit_once(' ').unwrap_or(("", typed));
                let (text, bpm) = match last.trim_start_matches('=').parse::<f64>() {
                    Ok(bpm) if bpm > 0.0 => (words.trim().trim_end_matches('=').trim(), Some(bpm)),
                    _ => (typed, None),
                };
                Ok(json!({"action": "control", "kind": "tempo", "text": text, "bpm": bpm}))
            }
            Param::Direction => Ok(json!({"action": "control", "kind": "dir", "text": typed})),
            Param::Rehearsal => Ok(json!({"action": "control", "kind": "reh", "text": typed})),
            Param::Fingering => {
                Ok(json!({"action": "mark", "mark": "fingering", "value": typed}))
            }
            Param::Harmony => Ok(json!({"action": "mark", "mark": "harmony", "value": typed})),
            Param::Lyric => {
                let (verse, text) = match typed.split_once(':') {
                    Some((n, text)) if n.trim().parse::<usize>().is_ok() => {
                        (n.trim().parse::<usize>().unwrap_or(1), text.trim())
                    }
                    _ => (1, typed),
                };
                Ok(json!({"action": "lyric", "verse": verse, "text": text}))
            }
            Param::StaffName => {
                let (label, abbr) = typed.split_once(',').unwrap_or((typed, ""));
                Ok(json!({"action": "staff", "label": label.trim(), "abbr": abbr.trim()}))
            }
            Param::Transposition => typed
                .parse::<i32>()
                .map(|semitones| json!({"action": "staff", "transpose": semitones}))
                .map_err(|_| format!("semitones are a whole number, not {typed:?}")),
            Param::Ending => Ok(json!({"action": "ending", "label": typed})),
        }
    }
}

impl Form {
    /// The form a menu entry's word names (`text`, `page`, `transpose`...).
    #[must_use]
    pub fn named(word: &str) -> Option<Form> {
        Some(match word {
            "text" => Form::Text,
            "page" => Form::Page,
            "open" => Form::File(File::Open),
            "save" => Form::File(File::SaveAs),
            "export_midi" => Form::File(File::ExportMidi),
            "export_clip" => Form::File(File::ExportClip),
            "close" => Form::Close,
            "transpose" => Form::Param(Param::Transpose),
            "repeat" => Form::Param(Param::Repeat),
            "stretch" => Form::Param(Param::Stretch),
            "tempo" => Form::Param(Param::Tempo),
            "direction" => Form::Param(Param::Direction),
            "rehearsal" => Form::Param(Param::Rehearsal),
            "fingering" => Form::Param(Param::Fingering),
            "harmony" => Form::Param(Param::Harmony),
            "lyric" => Form::Param(Param::Lyric),
            "staff_name" => Form::Param(Param::StaffName),
            "transposition" => Form::Param(Param::Transposition),
            "ending" => Form::Param(Param::Ending),
            _ => return None,
        })
    }

    /// The prefix its widgets are named under.
    #[must_use]
    pub fn prefix(self) -> &'static str {
        match self {
            Form::Text => "text",
            Form::Param(_) => "param",
            Form::Page => "page",
            Form::File(_) => "file",
            Form::Close => "close",
        }
    }

    /// The page of the stack it is on; page 0 is no dialog.
    #[must_use]
    pub fn page(self) -> i32 {
        match self {
            Form::Text => 1,
            Form::Param(_) => 2,
            Form::Page => 3,
            Form::File(_) => 4,
            Form::Close => 5,
        }
    }

    /// Its fields, by name, each with what it is labelled.
    #[must_use]
    pub fn fields(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Form::Text => vec![
                ("title", "Title"),
                ("subtitle", "Subtitle"),
                ("composer", "Composer"),
                ("lyricist", "Lyricist"),
                ("arranger", "Arranger"),
                ("translator", "Translator"),
                ("copyright", "Copyright"),
                (NOTES, "Footnotes"),
            ],
            Form::Param(param) => vec![("value", param.label())],
            Form::Page => vec![
                ("top", "Top margin (mm)"),
                ("right", "Right margin (mm)"),
                ("bottom", "Bottom margin (mm)"),
                ("left", "Left margin (mm)"),
                ("staff", "Staff height (mm)"),
            ],
            Form::File(_) => vec![("path", "File")],
            Form::Close => Vec::new(),
        }
    }

    /// What its dialog is titled; a form that serves several questions is
    /// corrected with the one it is asking when it is shown.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Form::Text => "Page text",
            Form::Param(Param::Transpose) => "Transpose",
            Form::Param(Param::Repeat) => "Repeat",
            Form::Param(Param::Stretch) => "Stretch",
            Form::Param(Param::Tempo) => "Tempo",
            Form::Param(Param::Direction) => "Direction",
            Form::Param(Param::Rehearsal) => "Rehearsal mark",
            Form::Param(Param::Fingering) => "Fingering",
            Form::Param(Param::Harmony) => "Chord symbol",
            Form::Param(Param::Lyric) => "Lyrics",
            Form::Param(Param::StaffName) => "Staff name",
            Form::Param(Param::Transposition) => "Transposition",
            Form::Param(Param::Ending) => "Ending",
            Form::Page => "Page setup",
            Form::File(File::Open) => "Open",
            Form::File(File::SaveAs) => "Save as",
            Form::File(File::ExportMidi) => "Export MIDI",
            Form::File(File::ExportClip) => "Export clip",
            Form::Close => "Close",
        }
    }
}

/// The forms the window holds; the parameter's is one form whatever
/// transformation asks, and the file's one whichever way the file goes.
const FORMS: [Form; 5] = [
    Form::Text,
    Form::Param(Param::Transpose),
    Form::Page,
    Form::File(File::Open),
    Form::Close,
];

/// What the close form asks.
pub const UNSAVED: &str = "The score has changes that are not saved.";

/// How high the footnotes' field is: four lines of them.
const NOTES_H: f64 = 76.0;

impl crate::forms::Spec for Form {
    fn prefix(self) -> &'static str {
        Form::prefix(self)
    }

    fn page(self) -> i32 {
        Form::page(self)
    }

    fn title(self) -> &'static str {
        Form::title(self)
    }

    fn fields(self) -> Vec<(&'static str, &'static str)> {
        Form::fields(self)
    }

    fn label(self) -> Option<&'static str> {
        match self {
            Form::Param(param) => Some(param.label()),
            _ => None,
        }
    }

    fn closes(self) -> Option<&'static str> {
        (self == Form::Close).then_some(UNSAVED)
    }

    fn field(self, name: &str, widget: &mut serde_json::Map<String, Value>) {
        if name == NOTES {
            widget.insert("multiline".into(), json!(true));
            widget.insert("h".into(), json!(NOTES_H));
        }
    }
}

/// **Every widget of the dialogs, by name**, for a caller to number
/// ([`crate::forms::names`]).
#[must_use]
pub fn names() -> Vec<String> {
    crate::forms::names(&FORMS)
}

/// Whether `ids` numbers every widget of the dialogs: a window has all of
/// them or none.
#[must_use]
pub fn numbered(ids: &Ids) -> bool {
    crate::forms::numbered(&FORMS, ids)
}

/// **The dialogs, as the stack the window holds** -- or `None` when the
/// caller did not number them ([`crate::forms::stack`]).
#[must_use]
pub fn stack(ids: &Ids) -> Option<Value> {
    crate::forms::stack(&FORMS, ids)
}

pub use crate::forms::{Said, hidden};

/// **What the widget `name` reporting `tag` said** ([`crate::forms::read`]).
#[must_use]
pub fn read(form: Form, name: &str, tag: &str, values: &[Value]) -> Option<Said> {
    crate::forms::read(form, name, tag, values)
}

/// **A form that is up**: which, and what its fields hold as they are typed.
pub type Open = crate::forms::Open<Form>;

/// **The text form, holding `header`**: each field its text, and the
/// footnotes one to a line.
#[must_use]
pub fn text_of(header: &Header) -> BTreeMap<String, String> {
    let mut values: BTreeMap<String, String> = FIELDS
        .iter()
        .map(|field| {
            (
                (*field).to_string(),
                header.text(field).unwrap_or_default().to_string(),
            )
        })
        .collect();
    values.insert(NOTES.into(), header.notes.join("\n"));
    values
}

/// **`header` with what the text form holds**: the fields replaced, the
/// footnotes read back one from each line that has text, and where each
/// field sits left as it was.
#[must_use]
pub fn header_from(header: &Header, form: &Open) -> Header {
    let mut out = header.clone();
    for field in FIELDS {
        if let (Some(slot), Some(typed)) = (out.text_mut(field), form.values.get(*field)) {
            *slot = typed.trim().to_string();
        }
    }
    if let Some(notes) = form.values.get(NOTES) {
        out.notes = notes
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
    }
    out
}

/// A length in tenths of a millimetre, as millimetres: `127` is `12.7`.
fn millimetres(tenths: u32, per_mm: u32) -> String {
    let (whole, part) = (tenths / per_mm, tenths % per_mm);
    if part == 0 {
        return whole.to_string();
    }
    let width = if per_mm == 100 { 2 } else { 1 };
    let digits = format!("{part:0width$}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

/// **The page form, holding `setup`**: the margins and the staff's height, in
/// millimetres.
#[must_use]
pub fn page_of(setup: &PageSetup) -> BTreeMap<String, String> {
    let [top, right, bottom, left] = setup.margins;
    [
        ("top", millimetres(top, 10)),
        ("right", millimetres(right, 10)),
        ("bottom", millimetres(bottom, 10)),
        ("left", millimetres(left, 10)),
        ("staff", millimetres(setup.staff, 100)),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect()
}

/// **The editor's verb for what the page form holds**, or why a field is not
/// a length.
///
/// # Errors
/// When a field is not a number of millimetres.
pub fn page_action(form: &Open) -> Result<Value, String> {
    let length = |name: &str, per_mm: f64| -> Result<u32, String> {
        let typed = form.value(name).trim().replace(',', ".");
        typed
            .parse::<f64>()
            .ok()
            .filter(|mm| mm.is_finite() && *mm >= 0.0)
            .map(|mm| (mm * per_mm).round() as u32)
            .ok_or_else(|| format!("{name}: a length in millimetres, not {typed:?}"))
    };
    Ok(json!({
        "action": "page",
        "margins": [
            length("top", 10.0)?,
            length("right", 10.0)?,
            length("bottom", 10.0)?,
            length("left", 10.0)?,
        ],
        "staff": length("staff", 100.0)?,
    }))
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_tempo_is_read_as_its_words_and_its_speed() {
        assert_eq!(
            Param::Tempo.action("Allegro 132").unwrap(),
            json!({"action": "control", "kind": "tempo", "text": "Allegro", "bpm": 132.0})
        );
        assert_eq!(Param::Tempo.action("Andante").unwrap()["bpm"], Value::Null);
        assert_eq!(
            Param::Lyric.action("2: la-").unwrap(),
            json!({"action": "lyric", "verse": 2, "text": "la-"})
        );
        assert_eq!(
            Param::StaffName.action("Clarinet in B flat, Cl.").unwrap()["abbr"],
            "Cl."
        );
        assert!(Param::Transposition.action("two").is_err());
    }
    use super::*;

    fn ids() -> Ids {
        names()
            .into_iter()
            .enumerate()
            .map(|(i, name)| (name, 200 + i as i32))
            .collect()
    }

    #[test]
    fn the_dialogs_are_pages_of_a_stack_that_shows_none() {
        let ids = ids();
        let stack = stack(&ids).expect("numbered");
        assert_eq!(
            (stack["flow"].as_str(), stack["index"].as_i64()),
            (Some("stack"), Some(0))
        );
        assert_eq!(stack["h"], 0, "it takes no room in the window");
        let pages = stack["children"].as_array().unwrap();
        assert_eq!(pages.len(), 6, "no dialog, and the five forms");
        assert!(pages[0].get("children").is_none());
        for (page, form) in pages[1..].iter().zip(FORMS) {
            let dialog = &page["children"][0];
            assert_eq!(dialog["modal"], true);
            assert_eq!(dialog["id"], ids[form.prefix()]);
            // a row per field -- the close form's question in place of them
            // -- and the buttons
            let rows = dialog["children"].as_array().unwrap();
            let asked = usize::from(form == Form::Close);
            assert_eq!(rows.len(), form.fields().len() + asked + 1);
        }
        // every name is a widget, each under an id of its own
        let text = stack.to_string();
        for (name, id) in &ids {
            assert!(text.contains(&format!("\"id\":{id}")), "{name}");
        }
        // and a window that numbered only some has none
        let mut some = ids.clone();
        some.remove("page:staff");
        assert!(super::stack(&some).is_none());
    }

    #[test]
    fn a_form_is_shown_by_correcting_its_fields_and_turning_the_stack() {
        let ids = ids();
        let open = Open {
            form: Form::Param(Param::Stretch),
            values: [("value".to_string(), "3/2".to_string())].into(),
        };
        let shown = open.shown(&ids);
        assert!(shown.contains(&(ids["param:value"], json!({"value": "3/2"}))));
        assert!(shown.contains(&(ids["param:label"], json!({"text": "Factor"}))));
        assert!(shown.contains(&(ids["param"], json!({"title": "Stretch"}))));
        assert_eq!(shown.last(), Some(&(ids[STACK], json!({"index": 2}))));
        // the file's form is one dialog for both ways a file goes
        let save = Open {
            form: Form::File(File::SaveAs),
            values: [("path".to_string(), "a.mei".to_string())].into(),
        };
        let shown = save.shown(&ids);
        assert!(shown.contains(&(ids["file"], json!({"title": "Save as"}))));
        assert_eq!(shown.last(), Some(&(ids[STACK], json!({"index": 4}))));
        assert_eq!(hidden(&ids), vec![(ids[STACK], json!({"index": 0}))]);
    }

    #[test]
    fn a_report_is_read_as_what_the_form_was_told() {
        let form = Form::Text;
        assert_eq!(
            read(form, "text:title", "A title", &[]),
            Some(Said::Field("title".into(), "A title".into()))
        );
        // a field holding the word a button says is still a field
        assert_eq!(
            read(form, "text:title", "click", &[]),
            Some(Said::Field("title".into(), "click".into()))
        );
        assert_eq!(read(form, "text:ok", "click", &[]), Some(Said::Accept));
        assert_eq!(
            read(form, "text:ok", "1", &[]),
            None,
            "a button's value is no command"
        );
        assert_eq!(read(form, "text:cancel", "click", &[]), Some(Said::Cancel));
        assert_eq!(read(form, "text", "cancel", &[]), Some(Said::Cancel));
        assert_eq!(
            read(form, "page:top", "12", &[]),
            None,
            "another form's field"
        );
    }

    #[test]
    fn the_text_form_reads_a_header_and_writes_it_back() {
        let header = Header {
            title: "A title".into(),
            notes: vec!["* one".into(), "** two".into()],
            ..Header::default()
        };
        let mut open = Open {
            form: Form::Text,
            values: text_of(&header),
        };
        assert_eq!(open.value("title"), "A title");
        assert_eq!(open.value(NOTES), "* one\n** two");
        open.values
            .insert("composer".into(), "  A. Composer ".into());
        open.values.insert(NOTES.into(), "* one\n\n".into());
        let written = header_from(&header, &open);
        assert_eq!(written.composer, "A. Composer");
        assert_eq!(written.notes, vec!["* one".to_string()]);
        assert_eq!(written.title, "A title");
    }

    #[test]
    fn a_parameter_is_the_number_its_transformation_takes() {
        assert_eq!(
            Param::Transpose.action(" -3 ").unwrap(),
            json!({"action": "transform", "name": "transpose", "semitones": -3})
        );
        assert_eq!(
            Param::Stretch.action("3/2").unwrap()["factor"],
            json!([3, 2])
        );
        assert_eq!(Param::Stretch.action("2").unwrap()["factor"], json!([2, 1]));
        assert!(Param::Repeat.action("0").is_err());
        assert!(Param::Transpose.action("a fifth").is_err());
    }

    #[test]
    fn the_page_form_is_in_millimetres() {
        let setup = PageSetup::default();
        let mut open = Open {
            form: Form::Page,
            values: page_of(&setup),
        };
        assert_eq!(open.value("top"), "12.7");
        assert_eq!(open.value("staff"), "7.2");
        open.values.insert("left".into(), "20".into());
        open.values.insert("staff".into(), "6,5".into());
        let action = page_action(&open).unwrap();
        assert_eq!(action["margins"], json!([127, 127, 127, 200]));
        assert_eq!(action["staff"], 650);
        open.values.insert("top".into(), "wide".into());
        assert!(page_action(&open).is_err());
    }
}
