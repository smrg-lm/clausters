//! **The file dialog**: one element, `file_dialog`, that an application or a
//! client declares and reads one report from, presented by the host.
//!
//! What a caller says is a **request** ([`Request`]): open a file (or
//! several), save one, or choose a folder; the kinds of file it reads or
//! writes; where to start. It says only what a system's own dialog can say
//! too, so a request is presented by whatever the host presents it with,
//! behind one seam ([`Presenter`]). The host has one presenter today, the
//! dialog it draws ([`Drawn`]), in a window and in a page alike: a modal of
//! the widgets it already has (the `files` chooser, a field, a choice, the
//! buttons), numbered from its own ids and answered by the host, so the
//! caller hears only `"chosen" <kind> <path>...` or `"cancel"`. A system
//! presenter would be a second implementation of the seam, standing in the
//! tree as nothing and answering the same two reports.
//!
//! The element is opened and closed by its `open` prop. What its widgets
//! say (a file selected, a folder entered, a name typed, a kind chosen, a
//! tool pressed) is the presenter's, and so is what it keeps between two
//! openings: the folder the hand last went into.
//!
//! **Where the hand is stays in sight**: over the chooser the drawn dialog
//! shows the folder as a `path_bar`, a button per folder, with back, forward
//! and up beside it; `Ctrl+L`, its "Type a path" button or a press past the
//! last folder gives the path to a field instead, where Enter goes and
//! Escape gives the buttons back. A save over a file the folder holds asks
//! before it replaces it, and a save opens with the name's stem selected,
//! so what is typed replaces it.

mod hosting;
mod storage;

pub(crate) use hosting::Presented;

use clausters_core::osc::OscType;
use serde_json::{Map, Value, json};

use crate::host::elements::path_bar::{EDIT, GO};
use crate::host::files::{self, Listing};
use storage::{Storage, Tool};

/// The element's type.
pub const KIND: &str = "file_dialog";

/// **What a request chooses.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A file to read, or several.
    Open,
    /// The file to write.
    Save,
    /// A folder.
    Folder,
}

/// **A kind of file**: what it is called and the extensions its files have,
/// without the dot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kind {
    pub name: String,
    pub extensions: Vec<String>,
}

/// **What a caller asks for**: the element's props, read.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub mode: Mode,
    /// The kinds it reads, or the kinds a save writes, the first chosen.
    pub kinds: Vec<Kind>,
    /// Whether every file is offered too: an open's default, and a save's
    /// that writes whatever name it is given (text).
    pub all_files: bool,
    /// Whether an open chooses several files.
    pub multiple: bool,
    /// Where it starts: a folder, or for a save the file it writes.
    pub path: String,
    pub title: String,
    /// What its accepting button says.
    pub accept: String,
}

/// A list prop as its items: the list itself, or the JSON text `/gui_set`
/// carries a list as.
fn list(v: Option<&Value>) -> Vec<Value> {
    match v {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Array(items)) => items,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty() && s != "0" && s != "false",
        _ => false,
    }
}

impl Kind {
    /// One `kinds` item: `[name, [extension...]]` or `{"name", "extensions"}`.
    fn read(v: &Value) -> Option<Kind> {
        let (name, extensions) = match v {
            Value::Array(pair) => (pair.first()?, pair.get(1)),
            Value::Object(map) => (map.get("name")?, map.get("extensions")),
            _ => return None,
        };
        Some(Kind {
            name: name.as_str()?.to_string(),
            extensions: list(extensions)
                .iter()
                .filter_map(Value::as_str)
                .map(|e| e.trim_start_matches('.').to_lowercase())
                .collect(),
        })
    }
}

impl Request {
    /// **A request, from the element's props.**
    #[must_use]
    pub fn read(props: &Map<String, Value>) -> Request {
        let text = |key: &str| props.get(key).and_then(Value::as_str).map(str::to_string);
        let mode = match text("mode").as_deref() {
            Some("save") => Mode::Save,
            Some("folder") => Mode::Folder,
            _ => Mode::Open,
        };
        let (title, accept) = match mode {
            Mode::Open => ("Open", "Open"),
            Mode::Save => ("Save as", "Save"),
            Mode::Folder => ("Choose a folder", "Choose"),
        };
        Request {
            mode,
            kinds: list(props.get("kinds"))
                .iter()
                .filter_map(Kind::read)
                .collect(),
            all_files: props.get("all_files").map_or(mode == Mode::Open, truthy),
            multiple: mode == Mode::Open && props.get("multiple").is_some_and(truthy),
            path: text("path").unwrap_or_default(),
            title: text("title").unwrap_or_else(|| title.into()),
            accept: text("accept").unwrap_or_else(|| accept.into()),
        }
    }

