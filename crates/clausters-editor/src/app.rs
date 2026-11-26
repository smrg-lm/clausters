//! **The editor's application**: its files, its session, its panels, its
//! menus and its dialogs, as a client of the host it links.
//!
//! Everything it shows is a window of the host's dock: one per open file (a
//! `code` element, with a dialog of its own for the questions asked about
//! it), the post window, the documentation, the preferences and the
//! environments. It says what to show the way a script does and reads back
//! what the hand did (`wire`), so the host draws and edits and this only
//! decides: what a verb does, what the session is asked, what is saved.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use clausters_core::osc::OscType;
use serde_json::{Value, json};

use clausters_apps::{closing, quitting};

use crate::arrangement;
use crate::docs::Book;
use crate::filedialog::{self, Form, PATH};
use crate::platform::{self, Language, MonoFamily, Offers, Platform, Purpose, Read};
use crate::settings::{Settings, ThemePref};
use crate::wire::{self, Link, Said};
use crate::{config, theme};

/// What reaches the application's thread.
pub enum Msg {
    /// What the host said.
    Host(Said),
    /// One message of the session's driver.
    Py(Value),
    /// The session's process started.
    PyStart {
        generation: u64,
        env: Option<String>,
    },
    /// The session's process ended.
    PyExit { generation: u64, code: Option<i32> },
    /// An environment operation ended.
    EnvsDone(Result<(), String>),
    /// The documentation, read.
    Docs(Result<Docs, String>),
    /// A file dialog answered.
    Chosen {
        purpose: Purpose,
        path: Option<PathBuf>,
    },
    /// The desktop's preference: dark or not.
    SystemDark(bool),
    /// Time to ask the dock again whether every pane has arrived.
    Arrange,
    /// The system's monospaced families, found.
    Fonts(Vec<MonoFamily>),
    /// A file read for a tab, or why it could not be.
    Read {
        path: PathBuf,
        why: Read,
        text: Result<String, String>,
    },
    /// The save of the code `code` written to `path`, or why it was not.
    Written {
        path: PathBuf,
        code: i32,
        done: Result<(), String>,
    },
}

/// Every book and every document.
/// Every book, every document, and the folder each book's documents were
/// read from, by the prefix their paths start with (where a picture a page
/// names is read from).
pub type Docs = (
    Vec<Book>,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
);

/// **The name of the editor's page in the dock**, the code editor's short
/// name beside the applications' (`Roll`, `Score`, `Audio`, `Tracks`): the
/// editor names the page its panes are on, which the host opens under a
/// name of its own for every dock.
const PAGE: &str = "Code";

// -- the fixed windows, and the ids of what is inside them

/// **Where the editor's ids start**: past the window a client allocates its
/// widgets from (1000 and a million after it) and the ids below 1000 a
/// script picks by hand, since the session's windows open on this host too.
const IDS: i32 = 1 << 24;

const POST_WIN: i32 = IDS + 1;
const POST: i32 = IDS + 2;
const DOCS_WIN: i32 = IDS + 3;
const DOCS: i32 = IDS + 4;
/// The application's bar, along the dock's top: the menu and the tools.
const BAR_WIN: i32 = IDS + 5;
/// The bar's height, in logical pixels.
const BAR_H: i32 = 40;
/// The side of one of the bar's tools, in logical pixels.
const TOOL: i32 = 32;
/// The application's footer, along the dock's bottom: the session's state.
const FOOT_WIN: i32 = IDS + 11;
const FOOT: i32 = IDS + 12;
/// The footer's height, in logical pixels.
const FOOT_H: i32 = 24;
/// The footer's text size: 12 pixels to the em of the interface's face
/// (Inter, whose capital is 0.727 of the em, over the 7-pixel cell).
const FOOT_TEXT: f64 = 12.0 * 0.727 / 7.0;

/// The line between the bar's menu titles and its tools.
const BAR_LINE: i32 = IDS + 18;
/// The room that pushes the applications' buttons to the bar's end.
const BAR_SPACE: i32 = IDS + 13;
/// **The applications the bar opens**, one button each at its end, each a
/// new page holding a new, empty one: the button's id, the application's
/// name as the host opens it (`/gui_app_open`), its icon and its tip.
const APPS: [(i32, &str, &str, &str); 4] = [
    (IDS + 14, "multitrack", "layers", "New multitrack"),
    (IDS + 15, "score", "music", "New score"),
    (IDS + 16, "notes", "keyboard-music", "New notes"),
    (IDS + 17, "audio", "audio-waveform", "New audio"),
];
/// Where the bar holds the dialog that is up: the preferences or the
/// environments, over the whole dock.
const DIALOG_HOLD: i32 = IDS + 19;
/// The preferences' dialog.
const PREFS_DIALOG: i32 = IDS + 20;
const PREFS_REOPEN: i32 = IDS + 22;
const PREFS_INDENT: i32 = IDS + 23;
const PREFS_THEME: i32 = IDS + 24;
const PREFS_DOCS_DIR: i32 = IDS + 25;
const PREFS_DOCS_CHOOSE: i32 = IDS + 27;
const PREFS_DOCS_BUNDLED: i32 = IDS + 28;
const PREFS_STARTUP: i32 = IDS + 30;
const PREFS_SAVE: i32 = IDS + 32;
const PREFS_SAVE_RESTART: i32 = IDS + 33;
const PREFS_CLOSE: i32 = IDS + 35;

/// The environments' dialog.
const ENVS_DIALOG: i32 = IDS + 40;
const ENVS_LIST: i32 = IDS + 42;
const ENVS_SOURCE: i32 = IDS + 71;
const ENVS_NAME: i32 = IDS + 72;
const ENVS_CREATE: i32 = IDS + 73;
const ENVS_STATUS: i32 = IDS + 74;
const ENVS_CLOSE: i32 = IDS + 75;
/// The rows of the list, redefined whole: ten ids a row from here.
const ENVS_ROWS: i32 = IDS + 300;
/// Where the nodes of the two dialogs with no role of their own are
/// numbered from.
const PREFS_SPARE: i32 = IDS + 100;
const ENVS_SPARE: i32 = IDS + 150;

/// A file window's ids, from its base: the window, its column, the code,
/// the stack its dialog is a page of, the empty page, the dialog, its text,
/// its row of buttons and the two buttons.
const FILE_BASE: i32 = IDS + 1000;
const FILE_STRIDE: i32 = 16;
/// Where a file window's nodes with no role of their own are numbered from.
const FILE_SPARE: i32 = 10;
const CODE: i32 = 2;

/// **The close form of the file window at `win`**, the one every
/// application asks in (`clausters_apps::closing`).
fn form(win: i32) -> closing::Ids {
    closing::Ids {
        stack: win + 3,
        dialog: win + 5,
        save: win + 6,
        discard: win + 7,
        cancel: win + 8,
    }
}

/// One open file: a window of the dock holding a `code` element.
struct File {
    win: i32,
    path: Option<PathBuf>,
    name: String,
    dirty: bool,
}

impl File {
    fn code(&self) -> i32 {
        self.win + CODE
    }
    /// What its tab says of it: its name, its changes not kept (a dot on
    /// the tab) and its path (the tab's tip).
    fn tab(&self) -> [(&'static str, Value); 3] {
        [
            ("title", json!(self.name)),
            ("modified", json!(i32::from(self.dirty))),
            (
                "dock_tip",
                json!(
                    self.path
                        .as_ref()
                        .map_or(String::new(), |p| p.to_string_lossy().into_owned())
                ),
            ),
        ]
    }
}

/// What a question asked in a file window's dialog will do if it is
/// answered yes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    CloseTab(i32),
    Quit,
}

/// A question the session was asked, by its request id.
enum Query {
    Complete(i32),
    Signatures(i32),
    Inspect(i32),
    /// Help on a name: inspected first, then pydoc's text.
    HelpInspect {
        text: String,
    },
    HelpText {
        info: Value,
    },
}

/// What a dock call was for.
enum DockFor {
    /// The first arrangement, once every pane is there.
    Arrange,
    /// Split the editor or put it back together.
    Split,
    /// Move the tab on screen to the other editor.
    MoveTab,
    /// The window's size, before the editor ends.
    Quit,
    /// Nothing but the state.
    Look,
    /// What the host's applications would lose, for a quit.
    Unsaved,
    /// The host's applications saved, for a quit.
    SaveAll,
    /// The session's windows freed: it ended, and nothing answers for them.
    Orphans,
}

/// The session's state, as the post window's title says it.
#[derive(Default)]
struct Py {
    alive: bool,
    version: String,
    env: Option<String>,
    generation: u64,
    busy: HashSet<u64>,
}

pub struct App {
    link: Link,
    platform: Box<dyn Platform>,
    /// The session's language, as the platform runs it.
    lang: &'static Language,
    /// What the platform has beside what every one has.
    offers: Offers,
    settings: Settings,
    files: Vec<File>,
    /// The window of the file the menus and the keys act on: the one whose
    /// code had the keyboard last.
    active: Option<i32>,
    untitled: u32,
    next_base: i32,
    py: Py,
    next_request: u64,
    queries: HashMap<u64, Query>,
    /// Text read back for a save, by the code element asked.
    saving: HashMap<i32, PathBuf>,
    /// The question up in a file window's close form.
    asking: Option<(i32, Ask)>,
    /// What a file's save goes on to once it is written.
    after_save: HashMap<i32, Ask>,
    docs: Option<Docs>,
    docs_shown: bool,
    /// **Whether the bar shows the editor's own menu**: on an
    /// application's page it shows that application's, and the session's
    /// tools, which are entries of the editor's menu, go with it.
    bar_own: bool,
    /// **The applications the bar's buttons opened**: the editor's, not
    /// the session's, so a restart leaves them open.
    apps: Vec<i32>,
    system_dark: bool,
    dock_calls: HashMap<i32, DockFor>,
    /// **A quit asked**: whether the host's applications hold work it would
    /// lose, which a Save all saves after the editor's own files.
    quit_host: bool,
    prefs_open: bool,
    envs_open: bool,
    /// The fields' values as last typed.
    fields: HashMap<i32, String>,
    /// A help asked before the documentation was read.
    help_waiting: Option<Value>,
    quitting: bool,
    /// The startup code is being read back to be saved, and whether the
    /// session restarts after.
    saving_startup: Option<bool>,
    /// Whether the files are in more than one group, as the dock last said.
    split: bool,
    /// The system's monospaced families, once found.
    fonts: Vec<MonoFamily>,
    /// **The files the start opens, still being read**: once the last is
    /// in, the start goes on ([`Self::started`]). `None` once it has.
    opening: Option<usize>,
    /// Whether a file the start asked for opened.
    opened: bool,
    /// What the file dialog up was opened for.
    file_dialog: Option<Purpose>,
    /// **Its Open and Save as**, every application's forms
    /// ([`clausters_apps::forms`]).
    dialogs: clausters_apps::forms::Dialogs<Form>,
}

impl App {
    pub fn new(link: Link, platform: Box<dyn Platform>, settings: Settings) -> Self {
        Self {
            link,
            lang: platform.language(),
            offers: platform.offers(),
            platform,
            settings,
            files: Vec::new(),
            active: None,
            untitled: 0,
            next_base: FILE_BASE,
            py: Py::default(),
            next_request: 1,
            queries: HashMap::new(),
            saving: HashMap::new(),
            asking: None,
            after_save: HashMap::new(),
            docs: None,
            docs_shown: true,
            bar_own: true,
            apps: Vec::new(),
            system_dark: false,
            dock_calls: HashMap::new(),
            quit_host: false,
            prefs_open: false,
            envs_open: false,
            fields: HashMap::new(),
            help_waiting: None,
            quitting: false,
            saving_startup: None,
            split: false,
            fonts: Vec::new(),
            opening: None,
            opened: false,
            file_dialog: None,
            dialogs: filedialog::dialogs(),
        }
    }

