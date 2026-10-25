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

use super::audio_app::{FromApp, MakeEngine, Reply, Said, ToApp, Wake, WakeSlot};
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
    /// The structure its own widget ids are named under.
    structure: i64,
    title: String,
    /// The widgets that draw the take, which a band and the loop switch are
    /// set on, once the window is bound.
    takes: Vec<i32>,
    /// The client that opened it through the door, told when it goes.
    opener: Option<ClientId>,
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
                structure,
                title: title.to_string(),
                takes: Vec::new(),
                opener,
            },
        );
        let asked = audio.ask(ToApp::Open {
            def_id,
            key: format!("audio:{structure}"),
            title: title.to_string(),
            path: path.map(str::to_string),
            reply,
        });
        if let Err(e) = asked {
            audio.windows.remove(&def_id);
            return Err(e);
        }
        Ok(def_id)
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

    /// Whether window `def_id` is an audio editor this host owns.
    pub(crate) fn owns_audio(&self, def_id: i32) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| audio.windows.contains_key(&def_id))
    }

    /// **A gesture on an audio editor's window**, handed to its application.
    pub(super) fn answer_audio(&mut self, def_id: i32, message: &OscMessage) -> bool {
        let args: Vec<Value> = message
            .args
            .iter()
            .map(document::multitrack::atom)
            .collect();
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
        let Some((structure, title)) = self
            .audio
            .as_ref()
            .and_then(|a| a.windows.get(&def_id))
            .map(|w| (w.structure, w.title.clone()))
        else {
            return;
        };
        let mut ids = serde_json::Map::new();
        let mut widgets = Vec::new();
        for role in ROLES.iter().map(|r| (*r).to_string()) {
            let Some(id) = self.own_widget(structure, AUDIO, &role) else {
                return answer(self, Err(("runtime".into(), "out of widget ids".into())));
            };
            ids.insert(role, json!(id));
            widgets.push(id);
        }
        let mut dialog_ids = serde_json::Map::new();
        for name in dialogs {
            let Some(id) = self.own_widget(structure, AUDIO, &format!("dialog:{name}")) else {
                return answer(self, Err(("runtime".into(), "out of widget ids".into())));
            };
            dialog_ids.insert(name, json!(id));
            widgets.push(id);
        }
        ids.insert("dialogs".into(), Value::Object(dialog_ids));
        let takes = vec![
            ids["widget"].as_i64().unwrap_or(0) as i32,
            ids["lane"].as_i64().unwrap_or(0) as i32,
        ];
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        if let Some(window) = audio.windows.get_mut(&def_id) {
            window.takes = takes;
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
                "closeWindow" => {
                    self.close_audio(def_id);
                    self.pending_effects.extend(fx);
                    return;
                }
                "closedWindow" => {
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

    /// The window is this host's no longer, and whoever opened it is told.
    fn forget_audio(&mut self, def_id: i32) {
        self.window_sources.remove(&def_id);
        let opener = self
            .audio
            .as_mut()
            .and_then(|audio| audio.windows.remove(&def_id))
            .and_then(|window| window.opener);
        if let Some(to) = opener {
            self.pending_effects.push(HostEffect::Send {
                to,
                msg: OscMessage {
                    addr: GUI_CLOSED.into(),
                    args: vec![OscType::Int(def_id)],
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
        assert_eq!(
            std::fs::metadata(&out).unwrap().len(),
            44,
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
}
