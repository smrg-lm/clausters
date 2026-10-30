//! The editor's wire decoded into calls before application dispatch.
use std::collections::HashMap;

use clausters_document::{SourceId, multitrack::Multitrack};
use clausters_editing::multitrack as projection;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use super::{Meter, TransportIds, editor::MultitrackEditor};
use crate::{
    closing,
    turn::{Event, int, number},
};

/// One decoded editor call. Its history version is supplied by the owner.
pub enum Call {
    Sync(Box<Sync>),
    Grid,
    Dialogs,
    Rewind,
    Toggle,
    Stop,
    Clock(f64),
    Window(i32, i32),
    SetWindow(Option<i32>),
    Props(i32),
    Notes,
    NotesView(Option<String>),
    Event(Event),
    Apply(Value),
    Resync,
    Span(Option<(f64, f64)>),
    Settle,
    Announce,
    Acknowledge(i64, Option<String>),
    Unknown,
}

/// A synchronization changes only the fields supplied by the caller.
#[derive(Default)]
pub struct Sync {
    multitrack: Option<Multitrack>,
    sources: Option<(HashMap<SourceId, i64>, HashMap<SourceId, u64>)>,
    meters: Option<Vec<Meter>>,
    cursor: Option<Option<f64>>,
    window: Option<Option<i32>>,
    controls: Option<Option<TransportIds>>,
    close_form: Option<Option<closing::Ids>>,
    asks: Option<bool>,
    saves: Option<bool>,
    dialogs: Option<crate::forms::Ids>,
    grid: Option<f64>,
    snap: Option<bool>,
}

