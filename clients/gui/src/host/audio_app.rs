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
}

/// The widget roles a multitrack window is numbered with: the multitrack,
/// its ruler and its transport row.
pub(super) const MULTITRACK_ROLES: &[&str] =
    &["widget", "ruler", "row", "rewind", "play", "stop", "clock"];

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
    /// The window asked to close and goes: what sounded it stops, then it is
    /// forgotten.
    Close { def_id: i32 },
    /// The window is gone: its renders are called off and its working
    /// folder removed.
    Forget { def_id: i32 },
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
    /// (`source`, the sequence it reads): the host opens a window for it.
    Roll { parent: i32, source: u64 },
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
    /// The session a multitrack window opens, once it is bound.
    session: Option<String>,
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
                save_to,
                reply,
            } => {
                let outcome = match kind {
                    AppKind::Audio => self.open(def_id, &key, &title, path.as_deref()),
                    AppKind::Multitrack => {
                        self.open_multitrack(def_id, &key, &title, path, save_to)
                    }
                    AppKind::Roll { source, parent } => {
                        self.open_roll(def_id, &title, source, parent)
                    }
                };
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
            ToApp::Forget { def_id } => self.forget(def_id),
        }
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
                session: None,
                save_to: None,
                ids: None,
                widgets: Vec::new(),
                _workspace: workspace,
                actions: None,
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
        let opened = self.app.call(
            &mut self.editing,
            &json!({"verb": "openMultitrack", "key": key, "multitrack": {},
                "rate": rate, "title": title}),
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
                session: path,
                save_to,
                ids: None,
                widgets: Vec::new(),
                _workspace: workspace,
                actions: None,
                empty: false,
            },
        );
        Ok(Vec::new())
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
        let opened: Value = serde_json::from_str(&self.editing.open_notes(
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
                session: None,
                save_to: None,
                ids: None,
                widgets: Vec::new(),
                _workspace: workspace,
                actions: None,
                empty: false,
            },
        );
        Ok(Vec::new())
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
        self.member_call(
            member,
            json!({"verb": "sync", "window": def_id, "controls": controls}),
        );
        let mut action = match session {
            Some(path) => json!({"verb": "open", "session": path, "window": true}),
            None => json!({"verb": "open", "window": true}),
        };
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
            AppKind::Roll { .. } => {
                self.member_call(member, json!({"verb": "sync", "window": def_id}));
                let first = Ran {
                    member,
                    token: 0,
                    effects: vec![json!({"kind": "publish"})],
                    end: Some(Ok(Value::Null)),
                };
                return self.said(def_id, first, true);
            }
            AppKind::Audio => {}
        }
        self.member_call(
            member,
            json!({"verb": "sync", "window": def_id, "title": title}),
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
        let ran = self.app.act(&mut self.editing, member, action, 0, lease);
        self.said(def_id, ran, true);
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
        for mut effect in ran.effects {
            match effect["kind"].as_str().unwrap_or_default() {
                "publish" => effect["def"] = self.window_def(def_id),
                "resync" => effect["props"] = self.props(def_id),
                // a box of notes asked for its roll: the host makes its window
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
    }

    /// The window's tree, drawn with the ids it was numbered with.
    fn window_def(&mut self, def_id: i32) -> Value {
        let Some((member, Some(mut ids))) =
            self.windows.get(&def_id).map(|w| (w.member, w.ids.clone()))
        else {
            return Value::Null;
        };
        ids["verb"] = json!("window");
        self.member_call(member, ids)
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
