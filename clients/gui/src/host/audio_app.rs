//! **The audio editor's application, on a thread of its own**: its
//! coordinator, its editing context, its engine and its working folders,
//! spoken to by the host with messages and answering with what the host
//! draws.
//!
//! The window never waits on the application. A file opened, a header read,
//! a history step carried to its end with its renders, a working folder made
//! or swept: each touches the disk, and some take as long as a take is long.
//! On a thread of its own the application may wait on all of them, natively
//! and in a page, where the thread is a worker (`clausters_core::threads`)
//! and the host's main thread cannot wait at all. The host
//! (`super::audio_editor`) keeps the windows, their widget ids and their
//! drawing; what it asks crosses as a [`ToApp`], and what comes back as a
//! [`FromApp`], already carrying what drawing it needs (a window's tree, a
//! widget's props, the actions' readings, and the takes those name, handed
//! over by reference ahead of them), so the host never asks the application
//! anything synchronously.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clausters_app_engine::AppEngine;
use clausters_app_engine::app::{App as Application, Raised, Ran};
use clausters_apps::editing::{Editing, MemberId};
use serde_json::{Value, json};

use super::ClientId;

/// How long the application's thread sleeps with nothing asked of it, when
/// a render may land meanwhile; an ask wakes it sooner. Parked rather than
/// waiting on its channel with a timeout, which reads a clock a page's
/// threads do not have.
const IDLE: Duration = Duration::from_millis(5);

/// How long it sleeps when nothing under way can land (`App::busy`): only
/// the engine's garbage is taken in meanwhile, and an ask still wakes it.
const REST: Duration = Duration::from_millis(100);

/// What makes the application's engine, on its thread: the machine's output
/// device, or one a caller brings (a test driving it block by block).
pub(super) type MakeEngine = Box<dyn FnOnce() -> Result<Arc<Mutex<AppEngine>>, String> + Send>;

/// **Who waits for an answer**: a client of the host's application door,
/// answered through the host, or a test in the host's own process, answered
/// on its own channel.
pub(super) enum Reply {
    Client(ClientId, i32),
    #[cfg(test)]
    Local(Sender<Result<Value, (String, String)>>),
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reply::Client(to, seq) => write!(f, "Client({to}, {seq})"),
            #[cfg(test)]
            Reply::Local(_) => f.write_str("Local"),
        }
    }
}

/// **Which application a window is**.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AppKind {
    /// The audio editor, over a sound file.
    Audio,
    /// The multitrack editor, over a session.
    Multitrack,
    /// **The roll over a box of notes** of multitrack window `parent`: a
    /// notes member over the box's sequence (`source`), in the same context.
    Roll { source: u64, parent: i32 },
    /// The score editor, over a symbolic score: its document and its engraver
    /// are the application's.
    #[cfg(feature = "notation")]
    Score,
    /// **A roll a client opened** over a sequence it handed over, which the
    /// window holds from then on.
    Notes,
}

/// The widget roles a score window is numbered with: the page, the scroll it
/// sits in and the status line.
#[cfg(feature = "notation")]
pub(super) const SCORE_ROLES: &[&str] = &["widget", "scroll", "status"];

/// The size a score window opens at, in logical pixels.
#[cfg(feature = "notation")]
const SCORE_SIZE: (i64, i64) = (960, 640);

/// **Where a score window opens and what it keeps**: its kind, the order it
/// joins (`key`, `place`), its title, and the file it read and where its
/// Save writes.
#[cfg(feature = "notation")]
struct ScoreOpen<'a> {
    kind: AppKind,
    key: String,
    place: clausters_apps::editing::Place,
    title: &'a str,
    session: Option<String>,
    save_to: Option<Option<String>>,
}

/// The size a roll's window opens at, in logical pixels.
const ROLL_SIZE: (i64, i64) = (1000, 520);

/// The widget roles a multitrack window is numbered with: the multitrack,
/// its ruler, its transport row and the form a close asks in.
pub(super) const MULTITRACK_ROLES: &[&str] = &[
    "widget",
    "ruler",
    "row",
    "rewind",
    "play",
    "stop",
    "clock",
    "close_stack",
    "close_dialog",
    "close_save",
    "close_discard",
    "close_cancel",
];

/// The size a multitrack window opens at, in logical pixels.
const MULTITRACK_SIZE: (i64, i64) = (1000, 560);

/// Contents handed over by a client, or a session a handler opens again
/// over what a window handed back; or a symbolic score, as MEI.
#[derive(Debug)]
pub(super) enum AppData {
    Contents(Value),
    Snapshot(Box<clausters_apps::multitrack::operations::Snapshot>),
    /// A symbolic score: the document handed over, or none for what its
    /// `path` holds or a blank page; the window it is another view beside;
    /// and how the editor opens (`request`: its size, its chrome, the value
    /// in hand, and over a sequence the sequence and how it is read).
    #[cfg(feature = "notation")]
    Score {
        mei: Option<String>,
        over: Option<i32>,
        request: Value,
    },
    /// A sequence of events: handed over (`sequence` in `request`), or that
    /// of the window `over`, which this one is another view beside.
    Notes {
        over: Option<i32>,
        request: Value,
    },
}

/// **A symbolic score read by the engraver this build holds**, or why not.
#[cfg(feature = "score")]
fn engraved(data: &str) -> Result<clausters_apps::score::Shared, String> {
    clausters_notation::open(data, &clausters_notation::EngraveOptions::default())
        .map(|score| Arc::new(Mutex::new(score)))
        .map_err(|e| format!("the engraver could not read the score: {e}"))
}

/// **A symbolic score read by the page's engraver**: verovio in a worker this
/// thread waits on.
#[cfg(all(target_arch = "wasm32", feature = "notation"))]
fn engraved(data: &str) -> Result<clausters_apps::score::Shared, String> {
    super::web::engraver::open(data).map(|score| Arc::new(Mutex::new(score)))
}

/// A build with no engraver opens no score.
#[cfg(all(
    feature = "notation",
    not(feature = "score"),
    not(target_arch = "wasm32")
))]
fn engraved(_: &str) -> Result<clausters_apps::score::Shared, String> {
    Err("this host was built without an engraver, so it opens no score".into())
}

