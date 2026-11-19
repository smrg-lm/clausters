//! **Quitting is the application's act, asked once** (`clausters_apps::quitting`).
//!
//! The close of the dock's window is one request, never a close per pane.
//! Who answers it:
//!
//! - **A client that said it quits the application** (`{"verb":
//!   "own_quit"}` on [`GUI_DOCK`]) is told [`GUI_DOCK_QUIT`], with what the
//!   applications this host holds would lose, and answers by quitting, or
//!   with `stay`. Its own documents and the host's are one question, which it
//!   asks; `save_all` saves the host's, answered once they are saved.
//! - **Else, the applications this host holds** are asked what they would
//!   lose: with nothing, everything closes; with something, the host asks one
//!   question over all of it (Don't save, Cancel, Save all) in a window of
//!   its own along the dock's edge, whose dialog stands over the dock.
//! - **Else** (a script's windows), the dock's panes close as each closes.
//!
//! As a window's `ask_close` does, a second close while the first is
//! unanswered ends it anyway: an owner that stopped answering cannot keep
//! the application open.

use serde_json::{Value, json};

use super::*;

/// **What the close of the dock's window does**, as [`Host::dock_close`]
/// decides it: the front carries it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockClose {
    /// Each pane closes as its own close does: no application asked.
    Panes,
    /// Every window closes, nothing asked: the application quits.
    All,
    /// The application was asked; its answer decides.
    Asked,
}

/// Who a quit under way is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuitFor {
    /// The client that quits the application.
    Client(ClientId),
    /// The host, over the applications it holds.
    Host,
}

/// **A quit under way.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Quitting {
    /// What the applications would lose is being gathered.
    Gathering(QuitFor),
    /// The question is up, or the client was told: unanswered.
    Asked(QuitFor),
    /// Save all is under way.
    #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
    Saving,
}

/// A dock call waiting on the applications' answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Waiting {
    Unsaved(ClientId, i32),
    SaveAll(ClientId, i32),
}

/// The structure the host's own question is numbered under.
#[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
const QUESTION: i64 = -7;

impl Host {
    /// **The dock's window was asked to close** -- by the desktop, or a
    /// script's `{"verb": "close"}`: what to do, by who said it answers.
    pub fn dock_close(&mut self, effects: &mut Vec<HostEffect>) -> DockClose {
        if matches!(self.quit, Some(Quitting::Gathering(_) | Quitting::Asked(_))) {
            // asked once and not answered: it goes anyway
            self.quit = None;
            self.drop_question(effects);
            return DockClose::All;
        }
        self.quit = None;
        if let Some(client) = self.quitter {
            self.gather(QuitFor::Client(client), effects);
            return DockClose::Asked;
        }
        if self.holds_applications() {
            self.gather(QuitFor::Host, effects);
            return DockClose::Asked;
        }
        DockClose::Panes
    }

    /// **What the applications would lose, gathered for `who`**: asked of
    /// their thread, or none at once where there are none.
    fn gather(&mut self, who: QuitFor, effects: &mut Vec<HostEffect>) {
        self.quit = Some(Quitting::Gathering(who));
        #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
        if self.holds_applications()
            && self
                .audio_host()
                .ask(super::audio_app::ToApp::Unsaved)
                .is_ok()
        {
            return;
        }
        self.unsaved_gathered(&[], effects);
    }

    /// Whether this host holds an application's window.
    fn holds_applications(&self) -> bool {
        #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
        {
            self.audio.as_ref().is_some_and(|audio| audio.has_windows())
        }
        #[cfg(all(target_arch = "wasm32", not(feature = "apps")))]
        false
    }

