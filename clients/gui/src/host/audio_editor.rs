//! **The audio editor this host owns**: the applications crate's audio
//! editor opened here, over a file, on an engine this host holds, and every
//! gesture on its window answered here.
//!
//! The application is one program (`crates/clausters-apps/PLAN.md`, "The
//! second turn"): the editor's coordinator, the take it reads and the engine
//! it plays on are in this process, and the coordinator's runs are carried out
//! by [`AudioApp`], once, in Rust. What is this module's is the window: the
//! widget ids it is drawn with, the effects that are the window's (props set,
//! the window drawn again, an answer, the clipboard), and the meters and the
//! play cursor, which read the engine the application holds rather than a
//! server's segment.

use std::sync::{Arc, Mutex};

use clausters_app_engine::AppEngine;
use clausters_app_engine::audio::{AudioApp, Ran};
use clausters_apps::editing::{Editing, MemberId};
use clausters_editing::conversation::Answer;
use serde_json::{Value, json};

use super::*;

/// The role an audio editor's widgets play, in the names the host's own
/// widget ids are asked for by.
const AUDIO: &str = "audio";

/// The widgets of an audio editor's window, by the role the editor's
/// `window` verb names them with.
const ROLES: &[&str] = &["widget", "meter", "pane", "envelope", "lane", "filesWidget"];

/// **The audio editor's application in this host**: its engine and the
/// carrier of its runs, the editing context its files' histories are in,
/// and its windows.
pub(crate) struct AudioHost {
    app: AudioApp,
    editing: Editing,
    windows: HashMap<i32, AudioWindow>,
    /// What the windows' meters and play cursors read: the engine's own.
    source: Arc<dyn BusSource>,
    next_structure: i64,
}

impl std::fmt::Debug for AudioHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioHost")
            .field("windows", &self.windows.len())
            .finish_non_exhaustive()
    }
}

/// One audio editor's window.
#[derive(Debug)]
struct AudioWindow {
    member: MemberId,
    /// The structure its own widget ids are named under.
    structure: i64,
    /// The ids its `window` verb is drawn with.
    ids: Value,
    /// Every widget it numbered, which a history step draws again.
    widgets: Vec<i32>,
    /// The widgets that draw the take, which a band and the loop switch are
    /// set on.
    takes: Vec<i32>,
    /// The folder its renders are written to, removed when it closes.
    workspace: std::path::PathBuf,
    /// The actions last handed to the window, so an unchanged set is not
    /// sent again.
    actions: Option<Value>,
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
}

impl AudioHost {
    /// The application over `engine`, holding no window yet.
    pub(crate) fn new(engine: Arc<Mutex<AppEngine>>) -> AudioHost {
        let reading = engine.lock().unwrap_or_else(|e| e.into_inner()).reading();
        let app = AudioApp::new(engine);
        let mut editing = Editing::new();
        app.hold(&mut editing);
        AudioHost {
            app,
            editing,
            windows: HashMap::new(),
            source: Arc::new(EngineSource(reading)),
            next_structure: 0,
        }
    }

    fn member_call(&mut self, member: MemberId, call: Value) -> Value {
        self.app.call(
            &mut self.editing,
            &json!({"verb": "member", "member": member, "call": call}),
        )
    }
}

impl Host {
    /// **Holds `engine` as the audio editor's**, for a caller that brings its
    /// own (a test driving it block by block); [`Host::open_audio`] opens one
    /// on the machine's output otherwise.
    pub fn hold_audio_engine(&mut self, engine: Arc<Mutex<AppEngine>>) {
        self.audio = Some(AudioHost::new(engine));
    }