    /// **The kinds its file-type field offers**, the first chosen when it
    /// opens. A save offers the kinds it writes, every file after them where
    /// it writes any name. An open offers every kind it reads at once first,
    /// where it reads several, then each of them, then every file. A folder
    /// offers none.
    #[must_use]
    pub fn offers(&self) -> Vec<Offer> {
        let one = |k: &Kind| Offer::of(&k.name, k.extensions.clone());
        let kinds: Vec<&Kind> = self
            .kinds
            .iter()
            .filter(|k| !k.extensions.is_empty())
            .collect();
        let mut out = Vec::new();
        match self.mode {
            Mode::Folder => return out,
            Mode::Save => out.extend(kinds.iter().copied().map(one)),
            Mode::Open => {
                if kinds.len() > 1 {
                    let every = kinds.iter().flat_map(|k| k.extensions.clone()).collect();
                    out.push(Offer::of("All supported", every));
                }
                out.extend(kinds.iter().copied().map(one));
            }
        }
        if self.all_files || out.is_empty() {
            out.push(Offer::of(ALL_FILES, Vec::new()));
        }
        out
    }
}

/// What every file is called.
pub const ALL_FILES: &str = "All files";

/// **One kind in the file-type field**: its name, its words, and the
/// extensions of the files it lists -- none for every file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub name: String,
    pub label: String,
    pub extensions: Vec<String>,
}

impl Offer {
    fn of(name: &str, extensions: Vec<String>) -> Offer {
        let globs = if extensions.is_empty() {
            "*.*".to_string()
        } else {
            extensions
                .iter()
                .map(|e| format!("*.{e}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        Offer {
            name: name.into(),
            label: format!("{name} ({globs})"),
            extensions,
        }
    }

    /// **Whether `path` is a file of this kind**, by its extension in any
    /// case: every file, for the kind that names none.
    #[must_use]
    pub fn holds(&self, path: &str) -> bool {
        self.extensions.is_empty()
            || std::path::Path::new(path)
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .is_some_and(|e| self.extensions.contains(&e))
    }

    /// **`path` named as a file of this kind**: as it is when it is one,
    /// else with the kind's first extension added.
    #[must_use]
    pub fn named(&self, path: &str) -> String {
        match self.extensions.first() {
            Some(ext) if !self.holds(path) => format!("{path}.{ext}"),
            _ => path.to_string(),
        }
    }
}

/// **What a presenter answers**: props for its own widgets, and the one
/// report the caller hears, if any.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Answer {
    /// Each widget's props to set, in order.
    pub set: Vec<(i32, Vec<(String, Value)>)>,
    /// What the caller hears: `"chosen" <kind> <path>...` or `"cancel"`.
    pub report: Option<Vec<OscType>>,
}

impl Answer {
    fn set(id: i32, props: Value) -> Answer {
        let mut a = Answer::default();
        a.push(id, props);
        a
    }