    /// **The application's turn**: what is open at start, then every
    /// message as it comes, until the editor ends.
    pub fn run(mut self, rx: Receiver<Msg>, files: Vec<PathBuf>) {
        self.start(files);
        while let Ok(msg) = rx.recv() {
            self.on(msg);
        }
    }

    /// Whether a file has changes that are not saved.
    pub fn unsaved(&self) -> bool {
        self.files.iter().any(|f| f.dirty)
    }

    /// **The editor starts**: its panels, its look, and the files of the
    /// last run and then the ones `cli` names, read before the start goes
    /// on (`started`).
    pub fn start(&mut self, cli: Vec<PathBuf>) {
        // the dock's close is asked of the editor, once, as a quit
        if self.offers.quit {
            self.link.dock(&json!({"verb": "own_quit"}));
        }
        self.apply_theme();
        if self.offers.fonts {
            self.platform.find_fonts();
        }
        self.platform.follow_color_scheme();
        self.platform.read_docs();
        self.open_bar();
        self.open_footer();
        self.open_post();
        self.open_docs();
        // the files of the last run, then the ones the command line names,
        // the last of them on screen
        let mut reading = 0;
        if self.settings.reopen_files {
            for path in self.settings.open_files.clone() {
                reading += usize::from(self.open_path(Path::new(&path), Read::Reopen));
            }
        }
        for path in cli {
            if self.platform.exists(&path) {
                reading += usize::from(self.open_path(&path, Read::Open));
            } else {
                self.open_missing(&path);
                self.opened = true;
            }
        }
        self.opening = Some(reading);
        if reading == 0 {
            self.started();
        }
    }

    /// **The start, once its files are read**: an empty tab if none opened,
    /// the one the last run left on screen shown, the panels arranged and
    /// the session started.
    fn started(&mut self) {
        self.opening = None;
        if !self.opened && self.files.is_empty() {
            self.new_file();
        }
        if let Some(active) = self.settings.active_file.clone()
            && self.settings.reopen_files
            && let Some(win) = self.file_at(Path::new(&active)).map(|f| f.win)
        {
            self.show_file(win);
        }
        self.dock(json!({"verb": "layout"}), DockFor::Arrange);
        if let Err(e) = self.platform.start_session() {
            self.post("err", &format!("{e}\n"));
        }
    }

    /// **One message, on the application's turn**: what the host said, the
    /// session, or a platform's answer.
    pub fn on(&mut self, msg: Msg) {
        match msg {
            Msg::Host(said) => self.on_host(said),
            Msg::Py(m) => self.on_py(m),
            Msg::PyStart { generation, env } => {
                self.py.generation = generation;
                self.py.env = env;
                self.py.busy.clear();
                self.platform.read_docs();
                self.say_session();
            }
            Msg::PyExit { generation, code } => {
                if generation != self.py.generation {
                    return;
                }
                self.py.alive = false;
                self.py.busy.clear();
                let code = code.map_or_else(|| "?".to_string(), |c| c.to_string());
                let says = format!("{} session ended (code {code})\n", self.lang.name);
                self.post("info", &says);
                self.say_session();
                self.free_session_windows();
            }
            Msg::EnvsDone(result) => {
                let status = match result {
                    Ok(()) => String::new(),
                    Err(e) => {
                        self.post("info", &format!("{e}\n"));
                        e
                    }
                };
                self.envs_refresh(&status);
            }
            Msg::Docs(Ok(docs)) => {
                self.docs = Some(docs);
                self.feed_docs();
                if let Some(help) = self.help_waiting.take() {
                    self.show_help(help);
                }
            }
            Msg::Docs(Err(e)) => self.post("info", &format!("{e}\n")),
            Msg::Chosen { purpose, path } => self.chosen(purpose, path),
            Msg::Arrange => self.dock(json!({"verb": "layout"}), DockFor::Arrange),
            Msg::Fonts(fonts) => {
                self.fonts = fonts;
                if self.settings.font.is_some() {
                    self.apply_font();
                }
                self.menus();
            }
            Msg::SystemDark(dark) => {
                self.system_dark = dark;
                if self.settings.theme == ThemePref::System {
                    self.apply_theme();
                }
            }
            Msg::Read { path, why, text } => self.on_read(path, why, text),
            Msg::Written { path, code, done } => self.on_written(path, code, done),
        }
    }

    // -- what the host said

    fn on_host(&mut self, said: Said) {
        match said {
            Said::Event { id, tag, args } => self.on_event(id, &tag, &args),
            Said::Closed(id) => self.on_closed(id),
            Said::Dock { seq, reply } => self.on_dock(seq, &reply),
            Said::DockQuit(asked) => self.quit_over(&asked),
            // an application the bar could not open says why
            Said::App { reply } => {
                if let Some(message) = reply["error"]["message"].as_str() {
                    self.post("info", &format!("{message}\n"));
                }
                if let Some(window) = reply["ok"]["window"].as_i64() {
                    self.apps.extend(i32::try_from(window).ok());
                }
            }
            Said::Info { id, props } => self.on_info(id, &props),
            Said::Other => {}
        }
    }

    fn on_event(&mut self, id: i32, tag: &str, args: &[OscType]) {
        // a window's own: a close, a menu pick, a verb no element took
        if self.is_window(id) {
            return match tag {
                "close" => self.close_pane(id),
                "menu_from" if id == BAR_WIN => self.bar_shows(wire::int(args, 0)),
                "menu" => self.verb(id, &wire::text(args, 0), args.get(1).is_some_and(on)),
                "save" => self.save(self.file_win(id), false),
                verb if !verb.is_empty() => self.verb(id, verb, true),
                _ => {}
            };
        }
        match id {
            POST | DOCS => match tag {
                "link" => self.platform.open_url(&wire::text(args, 0)),
                // the post window's own tool, kept between runs
                "autoscroll" => {
                    self.settings.autoscroll = args.first().is_some_and(on);
                    self.settings.save();
                }
                _ => {}
            },
            PREFS_REOPEN => {
                self.settings.reopen_files = args.first().is_some_and(on);
                self.settings.save();
            }
            PREFS_INDENT => {
                self.settings.indent =
                    ["tab", "2", "4"][wire::int(args, 0).clamp(0, 2) as usize].into();
                self.settings.save();
                let indent = self.settings.indent_prop();
                self.set_codes("indent", &indent);
            }
            PREFS_THEME => {
                let verb = ["theme_light", "theme_dark", "theme_system"]
                    [wire::int(args, 0).clamp(0, 2) as usize];
                self.verb(BAR_WIN, verb, true);
            }
            PREFS_DOCS_CHOOSE if tag == "click" => self.choose_docs_folder(),
            PREFS_DOCS_BUNDLED if tag == "click" => self.set_docs_dir(None),
            PREFS_SAVE if tag == "click" => self.ask_startup_code(false),
            PREFS_SAVE_RESTART if tag == "click" => self.ask_startup_code(true),
            PREFS_CLOSE if tag == "click" => self.close_dialog(),
            // the close button, Escape and the scrim ask a dialog to go
            PREFS_DIALOG | ENVS_DIALOG if tag == "cancel" => self.close_dialog(),
            ENVS_SOURCE | ENVS_NAME => {
                if let Some(text) = wire::typed(tag, args) {
                    self.fields.insert(id, text);
                }
            }
            ENVS_CREATE if tag == "click" => self.envs_create(),
            id if filedialog::owns(id) => self.on_file_dialog(id, tag, args),
            ENVS_CLOSE if tag == "click" => self.close_dialog(),
            id if (ENVS_ROWS..ENVS_ROWS + 500).contains(&id) && tag == "click" => self.envs_row(id),
            id if id >= FILE_BASE => self.on_file_event(id, tag, args),
            _ => {}
        }
    }

    /// An event from inside a file window: its code, or its dialog.
    fn on_file_event(&mut self, id: i32, tag: &str, args: &[OscType]) {
        let win = id - (id - FILE_BASE) % FILE_STRIDE;
        if !self.files.iter().any(|f| f.win == win) {
            return;
        }
        match (id - win, tag) {
            (CODE, "focus") if args.first().is_some_and(on) => self.active = Some(win),
            (CODE, "dirty") => {
                let dirty = args.first().is_some_and(on);
                if let Some(f) = self.files.iter_mut().find(|f| f.win == win) {
                    f.dirty = dirty;
                    self.link.set(win, &f.tab());
                }
            }
            (CODE, "evaluate") => {
                let code = wire::text(args, 0);
                let line = wire::int(args, 1);
                self.evaluate(win, &code, line);
            }
            (CODE, "interrupt") => self.interrupt(),
            (CODE, "restart") => self.restart(),
            (CODE, "complete") => {
                let request = self.at(win, args);
                self.query("complete", request, Query::Complete(id));
            }
            (CODE, "signatures") => {
                let request = self.at(win, args);
                self.query("signatures", request, Query::Signatures(id));
            }
            (CODE, "inspect") => {
                let request = self.at(win, args);
                self.query("inspect", request, Query::Inspect(id));
            }
            (CODE, "help") => {
                let text = wire::text(args, 0);
                let request = self.at(win, &args[1.min(args.len())..]);
                self.query("inspect", request, Query::HelpInspect { text });
            }
            _ => {
                if let Some(choice) = form(win).read(i64::from(id), tag) {
                    self.answer(win, choice);
                }
            }
        }
    }

    fn on_closed(&mut self, id: i32) {
        self.apps.retain(|&a| a != id);
        match id {
            // the post window is freed by nobody but the quit: gone, it is the
            // dock's window closed over what was asked, and the editor ends
            POST_WIN => return self.quit_now(),
            // the bar and the footer go with the dock's window, as the post
            // window does
            BAR_WIN | FOOT_WIN => return,
            DOCS_WIN => self.docs_shown = false,
            _ => {
                if let Some(i) = self.files.iter().position(|f| f.win == id) {
                    self.files.remove(i);
                    if self.active == Some(id) {
                        self.active = self.files.last().map(|f| f.win);
                    }
                }
            }
        }
        self.remember_files();
        self.menus();
    }

    /// **A pane's own close**: its tab's close mark or its panel's. The
    /// dock's window is not closed this way: its close is the quit the host
    /// asks for ([`Said::DockQuit`]).
    fn close_pane(&mut self, win: i32) {
        tracing::debug!("close asked of window {win}");
        match win {
            // the post window stays: it is where the session speaks; the
            // bar is the application's, and goes only with it
            BAR_WIN | FOOT_WIN | POST_WIN => {}
            // freed here, the panel is gone: F1 and the menu open it again
            DOCS_WIN => {
                self.link.free(win);
                self.docs_shown = false;
                self.menus();
            }
            win => self.close_tab(Some(win)),
        }
    }