impl Call {
    /// Decode the editor wire, preserving its optional-field behavior.
    pub fn read(request: &Value) -> Self {
        let get = |key: &str| request.get(key).cloned().unwrap_or(Value::Null);
        match request
            .get("verb")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "sync" => {
                let close_form = request.get("closeForm").map(|form| {
                    let id = |key: &str| form[key].as_i64().map(|id| id as i32);
                    Some(closing::Ids {
                        stack: id("stack")?,
                        dialog: id("dialog")?,
                        save: id("save")?,
                        discard: id("discard")?,
                        cancel: id("cancel")?,
                    })
                });
                Self::Sync(Box::new(Sync {
                    multitrack: serde_json::from_value(get("multitrack")).ok(),
                    sources: request
                        .get("sources")
                        .map(|sources| (projection::table(sources), projection::lengths(sources))),
                    meters: serde_json::from_value(get("meters")).ok(),
                    cursor: request.get("cursor").map(Value::as_f64),
                    window: request.get("window").map(|w| w.as_i64().map(|w| w as i32)),
                    controls: request
                        .get("controls")
                        .map(|c| serde_json::from_value(c.clone()).ok()),
                    close_form,
                    asks: get("asks").as_bool(),
                    saves: get("saves").as_bool(),
                    dialogs: get("dialogs").as_object().map(|ids| {
                        ids.iter()
                            .filter_map(|(name, id)| Some((name.clone(), id.as_i64()? as i32)))
                            .collect()
                    }),
                    grid: get("grid").as_f64(),
                    snap: get("snap").as_bool(),
                }))
            }
            "grid" => Self::Grid,
            "dialogs" => Self::Dialogs,
            "rewind" => Self::Rewind,
            "toggle" => Self::Toggle,
            "stop" => Self::Stop,
            "clock" => Self::Clock(number(&get("position"))),
            "window" => Self::Window(int(&get("widget")) as i32, int(&get("ruler")) as i32),
            "setWindow" => Self::SetWindow(get("window").as_i64().map(|w| w as i32)),
            "props" => Self::Props(int(&get("widget")) as i32),
            "notes" => Self::Notes,
            "notesView" => Self::NotesView(get("view").as_str().map(str::to_string)),
            "event" => Self::Event(Event {
                addr: get("addr").as_str().unwrap_or_default().into(),
                args: get("args").as_array().cloned().unwrap_or_default(),
            }),
            "apply" => Self::Apply(get("payload")),
            "resync" => Self::Resync,
            "span" => Self::Span(
                get("span")
                    .as_array()
                    .and_then(|r| Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?)))
                    .filter(|(a, b)| b > a),
            ),
            "settle" => Self::Settle,
            "announce" => Self::Announce,
            "acknowledge" => {
                Self::Acknowledge(int(&get("seq")), get("reason").as_str().map(str::to_string))
            }
            _ => Self::Unknown,
        }
    }

    /// Whether this call changes what the window draws.
    pub fn resyncs(&self) -> bool {
        matches!(self, Self::Sync(_) | Self::NotesView(_))
    }

    /// Execute using the owner's current history version.
    pub fn execute(&self, editor: &mut MultitrackEditor, version: i64) -> Value {
        match self {
            Self::Sync(sync) => {
                if let Some(mt) = &sync.multitrack {
                    editor.set_multitrack(mt.clone());
                }
                if let Some((sources, lengths)) = &sync.sources {
                    editor.set_sources(sources.clone());
                    editor.set_lengths(lengths.clone());
                }
                if let Some(meters) = &sync.meters {
                    editor.set_meters(meters.clone());
                }
                if let Some(cursor) = sync.cursor {
                    editor.set_cursor(cursor);
                }
                if let Some(window) = sync.window {
                    editor.set_window(window);
                }
                if let Some(controls) = sync.controls {
                    editor.set_controls(controls);
                }
                if let Some(form) = sync.close_form {
                    editor.set_close_form(form);
                }
                if let Some(asks) = sync.asks {
                    editor.set_asks_to_close(asks);
                }
                if let Some(saves) = sync.saves {
                    editor.set_saves(saves);
                }
                if let Some(dialogs) = &sync.dialogs {
                    editor.set_dialogs(dialogs.clone());
                }
                if let Some(grid) = sync.grid {
                    editor.set_grid(grid);
                }
                if let Some(snap) = sync.snap {
                    editor.set_snap(snap);
                }
                json!({})
            }
            Self::Grid => json!({"grid": editor.grid(), "snap": editor.snap()}),
            Self::Dialogs => json!({"dialogs": super::dialogs::names()}),
            Self::Rewind => serde_json::to_value(editor.rewind(version)).unwrap_or(Value::Null),
            Self::Toggle => serde_json::to_value(editor.toggle(version)).unwrap_or(Value::Null),
            Self::Stop => serde_json::to_value(editor.stop(version)).unwrap_or(Value::Null),
            Self::Clock(at) => json!({"text": editor.clock(*at)}),
            Self::Window(widget, ruler) => editor.window(*widget, *ruler),
            Self::SetWindow(window) => {
                editor.set_window(*window);
                json!({})
            }
            Self::Props(widget) => Value::Object(editor.props(*widget)),
            Self::Notes => json!({"placed": editor.placed_notes()}),
            Self::NotesView(view) => {
                let set = view.as_ref().is_none_or(|view| editor.set_notes_view(view));
                json!({"view": editor.notes_view(), "set": set})
            }
            Self::Event(event) => {
                serde_json::to_value(editor.event(event, version)).unwrap_or(Value::Null)
            }
            Self::Apply(payload) => {
                serde_json::to_value(editor.apply(payload)).unwrap_or(Value::Null)
            }
            Self::Resync => serde_json::to_value(editor.resync_all(version)).unwrap_or(Value::Null),
            Self::Span(span) => {
                editor.set_range(*span);
                json!({})
            }
            Self::Settle => serde_json::to_value(editor.settle(version)).unwrap_or(Value::Null),
            Self::Announce => serde_json::to_value(editor.announce(version)).unwrap_or(Value::Null),
            Self::Acknowledge(seq, reason) => {
                serde_json::to_value(editor.acknowledge(*seq, version, reason.clone()))
                    .unwrap_or(Value::Null)
            }
            Self::Unknown => json!({}),
        }
    }
}

impl<'de> Deserialize<'de> for Call {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Value::deserialize(deserializer).map(|value| Self::read(&value))
    }
}