    fn push(&mut self, id: i32, props: Value) {
        let props = props
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.set.push((id, props));
    }
}

/// **How a request is presented**: the seam a system dialog would be a
/// second side of.
pub trait Presenter: std::fmt::Debug {
    /// **What stands in the tree** for the request now: the drawn dialog's
    /// widgets, up or not; nothing, for a dialog the platform draws.
    fn stand_in(&self) -> Value;
    /// The request, defined again: the caller may have changed what it asks.
    fn update(&mut self, request: Request);
    /// **Opened**, as its `open` prop said.
    fn open(&mut self) -> Answer;
    /// **Closed** by its caller, which hears nothing.
    fn close(&mut self) -> Answer;
    /// Whether it is up.
    fn is_up(&self) -> bool;
    /// Whether widget `id` is one of its own.
    fn owns(&self, id: i32) -> bool;
    /// **What one of its widgets said**: its tag (empty for a value), and
    /// what followed.
    fn event(&mut self, id: i32, tag: &str, args: &[OscType]) -> Answer;
    /// **A folder one of its widgets shows was listed**: where it resolved
    /// and what it holds, or why it could not be. Nothing, for a dialog
    /// whose platform lists its own folders.
    fn listed(&mut self, _id: i32, _listing: Result<&Listing, &str>) -> Answer {
        Answer::default()
    }
}

/// How many ids a drawn dialog takes from the host's.
pub const IDS: usize = 64;

/// Offsets of the drawn dialog's widgets in its block of ids.
const DIALOG: i32 = 0;
const FILES: i32 = 1;
const NAME: i32 = 2;
const TYPE: i32 = 3;
const SAYS: i32 = 4;
const CANCEL: i32 = 5;
const ACCEPT: i32 = 6;
const TOOLS: i32 = 7;
const BACK: i32 = 12;
const FORWARD: i32 = 13;
const UP: i32 = 14;
/// The path bar, the field the path is typed in instead, and the stack that
/// shows one of the two.
const BAR: i32 = 15;
const PATH: i32 = 16;
const PLACE: i32 = 17;
/// The button that gives the path to the field, and its chord.
const TYPE_PATH: i32 = 18;
const TYPE_PATH_CHORD: &str = "Ctrl+L";
/// Where its nodes with no role are numbered from.
const SPARE: i32 = 24;

/// How large the chooser is drawn, in logical pixels.
const CHOOSER_W: f64 = 520.0;
const CHOOSER_H: f64 = 300.0;
/// Between a caption and its field, the fields, and the footer's buttons:
/// the specification's gaps.
const CAPTION_GAP: f64 = 8.0;
const BODY_GAP: f64 = 16.0;
const FOOTER_GAP: f64 = 8.0;
/// Between the buttons of the row over the chooser.
const NAV_GAP: f64 = 4.0;

/// The folder a chooser starts in before anything else says where: the one
/// the host resolves `.` to.
const START: &str = ".";

/// The extension a folder chooser filters on, which no file can have: it
/// lists the folders alone.
const FOLDERS_ONLY: &str = "/";

/// **What the root of the storage is called** in the path bar: the disk's
/// is `/`, and a page's is its storage.
fn root_name() -> &'static str {
    if storage::STORAGE_IS_THE_DISK {
        "/"
    } else {
        "Storage"
    }
}

/// **The dialog the host draws.**
#[derive(Debug)]
pub struct Drawn {
    /// The element's own id, which the stand-in keeps.
    id: i32,
    /// The first of its widgets' ids.
    base: i32,
    request: Request,
    up: bool,
    /// The folder shown.
    dir: String,
    /// The folders the hand left, the last left last, and those it came
    /// back from.
    back: Vec<String>,
    forward: Vec<String>,
    /// The files of the folder shown, by name, once its listing is in.
    names: Option<Vec<String>>,
    /// The files the chooser has selected.
    selected: Vec<String>,
    /// What the name field holds.
    typed: String,
    /// Whether the path is being typed, the field shown for the bar, and
    /// what it holds.
    editing: bool,
    typed_dir: String,
    /// The file a save asked about replacing: a second accept replaces it.
    replacing: Option<String>,
    /// A save to a folder not listed yet, decided when its listing is in.
    pending: bool,
    /// Whether it was opened and nothing has been said since: the name's
    /// stem is still the selection it opened with.
    fresh: bool,
    /// The kind chosen, among [`Request::offers`].
    kind: usize,
    storage: Storage,
    /// The folder the hand last went into, kept from one opening to the
    /// next.
    last: Option<String>,
}

impl Drawn {
    /// The dialog for `request`, element `id`, its widgets numbered from
    /// `base`.
    #[must_use]
    pub fn new(id: i32, base: i32, request: Request) -> Drawn {
        Drawn {
            id,
            base,
            request,
            up: false,
            dir: START.into(),
            back: Vec::new(),
            forward: Vec::new(),
            names: None,
            selected: Vec::new(),
            typed: String::new(),
            editing: false,
            typed_dir: String::new(),
            replacing: None,
            pending: false,
            fresh: false,
            kind: 0,
            storage: Storage::default(),
            last: None,
        }
    }

    fn at(&self, offset: i32) -> i32 {
        self.base + offset
    }

    fn tool_id(&self, tool: Tool) -> i32 {
        let i = Tool::ALL.iter().position(|t| *t == tool).unwrap_or(0);
        self.at(TOOLS + i as i32)
    }

    fn offer(&self) -> Offer {
        let offers = self.request.offers();
        offers
            .get(self.kind)
            .or(offers.first())
            .cloned()
            .unwrap_or_else(|| Offer::of(ALL_FILES, Vec::new()))
    }

    /// The extensions the chooser lets through.
    fn filter(&self) -> Vec<String> {
        match self.request.mode {
            Mode::Folder => vec![FOLDERS_ONLY.into()],
            Mode::Open | Mode::Save => self.offer().extensions,
        }
    }

    /// `name` from the root when it is a path from there, else in the folder
    /// shown.
    fn resolve(&self, name: &str) -> String {
        if std::path::Path::new(name).has_root() || self.dir.is_empty() || self.dir == START {
            return name.to_string();
        }
        files::join(&self.dir, name)
    }

