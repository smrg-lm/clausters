//! The host as the **owner** of what it edits -- the third writer.
//!
//! Every gesture in this host emits an *intent* and waits for somebody to apply
//! it: a script drives the window, edits what it holds, and pushes back what
//! now stands. A host with **no language client at all** must be that
//! somebody itself, and this is the role: the editing context its own editors
//! are members of, which holds their one history.
//!
//! What it holds today is the score editors this host opens (`--score`). The
//! audio and the multitrack editors are not here: their application runs on a
//! thread of its own, with its own context and its own engine
//! (`audio_app.rs`), which is where a window this host owns is answered.

#[cfg(feature = "notation")]
use std::collections::HashMap;

use clausters_apps::editing::Editing;
#[cfg(feature = "notation")]
use clausters_apps::editing::MemberId;

/// **What this host edits on its own**: its editing context, and the windows
/// of it that are open.
#[derive(Debug)]
pub struct Owner {
    /// **The editing context**: one history, the applications crate's, as a
    /// client's is.
    pub editing: Editing,
    /// **The score editors open here**, by the window each is in: its member
    /// of [`Owner::editing`], and the file Ctrl+S writes the score to.
    #[cfg(feature = "notation")]
    pub scores: HashMap<i32, (MemberId, Option<std::path::PathBuf>)>,
}

impl Default for Owner {
    fn default() -> Self {
        Self::new()
    }
}

impl Owner {
    /// An owner with nothing open.
    pub fn new() -> Self {
        Self {
            editing: Editing::new(),
            #[cfg(feature = "notation")]
            scores: HashMap::new(),
        }
    }
}

/// An OSC atom as the JSON a reading, or an editor's turn, takes.
///
/// The wire's framing is the host's and the reading is the crate's, so this is
/// where the one becomes the other -- the same line each client draws for its own
/// transport.
pub(crate) fn atom(value: &clausters_core::osc::OscType) -> serde_json::Value {
    use clausters_core::osc::OscType;
    match value {
        OscType::String(s) => serde_json::Value::String(s.clone()),
        OscType::Float(v) => serde_json::json!(f64::from(*v)),
        OscType::Double(v) => serde_json::json!(*v),
        OscType::Int(v) => serde_json::json!(*v),
        OscType::Long(v) => serde_json::json!(*v),
        OscType::Bool(v) => serde_json::json!(*v),
        _ => serde_json::Value::Null,
    }
}
