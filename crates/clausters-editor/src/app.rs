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
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use clausters_core::osc::OscType;
use serde_json::{Value, json};

use clausters_apps::closing;

use crate::desktop::{self, Purpose};
use crate::docs::Book;
use crate::session::Session;
use crate::settings::{Settings, ThemePref};
use crate::wire::{self, Link, Said};
use crate::{config, envs, theme};

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
}

/// Every book and every document.
pub type Docs = (Vec<Book>, BTreeMap<String, String>);

// -- the fixed windows, and the ids of what is inside them

const POST_WIN: i32 = 1;
const POST: i32 = 2;
const DOCS_WIN: i32 = 3;
const DOCS: i32 = 4;
/// The application's bar, along the dock's top: the menu and the tools.
const BAR_WIN: i32 = 5;
/// The bar's height, in logical pixels.
const BAR_H: i32 = 40;
/// The application's footer, along the dock's bottom: the session's state.
const FOOT_WIN: i32 = 11;
const FOOT: i32 = 12;
/// The footer's height, in logical pixels.
const FOOT_H: i32 = 24;
/// The footer's text size: 12 pixels to the em of the interface's face
/// (Inter, whose capital is 0.727 of the em, over the 7-pixel cell).
const FOOT_TEXT: f64 = 12.0 * 0.727 / 7.0;

const PREFS_WIN: i32 = 20;
const PREFS_REOPEN: i32 = 22;
const PREFS_INDENT: i32 = 23;
const PREFS_THEME: i32 = 24;
const PREFS_DOCS_DIR: i32 = 25;
const PREFS_DOCS_CHOOSE: i32 = 27;
const PREFS_DOCS_BUNDLED: i32 = 28;
const PREFS_STARTUP: i32 = 30;
const PREFS_SAVE: i32 = 32;
const PREFS_SAVE_RESTART: i32 = 33;
const PREFS_CLOSE: i32 = 35;

const ENVS_WIN: i32 = 40;
const ENVS_LIST: i32 = 42;
const ENVS_SOURCE: i32 = 71;
const ENVS_NAME: i32 = 72;
const ENVS_CREATE: i32 = 73;
const ENVS_STATUS: i32 = 74;
const ENVS_CLOSE: i32 = 75;
/// The rows of the list, redefined whole: ten ids a row from here.
const ENVS_ROWS: i32 = 300;
/// Where the nodes of the two panels with no role of their own are numbered
/// from.
const PREFS_SPARE: i32 = 100;
const ENVS_SPARE: i32 = 150;

/// A file window's ids, from its base: the window, its column, the code,
/// the stack its dialog is a page of, the empty page, the dialog, its text,
/// its row of buttons and the two buttons.
const FILE_BASE: i32 = 1000;
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

/// How long the closes a window's own close sends (one per pane) are
/// gathered before they are read as one, counted from the last. A desktop's
/// window draws each pane as its close is told, so the closes of one window
/// arrive tens of milliseconds apart; 60 ms split them, and the window's close
/// was read as a tab's.
const CLOSE_BURST: Duration = Duration::from_millis(400);

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
    tx: Sender<Msg>,
    session: Session,
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
    system_dark: bool,
    dock_calls: HashMap<i32, DockFor>,
    /// The closes gathered, and when the last arrived.
    closes: (HashSet<i32>, Option<Instant>),
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
}

impl App {
    pub fn new(link: Link, tx: Sender<Msg>, settings: Settings) -> Self {
        let session = Session::new(tx.clone());
        Self {
            link,
            tx,
            session,
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
            system_dark: false,
            dock_calls: HashMap::new(),
            closes: (HashSet::new(), None),
            prefs_open: false,
            envs_open: false,
            fields: HashMap::new(),
            help_waiting: None,
            quitting: false,
            saving_startup: None,
            split: false,
        }
    }