    /// **The path the name field says**: from the root when it is one, else
    /// in the folder shown; `None` with no name.
    fn typed_path(&self) -> Option<String> {
        let name = self.typed.trim();
        (!name.is_empty()).then(|| self.resolve(name))
    }

    fn says(&self) -> Value {
        json!({"text": self.storage.says, "shown": !self.storage.says.is_empty()})
    }

    fn say(&mut self, says: &str) -> Answer {
        self.storage.says = says.into();
        Answer::set(self.at(SAYS), self.says())
    }

    /// The stem of the name field's value, as its selection: a save's name,
    /// which what is typed then replaces.
    fn stem(&self) -> Value {
        json!({"select": [0, stem_len(&self.typed)]})
    }

    /// **Where it opens**: a save in the folder of the file it writes, that
    /// file's name typed; an open and a folder where the hand last went, else
    /// where the caller said.
    fn start(&mut self) {
        let given = self.request.path.trim().to_string();
        self.typed.clear();
        let (dir, name) = split(&given);
        match self.request.mode {
            Mode::Save if !given.is_empty() => {
                if !dir.is_empty() {
                    self.dir = dir;
                } else if let Some(last) = &self.last {
                    self.dir = last.clone();
                }
                self.typed = name;
            }
            _ => {
                self.dir = self
                    .last
                    .clone()
                    .or_else(|| (!given.is_empty()).then_some(given))
                    .unwrap_or_else(|| START.into());
            }
        }
    }

    /// **The chooser is in folder `to`**, gone there by the hand or asked:
    /// what the last one held is gone, and the bar and the way back say
    /// where it is.
    fn moved(&mut self, to: String, answer: &mut Answer) {
        self.dir = to.clone();
        self.last = Some(to.clone());
        self.names = None;
        self.selected.clear();
        self.disarm(answer);
        answer.push(self.at(BAR), json!({"path": to}));
        self.ways(answer);
    }

    /// **The hand goes to folder `to`**: the folder left is the way back, and
    /// the chooser is asked to list `to`.
    fn go(&mut self, to: String, answer: &mut Answer) {
        if to != self.dir {
            self.back.push(self.dir.clone());
            self.forward.clear();
        }
        answer.push(self.at(FILES), json!({"path": to}));
        self.moved(to, answer);
    }

    /// Back, forward and up, enabled where they go somewhere.
    fn ways(&self, answer: &mut Answer) {
        answer.push(self.at(BACK), json!({"enabled": !self.back.is_empty()}));
        answer.push(
            self.at(FORWARD),
            json!({"enabled": !self.forward.is_empty()}),
        );
        answer.push(
            self.at(UP),
            json!({"enabled": files::parent(&self.dir).is_some()}),
        );
    }

    /// **Back or forward**: the folder the hand left, the one it leaves the
    /// way it did not go.
    fn step(&mut self, forward: bool) -> Answer {
        let mut answer = Answer::default();
        let (from, to) = if forward {
            (&mut self.forward, &mut self.back)
        } else {
            (&mut self.back, &mut self.forward)
        };
        let Some(dir) = from.pop() else {
            return answer;
        };
        to.push(self.dir.clone());
        answer.push(self.at(FILES), json!({"path": dir}));
        self.moved(dir, &mut answer);
        answer
    }

    /// **The path given to the field**, or the bar back, the keys going where
    /// they were.
    fn edit(&mut self, on: bool) -> Answer {
        self.editing = on;
        let mut answer = Answer::set(self.at(PLACE), json!({"index": i32::from(on)}));
        if on {
            self.typed_dir = self.dir.clone();
            let end = self.typed_dir.chars().count();
            answer.push(self.at(PATH), json!({"value": self.typed_dir}));
            answer.push(self.at(PATH), json!({"select": [0, end], "focus": 1}));
        } else {
            answer.push(self.at(self.keys()), json!({"focus": 1}));
        }
        answer
    }

    /// Where the keys go in it: a save's name, an open's files.
    fn keys(&self) -> i32 {
        if self.request.mode == Mode::Save {
            NAME
        } else {
            FILES
        }
    }

    /// **The question about replacing a file, taken back**: the name, the
    /// kind or the folder changed.
    fn disarm(&mut self, answer: &mut Answer) {
        if self.replacing.take().is_some() {
            self.storage.says.clear();
            answer.push(self.at(SAYS), self.says());
            answer.push(self.at(ACCEPT), json!({"label": self.request.accept}));
        }
    }

