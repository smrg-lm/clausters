//! Multitrack calls decoded once at the application's boundary.
use serde::Deserialize;
use serde_json::Value;

use crate::turn::Event;
use clausters_document::multitrack::{edit::MultitrackIntent, handle::Request};

pub(super) struct Action {
    pub kind: Kind,
    // Missing keeps the current destination; explicit null is read-only.
    pub save_to: Option<Option<String>>,
}

impl Action {
    pub fn read(value: &Value) -> Result<Self, String> {
        Ok(Self {
            kind: serde_json::from_value(value.clone()).map_err(|e| e.to_string())?,
            save_to: value
                .get("saveTo")
                .map(|path| path.as_str().map(str::to_string)),
        })
    }
}

#[derive(Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub(super) enum Kind {
    Open {
        session: Option<String>,
    },
    Sound,
    Save {
        path: Option<String>,
    },
    Retry,
    Event(Event),
    Verb {
        call: crate::multitrack::calls::Call,
    },
    Handle(Handle),
    Play {
        range: Option<[f64; 2]>,
        #[serde(default)]
        looping: bool,
    },
    Pause,
    Stop {
        #[serde(default)]
        mark: f64,
    },
    Locate {
        #[serde(default)]
        at: f64,
    },
    Playing,
    Undo,
    Redo,
    Close,
    Closed,
}

#[derive(Deserialize)]
#[serde(try_from = "RawHandle")]
pub(super) struct Handle {
    pub call: HandleCall,
    pub label: String,
    pub record: bool,
}

pub(super) enum HandleCall {
    Record(Vec<RecordLeg>),
    Request(Request),
}

#[derive(Deserialize)]
pub(super) struct RecordLeg {
    pub forward: MultitrackIntent,
    pub backward: MultitrackIntent,
}

#[derive(Deserialize)]
struct RawHandle {
    call: Value,
    #[serde(default = "edit_label")]
    pub label: String,
    #[serde(default = "yes")]
    pub record: bool,
}

fn edit_label() -> String {
    "edit".into()
}
fn yes() -> bool {
    true
}

impl TryFrom<RawHandle> for Handle {
    type Error = String;

    fn try_from(raw: RawHandle) -> Result<Self, Self::Error> {
        let call = if raw.call["verb"] == "record" {
            HandleCall::Record(
                serde_json::from_value(raw.call["legs"].clone()).map_err(|e| e.to_string())?,
            )
        } else {
            HandleCall::Request(serde_json::from_value(raw.call).map_err(|e| e.to_string())?)
        };
        Ok(Self {
            call,
            label: raw.label,
            record: raw.record,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_omitted_save_destination_does_not_mean_read_only() {
        assert_eq!(Action::read(&json!({"verb":"open"})).unwrap().save_to, None);
        assert_eq!(
            Action::read(&json!({"verb":"open", "saveTo":null}))
                .unwrap()
                .save_to,
            Some(None)
        );
        assert_eq!(
            Action::read(&json!({"verb":"open", "saveTo":"session.json"}))
                .unwrap()
                .save_to,
            Some(Some("session.json".into()))
        );
    }
}
