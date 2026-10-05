//! **The score editor's dialogs**: the forms a menu entry opens over the
//! window while it is up.
//!
//! Four of them -- the page's text, a transformation's parameter, the page's
//! margins and staff, a file's path -- and one rule for how a window comes to
//! show one. A
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
pub type Ids = BTreeMap<String, i32>;

/// The field of the text form holding the footnotes, one to a line.
pub const NOTES: &str = "notes";

/// The stack the dialogs are pages of.
pub const STACK: &str = "stack";

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
}

/// **What a path is asked for.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum File {
    /// The file to read into the editor.
    Open,
    /// The file to write the score to, from now on.
    SaveAs,
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
}

impl Param {
    /// What the field is labelled, which is the question it asks.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Param::Transpose => "Semitones",
            Param::Repeat => "Times",
            Param::Stretch => "Factor",
        }
    }

    /// What the field opens holding.
    #[must_use]
    pub fn initial(self) -> &'static str {
        match self {
            Param::Transpose => "2",
            Param::Repeat => "2",
            Param::Stretch => "3/2",
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
            "transpose" => Form::Param(Param::Transpose),
            "repeat" => Form::Param(Param::Repeat),
            "stretch" => Form::Param(Param::Stretch),
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
            Form::Page => "Page setup",
            Form::File(File::Open) => "Open",
            Form::File(File::SaveAs) => "Save as",
        }
    }
}

/// The forms the window holds; the parameter's is one form whatever
/// transformation asks, and the file's one whichever way the file goes.
const FORMS: [Form; 4] = [
    Form::Text,
    Form::Param(Param::Transpose),
    Form::Page,
    Form::File(File::Open),
];

/// **Every widget of the dialogs, by name**, for a caller to number: the
/// stack; and for each form its dialog (`text`), its fields (`text:title`),
/// the labels a form corrects (`param:label`) and its two buttons
/// (`text:ok`, `text:cancel`).
#[must_use]
pub fn names() -> Vec<String> {
    let mut out = vec![STACK.to_string()];
    for form in FORMS {
        let prefix = form.prefix();
        out.push(prefix.to_string());
        out.extend(
            form.fields()
                .iter()
                .map(|(name, _)| format!("{prefix}:{name}")),
        );
        if matches!(form, Form::Param(_)) {
            out.push(format!("{prefix}:label"));
        }
        out.push(format!("{prefix}:ok"));
        out.push(format!("{prefix}:cancel"));
    }
    out
}

/// Whether `ids` numbers every widget of the dialogs: a window has all of
/// them or none.
#[must_use]
pub fn numbered(ids: &Ids) -> bool {
    names().iter().all(|name| ids.contains_key(name))
}

/// One form as a dialog: a row per field, and `Cancel` and `OK` under them.
fn dialog(form: Form, ids: &Ids) -> Value {
    let prefix = form.prefix();
    let id = |name: &str| ids.get(&format!("{prefix}:{name}")).copied();
    let rows: Vec<Value> = form
        .fields()
        .iter()
        .map(|(name, label)| {
            let mut caption = json!({"type": "label", "text": label, "w": 150});
            // the parameter's caption is the question the transformation asks
            if let (Form::Param(_), Some(map)) = (form, caption.as_object_mut()) {
                map.insert("id".into(), json!(id("label")));
            }
            let mut field = json!({"type": "text", "id": id(name), "value": "", "w": 320});
            if let (true, Some(map)) = (*name == NOTES, field.as_object_mut()) {
                map.insert("multiline".into(), json!(true));
                map.insert("h".into(), json!(72));
            }
            json!({"type": "layout", "flow": "row", "hug": true, "children": [caption, field]})
        })
        .collect();
    let buttons = json!({
        "type": "layout",
        "flow": "row",
        "hug": true,
        "pack": true,
        "children": [
            {"type": "separator", "weight": 1, "line": false},
            {"type": "button", "id": id("cancel"), "label": "Cancel"},
            {"type": "button", "id": id("ok"), "label": "OK"},
        ],
    });
    json!({
        "type": "layout",
        "id": ids.get(prefix),
        "modal": true,
        "frame": true,
        "hug": true,
        "title": form.title(),
        "flow": "col",
        "children": rows.into_iter().chain([buttons]).collect::<Vec<_>>(),
    })
}