    /// **Accepted**: the paths chosen, or why there are none.
    fn accept(&mut self) -> Answer {
        if self.editing {
            return self.go_typed();
        }
        let paths = match self.request.mode {
            Mode::Open => match self.typed_path() {
                Some(typed) if !self.selected.contains(&typed) => vec![typed],
                _ => self.selected.clone(),
            },
            Mode::Save => match self.typed_path() {
                Some(typed) => return self.save(self.offer().named(&typed)),
                None => Vec::new(),
            },
            Mode::Folder => vec![self.typed_path().unwrap_or_else(|| self.dir.clone())],
        };
        if paths.is_empty() {
            return self.say(match self.request.mode {
                Mode::Save => "Type a file's name first.",
                _ => "Select a file first.",
            });
        }
        self.chosen(paths)
    }

    /// **The folder typed in the path field, gone to**, and the bar back.
    fn go_typed(&mut self) -> Answer {
        let to = self.typed_dir.trim().to_string();
        let mut answer = self.edit(false);
        if !to.is_empty() {
            let to = self.resolve(&to);
            self.go(to, &mut answer);
        }
        answer
    }

    /// **A save to `path`**: written where its folder holds no file by its
    /// name; else asked once, and written on the second accept. A folder
    /// other than the one shown is gone to first, the name kept, and the
    /// save decided once it is listed.
    fn save(&mut self, path: String) -> Answer {
        if self.replacing.as_ref() == Some(&path) {
            return self.chosen(vec![path]);
        }
        let (dir, name) = split(&path);
        let mut answer = Answer::default();
        if !dir.is_empty() && dir != self.dir {
            self.typed = name;
            answer.push(self.at(NAME), json!({"value": self.typed}));
            self.go(dir, &mut answer);
            self.pending = true;
            return answer;
        }
        if self.names.is_none() {
            self.pending = true;
            return answer;
        }
        self.decide(path)
    }

    /// **A save to `path` in the folder shown, once it is listed**: written
    /// where it holds no file by that name, else asked about.
    fn decide(&mut self, path: String) -> Answer {
        let (_, name) = split(&path);
        let mut answer = Answer::default();
        match &self.names {
            None => self.chosen(vec![path]),
            Some(names) if names.contains(&name) => {
                self.storage.says = format!("{name} already exists. Replace it?");
                self.replacing = Some(path);
                answer.push(self.at(SAYS), self.says());
                answer.push(self.at(ACCEPT), json!({"label": "Replace"}));
                answer
            }
            Some(_) => self.chosen(vec![path]),
        }
    }

    /// **Chosen**: the dialog goes, and the caller hears the kind and the
    /// paths.
    fn chosen(&mut self, paths: Vec<String>) -> Answer {
        let kind = match self.request.mode {
            Mode::Folder => String::new(),
            _ => self.offer().name,
        };
        let mut answer = self.close();
        answer.report = Some(
            [OscType::String("chosen".into()), OscType::String(kind)]
                .into_iter()
                .chain(paths.into_iter().map(OscType::String))
                .collect(),
        );
        answer
    }

    /// **A tool pressed**: the change the chooser is asked for, and what
    /// the tools now say.
    fn tool(&mut self, tool: Tool) -> Answer {
        let typed = self.typed_path();
        let selected = self.selected.last().cloned();
        let pressed = self
            .storage
            .press(tool, &self.dir, selected.as_deref(), typed.as_deref());
        let mut answer = Answer::default();
        self.replacing = None;
        answer.push(self.at(ACCEPT), json!({"label": self.request.accept}));
        if let Some(to) = pressed.go {
            self.go(to, &mut answer);
        }
        if let Some(change) = pressed.change {
            self.selected.clear();
            answer.push(self.at(FILES), json!({"change": change}));
        }
        if pressed.clear_name {
            self.typed.clear();
            answer.push(self.at(NAME), json!({"value": ""}));
        }
        answer.push(self.at(SAYS), self.says());
        answer
    }

    /// **The row over the chooser**: back, forward, up, where the folder is
    /// (its bar, or the field its path is typed in), and the button that
    /// gives the path to the field.
    fn nav(&self) -> Value {
        let way = |id: i32, icon: &str, tip: &str, enabled: bool| {
            json!({"id": self.at(id), "type": "button", "icon": icon, "tip": tip,
                "variant": "ghost", "size": "sm", "enabled": enabled})
        };
        let mut field = json!({"type": "text", "id": self.at(PATH), "value": self.typed_dir});
        if self.editing && self.fresh {
            field["select"] = json!([0, self.typed_dir.chars().count()]);
        }
        json!({"type": "layout", "flow": "row", "pack": 1, "hug": 1, "margin": 0,
            "gap": NAV_GAP, "w": CHOOSER_W, "children": [
            way(BACK, "arrow-left", "Back", !self.back.is_empty()),
            way(FORWARD, "arrow-right", "Forward", !self.forward.is_empty()),
            way(UP, "chevron-up", "Up", files::parent(&self.dir).is_some()),
            {"type": "layout", "id": self.at(PLACE), "flow": "stack", "weight": 1, "margin": 0,
                "index": i32::from(self.editing), "children": [
                {"type": "path_bar", "id": self.at(BAR), "path": self.dir, "root": root_name()},
                field,
            ]},
            {"id": self.at(TYPE_PATH), "type": "button", "label": "Type a path",
                "variant": "ghost", "size": "sm", "chord": TYPE_PATH_CHORD,
                "tip": TYPE_PATH_CHORD},
        ]})
    }