    fn on_dock(&mut self, seq: i32, reply: &Value) {
        let Some(purpose) = self.dock_calls.remove(&seq) else {
            return;
        };
        match purpose {
            DockFor::Unsaved => return self.quit_over(&reply["ok"]),
            DockFor::SaveAll => {
                if reply.get("ok").is_some() {
                    return self.quit_saved();
                }
                let why = reply["error"]["message"].as_str().unwrap_or("not saved");
                self.post("info", &format!("Quit called off: {why}\n"));
                self.link.dock(&json!({"verb": "stay"}));
                return;
            }
            _ => {}
        }
        let Some(state) = reply.get("ok") else {
            // a dock that answered nothing does not keep the editor open
            if matches!(purpose, DockFor::Quit) {
                self.quit_now();
            }
            return;
        };
        if let (Some(size), Some(scale)) = (state.get("size"), state.get("scale")) {
            let scale = scale.as_f64().unwrap_or(1.0).max(0.1);
            let w = size[0].as_f64().unwrap_or(0.0) / scale;
            let h = size[1].as_f64().unwrap_or(0.0) / scale;
            if w >= 1.0 && h >= 1.0 {
                self.settings.window = Some((w.round() as u32, h.round() as u32));
            }
        }
        if let Some([x, y]) = state.get("position").and_then(|p| {
            let p = p.as_array()?;
            Some([p.first()?.as_i64()?, p.get(1)?.as_i64()?])
        }) && let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y))
        {
            self.settings.position = Some((x, y));
        }
        let groups = groups(state);
        let split = self.editor_groups(&groups).len() > 1;
        if split != self.split {
            self.split = split;
            self.menus();
        }
        match purpose {
            DockFor::Arrange => {
                if let Some(call) = page_rename(state) {
                    self.link.dock(&call);
                }
                self.arrange(&groups)
            }
            DockFor::Split => self.split_or_join(&groups),
            DockFor::MoveTab => self.move_tab(&groups),
            DockFor::Quit => {
                self.keep_arrangement(state);
                self.quit_now()
            }
            DockFor::Look | DockFor::Unsaved | DockFor::SaveAll => {}
            DockFor::Orphans => {
                // every window not the editor's own is the session's, on
                // whichever page, but what the bar's buttons opened
                let ids = state["members"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64)
                    .chain(
                        state["out"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_i64),
                    );
                for id in ids {
                    if let Ok(id) = i32::try_from(id)
                        && id < IDS
                        && !self.apps.contains(&id)
                    {
                        self.link.free(id);
                    }
                }
            }
        }
    }

    /// The text of a code element read back: what a save writes.
    fn on_info(&mut self, id: i32, props: &[(String, OscType)]) {
        let text = props
            .iter()
            .find(|(k, _)| k == "text")
            .map(|(_, v)| wire::text(std::slice::from_ref(v), 0))
            .unwrap_or_default();
        if id == PREFS_STARTUP {
            if let Some(and_restart) = self.saving_startup.take() {
                match config::update(|c| c.startup_code = text) {
                    Ok(_) if and_restart => self.restart(),
                    Ok(_) => {}
                    Err(e) => self.post("info", &format!("{e}\n")),
                }
            }
            return;
        }
        let Some(path) = self.saving.remove(&id) else {
            return;
        };
        self.platform.write(path, text, id);
    }

    /// **A save written**, or why not: the tab names its file and shows no
    /// changes, and whatever waited for the save goes on.
    fn on_written(&mut self, path: PathBuf, id: i32, done: Result<(), String>) {
        let win = id - CODE;
        match done {
            Ok(()) => {
                self.link.set(id, &[("saved", json!(1))]);
                if let Some(f) = self.files.iter_mut().find(|f| f.win == win) {
                    f.name = file_name(&path);
                    f.path = Some(path.clone());
                    f.dirty = false;
                    self.link.set(win, &f.tab());
                }
                self.settings.add_recent(&path.to_string_lossy());
                self.remember_files();
                self.menus();
                self.saved(win);
            }
            Err(e) => {
                self.after_save.remove(&win);
                self.post("info", &format!("Could not save {}: {e}\n", path.display()));
            }
        }
    }

    // -- the verbs: a menu's pick, a key's, a tool's

    /// **Performs `verb`**, asked from window `from`; `state` is a switch's
    /// new state.
    fn verb(&mut self, from: i32, verb: &str, state: bool) {
        let file = self.file_win(from);
        match verb {
            "new_file" => self.new_file(),
            "open_file" => self.choose(Purpose::Open, file),
            "save" => self.save(file, false),
            "save_as" => self.save(file, true),
            "close_tab" => self.close_tab(file),
            "quit" => self.quit(),
            "preferences" => self.open_prefs(),
            "evaluate_line" | "evaluate_block" | "find" | "find_next" | "go_to_line"
            | "toggle_comment" | "undo" | "redo" | "cut" | "copy" | "paste" | "select_all" => {
                // asked from a panel that is not the code's: the code's
                if let Some(win) = file.filter(|&w| w != from) {
                    self.link.verb(win, verb);
                }
            }
            "eval_paragraph" => {
                self.settings.eval_paragraph = state;
                self.settings.save();
                self.set_codes("paragraph", &json!(state));
                self.menus();
            }
            "interrupt" => self.interrupt(),
            _ if verb.starts_with("app:") => self.open_app(verb.trim_start_matches("app:")),
            "restart" => self.restart(),
            "environments" => self.open_envs(),
            "clear_post" => self.link.set(POST, &[("clear", json!(1))]),
            "show_docs" => {
                if self.docs_shown {
                    self.link.free(DOCS_WIN);
                    self.docs_shown = false;
                } else {
                    self.open_docs();
                    self.place_docs();
                }
                self.menus();
            }
            "split_editor" => self.dock(json!({"verb": "layout"}), DockFor::Split),
            "move_tab" => self.dock(json!({"verb": "layout"}), DockFor::MoveTab),
            "theme_light" | "theme_dark" | "theme_system" => {
                self.settings.theme = match verb {
                    "theme_light" => ThemePref::Light,
                    "theme_dark" => ThemePref::Dark,
                    _ => ThemePref::System,
                };
                self.settings.save();
                self.apply_theme();
                self.menus();
            }
            font if font.starts_with("font:") => {
                let name = &font["font:".len()..];
                self.settings.font = (name != platform::BUNDLED).then(|| name.to_string());
                self.settings.save();
                self.apply_font();
                self.menus();
            }
            "zoom_in" => self.zoom(Some(1.0)),
            "zoom_out" => self.zoom(Some(-1.0)),
            "zoom_reset" => self.zoom(None),
            "docs" => {
                self.show_docs();
                self.link.set(DOCS, &[("open", json!(""))]);
            }
            "search_docs" => {
                self.show_docs();
                self.link.verb(DOCS_WIN, "search");
            }
            "docs_folder" => self.choose_docs_folder(),
            "clear_recent" => {
                self.settings.recent.clear();
                self.settings.save();
                self.menus();
            }
            recent if recent.starts_with("recent_") => {
                let i: usize = recent["recent_".len()..].parse().unwrap_or(usize::MAX);
                if let Some(path) = self.settings.recent.get(i).cloned() {
                    self.open_path(Path::new(&path), Read::Open);
                }
            }
            _ => {}
        }
    }

    // -- files

    fn new_file(&mut self) {
        self.untitled += 1;
        let name = format!("untitled {}", self.untitled);
        self.open_window(None, name, String::new());
    }

    /// Opens `path` in a tab, or shows the tab that has it: `true` when it
    /// is read for one, which opens once it is in ([`Self::on_read`]).
    fn open_path(&mut self, path: &Path, why: Read) -> bool {
        let path = self.platform.absolute(path);
        if let Some(win) = self.file_at(&path).map(|f| f.win) {
            self.show_file(win);
            self.opened = true;
            return false;
        }
        self.platform.read(path, why);
        true
    }

    /// **A file read for a tab**: it opens, taking the place of an empty
    /// one; one that could not be read leaves the recent files.
    fn on_read(&mut self, path: PathBuf, why: Read, text: Result<String, String>) {
        match text {
            // a tab opened for it meanwhile is the one it has
            Ok(_) if self.file_at(&path).is_some() => self.opened = true,
            Ok(text) => {
                let blank = self.blank_tab();
                self.open_window(Some(path.clone()), file_name(&path), text);
                if let Some(win) = blank {
                    self.link.free(win);
                }
                if why == Read::Open {
                    self.settings.add_recent(&path.to_string_lossy());
                    self.settings.save();
                    self.menus();
                }
                self.opened = true;
            }
            Err(e) => {
                let shown = path.to_string_lossy().into_owned();
                self.settings.recent.retain(|p| *p != shown);
                self.settings.save();
                self.post("info", &format!("Could not read {shown}: {e}\n"));
            }
        }
        if let Some(left) = self.opening {
            self.opening = Some(left.saturating_sub(1));
            if left <= 1 {
                self.started();
            }
        }
    }

    /// A tab for a file that does not exist yet: empty, and made where it
    /// says when it is saved.
    fn open_missing(&mut self, path: &Path) {
        let blank = self.blank_tab();
        self.open_window(Some(path.to_path_buf()), file_name(path), String::new());
        if let Some(win) = blank {
            self.link.free(win);
        }
    }

    /// An untitled tab nothing was typed into, which the next file takes
    /// the place of.
    fn blank_tab(&self) -> Option<i32> {
        (self.files.len() == 1)
            .then(|| &self.files[0])
            .filter(|f| f.path.is_none() && !f.dirty)
            .map(|f| f.win)
    }

    fn open_window(&mut self, path: Option<PathBuf>, name: String, text: String) {
        let win = self.take_base();
        let file = File {
            win,
            path,
            name,
            dirty: false,
        };
        let tree = self.file_tree(&file, &text);
        self.link.def(win, &tree);
        // a tab of the editor on screen, where there is one
        if let Some(of) = self
            .active
            .filter(|a| self.files.iter().any(|f| f.win == *a))
        {
            self.dock(
                json!({"verb": "join", "pane": win, "of": of}),
                DockFor::Look,
            );
        }
        self.files.push(file);
        self.active = Some(win);
        self.link.set(win + CODE, &[("focus", json!(1))]);
        self.remember_files();
    }

    fn take_base(&mut self) -> i32 {
        let used: HashSet<i32> = self.files.iter().map(|f| f.win).collect();
        let mut base = self.next_base;
        while used.contains(&base) {
            base += FILE_STRIDE;
        }
        self.next_base = base + FILE_STRIDE;
        base
    }

    fn file_tree(&self, file: &File, text: &str) -> Value {
        let win = file.win;
        let mut tree = self.window(win, &file.name);
        // a file is a document: its panel shows it as its tab, even alone,
        // with its changes not kept and its path
        tree["dock_tab"] = json!(1);
        for (key, value) in file.tab() {
            tree[key] = value;
        }
        tree["main"] = json!(win + CODE);
        tree["children"] = json!([{
            "id": win + 1, "type": "layout", "flow": "col", "margin": 0, "gap": 0,
            "children": [
                {
                    "id": win + CODE, "type": "code", "text": text, "language": self.lang.code,
                    "weight": 1,
                    "indent": self.settings.indent_prop(),
                    "paragraph": self.settings.eval_paragraph,
                    "text_size": self.settings.text_size(),
                },
                closing::stack(&form(win), ""),
            ]
        }]);
        number(&mut tree, win + FILE_SPARE);
        tree
    }

    /// A window of the editor's: its title, its keys and, on the
    /// application's bar, the menu -- the application's, over every panel.
    fn window(&self, id: i32, title: &str) -> Value {
        let mut window = json!({
            "id": id,
            "type": "window",
            "title": title,
            "ask_close": 1,
            "status": 0,
            "keys": [clausters_editing::verbs::EDITOR_SCOPE],
            "verbs": clausters_editing::verbs::declaration(&[(
                clausters_editing::verbs::EDITOR_SCOPE,
                clausters_editing::verbs::EDITOR,
            )]),
            "margin": 0,
        });
        if id == BAR_WIN {
            window["menu"] = self.menu();
        }
        window
    }

    fn file_at(&self, path: &Path) -> Option<&File> {
        self.files.iter().find(|f| f.path.as_deref() == Some(path))
    }

    /// The file a verb asked from `win` acts on: its own, or the active one.
    fn file_win(&self, win: i32) -> Option<i32> {
        if self.files.iter().any(|f| f.win == win) {
            return Some(win);
        }
        self.active
            .filter(|a| self.files.iter().any(|f| f.win == *a))
            .or_else(|| self.files.last().map(|f| f.win))
    }

    fn show_file(&mut self, win: i32) {
        self.dock(json!({"verb": "select", "pane": win}), DockFor::Look);
        self.active = Some(win);
        self.link.set(win + CODE, &[("focus", json!(1))]);
    }

    /// Saves the file of window `win`, asking where for a new one or with
    /// `as_new`.
    fn save(&mut self, win: Option<i32>, as_new: bool) {
        let Some(file) = win.and_then(|w| self.files.iter().find(|f| f.win == w)) else {
            return;
        };
        match (&file.path, as_new) {
            (Some(path), false) => {
                self.saving.insert(file.code(), path.clone());
                self.link.query(file.code());
            }
            (path, _) => {
                let name = format!("{}.{}", file.name.replace(' ', "_"), self.lang.extension);
                let start = path
                    .clone()
                    .unwrap_or_else(|| self.platform.home().join(name));
                let purpose = Purpose::SaveAs { win: file.win };
                self.ask_path(purpose, Some(start));
            }
        }
    }

    fn choose(&mut self, purpose: Purpose, win: Option<i32>) {
        let start = win
            .and_then(|w| self.files.iter().find(|f| f.win == w))
            .and_then(|f| f.path.clone())
            .or_else(|| self.settings.recent.first().map(PathBuf::from));
        self.ask_path(purpose, start);
    }

    /// **A path asked for `purpose`**, in the editor's file dialogs over
    /// the platform's storage: every application's forms, drawn by the
    /// host, the same natively and in a page. A save starts at the file it
    /// writes; an open in the folder of `start`, with its name typed.
    fn ask_path(&mut self, purpose: Purpose, start: Option<PathBuf>) {
        use clausters_apps::forms::NAME;
        if self.prefs_open || self.envs_open || self.file_dialog.is_some() {
            return;
        }
        let text = |p: &Path| p.to_string_lossy().into_owned();
        let types = self.lang.types;
        let (form, values) = match purpose {
            Purpose::SaveAs { .. } => {
                let file = start.as_deref().map(text).unwrap_or_default();
                (Form::SaveAs(types), [(PATH.to_string(), file)].into())
            }
            Purpose::Open | Purpose::DocsFolder => {
                let file = start.as_deref().filter(|p| p.extension().is_some());
                let dir = match file {
                    Some(f) => f.parent().map(text),
                    None => start.as_deref().map(text),
                };
                let mut values = std::collections::BTreeMap::new();
                values.extend(dir.filter(|d| !d.is_empty()).map(|d| (PATH.to_string(), d)));
                values.extend(file.map(|f| (NAME.to_string(), text(f))));
                (Form::Open(types), values)
            }
        };
        self.file_dialog = Some(purpose);
        let shown = self.dialogs.open(form, values);
        if let Some(stack) = self.dialogs.stack(&filedialog::forms(types)) {
            self.show_dialog(stack, filedialog::SPARE);
        }
        self.correct(shown);
    }

    /// The dialogs' widgets corrected with what the forms answered.
    fn correct(&self, list: Vec<(i32, Value)>) {
        for (id, props) in list {
            let props: Vec<(&str, Value)> = props
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| (k.as_str(), v.clone()))
                .collect();
            self.link.set(id, &props);
        }
    }

    /// **What the hand did in the file dialog up**, read by the forms: a
    /// field, a chooser, a tool, the file type -- or the path chosen, which
    /// takes the dialog down.
    fn on_file_dialog(&mut self, id: i32, tag: &str, args: &[OscType]) {
        use clausters_apps::forms::Said;
        let (Some(purpose), Some(name)) = (
            self.file_dialog.clone(),
            self.dialogs.name(i64::from(id)).map(str::to_string),
        ) else {
            return;
        };
        let (tag, values) = filedialog::report(tag, args);
        let Some(said) = self.dialogs.read(&name, &tag, &values) else {
            return;
        };
        let Some(open) = self.dialogs.up().cloned() else {
            return;
        };
        let opens = matches!(open.form, Form::Open(_));
        let path = match said {
            Said::Cancel | Said::Discard => None,
            Said::Corrected(list) => return self.correct(list),
            Said::Picked(_, paths) if opens => paths.first().map(PathBuf::from),
            Said::Accept if opens => match self.dialogs.chosen(PATH).first() {
                Some(path) => Some(PathBuf::from(path)),
                None => return self.dialog_says(&open.form, "Select a file to open."),
            },
            Said::Accept => match open.save_path(PATH) {
                Some(path) => Some(PathBuf::from(path)),
                None => return self.dialog_says(&open.form, "Type a file's name first."),
            },
            _ => return,
        };
        self.close_dialog();
        self.chosen(purpose, path);
    }

    /// The line the file dialog up speaks in, saying `says`.
    fn dialog_says(&self, form: &Form, says: &str) {
        use clausters_apps::forms::Spec;
        use clausters_apps::forms::storage::{SAYS, says_props};
        if let Some(id) = self.dialogs.ids.get(&format!("{}:{SAYS}", form.prefix())) {
            self.correct(vec![(*id, says_props(says))]);
        }
    }

    fn chosen(&mut self, purpose: Purpose, path: Option<PathBuf>) {
        let Some(path) = path else {
            // a place not given calls the quit waiting for it off
            if let Purpose::SaveAs { win } = purpose
                && self.after_save.remove(&win) == Some(Ask::Quit)
            {
                self.after_save.retain(|_, a| *a != Ask::Quit);
                self.link.dock(&json!({"verb": "stay"}));
            }
            return;
        };
        match purpose {
            Purpose::Open => {
                self.open_path(&path, Read::Open);
            }
            Purpose::SaveAs { win } => {
                self.saving.insert(win + CODE, path);
                self.link.query(win + CODE);
            }
            Purpose::DocsFolder => self.set_docs_dir(Some(path.to_string_lossy().into_owned())),
        }
    }

    /// Closes a tab, asking first when it holds changes; the last one gives
    /// way to a new empty one.
    fn close_tab(&mut self, win: Option<i32>) {
        let Some(file) = win.and_then(|w| self.files.iter().find(|f| f.win == w)) else {
            return;
        };
        if file.dirty {
            let (win, name) = (file.win, file.name.clone());
            let says = format!("\"{name}\" has changes that are not saved.");
            return self.ask(win, Ask::CloseTab(win), &says);
        }
        self.close_now(file.win);
    }

    fn close_now(&mut self, win: i32) {
        if self.files.len() == 1 {
            self.new_file();
        }
        self.link.free(win);
    }

    // -- questions, in a file window's close form

    /// The close form up in window `win`, saying `says` of what is not
    /// saved: the form's stack defined again around it, shown.
    fn ask(&mut self, win: i32, what: Ask, says: &str) {
        self.asking = Some((win, what));
        self.show_file(win);
        let ids = form(win);
        let mut stack = closing::stack(&ids, says);
        stack["index"] = json!(1);
        number(&mut stack, win + FILE_SPARE);
        self.link.def(ids.stack, &stack);
    }

    fn answer(&mut self, win: i32, choice: closing::Choice) {
        self.link.set(form(win).stack, &[("index", json!(0))]);
        let Some((asked_in, what)) = self.asking.take() else {
            return;
        };
        if asked_in != win {
            return;
        }
        match (what, choice) {
            (Ask::Quit, closing::Choice::Cancel) => {
                self.link.dock(&json!({"verb": "stay"}));
            }
            (_, closing::Choice::Cancel) => {}
            (Ask::CloseTab(win), closing::Choice::Discard) => self.close_now(win),
            (Ask::Quit, closing::Choice::Discard) => self.quit_saved(),
            (Ask::CloseTab(win), closing::Choice::Save) => {
                self.after_save.insert(win, Ask::CloseTab(win));
                self.save(Some(win), false);
            }
            (Ask::Quit, closing::Choice::Save) => {
                // every file with a place is written; one with none is
                // asked where, and the quit waits for the last of them
                let unsaved: Vec<i32> = self
                    .files
                    .iter()
                    .filter(|f| f.dirty)
                    .map(|f| f.win)
                    .collect();
                if unsaved.is_empty() {
                    return self.quit_after_files();
                }
                for win in unsaved {
                    self.after_save.insert(win, Ask::Quit);
                    self.save(Some(win), false);
                }
            }
        }
    }

    /// A file was written: what its save was waiting to do.
    fn saved(&mut self, win: i32) {
        match self.after_save.remove(&win) {
            Some(Ask::CloseTab(win)) => self.close_now(win),
            Some(Ask::Quit) if !self.after_save.values().any(|a| *a == Ask::Quit) => {
                self.quit_after_files();
            }
            _ => {}
        }
    }

    // -- quitting

    /// **Quit, from the menu or a key**: what the host's applications would
    /// lose is asked first, and the quit goes on as the dock's close does.
    fn quit(&mut self) {
        tracing::debug!("quit asked");
        self.dock(json!({"verb": "unsaved"}), DockFor::Unsaved);
    }

    /// **The quit, over all the unsaved work** (`clausters_apps::quitting`):
    /// the editor's files with changes and the documents of the host's
    /// applications (`asked`, as the host listed them), named in one
    /// question in the window of the first file that has changes, or the one
    /// on screen; with nothing to lose, the editor ends at once.
    fn quit_over(&mut self, asked: &Value) {
        let host: Vec<String> = asked["unsaved"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|u| u["title"].as_str().map(str::to_string))
            .collect();
        self.quit_host = !host.is_empty();
        let mut names: Vec<String> = self
            .files
            .iter()
            .filter(|f| f.dirty)
            .map(|f| f.name.clone())
            .collect();
        names.extend(host);
        if names.is_empty() {
            return self.quit_saved();
        }
        let Some(win) = self
            .files
            .iter()
            .find(|f| f.dirty)
            .map(|f| f.win)
            .or(self.active)
            .or_else(|| self.files.first().map(|f| f.win))
        else {
            return self.quit_saved();
        };
        self.asking = Some((win, Ask::Quit));
        self.show_file(win);
        let ids = form(win);
        let mut stack = quitting::stack(&ids, &quitting::says(&names), names.len());
        stack["index"] = json!(1);
        number(&mut stack, win + FILE_SPARE);
        self.link.def(ids.stack, &stack);
    }

    /// The editor's own files are saved: the host's applications next, if
    /// they hold work, and then the end.
    fn quit_after_files(&mut self) {
        if self.quit_host {
            self.dock(json!({"verb": "save_all"}), DockFor::SaveAll);
        } else {
            self.quit_saved();
        }
    }

    /// Quits once the window's size and place are read, to open at them
    /// next time.
    fn quit_saved(&mut self) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        self.remember_files();
        self.dock(json!({"verb": "layout"}), DockFor::Quit);
        // a dock that does not answer does not keep the editor open
        let silence = Msg::Host(Said::Dock {
            seq: i32::MAX,
            reply: Value::Null,
        });
        self.platform.later(500, silence);
        self.dock_calls.insert(i32::MAX, DockFor::Quit);
    }

    /// **The arrangement kept by role**: the page the post window is on, as
    /// the dock answered it, every pane named by its role
    /// ([`role_of`](Self::role_of)) and the ones with none left out.
    fn keep_arrangement(&mut self, state: &Value) {
        let Some(layout) = post_page(state).map(|p| &p["layout"]) else {
            return;
        };
        self.settings.layout = arrangement::rename(layout, &|id| {
            let win = i32::try_from(id.as_i64()?).ok()?;
            self.role_of(win).map(Value::from)
        });
    }

    /// **The editor ends**: what it remembers is written, the session is
    /// stopped and the host's loop is asked to end, which ends the program on
    /// the thread that holds the windows. Ending it from this thread instead
    /// left a desktop's process alive after its window had gone.
    fn quit_now(&mut self) {
        tracing::debug!("quitting");
        self.settings.save();
        self.platform.stop();
        self.link.quit();
    }

    fn remember_files(&mut self) {
        self.settings.open_files = self
            .files
            .iter()
            .filter_map(|f| f.path.as_ref().map(|p| p.to_string_lossy().into_owned()))
            .collect();
        self.settings.active_file = self
            .active
            .and_then(|a| self.files.iter().find(|f| f.win == a))
            .and_then(|f| f.path.as_ref())
            .map(|p| p.to_string_lossy().into_owned());
        self.settings.save();
    }

    // -- the dock

    fn dock(&mut self, call: Value, purpose: DockFor) {
        let seq = self.link.dock(&call);
        self.dock_calls.insert(seq, purpose);
    }

    /// **The first arrangement**: the editor
    /// on the left, the documentation over the post window on the right.
    /// Asked again until every pane has arrived in the dock.
    fn arrange(&mut self, groups: &[Vec<i32>]) {
        let present: HashSet<i32> = groups.iter().flatten().copied().collect();
        let mut wanted = vec![POST_WIN];
        if self.docs_shown {
            wanted.push(DOCS_WIN);
        }
        wanted.extend(self.files.iter().map(|f| f.win));
        if !wanted.iter().all(|w| present.contains(w)) {
            self.platform.later(30, Msg::Arrange);
            return;
        }
        let Some(first) = self.files.first().map(|f| f.win) else {
            return;
        };
        if self.restore_arrangement() {
            if let Some(win) = self.active {
                self.show_file(win);
            }
            return;
        }
        for f in self.files.iter().skip(1) {
            let call = json!({"verb": "join", "pane": f.win, "of": first});
            self.link.dock(&call);
        }
        if self.docs_shown {
            self.link
                .dock(&json!({"verb": "split", "pane": DOCS_WIN, "side": "right", "of": first}));
            self.link.dock(
                &json!({"verb": "split", "pane": POST_WIN, "side": "bottom", "of": DOCS_WIN}),
            );
        } else {
            self.link
                .dock(&json!({"verb": "split", "pane": POST_WIN, "side": "right", "of": first}));
        }
        if let Some(win) = self.active {
            self.show_file(win);
        }
    }

    /// **The arrangement of the last run, put back**: what was kept by role
    /// ([`Settings::layout`]) in this run's ids, and a file it does not name
    /// joined to the group of the first file it does. `false`, and nothing
    /// done, when nothing was kept or what was kept misses a pane this run
    /// shows (the post window, the documentation, every file), which the
    /// first arrangement then places.
    fn restore_arrangement(&mut self) -> bool {
        let Some(saved) = self.settings.layout.clone() else {
            return false;
        };
        let Some(layout) = arrangement::rename(&saved, &|role| {
            self.pane_of(role.as_str()?).map(Value::from)
        }) else {
            return false;
        };
        let named: Vec<i32> = arrangement::panes(&layout)
            .iter()
            .filter_map(|v| v.as_i64().and_then(|v| i32::try_from(v).ok()))
            .collect();
        let Some(&of) = named
            .iter()
            .find(|w| self.files.iter().any(|f| f.win == **w))
        else {
            return false;
        };
        if !named.contains(&POST_WIN) || (self.docs_shown && !named.contains(&DOCS_WIN)) {
            return false;
        }
        self.link
            .dock(&json!({"verb": "restore", "layout": layout}));
        for f in self.files.iter().filter(|f| !named.contains(&f.win)) {
            self.link
                .dock(&json!({"verb": "join", "pane": f.win, "of": of}));
        }
        true
    }

    /// The role pane `win` is kept by: the post window, the documentation, a
    /// file by its path; none for an untitled file or a window not the
    /// editor's.
    fn role_of(&self, win: i32) -> Option<String> {
        match win {
            POST_WIN => Some("post".into()),
            DOCS_WIN => Some("docs".into()),
            _ => self
                .files
                .iter()
                .find(|f| f.win == win)
                .and_then(|f| f.path.as_ref())
                .map(|p| format!("file:{}", p.to_string_lossy())),
        }
    }

    /// The pane of this run that `role` names, if it is open.
    fn pane_of(&self, role: &str) -> Option<i32> {
        match role {
            "post" => Some(POST_WIN),
            "docs" => self.docs_shown.then_some(DOCS_WIN),
            _ => {
                let path = Path::new(role.strip_prefix("file:")?);
                self.file_at(path).map(|f| f.win)
            }
        }
    }

    /// Split: the tab on screen goes to a group of its own on the right; with
    /// the files in two groups, they go back into one.
    fn split_or_join(&mut self, groups: &[Vec<i32>]) {
        let editors = self.editor_groups(groups);
        if editors.len() > 1 {
            let first = editors[0][0];
            for g in &editors[1..] {
                for &win in g {
                    self.link
                        .dock(&json!({"verb": "join", "pane": win, "of": first}));
                }
            }
        } else if let Some(win) = self.file_win(0) {
            let others: Vec<i32> = editors
                .first()
                .map(|g| g.iter().copied().filter(|&w| w != win).collect())
                .unwrap_or_default();
            if let Some(&of) = others.first() {
                self.link
                    .dock(&json!({"verb": "split", "pane": win, "side": "right", "of": of}));
            } else {
                // one file: the new one goes beside it
                self.new_file();
                if let Some(new) = self.active {
                    self.link
                        .dock(&json!({"verb": "split", "pane": new, "side": "right", "of": win}));
                }
            }
        }
        self.dock(json!({"verb": "layout"}), DockFor::Look);
        self.menus();
    }

    fn move_tab(&mut self, groups: &[Vec<i32>]) {
        let editors = self.editor_groups(groups);
        let Some(win) = self.file_win(0) else {
            return;
        };
        if let Some(other) = editors.iter().find(|g| !g.contains(&win)) {
            self.link
                .dock(&json!({"verb": "join", "pane": win, "of": other[0]}));
            self.show_file(win);
        }
    }

    /// The groups of the dock that hold files, each its files.
    fn editor_groups(&self, groups: &[Vec<i32>]) -> Vec<Vec<i32>> {
        groups
            .iter()
            .map(|g| {
                g.iter()
                    .copied()
                    .filter(|w| self.files.iter().any(|f| f.win == *w))
                    .collect::<Vec<i32>>()
            })
            .filter(|g| !g.is_empty())
            .collect()
    }

    fn is_window(&self, id: i32) -> bool {
        matches!(id, BAR_WIN | FOOT_WIN | POST_WIN | DOCS_WIN)
            || self.files.iter().any(|f| f.win == id)
    }

    // -- the application's bar, the post window and the documentation

    /// **The application's bar**: a window along the dock's top, across it,
    /// holding the menu and the tools -- the application's, never a
    /// panel's. A tool performs the verb its menu entry does, and its tip
    /// says the key. At its end, one button per application opens a new
    /// one on a page of its own; the bar's tree stays whichever menu it
    /// shows, so they are there on every page.
    fn open_bar(&mut self) {
        let mut tree = self.window(BAR_WIN, "Editor");
        tree["dock_edge"] = json!("top");
        tree["h"] = json!(BAR_H);
        // a tool: a ghost button of 32 with its icon
        let tool = |id: i32, icon: &str, verb: &str, tip: &str| {
            json!({"id": id, "type": "button", "icon": icon, "flat": 1, "w": TOOL, "h": TOOL,
                "verb": verb, "tip": tip})
        };
        // the session's tools, shown beside the editor's menu alone
        let session = |id: i32, icon: &str, verb: &str, tip: &str| {
            let mut t = tool(id, icon, verb, tip);
            t["shown"] = json!(self.bar_own);
            t
        };
        tree["children"] = json!([{
            "id": BAR_WIN + 1, "type": "layout", "flow": "row", "pack": 1,
            "margin": 4, "gap": 0, "children": [
                // the line between the menu's titles and the tools
                {"id": BAR_LINE, "type": "separator", "shown": self.bar_own},
                session(BAR_WIN + 2, "play", "evaluate_line", "Evaluate line or selection"),
                session(BAR_WIN + 3, "list-video", "evaluate_block", "Evaluate block"),
                session(BAR_WIN + 4, "square", "interrupt", "Interrupt"),
                session(BAR_WIN + 5, "rotate-ccw", "restart", "Restart the session"),
                // the applications, at the bar's end
                {"id": BAR_SPACE, "type": "layout", "weight": 1, "margin": 0},
                tool(APPS[0].0, APPS[0].2, &format!("app:{}", APPS[0].1), APPS[0].3),
                tool(APPS[1].0, APPS[1].2, &format!("app:{}", APPS[1].1), APPS[1].3),
                tool(APPS[2].0, APPS[2].2, &format!("app:{}", APPS[2].1), APPS[2].3),
                tool(APPS[3].0, APPS[3].2, &format!("app:{}", APPS[3].1), APPS[3].3),
                // where a dialog of the application's is defined, over the dock
                {"id": DIALOG_HOLD, "type": "layout", "w": 0, "h": 0, "margin": 0},
            ],
        }]);
        self.link.def(BAR_WIN, &tree);
    }

    /// **The bar shows the menu of window `from`**: its own, or an
    /// application's on the page shown. The session's tools are entries of
    /// the editor's menu, so they are shown beside it alone, and their line
    /// with them; the buttons that open an application stay on every page.
    fn bar_shows(&mut self, from: i64) {
        let own = from == i64::from(BAR_WIN);
        if own == self.bar_own {
            return;
        }
        self.bar_own = own;
        for id in [BAR_LINE, BAR_WIN + 2, BAR_WIN + 3, BAR_WIN + 4, BAR_WIN + 5] {
            self.link.set(id, &[("shown", json!(own))]);
        }
    }

    /// **The application's footer**: a window along the dock's bottom, across
    /// it, saying the session's state -- a dot in its state's color and the
    /// interpreter, the environment and what it is doing.
    fn open_footer(&mut self) {
        let mut tree = self.window(FOOT_WIN, "Status");
        tree["dock_edge"] = json!("bottom");
        tree["h"] = json!(FOOT_H);
        tree["children"] = json!([{
            "id": FOOT, "type": "label", "text": "Python", "text_size": FOOT_TEXT,
            "muted": 1, "dot": "destructive",
        }]);
        self.link.def(FOOT_WIN, &tree);
    }

    fn open_post(&mut self) {
        let mut tree = self.window(POST_WIN, "Post window");
        // its tools (clear, autoscroll) are its panel's header, and nothing
        // could open it again once closed
        tree["header"] = json!(POST);
        tree["closable"] = json!(0);
        tree["children"] = json!([{
            "id": POST, "type": "post", "autoscroll": self.settings.autoscroll,
            "tools": 1, "text_size": self.settings.text_size(),
        }]);
        self.link.def(POST_WIN, &tree);
    }

    fn post(&self, kind: &str, text: &str) {
        self.link.set(POST, &[(kind, json!(text))]);
    }

    fn open_docs(&mut self) {
        let mut tree = self.window(DOCS_WIN, "Documentation");
        // its way back, where the page is and find are its panel's header
        tree["header"] = json!(DOCS);
        // its view takes the window's verbs (search, from the menu) and its
        // keys, before anything in it was pressed
        tree["main"] = json!(DOCS);
        let mut docs = json!({
            "id": DOCS, "type": "docs", "text_size": self.settings.text_size(),
        });
        // the header's folder button, where a folder can be chosen
        if self.offers.docs_folder {
            docs["folder"] = json!("docs_folder");
        }
        tree["children"] = json!([docs]);
        self.link.def(DOCS_WIN, &tree);
        self.docs_shown = true;
        self.feed_docs();
    }

    /// The documentation beside the editor, as the first arrangement puts it.
    fn place_docs(&mut self) {
        if let Some(of) = self.file_win(0) {
            self.link
                .dock(&json!({"verb": "split", "pane": DOCS_WIN, "side": "right", "of": of}));
        }
    }

    fn show_docs(&mut self) {
        if !self.docs_shown {
            self.open_docs();
            self.place_docs();
            self.menus();
        }
        self.dock(json!({"verb": "select", "pane": DOCS_WIN}), DockFor::Look);
        // and the keyboard on it, so what it was opened for is typed there
        self.link.set(DOCS, &[("focus", json!(1))]);
    }

    fn feed_docs(&self) {
        let Some((books, documents, roots)) = &self.docs else {
            return;
        };
        if !self.docs_shown {
            return;
        }
        // what a document no longer has is taken away
        let documents: serde_json::Map<String, Value> = documents
            .iter()
            .map(|(k, v)| (k.clone(), Value::from(v.as_str())))
            .collect();
        self.link.set(
            DOCS,
            &[
                ("books", serde_json::to_value(books).unwrap_or(Value::Null)),
                ("documents", Value::Object(documents)),
                ("roots", json!(roots)),
            ],
        );
    }

    /// Help on a name in the documentation: the panel is shown, at the
    /// name's entry, or at pydoc's text for it.
    fn show_help(&mut self, help: Value) {
        if self.docs.is_none() {
            self.help_waiting = Some(help);
            return;
        }
        self.show_docs();
        self.link.set(DOCS, &[("help", help)]);
    }

    fn choose_docs_folder(&mut self) {
        if !self.offers.docs_folder {
            return;
        }
        let start = config::get().docs_dir.map(PathBuf::from);
        self.platform.choose(Purpose::DocsFolder, start);
    }

    fn set_docs_dir(&mut self, dir: Option<String>) {
        if let Some(d) = &dir
            && !self.platform.is_dir(Path::new(d))
        {
            return self.post("info", &format!("Not a folder: {d}\n"));
        }
        match config::update(|c| c.docs_dir = dir) {
            Ok(config) => {
                self.platform.read_docs();
                if self.prefs_open {
                    self.link
                        .set(PREFS_DOCS_DIR, &[("text", json!(docs_dir_text(&config)))]);
                }
            }
            Err(e) => self.post("info", &format!("{e}\n")),
        }
    }

    // -- the session

    fn on_py(&mut self, m: Value) {
        let kind = m.get("type").and_then(Value::as_str).unwrap_or("");
        let text = m.get("text").and_then(Value::as_str).unwrap_or("");
        let id = m.get("id").and_then(Value::as_u64);
        match kind {
            "ready" => {
                self.py.alive = true;
                self.py.version = text.split(' ').next().unwrap_or("").to_string();
                let says = format!("{} {} ready\n", self.lang.name, self.py.version);
                self.post("info", &says);
                self.say_session();
                self.run_startup_code();
            }
            "done" => {
                if let Some(id) = id {
                    self.py.busy.remove(&id);
                }
                self.say_session();
            }
            "out" | "err" | "result" | "info" => self.post(kind, text),
            "sys" => self.post("out", text),
            "help" | "inspect" | "signatures" | "complete" => {
                if let Some(query) = id.and_then(|id| self.queries.remove(&id)) {
                    self.answered(query, &m);
                }
            }
            _ => {}
        }
    }

    fn answered(&mut self, query: Query, m: &Value) {
        let data = m.get("data").cloned().unwrap_or(Value::Null);
        match query {
            Query::Complete(code) => {
                let list = if data.is_array() { data } else { json!([]) };
                self.link.set(code, &[("completions", list)]);
            }
            Query::Signatures(code) => {
                let first = data.get(0).cloned().unwrap_or(Value::Null);
                self.link.set(code, &[("signature", first)]);
            }
            Query::Inspect(code) => self.link.set(code, &[("inspected", data)]),
            Query::HelpInspect { text } => {
                let expr = data
                    .get("fullname")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&text)
                    .to_string();
                let info = if data.is_object() {
                    data
                } else {
                    json!({"name": text})
                };
                self.query("help", json!({"expr": expr}), Query::HelpText { info });
            }
            Query::HelpText { info } => {
                let text = m.get("text").and_then(Value::as_str).unwrap_or("");
                let name = info.get("name").cloned().unwrap_or(Value::Null);
                let help = json!({
                    "name": name,
                    "qualname": info.get("qualname").cloned().unwrap_or_else(|| name.clone()),
                    "fullname": info.get("fullname").cloned().unwrap_or_else(|| name.clone()),
                    "text": text,
                    "source": "Python",
                });
                self.show_help(help);
            }
        }
    }

    /// A question to the session; one asked while it is not running has no
    /// answer, and the element is told so at once -- unless the platform
    /// answers questions apart from the session.
    fn query(&mut self, op: &str, mut request: Value, query: Query) {
        if !self.py.alive && !self.platform.answers_without_session() {
            let empty = json!({"type": op, "data": null, "text": ""});
            return self.answered(query, &empty);
        }
        let id = self.next_request;
        self.next_request += 1;
        request["id"] = json!(id);
        request["op"] = json!(op);
        self.queries.insert(id, query);
        if let Err(e) = self.platform.ask(&request) {
            self.queries.remove(&id);
            self.post("err", &format!("{e}\n"));
        }
    }

    /// A position in the code of window `win`, as the driver reads one:
    /// the text, a line counted from one and a column from zero, and the
    /// file it is in.
    fn at(&self, win: i32, args: &[OscType]) -> Value {
        let file = self
            .files
            .iter()
            .find(|f| f.win == win)
            .and_then(|f| f.path.as_ref())
            .map(|p| p.to_string_lossy().into_owned());
        json!({
            "source": wire::text(args, 0),
            "line": wire::int(args, 1),
            "column": wire::int(args, 2),
            "file": file,
        })
    }

    fn evaluate(&mut self, win: i32, code: &str, line: i64) {
        let file = self
            .files
            .iter()
            .find(|f| f.win == win)
            .map(|f| match &f.path {
                Some(p) => p.to_string_lossy().into_owned(),
                None => format!("<{}>", f.name),
            })
            .unwrap_or_default();
        self.eval(code, &file, line);
    }

    fn eval(&mut self, code: &str, file: &str, line: i64) {
        let id = self.next_request;
        self.next_request += 1;
        self.py.busy.insert(id);
        self.say_session();
        let request = json!({"id": id, "op": "eval", "code": code, "file": file, "line": line});
        if let Err(e) = self.platform.send(&request) {
            self.py.busy.remove(&id);
            self.post("err", &format!("{e}\n"));
            self.say_session();
        }
    }

    fn run_startup_code(&mut self) {
        let code = config::get().startup_code;
        if code.trim().is_empty() {
            return;
        }
        self.post("info", &format!("Startup code ({}):\n", self.lang.name));
        self.post("out", &format!("{}\n", code.trim_end()));
        self.eval(&code, "<startup code>", 1);
    }

    /// **A new, empty application of `kind`**, which the host opens on a
    /// page of its own (`/gui_app_open`): the host's to hold, the editor's
    /// session none of its business.
    fn open_app(&mut self, kind: &str) {
        self.link.send(
            "/gui_app_open",
            vec![
                OscType::Int(0),
                OscType::String(kind.into()),
                OscType::String("{}".into()),
            ],
        );
    }

    fn interrupt(&mut self) {
        if let Err(e) = self.platform.interrupt() {
            self.post("info", &format!("{e}\n"));
        }
    }

    /// **The session's windows go with it**: they opened in the editor's
    /// dock, and a session that ended or restarts answers for none of them.
    fn free_session_windows(&mut self) {
        self.dock(json!({"verb": "layout"}), DockFor::Orphans);
    }

    fn restart(&mut self) {
        self.post(
            "info",
            &format!("Restarting the {} session...\n", self.lang.name),
        );
        self.py.alive = false;
        self.free_session_windows();
        if let Err(e) = self.platform.restart() {
            self.post("err", &format!("{e}\n"));
        }
    }

    /// **The session's state, in the footer**: its dot (stopped, running,
    /// ready) and `<language> <version> | <environment> | <state>`, the
    /// environment where the platform keeps them.
    fn say_session(&self) {
        let state = if !self.py.alive {
            "stopped".to_string()
        } else if self.py.busy.is_empty() {
            "ready".to_string()
        } else {
            format!("running ({})", self.py.busy.len())
        };
        let env = match &self.py.env {
            Some(name) => format!(" | environment \"{name}\""),
            None if self.offers.envs => " | system".into(),
            None => String::new(),
        };
        let dot = if !self.py.alive {
            "destructive"
        } else if self.py.busy.is_empty() {
            "success"
        } else {
            "warning"
        };
        let python = if self.py.version.is_empty() {
            self.lang.name.to_string()
        } else {
            format!("{} {}", self.lang.name, self.py.version)
        };
        let line = format!("{python}{env} | {state}");
        self.link
            .set(FOOT, &[("text", json!(line)), ("dot", json!(dot))]);
    }

    // -- the look

    fn dark(&self) -> bool {
        match self.settings.theme {
            ThemePref::Dark => true,
            ThemePref::Light => false,
            ThemePref::System => self.system_dark,
        }
    }

    fn apply_theme(&self) {
        self.link.theme(&self.look());
    }

    /// The look in use: its table of roles.
    fn look(&self) -> Value {
        if self.dark() {
            theme::dark()
        } else {
            theme::light()
        }
    }

    /// **Code drawn in the chosen family**: its two weights handed to the
    /// host, or the host's own face again for the bundled one -- and for a
    /// family no longer installed, which is how the current editor falls
    /// back too.
    fn apply_font(&self) {
        let family = self
            .settings
            .font
            .as_ref()
            .and_then(|name| self.fonts.iter().find(|f| &f.name == name));
        let read = |path: Option<&PathBuf>| path.map(|p| self.platform.font(p)).unwrap_or_default();
        let regular = read(family.map(|f| &f.regular));
        // a family with no bold draws its bold as its regular, not as the
        // host's own
        let bold = match family.and_then(|f| f.bold.as_ref()) {
            Some(path) => read(Some(path)),
            None => regular.clone(),
        };
        for (bytes, weight) in [(regular, "mono"), (bold, "mono_bold")] {
            self.link.send(
                "/gui_font",
                vec![OscType::Blob(bytes), OscType::String(weight.into())],
            );
        }
    }

    fn zoom(&mut self, delta: Option<f32>) {
        self.settings.zoom(delta);
        self.settings.save();
        let size = json!(self.settings.text_size());
        self.set_codes("text_size", &size);
        self.link.set(POST, &[("text_size", size.clone())]);
        self.link.set(DOCS, &[("text_size", size)]);
    }

    fn set_codes(&self, key: &str, value: &Value) {
        for f in &self.files {
            self.link.set(f.code(), &[(key, value.clone())]);
        }
    }

    // -- the menus

    /// **The menu every window of the editor carries**, as its state reads
    /// now.
    fn menu(&self) -> Value {
        let recent: Vec<Value> = if self.settings.recent.is_empty() {
            vec![json!({"label": "No Recent Files", "enabled": false})]
        } else {
            self.settings
                .recent
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let (name, folder) = recent_label(p, &self.platform.home());
                    json!({"label": name, "aside": folder, "verb": format!("recent_{i}")})
                })
                .collect()
        };
        let mut recent = recent;
        recent.push(json!("-"));
        recent.push(
            json!({"label": "Clear Recent Files", "verb": "clear_recent",
            "enabled": !self.settings.recent.is_empty()}),
        );
        let theme = |verb: &str, label: &str, pref: ThemePref| {
            json!({"label": label, "verb": verb, "group": "theme",
                "checked": self.settings.theme == pref})
        };
        let split = self.editor_split();
        let file = self.file_menu(recent);
        let mut menu = json!([
            file,
            {"label": "Edit", "menu": [
                {"label": "Undo", "verb": "undo"},
                {"label": "Redo", "verb": "redo"},
                "-",
                {"label": "Cut", "verb": "cut"},
                {"label": "Copy", "verb": "copy"},
                {"label": "Paste", "verb": "paste"},
                {"label": "Select All", "verb": "select_all"},
                "-",
                {"label": "Find and Replace", "verb": "find"},
                {"label": "Find Next", "verb": "find_next"},
                {"label": "Go to Line", "verb": "go_to_line"},
                {"label": "Toggle Comment", "verb": "toggle_comment"},
            ]},
            {"label": "Session", "menu": [
                {"label": "Evaluate Line or Selection", "verb": "evaluate_line"},
                {"label": "Evaluate Block", "verb": "evaluate_block"},
                {"label": "Evaluate Contiguous Lines as a Block", "verb": "eval_paragraph",
                    "checked": self.settings.eval_paragraph},
                "-",
                {"label": "Interrupt", "verb": "interrupt"},
                {"label": "Restart Session", "verb": "restart"},
                {"label": "Python Environments...", "verb": "environments"},
                "-",
                {"label": "Clear Post Window", "verb": "clear_post"},
            ]},
            {"label": "View", "menu": [
                {"label": "Theme", "menu": [
                    theme("theme_light", "Light", ThemePref::Light),
                    theme("theme_dark", "Dark", ThemePref::Dark),
                    theme("theme_system", "System", ThemePref::System),
                ]},
                "-",
                {"label": "Documentation", "verb": "show_docs", "checked": self.docs_shown},
                {"label": "Split Editor", "verb": "split_editor", "checked": split},
                {"label": "Move Tab to Other Editor", "verb": "move_tab", "enabled": split},
                {"label": "Font", "menu": self.font_menu()},
                "-",
                {"label": "Increase Size", "verb": "zoom_in"},
                {"label": "Decrease Size", "verb": "zoom_out"},
                {"label": "Default Size", "verb": "zoom_reset"},
            ]},
            {"label": "Help", "menu": [
                {"label": "Documentation", "verb": "docs"},
                {"label": "Search Documentation", "verb": "search_docs"},
                {"label": "Documentation Folder...", "verb": "docs_folder"},
                "-",
                {"label": "Keyboard Shortcuts", "verb": "keys"},
            ]},
        ]);
        // what the platform does not have is not offered
        let mut hidden = Vec::new();
        if !self.offers.quit {
            hidden.push("quit");
        }
        if !self.offers.envs {
            hidden.push("environments");
        }
        if !self.offers.docs_folder {
            hidden.push("docs_folder");
        }
        if !self.offers.fonts {
            hidden.push("Font");
        }
        hide(&mut menu, &hidden);
        menu
    }

    /// **File**: every application's ([`clausters_apps::chrome::file_menu`]),
    /// its file a tab's, with the recent files the editor keeps and the
    /// preferences, and the program's quit where the window has none.
    fn file_menu(&self, recent: Vec<Value>) -> Value {
        use clausters_apps::chrome::{File, file_menu, word};
        use clausters_editing::verbs::{EDITOR, titled};
        let mut file = titled(file_menu(&File {
            files: true,
            saves: true,
            recent: Some(Vec::new()),
            close_file: "close_tab",
            own: vec![vec![word(EDITOR, "preferences")]],
            last: vec![word(EDITOR, "quit")],
            ..File::new(EDITOR)
        }));
        // the recent files are named as they are, never in a title's capitals
        if let Some(entry) = file["menu"]
            .as_array_mut()
            .and_then(|entries| entries.iter_mut().find(|e| e.get("menu").is_some()))
        {
            entry["menu"] = Value::Array(recent);
        }
        file
    }

    /// View > Font: the bundled face, then the system's monospaced
    /// families, the one in use checked.
    fn font_menu(&self) -> Value {
        let chosen = self.settings.font.as_deref().unwrap_or(platform::BUNDLED);
        let entry = |name: &str, label: String| {
            json!({"label": label, "verb": format!("font:{name}"), "group": "font",
                "checked": name == chosen})
        };
        let mut entries = vec![entry(
            platform::BUNDLED,
            format!("{} (bundled)", platform::BUNDLED),
        )];
        if !self.fonts.is_empty() {
            entries.push(json!("-"));
        }
        entries.extend(self.fonts.iter().map(|f| entry(&f.name, f.name.clone())));
        Value::Array(entries)
    }

    /// Whether the files are in more than one group, as last seen.
    fn editor_split(&self) -> bool {
        self.split
    }

    /// The application's menu, as the state now reads.
    fn menus(&self) {
        self.link.set(BAR_WIN, &[("menu", self.menu())]);
    }

    // -- the application's dialogs: the preferences and the environments

    /// **A dialog of the application's goes up**, over the whole dock: the
    /// bar holds it, so it is the application's rather than a panel's. One
    /// at a time, as a dialog takes everything behind it.
    fn show_dialog(&mut self, dialog: Value, spare: i32) {
        let mut hold = json!({"type": "layout", "w": 0, "h": 0, "margin": 0, "children": [dialog]});
        number(&mut hold, spare);
        self.link.def(DIALOG_HOLD, &hold);
        self.menus();
    }

    /// The dialog that is up goes.
    fn close_dialog(&mut self) {
        self.prefs_open = false;
        self.envs_open = false;
        self.file_dialog = None;
        self.dialogs.closed();
        self.link.def(
            DIALOG_HOLD,
            &json!({"type": "layout", "w": 0, "h": 0, "margin": 0}),
        );
        self.menus();
    }

    fn open_prefs(&mut self) {
        if self.prefs_open || self.envs_open || self.file_dialog.is_some() {
            return;
        }
        let config = config::get();
        let indent = ["tab", "2", "4"]
            .iter()
            .position(|i| *i == self.settings.indent)
            .unwrap_or(2);
        let theme = match self.settings.theme {
            ThemePref::Light => 0,
            ThemePref::Dark => 1,
            ThemePref::System => 2,
        };
        let mut dialog = json!({
            "id": PREFS_DIALOG, "type": "layout", "modal": 1, "wide": 1, "hug": 1,
            "title": "Preferences", "flow": "col",
            "children": [
                {"id": PREFS_REOPEN, "type": "toggle",
                    "label": "Open the last session's files at startup",
                    "value": i32::from(self.settings.reopen_files)},
                field(&[
                    caption("Theme"),
                    json!({"id": PREFS_THEME, "type": "choice",
                        "options": ["Light", "Dark", "System"], "index": theme,
                        "max_w": 176, "anchor": "left"}),
                ]),
                field(&[
                    caption("Indentation"),
                    json!({"id": PREFS_INDENT, "type": "choice",
                        "options": ["Tab character", "2 spaces", "4 spaces"], "index": indent,
                        "max_w": 176, "anchor": "left"}),
                    help("What Tab inserts at the caret, and one level of indentation for the selected lines."),
                ]),
                field(&[
                    caption("Documentation folder (.md files)"),
                    json!({"id": PREFS_DOCS_DIR, "type": "label", "variant": "path", "text": docs_dir_text(&config)}),
                    buttons(&[
                        json!({"id": PREFS_DOCS_CHOOSE, "type": "button", "label": "Choose folder...",
                            "variant": "outline"}),
                        json!({"id": PREFS_DOCS_BUNDLED, "type": "button", "label": "Use bundled",
                            "variant": "ghost"}),
                    ]),
                ]),
                field(&[
                    caption("Startup code"),
                    json!({"id": PREFS_STARTUP, "type": "code", "text": config.startup_code,
                        "language": "python", "h": 112, "line_numbers": 0,
                        "indent": self.settings.indent_prop(),
                        "text_size": self.settings.text_size()}),
                    help("Runs every time the session starts (when it is first needed, when it is restarted or when its environment changes) and is shown in the post window."),
                    buttons(&[
                        json!({"id": PREFS_SAVE, "type": "button", "label": "Save",
                            "variant": "outline"}),
                        json!({"id": PREFS_SAVE_RESTART, "type": "button",
                            "label": "Save and restart session", "variant": "ghost"}),
                    ]),
                ]),
                footer(json!({"id": PREFS_CLOSE, "type": "button", "label": "Close", "default": 1,
                    "variant": "outline"})),
            ]
        });
        // a folder of the user's only where the platform has one to choose
        if !self.offers.docs_folder
            && let Some(children) = dialog["children"].as_array_mut()
        {
            children.retain(|field| !holds(field, PREFS_DOCS_DIR));
        }
        self.prefs_open = true;
        self.show_dialog(dialog, PREFS_SPARE);
    }

    /// The startup code is read back from its editor, then saved.
    fn ask_startup_code(&mut self, and_restart: bool) {
        self.saving_startup = Some(and_restart);
        self.link.query(PREFS_STARTUP);
    }

    fn open_envs(&mut self) {
        if self.prefs_open || self.envs_open || self.file_dialog.is_some() {
            return;
        }
        let dialog = json!({
            "id": ENVS_DIALOG, "type": "layout", "modal": 1, "wide": 1, "hug": 1,
            "title": "Python Environments",
            "description": "They are kept in the editor's data folder. The session uses the marked environment, also after reopening the editor.",
            "flow": "col",
            "children": [
                field(&[caption("Environments"), self.envs_list()]),
                {"type": "layout", "flow": "col", "hug": 1, "margin": 16, "gap": 16, "frame": 1,
                    "children": [
                    json!({"type": "label", "text": "New environment", "variant": "caption"}),
                    field(&[
                        caption("1. Install source"),
                        json!({"id": ENVS_SOURCE, "type": "text",
                            "value": self.fields.get(&ENVS_SOURCE).cloned().unwrap_or_default()}),
                        help("What pip installs in it: a package name, a path or a URL, with any pip options."),
                    ]),
                    field(&[
                        caption("2. Environment name"),
                        buttons(&[
                            json!({"id": ENVS_NAME, "type": "text", "weight": 1,
                                "value": self.fields.get(&ENVS_NAME).cloned().unwrap_or_default()}),
                            json!({"id": ENVS_CREATE, "type": "button", "label": "Create",
                                "variant": "default"}),
                        ]),
                        help("A short name: letters, digits, \".\", \"_\" or \"-\"."),
                    ]),
                ]},
                // **nothing to say is no row**: shown once it says something
                {"id": ENVS_STATUS, "type": "label", "text": "", "wrap": 1, "muted": 1,
                    "shown": 0},
                footer(json!({"id": ENVS_CLOSE, "type": "button", "label": "Close", "default": 1,
                    "variant": "outline"})),
            ]
        });
        self.envs_open = true;
        self.show_dialog(dialog, ENVS_SPARE);
    }

    /// The list of environments, a row each, the system's Python first.
    fn envs_list(&self) -> Value {
        let envs = self.platform.envs();
        let system_active = !envs.iter().any(|e| e.active);
        // **a row is two lines**, as the reference's: the name in the medium
        // weight with its Python beside it, quiet, and under them where it
        // was installed from, in the code face
        let row =
            |i: i32, name: String, version: String, source: String, active: bool, tools: bool| {
                let base = ENVS_ROWS + i * 10;
                let mut head = vec![json!({"id": base + 6, "type": "label", "text": name,
                "variant": "caption"})];
                if !version.is_empty() {
                    head.push(json!({"id": base + 7, "type": "label", "text": version,
                    "muted": 1}));
                }
                let text = json!({"id": base + 1, "type": "layout", "flow": "col", "weight": 1,
                "margin": 0, "gap": 2, "hug": 1, "children": [
                    {"type": "layout", "flow": "row", "pack": 1, "margin": 0, "gap": 8,
                        "hug": 1, "children": head},
                    {"id": base + 8, "type": "label", "text": source, "variant": "code"},
                ]});
                let mut children = vec![text];
                if active {
                    children.push(
                    json!({"id": base + 2, "type": "label", "text": "in use", "variant": "badge"}),
                );
                } else {
                    children.push(json!({"id": base + 3, "type": "button", "label": "Use",
                    "variant": "ghost", "size": "sm"}));
                }
                if tools {
                    children.push(
                        json!({"id": base + 4, "type": "button", "icon": "refresh-cw",
                    "variant": "ghost", "size": "sm", "tip": "Reinstall from its install source"}),
                    );
                    children.push(json!({"id": base + 5, "type": "button", "icon": "trash",
                    "variant": "ghost", "size": "sm", "tip": "Delete the environment"}));
                }
                json!({"id": base, "type": "layout", "flow": "row", "pack": 1, "gap": 8,
                "children": children})
            };
        let mut rows = vec![row(
            0,
            self.lang.system.into(),
            String::new(),
            self.platform.default_interpreter(),
            system_active,
            false,
        )];
        for (i, e) in envs.iter().enumerate() {
            let version = if e.version.is_empty() {
                String::new()
            } else {
                format!("Python {}", e.version)
            };
            let source = if e.source.is_empty() {
                "only jedi, for the completions".to_string()
            } else {
                e.source.clone()
            };
            rows.push(row(
                i as i32 + 1,
                e.name.clone(),
                version,
                source,
                e.active,
                true,
            ));
        }
        // a divided list in a frame, as the reference's
        json!({"id": ENVS_LIST, "type": "layout", "flow": "col", "frame": 1, "divide": 1,
            "margin": 0, "hug": 1, "children": rows})
    }

    fn envs_refresh(&mut self, status: &str) {
        if !self.envs_open {
            return;
        }
        self.link.def(ENVS_LIST, &self.envs_list());
        self.envs_say(status);
    }

    /// What the environments' dialog says it is doing.
    fn envs_say(&self, text: &str) {
        self.link.set(
            ENVS_STATUS,
            &[("text", json!(text)), ("shown", json!(!text.is_empty()))],
        );
    }

    fn envs_row(&mut self, id: i32) {
        let i = (id - ENVS_ROWS) / 10;
        let what = (id - ENVS_ROWS) % 10;
        let name = if i == 0 {
            None
        } else {
            self.platform
                .envs()
                .get(i as usize - 1)
                .map(|e| e.name.clone())
        };
        let result = match (what, name) {
            (3, name) => self.platform.use_env(name),
            (4, Some(name)) => {
                self.envs_say(&format!(
                    "Updating \"{name}\"... (progress in the post window)"
                ));
                let started = self.platform.install_in_env(name);
                return self.report_envs(started);
            }
            (5, Some(name)) => self.platform.delete_env(name),
            _ => return,
        };
        let status = result.err().unwrap_or_default();
        self.envs_refresh(&status);
    }

    fn envs_create(&mut self) {
        let name = self.fields.get(&ENVS_NAME).cloned().unwrap_or_default();
        let name = name.trim().to_string();
        let source = self.fields.get(&ENVS_SOURCE).cloned().unwrap_or_default();
        if name.is_empty() {
            return self.envs_say("Name the environment first.");
        }
        self.envs_say(&format!(
            "Creating \"{name}\"... (progress in the post window)"
        ));
        let result = self.platform.create_env(name, source);
        self.report_envs(result);
    }

    fn report_envs(&mut self, started: Result<(), String>) {
        if let Err(e) = started {
            self.envs_say(&e);
        }
    }
}

