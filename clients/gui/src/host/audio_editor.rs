//! **The audio editor this host owns**: the applications crate's audio
//! editor opened here, over a file, on an engine of its own, and every
//! gesture on its window answered by it.
//!
//! The application is one program (`crates/clausters-apps/PLAN.md`, "The
//! second turn"): the editor's coordinator, the take it reads and the engine
//! it plays on are in this process, on a thread of their own
//! (`super::audio_app`), so the window never waits on a disk or a render.
//! What is this module's is the window: the widget ids it is drawn with,
//! what a run left for it (props set, the window drawn again, an answer, the
//! clipboard, its progress), applied on the host's tick, and the meters and
//! the play cursor, which read the engine the application holds rather than
//! a server's segment.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use clausters_app_engine::AppEngine;
use clausters_editing::conversation::Answer;
use serde_json::{Value, json};

use super::audio_app::{AppKind, FromApp, MakeEngine, Reply, Said, ToApp, Wake, WakeSlot};
use super::*;

/// The role an audio editor's widgets play, in the names the host's own
/// widget ids are asked for by.
const AUDIO: &str = "audio";

/// The widgets of an audio editor's window, by the role the editor's
/// `window` verb names them with.
const ROLES: &[&str] = &["widget", "meter", "pane", "envelope", "lane", "filesWidget"];

/// **The audio editor's application in this host**, as the host holds it:
/// the line to its thread, and its windows.
pub(crate) struct AudioHost {
    /// What makes the engine, until the thread starts and takes it.
    make: Option<MakeEngine>,
    /// What the host asks of the application, once its thread runs.
    to_app: Option<Sender<ToApp>>,
    /// What the application says back, read on the host's tick.
    from_app: Receiver<FromApp>,
    /// Its other end, for the thread.
    out: Said,
    thread: Option<clausters_core::threads::Spawned>,
    windows: HashMap<i32, AudioWindow>,
    /// What the windows' meters and play cursors read: the engine's own,
    /// once it is open.
    source: Option<Arc<dyn BusSource>>,
    next_structure: i64,
}

impl std::fmt::Debug for AudioHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioHost")
            .field("windows", &self.windows.len())
            .finish_non_exhaustive()
    }
}

impl Drop for AudioHost {
    fn drop(&mut self) {
        // the thread ends as its line does, and natively it is waited for,
        // so the working folders it holds are gone when the host is
        self.to_app = None;
        if let Some(thread) = self.thread.take() {
            thread.unpark();
            thread.join();
        }
    }
}

/// One audio editor's window, as the host holds it.
#[derive(Debug)]
struct AudioWindow {
    /// Which application it is.
    kind: AppKind,
    /// The structure its own widget ids are named under.
    structure: i64,
    title: String,
    /// The widgets that draw the take, which a band and the loop switch are
    /// set on, once the window is bound.
    takes: Vec<i32>,
    /// **The widget that keeps what the hand holds**, a multitrack's own
    /// once it is bound: its boxes held are the window's, never the
    /// application's, so a client's `selected` and `select` are answered
    /// here ([`Host::multitrack_marks`]).
    marks: Option<i32>,
    /// The client that opened it through the door, told when it goes.
    opener: Option<ClientId>,
    /// **What its application left it as**, when it closed: the
    /// multitrack's, told the opener with the close.
    last: Option<Value>,
    /// **It let go of the session its opener handed it**
    /// ([`GUI_APP_RELEASED`]): the opener still hears it close, since it
    /// opened the window, but is handed nothing with the close.
    released: bool,
}

/// **What an engine publishes, as the bus source a window reads**: the
/// control buses its meters read and the transport positions its play
/// cursors are drawn from.
struct EngineSource(clausters_app_engine::EngineReading);

impl BusSource for EngineSource {
    fn control(&self, index: usize) -> f32 {
        self.0.control(index)
    }

    fn sample_rate(&self) -> f64 {
        f64::from(self.0.sample_rate())
    }

    fn sample_clock(&self) -> f64 {
        self.0.current_samples() as f64
    }

    fn transport_position(&self, transport: usize) -> f64 {
        self.0.transport_position(transport) as f64
    }

    fn transport_reading(
        &self,
        transport: usize,
    ) -> Option<clausters_core::transport_pass::TransportReading> {
        self.0.transport_reading(transport)
    }
}

impl AudioHost {
    /// The application, its engine made by `make` when its first window
    /// opens.
    fn new(make: MakeEngine, wake: WakeSlot) -> AudioHost {
        let (out, from_app) = channel();
        let out = Said::new(out, wake);
        AudioHost {
            make: Some(make),
            to_app: None,
            from_app,
            out,
            thread: None,
            windows: HashMap::new(),
            source: None,
            next_structure: 0,
        }
    }

    /// **Asks the application**, its thread started on the first ask.
    fn ask(&mut self, ask: ToApp) -> Result<(), String> {
        if self.to_app.is_none() {
            let make = self.make.take().ok_or("the audio editor's thread ended")?;
            let (tx, thread) = super::audio_app::start(make, self.out.clone())
                .map_err(|e| format!("no thread for the audio editor: {e}"))?;
            self.to_app = Some(tx);
            self.thread = Some(thread);
        }
        self.to_app
            .as_ref()
            .and_then(|tx| tx.send(ask).ok())
            .ok_or_else(|| "the audio editor's thread ended".to_string())?;
        // the thread sleeps between asks: this one wakes it
        if let Some(thread) = &self.thread {
            thread.unpark();
        }
        Ok(())
    }
}

impl Host {
    /// **What wakes this host's loop when an application it holds has said
    /// something**: the front's, set before the first application opens.
    pub fn set_app_wake(&mut self, wake: Wake) {
        // set once, and seen by an application opened before it
        let _ = self.app_wake.set(wake);
    }

    /// **Holds `engine` as the audio editor's**, for a caller that brings its
    /// own (a test driving it block by block); [`Host::open_audio`] opens one
    /// on the machine's output otherwise.
    pub fn hold_audio_engine(&mut self, engine: Arc<Mutex<AppEngine>>) {
        self.audio = Some(AudioHost::new(
            Box::new(move || Ok(engine)),
            self.app_wake.clone(),
        ));
    }

    /// The audio editor's application, made on the first open: its engine
    /// on the machine's output, or in a page on the page's.
    fn audio_host(&mut self) -> &mut AudioHost {
        let wake = self.app_wake.clone();
        self.audio.get_or_insert_with(|| {
            AudioHost::new(
                Box::new(|| {
                    #[cfg(not(target_arch = "wasm32"))]
                    let engine = AppEngine::open();
                    #[cfg(target_arch = "wasm32")]
                    let engine = super::web::apps_engine::open();
                    engine.map(|engine| Arc::new(Mutex::new(engine)))
                }),
                wake,
            )
        })
    }

    /// **Opens the audio editor** in a window of its own, over the sound file
    /// at `path` or over none, and answers the window's id. The engine is
    /// opened on the machine's output the first time, unless one is held.
    /// The window is drawn once the application has opened the file, on a
    /// later tick; a file it cannot open is said then.
    pub fn open_audio(&mut self, path: Option<&str>, title: &str) -> Result<i32, String> {
        self.open_audio_for(path, title, None)
    }

    /// [`Host::open_audio`] answered to `reply` once it opened, or failed: a
    /// client of the door is answered with the window, and told
    /// [`GUI_CLOSED`] when it goes.
    pub(super) fn open_audio_for(
        &mut self,
        path: Option<&str>,
        title: &str,
        reply: Option<Reply>,
    ) -> Result<i32, String> {
        self.open_app_for(AppKind::Audio, path, title, reply)
    }

    /// **Opens the multitrack editor** in a window of its own, over the
    /// session at `path` or over no session, and answers the
    /// window's id: its session is opened, its takes' peaks read or
    /// measured, and its tracks sounded on the application's engine.
    pub fn open_multitrack(&mut self, path: Option<&str>, title: &str) -> Result<i32, String> {
        self.open_app_for(AppKind::Multitrack, path, title, None)
    }

    /// [`Host::open_multitrack`] as a host started over one opens it
    /// (`--session [file]`, `--save-to`): the session at `path` saves to
    /// `save_to` and nowhere without one, since overwriting the file it
    /// opened is a decision; a window opened over none saves its first
    /// session to `save_to`, or asks where.
    pub fn open_multitrack_saving_to(
        &mut self,
        path: Option<&str>,
        save_to: Option<&str>,
        title: &str,
    ) -> Result<i32, String> {
        let save_to = match (path, save_to) {
            (_, Some(to)) => Some(Some(to.to_string())),
            (Some(_), None) => Some(None),
            (None, None) => None,
        };
        self.open_app_saving(AppKind::Multitrack, path, None, title, None, save_to)
    }

    /// **The score editor opened for a client or by this host**, in a window
    /// of its own, over the file at `path`, the MEI `mei`, or a blank page.
    #[cfg(feature = "notation")]
    pub(super) fn open_app_score_for(
        &mut self,
        path: Option<&str>,
        mei: Option<String>,
        over: Option<i32>,
        request: Value,
        title: &str,
        reply: Option<Reply>,
    ) -> Result<i32, String> {
        self.open_app_saving(
            AppKind::Score,
            path,
            Some(super::audio_app::AppData::Score { mei, over, request }),
            title,
            reply,
            None,
        )
    }

    /// **The score editor over the file at `path`, for this host alone**: what
    /// `--score` opens. Its Save writes to `save_to`, and asks where without it.
    #[cfg(feature = "notation")]
    pub fn open_score_saving_to(
        &mut self,
        path: Option<&str>,
        save_to: Option<&str>,
        title: &str,
    ) -> Result<i32, String> {
        self.open_app_saving(
            AppKind::Score,
            path,
            Some(super::audio_app::AppData::Score {
                mei: None,
                over: None,
                request: json!({}),
            }),
            title,
            None,
            Some(save_to.map(str::to_string)),
        )
    }

    /// **A roll opened over a sequence**, handed over in `request` or the
    /// one window `over` shows, answered to `reply` once it opened.
    pub(super) fn open_app_notes_for(
        &mut self,
        over: Option<i32>,
        request: Value,
        title: &str,
        reply: Option<Reply>,
    ) -> Result<i32, String> {
        self.open_app_saving(
            AppKind::Notes,
            None,
            Some(super::audio_app::AppData::Notes { over, request }),
            title,
            reply,
            None,
        )
    }

    /// An application of `kind` opened in a window of its own, answered to
    /// `reply` once it opened, or failed.
    pub(super) fn open_app_for(
        &mut self,
        kind: AppKind,
        path: Option<&str>,
        title: &str,
        reply: Option<Reply>,
    ) -> Result<i32, String> {
        self.open_app_saving(kind, path, None, title, reply, None)
    }

    /// **The multitrack opened over `multitrack`**, the document's JSON, as
    /// an untitled session: no file behind it, its first save naming one.
    pub(super) fn open_multitrack_over(
        &mut self,
        multitrack: Value,
        title: &str,
        reply: Option<Reply>,
    ) -> Result<i32, String> {
        self.open_app_saving(
            AppKind::Multitrack,
            None,
            Some(super::audio_app::AppData::Contents(multitrack)),
            title,
            reply,
            None,
        )
    }

    pub(super) fn open_multitrack_retained(
        &mut self,
        snapshot: clausters_apps::multitrack::operations::Snapshot,
        title: &str,
        reply: Option<Reply>,
    ) -> Result<i32, String> {
        self.open_app_saving(
            AppKind::Multitrack,
            None,
            Some(super::audio_app::AppData::Snapshot(Box::new(snapshot))),
            title,
            reply,
            None,
        )
    }

    fn open_app_saving(
        &mut self,
        kind: AppKind,
        path: Option<&str>,
        data: Option<super::audio_app::AppData>,
        title: &str,
        reply: Option<Reply>,
        save_to: Option<Option<String>>,
    ) -> Result<i32, String> {
        let structure = {
            let audio = self.audio_host();
            audio.next_structure += 1;
            audio.next_structure
        };
        let def_id = self
            .own_widget(structure, AUDIO, "window")
            .ok_or("out of widget ids for the audio editor")?;
        let opener = match &reply {
            Some(Reply::Client(to, _)) => Some(*to),
            _ => None,
        };
        let audio = self.audio_host();
        audio.windows.insert(
            def_id,
            AudioWindow {
                kind,
                structure,
                title: title.to_string(),
                takes: Vec::new(),
                marks: None,
                opener,
                last: None,
                released: false,
            },
        );
        let asked = audio.ask(ToApp::Open {
            save_to,
            def_id,
            kind,
            key: format!("audio:{structure}"),
            title: title.to_string(),
            path: path.map(str::to_string),
            data,
            reply,
        });
        if let Err(e) = asked {
            audio.windows.remove(&def_id);
            return Err(e);
        }
        Ok(def_id)
    }