/// **The dialogs, as the stack the window holds** -- or `None` when the
/// caller did not number them. It takes no room in the window's flow: page 0
/// is empty, and each form's dialog is alone on a page of its own.
#[must_use]
pub fn stack(ids: &Ids) -> Option<Value> {
    if !numbered(ids) {
        return None;
    }
    let pages: Vec<Value> = std::iter::once(json!({"type": "layout"}))
        .chain(
            FORMS
                .iter()
                .map(|form| json!({"type": "layout", "children": [dialog(*form, ids)]})),
        )
        .collect();
    Some(json!({
        "type": "layout",
        "id": ids.get(STACK),
        "flow": "stack",
        "index": 0,
        "margin": 0,
        "h": 0,
        "children": pages,
    }))
}

/// **What a widget of the dialogs said.**
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    /// A field now holds this text.
    Field(String, String),
    /// `OK`.
    Accept,
    /// `Cancel`, Escape, or the dialog's close mark.
    Cancel,
}

/// **What the widget `name` reporting `tag` said** -- or `None` for a name
/// that is no widget of `form`, and for a report that says nothing (a
/// button's value, which rises and falls with the hand).
#[must_use]
pub fn read(form: Form, name: &str, tag: &str) -> Option<Said> {
    let prefix = form.prefix();
    if name == prefix {
        return (tag == "cancel").then_some(Said::Cancel);
    }
    let part = name.strip_prefix(prefix)?.strip_prefix(':')?;
    match part {
        "ok" => (tag == "click").then_some(Said::Accept),
        "cancel" => (tag == "click").then_some(Said::Cancel),
        field if form.fields().iter().any(|(name, _)| *name == field) => {
            Some(Said::Field(field.to_string(), tag.to_string()))
        }
        _ => None,
    }
}

/// **A form that is up**: which, and what its fields hold as they are typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    pub form: Form,
    pub values: BTreeMap<String, String>,
}

impl Open {
    /// What the field `name` holds.
    #[must_use]
    pub fn value(&self, name: &str) -> &str {
        self.values.get(name).map_or("", String::as_str)
    }

    /// **What the window is corrected with to show it**: every field's text,
    /// the parameter's caption, and the stack turned to its page.
    #[must_use]
    pub fn shown(&self, ids: &Ids) -> Vec<(i32, Value)> {
        let prefix = self.form.prefix();
        let mut out: Vec<(i32, Value)> = self
            .form
            .fields()
            .iter()
            .filter_map(|(name, _)| {
                let id = *ids.get(&format!("{prefix}:{name}"))?;
                Some((id, json!({"value": self.value(name)})))
            })
            .collect();
        if let (Form::Param(param), Some(id)) = (self.form, ids.get("param:label")) {
            out.push((*id, json!({"text": param.label()})));
        }
        // a dialog that serves several questions says which it is asking
        if let Some(id) = ids.get(prefix) {
            out.push((*id, json!({"title": self.form.title()})));
        }
        if let Some(stack) = ids.get(STACK) {
            out.push((*stack, json!({"index": self.form.page()})));
        }
        out
    }
}

/// What the stack is corrected with to take a dialog down.
#[must_use]
pub fn hidden(ids: &Ids) -> Vec<(i32, Value)> {
    ids.get(STACK)
        .map(|stack| (*stack, json!({"index": 0})))
        .into_iter()
        .collect()
}

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
        assert_eq!(pages.len(), 5, "no dialog, and the four forms");
        assert!(pages[0].get("children").is_none());
        for (page, form) in pages[1..].iter().zip(FORMS) {
            let dialog = &page["children"][0];
            assert_eq!(dialog["modal"], true);
            assert_eq!(dialog["id"], ids[form.prefix()]);
            // a row per field, and the buttons
            let rows = dialog["children"].as_array().unwrap();
            assert_eq!(rows.len(), form.fields().len() + 1);
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
            read(form, "text:title", "A title"),
            Some(Said::Field("title".into(), "A title".into()))
        );
        // a field holding the word a button says is still a field
        assert_eq!(
            read(form, "text:title", "click"),
            Some(Said::Field("title".into(), "click".into()))
        );
        assert_eq!(read(form, "text:ok", "click"), Some(Said::Accept));
        assert_eq!(
            read(form, "text:ok", "1"),
            None,
            "a button's value is no command"
        );
        assert_eq!(read(form, "text:cancel", "click"), Some(Said::Cancel));
        assert_eq!(read(form, "text", "cancel"), Some(Said::Cancel));
        assert_eq!(read(form, "page:top", "12"), None, "another form's field");
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
