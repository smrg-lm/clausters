//! **The door a client opens and calls an application through**
//! ([`GUI_APP_OPEN`], [`GUI_APP_CALL`]): an application this host owns -- the
//! audio editor -- is opened in a window of the host's, and a client is its
//! handler, asking it what the window would and reading what it answers.
//!
//! The client draws nothing and carries nothing out: what a call does is the
//! application's, in this process, and what comes back is its answer
//! ([`GUI_APP_REPLY`]). An action that renders is answered when its render
//! lands, which is a later tick than the packet that asked, so that answer is
//! sent to its client by name ([`HostEffect::Send`]).

use serde_json::{Value, json};

use super::wire::{int_arg, string_arg};
use super::*;

/// **An application's answer**, as [`GUI_APP_REPLY`] carries it.
pub(super) fn app_reply(seq: i32, answer: Result<Value, (String, String)>) -> OscMessage {
    let doc = match answer {
        Ok(value) => json!({"ok": value}),
        Err((kind, message)) => json!({"error": {"type": kind, "message": message}}),
    };
    OscMessage {
        addr: GUI_APP_REPLY.into(),
        args: vec![OscType::Int(seq), OscType::String(doc.to_string())],
    }
}

/// A request the door could not take, as an answer of kind `value`.
fn refused(seq: i32, message: impl Into<String>) -> HostEffect {
    HostEffect::Reply(app_reply(seq, Err(("value".into(), message.into()))))
}

impl Host {
    /// `/gui_app_open <seq> <app> <json>` ([`GUI_APP_OPEN`]).
    pub(super) fn on_app_open(
        &mut self,
        args: &[OscType],
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let (Some(seq), Some(app)) = (int_arg(args, 0), string_arg(args, 1)) else {
            return diag::warn!("{from}: {GUI_APP_OPEN} needs a seq and an application");
        };
        let options: Value = match string_arg(args, 2).map(serde_json::from_str) {
            None => json!({}),
            Some(Ok(options @ Value::Object(_))) => options,
            Some(_) => return effects.push(refused(seq, "the options are not a JSON object")),
        };
        match app {
            "audio" => self.open_app_audio(seq, &options, from, effects),
            other => effects.push(refused(seq, format!("no application named {other:?}"))),
        }
    }

    /// `/gui_app_call <seq> <window> <json>` ([`GUI_APP_CALL`]).
    pub(super) fn on_app_call(
        &mut self,
        args: &[OscType],
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let (Some(seq), Some(window)) = (int_arg(args, 0), int_arg(args, 1)) else {
            return diag::warn!("{from}: {GUI_APP_CALL} needs a seq and a window");
        };
        let call: Value = match string_arg(args, 2).map(serde_json::from_str) {
            Some(Ok(call @ Value::Object(_))) => call,
            _ => return effects.push(refused(seq, "the call is not a JSON object")),
        };
        #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
        if self.owns_audio(window) {
            return self.call_app_audio(seq, window, &call, from, effects);
        }
        let _ = call;
        effects.push(HostEffect::Reply(app_reply(
            seq,
            Err((
                "runtime".into(),
                format!("window {window} holds no application"),
            )),
        )));
    }

    /// The audio editor opened for a client, or why not.
    #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
    fn open_app_audio(
        &mut self,
        seq: i32,
        options: &Value,
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let path = options["path"].as_str();
        let title = options["title"].as_str().unwrap_or("Audio");
        // answered once the application opened the file, or could not
        let reply = super::audio_app::Reply::Client(from, seq);
        if let Err(message) = self.open_audio_for(path, title, Some(reply)) {
            effects.push(HostEffect::Reply(app_reply(
                seq,
                Err(("value".into(), message)),
            )));
        }
    }

    /// The ordinary build of a page's host holds no application: the
    /// applications' build does, in a page that is cross-origin isolated.
    #[cfg(all(target_arch = "wasm32", not(feature = "apps")))]
    fn open_app_audio(
        &mut self,
        seq: i32,
        _options: &Value,
        _from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        effects.push(HostEffect::Reply(app_reply(
            seq,
            Err((
                "runtime".into(),
                "this page is not cross-origin isolated: its host holds no audio editor".into(),
            )),
        )));
    }
}
