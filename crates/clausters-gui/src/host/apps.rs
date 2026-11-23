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

/// **Who an open answers**, and whether it keeps what the window edits:
/// the `keeps` option, a handler's word that it takes its structure back
/// when the window goes. Without it the window is the work's only holder and
/// asks before a close loses it ([`clausters_apps::closing`]).
#[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
fn opener(from: ClientId, seq: i32, options: &Value) -> super::audio_app::Reply {
    let keeps = options["keeps"].as_bool().unwrap_or(false);
    super::audio_app::Reply::Client(from, seq, keeps)
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
            "multitrack" => self.open_app_multitrack(seq, &options, from, effects),
            #[cfg(all(
                feature = "notation",
                any(not(target_arch = "wasm32"), feature = "apps")
            ))]
            "score" => self.open_app_score(seq, &options, from, effects),
            #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
            "notes" => self.open_app_notes(seq, &options, from, effects),
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
        let chrome = options["chrome"].as_bool().unwrap_or(true);
        // answered once the application opened the file, or could not
        let reply = opener(from, seq, options);
        let opened = self.open_app_dressed(
            super::audio_app::AppKind::Audio,
            path,
            None,
            title,
            Some(reply),
            None,
            chrome,
        );
        if let Err(message) = opened {
            effects.push(HostEffect::Reply(app_reply(
                seq,
                Err(("value".into(), message)),
            )));
        }
    }

    /// **The multitrack editor opened for a client**, over the session its
    /// options name (`path`), over a multitrack they hand over whole
    /// (`multitrack`, an untitled session), or over none; or why not.
    #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
    fn open_app_multitrack(
        &mut self,
        seq: i32,
        options: &Value,
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let path = options["path"].as_str();
        let title = options["title"].as_str().unwrap_or("Multitrack");
        let chrome = options["chrome"].as_bool().unwrap_or(true);
        let reply = opener(from, seq, options);
        use super::audio_app::{AppData, AppKind};
        // a multitrack handed over whole is an untitled session; a path, the
        // session at it
        let data = if let Some(snapshot) = options.get("snapshot") {
            match serde_json::from_value(snapshot.clone()) {
                Ok(snapshot) => Ok(Some(AppData::Snapshot(Box::new(snapshot)))),
                Err(e) => Err(e.to_string()),
            }
        } else {
            match (&options["multitrack"], path) {
                (Value::Object(_), None) => {
                    Ok(Some(AppData::Contents(options["multitrack"].clone())))
                }
                _ => Ok(None),
            }
        };
        let opened = data.and_then(|data| {
            let path = if data.is_some() { None } else { path };
            self.open_app_dressed(
                AppKind::Multitrack,
                path,
                data,
                title,
                Some(reply),
                None,
                chrome,
            )
        });
        if let Err(message) = opened {
            effects.push(HostEffect::Reply(app_reply(
                seq,
                Err(("value".into(), message)),
            )));
        }
    }

    /// **A roll opened for a client**, over a sequence its options hand over
    /// (`sequence`) or the one a window it holds shows (`over`), or why not.
    /// The rest of the options are how the roll opens (`domain`, `editable`,
    /// `midi_in`, `chrome`, `w`, `h`).
    #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
    fn open_app_notes(
        &mut self,
        seq: i32,
        options: &Value,
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let reply = opener(from, seq, options);
        let over = options["over"].as_i64().map(|over| over as i32);
        let opened = self.open_app_notes_for(
            over,
            options.clone(),
            options["title"].as_str().unwrap_or("Notes"),
            Some(reply),
        );
        if let Err(message) = opened {
            effects.push(refused(seq, message));
        }
    }

    /// **The score editor opened for a client**, over the file its options
    /// name (`path`), a document they hand over (`mei`), or a blank page, or
    /// over a sequence (`sequence`, or `over`, a window showing one), read as
    /// `how` and `interp` say; or why not. The rest of the options are how
    /// it opens (`value`, `chrome`, `w`, `h`).
    #[cfg(all(
        feature = "notation",
        any(not(target_arch = "wasm32"), feature = "apps")
    ))]
    fn open_app_score(
        &mut self,
        seq: i32,
        options: &Value,
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        let reply = opener(from, seq, options);
        let opened = self.open_app_score_for(
            options["path"].as_str(),
            options["mei"].as_str().map(str::to_string),
            options["over"].as_i64().map(|over| over as i32),
            options.clone(),
            options["title"].as_str().unwrap_or("Score"),
            Some(reply),
        );
        if let Err(message) = opened {
            effects.push(refused(seq, message));
        }
    }

    /// The applications' build is not this page's.
    #[cfg(all(target_arch = "wasm32", not(feature = "apps")))]
    fn open_app_multitrack(
        &mut self,
        seq: i32,
        options: &Value,
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        self.open_app_audio(seq, options, from, effects)
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