    /// **The dialog's widgets**, as it stands now.
    fn dialog(&self) -> Value {
        let row = |caption: &str, field: Value, w: Option<f64>| {
            let mut children = Vec::new();
            if !caption.is_empty() {
                children.push(json!({"type": "label", "text": caption, "variant": "caption"}));
            }
            children.push(field);
            let mut row = json!({"type": "layout", "flow": "col", "hug": true, "margin": 0,
                "gap": CAPTION_GAP, "children": children});
            if let (Some(w), Some(map)) = (w, row.as_object_mut()) {
                map.insert("w".into(), json!(w));
            }
            row
        };
        let files = json!({"type": "files", "id": self.at(FILES), "path": self.dir,
            "filter": self.filter(), "multiple": self.request.multiple, "weight": 1,
            "h": CHOOSER_H, "bind": ["widget", self.at(NAME), "value"]});
        let tools: Vec<Value> = Tool::ALL
            .into_iter()
            .filter(|tool| tool.shown())
            .map(|tool| {
                json!({"id": self.tool_id(tool), "type": "button", "label": tool.label(),
                    "variant": "ghost", "size": "sm"})
            })
            .collect();
        let mut name = json!({"type": "text", "id": self.at(NAME), "value": self.typed});
        if self.up && self.fresh && self.request.mode == Mode::Save {
            name["select"] = self.stem()["select"].clone();
        }
        let caption = match self.request.mode {
            Mode::Folder => "Folder",
            Mode::Open | Mode::Save => "Name",
        };
        let mut rows = vec![
            self.nav(),
            row("", files, Some(CHOOSER_W)),
            json!({"type": "layout", "flow": "row", "pack": 1, "hug": 1, "margin": 0,
                "gap": 4, "children": tools}),
            row(caption, name, None),
        ];
        if self.request.mode != Mode::Folder {
            let labels: Vec<String> = self.request.offers().into_iter().map(|o| o.label).collect();
            let kinds = json!({"type": "choice", "id": self.at(TYPE), "options": labels,
                "index": self.kind});
            rows.push(row("File type", kinds, Some(CHOOSER_W)));
        }
        rows.push(
            json!({"type": "label", "id": self.at(SAYS), "text": self.storage.says,
            "wrap": 1, "muted": 1, "shown": !self.storage.says.is_empty()}),
        );
        let accept = if self.replacing.is_some() {
            "Replace"
        } else {
            self.request.accept.as_str()
        };
        let footer = json!({"type": "layout", "flow": "row", "hug": true, "pack": true,
        "margin": 0, "gap": FOOTER_GAP, "children": [
            {"type": "separator", "weight": 1, "line": false},
            {"type": "button", "id": self.at(CANCEL), "label": "Cancel", "variant": "outline"},
            {"type": "button", "id": self.at(ACCEPT), "label": accept,
                "default": true, "variant": "default"},
        ]});
        json!({"type": "layout", "id": self.at(DIALOG), "modal": true, "hug": true,
        "title": self.request.title, "flow": "col", "children": [
            {"type": "layout", "flow": "col", "hug": true, "margin": 0, "gap": BODY_GAP,
                "children": rows},
            footer,
        ]})
    }
}

/// How many characters of `name` are its stem: all but its last extension,
/// and all of a name that has none (or only starts with a dot).
fn stem_len(name: &str) -> usize {
    match name.rfind('.') {
        Some(dot) if dot > 0 => name[..dot].chars().count(),
        _ => name.chars().count(),
    }
}

/// `path` as its folder and its name; a path with no folder is all name.
fn split(path: &str) -> (String, String) {
    let p = std::path::Path::new(path);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = p
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    (dir, name)
}