    /// **The application's turn**: what is open at start, then every
    /// message as it comes, until the editor ends.
    pub fn run(mut self, rx: Receiver<Msg>, files: Vec<PathBuf>) {
        self.start(files);
        loop {
            let wait = match self.closes.1 {
                Some(at) => CLOSE_BURST.saturating_sub(at.elapsed()),
                None => Duration::from_secs(3600),
            };
            match rx.recv_timeout(wait) {
                Ok(msg) => self.on(msg),
                Err(RecvTimeoutError::Timeout) => {
                    if self.closes.1.is_some() {
                        self.closed_burst();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn start(&mut self, cli: Vec<PathBuf>) {
        self.apply_theme();
        desktop::follow_color_scheme(&self.tx);
        self.read_docs();
        self.open_bar();
        self.open_footer();
        self.open_post();
        self.open_docs();
        // the files of the last run, then the ones the command line names,
        // the last of them on screen
        let mut opened = false;
        if self.settings.reopen_files {
            for path in self.settings.open_files.clone() {
                opened |= self.open_path(Path::new(&path), false);
            }
        }
        for path in cli {
            if path.exists() {
                opened |= self.open_path(&path, true);
            } else {
                self.open_missing(&path);
                opened = true;
            }
        }
        if !opened {
            self.new_file();
        }
        if let Some(active) = self.settings.active_file.clone()
            && self.settings.reopen_files
            && let Some(win) = self.file_at(Path::new(&active)).map(|f| f.win)
        {
            self.show_file(win);
        }
        self.dock(json!({"verb": "layout"}), DockFor::Arrange);
        if let Err(e) = self.session.start() {
            self.post("err", &format!("{e}\n"));
        }
    }

    fn on(&mut self, msg: Msg) {
        match msg {
            Msg::Host(said) => self.on_host(said),
            Msg::Py(m) => self.on_py(m),
            Msg::PyStart { generation, env } => {
                self.py.generation = generation;
                self.py.env = env;
                self.py.busy.clear();
                self.read_docs();
                self.say_session();
            }
            Msg::PyExit { generation, code } => {
                if generation != self.py.generation {
                    return;
                }
                self.py.alive = false;
                self.py.busy.clear();
                let code = code.map_or_else(|| "?".to_string(), |c| c.to_string());
                self.post("info", &format!("Python session ended (code {code})\n"));
                self.say_session();
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
            Msg::SystemDark(dark) => {
                self.system_dark = dark;
                if self.settings.theme == ThemePref::System {
                    self.apply_theme();
                }
            }
        }
    }

    // -- what the host said

    fn on_host(&mut self, said: Said) {
        match said {
            Said::Event { id, tag, args } => self.on_event(id, &tag, &args),
            Said::Closed(id) => self.on_closed(id),
            Said::Dock { seq, reply } => self.on_dock(seq, &reply),
            Said::Info { id, props } => self.on_info(id, &props),
            Said::Other => {}
        }
    }

    fn on_event(&mut self, id: i32, tag: &str, args: &[OscType]) {
        // a window's own: a close, a menu pick, a verb no element took
        if self.is_window(id) {
            return match tag {
                "close" => {
                    tracing::debug!("close asked of window {id}");
                    self.closes.0.insert(id);
                    self.closes.1 = Some(Instant::now());
                }
                "menu" => self.verb(id, &wire::text(args, 0), args.get(1).is_some_and(on)),
                "save" => self.save(self.file_win(id), false),
                verb if !verb.is_empty() => self.verb(id, verb, true),
                _ => {}
            };
        }
        match id {
            POST | DOCS => match tag {
                "link" => open_url(&wire::text(args, 0)),
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
                self.verb(PREFS_WIN, verb, true);
            }
            PREFS_DOCS_CHOOSE if tag == "click" => self.choose_docs_folder(),
            PREFS_DOCS_BUNDLED if tag == "click" => self.set_docs_dir(None),
            PREFS_SAVE if tag == "click" => self.ask_startup_code(false),
            PREFS_SAVE_RESTART if tag == "click" => self.ask_startup_code(true),
            PREFS_CLOSE if tag == "click" => self.link.free(PREFS_WIN),
            ENVS_SOURCE | ENVS_NAME if tag.is_empty() => {
                self.fields.insert(id, wire::text(args, 0));
            }
            ENVS_CREATE if tag == "click" => self.envs_create(),
            ENVS_CLOSE if tag == "click" => self.link.free(ENVS_WIN),
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
        match id {
            // the post window is freed by nobody but the quit: gone, it is the
            // dock's window closed over what was asked, and the editor ends
            POST_WIN => return self.quit_now(),
            // the bar and the footer go with the dock's window, as the post
            // window does
            BAR_WIN | FOOT_WIN => return,
            DOCS_WIN => self.docs_shown = false,
            PREFS_WIN => self.prefs_open = false,
            ENVS_WIN => self.envs_open = false,
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

    /// **The closes gathered**: every pane at once is the dock's window
    /// closing, which is quitting; otherwise each is its pane's own.
    fn closed_burst(&mut self) {
        let closes = std::mem::take(&mut self.closes.0);
        self.closes.1 = None;
        let mut panes: HashSet<i32> = self.files.iter().map(|f| f.win).collect();
        panes.insert(BAR_WIN);
        panes.insert(FOOT_WIN);
        panes.insert(POST_WIN);
        for (shown, win) in [
            (self.docs_shown, DOCS_WIN),
            (self.prefs_open, PREFS_WIN),
            (self.envs_open, ENVS_WIN),
        ] {
            if shown {
                panes.insert(win);
            }
        }
        tracing::debug!("closes {closes:?} of the panes {panes:?}");
        if panes.is_subset(&closes) {
            return self.quit();
        }
        for win in closes {
            match win {
                // the post window stays: it is where the session speaks; the
                // bar is the application's, and goes only with it
                BAR_WIN | FOOT_WIN | POST_WIN => {}
                DOCS_WIN | PREFS_WIN | ENVS_WIN => self.link.free(win),
                win => self.close_tab(Some(win)),
            }
        }
    }

    fn on_dock(&mut self, seq: i32, reply: &Value) {
        let Some(purpose) = self.dock_calls.remove(&seq) else {
            return;
        };
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
        let groups = groups(state);
        let split = self.editor_groups(&groups).len() > 1;
        if split != self.split {
            self.split = split;
            self.menus();
        }
        match purpose {
            DockFor::Arrange => self.arrange(&groups),
            DockFor::Split => self.split_or_join(&groups),
            DockFor::MoveTab => self.move_tab(&groups),
            DockFor::Quit => self.quit_now(),
            DockFor::Look => {}
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
        let win = id - CODE;
        match std::fs::write(&path, &text) {
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
                    self.open_path(Path::new(&path), true);
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

    /// Opens `path` in a tab, or shows the tab that has it; `false` when it
    /// could not be read.
    fn open_path(&mut self, path: &Path, remember: bool) -> bool {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(win) = self.file_at(&path).map(|f| f.win) {
            self.show_file(win);
            return true;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let blank = self.blank_tab();
                self.open_window(Some(path.clone()), file_name(&path), text);
                if let Some(win) = blank {
                    self.link.free(win);
                }
                if remember {
                    self.settings.add_recent(&path.to_string_lossy());
                    self.settings.save();
                    self.menus();
                }
                true
            }
            Err(e) => {
                let shown = path.to_string_lossy().into_owned();
                self.settings.recent.retain(|p| *p != shown);
                self.settings.save();
                self.post("info", &format!("Could not read {shown}: {e}\n"));
                false
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
                    "id": win + CODE, "type": "code", "text": text, "language": "python",
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
                let start = path
                    .clone()
                    .unwrap_or_else(|| home().join(format!("{}.py", file.name.replace(' ', "_"))));
                desktop::choose(&self.tx, Purpose::SaveAs { win: file.win }, Some(start));
            }
        }
    }

    fn choose(&mut self, purpose: Purpose, win: Option<i32>) {
        let start = win
            .and_then(|w| self.files.iter().find(|f| f.win == w))
            .and_then(|f| f.path.clone())
            .or_else(|| self.settings.recent.first().map(PathBuf::from));
        desktop::choose(&self.tx, purpose, start);
    }

    fn chosen(&mut self, purpose: Purpose, path: Option<PathBuf>) {
        let Some(path) = path else {
            if let Purpose::SaveAs { win } = purpose {
                self.after_save.remove(&win);
            }
            return;
        };
        match purpose {
            Purpose::Open => {
                self.open_path(&path, true);
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
            let question = format!("Save \"{name}\"?");
            return self.ask(win, Ask::CloseTab(win), &question);
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

    /// The close form up in window `win`, asking `question`: the form's
    /// stack defined again around the question, shown.
    fn ask(&mut self, win: i32, what: Ask, question: &str) {
        self.asking = Some((win, what));
        self.show_file(win);
        let ids = form(win);
        let mut stack = closing::stack(&ids, question);
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
                self.quit_saved();
            }
            _ => {}
        }
    }

    // -- quitting

    fn quit(&mut self) {
        tracing::debug!("quit asked");
        let unsaved: Vec<&File> = self.files.iter().filter(|f| f.dirty).collect();
        let Some(first) = unsaved.first() else {
            return self.quit_saved();
        };
        let question = if unsaved.len() == 1 {
            format!("Save \"{}\"?", first.name)
        } else {
            let names: Vec<String> = unsaved.iter().map(|f| format!("\"{}\"", f.name)).collect();
            format!("Save {}?", names.join(", "))
        };
        let win = first.win;
        self.ask(win, Ask::Quit, &question);
    }

    /// Quits once the window's size is read, to open at it next time.
    fn quit_saved(&mut self) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        self.remember_files();
        self.dock(json!({"verb": "layout"}), DockFor::Quit);
        // a dock that does not answer does not keep the editor open
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            let _ = tx.send(Msg::Host(Said::Dock {
                seq: i32::MAX,
                reply: Value::Null,
            }));
        });
        self.dock_calls.insert(i32::MAX, DockFor::Quit);
    }

    /// **The editor ends**: what it remembers is written, the session is
    /// stopped and the host's loop is asked to end, which ends the program on
    /// the thread that holds the windows. Ending it from this thread instead
    /// left a desktop's process alive after its window had gone.
    fn quit_now(&mut self) {
        tracing::debug!("quitting");
        self.settings.save();
        self.session.kill();
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

    /// **The first arrangement**, as the webview application's: the editor
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
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                let _ = tx.send(Msg::Arrange);
            });
            return;
        }
        let Some(first) = self.files.first().map(|f| f.win) else {
            return;
        };
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
        matches!(
            id,
            BAR_WIN | FOOT_WIN | POST_WIN | DOCS_WIN | PREFS_WIN | ENVS_WIN
        ) || self.files.iter().any(|f| f.win == id)
    }

    // -- the application's bar, the post window and the documentation

    /// **The application's bar**: a window along the dock's top, across it,
    /// holding the menu and the tools -- the application's, never a
    /// panel's. A tool performs the verb its menu entry does, and its tip
    /// says the key.
    fn open_bar(&mut self) {
        let mut tree = self.window(BAR_WIN, "Editor");
        tree["dock_edge"] = json!("top");
        tree["h"] = json!(BAR_H);
        let tool = |id: i32, symbol: char, verb: &str, tip: &str| {
            json!({"id": id, "type": "button", "label": symbol.to_string(), "flat": 1,
                "verb": verb, "tip": tip})
        };
        tree["children"] = json!([{
            "id": BAR_WIN + 1, "type": "layout", "flow": "row", "pack": 1,
            "margin": 4, "gap": 2, "children": [
                tool(BAR_WIN + 2, '\u{25B6}', "evaluate_line", "Evaluate line or selection"),
                tool(BAR_WIN + 3, '\u{2193}', "evaluate_block", "Evaluate block"),
                tool(BAR_WIN + 4, '\u{25A0}', "interrupt", "Interrupt"),
                tool(BAR_WIN + 5, '\u{21BB}', "restart", "Restart the session"),
            ],
        }]);
        self.link.def(BAR_WIN, &tree);
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
        tree["children"] = json!([{
            "id": DOCS, "type": "docs", "text_size": self.settings.text_size(),
        }]);
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
    }

    fn read_docs(&self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Docs(crate::docs::read_all()));
        });
    }

    fn feed_docs(&self) {
        let Some((books, documents)) = &self.docs else {
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
        let start = config::get().docs_dir.map(PathBuf::from);
        desktop::choose(&self.tx, Purpose::DocsFolder, start);
    }

    fn set_docs_dir(&mut self, dir: Option<String>) {
        if let Some(d) = &dir
            && !Path::new(d).is_dir()
        {
            return self.post("info", &format!("Not a folder: {d}\n"));
        }
        match config::update(|c| c.docs_dir = dir) {
            Ok(config) => {
                self.read_docs();
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
                self.post("info", &format!("Python {} ready\n", self.py.version));
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
    /// answer, and the element is told so at once.
    fn query(&mut self, op: &str, mut request: Value, query: Query) {
        if !self.py.alive {
            let empty = json!({"type": op, "data": null, "text": ""});
            return self.answered(query, &empty);
        }
        let id = self.next_request;
        self.next_request += 1;
        request["id"] = json!(id);
        request["op"] = json!(op);
        self.queries.insert(id, query);
        if let Err(e) = self.session.send(&request) {
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
        if let Err(e) = self.session.send(&request) {
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
        self.post("info", "Startup code (Python):\n");
        self.post("out", &format!("{}\n", code.trim_end()));
        self.eval(&code, "<startup code>", 1);
    }

    fn interrupt(&mut self) {
        if let Err(e) = self.session.interrupt() {
            self.post("info", &format!("{e}\n"));
        }
    }

    fn restart(&mut self) {
        self.post("info", "Restarting the Python session...\n");
        self.py.alive = false;
        if let Err(e) = self.session.restart() {
            self.post("err", &format!("{e}\n"));
        }
    }

    /// **The session's state, in the footer**: its dot (stopped, running,
    /// ready) and `Python <version> | <environment> | <state>`.
    fn say_session(&self) {
        let state = if !self.py.alive {
            "stopped".to_string()
        } else if self.py.busy.is_empty() {
            "ready".to_string()
        } else {
            format!("running ({})", self.py.busy.len())
        };
        let env = match &self.py.env {
            Some(name) => format!("environment \"{name}\""),
            None => "system".into(),
        };
        let dot = if !self.py.alive {
            "destructive"
        } else if self.py.busy.is_empty() {
            "success"
        } else {
            "warning"
        };
        let python = if self.py.version.is_empty() {
            "Python".to_string()
        } else {
            format!("Python {}", self.py.version)
        };
        let line = format!("{python} | {env} | {state}");
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
            vec![json!({"label": "No recent files", "enabled": false})]
        } else {
            self.settings
                .recent
                .iter()
                .enumerate()
                .map(|(i, p)| json!({"label": recent_label(p), "verb": format!("recent_{i}")}))
                .collect()
        };
        let mut recent = recent;
        recent.push(json!("-"));
        recent.push(
            json!({"label": "Clear recent files", "verb": "clear_recent",
            "enabled": !self.settings.recent.is_empty()}),
        );
        let theme = |verb: &str, label: &str, pref: ThemePref| {
            json!({"label": label, "verb": verb, "group": "theme",
                "checked": self.settings.theme == pref})
        };
        let split = self.editor_split();
        json!([
            {"label": "File", "menu": [
                {"label": "New", "verb": "new_file"},
                {"label": "Open...", "verb": "open_file"},
                {"label": "Open recent", "menu": recent},
                "-",
                {"label": "Save", "verb": "save"},
                {"label": "Save as...", "verb": "save_as"},
                "-",
                {"label": "Close tab", "verb": "close_tab"},
                "-",
                {"label": "Preferences...", "verb": "preferences"},
                "-",
                {"label": "Quit", "verb": "quit"},
            ]},
            {"label": "Edit", "menu": [
                {"label": "Undo", "verb": "undo"},
                {"label": "Redo", "verb": "redo"},
                "-",
                {"label": "Cut", "verb": "cut"},
                {"label": "Copy", "verb": "copy"},
                {"label": "Paste", "verb": "paste"},
                {"label": "Select all", "verb": "select_all"},
                "-",
                {"label": "Find and replace", "verb": "find"},
                {"label": "Find next", "verb": "find_next"},
                {"label": "Go to line", "verb": "go_to_line"},
                {"label": "Toggle comment", "verb": "toggle_comment"},
            ]},
            {"label": "Session", "menu": [
                {"label": "Evaluate line or selection", "verb": "evaluate_line"},
                {"label": "Evaluate block", "verb": "evaluate_block"},
                {"label": "Evaluate contiguous lines as a block", "verb": "eval_paragraph",
                    "checked": self.settings.eval_paragraph},
                "-",
                {"label": "Interrupt", "verb": "interrupt"},
                {"label": "Restart session", "verb": "restart"},
                {"label": "Python environments...", "verb": "environments"},
                "-",
                {"label": "Clear post window", "verb": "clear_post"},
            ]},
            {"label": "View", "menu": [
                {"label": "Theme", "menu": [
                    theme("theme_light", "Light", ThemePref::Light),
                    theme("theme_dark", "Dark", ThemePref::Dark),
                    theme("theme_system", "System", ThemePref::System),
                ]},
                "-",
                {"label": "Documentation", "verb": "show_docs", "checked": self.docs_shown},
                {"label": "Split editor", "verb": "split_editor", "checked": split},
                {"label": "Move tab to other editor", "verb": "move_tab", "enabled": split},
                "-",
                {"label": "Increase size", "verb": "zoom_in"},
                {"label": "Decrease size", "verb": "zoom_out"},
                {"label": "Default size", "verb": "zoom_reset"},
            ]},
            {"label": "Help", "menu": [
                {"label": "Documentation", "verb": "docs"},
                {"label": "Search the documentation", "verb": "search_docs"},
                {"label": "Documentation folder...", "verb": "docs_folder"},
                "-",
                {"label": "Keyboard shortcuts", "verb": "keys"},
            ]},
        ])
    }

    /// Whether the files are in more than one group, as last seen.
    fn editor_split(&self) -> bool {
        self.split
    }

    /// The application's menu, as the state now reads.
    fn menus(&self) {
        self.link.set(BAR_WIN, &[("menu", self.menu())]);
    }

    // -- the preferences, a panel of the dock

    fn open_prefs(&mut self) {
        if self.prefs_open {
            self.dock(json!({"verb": "select", "pane": PREFS_WIN}), DockFor::Look);
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
        let hint = |text: &str| json!({"type": "label", "text": text, "wrap": 1});
        let mut tree = self.window(PREFS_WIN, "Preferences");
        tree["children"] = json!([{
            "id": 21, "type": "layout", "flow": "col", "margin": 12, "gap": 8,
            "children": [
                {"id": PREFS_REOPEN, "type": "toggle",
                    "label": "Open the last session's files at startup",
                    "value": i32::from(self.settings.reopen_files)},
                {"id": PREFS_THEME, "type": "choice", "label": "Theme",
                    "options": ["Light", "Dark", "System"], "index": theme},
                {"id": PREFS_INDENT, "type": "choice", "label": "Indentation",
                    "options": ["Tab character", "2 spaces", "4 spaces"], "index": indent},
                hint("What Tab inserts at the caret, and one level of indentation for the selected lines."),
                {"type": "label", "text": "Documentation folder (.md files)"},
                {"id": PREFS_DOCS_DIR, "type": "label", "text": docs_dir_text(&config)},
                {"id": 26, "type": "layout", "flow": "row", "pack": 1, "children": [
                    {"id": PREFS_DOCS_CHOOSE, "type": "button", "label": "Choose folder..."},
                    {"id": PREFS_DOCS_BUNDLED, "type": "button", "label": "Use the one that comes with the editor"},
                ]},
                {"type": "label", "text": "Startup code"},
                {"id": PREFS_STARTUP, "type": "code", "text": config.startup_code,
                    "language": "python", "h": 140, "line_numbers": 0,
                    "indent": self.settings.indent_prop(),
                    "text_size": self.settings.text_size()},
                hint("Runs every time the session starts (when it is first needed, when it is restarted or when its environment changes) and is shown in the post window."),
                {"id": 31, "type": "layout", "flow": "row", "pack": 1, "children": [
                    {"id": PREFS_SAVE, "type": "button", "label": "Save"},
                    {"id": PREFS_SAVE_RESTART, "type": "button", "label": "Save and restart the session"},
                ]},
                {"type": "separator", "weight": 1, "line": 0},
                {"id": 34, "type": "layout", "flow": "row", "pack": 1, "children": [
                    {"type": "separator", "weight": 1, "line": 0},
                    {"id": PREFS_CLOSE, "type": "button", "label": "Close"},
                ]},
            ]
        }]);
        number(&mut tree, PREFS_SPARE);
        self.link.def(PREFS_WIN, &tree);
        self.prefs_open = true;
        self.beside_editor(PREFS_WIN);
        self.menus();
    }

    /// A panel opened from the editor, as a tab of the editor's group.
    fn beside_editor(&mut self, win: i32) {
        if let Some(of) = self.file_win(0) {
            self.link
                .dock(&json!({"verb": "join", "pane": win, "of": of}));
        }
        self.dock(json!({"verb": "select", "pane": win}), DockFor::Look);
    }

    /// The startup code is read back from its editor, then saved.
    fn ask_startup_code(&mut self, and_restart: bool) {
        self.saving_startup = Some(and_restart);
        self.link.query(PREFS_STARTUP);
    }

    // -- the environments, a panel of the dock

    fn open_envs(&mut self) {
        if self.envs_open {
            self.dock(json!({"verb": "select", "pane": ENVS_WIN}), DockFor::Look);
            return;
        }
        let hint = |text: &str| json!({"type": "label", "text": text, "wrap": 1});
        let mut tree = self.window(ENVS_WIN, "Python environments");
        tree["children"] = json!([{
            "id": 41, "type": "layout", "flow": "col", "margin": 12, "gap": 8,
            "children": [
                {"type": "label", "text": "Environments"},
                self.envs_list(),
                {"type": "label", "text": "Create a new environment"},
                {"id": ENVS_SOURCE, "type": "text", "label": "1. Install source",
                    "value": self.fields.get(&ENVS_SOURCE).cloned().unwrap_or_default()},
                hint("What pip installs in it: a package name, a path or a URL, with any pip options."),
                {"id": ENVS_NAME, "type": "text", "label": "2. Environment name",
                    "value": self.fields.get(&ENVS_NAME).cloned().unwrap_or_default()},
                hint("A short name: letters, digits, \".\", \"_\" or \"-\"."),
                {"id": 76, "type": "layout", "flow": "row", "pack": 1, "children": [
                    {"id": ENVS_CREATE, "type": "button", "label": "Create"},
                ]},
                {"id": ENVS_STATUS, "type": "label", "text": "", "wrap": 1},
                {"type": "separator", "weight": 1, "line": 0},
                {"id": 77, "type": "layout", "flow": "row", "pack": 1, "children": [
                    {"type": "separator", "weight": 1, "line": 0},
                    {"id": ENVS_CLOSE, "type": "button", "label": "Close"},
                ]},
            ]
        }]);
        number(&mut tree, ENVS_SPARE);
        self.link.def(ENVS_WIN, &tree);
        self.envs_open = true;
        self.beside_editor(ENVS_WIN);
        self.menus();
    }

    /// The list of environments, a row each, the system's Python first.
    fn envs_list(&self) -> Value {
        let envs = envs::list_envs();
        let system_active = !envs.iter().any(|e| e.active);
        let row = |i: i32, text: String, active: bool, tools: bool| {
            let base = ENVS_ROWS + i * 10;
            let mut children =
                vec![json!({"id": base + 1, "type": "label", "text": text, "weight": 1})];
            if active {
                children.push(json!({"id": base + 2, "type": "label", "text": "in use"}));
            } else {
                children.push(json!({"id": base + 3, "type": "button", "label": "Use"}));
            }
            if tools {
                children.push(json!({"id": base + 4, "type": "button", "label": "Update"}));
                children.push(json!({"id": base + 5, "type": "button", "label": "Delete"}));
            }
            json!({"id": base, "type": "layout", "flow": "row", "pack": 1, "children": children})
        };
        let mut rows = vec![row(
            0,
            format!("System Python ({})", crate::session::default_python()),
            system_active,
            false,
        )];
        for (i, e) in envs.iter().enumerate() {
            let mut text = e.name.clone();
            if !e.version.is_empty() {
                text.push_str(&format!("  Python {}", e.version));
            }
            if !e.source.is_empty() {
                text.push_str(&format!("  ({})", e.source));
            }
            rows.push(row(i as i32 + 1, text, e.active, true));
        }
        json!({"id": ENVS_LIST, "type": "layout", "flow": "col", "gap": 4, "children": rows})
    }

    fn envs_refresh(&mut self, status: &str) {
        if !self.envs_open {
            return;
        }
        self.link.def(ENVS_LIST, &self.envs_list());
        self.link.set(ENVS_STATUS, &[("text", json!(status))]);
    }

    fn envs_row(&mut self, id: i32) {
        let i = (id - ENVS_ROWS) / 10;
        let what = (id - ENVS_ROWS) % 10;
        let name = if i == 0 {
            None
        } else {
            envs::list_envs()
                .get(i as usize - 1)
                .map(|e| e.name.clone())
        };
        let result = match (what, name) {
            (3, name) => envs::use_env(&self.session, name),
            (4, Some(name)) => {
                self.link.set(
                    ENVS_STATUS,
                    &[(
                        "text",
                        json!(format!(
                            "Updating \"{name}\"... (progress in the post window)"
                        )),
                    )],
                );
                return self.report_envs(envs::install_in_env(&self.tx, &self.session, name));
            }
            (5, Some(name)) => envs::delete_env(&self.session, name),
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
            return self.link.set(
                ENVS_STATUS,
                &[("text", json!("Name the environment first."))],
            );
        }
        self.link.set(
            ENVS_STATUS,
            &[(
                "text",
                json!(format!(
                    "Creating \"{name}\"... (progress in the post window)"
                )),
            )],
        );
        let result = envs::create_env(&self.tx, &self.session, name, source);
        self.report_envs(result);
    }

    fn report_envs(&mut self, started: Result<(), String>) {
        if let Err(e) = started {
            self.link.set(ENVS_STATUS, &[("text", json!(e))]);
        }
    }
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

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// A recent file as a menu lists it: its name, then its folder, written
/// from `~` under the home folder.
fn recent_label(path: &str) -> String {
    let p = Path::new(path);
    let dir = p
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let home = home().to_string_lossy().into_owned();
    let dir = match dir.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => dir,
    };
    format!("{}    {dir}", file_name(p))
}

fn docs_dir_text(config: &config::Config) -> String {
    config
        .docs_dir
        .clone()
        .unwrap_or_else(|| "The one that comes with the editor".into())
}

/// Opens a web address in the desktop's browser.
fn open_url(url: &str) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program).arg(url).spawn() {
        tracing::warn!("cannot open {url}: {e}");
    }
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

    /// The groups of a dock's state are read from its bars, in tab order.
    #[test]
    fn the_groups_are_its_bars() {
        let state = json!({"bars": [{"panes": [1000, 1016]}, {"panes": [3]}]});
        assert_eq!(groups(&state), vec![vec![1000, 1016], vec![3]]);
        assert!(groups(&json!({})).is_empty());
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