/// What the host asks of the application.
#[derive(Debug)]
pub(super) enum ToApp {
    /// Open an application in window `def_id`: the audio editor over the
    /// file at `path`, the multitrack over the session at `path`, or either
    /// over none.
    Open {
        def_id: i32,
        kind: AppKind,
        key: String,
        title: String,
        path: Option<String>,
        /// The multitrack to open, as the document's JSON, when it has no
        /// file: an untitled session.
        data: Option<AppData>,
        /// Where a multitrack's Save writes, when the host that opened it
        /// said: `Some(None)` writes nowhere.
        save_to: Option<Option<String>>,
        reply: Option<Reply>,
    },
    /// The window's widget ids, every dialog's included, which its tree is
    /// drawn with: the editor binds the window and opens its file.
    Bind {
        def_id: i32,
        ids: Value,
        widgets: Vec<i32>,
        title: String,
    },
    /// A message of the window's: a gesture's `/gui_event`.
    Event {
        def_id: i32,
        addr: String,
        args: Vec<Value>,
    },
    /// One call through the application door ([`super::GUI_APP_CALL`]).
    Call {
        def_id: i32,
        call: Value,
        reply: Reply,
    },
    /// **A question the host answers once the window's turns before it are
    /// in**: the application has nothing to say to it and sends it back in
    /// its order, after every correction its earlier turns sent, so what the
    /// host then reads of the window is what those turns left (a selection
    /// a verb asked the application for).
    After {
        def_id: i32,
        call: Value,
        to: ClientId,
        seq: i32,
    },
    /// The window asked to close and goes: what sounded it stops, then it is
    /// forgotten.
    Close { def_id: i32 },
    /// **The person closed the window**, which asked nobody first (Escape,
    /// the close mark): the application is told as its own close tells it,
    /// and says the window went, with what it left.
    Closed { def_id: i32 },
    /// The window is gone: its renders are called off and its working
    /// folder removed.
    Forget { def_id: i32 },
    /// **A client is window `def_id`'s holder now**, which held none: the
    /// window it asked to open again. It is the client's to keep, as one it
    /// opened, and asks nothing before a close.
    Adopt { def_id: i32 },
}

/// What the application says back to the host.
pub(super) enum FromApp {
    /// The takes what follows names by number, handed over by reference
    /// (`clausters_apps::audio::shared`, the context's): sent before the answer that names
    /// them, so the host holds each one when it reads its number.
    Lists(Vec<(u64, clausters_document::parts::FileList)>),
    /// The engine is open: what its windows' meters and play cursors read.
    Engine(clausters_app_engine::EngineReading),
    /// Window `def_id`'s editor opened, and the dialogs whose widget ids it
    /// needs; or why it did not.
    Opened {
        def_id: i32,
        outcome: Result<Vec<String>, String>,
        reply: Option<Reply>,
    },
    /// **Window `def_id` was not opened**: the session it was asked to open
    /// (`path`) is window `window`'s, which comes forward instead. `adopt`
    /// says whether the open asked for the file alone, which the asker may
    /// take that window over for, or brought contents of its own.
    Held {
        def_id: i32,
        window: i32,
        path: String,
        adopt: bool,
        reply: Option<Reply>,
    },
    /// A run of window `def_id`'s editor, as far as it went: the effects
    /// that are the window's, with a `publish` carrying the window's tree and
    /// a `resync` every widget's props; the actions' readings when they
    /// moved; its end; and whether a failure is said in the window.
    Ran {
        def_id: i32,
        effects: Vec<Value>,
        actions: Option<Value>,
        end: Option<Result<Value, Raised>>,
        say: bool,
    },
    /// **A box of notes in multitrack window `parent` asked for its roll**
    /// (`source`, the sequence it reads). The host opens a window for it.
    Roll { parent: i32, source: u64 },
    /// **Multitrack window `def_id`'s multitrack moved by a hand**, to the
    /// context's `version`: its opener is told ([`super::GUI_APP_CHANGED`]).
    Changed { def_id: i32, version: i64 },
    /// [`ToApp::After`], back in its order: the host answers it now.
    After {
        def_id: i32,
        call: Value,
        to: ClientId,
        seq: i32,
    },
    /// An answer for a client of the door.
    Reply {
        to: ClientId,
        seq: i32,
        answer: Result<Value, (String, String)>,
    },
}

/// One window, as the application holds it.
struct Window {
    member: MemberId,
    /// Which application it is.
    kind: AppKind,
    /// The structure it opened as, which another view over the same contents
    /// joins.
    key: String,
    /// The session a multitrack window opens, once it is bound.
    session: Option<String>,
    snapshot: Option<Box<clausters_apps::multitrack::operations::Snapshot>>,
    client_owned: bool,
    /// Where its Save writes, when the host said.
    save_to: Option<Option<String>>,
    /// The ids its tree is drawn with, once the host bound them.
    ids: Option<Value>,
    /// Every widget it numbered, which a history step draws again.
    widgets: Vec<i32>,
    /// The folder its renders are written to, held while it lives and
    /// removed when it goes, however the process ends.
    _workspace: clausters_core::workspace::Workspace,
    /// The actions last said, so an unchanged set is not said again.
    actions: Option<Value>,
    /// **The title last set on it or published with its tree**, so an
    /// unchanged one is not set again.
    told_title: Option<String>,
    /// Whether it opened over no file.
    empty: bool,
}

/// **The application on its thread**.
struct App {
    app: Application,
    editing: Editing,
    windows: HashMap<i32, Window>,
    /// A door's actions waiting on a render, by the token they started with,
    /// and the window each was asked through.
    asked: HashMap<u64, (i32, Reply)>,
    next_token: u64,
    out: Said,
}

/// **What wakes the host's loop** when the application has said something:
/// the front's, set on the host ([`Host::set_app_wake`](super::Host::set_app_wake)).
/// A loop that sleeps until its next input would otherwise read an answer
/// only when the pointer happens to move.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// **Where the host's [`Wake`] is set**, shared with an application made
/// before it: a host opened over a file opens its editor before its front
/// runs, and the front's wake reaches that editor's thread through here.
pub type WakeSlot = Arc<std::sync::OnceLock<Wake>>;

/// **Where the application's answers go**: the host's channel, and its loop
/// woken to read them.
#[derive(Clone)]
pub(super) struct Said {
    tx: Sender<FromApp>,
    wake: WakeSlot,
}

impl Said {
    pub(super) fn new(tx: Sender<FromApp>, wake: WakeSlot) -> Said {
        Said { tx, wake }
    }

    /// Says `said`, the lists it names (`lists`, the context's since it was
    /// last asked) going first.
    fn send(&self, lists: Vec<(u64, clausters_document::parts::FileList)>, said: FromApp) {
        if !lists.is_empty() {
            let _ = self.tx.send(FromApp::Lists(lists));
        }
        if self.tx.send(said).is_ok()
            && let Some(wake) = self.wake.get()
        {
            wake();
        }
    }
}