    /// **A dock call about quitting**, answered here rather than by the
    /// front's dock: `None` for any other verb. `Some(None)` is answered
    /// later, once the applications have.
    pub(super) fn quit_call(
        &mut self,
        call: &Value,
        from: ClientId,
        seq: i32,
        effects: &mut Vec<HostEffect>,
    ) -> Option<Option<Value>> {
        match call["verb"].as_str()? {
            "own_quit" => {
                self.quitter = Some(from);
                Some(Some(json!({"own_quit": true})))
            }
            "stay" => {
                self.quit = None;
                Some(Some(json!({"stay": true})))
            }
            "close" => {
                let close = self.dock_close(effects);
                effects.push(HostEffect::DockClose(close));
                Some(Some(json!({"close": format!("{close:?}").to_lowercase()})))
            }
            "unsaved" => {
                if !self.holds_applications() {
                    return Some(Some(json!({"unsaved": []})));
                }
                self.quit_waiting.push(Waiting::Unsaved(from, seq));
                #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
                let _ = self.audio_host().ask(super::audio_app::ToApp::Unsaved);
                Some(None)
            }
            "save_all" => {
                if !self.holds_applications() {
                    return Some(Some(json!({"saved": true})));
                }
                self.quit_waiting.push(Waiting::SaveAll(from, seq));
                #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
                let _ = self.audio_host().ask(super::audio_app::ToApp::SaveAll);
                Some(None)
            }
            _ => None,
        }
    }

    /// **What the applications would lose, as they said it**: the dock
    /// calls that waited are answered, and a quit gathering it goes on.
    pub(super) fn unsaved_gathered(
        &mut self,
        work: &[(i32, String)],
        effects: &mut Vec<HostEffect>,
    ) {
        let listed: Vec<Value> = work
            .iter()
            .map(|(window, title)| json!({"window": window, "title": title}))
            .collect();
        let waiting = std::mem::take(&mut self.quit_waiting);
        for w in waiting {
            match w {
                Waiting::Unsaved(to, seq) => effects.push(HostEffect::Send {
                    to,
                    msg: dock::call::reply(seq, Ok(json!({"unsaved": listed}))),
                }),
                other => self.quit_waiting.push(other),
            }
        }
        match self.quit {
            Some(Quitting::Gathering(QuitFor::Client(to))) => {
                self.quit = Some(Quitting::Asked(QuitFor::Client(to)));
                effects.push(HostEffect::Send {
                    to,
                    msg: OscMessage {
                        addr: GUI_DOCK_QUIT.into(),
                        args: vec![OscType::String(json!({"unsaved": listed}).to_string())],
                    },
                });
            }
            Some(Quitting::Gathering(QuitFor::Host)) if work.is_empty() => {
                self.quit = None;
                effects.push(HostEffect::DockClose(DockClose::All));
            }
            Some(Quitting::Gathering(QuitFor::Host)) => {
                self.quit = Some(Quitting::Asked(QuitFor::Host));
                let titles: Vec<&str> = work.iter().map(|(_, t)| t.as_str()).collect();
                self.ask_question(&titles, effects);
            }
            _ => {}
        }
    }

    /// **The applications' Save all ended**: `true` with nothing left to
    /// lose. The dock calls that waited are answered, and the host's own quit
    /// ends, or is called off.
    #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
    pub(super) fn saved_all(&mut self, saved: bool, effects: &mut Vec<HostEffect>) {
        let waiting = std::mem::take(&mut self.quit_waiting);
        for w in waiting {
            match w {
                Waiting::SaveAll(to, seq) => {
                    let answer = if saved {
                        Ok(json!({"saved": true}))
                    } else {
                        Err((
                            "runtime".to_string(),
                            "the saving was called off: there is work left to lose".to_string(),
                        ))
                    };
                    effects.push(HostEffect::Send {
                        to,
                        msg: dock::call::reply(seq, answer),
                    });
                }
                other => self.quit_waiting.push(other),
            }
        }
        if self.quit == Some(Quitting::Saving) {
            self.quit = None;
            if saved {
                effects.push(HostEffect::DockClose(DockClose::All));
            }
        }
    }