    /// **Opens the audio editor** in a window of its own, over the sound file
    /// at `path` or over none, and answers the window's id. The engine is
    /// opened on the machine's output the first time, unless one is held.
    pub fn open_audio(&mut self, path: Option<&str>, title: &str) -> Result<i32, String> {
        if self.audio.is_none() {
            let engine = AppEngine::open()?;
            self.hold_audio_engine(Arc::new(Mutex::new(engine)));
        }
        let structure = self.audio.as_mut().map(|audio| {
            audio.next_structure += 1;
            audio.next_structure
        });
        let structure = structure.ok_or("the audio editor has no engine")?;
        let own = |host: &mut Host, key: &str| {
            host.own_widget(structure, AUDIO, key)
                .ok_or_else(|| "out of widget ids for the audio editor".to_string())
        };
        let def_id = own(self, "window")?;
        let mut ids = serde_json::Map::new();
        let mut widgets = Vec::new();
        for role in ROLES {
            let id = own(self, role)?;
            ids.insert((*role).into(), json!(id));
            widgets.push(id);
        }
        let workspace = std::env::temp_dir().join(format!(
            "clausters-audio-{}-{structure}",
            std::process::id()
        ));
        std::fs::create_dir_all(&workspace)
            .map_err(|e| format!("the working folder {}: {e}", workspace.display()))?;
        let mut request = json!({"verb": "openAudio", "key": format!("audio:{structure}"),
            "workspace": workspace.to_string_lossy(), "title": title, "display": 0});
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
        let Some(audio) = self.audio.as_mut() else {
            return Err("the audio editor has no engine".into());
        };
        let opened = audio.app.call(&mut audio.editing, &request);
        let member = opened["member"]
            .as_u64()
            .and_then(|m| MemberId::try_from(m).ok())
            .ok_or_else(|| opened["error"].as_str().unwrap_or("not opened").to_string())?;
        let dialogs = audio.member_call(member, json!({"verb": "dialogs"}));
        let names: Vec<String> = dialogs["dialogs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| n.as_str().map(str::to_string))
            .collect();
        let mut dialog_ids = serde_json::Map::new();
        for name in names {
            let id = own(self, &format!("dialog:{name}"))?;
            dialog_ids.insert(name, json!(id));
            widgets.push(id);
        }
        ids.insert("dialogs".into(), Value::Object(dialog_ids));
        let takes = vec![
            ids["widget"].as_i64().unwrap_or(0) as i32,
            ids["lane"].as_i64().unwrap_or(0) as i32,
        ];
        let Some(audio) = self.audio.as_mut() else {
            return Err("the audio editor has no engine".into());
        };
        audio.member_call(
            member,
            json!({"verb": "sync", "window": def_id, "title": title}),
        );
        audio.member_call(member, json!({"verb": "attach_view"}));
        audio.windows.insert(
            def_id,
            AudioWindow {
                member,
                structure,
                ids: Value::Object(ids),
                widgets,
                takes,
                workspace,
                actions: None,
            },
        );
        let source = Arc::clone(&audio.source);
        self.window_sources.insert(def_id, source);
        self.publish_audio(def_id);
        let rate = self.audio.as_ref().map_or(0.0, |a| a.source.sample_rate());
        self.act_audio(
            def_id,
            json!({"verb": "open", "chunk": 1, "rate": rate, "empty": path.is_none(),
                "window": true}),
            false,
        );
        // **The play cursor is the editor's transport's position**: taken as
        // the open starts, which a render of the file's peaks may outlast.
        let transport = self.audio.as_mut().and_then(|audio| {
            audio.app.call(
                &mut audio.editing,
                &json!({"verb": "audio", "member": member, "query": "transport"}),
            )["transport"]
                .as_u64()
        });
        if let Some(transport) = transport {
            self.set_head_clock_of(def_id, clocks::HeadClock::Transport(transport as usize));
        }
        diag::info!("audio: opened {}", path.unwrap_or("an empty editor"));
        Ok(def_id)
    }

    /// Whether window `def_id` is an audio editor this host owns.
    pub(crate) fn owns_audio(&self, def_id: i32) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| audio.windows.contains_key(&def_id))
    }

    /// **A gesture on an audio editor's window**, answered by its coordinator:
    /// a paste or a mix takes its clipboard lease first, as a client's does.
    pub(super) fn answer_audio(&mut self, def_id: i32, message: &OscMessage) -> bool {
        let Some(audio) = self.audio.as_mut() else {
            return false;
        };
        let Some(member) = audio.windows.get(&def_id).map(|w| w.member) else {
            return false;
        };
        let args: Vec<Value> = message
            .args
            .iter()
            .map(document::multitrack::atom)
            .collect();
        let prepared = audio.app.call(
            &mut audio.editing,
            &json!({"verb": "exchange", "action": "event", "member": member, "args": args}),
        );
        let lease = prepared.get("lease").is_some_and(|l| !l.is_null());
        let mut action = json!({"verb": "event", "addr": message.addr, "window": true,
            "args": prepared["args"]});
        if lease {
            action["lease"] = json!(true);
        }
        self.act_audio(def_id, action, lease);
        true
    }

    /// Starts `action` on window `def_id`'s editor and carries out what is the
    /// window's; its end, when it did not stop on a render.
    fn act_audio(
        &mut self,
        def_id: i32,
        action: Value,
        lease: bool,
    ) -> Option<Result<Value, clausters_app_engine::audio::Raised>> {
        let audio = self.audio.as_mut()?;
        let member = audio.windows.get(&def_id)?.member;
        let ran = audio.app.act(&mut audio.editing, member, action, 0, lease);
        let end = ran.end.clone();
        if let Some(Err(raised)) = &end {
            diag::warn!("audio: {}", raised.message);
        }
        self.perform_audio(def_id, ran);
        end
    }

    /// **The audio editor's turn on this host's tick**: what its engine handed
    /// back taken, and every run whose render landed carried on.
    pub fn tick_audio(&mut self) {
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        let landed = audio.app.landed(&mut audio.editing);
        for ran in landed {
            let def_id = self.audio.as_ref().and_then(|audio| {
                audio
                    .windows
                    .iter()
                    .find(|(_, w)| w.member == ran.member)
                    .map(|(id, _)| *id)
            });
            if let Some(def_id) = def_id {
                if let Some(Err(raised)) = &ran.end {
                    diag::warn!("audio: {}", raised.message);
                }
                self.perform_audio(def_id, ran);
            }
        }
    }

    /// **What a run left the window**, carried out on it.
    fn perform_audio(&mut self, def_id: i32, ran: Ran) {
        let mut fx = Vec::new();
        for effect in ran.effects {
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
                "publish" => self.publish_audio(def_id),
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
                "resync" => self.resync_audio(def_id, &mut fx),
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
                            let name = Path::new(effect["path"].as_str().unwrap_or_default())
                                .file_name()
                                .map_or_else(
                                    || "measuring".to_string(),
                                    |n| n.to_string_lossy().into_owned(),
                                );
                            vec![
                                (
                                    "progress".to_string(),
                                    json!((done as f64 / total as f64).min(1.0)),
                                ),
                                (
                                    "progress_label".to_string(),
                                    json!(format!("rendering {name}")),
                                ),
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
                "closedWindow" => self.forget_audio(def_id),
                // what a client kept of the file in focus, the cursor and the
                // layers is the editor's own here
                _ => {}
            }
        }
        self.publish_audio_actions(def_id);
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

    /// **The window drawn again** with the ids it was numbered with.
    fn publish_audio(&mut self, def_id: i32) {
        use std::net::{Ipv4Addr, SocketAddr};

        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        let Some((member, mut ids)) = audio
            .windows
            .get(&def_id)
            .map(|w| (w.member, w.ids.clone()))
        else {
            return;
        };
        ids["verb"] = json!("window");
        let mut def = audio.member_call(member, ids);
        def["id"] = json!(def_id);
        let structure = audio.windows.get(&def_id).map_or(0, |w| w.structure);
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

    /// **Every widget drawn again** from the editor, after a history step.
    fn resync_audio(&mut self, def_id: i32, fx: &mut Vec<HostEffect>) {
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        let Some((member, widgets)) = audio
            .windows
            .get(&def_id)
            .map(|w| (w.member, w.widgets.clone()))
        else {
            return;
        };
        let props: Vec<(i32, Value)> = widgets
            .into_iter()
            .map(|widget| {
                (
                    widget,
                    audio.member_call(member, json!({"verb": "props", "widget": widget})),
                )
            })
            .collect();
        for (widget, props) in props {
            if let Value::Object(props) = props
                && !props.is_empty()
            {
                self.set_props(widget, props.into_iter().collect(), fx);
            }
        }
    }

    /// The window's actions handed over when they moved.
    fn publish_audio_actions(&mut self, def_id: i32) {
        let Some(audio) = self.audio.as_mut() else {
            return;
        };
        let Some(member) = audio.windows.get(&def_id).map(|w| w.member) else {
            return;
        };
        let actions = audio.app.call(
            &mut audio.editing,
            &json!({"verb": "actions", "member": member}),
        );
        let Some(window) = audio.windows.get_mut(&def_id) else {
            return;
        };
        if window.actions.as_ref() == Some(&actions) || !actions.is_object() {
            return;
        }
        window.actions = Some(actions.clone());
        let mut fx = Vec::new();
        self.set_props(def_id, vec![("actions".into(), actions)], &mut fx);
        self.pending_effects.extend(fx);
    }

    /// **Closes the audio editor's window `def_id`**, as its editor asked once
    /// nothing was left to lose: what sounded it freed, the window freed, and
    /// its working folder removed with the renders it holds.
    fn close_audio(&mut self, def_id: i32) {
        self.act_audio(def_id, json!({"verb": "closed"}), false);
        self.free_own_window(def_id);
        self.forget_audio(def_id);
    }

    /// The window is this host's no longer.
    fn forget_audio(&mut self, def_id: i32) {
        self.window_sources.remove(&def_id);
        let Some(window) = self
            .audio
            .as_mut()
            .and_then(|audio| audio.windows.remove(&def_id))
        else {
            return;
        };
        if let Err(e) = std::fs::remove_dir_all(&window.workspace)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            diag::warn!("audio: {}: {e}", window.workspace.display());
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

    /// **The open's work landed**: the peaks render it hands off has run,
    /// taken on the host's ticks, and nothing is pending on the file.
    fn settled(host: &mut Host, def_id: i32) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            host.take_effects();
            let audio = host.audio.as_mut().unwrap();
            let member = audio.windows[&def_id].member;
            let actions = audio.app.call(
                &mut audio.editing,
                &json!({"verb": "actions", "member": member}),
            );
            if actions["save"]["pending"] != true {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the open never settled"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The parts the take of window `def_id` is made of.
    fn parts(host: &mut Host, def_id: i32) -> Vec<Value> {
        let audio = host.audio.as_mut().unwrap();
        let member = audio.windows[&def_id].member;
        audio.member_call(member, json!({"verb": "segments"}))["parts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// **The host opens the audio editor over a file and answers its
    /// window**: the window is drawn, its meters and play cursor read the
    /// application's engine, and a delete made on its picture is the
    /// editor's edit -- the take is the part after it.
    #[test]
    fn the_host_owns_an_audio_editor_and_answers_its_window() {
        let path = take("owns", 24_000);
        let (engine, _processed) = AppEngine::detached(48_000.0, 2);
        let mut host = Host::new();
        host.hold_audio_engine(Arc::new(Mutex::new(engine)));
        let def_id = host.open_audio(Some(&path), "take").expect("opened");
        assert!(host.window_def(def_id).is_some(), "the window is drawn");
        assert!(host.source_of(def_id).is_some(), "it reads the engine");
        assert!(
            matches!(
                host.head_clock_of(def_id, None),
                clocks::HeadClock::Transport(_)
            ),
            "its play cursor is drawn from the editor's transport"
        );
        // the open's peaks render lands on a tick
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while host.audio.as_ref().is_some_and(|a| {
            std::fs::read_dir(&a.windows[&def_id].workspace).map_or(0, |d| d.count()) == 0
        }) {
            assert!(
                std::time::Instant::now() < deadline,
                "the peaks never landed"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
            host.take_effects();
        }
        let before = parts(&mut host, def_id);
        assert_eq!(before.len(), 1, "{before:?}");
        let widget = host.audio.as_ref().unwrap().windows[&def_id].ids["widget"]
            .as_i64()
            .unwrap() as i32;
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

    /// **A take may be cut down to nothing**: a delete of all of it leaves
    /// an empty take, which the window draws on an axis of its own, a save
    /// writes as a file that is a header, and an undo puts back.
    #[test]
    fn a_take_cut_down_to_nothing_saves_as_a_header() {
        let path = take("empty", 24_000);
        let out =
            std::env::temp_dir().join(format!("host-audio-empty-out-{}.wav", std::process::id()));
        let (engine, _processed) = AppEngine::detached(48_000.0, 2);
        let mut host = Host::new();
        host.hold_audio_engine(Arc::new(Mutex::new(engine)));
        let def_id = host.open_audio(Some(&path), "take").expect("opened");
        settled(&mut host, def_id);
        let widget = host.audio.as_ref().unwrap().windows[&def_id].ids["widget"]
            .as_i64()
            .unwrap() as i32;
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
        host.take_effects();
        assert!(parts(&mut host, def_id).is_empty(), "nothing is left");
        assert!(host.window_def(def_id).is_some(), "the window still draws");
        // saved, it is a header and no frames
        let audio = host.audio.as_mut().unwrap();
        let member = audio.windows[&def_id].member;
        let mut ran = audio.app.act(
            &mut audio.editing,
            member,
            json!({"verb": "save", "request": {"path": out.to_string_lossy(), "format": "int16"}}),
            3,
            false,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while ran.end.is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the save never landed"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
            let audio = host.audio.as_mut().unwrap();
            if let Some(landed) = audio
                .app
                .landed(&mut audio.editing)
                .into_iter()
                .find(|r| r.end.is_some())
            {
                ran = landed;
            }
        }
        assert!(matches!(ran.end, Some(Ok(_))), "{:?}", ran.end);
        let layout =
            clausters_core::pcm::Layout::open(&out.to_string_lossy()).expect("a sound file");
        assert_eq!(layout.frames, 0);
        assert_eq!(
            std::fs::metadata(&out).unwrap().len(),
            44,
            "a WAV header and nothing else"
        );
        // and an undo puts the take back
        let window = host.audio.as_ref().unwrap().windows[&def_id].member;
        let _ = window;
        let undo = host.event_message(def_id, 2, vec![OscType::String("undo".into())]);
        assert!(host.deliver(def_id, &undo));
        host.take_effects();
        assert_eq!(parts(&mut host, def_id).len(), 1, "undone");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&out);
    }

    /// **A paste past what the view shows opens the view out to it**: the
    /// view zoomed in by a hand, a paste landing past its right edge, and
    /// the window's axis then holds the whole pasted span.
    #[test]
    fn a_paste_past_the_view_opens_it_out() {
        let path = take("paste-view", 24_000);
        let (engine, _processed) = AppEngine::detached(48_000.0, 2);
        let mut host = Host::new();
        host.hold_audio_engine(Arc::new(Mutex::new(engine)));
        let def_id = host.open_audio(Some(&path), "take").expect("opened");
        settled(&mut host, def_id);
        let widget = host.audio.as_ref().unwrap().windows[&def_id].ids["widget"]
            .as_i64()
            .unwrap() as i32;
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
                OscType::Float(nav.start as f32),
                OscType::Float(nav.len as f32),
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
        host.take_effects();
        assert_eq!(
            parts(&mut host, def_id).len(),
            3,
            "pasted: {:?}",
            parts(&mut host, def_id)
        );
        let (nav, total) = host.timeline_nav(widget).expect("an axis");
        assert_eq!(total, 36_000, "the take is the paste longer");
        assert!(
            nav.start <= at && nav.start + nav.len >= at + 12_000.0,
            "the pasted span {at}..{} is in view: {nav:?}",
            at + 12_000.0
        );
        let _ = std::fs::remove_file(&path);
    }
}
