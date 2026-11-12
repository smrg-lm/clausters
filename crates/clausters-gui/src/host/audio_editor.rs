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
const ROLES: &[&str] = &[
    "widget",
    "meter",
    "pane",
    "envelope",
    "lane",
    "ruler",
    "filesWidget",
];

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
    /// (`--multitrack [file]`, `--save-to`): the session at `path` saves to
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

    /// **The roll over the file at `path`, for this host alone**: what
    /// `--roll` opens. A MIDI file is read as the sequence it holds, and a
    /// score (MEI, MusicXML, ABC -- what the engraver reads) as the sequence
    /// it plays as, as the roll's own File > Open reads one
    /// (`clausters_apps::notes::file::read`); with no file the roll opens
    /// over an empty MIDI 1.0 sequence. Its Save writes to `save_to`, and
    /// asks where without it, so the file opened is not written over unasked.
    pub fn open_roll_file(
        &mut self,
        path: Option<&str>,
        save_to: Option<&str>,
        title: &str,
    ) -> Result<i32, String> {
        let request = match path {
            Some(_) => json!({}),
            None => json!({"sequence": {"events": [], "midi": "1.0"}}),
        };
        self.open_app_saving(
            AppKind::Notes,
            path,
            Some(super::audio_app::AppData::Notes {
                over: None,
                request,
            }),
            title,
            None,
            Some(save_to.map(str::to_string)),
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

    /// **The roll over box of notes `source` of multitrack window
    /// `parent`**, in a window of its own that goes when the multitrack's
    /// does; one already open is left as it is. A client that asked for it
    /// (`reply`) is answered with the window, as an opener is, and hears it
    /// close when nobody else opened it.
    fn open_roll_of(&mut self, parent: i32, source: u64, reply: Option<Reply>) {
        let (kind, title) = (AppKind::Roll { source, parent }, format!("notes {source}"));
        let owned = self.owns_audio(parent);
        let open = self.audio.as_mut().and_then(|audio| {
            audio
                .windows
                .iter_mut()
                .find(|(_, w)| w.kind == kind)
                .map(|(id, w)| (*id, w))
        });
        let answered = match open {
            Some((def_id, window)) => {
                if let Some(Reply::Client(to, _)) = &reply {
                    window.opener.get_or_insert(*to);
                }
                Ok(json!({ "window": def_id }))
            }
            None if !owned => Err((
                "runtime".to_string(),
                "the multitrack's window is gone".to_string(),
            )),
            None => {
                return match self.open_app_for(kind, None, &title, reply) {
                    Ok(def_id) => self.depend(def_id, parent),
                    Err(why) => diag::warn!("roll: {why}"),
                };
            }
        };
        match reply {
            Some(Reply::Client(to, seq)) => {
                let msg = super::apps::app_reply(seq, answered);
                self.pending_effects.push(HostEffect::Send { to, msg });
            }
            #[cfg(test)]
            Some(Reply::Local(tx)) => {
                let _ = tx.send(answered);
            }
            None => {}
        }
    }

    /// **The id of the widget window `def_id` numbers `role`**, for a test
    /// outside this module that drives one of an application's widgets.
    #[cfg(all(test, feature = "score"))]
    pub(crate) fn app_widget(&self, def_id: i32, role: &str) -> Option<i32> {
        let structure = self.audio.as_ref()?.windows.get(&def_id)?.structure;
        self.own_widget_id(structure, AUDIO, role)
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
        // **What the hand holds is read after the turns before it**: a verb
        // the application answers with a selection (a roll's selection by
        // pitch) corrects it from the application's thread, so the question
        // goes round that thread and is answered when it is back.
        if call["query"] == "selected" && self.multitrack_marks_held(def_id) {
            let step = super::scripted::Step::Ask {
                call: call.clone(),
                from,
                seq,
            };
            return self.script(def_id, step, effects);
        }
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
                FromApp::After {
                    def_id,
                    call,
                    to,
                    seq,
                } => {
                    // a step of a script, performed and taken in
                    if call["settled"] == true {
                        self.script_answered(def_id);
                        continue;
                    }
                    if call["opened"].is_i64() {
                        let msg = super::apps::app_reply(seq, Ok(json!({ "window": def_id })));
                        self.pending_effects.push(HostEffect::Send { to, msg });
                        continue;
                    }
                    if let Some(id) = call["info"].as_i64() {
                        let msg = self.info_of(id as i32, to);
                        self.pending_effects.push(HostEffect::Send { to, msg });
                        self.script_answered(def_id);
                        continue;
                    }
                    let mut fx = Vec::new();
                    let answer = self
                        .multitrack_marks(def_id, &call, &mut fx)
                        .ok_or_else(|| {
                            (
                                "runtime".to_string(),
                                format!("window {def_id} holds nothing to mark"),
                            )
                        });
                    self.pending_effects.extend(fx);
                    let msg = super::apps::app_reply(seq, answer);
                    self.pending_effects.push(HostEffect::Send { to, msg });
                    self.script_answered(def_id);
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
                    reply,
                } => self.open_roll_of(parent, source, reply),
                FromApp::Converted { sequence, title } => {
                    let request = json!({ "sequence": sequence });
                    if let Err(why) = self.open_app_notes_for(None, request, &title, None) {
                        diag::warn!("the conversion's roll did not open: {why}");
                    }
                }
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
        // whoever opened it, answered once the window is there (below)
        let opener = match &reply {
            Some(Reply::Client(to, seq)) => Some((*to, *seq)),
            _ => None,
        };
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
            AppKind::Score => super::audio_app::SCORE_ROLES,
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
            window.marks = matches!(
                kind,
                AppKind::Multitrack | AppKind::Notes | AppKind::Roll { .. }
            )
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
        // **the opener is answered once the window is there**: the
        // application publishes it in answer to the binding, so the answer
        // goes round its thread behind that, and a script's next step finds
        // the window
        if let Some((to, seq)) = opener
            && let Some(audio) = self.audio.as_mut()
            && audio
                .ask(ToApp::After {
                    def_id,
                    call: json!({ "opened": def_id }),
                    to,
                    seq,
                })
                .is_ok()
        {
            return;
        }
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

    /// **Whether a script's step on `window` waits its turn**: on a window an
    /// application holds ([`super::scripted`]).
    pub(super) fn scripts(&self, window: i32) -> bool {
        self.owns_audio(window)
    }

    /// **A step of a script on window `window`**, which an application
    /// holds: queued behind the window's others and released when the
    /// application has answered the one before ([`super::scripted`]).
    pub(super) fn script(
        &mut self,
        window: i32,
        step: super::scripted::Step,
        effects: &mut Vec<HostEffect>,
    ) {
        self.scripted.push(window, step);
        self.release_script(window, effects);
    }

    /// The next step of `window`'s script, released if none is out.
    fn release_script(&mut self, window: i32, effects: &mut Vec<HostEffect>) {
        use super::scripted::Step;
        while let Some(step) = self.scripted.next(window) {
            match step {
                // the front performs it, and says so (`performed`)
                Step::Effect(effect) => return effects.push(effect),
                Step::Query { id, from } => {
                    if self.after_query(window, id, from) {
                        return;
                    }
                    effects.push(HostEffect::Reply(self.info_of(id, from)));
                }
                Step::Set { args, from } => self.set_now(&args, from, effects),
                Step::Ask { call, from, seq } => {
                    let asked = self.audio_host().ask(ToApp::After {
                        def_id: window,
                        call,
                        to: from,
                        seq,
                    });
                    if asked.is_ok() {
                        return;
                    }
                    effects.push(HostEffect::Reply(super::apps::app_reply(
                        seq,
                        Err(("runtime".into(), "the application is gone".into())),
                    )));
                }
            }
            self.scripted.done(window);
        }
    }

    /// **The front performed the step of `window`'s script that was out**:
    /// what it asked of the application is on its way there, and the host
    /// asks to hear back behind it; the next step waits for that word.
    pub fn performed(&mut self, window: i32) {
        if !self.scripted.busy(window) {
            return;
        }
        let asked = self.audio_host().ask(ToApp::After {
            def_id: window,
            call: json!({ "settled": true }),
            to: ClientId::Web,
            seq: 0,
        });
        if asked.is_err() {
            self.script_answered(window);
        }
    }

    /// The step out on `window` is answered: the next is released.
    fn script_answered(&mut self, window: i32) {
        self.scripted.done(window);
        let mut fx = Vec::new();
        self.release_script(window, &mut fx);
        self.pending_effects.extend(fx);
    }

    /// **The answer to `/gui_query <id>` given once the application has taken
    /// in every turn before it** (`ToApp::After`); whether it was asked, so a
    /// host whose application is gone answers at once.
    pub(super) fn after_query(&mut self, def_id: i32, id: i32, from: ClientId) -> bool {
        self.audio_host()
            .ask(ToApp::After {
                def_id,
                call: json!({ "info": id }),
                to: from,
                seq: 0,
            })
            .is_ok()
    }

    /// Whether window `def_id` holds marks [`multitrack_marks`](Self::multitrack_marks) reads.
    fn multitrack_marks_held(&self, def_id: i32) -> bool {
        self.audio
            .as_ref()
            .and_then(|audio| audio.windows.get(&def_id))
            .is_some_and(|window| window.marks.is_some())
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
        // a box's roll marks what a roll marks
        let roll = matches!(window.kind, AppKind::Notes | AppKind::Roll { .. });
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
            let held = if roll {
                Value::from(call["do"]["regions"].to_string())
            } else {
                Value::from(json!(names).to_string())
            };
            self.set_props(widget, vec![("selected".into(), held)], effects);
            // **a roll's owner hears what a client marked**, as it hears what
            // a hand marked, so a panel that follows the selection follows it
            if roll {
                let mut args = vec![OscType::String("marked".into())];
                args.extend(
                    call["do"]["regions"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_f64)
                        .map(OscType::Double),
                );
                let seq = self.outbox.borrow_mut().stamp(def_id, widget);
                let message = self.event_message(widget, seq, args);
                self.answer_audio(def_id, &message);
            }
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
    pub(super) fn close_audio(&mut self, def_id: i32) {
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
        self.scripted.forget(def_id);
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
mod tests;