    /// **The host's question goes up**: one alert over the dock naming every
    /// document, in a window of the host's own along the dock's top edge.
    #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
    fn ask_question(&mut self, titles: &[&str], effects: &mut Vec<HostEffect>) {
        use clausters_apps::{closing, quitting};

        let ids = [
            self.own_widget(QUESTION, "quit", "window"),
            self.own_widget(QUESTION, "quit", "dialog"),
            self.own_widget(QUESTION, "quit", closing::SAVE),
            self.own_widget(QUESTION, "quit", closing::DISCARD),
            self.own_widget(QUESTION, "quit", closing::CANCEL),
        ];
        let [
            Some(window),
            Some(dialog),
            Some(save),
            Some(discard),
            Some(cancel),
        ] = ids
        else {
            return;
        };
        let ids = closing::Ids {
            stack: window,
            dialog,
            save,
            discard,
            cancel,
        };
        let form = quitting::dialog(Some(dialog), &quitting::says(titles), titles.len(), |n| {
            ids.button(n)
        });
        self.question = Some(ids);
        let mut def = json!({
            "type": "window", "id": window, "title": quitting::TITLE,
            "dock_edge": "top", "h": 1,
            "children": [{"type": "layout", "w": 0, "h": 0, "margin": 0, "children": [form]}],
        });
        self.number_own(&mut def, QUESTION, "quit");
        let origin = own_origin();
        effects.extend(self.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_DEF.into(),
                args: vec![OscType::Int(window), OscType::String(def.to_string())],
            }),
            origin,
        ));
    }

    #[cfg(all(target_arch = "wasm32", not(feature = "apps")))]
    fn ask_question(&mut self, _titles: &[&str], _effects: &mut Vec<HostEffect>) {}

    /// **An event of the host's own question**, which no client hears:
    /// `true` when it was one.
    pub(super) fn question_event(&mut self, def_id: i32, message: &OscMessage) -> bool {
        #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
        {
            let Some(ids) = self.question else {
                return false;
            };
            if def_id != ids.stack {
                return false;
            }
            let widget = match message.args.first() {
                Some(OscType::Int(w)) => i64::from(*w),
                _ => return true,
            };
            let tag = match message.args.get(3) {
                Some(OscType::String(t)) => t.as_str(),
                _ => "",
            };
            let mut effects = Vec::new();
            match ids.read(widget, tag) {
                Some(clausters_apps::closing::Choice::Discard) => {
                    self.quit = None;
                    self.drop_question(&mut effects);
                    effects.push(HostEffect::DockClose(DockClose::All));
                }
                Some(clausters_apps::closing::Choice::Cancel) => {
                    self.quit = None;
                    self.drop_question(&mut effects);
                }
                Some(clausters_apps::closing::Choice::Save) => {
                    self.drop_question(&mut effects);
                    self.quit = Some(Quitting::Saving);
                    let _ = self.audio_host().ask(super::audio_app::ToApp::SaveAll);
                }
                None => {}
            }
            self.pending_effects.extend(effects);
            true
        }
        #[cfg(all(target_arch = "wasm32", not(feature = "apps")))]
        {
            let _ = (def_id, message);
            false
        }
    }

    /// The host's question taken down, if it is up.
    fn drop_question(&mut self, effects: &mut Vec<HostEffect>) {
        #[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
        if let Some(ids) = self.question.take() {
            effects.extend(self.handle_packet(
                OscPacket::Message(OscMessage {
                    addr: GUI_FREE.into(),
                    args: vec![OscType::Int(ids.stack)],
                }),
                own_origin(),
            ));
        }
        #[cfg(all(target_arch = "wasm32", not(feature = "apps")))]
        let _ = effects;
    }
}

/// The origin of what the host sends itself.
#[cfg(any(not(target_arch = "wasm32"), feature = "apps"))]
fn own_origin() -> ClientId {
    use std::net::{Ipv4Addr, SocketAddr};
    ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
}