/// **A field of a dialog**: its caption, its control and what helps with
/// it, set close together.
fn field(children: &[Value]) -> Value {
    json!({"type": "layout", "flow": "col", "hug": 1, "margin": 0, "gap": 8, "children": children})
}

/// A field's caption.
fn caption(text: &str) -> Value {
    json!({"type": "label", "text": text, "variant": "caption"})
}

/// The sentence under a field that says what it is for, quieter.
fn help(text: &str) -> Value {
    json!({"type": "label", "text": text, "wrap": 1, "variant": "help"})
}

/// A row of buttons, as wide as what they say.
fn buttons(children: &[Value]) -> Value {
    json!({"type": "layout", "flow": "row", "pack": 1, "hug": 1, "margin": 0, "gap": 8,
        "children": children})
}

/// **A dialog's footer**: its buttons at its right.
fn footer(button: Value) -> Value {
    json!({"type": "layout", "flow": "row", "pack": 1, "hug": 1, "margin": 0, "gap": 8,
        "children": [{"type": "separator", "weight": 1, "line": 0}, button]})
}

/// **Takes out of a menu the entries named in `hidden`**, by their verb or,
/// for a submenu, its label, and the separators that are left with nothing
/// to separate.
fn hide(menu: &mut Value, hidden: &[&str]) {
    let Some(items) = menu.as_array_mut() else {
        return;
    };
    items.retain(|item| {
        let named = |key: &str| item[key].as_str().is_some_and(|v| hidden.contains(&v));
        !named("verb") && !(item.get("menu").is_some() && named("label"))
    });
    for item in items.iter_mut() {
        if let Some(sub) = item.get_mut("menu") {
            hide(sub, hidden);
        }
    }
    let line = |item: &Value| item.as_str() == Some("-");
    let mut tidy: Vec<Value> = Vec::new();
    for item in items.drain(..) {
        if line(&item) && tidy.last().is_none_or(line) {
            continue;
        }
        tidy.push(item);
    }
    if tidy.last().is_some_and(line) {
        tidy.pop();
    }
    *items = tidy;
}