/// Numbers every node of `tree` that has no id, from `next`.
fn number(tree: &mut Value, next: &mut i32) {
    if let Some(node) = tree.as_object_mut() {
        if !node.contains_key("id") {
            node.insert("id".into(), json!(*next));
            *next += 1;
        }
        if let Some(Value::Array(children)) = node.get_mut("children") {
            for child in children {
                number(child, next);
            }
        }
    }
}

/// The number an event's first argument or its tag carries: a choice's
/// index.
fn index_of(tag: &str, args: &[OscType]) -> Option<usize> {
    let n = match args.first() {
        Some(OscType::Int(v)) => f64::from(*v),
        Some(OscType::Long(v)) => *v as f64,
        Some(OscType::Float(v)) => f64::from(*v),
        Some(OscType::Double(v)) => *v,
        _ => tag.parse().ok()?,
    };
    (n >= 0.0).then(|| n.round() as usize)
}

/// The paths an event carries after its tag.
fn paths(args: &[OscType]) -> Vec<String> {
    args.iter()
        .filter_map(|a| match a {
            OscType::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

impl Presenter for Drawn {
    fn stand_in(&self) -> Value {
        let mut dialog = self.dialog();
        let mut next = self.at(SPARE);
        number(&mut dialog, &mut next);
        let mut empty = json!({"type": "layout"});
        number(&mut empty, &mut next);
        let mut page = json!({"type": "layout", "children": [dialog]});
        number(&mut page, &mut next);
        debug_assert!(
            next <= self.at(IDS as i32),
            "a drawn dialog outgrew its ids"
        );
        json!({"type": "layout", "id": self.id, "flow": "stack", "index": i32::from(self.up),
            "margin": 0, "h": 0, "children": [empty, page]})
    }

    fn update(&mut self, request: Request) {
        if request != self.request {
            self.request = request;
            self.kind = self.kind.min(self.request.offers().len().saturating_sub(1));
        }
    }

    fn open(&mut self) -> Answer {
        self.up = true;
        self.fresh = true;
        self.start();
        self.selected.clear();
        self.names = None;
        self.back.clear();
        self.forward.clear();
        self.editing = false;
        self.replacing = None;
        self.pending = false;
        self.kind = 0;
        self.storage.reset();
        let labels: Vec<String> = self.request.offers().into_iter().map(|o| o.label).collect();
        let mut answer = Answer::default();
        answer.push(self.at(SAYS), self.says());
        // the keys go where the hand starts: a save's name, its stem
        // selected, and an open's files
        answer.push(self.at(NAME), json!({"value": self.typed}));
        if self.request.mode == Mode::Save {
            // after the value, which it is a selection of
            answer.push(self.at(NAME), self.stem());
        }
        if self.request.mode != Mode::Folder {
            answer.push(self.at(TYPE), json!({"options": labels, "index": 0}));
        }
        answer.push(
            self.at(FILES),
            json!({"path": self.dir, "filter": self.filter()}),
        );
        answer.push(self.at(PLACE), json!({"index": 0}));
        answer.push(self.at(BAR), json!({"path": self.dir}));
        self.ways(&mut answer);
        answer.push(self.at(DIALOG), json!({"title": self.request.title}));
        answer.push(self.at(ACCEPT), json!({"label": self.request.accept}));
        answer.push(self.id, json!({"index": 1}));
        answer.push(self.at(self.keys()), json!({"focus": 1}));
        answer
    }

    fn close(&mut self) -> Answer {
        self.up = false;
        self.selected.clear();
        self.editing = false;
        self.replacing = None;
        self.pending = false;
        Answer::set(self.id, json!({"index": 0}))
    }

    fn is_up(&self) -> bool {
        self.up
    }

    fn owns(&self, id: i32) -> bool {
        (self.base..self.base + IDS as i32).contains(&id)
    }

    fn event(&mut self, id: i32, tag: &str, args: &[OscType]) -> Answer {
        if !self.up {
            return Answer::default();
        }
        self.fresh = false;
        let saves = self.request.mode == Mode::Save;
        match id - self.base {
            // Escape while the path is typed gives the bar back
            DIALOG if tag == "cancel" && self.editing => self.edit(false),
            DIALOG if tag == "cancel" => self.cancel(),
            CANCEL if tag == "click" => self.cancel(),
            ACCEPT if tag == "click" => self.accept(),
            BACK if tag == "click" => self.step(false),
            FORWARD if tag == "click" => self.step(true),
            UP if tag == "click" => {
                let mut answer = Answer::default();
                if let Some(up) = files::parent(&self.dir) {
                    self.go(up, &mut answer);
                }
                answer
            }
            TYPE_PATH if tag == "click" => self.edit(!self.editing),
            BAR => match (tag, paths(args).into_iter().next()) {
                (GO, Some(to)) => {
                    let mut answer = Answer::default();
                    self.go(to, &mut answer);
                    answer
                }
                (EDIT, _) => self.edit(true),
                _ => Answer::default(),
            },
            PATH if args.is_empty() && tag != "focus" => {
                self.typed_dir = tag.to_string();
                Answer::default()
            }
            FILES => match tag {
                "select" => {
                    self.selected = paths(args);
                    if let Some(last) = self.selected.last() {
                        self.typed = last.clone();
                    }
                    let mut answer = Answer::default();
                    self.disarm(&mut answer);
                    answer
                }
                "pick" => {
                    let picked = paths(args);
                    match self.request.mode {
                        Mode::Save => {
                            if let Some(first) = picked.first() {
                                self.typed = first.clone();
                            }
                            self.accept()
                        }
                        Mode::Open => {
                            self.selected = picked;
                            self.typed.clear();
                            self.accept()
                        }
                        Mode::Folder => Answer::default(),
                    }
                }
                "path" => {
                    let Some(dir) = paths(args).into_iter().next() else {
                        return Answer::default();
                    };
                    // the hand went there in the chooser: the way back is
                    // where it was
                    if dir != self.dir {
                        self.back.push(self.dir.clone());
                        self.forward.clear();
                    }
                    // a file chosen in a folder left behind is not what an
                    // open opens; a save's name is the file it writes
                    let forget = !saves && self.selected.contains(&self.typed);
                    let mut answer = Answer::default();
                    self.moved(dir, &mut answer);
                    if forget {
                        self.typed.clear();
                        answer.push(self.at(NAME), json!({"value": ""}));
                    }
                    answer
                }
                "changed" => {
                    let why = paths(args).into_iter().next().unwrap_or_default();
                    self.storage.changed(&why);
                    self.selected.clear();
                    Answer::set(self.at(SAYS), self.says())
                }
                // its value: the file selected, one at a time
                path if args.is_empty() && !path.is_empty() => {
                    self.selected = vec![path.to_string()];
                    self.typed = path.to_string();
                    let mut answer = Answer::default();
                    self.disarm(&mut answer);
                    answer
                }
                _ => Answer::default(),
            },
            NAME if args.is_empty() && tag != "focus" => {
                let mut answer = Answer::default();
                if tag != self.typed {
                    self.typed = tag.to_string();
                    self.disarm(&mut answer);
                }
                answer
            }
            TYPE if tag != "focus" => {
                let Some(i) = index_of(tag, args).filter(|i| *i < self.request.offers().len())
                else {
                    return Answer::default();
                };
                self.kind = i;
                let mut answer = Answer::set(self.at(FILES), json!({"filter": self.filter()}));
                self.disarm(&mut answer);
                answer
            }
            offset
                if (TOOLS..TOOLS + Tool::ALL.len() as i32).contains(&offset) && tag == "click" =>
            {
                self.tool(Tool::ALL[(offset - TOOLS) as usize])
            }
            _ => Answer::default(),
        }
    }

    /// **The chooser's folder listed**: where it resolved is the folder
    /// shown, its files are what a save asks about replacing, and a save
    /// that waited for them is decided.
    fn listed(&mut self, id: i32, listing: Result<&Listing, &str>) -> Answer {
        if id != self.at(FILES) || !self.up {
            return Answer::default();
        }
        let mut answer = Answer::default();
        match listing {
            Ok(listing) => {
                if listing.path != self.dir {
                    // as it resolved: the same folder, so no way back to it
                    self.dir = listing.path.clone();
                    self.last = Some(self.dir.clone());
                    answer.push(self.at(BAR), json!({"path": self.dir}));
                    self.ways(&mut answer);
                }
                self.names = Some(
                    listing
                        .entries
                        .iter()
                        .filter(|e| !e.dir)
                        .map(|e| e.name.clone())
                        .collect(),
                );
                if std::mem::take(&mut self.pending)
                    && let Some(typed) = self.typed_path()
                {
                    // decided here, in the folder listed, whatever path it
                    // resolved to
                    let decided = self.decide(self.offer().named(&typed));
                    answer.set.extend(decided.set);
                    answer.report = decided.report;
                }
            }
            Err(why) => {
                self.names = None;
                if std::mem::take(&mut self.pending) {
                    let said = self.say(why);
                    answer.set.extend(said.set);
                }
            }
        }
        answer
    }
}

impl Drawn {
    fn cancel(&mut self) -> Answer {
        let mut answer = self.close();
        answer.report = Some(vec![OscType::String("cancel".into())]);
        answer
    }
}

#[cfg(test)]
mod tests;
