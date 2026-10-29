//! **What an audio member is asked to do**: an action, typed, as its
//! coordinator starts it -- read once from the door's JSON, where a window
//! or a client says it.

use serde::Deserialize;

use super::*;

/// **An action of an audio member.**
#[derive(Clone, Debug)]
pub(crate) enum AudioAction {
    /// The window opened over its file, or over none.
    Open {
        empty: bool,
    },
    /// A message of the window's (`addr`, `args`); `lease` says it holds the
    /// clipboard lease its event was prepared with.
    Event {
        addr: String,
        args: Vec<Value>,
        lease: bool,
    },
    /// A client's verb that edits (`setautomation`, `apply_envelope`), as a
    /// request of the editor's door.
    Act {
        call: Value,
    },
    /// A client's verb of the editor's door that comes to an outcome.
    Verb {
        call: Value,
    },
    /// The file in focus saved, at `path` or its own, in `format`.
    Save {
        path: Option<String>,
        format: String,
    },
    /// An operation: from its command, or as a history step prepared it.
    Operation {
        command: Option<AudioCommand>,
        prepared: Option<Box<Operated>>,
        repairing: bool,
    },
    /// The repairs a member keeps, tried again.
    Retry,
    /// Files opened, by path.
    OpenFile {
        request: Value,
    },
    /// A new file of `shape`.
    NewFile {
        shape: Shape,
    },
    /// What sounds made the take as it now is.
    Sound,
    /// One file's playback brought in step.
    SyncSound {
        file: PlaybackFile,
    },
    /// The window asked to close.
    Close,
    /// The window is gone, closed by the host.
    Closed,
    /// Whether it plays.
    Playing,
    /// Play, from `at` seconds or where it stands.
    Play {
        at: Option<f64>,
    },
    Pause,
    Stop,
    /// The position cursor, at `at` seconds.
    Locate {
        at: f64,
    },
    /// The span the space bar plays, in seconds; `show` draws its band.
    Span {
        span: Option<(f64, f64)>,
        show: bool,
    },
    /// The file in focus's loop switch.
    Looping {
        on: bool,
    },
}

impl AudioAction {
    /// The transport's verbs, which need a file open.
    pub(super) fn transport(&self) -> bool {
        matches!(
            self,
            AudioAction::Playing
                | AudioAction::Play { .. }
                | AudioAction::Pause
                | AudioAction::Stop
                | AudioAction::Locate { .. }
                | AudioAction::Span { .. }
                | AudioAction::Looping { .. }
        )
    }

    /// **An action read from the door's JSON** (`{"verb", ...}`), and
    /// whether a window asked it (`window`); or why it reads as none.
    pub(crate) fn read(action: &Value) -> Result<(AudioAction, bool), String> {
        let window = action["window"] == true;
        let verb = action["verb"].as_str().unwrap_or_default();
        let read = match verb {
            "open" => AudioAction::Open {
                empty: action["empty"] == true,
            },
            "event" => AudioAction::Event {
                addr: action["addr"].as_str().unwrap_or_default().to_string(),
                args: action["args"].as_array().cloned().unwrap_or_default(),
                lease: action["lease"] == true,
            },
            "act" => AudioAction::Act {
                call: action["call"].clone(),
            },
            "verb" => AudioAction::Verb {
                call: action["call"].clone(),
            },
            "save" => {
                #[derive(Deserialize)]
                struct Request {
                    #[serde(default)]
                    path: Option<String>,
                    #[serde(default)]
                    format: Option<String>,
                }
                let request = serde_json::from_value::<Request>(action["request"].clone())
                    .unwrap_or(Request {
                        path: None,
                        format: None,
                    });
                AudioAction::Save {
                    path: request.path,
                    format: request.format.unwrap_or_else(|| "float".into()),
                }
            }
            "operation" => AudioAction::Operation {
                command: match action.get("command").filter(|c| !c.is_null()) {
                    Some(command) => {
                        Some(serde_json::from_value(command.clone()).map_err(|e| e.to_string())?)
                    }
                    None => None,
                },
                prepared: None,
                repairing: action["repairing"] == true,
            },
            "retry" => AudioAction::Retry,
            "openFile" => AudioAction::OpenFile {
                request: action.clone(),
            },
            "newFile" => AudioAction::NewFile {
                shape: Shape {
                    channels: action["shape"]["channels"].as_u64().unwrap_or(1) as u32,
                    frames: action["shape"]["frames"].as_u64().unwrap_or(0),
                    rate: action["shape"]["rate"].as_f64().unwrap_or(0.0),
                },
            },
            "sound" => AudioAction::Sound,
            "close" => AudioAction::Close,
            "closed" => AudioAction::Closed,
            "playing" => AudioAction::Playing,
            "play" => AudioAction::Play {
                at: action["at"].as_f64(),
            },
            "pause" => AudioAction::Pause,
            "stop" => AudioAction::Stop,
            "locate" => AudioAction::Locate {
                at: action["at"].as_f64().unwrap_or(0.0),
            },
            "span" => AudioAction::Span {
                span: action["span"]
                    .as_array()
                    .and_then(|s| Some((s.first()?.as_f64()?, s.get(1)?.as_f64()?))),
                show: action["show"] != false,
            },
            "looping" => AudioAction::Looping {
                on: action["on"] == true,
            },
            other => return Err(format!("no audio verb {other:?}")),
        };
        Ok((read, window))
    }
}