/// Whether `tree` holds the node `id`.
fn holds(tree: &Value, id: i32) -> bool {
    tree["id"].as_i64() == Some(i64::from(id))
        || tree["children"]
            .as_array()
            .is_some_and(|c| c.iter().any(|child| holds(child, id)))
}

/// **Gives every node of `tree` that has none an id**, counting from
/// `next`: the host skips a node it cannot address.
fn number(tree: &mut Value, next: i32) -> i32 {
    let mut next = next;
    if let Some(node) = tree.as_object_mut() {
        if !node.contains_key("id") {
            node.insert("id".into(), json!(next));
            next += 1;
        }
        if let Some(Value::Array(children)) = node.get_mut("children") {
            for child in children {
                next = number(child, next);
            }
        }
    }
    next
}

/// Whether an event's argument is on.
fn on(v: &OscType) -> bool {
    match v {
        OscType::Int(i) => *i != 0,
        OscType::Long(i) => *i != 0,
        OscType::Float(f) => *f != 0.0,
        OscType::Double(f) => *f != 0.0,
        OscType::Bool(b) => *b,
        _ => false,
    }
}

/// **The call that names the editor's page [`PAGE`]**, from a dock's
/// state: the page the post window is on, renamed when it is named
/// otherwise; none once it is, or while the post window is on no page.
fn page_rename(state: &Value) -> Option<Value> {
    let name = post_page(state)?["name"].as_str()?;
    (name != PAGE).then(|| json!({"verb": "page_rename", "name": name, "to": PAGE}))
}