    /// **The view over box of notes `source` of multitrack window
    /// `parent`** -- its roll, or its page (`page`) where the boxes are drawn
    /// as pages -- in a window of its own that goes when the multitrack's
    /// does; one already open is left as it is.
    fn open_roll_of(&mut self, parent: i32, source: u64, page: bool) {
        #[cfg(feature = "notation")]
        let (kind, title) = if page {
            (AppKind::Page { source, parent }, format!("score {source}"))
        } else {
            (AppKind::Roll { source, parent }, format!("notes {source}"))
        };
        #[cfg(not(feature = "notation"))]
        let (kind, title) = {
            let _ = page;
            (AppKind::Roll { source, parent }, format!("notes {source}"))
        };
        let open = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.windows.values().any(|w| w.kind == kind));
        if open || !self.owns_audio(parent) {
            return;
        }
        match self.open_app_for(kind, None, &title, None) {
            Ok(def_id) => self.depend(def_id, parent),
            Err(why) => diag::warn!("roll: {why}"),
        }
    }

    /// **Asks window `def_id`'s editor `call`** from this process, answered
    /// on the channel this returns: a member's verb, a query or an action, as
    /// the door's [`GUI_APP_CALL`] takes them.
    #[cfg(test)]
    pub(crate) fn ask_audio(
        &mut self,
        def_id: i32,
        call: Value,
    ) -> Receiver<Result<Value, (String, String)>> {
        let (tx, rx) = channel();
        let _ = self.audio_host().ask(ToApp::Call {
            def_id,
            call,
            reply: Reply::Local(tx),
        });
        rx
    }

    /// **Whether this host owns an audio editor's window**, open or still
    /// opening: a host started over one (`--audio`) is that application, and
    /// closing its last window ends it. A page is never started over one.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn owns_an_application(&self) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| !audio.windows.is_empty())
    }

    /// **The person closed window `def_id`**, which asked its owner nothing
    /// first (no `ask_close`: a roll, a multitrack with nowhere to save):
    /// when an application holds it, the application is told, lets it go,
    /// and its opener hears what it left. A front calls this for every
    /// window it closes on the person's word.
    pub fn closed_by_user(&mut self, def_id: i32) {
        if self.owns_audio(def_id) {
            let _ = self.audio_host().ask(ToApp::Closed { def_id });
        }
    }

    /// Whether window `def_id` is an audio editor this host owns.
    pub(crate) fn owns_audio(&self, def_id: i32) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| audio.windows.contains_key(&def_id))
    }

    /// **A gesture on an audio editor's window**, handed to its application.
    pub(super) fn answer_audio(&mut self, def_id: i32, message: &OscMessage) -> bool {
        let args: Vec<Value> = message.args.iter().map(document::atom).collect();
        let addr = message.addr.clone();
        self.audio_host()
            .ask(ToApp::Event { def_id, addr, args })
            .is_ok()
    }

    /// **One call of a client to window `def_id`'s editor**
    /// ([`GUI_APP_CALL`]), answered when the application has done it.
    pub(super) fn call_app_audio(
        &mut self,
        seq: i32,
        def_id: i32,
        call: &Value,
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        if let Some(answer) = self.multitrack_marks(def_id, call, effects) {
            return effects.push(HostEffect::Reply(super::apps::app_reply(seq, Ok(answer))));
        }
        let asked = self.audio_host().ask(ToApp::Call {
            def_id,
            call: call.clone(),
            reply: Reply::Client(from, seq),
        });
        if let Err(e) = asked {
            effects.push(HostEffect::Reply(super::apps::app_reply(
                seq,
                Err(("runtime".into(), e)),
            )));
        }
    }

    /// **The audio editor's turn on this host's tick**: everything its
    /// application said since the last one, carried out.
    pub fn tick_audio(&mut self) {
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        let said: Vec<FromApp> = audio.from_app.try_iter().collect();
        for one in said {
            match one {
                FromApp::Lists(lists) => super::disk::receive(lists),
                FromApp::Engine(reading) => {
                    if let Some(audio) = self.audio.as_mut() {
                        audio.source = Some(Arc::new(EngineSource(reading)));
                    }
                }
                FromApp::Opened {
                    def_id,
                    outcome,
                    reply,
                } => self.opened_audio(def_id, outcome, reply),
                FromApp::Ran {
                    def_id,
                    effects,
                    actions,
                    end,
                    say,
                } => {
                    if !self.owns_audio(def_id) {
                        continue;
                    }
                    // **What failed is said in the window**, a render's above
                    // all, which ends where nobody is looking at a log
                    if say && let Some(Err(raised)) = &end {
                        diag::warn!("audio: {}", raised.message);
                        self.say(def_id, status::Line::of_reason(None, &raised.message));
                    }
                    self.perform_audio(def_id, effects, actions);
                }
                FromApp::Reply { to, seq, answer } => {
                    let msg = super::apps::app_reply(seq, answer);
                    self.pending_effects.push(HostEffect::Send { to, msg });
                }
                FromApp::Held {
                    def_id,
                    window,
                    path,
                    adopt,
                    reply,
                } => self.held_audio(def_id, window, &path, adopt, reply),
                FromApp::Roll {
                    parent,
                    source,
                    page,
                } => self.open_roll_of(parent, source, page),
                FromApp::Changed { def_id, version } => self.changed_audio(def_id, version),
            }
        }
    }

    /// **Window `def_id`'s editor opened**, or did not: its widgets
    /// numbered and handed over, its meters and its play cursor set to read
    /// the engine, and whoever opened it answered.
    fn opened_audio(
        &mut self,
        def_id: i32,
        outcome: Result<Vec<String>, String>,
        reply: Option<Reply>,
    ) {
        let answer = |host: &mut Host, answer: Result<Value, (String, String)>| match reply {
            Some(Reply::Client(to, seq)) => {
                let msg = super::apps::app_reply(seq, answer);
                host.pending_effects.push(HostEffect::Send { to, msg });
            }
            #[cfg(test)]
            Some(Reply::Local(tx)) => {
                let _ = tx.send(answer);
            }
            None => {
                if let Err((_, why)) = answer {
                    diag::warn!("audio: {why}");
                }
            }
        };
        let dialogs = match outcome {
            Ok(opened) => opened,
            Err(why) => {
                if let Some(audio) = self.audio.as_mut() {
                    audio.windows.remove(&def_id);
                }
                return answer(self, Err(("value".into(), why)));
            }
        };
        let Some((structure, title, kind)) = self
            .audio
            .as_ref()
            .and_then(|a| a.windows.get(&def_id))
            .map(|w| (w.structure, w.title.clone(), w.kind))
        else {
            return;
        };
        let mut ids = serde_json::Map::new();
        let mut widgets = Vec::new();
        let roles = match kind {
            AppKind::Audio => ROLES,
            AppKind::Multitrack => super::audio_app::MULTITRACK_ROLES,
            AppKind::Roll { .. } | AppKind::Notes => &["widget"],
            #[cfg(feature = "notation")]
            AppKind::Score | AppKind::Page { .. } => super::audio_app::SCORE_ROLES,
        };
        for role in roles.iter().map(|r| (*r).to_string()) {
            let Some(id) = self.own_widget(structure, AUDIO, &role) else {
                return answer(self, Err(("runtime".into(), "out of widget ids".into())));
            };
            ids.insert(role, json!(id));
            widgets.push(id);
        }
        // the widgets the application named: a dialog's, or -- where it says
        // which, as a score does -- a tool's or a palette entry's
        let (mut dialog_ids, mut tool_ids, mut palette_ids) = (
            serde_json::Map::new(),
            serde_json::Map::new(),
            serde_json::Map::new(),
        );
        for name in dialogs {
            let (group, plain) = match name.split_once(':') {
                Some((group @ ("tool" | "palette" | "dialog"), plain))
                    if !matches!(kind, AppKind::Multitrack | AppKind::Audio) =>
                {
                    (group, plain.to_string())
                }
                _ => ("dialog", name),
            };
            let Some(id) = self.own_widget(structure, AUDIO, &format!("{group}:{plain}")) else {
                return answer(self, Err(("runtime".into(), "out of widget ids".into())));
            };
            match group {
                "tool" => tool_ids.insert(plain, json!(id)),
                "palette" => palette_ids.insert(plain, json!(id)),
                _ => dialog_ids.insert(plain, json!(id)),
            };
            widgets.push(id);
        }
        ids.insert("dialogs".into(), Value::Object(dialog_ids));
        ids.insert("tools".into(), Value::Object(tool_ids));
        ids.insert("palettes".into(), Value::Object(palette_ids));
        let takes = match kind {
            AppKind::Audio => vec![
                ids["widget"].as_i64().unwrap_or(0) as i32,
                ids["lane"].as_i64().unwrap_or(0) as i32,
            ],
            _ => Vec::new(),
        };
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        if let Some(window) = audio.windows.get_mut(&def_id) {
            window.takes = takes;
            window.marks = matches!(kind, AppKind::Multitrack | AppKind::Notes)
                .then(|| ids["widget"].as_i64().map(|id| id as i32))
                .flatten();
        }
        let source = audio.source.clone();
        let _ = audio.ask(ToApp::Bind {
            def_id,
            ids: Value::Object(ids),
            widgets,
            title,
        });
        if let Some(source) = source {
            self.window_sources.insert(def_id, source);
        }
        diag::info!("audio: opened window {def_id}");
        answer(self, Ok(json!({"window": def_id})));
    }

    /// **What a run left the window**, carried out on it.
    fn perform_audio(&mut self, def_id: i32, effects: Vec<Value>, actions: Option<Value>) {
        // what the run sent the engine is taken at its next block
        self.settle_window(def_id);
        let mut fx = Vec::new();
        for effect in effects {
            match effect["kind"].as_str().unwrap_or_default() {
                "echo" => {
                    if let Ok(answer) = serde_json::from_value::<Answer>(effect["answer"].clone()) {
                        self.tell(answer);
                    }
                }
                "set" => {
                    if let (Some(widget), Value::Object(props)) =
                        (effect["widget"].as_i64(), effect["props"].clone())
                    {
                        self.set_props(widget as i32, props.into_iter().collect(), &mut fx);
                    }
                }
                "publish" => self.publish_audio(def_id, effect["def"].clone()),
                // the window that holds what this one was asked to open
                "front" => {
                    if let Some(window) = effect["window"].as_i64() {
                        self.pending_effects.push(HostEffect::Raise(window as i32));
                    }
                }
                // **the play cursor is the editor's transport's position**
                "transport" => {
                    if let Some(transport) = effect["transport"].as_u64() {
                        let clock = clocks::HeadClock::Transport(transport as usize);
                        self.set_head_clock_of(def_id, clock);
                    }
                }
                "band" => {
                    for widget in self.audio_takes(def_id) {
                        let props = vec![
                            ("sel_start".to_string(), effect["sel_start"].clone()),
                            ("sel_len".to_string(), effect["sel_len"].clone()),
                        ];
                        self.set_props(widget, props, &mut fx);
                    }
                }
                "looping" => {
                    if let Some(widget) = self.audio_takes(def_id).first() {
                        let on = i32::from(effect["on"] == true);
                        self.set_props(*widget, vec![("looping".into(), json!(on))], &mut fx);
                    }
                }
                // every widget drawn again from the editor, after a history
                // step
                "resync" => {
                    for pair in effect["props"].as_array().into_iter().flatten() {
                        if let (Some(widget), Value::Object(props)) = (pair[0].as_i64(), &pair[1])
                            && !props.is_empty()
                        {
                            let props = props.clone().into_iter().collect();
                            self.set_props(widget as i32, props, &mut fx);
                        }
                    }
                }
                "say" => {
                    let why = effect["reason"].as_str().unwrap_or_default();
                    self.say(def_id, status::Line::of_reason(None, why));
                }
                "clipboard" => {
                    let doc = effect["document"].as_str().unwrap_or_default();
                    match serde_json::from_str(doc) {
                        Ok(clip) => self.clipboard.put(clip, Vec::new()),
                        Err(e) => diag::warn!("audio: the clipboard did not read: {e}"),
                    }
                }
                "progress" => {
                    let props = match effect["done"].as_u64() {
                        Some(done) => {
                            let total = effect["total"].as_u64().unwrap_or(0).max(1);
                            // the operation the render is for, in its verb's words
                            let label = effect["label"].as_str().unwrap_or_default();
                            vec![
                                (
                                    "progress".to_string(),
                                    json!((done as f64 / total as f64).min(1.0)),
                                ),
                                ("progress_label".to_string(), json!(label)),
                            ]
                        }
                        None => vec![("progress".to_string(), json!(-1))],
                    };
                    self.set_props(def_id, props, &mut fx);
                }
                // the session the window was opened over is let go: its
                // opener is handed it and is done with the window
                // and a roll of the session let go goes with it: what it
                // draws is no longer the window's
                "released" => {
                    for roll in self.take_dependents(def_id) {
                        self.close_audio(roll);
                    }
                    if effect["handed"] == true {
                        self.released_audio(def_id, &effect["snapshot"]);
                    }
                }
                "closeWindow" => {
                    self.keep_last(def_id, &effect);
                    self.close_audio(def_id);
                    self.pending_effects.extend(fx);
                    return;
                }
                "closedWindow" => {
                    self.keep_last(def_id, &effect);
                    // the window is gone from the screen: its def goes too
                    self.free_own_window(def_id);
                    self.forget_audio(def_id);
                    if let Some(audio) = self.audio.as_mut() {
                        let _ = audio.ask(ToApp::Forget { def_id });
                    }
                }
                // what a client kept of the file in focus, the cursor and the
                // layers is the editor's own here
                _ => {}
            }
        }
        if let Some(actions) = actions
            && self.owns_audio(def_id)
        {
            self.set_props(def_id, vec![("actions".into(), actions)], &mut fx);
        }
        self.pending_effects.extend(fx);
    }

    /// **What the hand holds in multitrack window `def_id`**, for a client:
    /// the query `selected` answers the boxes held, by their regions' ids,
    /// in the order they are drawn; the action `select` holds the boxes of
    /// `regions` in place of what was held, `[]` holding none. The window
    /// keeps them, as a click or a marquee leaves them, and they enter no
    /// history. `None` for any other call, or another window.
    pub(super) fn multitrack_marks(
        &mut self,
        def_id: i32,
        call: &Value,
        effects: &mut Vec<HostEffect>,
    ) -> Option<Value> {
        let window = self.audio.as_ref()?.windows.get(&def_id)?;
        let widget = window.marks?;
        let kind = window.kind;
        if call["query"] == "selected" {
            let held: Vec<Value> = self
                .live_props(widget)
                .get("selected")
                .and_then(Value::as_str)
                .and_then(|names| serde_json::from_str::<Vec<Value>>(names).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|name| match name.as_str().map(str::parse::<u64>) {
                    Some(Ok(id)) => json!(id),
                    _ => name,
                })
                .collect();
            return Some(json!({ "selected": held }));
        }
        if call["do"]["verb"] == "select" {
            let names: Vec<String> = call["do"]["regions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|region| match region {
                    Value::String(name) => name.clone(),
                    other => other.to_string(),
                })
                .collect();
            // a roll marks its notes by their ids, as numbers
            let held = if kind == AppKind::Notes {
                Value::from(call["do"]["regions"].to_string())
            } else {
                Value::from(json!(names).to_string())
            };
            self.set_props(widget, vec![("selected".into(), held)], effects);
            effects.push(HostEffect::Redraw(def_id));
            return Some(Value::Null);
        }
        None
    }

    /// The widgets of window `def_id` that draw its take.
    fn audio_takes(&self, def_id: i32) -> Vec<i32> {
        self.audio
            .as_ref()
            .and_then(|audio| audio.windows.get(&def_id))
            .map(|w| w.takes.clone())
            .unwrap_or_default()
    }

    /// **The window drawn again**, from the tree its application drew with
    /// the ids it was numbered with.
    fn publish_audio(&mut self, def_id: i32, mut def: Value) {
        use std::net::{Ipv4Addr, SocketAddr};

        let Some(structure) = self
            .audio
            .as_ref()
            .and_then(|a| a.windows.get(&def_id))
            .map(|w| w.structure)
        else {
            return;
        };
        if !def.is_object() {
            return;
        }
        def["id"] = json!(def_id);
        self.number_own(&mut def, structure, AUDIO);
        let origin = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)));
        let effects = self.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![OscType::Int(def_id), OscType::String(def.to_string())],
            }),
            origin,
        );
        self.pending_effects.extend(effects);
    }

    /// **Closes the audio editor's window `def_id`**, as its editor asked once
    /// nothing was left to lose: the window freed here, and what sounded it
    /// freed, its renders called off and its working folder removed by its
    /// application.
    fn close_audio(&mut self, def_id: i32) {
        self.free_own_window(def_id);
        self.forget_audio(def_id);
        if let Some(audio) = self.audio.as_mut() {
            let _ = audio.ask(ToApp::Close { def_id });
        }
    }

    /// What a closing effect says the window's structure is now, kept for
    /// the close's message.
    fn keep_last(&mut self, def_id: i32, effect: &Value) {
        if let Some(window) = self
            .audio
            .as_mut()
            .and_then(|audio| audio.windows.get_mut(&def_id))
            && !window.released
        {
            if effect["left"].is_object() {
                window.last = Some(json!({"multitrack": effect["left"], "snapshot": null}));
            } else if effect["multitrack"].is_object() || effect["snapshot"].is_object() {
                window.last =
                    Some(json!({"multitrack":effect["multitrack"],"snapshot":effect["snapshot"]}));
            }
        }
    }

    /// **Window `def_id` let go of the session it was opened over**: whoever
    /// opened it is handed what the window left it as
    /// ([`GUI_APP_RELEASED`]): the window is the host's own work from
    /// then on, and its close hands the opener nothing more.
    fn released_audio(&mut self, def_id: i32, snapshot: &Value) {
        let Some(window) = self
            .audio
            .as_mut()
            .and_then(|audio| audio.windows.get_mut(&def_id))
        else {
            return;
        };
        window.last = None;
        window.released = true;
        let Some(to) = window.opener else {
            return;
        };
        self.pending_effects.push(HostEffect::Send {
            to,
            msg: OscMessage {
                addr: GUI_APP_RELEASED.into(),
                args: vec![OscType::Int(def_id), OscType::String(snapshot.to_string())],
            },
        });
    }

    /// **Window `def_id` was not opened, since window `window` holds the
    /// session it asked for** (`path`): that window comes forward, and is
    /// the answer. A client that asked for the file alone takes it over
    /// when no client holds it -- the host's own, or one a client let go
    /// of -- and is answered it as one it opened; otherwise it is told
    /// where the session is open.
    fn held_audio(
        &mut self,
        def_id: i32,
        window: i32,
        path: &str,
        adopt: bool,
        reply: Option<Reply>,
    ) {
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        audio.windows.remove(&def_id);
        self.pending_effects.push(HostEffect::Raise(window));
        diag::info!("audio: {path} is open in window {window}, brought forward");
        let refused = (
            "value".to_string(),
            format!("{path} is open in window {window}"),
        );
        match reply {
            Some(Reply::Client(to, seq)) => {
                let answer = match audio.windows.get_mut(&window) {
                    Some(held) if adopt && (held.opener.is_none() || held.released) => {
                        // **the client that let it go hears it gone**, as
                        // a window of its own that closed
                        let before = held.opener.replace(to).filter(|before| *before != to);
                        held.released = false;
                        held.last = None;
                        if let Some(before) = before {
                            self.pending_effects.push(HostEffect::Send {
                                to: before,
                                msg: OscMessage {
                                    addr: GUI_CLOSED.into(),
                                    args: vec![OscType::Int(window)],
                                },
                            });
                        }
                        let _ = audio.ask(ToApp::Adopt { def_id: window });
                        Ok(json!({"window": window}))
                    }
                    _ => Err(refused),
                };
                let msg = super::apps::app_reply(seq, answer);
                self.pending_effects.push(HostEffect::Send { to, msg });
            }
            #[cfg(test)]
            Some(Reply::Local(tx)) => {
                let _ = tx.send(Err(refused));
            }
            None => {}
        }
    }

    /// **What window `def_id` holds moved by a hand**: whoever opened it is
    /// told ([`GUI_APP_CHANGED`]).
    fn changed_audio(&mut self, def_id: i32, version: i64) {
        let Some(to) = self
            .audio
            .as_ref()
            .and_then(|audio| audio.windows.get(&def_id))
            .and_then(|w| w.opener)
        else {
            return;
        };
        self.pending_effects.push(HostEffect::Send {
            to,
            msg: OscMessage {
                addr: GUI_APP_CHANGED.into(),
                args: vec![OscType::Int(def_id), OscType::Long(version)],
            },
        });
    }

    /// **The window is this host's no longer, and whoever opened it is
    /// told**: `/gui_closed`, with what its application left it as when it
    /// said (the multitrack's, as the document's JSON).
    fn forget_audio(&mut self, def_id: i32) {
        self.window_sources.remove(&def_id);
        let Some(window) = self
            .audio
            .as_mut()
            .and_then(|audio| audio.windows.remove(&def_id))
        else {
            return;
        };
        if let Some(to) = window.opener {
            let mut args = vec![OscType::Int(def_id)];
            if let Some(last) = window.last.filter(|_| !window.released) {
                args.push(OscType::String(last["multitrack"].to_string()));
                if last["snapshot"].is_object() {
                    args.push(OscType::String(last["snapshot"].to_string()));
                }
            }
            self.pending_effects.push(HostEffect::Send {
                to,
                msg: OscMessage {
                    addr: GUI_CLOSED.into(),
                    args,
                },
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mono float file of `frames` frames, each told apart.
    fn take(name: &str, frames: usize) -> String {
        let path =
            std::env::temp_dir().join(format!("host-audio-{name}-{}.wav", std::process::id()));
        let path = path.to_string_lossy().into_owned();
        let mut w = clausters_core::pcm::Writer::create(&path, 1, 48_000.0, "float").unwrap();
        let values: Vec<f32> = (0..frames)
            .map(|n| 0.25 + n as f32 / (1 << 20) as f32)
            .collect();
        w.write(&values).unwrap();
        w.finalize().unwrap();
        path
    }

    /// The host's ticks turned until `done`, or a failure past 20 s.
    fn until(host: &mut Host, what: &str, mut done: impl FnMut(&mut Host) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            host.take_effects();
            if done(host) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "{what}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// **Window `def_id`'s editor asked `call`**, its answer waited for.
    fn ask(host: &mut Host, def_id: i32, call: Value) -> Value {
        let rx = host.ask_audio(def_id, call);
        let mut answer = None;
        until(host, "the editor never answered", |_| {
            answer = rx.try_recv().ok();
            answer.is_some()
        });
        answer
            .unwrap()
            .unwrap_or_else(|(kind, why)| json!({"raised": kind, "why": why}))
    }

    /// **A window opened over `path` and settled**: drawn, and the peaks
    /// render the open hands off landed.
    fn opened(host: &mut Host, path: &str) -> i32 {
        let def_id = host.open_audio(Some(path), "take").expect("opened");
        until(host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        settled(host, def_id);
        def_id
    }

    /// **The open's work landed**: nothing is pending on the file.
    fn settled(host: &mut Host, def_id: i32) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while ask(host, def_id, json!({"query": "actions"}))["save"]["pending"] == true {
            assert!(
                std::time::Instant::now() < deadline,
                "the open never settled"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The parts the take of window `def_id` is made of.
    fn parts(host: &mut Host, def_id: i32) -> Vec<Value> {
        ask(host, def_id, json!({"member": {"verb": "segments"}}))["parts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// A host over a detached engine of its own.
    fn host() -> Host {
        let (engine, _processed) = AppEngine::detached(48_000.0, 2);
        let mut host = Host::new();
        host.hold_audio_engine(Arc::new(Mutex::new(engine)));
        host
    }

    /// The `widget` id of window `def_id`, as its tree names it.
    fn widget_of(host: &Host, def_id: i32) -> i32 {
        let audio = host.audio.as_ref().unwrap();
        audio.windows[&def_id].takes[0]
    }

    /// **The host opens the multitrack over a session in its folder**: the
    /// window is drawn, its take is drawn from its file's peaks (measured
    /// into the session's folder), the multitrack plays on the
    /// application's engine, and it closes.
    #[test]
    fn the_host_owns_a_multitrack_opened_from_a_session() {
        use clausters_document::multitrack::{
            Content, Multitrack, Region, TakeLane, Track, Window as RegionWindow,
        };
        use clausters_document::session::{Session, Source};
        use clausters_document::{Document, Lifetime, NodeId, Opaque, SourceId, SourceRef};

        let folder = std::env::temp_dir().join(format!("host-mt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::rename(take("mt", 24_000), folder.join("take.wav")).unwrap();
        let content = Content::Window {
            window: RegionWindow {
                source: SourceRef {
                    source: SourceId(1),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                },
                start: 0.0,
                duration: 0.5,
            },
            playrate: 1.0,
            args: Opaque::none(),
            looping: false,
        };
        let mut track = Track::new(NodeId(2), NodeId(3));
        track.take_lanes[0] = TakeLane {
            regions: vec![Region::new(
                NodeId(4),
                clausters_document::Second(0.0),
                clausters_document::Second(0.5),
                content,
            )],
            ..TakeLane::new(NodeId(3))
        };
        let session = Session::new(Document::empty())
            .with_multitrack(Multitrack {
                tracks: vec![track],
                ..Multitrack::default()
            })
            .with_source(SourceId(1), Source::file("take.wav", Lifetime::Session));
        let path = folder.join("song.json");
        std::fs::write(&path, serde_json::to_string(&session).unwrap()).unwrap();

        let mut host = host();
        let def_id = host
            .open_multitrack(Some(path.to_str().unwrap()), "song")
            .expect("opened");
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        let structure = host.audio.as_ref().unwrap().windows[&def_id].structure;
        let widget = host.own_widget_id(structure, AUDIO, "widget").unwrap();
        // the take is drawn from its file, its peaks in the session's folder
        until(&mut host, "the peaks never landed", |h| {
            let props = ask(
                h,
                def_id,
                json!({"member": {"verb": "props", "widget": widget}}),
            );
            props["lists"]["1"]["parts"][0]["peaks"].is_string()
        });
        assert!(folder.join("peaks").is_dir());
        // **what the hand holds is the window's**, read and set by a client
        let mut fx = Vec::new();
        let mut marks = |h: &mut Host, call: Value| h.multitrack_marks(def_id, &call, &mut fx);
        assert_eq!(
            marks(&mut host, json!({"query": "selected"})),
            Some(json!({"selected": []}))
        );
        marks(&mut host, json!({"do": {"verb": "select", "regions": [4]}}));
        assert_eq!(
            marks(&mut host, json!({"query": "selected"})),
            Some(json!({"selected": [4]}))
        );
        marks(&mut host, json!({"do": {"verb": "select", "regions": []}}));
        assert_eq!(
            marks(&mut host, json!({"query": "selected"})),
            Some(json!({"selected": []}))
        );
        assert_eq!(marks(&mut host, json!({"query": "transport"})), None);
        let played = ask(&mut host, def_id, json!({"do": {"verb": "play"}}));
        assert!(played.get("raised").is_none(), "{played}");
        let transport = ask(&mut host, def_id, json!({"query": "transport"}));
        assert_eq!(transport["rolling"], true, "{transport}");
        host.close_audio(def_id);
        assert!(!host.owns_audio(def_id));
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// **A multitrack handed over whole opens as an untitled session**, and
    /// a script's handle reads and edits the multitrack the window holds:
    /// an edit is an entry of the window's history, undone there, and the
    /// first save names the file.
    #[test]
    fn a_handed_multitrack_opens_untitled_and_its_handle_edits_it() {
        let folder = std::env::temp_dir().join(format!("host-untitled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let mut host = host();
        let multitrack = json!({"tracks": [{"id": 1, "name": "a", "takeLanes": [{"id": 2}]}]});
        let def_id = host
            .open_multitrack_over(multitrack, "untitled", None)
            .expect("opened");
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        let handle = |host: &mut Host, call: Value| {
            ask(
                host,
                def_id,
                json!({"do": {"verb": "handle", "call": call}}),
            )
        };
        assert_eq!(
            handle(&mut host, json!({"verb": "ids", "of": "tracks"}))["ids"],
            json!([1])
        );
        let state = handle(&mut host, json!({"verb": "state"}));
        let mut tracks = state["tracks"].clone();
        tracks[0]["name"] = json!("b");
        let applied = handle(
            &mut host,
            json!({"verb": "apply", "intent": {"intent": "settracks", "tracks": tracks}}),
        );
        assert_eq!(applied["applied"], true, "{applied}");
        let name =
            |host: &mut Host| handle(host, json!({"verb": "track", "id": 1}))["name"].clone();
        assert_eq!(name(&mut host), "b");
        let history = ask(&mut host, def_id, json!({"query": "history"}));
        assert_eq!(history["canUndo"], true, "{history}");
        assert_eq!(
            ask(&mut host, def_id, json!({"do": {"verb": "undo"}})),
            true
        );
        assert_eq!(
            name(&mut host),
            "a",
            "the window's undo takes the script's edit back"
        );
        let history = ask(&mut host, def_id, json!({"query": "history"}));
        assert_eq!(history["canUndo"], false, "{history}");
        assert_eq!(history["canRedo"], true, "{history}");
        assert_eq!(
            ask(&mut host, def_id, json!({"do": {"verb": "redo"}})),
            true
        );
        assert_eq!(name(&mut host), "b", "a confirmed redo reapplies the edit");
        let history = ask(&mut host, def_id, json!({"query": "history"}));
        assert_eq!(history["canUndo"], true, "{history}");
        assert_eq!(history["canRedo"], false, "{history}");
        assert_eq!(
            ask(&mut host, def_id, json!({"do": {"verb": "undo"}})),
            true
        );
        assert_eq!(name(&mut host), "a");
        let path = folder.join("untitled.json");
        let saved = ask(
            &mut host,
            def_id,
            json!({"do": {"verb": "save", "path": path.to_str().unwrap()}}),
        );
        assert!(saved.get("raised").is_none(), "{saved}");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["multitrack"]["tracks"][0]["name"], "a", "{written}");
        host.close_audio(def_id);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// **A multitrack's window says whether its session is unsaved**, as
    /// every application's does: an edit marks its title, a save takes the
    /// mark away, and an undo past the save puts it back.
    #[test]
    fn a_multitrack_with_unsaved_work_marks_its_title() {
        let folder = std::env::temp_dir().join(format!("host-mt-marked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let mut host = host();
        let multitrack = json!({"tracks": [{"id": 1, "name": "a", "takeLanes": [{"id": 2}]}]});
        let def_id = host
            .open_multitrack_over(multitrack, "song", None)
            .expect("opened");
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        let title = |host: &mut Host| host.window_title(def_id).unwrap();
        let handle = |host: &mut Host, call: Value| {
            ask(
                host,
                def_id,
                json!({"do": {"verb": "handle", "call": call}}),
            )
        };
        let mut tracks = handle(&mut host, json!({"verb": "state"}))["tracks"].clone();
        tracks[0]["name"] = json!("b");
        handle(
            &mut host,
            json!({"verb": "apply", "intent": {"intent": "settracks", "tracks": tracks}}),
        );
        until(&mut host, "the edit never marked the title", |host| {
            title(host) == "*song"
        });
        let path = folder.join("song.json");
        let saved = ask(
            &mut host,
            def_id,
            json!({"do": {"verb": "save", "path": path.to_str().unwrap()}}),
        );
        assert!(saved.get("raised").is_none(), "{saved}");
        until(&mut host, "the save never took the mark away", |host| {
            title(host) == "song"
        });
        ask(&mut host, def_id, json!({"do": {"verb": "undo"}}));
        until(&mut host, "the undo never marked the title", |host| {
            title(host) == "*song"
        });
        host.close_audio(def_id);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// **An untitled multitrack's Save asks where**: the window's Save opens
    /// the Save as form, whose `OK` writes the session at the path typed,
    /// and a later Save writes there again without asking.
    #[test]
    fn an_untitled_multitrack_is_saved_through_the_save_as_form() {
        let folder = std::env::temp_dir().join(format!("host-save-as-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let mut host = host();
        let multitrack = json!({"tracks": [{"id": 1, "name": "a", "takeLanes": [{"id": 2}]}]});
        let def_id = host
            .open_multitrack_over(multitrack, "untitled", None)
            .expect("opened");
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        let structure = host.audio.as_ref().unwrap().windows[&def_id].structure;
        let widget = |host: &Host, name: &str| host.own_widget_id(structure, AUDIO, name).unwrap();
        let stack = widget(&host, "dialog:stack");
        let page = |host: &Host| match host.window_defs[&def_id].find(stack).map(|w| &w.kind) {
            Some(crate::host::widget::WidgetKind::Stack { index, .. }) => *index,
            other => panic!("a stack: {other:?}"),
        };
        assert_eq!(page(&host), 0, "no dialog is up");
        let save = host.event_message(def_id, 1, vec![OscType::String("save".into())]);
        assert!(host.deliver(def_id, &save));
        until(&mut host, "the Save as form never came up", |h| {
            page(h) == 1
        });
        let path = folder.join("named.json");
        let field = widget(&host, "dialog:file:path");
        let typed = host.event_message(field, 2, vec![OscType::String(path.display().to_string())]);
        assert!(host.deliver(def_id, &typed));
        let ok = widget(&host, "dialog:file:ok");
        let click = host.event_message(ok, 3, vec![OscType::String("click".into())]);
        assert!(host.deliver(def_id, &click));
        until(&mut host, "the session was never written", |h| {
            path.exists() && page(h) == 0
        });
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["multitrack"]["tracks"][0]["name"], "a", "{written}");
        // the next Save writes there, asking nothing
        std::fs::remove_file(&path).unwrap();
        let save = host.event_message(def_id, 4, vec![OscType::String("save".into())]);
        assert!(host.deliver(def_id, &save));
        until(&mut host, "the second save never wrote", |_| path.exists());
        assert_eq!(page(&host), 0);
        host.close_audio(def_id);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// **A multitrack opened over no session opens one from its File
    /// menu**: Open puts the form up, a pick in its chooser opens the
    /// session in the window, and File > New over unsaved work asks first,
    /// Don't save letting it go.
    #[test]
    fn an_empty_multitrack_opens_a_session_from_its_file_menu() {
        use clausters_document::Document;
        use clausters_document::session::Session;

        let folder = std::env::temp_dir().join(format!("host-file-menu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("chosen.json");
        std::fs::write(
            &path,
            serde_json::to_string(&Session::new(Document::empty())).unwrap(),
        )
        .unwrap();
        let mut host = host();
        let def_id = host.open_multitrack_saving_to(None, None, "empty").unwrap();
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        let structure = host.audio.as_ref().unwrap().windows[&def_id].structure;
        let widget = |host: &Host, name: &str| host.own_widget_id(structure, AUDIO, name).unwrap();
        let index = |host: &Host, stack: i32| match host.window_defs[&def_id]
            .find(stack)
            .map(|w| &w.kind)
        {
            Some(crate::host::widget::WidgetKind::Stack { index, .. }) => *index,
            other => panic!("a stack: {other:?}"),
        };
        let dialogs = widget(&host, "dialog:stack");
        let verb = |host: &mut Host, seq, verb: &str| {
            let message = host.event_message(def_id, seq, vec![OscType::String(verb.into())]);
            assert!(host.deliver(def_id, &message));
        };
        verb(&mut host, 1, "open_file");
        until(&mut host, "the Open form never came up", |h| {
            index(h, dialogs) == 2
        });
        let chooser = widget(&host, "dialog:open:path");
        let picked = host.event_message(
            chooser,
            2,
            vec![
                OscType::String("pick".into()),
                OscType::String(path.display().to_string()),
            ],
        );
        assert!(host.deliver(def_id, &picked));
        until(&mut host, "the session was never opened", |h| {
            index(h, dialogs) == 0
                && ask(h, def_id, json!({"query": "file"}))["path"] == json!(path)
        });
        // unsaved work: File > New asks, and Don't save lets it go
        let edited = ask(
            &mut host,
            def_id,
            json!({"do": {"verb": "handle", "call": {"verb": "apply",
                "intent": {"intent": "setmarker", "marker": 7, "at": 1.0}}}}),
        );
        assert_eq!(edited["applied"], true, "{edited}");
        verb(&mut host, 3, "new_file");
        let close = widget(&host, "close_stack");
        until(&mut host, "the close form never came up", |h| {
            index(h, close) == 1
        });
        assert_eq!(
            ask(&mut host, def_id, json!({"query": "file"}))["path"],
            json!(path)
        );
        let discard = widget(&host, "close_discard");
        let click = host.event_message(discard, 4, vec![OscType::String("click".into())]);
        assert!(host.deliver(def_id, &click));
        until(&mut host, "the new session never came", |h| {
            index(h, close) == 0 && ask(h, def_id, json!({"query": "file"}))["path"].is_null()
        });
        host.close_audio(def_id);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// **A session in `folder` holding one box of notes**: region 3 over
    /// sequence 5, one note long; the path of its file.
    fn a_session_with_a_box_of_notes(folder: &std::path::Path) -> std::path::PathBuf {
        use clausters_document::events::{Event, EventSequence};
        use clausters_document::multitrack::{
            Content, Multitrack, Region, TakeLane, Track, Window as RegionWindow,
        };
        use clausters_document::session::{Location, Session, Source};
        use clausters_document::{Document, Lifetime, NodeId, Opaque, SourceId, SourceRef};

        let _ = std::fs::remove_dir_all(folder);
        std::fs::create_dir_all(folder).unwrap();
        let mut source = Source::file("", Lifetime::Session);
        source.location = Location::Events {
            sequence: Box::new(EventSequence::new(vec![Event::new(
                0.0,
                json!({"midinote": 72, "dur": 0.5}),
            )])),
        };
        let content = Content::Window {
            window: RegionWindow {
                source: SourceRef {
                    source: SourceId(5),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                },
                start: 0.0,
                duration: 1.0,
            },
            playrate: 1.0,
            args: Opaque::none(),
            looping: false,
        };
        let mut track = Track::new(NodeId(1), NodeId(2));
        track.take_lanes[0] = TakeLane {
            regions: vec![Region::new(
                NodeId(3),
                clausters_document::Second(0.0),
                clausters_document::Second(1.0),
                content,
            )],
            ..TakeLane::new(NodeId(2))
        };
        let session = Session::new(Document::empty())
            .with_multitrack(Multitrack {
                tracks: vec![track],
                ..Multitrack::default()
            })
            .with_source(SourceId(5), source);
        let path = folder.join("notes.json");
        std::fs::write(&path, serde_json::to_string(&session).unwrap()).unwrap();
        path
    }

    /// **A double click on a box of notes opens its roll in the
    /// application**: a window of its own over the box's sequence, played
    /// on a transport of its own by the space bar, which goes when the
    /// multitrack's does.
    #[test]
    fn a_box_of_notes_opens_its_roll_in_the_application() {
        let folder = std::env::temp_dir().join(format!("host-roll-{}", std::process::id()));
        let path = a_session_with_a_box_of_notes(&folder);
        let mut host = host();
        let def_id = host
            .open_multitrack(Some(path.to_str().unwrap()), "notes")
            .expect("opened");
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        let structure = host.audio.as_ref().unwrap().windows[&def_id].structure;
        let widget = host.own_widget_id(structure, AUDIO, "widget").unwrap();
        let open = host.event_message(
            widget,
            1,
            vec![OscType::String("open".into()), OscType::String("3".into())],
        );
        assert!(
            host.deliver(def_id, &open),
            "the gesture is the multitrack's"
        );
        let mut roll = None;
        until(&mut host, "the roll never opened", |h| {
            roll = h.audio.as_ref().and_then(|audio| {
                audio
                    .windows
                    .iter()
                    .find(|(_, w)| matches!(w.kind, AppKind::Roll { source: 5, .. }))
                    .map(|(id, _)| *id)
            });
            roll.is_some_and(|roll| h.window_def(roll).is_some())
        });
        let roll = roll.unwrap();
        // **Its play cursor is a transport of its own**, not the multitrack's
        let clocks::HeadClock::Transport(transport) = host.head_clock_of(roll, None) else {
            panic!("the roll's play cursor reads no transport");
        };
        assert_ne!(transport, 0, "the multitrack's transport is not the roll's");
        // and the space bar plays the roll there, and stops it
        let rolling =
            |host: &mut Host| ask(host, roll, json!({"query": "transport"}))["rolling"].clone();
        assert_eq!(rolling(&mut host), false);
        let play = host.event_message(roll, 2, host.play_verb());
        assert!(host.deliver(roll, &play), "the space bar is the roll's");
        until(&mut host, "the roll never played", |h| rolling(h) == true);
        let play = host.event_message(roll, 3, host.play_verb());
        assert!(host.deliver(roll, &play));
        until(&mut host, "the roll never stopped", |h| rolling(h) == false);
        // **Closed by the person** (Escape, its close mark), it is let go,
        // and the box opens it again
        host.closed_by_user(roll);
        until(&mut host, "the closed roll stayed", |h| {
            !h.owns_audio(roll) && h.window_def(roll).is_none()
        });
        assert!(host.deliver(def_id, &open), "opened again");
        let mut again = None;
        until(&mut host, "the roll never opened again", |h| {
            again = h.audio.as_ref().and_then(|audio| {
                audio
                    .windows
                    .iter()
                    .find(|(_, w)| matches!(w.kind, AppKind::Roll { source: 5, .. }))
                    .map(|(id, _)| *id)
            });
            again.is_some_and(|again| h.window_def(again).is_some())
        });
        let roll = again.unwrap();
        // **The File menu's Close closes it** too
        let close = host.event_message(
            roll,
            4,
            vec![
                OscType::String("menu".into()),
                OscType::String("close".into()),
            ],
        );
        assert!(host.deliver(roll, &close), "the entry is the roll's");
        until(&mut host, "File > Close left the roll", |h| {
            !h.owns_audio(roll) && h.window_def(roll).is_none()
        });
        let find = |h: &Host| {
            h.audio.as_ref().and_then(|audio| {
                audio
                    .windows
                    .iter()
                    .find(|(_, w)| matches!(w.kind, AppKind::Roll { source: 5, .. }))
                    .map(|(id, _)| *id)
            })
        };
        assert!(host.deliver(def_id, &open), "opened again");
        until(&mut host, "the roll never opened a third time", |h| {
            find(h).is_some_and(|roll| h.window_def(roll).is_some())
        });
        let roll = find(&host).unwrap();
        // **A session let go takes its rolls along**: what they draw is no
        // longer the window's, and the multitrack goes on
        let new = host.event_message(def_id, 5, vec![OscType::String("new_file".into())]);
        assert!(host.deliver(def_id, &new));
        until(&mut host, "a roll outlived its session", |h| {
            !h.owns_audio(roll) && h.window_def(roll).is_none()
        });
        assert!(host.owns_audio(def_id), "the multitrack stays");
        host.close_audio(def_id);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// **A double click on a box drawn as its page opens its page**: a
    /// score window over the box's sequence, beside the multitrack, opened
    /// once however often it is asked for, and gone with the session it was
    /// opened from; drawn as rolls again, the box opens its roll.
    #[cfg(feature = "score")]
    #[test]
    fn a_box_drawn_as_its_page_opens_its_page() {
        let folder = std::env::temp_dir().join(format!("host-page-{}", std::process::id()));
        let path = a_session_with_a_box_of_notes(&folder);
        let mut host = host();
        let def_id = host
            .open_multitrack(Some(path.to_str().unwrap()), "notes")
            .expect("opened");
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        let menu = |host: &mut Host, seq: i32, verb: &str| {
            let message = host.event_message(
                def_id,
                seq,
                vec![OscType::String("menu".into()), OscType::String(verb.into())],
            );
            assert!(host.deliver(def_id, &message), "{verb} is the multitrack's");
        };
        menu(&mut host, 1, "notes_score");
        let structure = host.audio.as_ref().unwrap().windows[&def_id].structure;
        let widget = host.own_widget_id(structure, AUDIO, "widget").unwrap();
        let open = host.event_message(
            widget,
            2,
            vec![OscType::String("open".into()), OscType::String("3".into())],
        );
        let find = |h: &Host, page: bool| {
            h.audio.as_ref().and_then(|audio| {
                audio
                    .windows
                    .iter()
                    .find(|(_, w)| match w.kind {
                        AppKind::Page { source: 5, .. } => page,
                        AppKind::Roll { source: 5, .. } => !page,
                        _ => false,
                    })
                    .map(|(id, _)| *id)
            })
        };
        assert!(
            host.deliver(def_id, &open),
            "the gesture is the multitrack's"
        );
        until(&mut host, "the page never opened", |h| {
            find(h, true).is_some_and(|page| h.window_def(page).is_some())
        });
        assert!(
            find(&host, false).is_none(),
            "a roll opened beside the page"
        );
        let page = find(&host, true).unwrap();
        // **over the box's own sequence**, the multitrack's
        let len = ask(
            &mut host,
            page,
            json!({"do": {"verb": "handle", "call": {"verb": "len"}}}),
        );
        assert_eq!(len["len"], 1, "{len}");
        // asked again, the page already open is the one there is
        assert!(host.deliver(def_id, &open));
        for _ in 0..20 {
            host.take_effects();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let pages = host.audio.as_ref().map_or(0, |audio| {
            audio
                .windows
                .values()
                .filter(|w| matches!(w.kind, AppKind::Page { .. }))
                .count()
        });
        assert_eq!(pages, 1, "a second page opened");
        // **drawn as rolls, the box opens its roll**
        menu(&mut host, 3, "notes_roll");
        assert!(host.deliver(def_id, &open));
        until(&mut host, "the roll never opened", |h| {
            find(h, false).is_some_and(|roll| h.window_def(roll).is_some())
        });
        // **a session let go takes its page along**
        let new = host.event_message(def_id, 4, vec![OscType::String("new_file".into())]);
        assert!(host.deliver(def_id, &new));
        until(&mut host, "a page outlived its session", |h| {
            !h.owns_audio(page) && h.window_def(page).is_none()
        });
        host.close_audio(def_id);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// **The host opens the audio editor over a file and answers its
    /// window**: the window is drawn, its meters and play cursor read the
    /// application's engine, and a delete made on its picture is the
    /// editor's edit -- the take is the part after it.
    #[test]
    fn the_host_owns_an_audio_editor_and_answers_its_window() {
        let path = take("owns", 24_000);
        let mut host = host();
        let def_id = opened(&mut host, &path);
        assert!(host.window_def(def_id).is_some(), "the window is drawn");
        assert!(host.source_of(def_id).is_some(), "it reads the engine");
        assert!(
            matches!(
                host.head_clock_of(def_id, None),
                clocks::HeadClock::Transport(_)
            ),
            "its play cursor is drawn from the editor's transport"
        );
        let before = parts(&mut host, def_id);
        assert_eq!(before.len(), 1, "{before:?}");
        let widget = widget_of(&host, def_id);
        let delete = host.event_message(
            widget,
            1,
            vec![
                OscType::String("delete".into()),
                OscType::Float(0.0),
                OscType::Float(12_000.0),
            ],
        );
        assert!(
            host.deliver(def_id, &delete),
            "the window's gesture is the editor's"
        );
        let after = parts(&mut host, def_id);
        assert_eq!(after.len(), 1, "{after:?}");
        assert_eq!(after[0]["start"], 12_000, "{after:?}");
        assert_eq!(after[0]["frames"], 12_000, "{after:?}");
        let _ = std::fs::remove_file(&path);
    }

    /// **An editor opened before the front runs wakes it**: `--audio` opens
    /// its window before the loop sets its wake, and the window is drawn on
    /// the turn the application wakes, so a wake set later must still reach
    /// the editor's thread -- else the window never appears.
    #[test]
    fn an_editor_opened_before_the_wake_is_set_still_wakes_the_loop() {
        let mut host = host();
        host.open_audio(None, "Audio").expect("opened");
        let (tx, rx) = channel();
        let tx = Mutex::new(tx);
        host.set_app_wake(Arc::new(move || {
            let _ = tx.lock().unwrap().send(());
        }));
        rx.recv_timeout(std::time::Duration::from_secs(20))
            .expect("the application woke the loop");
    }

    /// **A host opened over an editor is that application** before its window
    /// is drawn: the front reads it as it starts, and ends the process when
    /// that window is closed.
    #[test]
    fn a_host_owns_its_application_before_the_window_is_drawn() {
        let mut host = host();
        assert!(!host.owns_an_application());
        let def_id = host.open_audio(None, "Audio").expect("opened");
        assert!(host.window_def(def_id).is_none(), "not drawn yet");
        assert!(host.owns_an_application());
    }

    /// **An applied gain curve leaves the window editing the take**: the
    /// apply lands after its render, and what the window is told then is
    /// the pane as the editor has it, with the hand back on the samples and
    /// the curve hidden, not only its label.
    #[test]
    fn an_applied_envelope_leaves_the_window_editing_the_take() {
        let path = take("envelope", 24_000);
        let mut host = host();
        let def_id = opened(&mut host, &path);
        let pane = *host
            .registry
            .kinds()
            .iter()
            .find(|(_, kind)| *kind == "pane")
            .expect("a pane")
            .0;
        let act = |host: &mut Host, call: Value| {
            ask(host, def_id, json!({"do": {"verb": "act", "call": call}}));
        };
        let prop = |host: &Host, key: &str| host.registry.get(pane).unwrap().props[key].clone();
        act(&mut host, json!({"verb": "show_envelope"}));
        until(&mut host, "the curve was never shown", |h| {
            prop(h, "layer") == "points"
        });
        assert_eq!(prop(&host, "hidden"), "");
        act(
            &mut host,
            json!({"verb": "setautomation", "points": [0, 0, 1, 0, 0.5, 1, 1, 0]}),
        );
        act(&mut host, json!({"verb": "apply_envelope"}));
        until(&mut host, "the window never went back to the take", |h| {
            prop(h, "layer") == "take"
        });
        assert_eq!(prop(&host, "hidden"), "points", "the curve is put away");
        let _ = std::fs::remove_file(&path);
    }

    /// **Every view of the take answers the same verbs**: the spectrogram
    /// alone is the take's picture as the waveform is, so Select all sweeps
    /// the whole take in each of the three views.
    #[test]
    fn select_all_sweeps_the_take_in_every_view() {
        let path = take("views", 24_000);
        for view in ["view_wave", "view_spectrogram", "view_both"] {
            let mut host = host();
            let def_id = opened(&mut host, &path);
            let widget = widget_of(&host, def_id);
            let message = host.event_message(widget, 1, vec![OscType::String(view.into())]);
            assert!(host.deliver(def_id, &message));
            until(&mut host, "the view never came", |h| {
                let layers = h.registry.get(widget).unwrap().props.get("layers").cloned();
                layers.is_some_and(|l| (l == json!("spectrogram")) == (view == "view_spectrogram"))
            });
            let widget = widget_of(&host, def_id);
            let key = host.timeline_key(widget).expect("on an axis");
            let ctx = crate::host::gestures::GestureCtx::new(def_id, 1000, 520);
            crate::host::gestures::Gestures::default().command(&mut host, &ctx, "select_all");
            let mut swept = (0.0, 0.0);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while std::time::Instant::now() < deadline && swept.1 == 0.0 {
                host.take_effects();
                let state = host.timelines().state(key).unwrap();
                swept = (state.sel_start, state.sel_len);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(swept, (0.0, 24_000.0), "{view}");
        }
        let _ = std::fs::remove_file(&path);
    }

    /// **A correction that comes alone redraws the window**: folding a
    /// channel changes one prop of the take's picture and nothing else, and
    /// the window must be drawn again when it lands rather than at the next
    /// frame something else asks for.
    #[test]
    fn a_folded_channel_redraws_the_window() {
        let path = std::env::temp_dir()
            .join(format!("host-audio-fold-{}.wav", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let mut w = clausters_core::pcm::Writer::create(&path, 2, 48_000.0, "float").unwrap();
        w.write(&[0.25; 2 * 4800]).unwrap();
        w.finalize().unwrap();
        let mut host = host();
        let def_id = opened(&mut host, &path);
        let widget = widget_of(&host, def_id);
        let _ = host.take_effects();
        let fold = host.event_message(
            widget,
            1,
            vec![OscType::String("fold_channel".into()), OscType::Int(1)],
        );
        assert!(host.deliver(def_id, &fold));
        let mut redrawn = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !redrawn {
            assert!(std::time::Instant::now() < deadline, "never redrawn");
            redrawn = host.take_effects().iter().any(|e| {
                matches!(e, HostEffect::Redraw(id) | HostEffect::RedrawLive(id) if *id == def_id)
            }) && host.registry.get(widget).unwrap().props.get("folded") == Some(&json!([1]));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let _ = std::fs::remove_file(&path);
    }

    /// **The window has its meter from the start**: the first window drawn
    /// holds it, before the file's work is done and before the playback says
    /// where it writes, and the window keeps that meter once the open has
    /// settled, so nothing in its layout moves.
    #[test]
    fn an_opened_window_draws_its_meter() {
        let path = take("meter", 24_000);
        let mut host = host();
        let def_id = host.open_audio(Some(&path), "take").expect("opened");
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        let meter = |h: &Host| {
            h.registry
                .kinds()
                .into_iter()
                .find_map(|(id, kind)| (kind == "meter").then_some(id))
        };
        let first = meter(&host);
        assert!(first.is_some(), "the first window draws its meter");
        settled(&mut host, def_id);
        host.take_effects();
        assert_eq!(meter(&host), first, "the same meter, once the open settled");
        let _ = std::fs::remove_file(&path);
    }

    /// **A copy is held by the application, and a paste reads it there**: the
    /// window's copy puts on the host's clipboard a document that names the
    /// copy the application keeps, and a paste carrying that document puts
    /// the copied span in, read from where the copy is kept.
    #[test]
    fn a_copy_is_pasted_from_where_the_application_keeps_it() {
        let path = take("copy", 24_000);
        let mut host = host();
        let def_id = opened(&mut host, &path);
        let widget = widget_of(&host, def_id);
        let copy = host.event_message(
            widget,
            1,
            vec![
                OscType::String("copy".into()),
                OscType::Float(0.0),
                OscType::Float(6_000.0),
            ],
        );
        assert!(host.deliver(def_id, &copy));
        until(&mut host, "the copy reached the clipboard", |host| {
            host.clipboard
                .doc()
                .is_some_and(|doc| doc.kind() == "segments")
        });
        let doc = host.clipboard.doc().unwrap().clone();
        let clausters_document::clipboard::Content::Segments {
            held, parts: named, ..
        } = &doc.content
        else {
            unreachable!("a copy of segments");
        };
        assert!(named.is_empty(), "the parts stay with the application");
        assert_eq!(held.as_ref().unwrap().files, std::slice::from_ref(&path));
        let paste = host.event_message(
            widget,
            2,
            vec![
                OscType::String("paste".into()),
                OscType::Float(24_000.0),
                OscType::String("segments".into()),
                OscType::String(doc.to_json()),
            ],
        );
        assert!(host.deliver(def_id, &paste));
        until(&mut host, "the paste landed", |host| {
            parts(host, def_id).len() == 2
        });
        let after = parts(&mut host, def_id);
        assert_eq!(after[1]["start"], 0, "{after:?}");
        assert_eq!(after[1]["frames"], 6_000, "{after:?}");
        let _ = std::fs::remove_file(&path);
    }

    /// **The working folder is on disk, and goes with its window**: under the
    /// user's cache rather than the temporary directory, which is memory on
    /// many machines, and removed when the window closes or the host ends.
    #[test]
    fn a_working_folder_is_on_disk_and_goes_with_its_window() {
        let path = take("folder", 4_800);
        let mut host = host();
        // the folder the open's peaks were written into
        let folder = |host: &mut Host, def_id: i32| {
            let peaks = parts(host, def_id)[0]["peaks"]
                .as_str()
                .unwrap()
                .to_string();
            std::path::Path::new(&peaks).parent().unwrap().to_path_buf()
        };
        let def_id = opened(&mut host, &path);
        let closed = folder(&mut host, def_id);
        assert!(
            closed.starts_with(clausters_core::workspace::root()),
            "{}",
            closed.display()
        );
        assert!(closed.is_dir());
        host.close_audio(def_id);
        until(&mut host, "closing the window removed it", |_| {
            !closed.exists()
        });
        let def_id = opened(&mut host, &path);
        let ended = folder(&mut host, def_id);
        drop(host);
        assert!(!ended.exists(), "the host ending removed it");
        let _ = std::fs::remove_file(&path);
    }

    /// **A call the application refuses says why**: a gain curve applied
    /// while the take plays is refused, and the door's answer carries the
    /// reason a script raises, not a bare "nothing changed".
    #[test]
    fn a_refused_call_says_why() {
        let path = take("refused", 48_000);
        let mut host = host();
        let def_id = opened(&mut host, &path);
        let act = |verb: &str, points: Option<Value>| {
            let mut call = json!({"verb": verb});
            if let Some(points) = points {
                call["points"] = points;
            }
            json!({"do": {"verb": "act", "call": call}})
        };
        let set = ask(
            &mut host,
            def_id,
            act("setautomation", Some(json!([0, 0, 3, 0, 1.0, 0.25, 1, 0]))),
        );
        assert_eq!(set["changed"], true, "{set}");
        ask(
            &mut host,
            def_id,
            json!({"do": {"verb": "play", "at": 0.0}}),
        );
        let applied = ask(&mut host, def_id, act("apply_envelope", None));
        assert_eq!(applied["changed"], false, "{applied}");
        assert_eq!(
            applied["reason"], "the file is playing: stop it to edit",
            "{applied}"
        );
        assert_eq!(parts(&mut host, def_id)[0]["path"], json!(path));
        let _ = std::fs::remove_file(&path);
    }

    /// **A window that holds unsaved work says so in its title**: an edit
    /// leads the title with the unsaved mark, the front told of it, a save
    /// takes the mark away, and an undo past the save puts it back.
    #[test]
    fn a_window_with_unsaved_work_marks_its_title() {
        let path = take("marked", 24_000);
        let out =
            std::env::temp_dir().join(format!("host-audio-marked-out-{}.wav", std::process::id()));
        let mut host = host();
        let def_id = opened(&mut host, &path);
        let plain = host.window_title(def_id).unwrap();
        assert!(!plain.starts_with('*'), "{plain}");
        // the title turns; answers whether the front heard of it meanwhile
        // (an `ask` between takes the effects it waited through)
        let turned = |host: &mut Host, marked: bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            let mut told = false;
            loop {
                told |= host
                    .take_effects()
                    .iter()
                    .any(|e| matches!(e, HostEffect::Retitle(id) if *id == def_id));
                let title = host.window_title(def_id).unwrap();
                if title.starts_with('*') == marked {
                    return (title, told);
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the title stayed {title:?}"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        let widget = widget_of(&host, def_id);
        let delete = host.event_message(
            widget,
            1,
            vec![
                OscType::String("delete".into()),
                OscType::Float(0.0),
                OscType::Float(12_000.0),
            ],
        );
        assert!(host.deliver(def_id, &delete));
        assert_eq!(turned(&mut host, true), (format!("*{plain}"), true));
        ask(
            &mut host,
            def_id,
            json!({"do": {"verb": "save", "request": {"path": out.to_string_lossy()}}}),
        );
        assert_eq!(turned(&mut host, false).0, plain);
        let undo = host.event_message(def_id, 2, vec![OscType::String("undo".into())]);
        assert!(host.deliver(def_id, &undo));
        turned(&mut host, true);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out);
    }

    /// **A take may be cut down to nothing**: a delete of all of it leaves
    /// an empty take, which the window draws on an axis of its own, a save
    /// writes as a file that is a header, and an undo puts back.
    #[test]
    fn a_take_cut_down_to_nothing_saves_as_a_header() {
        let path = take("empty", 24_000);
        let out =
            std::env::temp_dir().join(format!("host-audio-empty-out-{}.wav", std::process::id()));
        let mut host = host();
        let def_id = opened(&mut host, &path);
        let widget = widget_of(&host, def_id);
        let delete = host.event_message(
            widget,
            1,
            vec![
                OscType::String("delete".into()),
                OscType::Float(0.0),
                OscType::Float(24_000.0),
            ],
        );
        assert!(host.deliver(def_id, &delete));
        assert!(parts(&mut host, def_id).is_empty(), "nothing is left");
        assert!(host.window_def(def_id).is_some(), "the window still draws");
        // saved, it is a header and no frames
        let saved = ask(
            &mut host,
            def_id,
            json!({"do": {"verb": "save",
                "request": {"path": out.to_string_lossy(), "format": "int16"}}}),
        );
        assert_eq!(
            saved["destination"]["path"],
            json!(out.to_string_lossy()),
            "{saved}"
        );
        let layout =
            clausters_core::pcm::Layout::open(&out.to_string_lossy()).expect("a sound file");
        assert_eq!(layout.frames, 0);
        let header = clausters_core::pcm::header(
            clausters_core::pcm::Container::Wav,
            1,
            48_000.0,
            clausters_core::pcm::Encoding::I16,
            0,
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(&out).unwrap().len(),
            header.len() as u64,
            "a WAV header and nothing else"
        );
        // and an undo puts the take back
        let undo = host.event_message(def_id, 2, vec![OscType::String("undo".into())]);
        assert!(host.deliver(def_id, &undo));
        assert_eq!(parts(&mut host, def_id).len(), 1, "undone");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out);
    }

    /// The [`GUI_APP_REPLY`] for `seq` among `effects`, read as its JSON.
    fn reply_of(effects: &[HostEffect], seq: i32) -> Option<Value> {
        effects.iter().find_map(|effect| {
            let (HostEffect::Reply(msg) | HostEffect::Send { msg, .. }) = effect else {
                return None;
            };
            match (msg.addr.as_str(), msg.args.first(), msg.args.get(1)) {
                (GUI_APP_REPLY, Some(OscType::Int(s)), Some(OscType::String(doc))) if *s == seq => {
                    serde_json::from_str(doc).ok()
                }
                _ => None,
            }
        })
    }

    fn door(host: &mut Host, addr: &str, args: Vec<OscType>, from: ClientId) -> Vec<HostEffect> {
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: addr.into(),
                args,
            }),
            from,
        )
    }

    /// The door's answer for `seq`, the host's ticks turned until it came.
    fn answered(host: &mut Host, first: Vec<HostEffect>, seq: i32) -> Value {
        if let Some(answer) = reply_of(&first, seq) {
            return answer;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if let Some(answer) = reply_of(&host.take_effects(), seq) {
                return answer;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the door never answered"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// **A client opens the audio editor through the door and is its
    /// handler**: the open answers the window once it opened, a member's
    /// verb and a query are answered, a save that renders is answered when
    /// it lands, to the client that asked, and the client is told when the
    /// window goes.
    #[test]
    fn a_client_opens_and_calls_the_audio_editor_through_the_door() {
        use std::net::{Ipv4Addr, SocketAddr};

        let path = take("door", 24_000);
        let out =
            std::env::temp_dir().join(format!("host-audio-door-out-{}.wav", std::process::id()));
        let mut host = host();
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_999)));
        let options = json!({"path": path, "title": "take"}).to_string();
        let first = door(
            &mut host,
            GUI_APP_OPEN,
            vec![
                OscType::Int(1),
                OscType::String("audio".into()),
                OscType::String(options),
            ],
            client,
        );
        let opened = answered(&mut host, first, 1);
        let window = opened["ok"]["window"].as_i64().expect("a window") as i32;
        assert!(host.owns_audio(window));
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(window).is_some()
        });
        settled(&mut host, window);
        let call = |host: &mut Host, seq: i32, call: Value| {
            let first = door(
                host,
                GUI_APP_CALL,
                vec![
                    OscType::Int(seq),
                    OscType::Int(window),
                    OscType::String(call.to_string()),
                ],
                client,
            );
            answered(host, first, seq)
        };
        // a member's verb
        let file = call(&mut host, 2, json!({"member": {"verb": "file"}}));
        assert_eq!(file["ok"]["path"], json!(path), "{file}");
        assert_eq!(file["ok"]["frames"], 24_000, "{file}");
        // a query of its state
        let state = call(&mut host, 3, json!({"query": "transport"}));
        assert!(state["ok"]["transport"].is_u64(), "{state}");
        let history = call(&mut host, 30, json!({"query": "history"}));
        assert_eq!(history["ok"]["canUndo"], false, "{history}");
        let actions = call(&mut host, 31, json!({"query": "actions"}));
        assert!(actions["ok"].is_object(), "{actions}");
        // an action that renders, answered when it lands, to the client
        let save = json!({"do": {"verb": "save",
            "request": {"path": out.to_string_lossy(), "format": "int16"}}});
        let answer = call(&mut host, 4, save);
        assert_eq!(
            answer["ok"]["destination"]["path"],
            json!(out.to_string_lossy()),
            "{answer}"
        );
        // an edit undone through the door, as the handler's undo does
        let widget = widget_of(&host, window);
        let delete = host.event_message(
            widget,
            7,
            vec![
                OscType::String("delete".into()),
                OscType::Float(0.0),
                OscType::Float(12_000.0),
            ],
        );
        assert!(host.deliver(window, &delete));
        assert_eq!(parts(&mut host, window)[0]["frames"], 12_000);
        let history = call(&mut host, 40, json!({"query": "history"}));
        assert_eq!(history["ok"]["canUndo"], true, "{history}");
        let version = history["ok"]["version"].clone();
        let undo = json!({"do": {"verb": "event", "addr": "/gui_event",
            "args": [window, 0, version, "undo"]}});
        let undone = call(&mut host, 41, undo);
        assert!(undone["error"].is_null(), "{undone}");
        assert_eq!(parts(&mut host, window)[0]["frames"], 24_000, "undone");
        // what it cannot take is an error of its kind
        let wrong = call(&mut host, 5, json!({"nothing": 1}));
        assert_eq!(wrong["error"]["type"], "value", "{wrong}");
        // and the client is told when the window goes
        host.close_audio(window);
        let closed = host.take_effects().into_iter().any(|e| {
            matches!(&e, HostEffect::Send { to, msg }
                if *to == client && msg.addr == GUI_CLOSED
                    && msg.args.first() == Some(&OscType::Int(window)))
        });
        assert!(closed, "the opener heard the window close");
        let gone = call(&mut host, 6, json!({"query": "transport"}));
        assert_eq!(gone["error"]["type"], "runtime", "{gone}");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out);
    }

    /// **A hand in a multitrack window is told its opener**
    /// (`/gui_app_changed`), and the opener's own call is not.
    #[test]
    fn a_hand_in_a_multitrack_is_told_its_opener() {
        use std::net::{Ipv4Addr, SocketAddr};

        let mut host = host();
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_996)));
        let multitrack = json!({"tracks": [{"id": 1, "name": "a", "takeLanes": [{"id": 2}]}]});
        let options = json!({"multitrack": multitrack, "title": "untitled"}).to_string();
        let first = door(
            &mut host,
            GUI_APP_OPEN,
            vec![
                OscType::Int(1),
                OscType::String("multitrack".into()),
                OscType::String(options),
            ],
            client,
        );
        let opened = answered(&mut host, first, 1);
        let window = opened["ok"]["window"].as_i64().expect("a window") as i32;
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(window).is_some()
        });
        let told = |effects: Vec<HostEffect>| {
            effects.into_iter().find_map(|e| match e {
                HostEffect::Send { to, msg } if to == client && msg.addr == GUI_APP_CHANGED => {
                    Some(msg.args)
                }
                _ => None,
            })
        };
        // the client's own edit is not told back: it knows it
        let state = ask(
            &mut host,
            window,
            json!({"do": {"verb": "handle", "call": {"verb": "state"}}}),
        );
        let mut tracks = state["tracks"].clone();
        tracks[0]["name"] = json!("b");
        let _rx = host.ask_audio(
            window,
            json!({"do": {"verb": "handle", "call": {"verb": "apply",
            "intent": {"intent": "settracks", "tracks": tracks}}}}),
        );
        let mut heard = None;
        let t = std::time::Instant::now();
        while t.elapsed() < std::time::Duration::from_millis(300) {
            heard = heard.or(told(host.take_effects()));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(heard.is_none(), "its own edit was told: {heard:?}");
        // an undo in the window is a hand's, and is told
        let undo = host.event_message(window, 1, vec![OscType::String("undo".into())]);
        assert!(host.deliver(window, &undo));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let args = loop {
            if let Some(args) = told(host.take_effects()) {
                break args;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the opener never heard the hand"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(args[0], OscType::Int(window));
        assert!(matches!(args[1], OscType::Long(_)), "{args:?}");
        host.close_audio(window);
    }

    /// **A multitrack a client hands over is told back when its window
    /// goes**: the `/gui_closed` its opener hears carries the multitrack as
    /// the window left it, which the script holds on with.
    #[test]
    fn a_closed_multitrack_is_told_back_to_its_opener() {
        use std::net::{Ipv4Addr, SocketAddr};

        let mut host = host();
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_997)));
        let multitrack = json!({"tracks": [{"id": 1, "name": "a", "takeLanes": [{"id": 2}]}]});
        let options = json!({"multitrack": multitrack, "title": "untitled"}).to_string();
        let first = door(
            &mut host,
            GUI_APP_OPEN,
            vec![
                OscType::Int(1),
                OscType::String("multitrack".into()),
                OscType::String(options),
            ],
            client,
        );
        let opened = answered(&mut host, first, 1);
        let window = opened["ok"]["window"].as_i64().expect("a window") as i32;
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(window).is_some()
        });
        let state = ask(
            &mut host,
            window,
            json!({"do": {"verb": "handle",
            "call": {"verb": "state"}}}),
        );
        let mut tracks = state["tracks"].clone();
        tracks[0]["name"] = json!("b");
        ask(
            &mut host,
            window,
            json!({"do": {"verb": "handle", "call": {"verb": "apply",
            "intent": {"intent": "settracks", "tracks": tracks}}}}),
        );
        // the effects are read here, not by `ask`, which drops what it ticks
        let _rx = host.ask_audio(window, json!({"do": {"verb": "close"}}));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let told = loop {
            let told = host.take_effects().into_iter().find_map(|e| match e {
                HostEffect::Send { to, msg } if to == client && msg.addr == GUI_CLOSED => {
                    Some(msg.args)
                }
                _ => None,
            });
            if let Some(told) = told {
                break told;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the opener never heard the close"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(told[0], OscType::Int(window));
        let Some(OscType::String(last)) = told.get(1) else {
            panic!("the close carries no multitrack: {told:?}");
        };
        let last: Value = serde_json::from_str(last).unwrap();
        assert_eq!(last["tracks"][0]["name"], "b", "{last}");
        let Some(OscType::String(snapshot)) = told.get(2) else {
            panic!("the close carries no snapshot: {told:?}");
        };
        let snapshot: Value = serde_json::from_str(snapshot).unwrap();
        assert_eq!(snapshot["session"]["multitrack"], last);
    }

    /// **A window that lets go of the session a client handed it hands it
    /// back**: another session in its place tells the opener what the
    /// window left the first as, and the window is the host's own from
    /// then on, so its close hands the opener nothing.
    #[test]
    fn a_released_multitrack_is_handed_back_and_its_close_carries_nothing() {
        use std::net::{Ipv4Addr, SocketAddr};

        let mut host = host();
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_996)));
        let multitrack = json!({"tracks": [{"id": 1, "name": "a", "takeLanes": [{"id": 2}]}]});
        let options = json!({"multitrack": multitrack, "title": "untitled"}).to_string();
        let first = door(
            &mut host,
            GUI_APP_OPEN,
            vec![
                OscType::Int(1),
                OscType::String("multitrack".into()),
                OscType::String(options),
            ],
            client,
        );
        let window = answered(&mut host, first, 1)["ok"]["window"]
            .as_i64()
            .expect("a window") as i32;
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(window).is_some()
        });
        let _rx = host.ask_audio(window, json!({"do": {"verb": "new_file"}}));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let released = loop {
            let told = host.take_effects().into_iter().find_map(|e| match e {
                HostEffect::Send { to, msg } if to == client && msg.addr == GUI_APP_RELEASED => {
                    Some(msg.args)
                }
                _ => None,
            });
            if let Some(told) = told {
                break told;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the opener was never handed its session"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(released[0], OscType::Int(window));
        let Some(OscType::String(snapshot)) = released.get(1) else {
            panic!("the release carries no snapshot: {released:?}");
        };
        let snapshot: Value = serde_json::from_str(snapshot).unwrap();
        assert_eq!(
            snapshot["session"]["multitrack"]["tracks"][0]["name"], "a",
            "{snapshot}"
        );
        assert_eq!(
            ask(&mut host, window, json!({"query":"file"}))["empty"],
            false
        );
        let _rx = host.ask_audio(window, json!({"do": {"verb": "close"}}));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let closed = loop {
            let told = host.take_effects().into_iter().find_map(|e| match e {
                HostEffect::Send { to, msg } if to == client && msg.addr == GUI_CLOSED => {
                    Some(msg.args)
                }
                _ => None,
            });
            if let Some(told) = told {
                break told;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the opener never heard the close"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(
            closed,
            vec![OscType::Int(window)],
            "the session was the host's own"
        );
    }

    /// **The audio editor's switches are checks in its menus**, and its
    /// choices one of a group, each kept in step with the state it reads:
    /// the file list and the loop a check, the views and the body the hand
    /// edits a radio, and a switch turned from a script reads turned.
    #[test]
    fn the_audio_editors_switches_read_as_checks_in_its_menus() {
        use crate::host::menu::{self, Entry, EntryKind};

        fn find<'a>(entries: &'a [Entry], verb: &str) -> Option<&'a Entry> {
            entries.iter().find_map(|entry| match &entry.kind {
                EntryKind::Submenu(children) => find(children, verb),
                _ => (entry.verb.as_deref() == Some(verb)).then_some(entry),
            })
        }
        let path = take("checks", 4_800);
        let mut host = host();
        let window = opened(&mut host, &path);
        let marked = |host: &Host| {
            let info = host.window_def(window).unwrap().info();
            let menu = info.iter().find(|(k, _)| k == "menu").unwrap().1.clone();
            let menu: Value = serde_json::from_str(menu.as_str().unwrap()).unwrap();
            let mut entries = menu::parse(&menu);
            host.action_entries(window, &mut entries);
            entries
        };
        let kind = |entries: &[Entry], verb: &str| {
            find(entries, verb)
                .unwrap_or_else(|| panic!("no {verb} in the menus"))
                .kind
                .clone()
        };
        let entries = marked(&host);
        assert_eq!(kind(&entries, "toggle_files"), EntryKind::Check(true));
        assert_eq!(
            kind(&entries, "spectrogram_settings"),
            EntryKind::Check(false)
        );
        assert_eq!(kind(&entries, "loop"), EntryKind::Check(false));
        let radio = |group: &str, on: bool| EntryKind::Radio {
            group: group.into(),
            on,
        };
        assert_eq!(kind(&entries, "view_wave"), radio("views", true));
        assert_eq!(kind(&entries, "view_both"), radio("views", false));
        assert_eq!(kind(&entries, "layer_take"), radio("layer", true));
        assert_eq!(kind(&entries, "show_envelope"), radio("layer", false));
        for verb in ["hide_files", "view_both"] {
            ask(
                &mut host,
                window,
                json!({"do": {"verb": "verb", "call": {"verb": verb}}}),
            );
        }
        let entries = marked(&host);
        assert_eq!(kind(&entries, "toggle_files"), EntryKind::Check(false));
        assert_eq!(kind(&entries, "view_wave"), radio("views", false));
        assert_eq!(kind(&entries, "view_both"), radio("views", true));
        let _ = std::fs::remove_file(&path);
    }

    /// **A sound file open in a window is not opened twice**: a client's
    /// open of it is answered that window, which comes forward with the file
    /// in focus, and the window's own open of it focuses it in its list.
    #[test]
    fn a_sound_file_already_open_is_focused_in_its_window() {
        use std::net::{Ipv4Addr, SocketAddr};

        let a = take("held-a", 4_800);
        let b = take("held-b", 4_800);
        let mut host = host();
        let window = opened(&mut host, &a);
        let focused = |host: &mut Host| ask(host, window, json!({"member": {"verb": "file"}}));
        // a second file read into the window, which takes the focus
        ask(
            &mut host,
            window,
            json!({"do": {"verb": "openFile", "paths": [b.clone()]}}),
        );
        settled(&mut host, window);
        assert_eq!(focused(&mut host)["path"], json!(b));
        // the first opened again from the window: focused, not read twice
        ask(
            &mut host,
            window,
            json!({"do": {"verb": "openFile", "paths": [a.clone()]}}),
        );
        assert_eq!(focused(&mut host)["path"], json!(a));
        let files = ask(&mut host, window, json!({"member": {"verb": "files"}}));
        assert_eq!(files["files"].as_array().map(Vec::len), Some(2), "{files}");
        ask(
            &mut host,
            window,
            json!({"do": {"verb": "openFile", "paths": [b.clone()]}}),
        );
        // a client's open of the first: that window, forward, the file in focus
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_992)));
        let options = json!({"path": a}).to_string();
        let mut seen = door(
            &mut host,
            GUI_APP_OPEN,
            vec![
                OscType::Int(1),
                OscType::String("audio".into()),
                OscType::String(options),
            ],
            client,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while reply_of(&seen, 1).is_none() {
            assert!(std::time::Instant::now() < deadline, "never answered");
            std::thread::sleep(std::time::Duration::from_millis(5));
            seen.extend(host.take_effects());
        }
        assert_eq!(reply_of(&seen, 1).unwrap()["ok"]["window"], window);
        assert!(
            seen.iter()
                .any(|e| matches!(e, HostEffect::Raise(id) if *id == window)),
            "the window was not brought forward"
        );
        until(&mut host, "the file was never focused", |host| {
            focused(host)["path"] == json!(a)
        });
        assert_eq!(host.audio.as_ref().unwrap().windows.len(), 1);
        // another window's File > Open of the second: it opens nothing, and
        // the window holding it comes forward with it in focus
        let other = host.open_audio(None, "empty").expect("opened");
        until(&mut host, "the second window was never drawn", |h| {
            h.window_def(other).is_some()
        });
        let rx = host.ask_audio(
            other,
            json!({"do": {"verb": "openFile", "paths": [b.clone()]}}),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let (mut raised, mut ended) = (false, false);
        while !(raised && ended) {
            assert!(
                std::time::Instant::now() < deadline,
                "raised {raised}, ended {ended}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
            raised |= host
                .take_effects()
                .iter()
                .any(|e| matches!(e, HostEffect::Raise(id) if *id == window));
            ended |= rx.try_recv().is_ok();
        }
        assert_eq!(focused(&mut host)["path"], json!(b));
        assert_eq!(
            ask(&mut host, other, json!({"member": {"verb": "file"}}))["empty"],
            true,
            "the second window opened nothing"
        );
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    /// **A session open in a window is not opened twice**: a client that
    /// asks for it again is answered that window, which comes forward and
    /// is its from then on when no client held it; another client is told
    /// where it is open, and the host's own open brings it forward alone.
    #[test]
    fn a_session_already_open_brings_its_window_forward() {
        use clausters_document::Document;
        use clausters_document::session::Session;
        use std::net::{Ipv4Addr, SocketAddr};

        let folder = std::env::temp_dir().join(format!("host-mt-held-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("song.json");
        std::fs::write(
            &path,
            serde_json::to_string(&Session::new(Document::empty())).unwrap(),
        )
        .unwrap();
        let path = path.to_str().unwrap().to_string();
        let mut host = host();
        let window = host.open_multitrack(Some(&path), "song").unwrap();
        until(&mut host, "the window never opened its session", |host| {
            host.window_def(window).is_some()
        });
        until(&mut host, "the session was never opened", |host| {
            ask(host, window, json!({"query": "file"}))["path"] == json!(path)
        });
        // **every effect until the door answers `seq`**
        let effects = |host: &mut Host, first: Vec<HostEffect>, seq: i32| {
            let mut seen = first;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while reply_of(&seen, seq).is_none() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the door never answered"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
                seen.extend(host.take_effects());
            }
            let answer = reply_of(&seen, seq).unwrap();
            let raised = seen
                .iter()
                .any(|e| matches!(e, HostEffect::Raise(id) if *id == window));
            (answer, raised)
        };
        let options = json!({"path": format!("{}/./song.json", folder.display())}).to_string();
        let open = |host: &mut Host, seq: i32, client: ClientId| {
            door(
                host,
                GUI_APP_OPEN,
                vec![
                    OscType::Int(seq),
                    OscType::String("multitrack".into()),
                    OscType::String(options.clone()),
                ],
                client,
            )
        };
        let first = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_994)));
        let sent = open(&mut host, 1, first);
        let (answer, raised) = effects(&mut host, sent, 1);
        assert_eq!(answer["ok"]["window"], window, "{answer}");
        assert!(raised, "the window was not brought forward");
        let second = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_993)));
        let sent = open(&mut host, 2, second);
        let (answer, raised) = effects(&mut host, sent, 2);
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|why| why.contains(&format!("window {window}"))),
            "{answer}"
        );
        assert!(raised);
        // the host's own open: nothing opens, the window comes forward
        let again = host.open_multitrack(Some(&path), "song").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !host
            .take_effects()
            .iter()
            .any(|e| matches!(e, HostEffect::Raise(id) if *id == window))
        {
            assert!(
                std::time::Instant::now() < deadline,
                "never brought forward"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!host.owns_audio(again));
        // the client took it over: it is told the window's close
        host.closed_by_user(window);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !host.take_effects().iter().any(
            |e| matches!(e, HostEffect::Send { to, msg } if *to == first && msg.addr == GUI_CLOSED),
        ) {
            assert!(
                std::time::Instant::now() < deadline,
                "the client never heard the close"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// **With no engraver a score is refused, and says why**, and no window
    /// is left behind.
    #[cfg(all(feature = "notation", not(feature = "score")))]
    #[test]
    fn a_host_with_no_engraver_refuses_a_score_and_says_why() {
        use std::net::{Ipv4Addr, SocketAddr};

        let mut host = host();
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_992)));
        let first = door(
            &mut host,
            GUI_APP_OPEN,
            vec![
                OscType::Int(1),
                OscType::String("score".into()),
                OscType::String(json!({"title": "s"}).to_string()),
            ],
            client,
        );
        let answer = answered(&mut host, first, 1);
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|why| why.contains("engraver")),
            "{answer}"
        );
        assert!(!host.owns_an_application());
    }

    /// A window a client opened through the door, drawn.
    fn opened_app(host: &mut Host, client: ClientId, seq: i32, app: &str, options: Value) -> i32 {
        let first = door(
            host,
            GUI_APP_OPEN,
            vec![
                OscType::Int(seq),
                OscType::String(app.into()),
                OscType::String(options.to_string()),
            ],
            client,
        );
        let answer = answered(host, first, seq);
        let window = answer["ok"]["window"].as_i64().expect("a window") as i32;
        until(host, "the window was never drawn", |h| {
            h.window_def(window).is_some()
        });
        window
    }

    /// A sequence of two notes, as a client hands one over.
    fn two_notes() -> Value {
        json!({"events": [
            {"at": 0.0, "id": 1, "data": {"midinote": 60, "dur": 1.0}},
            {"at": 1.0, "id": 2, "data": {"midinote": 64, "dur": 1.0}},
        ]})
    }

    /// **A roll is an application of this host's**: a client hands its
    /// sequence over, a script's change through the handle is an entry of the
    /// window's history, two rolls over it walk one order on a transport each,
    /// and the close hands the sequence back as it was left.
    #[test]
    fn a_client_opens_a_roll_in_the_hosts_application() {
        use std::net::{Ipv4Addr, SocketAddr};

        let mut host = host();
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_990)));
        let keys = opened_app(
            &mut host,
            client,
            1,
            "notes",
            json!({"sequence": two_notes(), "title": "keys"}),
        );
        let hertz = opened_app(
            &mut host,
            client,
            2,
            "notes",
            json!({"over": keys, "domain": "hz", "title": "hertz"}),
        );
        let one = ask(&mut host, keys, json!({"query": "transport"}));
        let other = ask(&mut host, hertz, json!({"query": "transport"}));
        assert_ne!(one["transport"], other["transport"], "a transport each");
        // a script's change: applied, then recorded under its label
        let applied = ask(
            &mut host,
            keys,
            json!({"do": {"verb": "handle", "call": {"verb": "apply", "inverse": true,
                "intent": {"intent": "remove", "id": 2}}}}),
        );
        assert_eq!(applied["applied"], true, "{applied}");
        let recorded = ask(
            &mut host,
            keys,
            json!({"do": {"verb": "handle", "label": "take it out", "call": {"verb": "record",
                "legs": [{"forward": {"intent": "remove", "id": 2},
                    "backward": applied["current"]}]}}}),
        );
        assert_eq!(recorded, true);
        // and the window was told, so a gesture after it is not overtaken
        host.take_effects();
        let at = ask(&mut host, keys, json!({"query": "history"}))["version"].clone();
        assert_eq!(json!(host.outbox.borrow().version()), at);
        // the other roll reads it, and its history is the same one
        let len = ask(
            &mut host,
            hertz,
            json!({"do": {"verb": "handle", "call": {"verb": "len"}}}),
        );
        assert_eq!(len["len"], 1, "{len}");
        let history = ask(&mut host, hertz, json!({"query": "history"}));
        assert_eq!(history["undoLabel"], "take it out", "{history}");
        assert_eq!(ask(&mut host, hertz, json!({"do": {"verb": "undo"}})), true);
        let len = ask(
            &mut host,
            keys,
            json!({"do": {"verb": "handle", "call": {"verb": "len"}}}),
        );
        assert_eq!(len["len"], 2, "{len}");
        // **another sequence is another order**: its window walks nothing of
        // these two's
        let other = opened_app(
            &mut host,
            client,
            3,
            "notes",
            json!({"sequence": two_notes(), "title": "another"}),
        );
        assert_eq!(
            ask(&mut host, other, json!({"query": "history"}))["canUndo"],
            false
        );
        assert_eq!(
            ask(&mut host, other, json!({"do": {"verb": "undo"}})),
            false
        );
        // the transport, as a script plays it
        ask(
            &mut host,
            keys,
            json!({"do": {"verb": "play", "beat": 0.5}}),
        );
        assert_eq!(
            ask(&mut host, keys, json!({"do": {"verb": "playing"}})),
            true
        );
        assert_eq!(
            ask(&mut host, hertz, json!({"do": {"verb": "playing"}})),
            false
        );
        ask(&mut host, keys, json!({"do": {"verb": "stop"}}));
        // what the hand holds is each window's own
        let called = |host: &mut Host, seq: i32, window: i32, call: Value| {
            let first = door(
                host,
                GUI_APP_CALL,
                vec![
                    OscType::Int(seq),
                    OscType::Int(window),
                    OscType::String(call.to_string()),
                ],
                client,
            );
            answered(host, first, seq)
        };
        called(
            &mut host,
            10,
            keys,
            json!({"do": {"verb": "select", "regions": [1]}}),
        );
        let held = called(&mut host, 11, keys, json!({"query": "selected"}));
        assert_eq!(held["ok"]["selected"], json!([1]), "{held}");
        let held = called(&mut host, 12, hertz, json!({"query": "selected"}));
        assert_eq!(held["ok"]["selected"], json!([]), "{held}");
        // the close hands the sequence back
        let closed = loop_closed(&mut host, client, keys);
        let left: Value = serde_json::from_str(&closed).unwrap();
        assert_eq!(
            left["sequence"]["events"].as_array().map(Vec::len),
            Some(2),
            "{left}"
        );
        assert!(host.owns_audio(hertz), "the other window goes on");
    }

    /// Closes window `window` by its door and answers what its opener is
    /// handed on `/gui_closed`.
    fn loop_closed(host: &mut Host, client: ClientId, window: i32) -> String {
        let first = door(
            host,
            GUI_APP_CALL,
            vec![
                OscType::Int(99),
                OscType::Int(window),
                OscType::String(json!({"do": {"verb": "close"}}).to_string()),
            ],
            client,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut effects = first;
        loop {
            for effect in std::mem::take(&mut effects) {
                if let HostEffect::Send { msg, .. } = effect
                    && msg.addr == GUI_CLOSED
                    && msg.args.first() == Some(&OscType::Int(window))
                    && let Some(OscType::String(left)) = msg.args.get(1)
                {
                    return left.clone();
                }
            }
            assert!(std::time::Instant::now() < deadline, "never told it closed");
            std::thread::sleep(std::time::Duration::from_millis(5));
            effects = host.take_effects();
        }
    }

    /// **`--score`'s window saves where the host said**, and the file it read
    /// is not written over: without a destination a Save asks where.
    #[cfg(feature = "score")]
    #[test]
    fn a_score_the_host_opens_saves_where_it_was_told() {
        let folder = std::env::temp_dir().join(format!("host-own-score-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let (read, out) = (folder.join("read.abc"), folder.join("out.mei"));
        std::fs::write(&read, "X:1\nM:4/4\nL:1/4\nK:C\nC D E F|\n").unwrap();
        let mut host = host();
        let told = host
            .open_score_saving_to(read.to_str(), out.to_str(), "told")
            .unwrap();
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(told).is_some()
        });
        let saved = ask(&mut host, told, json!({"do": {"verb": "save"}}));
        assert_eq!(saved["destination"], out.to_str().unwrap(), "{saved}");
        assert!(std::fs::read_to_string(&out).unwrap().contains("<mei"));
        let before = std::fs::read_to_string(&read).unwrap();
        ask(&mut host, told, json!({"do": {"verb": "close"}}));
        let alone = host
            .open_score_saving_to(read.to_str(), None, "alone")
            .unwrap();
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(alone).is_some()
        });
        let refused = ask(&mut host, alone, json!({"do": {"verb": "save"}}));
        assert!(refused["raised"].is_string(), "{refused}");
        assert_eq!(std::fs::read_to_string(&read).unwrap(), before);
    }

    /// **A page over a sequence and a roll over it are one order**, through
    /// the door: the page is opened beside the roll, an edit in either walks
    /// back from the other, each with its own conversation and transport, and
    /// a script reads the page's document where the window has it.
    #[cfg(feature = "score")]
    #[test]
    fn a_roll_and_a_page_a_client_opens_are_one_order() {
        use std::net::{Ipv4Addr, SocketAddr};

        let mut host = host();
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_989)));
        let roll = opened_app(
            &mut host,
            client,
            1,
            "notes",
            json!({"sequence": two_notes(), "title": "roll"}),
        );
        let page = opened_app(
            &mut host,
            client,
            2,
            "score",
            json!({"over": roll, "title": "page", "how": {"division": 8}}),
        );
        let mei = ask(
            &mut host,
            page,
            json!({"do": {"verb": "page", "call": {"verb": "mei"}}}),
        );
        assert!(
            mei["mei"].as_str().is_some_and(|m| m.contains("<mei")),
            "{mei}"
        );
        assert_ne!(
            ask(&mut host, roll, json!({"query": "transport"}))["transport"],
            ask(&mut host, page, json!({"query": "transport"}))["transport"],
        );
        let applied = ask(
            &mut host,
            page,
            json!({"do": {"verb": "handle", "call": {"verb": "apply", "inverse": true,
                "intent": {"intent": "remove", "id": 2}}}}),
        );
        ask(
            &mut host,
            page,
            json!({"do": {"verb": "handle", "label": "out", "call": {"verb": "record",
                "legs": [{"forward": {"intent": "remove", "id": 2},
                    "backward": applied["current"]}]}}}),
        );
        assert_eq!(
            ask(&mut host, roll, json!({"query": "history"}))["undoLabel"],
            "out"
        );
        assert_eq!(ask(&mut host, roll, json!({"do": {"verb": "undo"}})), true);
        let len = ask(
            &mut host,
            page,
            json!({"do": {"verb": "handle", "call": {"verb": "len"}}}),
        );
        assert_eq!(len["len"], 2, "{len}");
        let closed = loop_closed(&mut host, client, page);
        let left: Value = serde_json::from_str(&closed).unwrap();
        assert!(left["mei"].is_string(), "{left}");
        assert_eq!(left["sequence"]["events"].as_array().map(Vec::len), Some(2));
    }

    /// **Close file lets the document go, and the window stays**: the client
    /// is handed what the window left, the file it had is nobody's to bring
    /// forward, a verb is refused with why, and a later document is the
    /// host's own.
    #[cfg(feature = "score")]
    #[test]
    fn a_score_window_lets_its_document_go_and_stays() {
        use std::net::{Ipv4Addr, SocketAddr};

        let folder = std::env::temp_dir().join(format!("host-score-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("kept.mei");
        let mut host = host();
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_992)));
        let first = door(
            &mut host,
            GUI_APP_OPEN,
            vec![
                OscType::Int(1),
                OscType::String("score".into()),
                OscType::String(json!({"title": "score"}).to_string()),
            ],
            client,
        );
        let window = answered(&mut host, first, 1)["ok"]["window"]
            .as_i64()
            .expect("a window") as i32;
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(window).is_some()
        });
        ask(&mut host, window, json!({"do": {"verb": "new_file"}}));
        let saved = ask(
            &mut host,
            window,
            json!({"do": {"verb": "save", "path": path.to_str().unwrap()}}),
        );
        assert_eq!(saved["destination"], path.to_str().unwrap(), "{saved}");
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("<mei"), "{written}");
        let page = json!({"do": {"verb": "page", "call": {"verb": "mei"}}});
        assert_eq!(ask(&mut host, window, page.clone())["mei"], json!(written));
        let sheet = json!({"do": {"verb": "page", "call": {"verb": "sheet"}}});
        let contents = ask(&mut host, window, sheet.clone());
        assert!(contents["sheet"]["staves"].is_array(), "{contents}");
        ask(&mut host, window, json!({"do": {"verb": "close_file"}}));
        assert_eq!(
            ask(&mut host, window, json!({"query": "file"}))["empty"],
            true
        );
        assert!(host.window_def(window).is_some(), "the window stays");
        let refused = ask(
            &mut host,
            window,
            json!({"do": {"verb": "page", "call": {"verb": "mei"}}}),
        );
        assert_eq!(refused["why"], "no file is open", "{refused}");
        // nobody holds the file now: a read takes it, and it is the same
        ask(
            &mut host,
            window,
            json!({"do": {"verb": "open_file", "path": path.to_str().unwrap()}}),
        );
        let file = ask(&mut host, window, json!({"query": "file"}));
        assert_eq!(file["path"], path.to_str().unwrap(), "{file}");
        assert_eq!(file["dirty"], false, "{file}");
        // what the engraver mints for itself aside, it is what was written
        assert_eq!(ask(&mut host, window, sheet), contents);
        assert_eq!(
            ask(&mut host, window, json!({"query": "history"}))["canUndo"],
            false
        );
    }

    /// **An audio window asks only where no client holds its file**: the
    /// host's own carries `ask_close`, and a client's goes at once.
    #[test]
    fn an_audio_window_asks_only_where_no_client_holds_it() {
        use std::net::{Ipv4Addr, SocketAddr};

        let mut host = host();
        let own = host.open_audio(None, "alone").expect("opened");
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(own).is_some()
        });
        assert!(!host.close_request(own), "its owner is asked first");
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_993)));
        let asked = door(
            &mut host,
            GUI_APP_OPEN,
            vec![
                OscType::Int(1),
                OscType::String("audio".into()),
                OscType::String(json!({"title": "client"}).to_string()),
            ],
            client,
        );
        let window = answered(&mut host, asked, 1)["ok"]["window"]
            .as_i64()
            .expect("a window") as i32;
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(window).is_some()
        });
        assert!(host.close_request(window), "it goes at once");
    }

    /// **The audio window's close form, the whole way through this host**: its
    /// Save renders over the opened file, and only the confirmed write lets the
    /// window go. A write that fails leaves the window, its work and the file on
    /// disk as they were, and Cancel leaves them too.
    #[test]
    fn an_audio_window_s_close_form_waits_for_the_write_in_the_host() {
        use std::os::unix::fs::PermissionsExt;

        let folder = std::env::temp_dir().join(format!("host-audio-close-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("take.wav");
        let source = take("close-form", 24_000);
        std::fs::copy(&source, &file).unwrap();
        let path = file.to_string_lossy().into_owned();
        let frames = |path: &str| clausters_core::pcm::Layout::open(path).unwrap().frames;

        let mut host = host();
        let def_id = opened(&mut host, &path);
        let widget = widget_of(&host, def_id);
        let structure = host.audio.as_ref().unwrap().windows[&def_id].structure;
        let id = |host: &Host, name: &str| host.own_widget_id(structure, AUDIO, name).unwrap();
        let stack = id(&host, "dialog:stack");
        let page = |host: &Host| match host.window_defs[&def_id].find(stack).map(|w| &w.kind) {
            Some(crate::host::widget::WidgetKind::Stack { index, .. }) => *index,
            other => panic!("a stack: {other:?}"),
        };
        let delete = host.event_message(
            widget,
            1,
            vec![
                OscType::String("delete".into()),
                OscType::Float(0.0),
                OscType::Float(12_000.0),
            ],
        );
        assert!(host.deliver(def_id, &delete));
        until(&mut host, "the edit never landed", |h| {
            ask(h, def_id, json!({"member": {"verb": "binding"}}))["dirty"] == true
        });
        let close = |host: &mut Host, seq: i32| {
            let close = host.event_message(def_id, seq, vec![OscType::String("close".into())]);
            assert!(host.deliver(def_id, &close));
        };
        let click = |host: &mut Host, name: &str, seq: i32| {
            let button = id(host, name);
            let click = host.event_message(button, seq, vec![OscType::String("click".into())]);
            assert!(host.deliver(def_id, &click));
        };
        let up = 4;

        // Cancel keeps it all
        close(&mut host, 2);
        until(&mut host, "the close form never came up", |h| page(h) == up);
        click(&mut host, "dialog:close:cancel", 3);
        until(&mut host, "Cancel never took the form down", |h| {
            page(h) == 0
        });
        assert!(host.window_def(def_id).is_some());

        // a write that fails: the form went down with the answer
        close(&mut host, 4);
        until(&mut host, "the close form never came up", |h| page(h) == up);
        let read_only = std::fs::Permissions::from_mode(0o555);
        std::fs::set_permissions(&folder, read_only).unwrap();
        click(&mut host, "dialog:close:ok", 5);
        until(&mut host, "the form never went down", |h| page(h) == 0);
        settled(&mut host, def_id);
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(host.window_def(def_id).is_some(), "the window stays");
        assert_eq!(frames(&path), 24_000, "the file is as it was");
        assert_eq!(
            ask(&mut host, def_id, json!({"member": {"verb": "binding"}}))["dirty"],
            true
        );

        // a write that lands lets the window go, the work in the file
        close(&mut host, 6);
        until(&mut host, "the close form never came up", |h| page(h) == up);
        click(&mut host, "dialog:close:ok", 7);
        until(&mut host, "the window never went", |h| {
            h.window_def(def_id).is_none()
        });
        assert_eq!(frames(&path), 12_000, "the work is in the file");
        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::remove_file(&source);
    }

    /// **A play asks for the window's frames before the engine has taken
    /// it.** The run that plays ends, and the window is told, before the
    /// engine's next block takes the command, so the transport still reads
    /// stopped when the front asks what the window needs. A front that found
    /// nothing to tick for slept, and the play cursor stood still for seconds
    /// while the pass sounded, until another event happened to wake it. The
    /// window keeps the tick for a settling after its application answers,
    /// as a playhead over the server's transport does after a push, and a
    /// play that never lands gives it back.
    #[test]
    fn a_play_keeps_the_tick_until_the_engine_takes_it() {
        use std::net::{Ipv4Addr, SocketAddr};

        // an engine nothing processes: the play is never taken
        let mut host = host();
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_989)));
        let roll = opened_app(
            &mut host,
            client,
            1,
            "notes",
            json!({"sequence": two_notes(), "title": "keys"}),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !host.tree_demand(roll, true).is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "the roll never settled"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        ask(&mut host, roll, json!({"do": {"verb": "play"}}));
        assert_eq!(
            host.tree_demand(roll, true),
            vec![crate::host::activity::Reason::Playhead],
            "the play is asked for before the engine reads it rolling"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !host.tree_demand(roll, true).is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "a play that never lands kept the tick"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// **A stopped window of any application costs nothing**: with a client's
    /// audio, multitrack, roll and symbolic score windows open and stopped, none
    /// asks for a tick and the application has nothing that may land unasked. A
    /// pass asks only where it plays, a stop gives that back, and a window closed
    /// while it plays lets its transport and its demand go.
    #[test]
    fn a_stopped_or_closed_window_of_any_application_asks_for_nothing() {
        use std::net::{Ipv4Addr, SocketAddr};

        let path = take("quiet", 24_000);
        let (engine, mut processed) = AppEngine::detached(48_000.0, 2);
        let engine = Arc::new(Mutex::new(engine));
        let mut host = Host::new();
        host.hold_audio_engine(Arc::clone(&engine));
        // the device a detached engine has not got
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let device = {
            let running = Arc::clone(&running);
            std::thread::spawn(move || {
                let mut out = vec![0.0f32; clausters_app_engine::BLOCK_SIZE * 2];
                while running.load(std::sync::atomic::Ordering::Relaxed) {
                    processed.process_block(&mut out);
                    std::thread::sleep(std::time::Duration::from_micros(200));
                }
            })
        };
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_994)));
        let mut windows = vec![
            ("audio", opened(&mut host, &path)),
            (
                "multitrack",
                opened_app(
                    &mut host,
                    client,
                    1,
                    "multitrack",
                    json!({"multitrack": {}, "title": "session"}),
                ),
            ),
            (
                "roll",
                opened_app(
                    &mut host,
                    client,
                    2,
                    "notes",
                    json!({"sequence": two_notes(), "title": "keys"}),
                ),
            ),
        ];
        #[cfg(feature = "score")]
        windows.push((
            "score",
            opened_app(&mut host, client, 3, "score", json!({"title": "page"})),
        ));
        #[cfg(feature = "score")]
        {
            let score = windows[3].1;
            ask(&mut host, score, json!({"do": {"verb": "new_file"}}));
        }
        let quiet = |host: &Host, window: i32| host.tree_demand(window, true).is_empty();
        let busy = |_: &Host| {
            let mut engine = engine.lock().unwrap();
            engine.pump();
            engine.busy()
        };
        for &(kind, window) in &windows {
            until(&mut host, kind, |h| quiet(h, window) && !busy(h));
        }
        // one plays: only it asks
        let &(_, roll) = windows.iter().find(|(kind, _)| *kind == "roll").unwrap();
        ask(&mut host, roll, json!({"do": {"verb": "play"}}));
        assert!(busy(&host), "the pass may land unasked");
        for &(kind, window) in &windows {
            if window != roll {
                assert!(quiet(&host, window), "{kind} stands");
            }
        }
        ask(&mut host, roll, json!({"do": {"verb": "stop"}}));
        until(&mut host, "the stop never settled", |h| {
            quiet(h, roll) && !busy(h)
        });
        // and closed while it plays, nothing of it is left
        ask(&mut host, roll, json!({"do": {"verb": "play"}}));
        host.close_audio(roll);
        until(&mut host, "the closed window's transport never went", |h| {
            !busy(h)
        });
        assert!(host.tree_demand(roll, true).is_empty());
        windows.retain(|&(_, window)| window != roll);
        for &(kind, window) in &windows {
            host.close_audio(window);
            until(&mut host, kind, |h| !busy(h));
            assert!(host.tree_demand(window, true).is_empty(), "{kind}");
        }
        running.store(false, std::sync::atomic::Ordering::Relaxed);
        device.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    /// **A score is an application of this host's**: a client opens it over
    /// a document through the door, the engraver here draws its window, a
    /// verb is its turn, its file is read and written here, a file a window
    /// holds is not opened twice, and File > Close frees the window and tells
    /// the client.
    #[cfg(feature = "score")]
    #[test]
    fn a_client_opens_a_score_in_the_hosts_application() {
        use std::net::{Ipv4Addr, SocketAddr};

        let folder = std::env::temp_dir().join(format!("host-score-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let mut host = host();
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_991)));
        let first = door(
            &mut host,
            GUI_APP_OPEN,
            vec![
                OscType::Int(1),
                OscType::String("score".into()),
                OscType::String(json!({"title": "score"}).to_string()),
            ],
            client,
        );
        let window = answered(&mut host, first, 1)["ok"]["window"]
            .as_i64()
            .expect("a window") as i32;
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(window).is_some()
        });
        assert!(
            matches!(
                host.head_clock_of(window, None),
                clocks::HeadClock::Transport(_)
            ),
            "its play cursor reads a transport of its own"
        );
        // named nothing, it has no document, and is told to make one
        assert_eq!(
            ask(&mut host, window, json!({"query": "file"}))["empty"],
            true
        );
        ask(&mut host, window, json!({"do": {"verb": "new_file"}}));
        assert_eq!(
            ask(&mut host, window, json!({"query": "file"}))["empty"],
            false
        );
        assert_eq!(
            ask(&mut host, window, json!({"query": "transport"}))["rolling"],
            false
        );
        let refused = ask(
            &mut host,
            window,
            json!({"do": {"verb": "act", "call": {"action": "articulation", "name": "stacc"}}}),
        );
        assert!(refused["refused"].is_string(), "{refused}");
        let path = folder.join("score.mei");
        let saved = ask(
            &mut host,
            window,
            json!({"do": {"verb": "save", "path": path.to_str().unwrap()}}),
        );
        assert_eq!(saved["destination"], path.to_str().unwrap(), "{saved}");
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("<mei"), "{written}");
        // the file it holds now brings it forward, for the host's own open
        let again = host
            .open_app_score_for(path.to_str(), None, None, json!({}), "again", None)
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !host
            .take_effects()
            .iter()
            .any(|e| matches!(e, HostEffect::Raise(id) if *id == window))
        {
            assert!(
                std::time::Instant::now() < deadline,
                "never brought forward"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!host.owns_audio(again));
        let close = host.event_message(
            window,
            2,
            vec![
                OscType::String("menu".into()),
                OscType::String("close".into()),
            ],
        );
        assert!(host.deliver(window, &close));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !host.take_effects().iter().any(|e| {
            matches!(e, HostEffect::Send { to, msg } if *to == client && msg.addr == GUI_CLOSED)
        }) {
            assert!(
                std::time::Instant::now() < deadline,
                "the client never heard the close"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(host.window_def(window).is_none());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_multitrack_window_opens_with_no_session_and_outlives_its_last() {
        let mut host = host();
        let window = host.open_multitrack(None, "empty").unwrap();
        until(&mut host, "the empty window was never drawn", |host| {
            host.window_def(window).is_some()
        });
        assert_eq!(
            ask(&mut host, window, json!({"query":"file"})),
            json!({"empty":true})
        );
        assert_eq!(
            ask(&mut host, window, json!({"query":"history"}))["canUndo"],
            false
        );
        assert_eq!(
            ask(&mut host, window, json!({"do":{"verb":"undo"}}))["why"],
            "no file is open"
        );
        ask(&mut host, window, json!({"do":{"verb":"new_file"}}));
        let object = ask(&mut host, window, json!({"query":"file"}))["object"].clone();
        assert!(object.is_i64());
        ask(&mut host, window, json!({"do":{"verb":"close_file"}}));
        assert!(
            host.window_def(window).is_some(),
            "the last session is not the window"
        );
        assert_eq!(
            ask(&mut host, window, json!({"query":"file"})),
            json!({"empty":true})
        );
        host.close_audio(window);
    }

    /// **A file the editor cannot open is said to whoever asked**, and no
    /// window is left of it.
    #[test]
    fn a_file_that_does_not_open_is_answered() {
        use std::net::{Ipv4Addr, SocketAddr};

        let mut host = host();
        let client = ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 57_998)));
        let options = json!({"path": "/nowhere/at/all.wav"}).to_string();
        let first = door(
            &mut host,
            GUI_APP_OPEN,
            vec![
                OscType::Int(1),
                OscType::String("audio".into()),
                OscType::String(options),
            ],
            client,
        );
        let answer = answered(&mut host, first, 1);
        assert_eq!(answer["error"]["type"], "value", "{answer}");
        assert!(host.audio.as_ref().unwrap().windows.is_empty());
    }

    /// **A paste past what the view shows opens the view out to it**: the
    /// view zoomed in by a hand, a paste landing past its right edge, and
    /// the window's axis then holds the whole pasted span.
    #[test]
    fn a_paste_past_the_view_opens_it_out() {
        let path = take("paste-view", 24_000);
        let mut host = host();
        let def_id = opened(&mut host, &path);
        let widget = widget_of(&host, def_id);
        // a hand zooms in to the first quarter, and the window says so
        host.zoom_timeline(widget, 0.25, 0.0);
        let (nav, total) = host.timeline_nav(widget).expect("an axis");
        assert_eq!(total, 24_000);
        assert!(nav.len < 7_000.0, "zoomed in: {nav:?}");
        let view = host.event_message(
            widget,
            1,
            vec![
                OscType::String("view".into()),
                OscType::Double(nav.start),
                OscType::Double(nav.len),
            ],
        );
        assert!(host.deliver(def_id, &view));
        // a copy of the file's second half, pasted after the view's end
        let part = clausters_document::clipboard::SegmentPart {
            path: path.clone(),
            start: 12_000,
            frames: 12_000,
            fade_in: 0,
            fade_out: 0,
            fade_in_shape: Default::default(),
            fade_out_shape: Default::default(),
            channels: None,
            source_channels: 1,
            source_frames: 24_000,
            source_rate: 48_000.0,
        };
        let doc =
            clausters_document::clipboard::Clipboard::segments(1, 48_000.0, vec![part]).to_json();
        let at = nav.start + nav.len + 1_000.0;
        let paste = host.event_message(
            widget,
            2,
            vec![
                OscType::String("paste".into()),
                OscType::Double(at),
                OscType::String("segments".into()),
                OscType::String(doc),
            ],
        );
        assert!(host.deliver(def_id, &paste));
        assert_eq!(
            parts(&mut host, def_id).len(),
            3,
            "pasted: {:?}",
            parts(&mut host, def_id)
        );
        // the window applies what the paste left on the next tick
        until(&mut host, "the view never opened out", |h| {
            h.timeline_nav(widget).is_some_and(|(nav, total)| {
                total == 36_000 && nav.start <= at && nav.start + nav.len >= at + 12_000.0
            })
        });
        let _ = std::fs::remove_file(&path);
    }

    /// **A window closing leaves another's action waiting**: a save asked
    /// of one window is answered with where it wrote, though a second
    /// window closed while its render ran.
    #[test]
    fn a_window_closing_leaves_another_s_save_to_land() {
        let first = take("saving", 48_000);
        let second = take("closing", 4_800);
        let out =
            std::env::temp_dir().join(format!("host-audio-saving-out-{}.wav", std::process::id()));
        let mut host = host();
        let saving = opened(&mut host, &first);
        let closing = opened(&mut host, &second);
        let rx = host.ask_audio(
            saving,
            json!({"do": {"verb": "save",
                "request": {"path": out.to_string_lossy(), "format": "float"}}}),
        );
        host.close_audio(closing);
        let mut answer = None;
        until(&mut host, "the save was never answered", |_| {
            answer = rx.try_recv().ok();
            answer.is_some()
        });
        let saved = answer.unwrap().expect("the save is answered as it lands");
        assert_eq!(
            saved["destination"]["path"],
            json!(out.to_string_lossy()),
            "{saved}"
        );
        for path in [&first, &second] {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(&out);
    }

    /// **Two new files are two files**: each a file of its own in the
    /// working folder, so writing the second leaves the first as it was.
    #[test]
    fn two_new_files_in_one_window_are_two_files() {
        let mut host = host();
        let def_id = host.open_audio(None, "new").expect("opened");
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        let path_of = |host: &mut Host| {
            parts(host, def_id)[0]["path"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        let made = |host: &mut Host, channels: u32| {
            let made = ask(
                host,
                def_id,
                json!({"do": {"verb": "newFile",
                    "shape": {"channels": channels, "frames": 4_800, "rate": 48_000.0}}}),
            );
            assert!(made.get("raised").is_none(), "{made}");
            settled(host, def_id);
        };
        made(&mut host, 1);
        let first = path_of(&mut host);
        made(&mut host, 2);
        let second = path_of(&mut host);
        assert!(!first.is_empty() && !second.is_empty(), "{first} {second}");
        assert_ne!(first, second, "each new file is a file of its own");
    }

    /// A session of one track and one box over a take, in a folder of its own.
    fn song_session(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        use clausters_document::multitrack::{
            Automation, Content, Multitrack, Region, TakeLane, Track, Window as RegionWindow,
        };
        use clausters_document::session::{Session, Source};
        use clausters_document::{Document, Lifetime, NodeId, Opaque, SourceId, SourceRef};

        let folder = std::env::temp_dir().join(format!("host-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::rename(take(tag, 24_000), folder.join("take.wav")).unwrap();
        let content = Content::Window {
            window: RegionWindow {
                source: SourceRef {
                    source: SourceId(1),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                },
                start: 0.0,
                duration: 0.5,
            },
            playrate: 1.0,
            args: Opaque::none(),
            looping: false,
        };
        let mut track = Track::new(NodeId(2), NodeId(3));
        track.take_lanes[0] = TakeLane {
            regions: vec![Region::new(
                NodeId(4),
                clausters_document::Second(0.0),
                clausters_document::Second(0.5),
                content,
            )],
            ..TakeLane::new(NodeId(3))
        };
        let mut gain = Automation::new(NodeId(5), Opaque(json!({"port": "gain"})));
        gain.points = [(0.0, 0.0), (0.5, 1.0)]
            .map(|(at, value)| clausters_document::Point {
                at: clausters_document::Second(at),
                value,
                data: Opaque::none(),
            })
            .to_vec();
        track.automation.push(gain);
        let session = Session::new(Document::empty())
            .with_multitrack(Multitrack {
                tracks: vec![track],
                ..Multitrack::default()
            })
            .with_source(SourceId(1), Source::file("take.wav", Lifetime::Session));
        let path = folder.join("song.json");
        std::fs::write(&path, serde_json::to_string(&session).unwrap()).unwrap();
        (folder, path)
    }

    /// What the front does with a gesture's effects: each event reaches the
    /// application that holds the window, and the application's turn runs.
    fn delivered(host: &mut Host, effects: Vec<crate::host::gestures::GestureEffect>) {
        for effect in effects {
            if let crate::host::gestures::GestureEffect::Emit {
                def_id,
                widget_id,
                seq,
                args,
            } = effect
            {
                let message = host.event_message(widget_id, seq, args);
                host.deliver(def_id, &message);
            }
        }
        host.take_effects();
    }

    /// **The window knows which parameter a curve moves, and a hand on its
    /// control is heard and never written**, through the host's application.
    #[test]
    fn a_hand_on_an_automated_control_reaches_the_playback_and_no_history() {
        use crate::host::gestures::GestureEffect;

        let (folder, path) = song_session("hold");
        let mut host = host();
        let def_id = host
            .open_multitrack(Some(path.to_str().unwrap()), "song")
            .expect("opened");
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        let structure = host.audio.as_ref().unwrap().windows[&def_id].structure;
        let widget = host.own_widget_id(structure, AUDIO, "widget").unwrap();
        until(&mut host, "the automated prop never came", |h| {
            let props = ask(
                h,
                def_id,
                json!({"member": {"verb": "props", "widget": widget}}),
            );
            props["automated"] == json!(["5", "gain"])
        });
        let arg = |text: &str| OscType::String(text.into());
        for args in [
            vec![arg("port_hold"), arg("2"), arg("gain"), OscType::Float(0.5)],
            vec![arg("port_hold"), arg("2"), arg("gain")],
        ] {
            delivered(
                &mut host,
                vec![GestureEffect::Emit {
                    def_id,
                    widget_id: widget,
                    seq: 1,
                    args,
                }],
            );
        }
        let history = ask(&mut host, def_id, json!({"query": "history"}));
        assert_eq!(history["canUndo"], false, "{history}");
        host.close_audio(def_id);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// **The space bar is the window's after a press on a tool of its
    /// transport row.** Rewind took the focus, so the space bar pressed Rewind
    /// again and nothing played.
    #[test]
    fn the_space_bar_plays_after_a_press_on_the_transport_row() {
        use crate::host::gestures::{GestureCtx, Gestures};
        use crate::host::widget::element::Key;

        let (folder, path) = song_session("space");
        let mut host = host();
        let def_id = host
            .open_multitrack(Some(path.to_str().unwrap()), "song")
            .expect("opened");
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        let structure = host.audio.as_ref().unwrap().windows[&def_id].structure;
        let rewind = host.own_widget_id(structure, AUDIO, "rewind").unwrap();
        let ctx = GestureCtx::new(def_id, 1000, 560);
        let mut hand = Gestures::default();
        let rolling =
            |h: &mut Host| ask(h, def_id, json!({"query": "transport"}))["rolling"].clone();
        let tool = host
            .layout_window(def_id, 1000, 560)
            .unwrap()
            .iter()
            .find(|p| p.widget.id == Some(rewind))
            .unwrap()
            .rect;
        let at = (
            f64::from(tool.x + tool.w * 0.5),
            f64::from(tool.y + tool.h * 0.5),
        );
        let pressed = hand.press(&mut host, &ctx, at.0, at.1);
        delivered(&mut host, pressed);
        let released = hand.release(&mut host, &ctx, at.0, at.1);
        delivered(&mut host, released);
        assert_eq!(
            host.focused(),
            None,
            "a tool leaves the keys where they were"
        );
        let space = hand
            .press_key(&mut host, &ctx, Key::Char(' '), Some(at))
            .unwrap();
        delivered(&mut host, space);
        assert_eq!(rolling(&mut host), true);
        host.close_audio(def_id);
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// **A script's edit is not a change the hand was overtaken by**: the window
    /// draws it at once, so a gesture made after it is applied, whether it is
    /// the window's first. Both reach the host's outbox as the version it names.
    #[test]
    fn a_gesture_after_a_script_s_edit_is_not_refused() {
        let (folder, path) = song_session("after-script");
        let mut host = host();
        let def_id = host
            .open_multitrack(Some(path.to_str().unwrap()), "song")
            .expect("opened");
        until(&mut host, "the window was never drawn", |h| {
            h.window_def(def_id).is_some()
        });
        let handle = |host: &mut Host, call: Value| {
            ask(
                host,
                def_id,
                json!({"do": {"verb": "handle", "call": call}}),
            )
        };
        let state = handle(&mut host, json!({"verb": "state"}));
        let mut tracks = state["tracks"].clone();
        tracks[0]["level"] = json!(0.5);
        let applied = handle(
            &mut host,
            json!({"verb": "apply", "intent": {"intent": "settracks", "tracks": tracks}}),
        );
        assert_eq!(applied["applied"], true, "{applied}");
        host.take_effects();
        let at = ask(&mut host, def_id, json!({"query": "history"}))["version"].clone();
        assert_eq!(
            json!(host.outbox.borrow().version()),
            at,
            "the window is told"
        );
        host.close_audio(def_id);
        let _ = std::fs::remove_dir_all(&folder);
    }
}