/// **Starts the application's thread**: its engine made there, and its
/// answers sent on `out`. Answers the channel it is asked on.
pub(super) fn start(
    make: MakeEngine,
    out: Said,
) -> std::io::Result<(Sender<ToApp>, clausters_core::threads::Spawned)> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = clausters_core::threads::spawn("audio-editor", move || run(make, rx, out))?;
    Ok((tx, spawned))
}

fn run(make: MakeEngine, rx: Receiver<ToApp>, out: Said) {
    let engine = match make() {
        Ok(engine) => engine,
        Err(e) => {
            // every open asked of an editor with no engine says why
            while let Ok(ask) = rx.recv() {
                if let ToApp::Open { def_id, reply, .. } = ask {
                    out.send(
                        Vec::new(),
                        FromApp::Opened {
                            def_id,
                            outcome: Err(format!("the audio editor has no engine: {e}")),
                            reply,
                        },
                    );
                }
            }
            return;
        }
    };
    let reading = engine.lock().unwrap_or_else(|e| e.into_inner()).reading();
    out.send(Vec::new(), FromApp::Engine(reading));
    let app = Application::new(engine);
    let mut editing = Editing::new();
    app.hold(&mut editing);
    let mut state = App {
        app,
        editing,
        windows: HashMap::new(),
        asked: HashMap::new(),
        next_token: 1,
        out,
    };
    loop {
        let mut asked = false;
        loop {
            match rx.try_recv() {
                Ok(ask) => {
                    state.handle(ask);
                    asked = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        state.landed();
        if !asked {
            std::thread::park_timeout(if state.app.busy() { IDLE } else { REST });
        }
    }
}

impl App {
    fn member_call(&mut self, member: MemberId, call: Value) -> Value {
        self.app.call(
            &mut self.editing,
            &json!({"verb": "member", "member": member, "call": call}),
        )
    }

    fn handle(&mut self, ask: ToApp) {
        match ask {
            ToApp::Open {
                def_id,
                kind,
                key,
                title,
                path,
                data,
                save_to,
                reply,
            } => {
                // **a session open in a window is not opened twice**: an open
                // of it is that window's, which comes forward instead
                let asked = match (&kind, &data) {
                    (AppKind::Multitrack, Some(AppData::Snapshot(snapshot))) => {
                        snapshot.path.clone().map(|path| (path, false))
                    }
                    (AppKind::Multitrack, None) => path.clone().map(|path| (path, true)),
                    #[cfg(feature = "notation")]
                    (AppKind::Score, Some(AppData::Score { mei, .. })) => {
                        path.clone().map(|path| (path, mei.is_none()))
                    }
                    _ => None,
                };
                // **a sound file open in a window is not opened twice**:
                // that window comes forward with the file in focus
                if let (AppKind::Audio, Some(path)) = (&kind, &path)
                    && let Some((member, file)) = self.editing.audio_file_holding(path)
                    && let Some(window) = self.window_of(member)
                {
                    self.focus_file(window, member, file);
                    return self.out.send(
                        Vec::new(),
                        FromApp::Held {
                            def_id,
                            window,
                            path: path.clone(),
                            adopt: true,
                            reply,
                        },
                    );
                }
                if let Some((path, adopt)) = asked
                    && let Some(window) = self.holding(&path)
                {
                    return self.out.send(
                        Vec::new(),
                        FromApp::Held {
                            def_id,
                            window,
                            path,
                            adopt,
                            reply,
                        },
                    );
                }
                let outcome = match kind {
                    AppKind::Audio => self.open(def_id, &key, &title, path.as_deref()),
                    AppKind::Multitrack => {
                        self.open_multitrack(def_id, &key, &title, path, data, save_to)
                    }
                    AppKind::Roll { source, parent } => {
                        self.open_roll(def_id, &title, source, parent)
                    }
                    #[cfg(feature = "notation")]
                    AppKind::Score => self.open_score(def_id, &key, &title, path, data, save_to),
                    AppKind::Notes => self.open_notes(def_id, &key, &title, data),
                };
                if let Some(window) = self.windows.get_mut(&def_id) {
                    window.client_owned = matches!(reply, Some(Reply::Client(..)));
                }
                self.out.send(
                    self.editing.said(),
                    FromApp::Opened {
                        def_id,
                        outcome,
                        reply,
                    },
                );
            }
            ToApp::Bind {
                def_id,
                ids,
                widgets,
                title,
            } => self.bind(def_id, ids, widgets, &title),
            ToApp::Event { def_id, addr, args } => self.event(def_id, &addr, args),
            ToApp::Call {
                def_id,
                call,
                reply,
            } => self.call(def_id, &call, reply),
            ToApp::After {
                def_id,
                call,
                to,
                seq,
            } => self.out.send(
                self.editing.said(),
                FromApp::After {
                    def_id,
                    call,
                    to,
                    seq,
                },
            ),
            ToApp::Close { def_id } => {
                if let Some(member) = self.windows.get(&def_id).map(|w| w.member) {
                    let _ = self.app.act(
                        &mut self.editing,
                        member,
                        json!({"verb": "closed"}),
                        0,
                        None,
                    );
                }
                self.forget(def_id);
            }
            ToApp::Closed { def_id } => {
                let Some(member) = self.windows.get(&def_id).map(|w| w.member) else {
                    return;
                };
                let mut ran = self.app.act(
                    &mut self.editing,
                    member,
                    json!({"verb": "closed"}),
                    0,
                    None,
                );
                // **The window is gone whatever the member said**: the host
                // lets it go, and its opener is told, on this effect
                let said = ran
                    .effects
                    .iter()
                    .any(|e| matches!(e["kind"].as_str(), Some("closedWindow" | "closeWindow")));
                if !said {
                    ran.effects.push(json!({"kind": "closedWindow"}));
                }
                self.said(def_id, ran, false);
            }
            ToApp::Forget { def_id } => self.forget(def_id),
            ToApp::Adopt { def_id } => {
                let Some(window) = self.windows.get_mut(&def_id) else {
                    return;
                };
                window.client_owned = true;
                let member = window.member;
                self.member_call(member, json!({"verb": "sync", "asks": false}));
                self.say_actions(def_id);
            }
        }
    }

    /// **The window that holds the session at `path`**: one open over it,
    /// or one still opening over it, whose session is opened once it is
    /// bound.
    fn holding(&self, path: &str) -> Option<i32> {
        self.windows
            .iter()
            .find(|(_, window)| {
                window
                    .session
                    .as_deref()
                    .is_some_and(|opening| clausters_apps::editing::same_file(opening, path))
            })
            .map(|(def_id, _)| *def_id)
            .or_else(|| self.editing.window_holding_file(path))
    }

    /// The window audio member `member` is drawn in.
    fn window_of(&self, member: MemberId) -> Option<i32> {
        self.windows
            .iter()
            .find(|(_, window)| window.member == member)
            .map(|(def_id, _)| *def_id)
    }

    /// **File `file` of audio window `def_id` focused**, as a click on it in
    /// its list focuses it: an open of a file the window holds.
    fn focus_file(&mut self, def_id: i32, member: MemberId, file: i64) {
        let action = json!({"verb": "verb", "window": true,
            "call": {"verb": "focus_file", "file": file}});
        let ran = self.app.act(&mut self.editing, member, action, 0, None);
        self.said(def_id, ran, false);
    }

    /// **The editor opened**, in its working folder, over `path` or none.
    fn open(
        &mut self,
        def_id: i32,
        key: &str,
        title: &str,
        path: Option<&str>,
    ) -> Result<Vec<String>, String> {
        // On disk under the user's cache, never the temporary directory: a
        // reverse of a day writes tens of gigabytes, and the temporary
        // directory is memory on many machines.
        let workspace = clausters_core::workspace::Workspace::create("audio")
            .map_err(|e| format!("the working folder: {e}"))?;
        let mut request = json!({"verb": "openAudio", "key": key,
            "workspace": workspace.path().to_string_lossy(), "title": title});
        match path {
            Some(path) => {
                let info = clausters_app_engine::probe_audio(path)?;
                request["path"] = json!(path);
                request["frames"] = json!(info.frames);
                request["channels"] = json!(info.channels);
                request["rate"] = json!(info.sample_rate);
                request["codec"] = json!(info.codec);
                request["sample_format"] = json!(info.sample_format);
            }
            None => request["empty"] = json!(true),
        }
        let opened = self.app.call(&mut self.editing, &request);
        let member = opened["member"]
            .as_u64()
            .and_then(|m| MemberId::try_from(m).ok())
            .ok_or_else(|| opened["error"].as_str().unwrap_or("not opened").to_string())?;
        let dialogs = self.member_call(member, json!({"verb": "dialogs"}))["dialogs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n.as_str().map(str::to_string))
            .collect();
        self.windows.insert(
            def_id,
            Window {
                member,
                kind: AppKind::Audio,
                key: key.to_string(),
                session: None,
                snapshot: None,
                client_owned: false,
                save_to: None,
                ids: None,
                widgets: Vec::new(),
                _workspace: workspace,
                actions: None,
                told_title: None,
                empty: path.is_none(),
            },
        );
        Ok(dialogs)
    }

    /// **The multitrack opened**, in its working folder, over no session
    /// yet: the session at `path` is opened once the window is bound.
    fn open_multitrack(
        &mut self,
        def_id: i32,
        key: &str,
        title: &str,
        path: Option<String>,
        data: Option<AppData>,
        save_to: Option<Option<String>>,
    ) -> Result<Vec<String>, String> {
        let workspace = clausters_core::workspace::Workspace::create("multitrack")
            .map_err(|e| format!("the working folder: {e}"))?;
        let rate = f64::from(
            self.app
                .engine()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .sample_rate(),
        );
        let (multitrack, snapshot) = match data {
            Some(AppData::Contents(contents)) => (Some(contents), None),
            Some(AppData::Snapshot(snapshot)) => (None, Some(snapshot)),
            #[cfg(feature = "notation")]
            Some(AppData::Score { .. }) => (None, None),
            Some(AppData::Notes { .. }) => (None, None),
            None => (None, None),
        };
        let empty = multitrack.is_none();
        let opened = self.app.call(
            &mut self.editing,
            &json!({"verb": "openMultitrack", "key": key,
                "multitrack": multitrack, "empty": empty,
                "rate": rate, "title": title, "transport": true,
                "w": MULTITRACK_SIZE.0, "h": MULTITRACK_SIZE.1}),
        );
        let member = opened["member"]
            .as_u64()
            .and_then(|m| MemberId::try_from(m).ok())
            .ok_or_else(|| opened["error"].as_str().unwrap_or("not opened").to_string())?;
        self.windows.insert(
            def_id,
            Window {
                member,
                kind: AppKind::Multitrack,
                key: key.to_string(),
                session: path,
                snapshot,
                client_owned: false,
                save_to,
                ids: None,
                widgets: Vec::new(),
                _workspace: workspace,
                actions: None,
                told_title: None,
                empty: false,
            },
        );
        // the Save as form's widgets, numbered with the window's
        let dialogs = self.member_call(member, json!({"verb": "dialogs"}))["dialogs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n.as_str().map(str::to_string))
            .collect();
        Ok(dialogs)
    }

    /// **The score editor opened**, in its working folder, over the document
    /// a client handed over, the file at `path`, or a blank page: read here,
    /// by the engraver this application holds. Answers the names of the
    /// widgets the window numbers, each under the group it belongs to; a
    /// document that does not read opens nothing.
    #[cfg(feature = "notation")]
    fn open_score(
        &mut self,
        def_id: i32,
        key: &str,
        title: &str,
        path: Option<String>,
        data: Option<AppData>,
        save_to: Option<Option<String>>,
    ) -> Result<Vec<String>, String> {
        use clausters_core::notation::{sheet_to_mei, voice_to_sheet};
        let (mei, over, mut request) = match data {
            Some(AppData::Score { mei, over, request }) => (mei, over, request),
            _ => (None, None, json!({})),
        };
        // **nothing named is no document**, which is not a new score: the
        // window offers to make or open one
        if mei.is_none() && path.is_none() && over.is_none() && !request["sequence"].is_object() {
            request["empty"] = json!(true);
        }
        let contents = match (mei, &path) {
            (Some(mei), _) => mei,
            (None, Some(path)) => {
                let bytes =
                    clausters_core::files::read(path).map_err(|e| format!("{path}: {e}"))?;
                String::from_utf8(bytes).map_err(|e| format!("{path}: {e}"))?
            }
            _ => sheet_to_mei(&voice_to_sheet(&[], "4/4", "G2", "C")).map_err(|e| e.to_string())?,
        };
        let score = engraved(&contents)?;
        // **a page over a sequence**: the sequence the script handed over, or
        // that of the window it stands beside, whose order it joins
        let sequence = self.sequence_of(&mut request, over)?;
        let (key, place) = self.placed(key, over);
        let opened = ScoreOpen {
            kind: AppKind::Score,
            key,
            place,
            title,
            session: path,
            save_to,
        };
        self.score_window(def_id, opened, score, sequence, request)
    }

    /// **The page over the box of notes `source` of multitrack window
    /// `parent`**: a score member over the very sequence the multitrack's
    /// boxes read, in the same context, so an edit in either is one history,
    /// as a roll over the box is.
    #[cfg(feature = "notation")]
    /// **A score window opened**: the member over `score` -- over `sequence`
    /// when there is one -- where `opened` places it, the window held, and
    /// the tools, dialogs and palettes it numbers, each under its group.
    #[cfg(feature = "notation")]
    fn score_window(
        &mut self,
        def_id: i32,
        opened: ScoreOpen<'_>,
        score: clausters_apps::score::Shared,
        sequence: Option<clausters_apps::notes::Shared>,
        mut request: Value,
    ) -> Result<Vec<String>, String> {
        let workspace = clausters_core::workspace::Workspace::create("score")
            .map_err(|e| format!("the working folder: {e}"))?;
        request["title"] = json!(opened.title);
        for (name, size) in [("w", SCORE_SIZE.0), ("h", SCORE_SIZE.1)] {
            if !request[name].is_number() {
                request[name] = json!(size);
            }
        }
        let text = request.to_string();
        let ScoreOpen {
            kind,
            key,
            place,
            session,
            save_to,
            ..
        } = opened;
        let answer = match sequence {
            Some(sequence) => self
                .editing
                .open_score_over_at(place, &key, score, sequence, &text),
            None => self.editing.open_score_at(place, &key, score, &text),
        };
        let answer: Value = serde_json::from_str(&answer).map_err(|e| e.to_string())?;
        let member = answer["member"]
            .as_u64()
            .and_then(|m| MemberId::try_from(m).ok())
            .ok_or_else(|| answer["error"].as_str().unwrap_or("not opened").to_string())?;
        self.windows.insert(
            def_id,
            Window {
                member,
                kind,
                key,
                session,
                snapshot: None,
                client_owned: false,
                save_to,
                ids: None,
                widgets: Vec::new(),
                _workspace: workspace,
                actions: None,
                told_title: None,
                empty: false,
            },
        );
        let mut names = Vec::new();
        for (verb, group) in [
            ("tools", "tool"),
            ("dialogs", "dialog"),
            ("palettes", "palette"),
        ] {
            let listed = self.member_call(member, json!({"verb": verb}));
            names.extend(
                listed[verb]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|name| format!("{group}:{name}")),
            );
        }
        Ok(names)
    }

    /// **The sequence a view opens over**: the one the window `over` shows,
    /// or the one handed over (`sequence`, taken out of `request`); `None`
    /// when neither is said.
    fn sequence_of(
        &self,
        request: &mut Value,
        over: Option<i32>,
    ) -> Result<Option<clausters_apps::notes::Shared>, String> {
        use clausters_apps::editing::Member;
        if let Some(over) = over {
            let member = self
                .windows
                .get(&over)
                .map(|w| w.member)
                .ok_or("the window it opens beside is gone")?;
            return match self.editing.member(member) {
                Some(Member::Notes(editor)) => Ok(Some(editor.sequence().clone())),
                #[cfg(feature = "notation")]
                Some(Member::Score(editor)) if editor.is_over() => Ok(editor.sequence().cloned()),
                _ => Err("the window it opens beside shows no sequence".into()),
            };
        }
        let Some(written) = request
            .as_object_mut()
            .and_then(|map| map.remove("sequence"))
            .filter(|written| !written.is_null())
        else {
            return Ok(None);
        };
        let sequence: clausters_document::EventSequence = serde_json::from_value(written)
            .map_err(|e| format!("not a sequence of events: {e}"))?;
        Ok(Some(Arc::new(Mutex::new(sequence))))
    }

    /// **Where a window goes among the undo orders**: beside the window `over`,
    /// under its key and in its order, or apart under `key`, its edits no step
    /// of any other window's.
    fn placed(&self, key: &str, over: Option<i32>) -> (String, clausters_apps::editing::Place) {
        use clausters_apps::editing::Place;
        match over.and_then(|over| self.windows.get(&over)) {
            Some(window) => (window.key.clone(), Place::Beside(window.member)),
            None => (key.to_string(), Place::Apart),
        }
    }

    /// **A roll a client opened**, in its working folder, over the sequence
    /// it handed over or the one a window of it already shows, whose order
    /// it joins.
    fn open_notes(
        &mut self,
        def_id: i32,
        key: &str,
        title: &str,
        data: Option<AppData>,
    ) -> Result<Vec<String>, String> {
        let (over, mut request) = match data {
            Some(AppData::Notes { over, request }) => (over, request),
            _ => (None, json!({})),
        };
        let sequence = self
            .sequence_of(&mut request, over)?
            .ok_or("a roll opens over a sequence")?;
        let (key, place) = self.placed(key, over);
        let rate = f64::from(
            self.app
                .engine()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .sample_rate(),
        );
        request["rate"] = json!(rate);
        request["title"] = json!(title);
        for (name, size) in [("w", ROLL_SIZE.0), ("h", ROLL_SIZE.1)] {
            if !request[name].is_number() {
                request[name] = json!(size);
            }
        }
        let opened: Value = serde_json::from_str(&self.editing.open_notes_at(
            place,
            &key,
            sequence,
            &request.to_string(),
        ))
        .map_err(|e| e.to_string())?;
        let member = opened["member"]
            .as_u64()
            .and_then(|m| MemberId::try_from(m).ok())
            .ok_or("the roll did not open")?;
        let workspace = clausters_core::workspace::Workspace::create("roll")
            .map_err(|e| format!("the working folder: {e}"))?;
        self.windows.insert(
            def_id,
            Window {
                member,
                kind: AppKind::Notes,
                key,
                session: None,
                snapshot: None,
                client_owned: false,
                save_to: None,
                ids: None,
                widgets: Vec::new(),
                _workspace: workspace,
                actions: None,
                told_title: None,
                empty: false,
            },
        );
        Ok(self.roll_dialogs(member))
    }

    /// **The roll over the box of notes `source` of multitrack window
    /// `parent`**: a notes member over the very sequence the multitrack's
    /// boxes read, in the same context, so an edit in either is one history.
    fn open_roll(
        &mut self,
        def_id: i32,
        title: &str,
        source: u64,
        parent: i32,
    ) -> Result<Vec<String>, String> {
        use clausters_apps::editing::Member;
        let parent = self
            .windows
            .get(&parent)
            .map(|w| w.member)
            .ok_or("the multitrack's window is gone")?;
        let shared = match self.editing.member(parent) {
            Some(Member::Multitrack(editor)) => editor
                .sequence(clausters_document::SourceId(source))
                .cloned()
                .ok_or("the box reads no sequence")?,
            _ => return Err("the window is not a multitrack's".into()),
        };
        let rate = f64::from(
            self.app
                .engine()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .sample_rate(),
        );
        let request = json!({"rate": rate, "title": title, "w": 1000, "h": 520}).to_string();
        let opened: Value = serde_json::from_str(&self.editing.open_notes_in(
            parent,
            &format!("sequence:{source}"),
            shared,
            &request,
        ))
        .unwrap_or_default();
        let member = opened["member"]
            .as_u64()
            .and_then(|m| MemberId::try_from(m).ok())
            .ok_or("the roll did not open")?;
        let workspace = clausters_core::workspace::Workspace::create("roll")
            .map_err(|e| format!("the working folder: {e}"))?;
        self.windows.insert(
            def_id,
            Window {
                member,
                kind: AppKind::Roll { source, parent: 0 },
                key: format!("sequence:{source}"),
                session: None,
                snapshot: None,
                client_owned: false,
                save_to: None,
                ids: None,
                widgets: Vec::new(),
                _workspace: workspace,
                actions: None,
                told_title: None,
                empty: false,
            },
        );
        Ok(self.roll_dialogs(member))
    }

    /// **The widgets a roll's window names for the host to number**: its
    /// dialog's, each under the dialog group.
    fn roll_dialogs(&mut self, member: MemberId) -> Vec<String> {
        self.member_call(member, json!({"verb": "dialogs"}))["dialogs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|name| format!("dialog:{name}"))
            .collect()
    }

    /// **A multitrack window bound**: the editor told its window, its
    /// transport row and its title, the window drawn, and the session opened
    /// and sounded.
    fn bind_multitrack(
        &mut self,
        def_id: i32,
        member: MemberId,
        ids: &Value,
        session: Option<String>,
        save_to: Option<Option<String>>,
    ) {
        let controls = json!({"row": ids["row"], "rewind": ids["rewind"], "play": ids["play"],
            "stop": ids["stop"], "clock": ids["clock"]});
        let form = json!({"stack": ids["close_stack"], "dialog": ids["close_dialog"],
            "save": ids["close_save"], "discard": ids["close_discard"],
            "cancel": ids["close_cancel"]});
        // **A window that saves its session asks before a close loses
        // work**: one over a file, or told where it writes. An untitled one
        // goes back to the script that handed it over, which keeps it.
        let asks = !self
            .windows
            .get(&def_id)
            .is_some_and(|window| window.client_owned);
        // **It saves unless it was opened read-only**: an untitled one too,
        // whose first Save asks where in the Save as form
        let saves = !matches!(save_to, Some(None));
        self.member_call(
            member,
            json!({"verb": "sync", "window": def_id, "controls": controls,
                "closeForm": form, "asks": asks, "saves": saves,
                "dialogs": ids["dialogs"]}),
        );
        let mut action = match session {
            Some(path) => json!({"verb": "open", "session": path, "window": true}),
            None => json!({"verb": "open", "window": true}),
        };
        if let Some(snapshot) = self
            .windows
            .get_mut(&def_id)
            .and_then(|window| window.snapshot.take())
        {
            action["snapshot"] = json!(snapshot);
        }
        if let Some(save_to) = save_to {
            action["saveTo"] = json!(save_to);
        }
        let ran = self.app.act(&mut self.editing, member, action, 0, None);
        let mut first = Ran {
            member,
            token: 0,
            effects: vec![json!({"kind": "publish"})],
            end: ran.end.clone(),
        };
        first.effects.extend(ran.effects);
        first
            .effects
            .push(json!({"kind": "transport", "transport": 0}));
        self.said(def_id, first, true);
    }

    /// The window's ids bound: the window synced, drawn, and its file opened.
    fn bind(&mut self, def_id: i32, ids: Value, widgets: Vec<i32>, title: &str) {
        let Some((member, empty, kind, session, save_to)) =
            self.windows.get_mut(&def_id).map(|w| {
                w.ids = Some(ids.clone());
                w.widgets = widgets;
                (
                    w.member,
                    w.empty,
                    w.kind,
                    w.session.take(),
                    w.save_to.take(),
                )
            })
        else {
            return;
        };
        match kind {
            AppKind::Multitrack => {
                return self.bind_multitrack(def_id, member, &ids, session, save_to);
            }
            AppKind::Roll { .. } | AppKind::Notes => {
                self.member_call(member, json!({"verb": "sync", "window": def_id}));
                // its own transport, taken now: what its play cursor reads
                let ran = self
                    .app
                    .act(&mut self.editing, member, json!({"verb": "open"}), 0, None);
                let mut first = Ran {
                    member,
                    token: 0,
                    effects: vec![json!({"kind": "publish"})],
                    end: ran.end,
                };
                first.effects.extend(ran.effects);
                return self.said(def_id, first, true);
            }
            #[cfg(feature = "notation")]
            AppKind::Score => {
                let asks = !self
                    .windows
                    .get(&def_id)
                    .is_some_and(|window| window.client_owned);
                // where its Save writes: the host's word when it said, or
                // the file it read
                let file = save_to.unwrap_or(session);
                let sync = json!({"verb": "sync", "window": def_id, "title": title, "path": file,
                    "asks": asks});
                self.member_call(member, sync);
                let ran = self
                    .app
                    .act(&mut self.editing, member, json!({"verb": "open"}), 0, None);
                let mut first = Ran {
                    member,
                    token: 0,
                    effects: vec![json!({"kind": "publish"})],
                    end: ran.end,
                };
                first.effects.extend(ran.effects);
                return self.said(def_id, first, true);
            }
            AppKind::Audio => {}
        }
        // **A window no client holds is the work's only holder**, and asks
        // before unsaved work goes
        let asks = !self
            .windows
            .get(&def_id)
            .is_some_and(|window| window.client_owned);
        self.member_call(
            member,
            json!({"verb": "sync", "window": def_id, "title": title, "asks": asks}),
        );
        self.member_call(member, json!({"verb": "attach_view"}));
        let mut first = Ran {
            member,
            token: 0,
            effects: vec![json!({"kind": "publish"})],
            end: Some(Ok(Value::Null)),
        };
        let ran = self.app.act(
            &mut self.editing,
            member,
            json!({"verb": "open", "empty": empty, "window": true}),
            0,
            None,
        );
        first.effects.extend(ran.effects);
        first.end = ran.end;
        // **The play cursor is the editor's transport's position**: taken as
        // the open starts, which a render of the file's peaks may outlast.
        let transport = self.app.query(&mut self.editing, member, "transport")["transport"].clone();
        first
            .effects
            .push(json!({"kind": "transport", "transport": transport}));
        self.said(def_id, first, true);
    }

    /// A gesture: a paste or a mix takes its clipboard lease first, as a
    /// client's does.
    fn event(&mut self, def_id: i32, addr: &str, args: Vec<Value>) {
        let Some(member) = self.windows.get(&def_id).map(|w| w.member) else {
            return;
        };
        let (args, lease) = self.editing.exchange_event(args, Default::default());
        let action = json!({"verb": "event", "addr": addr, "window": true, "args": args});
        let before = self.editing.version();
        let ran = self.app.act(&mut self.editing, member, action, 0, lease);
        self.said(def_id, ran, true);
        // **A hand moved the context**: every multitrack window's opener
        // is told, a roll's edit being its multitrack's sequence, and every
        // window a client holds
        let version = self.editing.version();
        if version != before {
            let told: Vec<i32> = self
                .windows
                .iter()
                .filter(|(_, w)| w.kind == AppKind::Multitrack || w.client_owned)
                .map(|(id, _)| *id)
                .collect();
            for def_id in told {
                self.out
                    .send(Vec::new(), FromApp::Changed { def_id, version });
            }
        }
    }

    /// **One call of the door**: a member's verb or a query answered at
    /// once, an action carried as far as it goes and answered when it ends.
    fn call(&mut self, def_id: i32, call: &Value, reply: Reply) {
        let Some(member) = self.windows.get(&def_id).map(|w| w.member) else {
            return self.answer(
                reply,
                Err((
                    "runtime".into(),
                    format!("window {def_id} holds no application"),
                )),
            );
        };
        if let Some(request) = call.get("member") {
            let answer = self.member_call(member, request.clone());
            self.say_actions(def_id);
            self.answer(reply, Ok(answer));
        } else if let Some(query) = call["query"].as_str() {
            // what the window reads of the editor: its transport, its
            // actions, the history of the file in focus
            let answer = match query {
                "actions" => self.app.call(
                    &mut self.editing,
                    &json!({"verb": "actions", "member": member}),
                ),
                "history" => self.app.call(
                    &mut self.editing,
                    &json!({"verb": "state", "member": member}),
                ),
                _ => self.app.query(&mut self.editing, member, query),
            };
            self.answer(reply, Ok(answer));
        } else if let Some(Value::Object(action)) = call.get("do") {
            let mut action = Value::Object(action.clone());
            action["window"] = json!(true);
            let token = self.next_token;
            self.next_token += 1;
            let ran = self.app.act(&mut self.editing, member, action, token, None);
            let answered = match ran.end.as_ref().map(ended) {
                Some(answer) => Some((reply, answer)),
                None => {
                    self.asked.insert(token, (def_id, reply));
                    None
                }
            };
            // **The window is told before the client is answered**: a
            // client acts on the answer at once (a verb after a selection),
            // and the host must already hold what the run left, the actions'
            // readings with it, or it answers the next ask from before.
            self.said(def_id, ran, false);
            if let Some((reply, answer)) = answered {
                self.answer(reply, answer);
            }
        } else {
            self.answer(
                reply,
                Err((
                    "value".into(),
                    "a call is a member's verb, a query or an action".into(),
                )),
            );
        }
    }

    /// Every run whose render landed carried on, and said.
    fn landed(&mut self) {
        for ran in self.app.landed(&mut self.editing) {
            let Some(def_id) = self
                .windows
                .iter()
                .find(|(_, w)| w.member == ran.member)
                .map(|(id, _)| *id)
            else {
                continue;
            };
            // **A client's action answered as it ends**, to the client
            let asked = match ran.end {
                Some(_) => self.asked.remove(&ran.token).map(|(_, reply)| reply),
                None => None,
            };
            let say = asked.is_none();
            let answered = match (asked, &ran.end) {
                (Some(reply), Some(end)) => Some((reply, ended(end))),
                _ => None,
            };
            // the window first: a client answered acts on what it drew
            self.said(def_id, ran, say);
            if let Some((reply, answer)) = answered {
                self.answer(reply, answer);
            }
        }
    }

    /// **A run said to the host**, with the drawing its effects need: a
    /// `publish` carrying the window's tree and a `resync` every widget's
    /// props, and the actions' readings when they moved.
    fn said(&mut self, def_id: i32, ran: Ran, say: bool) {
        let mut effects = Vec::with_capacity(ran.effects.len());
        let mut notes = false;
        let mut focus = Vec::new();
        for mut effect in ran.effects {
            match effect["kind"].as_str().unwrap_or_default() {
                // **a sound file another window holds**: that window comes
                // forward with the file in focus
                "front" => {
                    if let (Some(member), Some(file)) = (
                        effect["member"]
                            .as_u64()
                            .and_then(|m| MemberId::try_from(m).ok()),
                        effect["file"].as_i64(),
                    ) && let Some(window) = self.window_of(member)
                    {
                        effect["window"] = json!(window);
                        focus.push((window, member, file));
                    }
                }
                "publish" => effect["def"] = self.window_def(def_id),
                // **the session a client handed over is let go**: the host
                // is the work's only holder now, so the window asks before
                // a close loses it; a session no client held is nobody's
                // to hand back, and the host still hears of it, for the
                // rolls opened from it to go
                "released" => {
                    let Some(window) = self.windows.get_mut(&def_id) else {
                        continue;
                    };
                    // what it let go of is nobody's to bring forward
                    #[cfg(feature = "notation")]
                    if window.kind == AppKind::Score {
                        window.session = None;
                    }
                    let handed = std::mem::take(&mut window.client_owned);
                    effect["handed"] = json!(handed);
                    if handed {
                        let member = window.member;
                        self.member_call(member, json!({"verb": "sync", "asks": true}));
                    }
                }
                "resync" => effect["props"] = self.props(def_id),
                // a box of notes asked for its roll: the host makes its
                // window
                "open" => {
                    if let Some(source) = effect["source"].as_u64() {
                        self.out.send(
                            Vec::new(),
                            FromApp::Roll {
                                parent: def_id,
                                source,
                            },
                        );
                    }
                    continue;
                }
                // the writer of a MIDI file is this build's to link
                #[cfg(feature = "notation")]
                "export" => {
                    let path = effect["path"].as_str().unwrap_or_default().to_string();
                    let clip = effect["format"] == "clip";
                    let written = serde_json::from_value(effect["sequence"].take())
                        .map_err(|e| e.to_string())
                        .and_then(|sequence| {
                            super::score_editor::midi_file(&sequence, clip)
                                .ok_or_else(|| "this host has no MIDI file writer".to_string())
                        })
                        .and_then(|data| {
                            clausters_core::files::write(&path, data).map_err(|e| e.to_string())
                        });
                    match written {
                        Ok(()) => continue,
                        Err(why) => {
                            effect = json!({"kind": "say", "reason": format!("{path}: {why}")});
                        }
                    }
                }
                // a roll's sequence changed: the multitrack plays it again
                "notes" => {
                    notes = true;
                    continue;
                }
                _ => {}
            }
            effects.push(effect);
        }
        if notes {
            let multitracks: Vec<MemberId> = self
                .windows
                .values()
                .filter(|w| w.kind == AppKind::Multitrack)
                .map(|w| w.member)
                .collect();
            for member in multitracks {
                let _ = self
                    .app
                    .act(&mut self.editing, member, json!({"verb": "sound"}), 0, None);
            }
        }
        if let Some(title) = self.title(def_id) {
            effects.push(json!({"kind": "set", "widget": def_id, "props": {"title": title}}));
        }
        let actions = self.actions(def_id);
        self.out.send(
            self.editing.said(),
            FromApp::Ran {
                def_id,
                effects,
                actions,
                end: ran.end,
                say,
            },
        );
        for (window, member, file) in focus {
            self.focus_file(window, member, file);
        }
    }

    /// The window's tree, drawn with the ids it was numbered with.
    fn window_def(&mut self, def_id: i32) -> Value {
        let Some((member, Some(mut ids))) =
            self.windows.get(&def_id).map(|w| (w.member, w.ids.clone()))
        else {
            return Value::Null;
        };
        ids["verb"] = json!("window");
        let mut tree = self.member_call(member, ids);
        // **the title as the work stands**, its unsaved mark with it: a tree
        // drawn again keeps what the window says of its work
        if let (Some(title), Some(window)) = (
            self.editing.window_title(member),
            self.windows.get_mut(&def_id),
        ) && tree.is_object()
        {
            tree["title"] = json!(title);
            window.told_title = Some(title);
        }
        tree
    }

    /// Every widget's props, as a history step leaves them.
    fn props(&mut self, def_id: i32) -> Value {
        let Some((member, widgets)) = self
            .windows
            .get(&def_id)
            .map(|w| (w.member, w.widgets.clone()))
        else {
            return json!([]);
        };
        let props: Vec<Value> = widgets
            .into_iter()
            .map(|widget| {
                json!([
                    widget,
                    self.member_call(member, json!({"verb": "props", "widget": widget}))
                ])
            })
            .collect();
        Value::Array(props)
    }

    /// The window's actions, when they moved since they were last said.
    fn actions(&mut self, def_id: i32) -> Option<Value> {
        let member = self.windows.get(&def_id)?.member;
        let actions = self.app.call(
            &mut self.editing,
            &json!({"verb": "actions", "member": member}),
        );
        let window = self.windows.get_mut(&def_id)?;
        if window.actions.as_ref() == Some(&actions) || !actions.is_object() {
            return None;
        }
        window.actions = Some(actions.clone());
        Some(actions)
    }

    /// **The title window `def_id` shows, when it moved**: its own, marked
    /// while it holds unsaved work ([`Editing::window_title`]), and nothing
    /// when it reads as it was last set.
    fn title(&mut self, def_id: i32) -> Option<String> {
        let member = self.windows.get(&def_id)?.member;
        let title = self.editing.window_title(member)?;
        let window = self.windows.get_mut(&def_id)?;
        if window.told_title.as_ref() == Some(&title) {
            return None;
        }
        window.told_title = Some(title.clone());
        Some(title)
    }

    /// The actions said on their own, after a call that may have moved them.
    fn say_actions(&mut self, def_id: i32) {
        if let Some(actions) = self.actions(def_id) {
            self.out.send(
                self.editing.said(),
                FromApp::Ran {
                    def_id,
                    effects: Vec::new(),
                    actions: Some(actions),
                    end: None,
                    say: false,
                },
            );
        }
    }

    fn answer(&self, reply: Reply, answer: Result<Value, (String, String)>) {
        match reply {
            #[cfg(test)]
            Reply::Local(tx) => {
                let _ = tx.send(answer);
            }
            Reply::Client(to, seq) => {
                self.out
                    .send(self.editing.said(), FromApp::Reply { to, seq, answer });
            }
        }
    }

    /// The window is the application's no longer: its renders are called
    /// off, what a client still waited on ends with it, and its working
    /// folder goes with it, dropped.
    fn forget(&mut self, def_id: i32) {
        if let Some(window) = self.windows.remove(&def_id) {
            self.app.forget(&mut self.editing, window.member);
            // **the last view a client opened over a structure**: nothing
            // walks its order again, and it was handed back with the close
            let last = !self.windows.values().any(|w| w.key == window.key);
            let hosted = !matches!(
                window.kind,
                AppKind::Audio | AppKind::Multitrack | AppKind::Roll { .. }
            );
            if last && hosted {
                self.editing.close_order_of(window.member);
            }
            // only this window's: another's goes on waiting for its render
            let gone: Vec<u64> = self
                .asked
                .iter()
                .filter(|(_, (window, _))| *window == def_id)
                .map(|(token, _)| *token)
                .collect();
            let gone: Vec<Reply> = gone
                .into_iter()
                .filter_map(|token| self.asked.remove(&token))
                .map(|(_, reply)| reply)
                .collect();
            for reply in gone {
                self.answer(
                    reply,
                    Err(("runtime".to_string(), "the window closed".to_string())),
                );
            }
            drop(window);
        }
    }
}

/// An action's end as the door answers it.
fn ended(end: &Result<Value, Raised>) -> Result<Value, (String, String)> {
    end.clone()
        .map_err(|raised| (raised.kind.clone(), raised.message.clone()))
}