/// The page of a dock's state the post window is on, as the dock answered
/// it (its `name` and its `layout`).
fn post_page(state: &Value) -> Option<&Value> {
    state["pages"].as_array()?.iter().find(|p| {
        arrangement::panes(&p["layout"])
            .iter()
            .any(|id| id.as_i64() == Some(i64::from(POST_WIN)))
    })
}

/// The groups of a dock's state, each its panes in tab order.
fn groups(state: &Value) -> Vec<Vec<i32>> {
    state
        .get("bars")
        .and_then(Value::as_array)
        .map(|bars| {
            bars.iter()
                .map(|b| {
                    b.get("panes")
                        .and_then(Value::as_array)
                        .map(|p| {
                            p.iter()
                                .filter_map(|v| v.as_i64())
                                .map(|v| v as i32)
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// A recent file as a menu lists it: its name, then its folder, written
/// from `~` under the user's folder `home`.
fn recent_label(path: &str, home: &Path) -> (String, String) {
    let p = Path::new(path);
    let dir = p
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let home = home.to_string_lossy().into_owned();
    let dir = match dir.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => dir,
    };
    (file_name(p), dir)
}

fn docs_dir_text(config: &config::Config) -> String {
    config
        .docs_dir
        .clone()
        .unwrap_or_else(|| "The one that comes with the editor".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every node gets an id, and the ones that had one keep it.
    #[test]
    fn every_node_is_numbered() {
        let mut tree = json!({"id": 5, "type": "layout", "children": [
            {"type": "label"},
            {"id": 9, "type": "layout", "children": [{"type": "separator"}]},
        ]});
        let next = number(&mut tree, 100);
        assert_eq!(next, 102);
        assert_eq!(tree["id"], 5);
        assert_eq!(tree["children"][0]["id"], 100);
        assert_eq!(tree["children"][1]["id"], 9);
        assert_eq!(tree["children"][1]["children"][0]["id"], 101);
    }

    /// **The editor's page is named Code**: the page the post window is on
    /// is renamed while it has another name, and left alone once it has.
    #[test]
    fn the_post_window_s_page_is_named_code() {
        let state = |name: &str| {
            json!({"pages": [
                {"name": "Tracks", "layout": {"tabs": [7], "active": 7}},
                {"name": name, "layout": {"tabs": [POST_WIN], "active": POST_WIN}},
            ]})
        };
        assert_eq!(
            page_rename(&state("Main")),
            Some(json!({"verb": "page_rename", "name": "Main", "to": "Code"}))
        );
        assert_eq!(page_rename(&state("Code")), None);
        assert_eq!(page_rename(&json!({"pages": []})), None);
    }

    /// The groups of a dock's state are read from its bars, in tab order.
    #[test]
    fn the_groups_are_its_bars() {
        let state = json!({"bars": [{"panes": [1000, 1016]}, {"panes": [3]}]});
        assert_eq!(groups(&state), vec![vec![1000, 1016], vec![3]]);
        assert!(groups(&json!({})).is_empty());
    }

    /// A hidden entry goes with the separator it leaves alone.
    #[test]
    fn a_hidden_entry_takes_its_line() {
        let mut menu = json!([{"label": "File", "menu": [
            {"label": "New", "verb": "new_file"},
            "-",
            {"label": "Quit", "verb": "quit"},
        ]}, {"label": "View", "menu": [{"label": "Font", "menu": []}]}]);
        hide(&mut menu, &["quit", "Font"]);
        assert_eq!(
            menu[0]["menu"],
            json!([{"label": "New", "verb": "new_file"}])
        );
        assert_eq!(menu[1]["menu"], json!([]));
    }

    /// A file window's close form stays inside the ids the window owns.
    #[test]
    fn a_file_form_is_inside_its_window() {
        let ids = form(FILE_BASE);
        for id in [ids.stack, ids.dialog, ids.save, ids.discard, ids.cancel] {
            assert!((FILE_BASE + 3..FILE_BASE + FILE_SPARE).contains(&id));
        }
    }
}
