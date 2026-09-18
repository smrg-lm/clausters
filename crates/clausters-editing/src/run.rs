//! **Carrying steps out**: the messages that may go out now, and what a reply
//! releases.
//!
//! [`Applier`](crate::apply::Applier) and
//! [`MultitrackPlayback`](crate::playback::MultitrackPlayback) answer [`Step`]s -- a
//! message, a `/done` the rest waits for, a barrier -- and say nothing about how
//! they are walked. Every endpoint walked them its own way: the Python client
//! paired a send with a blocking request, the web client awaited, and the GUI
//! host kept a queue with a reply path of its own. Three walks of one list, and
//! the host's had nowhere to put a session's open, so a join's stitch went out
//! before the reads it is made of.
//!
//! So the walk is here, as data: steps in, the messages that may leave, and a
//! reply offered to what the queue is held behind. It opens no socket and waits
//! on nothing -- a script blocks on the reply, a page awaits it, a host resumes
//! when it arrives, and each hands it back the same way.
//!
//! # Two servers
//!
//! A step names the [`Server`] it goes to, and a reply the one it came from. A
//! client has one server and names [`Server::Sound`] throughout; the standalone
//! host has two -- a session that holds the samples and a player that sounds
//! them -- and a join is made in the first and attached in the second, in that
//! order. The queue is **one**, across both: order is what the steps state, and
//! a wait holds everything after it wherever it goes.

use std::collections::VecDeque;

use clausters_core::osc::{OscMessage, OscType};

use crate::apply::Step;

/// **Which server a step goes to**, or a reply came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Server {
    /// The server that makes sound: every step of a client, and the player of a
    /// host that has one.
    Sound,
    /// The server that holds the samples, where it is not the one that sounds:
    /// a standalone host's in-process session.
    Samples,
}

/// **What a reply did to the queue.**
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// Not what the queue is waiting on.
    Unrelated,
    /// The awaited answer: what was held behind it may go out.
    Released,
    /// A `/fail` of the awaited command. It releases the queue too -- a queue
    /// held forever behind a refusal never sounds again -- and carries the
    /// refusal's arguments so the endpoint can say why.
    Refused(Vec<OscType>),
}

/// **Steps not carried out yet**, in order, and the one they are held behind.
#[derive(Debug, Default, Clone)]
pub struct Runner {
    queue: VecDeque<(Server, Step)>,
    awaiting: Option<(Server, Step)>,
}

impl Runner {
    /// A runner holding nothing.
    pub fn new() -> Runner {
        Runner::default()
    }

    /// Queues `steps` for `to`, after everything already queued.
    pub fn push(&mut self, to: Server, steps: impl IntoIterator<Item = Step>) {
        self.queue.extend(steps.into_iter().map(|step| (to, step)));
    }

    /// **The messages that may go out now**, each with its server: everything
    /// up to the next step that waits. A barrier is sent and then holds the
    /// rest; an await holds them without sending anything.
    pub fn ready(&mut self) -> Vec<(Server, OscMessage)> {
        let mut out = Vec::new();
        while self.awaiting.is_none() {
            match self.queue.pop_front() {
                None => break,
                Some((to, Step::Send(message))) => out.push((to, message)),
                Some((to, Step::Sync(id))) => {
                    out.push((
                        to,
                        OscMessage {
                            addr: "/server_sync".into(),
                            args: vec![OscType::Int(id)],
                        },
                    ));
                    self.awaiting = Some((to, Step::Sync(id)));
                }
                Some((to, wait @ Step::AwaitDone { .. })) => self.awaiting = Some((to, wait)),
            }
        }
        out
    }

    /// **A reply from `from`, offered to the step the queue is held behind.**
    /// Only the server the wait was addressed to can release it.
    pub fn reply(&mut self, from: Server, msg: &OscMessage) -> Reply {
        let Some((to, step)) = &self.awaiting else {
            return Reply::Unrelated;
        };
        if *to != from {
            return Reply::Unrelated;
        }
        let names =
            |command: &str| matches!(msg.args.first(), Some(OscType::String(c)) if c == command);
        let answer = match (step, msg.addr.as_str()) {
            (Step::Sync(id), "/server_sync.reply")
                if msg.args.first() == Some(&OscType::Int(*id)) =>
            {
                Reply::Released
            }
            (Step::AwaitDone { command, index }, "/done")
                if names(command)
                    && index.is_none_or(|index| msg.args.get(1) == Some(&OscType::Int(index))) =>
            {
                Reply::Released
            }
            (Step::AwaitDone { command, .. }, "/fail") if names(command) => {
                Reply::Refused(msg.args.clone())
            }
            _ => Reply::Unrelated,
        };
        if answer != Reply::Unrelated {
            self.awaiting = None;
        }
        answer
    }

    /// The step the queue is held behind, and the server it waits on.
    pub fn awaiting(&self) -> Option<(Server, &Step)> {
        self.awaiting.as_ref().map(|(to, step)| (*to, step))
    }

    /// Whether everything pushed has gone out and nothing is waited on.
    pub fn is_idle(&self) -> bool {
        self.awaiting.is_none() && self.queue.is_empty()
    }
}

/// The name a server goes by over JSON.
fn server_name(server: Server) -> &'static str {
    match server {
        Server::Sound => "sound",
        Server::Samples => "samples",
    }
}

/// **The runner over JSON**, for a client that binds it rather than links it.
///
/// `request` names a `verb`:
///
/// - `push` — `to` (`"sound"` or `"samples"`, sound when absent) and `steps`,
///   in the shape a playback answers them ([`crate::apply::steps_json`]).
///   Answers `{}`.
/// - `ready` — answers `messages`, each `{"to", "addr", "args"}` with the
///   arguments tagged as a step's are, and `awaiting`: `null`, or `{"from",
///   "step"}` for the step the queue is now held behind. **When something is
///   awaited, the last message is the one it waits on** -- the barrier itself,
///   or the command a `/done` answers -- which is what lets a client that
///   pairs a send with its reply send that one last.
/// - `reply` — `from`, `addr` and `args` (tagged): answers `{"reply":
///   "released"}`, `{"reply": "unrelated"}`, or `{"reply": "refused", "args"}`.
/// - `idle` — answers `{"idle": bool}`.
///
/// A verb it does not know answers `{"error": ...}`.
pub fn call_json(run: &mut Runner, request: &str) -> String {
    use crate::apply::{arg_from_json, arg_json, steps_from_json, steps_json};
    use serde_json::{Value, json};

    let request: Value = serde_json::from_str(request).unwrap_or(Value::Null);
    let server = |key: &str| match request.get(key).and_then(Value::as_str) {
        Some("samples") => Server::Samples,
        _ => Server::Sound,
    };
    let answer = match request.get("verb").and_then(Value::as_str) {
        Some("push") => {
            run.push(
                server("to"),
                steps_from_json(request.get("steps").unwrap_or(&Value::Null)),
            );
            json!({})
        }
        Some("ready") => {
            let messages: Vec<Value> = run
                .ready()
                .into_iter()
                .map(|(to, message)| {
                    json!({
                        "to": server_name(to),
                        "addr": message.addr,
                        "args": message.args.iter().map(arg_json).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let awaiting = run.awaiting().map(|(from, step)| {
                json!({
                    "from": server_name(from),
                    "step": steps_json(std::slice::from_ref(step))[0].clone(),
                })
            });
            json!({ "messages": messages, "awaiting": awaiting })
        }
        Some("reply") => {
            let message = OscMessage {
                addr: request
                    .get("addr")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                args: request
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|args| args.iter().filter_map(arg_from_json).collect())
                    .unwrap_or_default(),
            };
            match run.reply(server("from"), &message) {
                Reply::Released => json!({ "reply": "released" }),
                Reply::Unrelated => json!({ "reply": "unrelated" }),
                Reply::Refused(args) => json!({
                    "reply": "refused",
                    "args": args.iter().map(arg_json).collect::<Vec<_>>(),
                }),
            }
        }
        Some("idle") => json!({ "idle": run.is_idle() }),
        other => json!({ "error": format!("the runner has no verb {other:?}") }),
    };
    answer.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The JSON door walks the same queue**: a playback's own steps pushed
    /// in, the barrier as the last message out, and its reply releasing the
    /// rest.
    #[test]
    fn the_json_door_walks_a_barrier() {
        let mut run = Runner::new();
        let steps = crate::apply::steps_json(&[
            send("/def_send", vec![OscType::String("synth".into())]),
            Step::Sync(3),
            send("/graph_new", vec![OscType::Int(1000)]),
        ]);
        let pushed = call_json(
            &mut run,
            &serde_json::json!({"verb": "push", "steps": steps}).to_string(),
        );
        assert_eq!(pushed, "{}");
        let ready: serde_json::Value =
            serde_json::from_str(&call_json(&mut run, r#"{"verb":"ready"}"#)).unwrap();
        let messages = ready["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["addr"], "/server_sync", "the awaited one last");
        assert_eq!(ready["awaiting"]["step"]["sync"], 3);
        assert_eq!(ready["awaiting"]["from"], "sound");
        let reply = call_json(
            &mut run,
            r#"{"verb":"reply","from":"sound","addr":"/server_sync.reply","args":[{"i":3}]}"#,
        );
        assert_eq!(reply, r#"{"reply":"released"}"#);
        let rest: serde_json::Value =
            serde_json::from_str(&call_json(&mut run, r#"{"verb":"ready"}"#)).unwrap();
        assert_eq!(rest["messages"][0]["addr"], "/graph_new");
        assert_eq!(rest["messages"][0]["args"][0]["i"], 1000);
        assert!(rest["awaiting"].is_null());
        assert_eq!(
            call_json(&mut run, r#"{"verb":"idle"}"#),
            r#"{"idle":true}"#
        );
    }

    fn send(addr: &str, args: Vec<OscType>) -> Step {
        Step::Send(OscMessage {
            addr: addr.into(),
            args,
        })
    }

    fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
        OscMessage {
            addr: addr.into(),
            args,
        }
    }

    fn done(command: &str, index: i32) -> OscMessage {
        msg(
            "/done",
            vec![OscType::String(command.into()), OscType::Int(index)],
        )
    }

    fn addrs(out: &[(Server, OscMessage)]) -> Vec<(Server, &str)> {
        out.iter().map(|(s, m)| (*s, m.addr.as_str())).collect()
    }

    /// **A barrier goes out and holds the rest**, until its own reply.
    #[test]
    fn a_barrier_holds_what_follows_it() {
        let mut run = Runner::new();
        run.push(
            Server::Sound,
            [
                send("/def_send", vec![]),
                Step::Sync(7),
                send("/graph_new", vec![]),
            ],
        );
        assert_eq!(
            addrs(&run.ready()),
            [
                (Server::Sound, "/def_send"),
                (Server::Sound, "/server_sync")
            ]
        );
        assert!(run.ready().is_empty(), "held");
        let other = msg("/server_sync.reply", vec![OscType::Int(8)]);
        assert_eq!(run.reply(Server::Sound, &other), Reply::Unrelated);
        let own = msg("/server_sync.reply", vec![OscType::Int(7)]);
        assert_eq!(run.reply(Server::Sound, &own), Reply::Released);
        assert_eq!(addrs(&run.ready()), [(Server::Sound, "/graph_new")]);
        assert!(run.is_idle());
    }

    /// **An await names its command and, where it has one, its index**: a
    /// buffer is filled after that very buffer exists.
    #[test]
    fn an_await_is_released_by_its_own_done() {
        let mut run = Runner::new();
        run.push(
            Server::Sound,
            [
                send("/buffer_alloc", vec![OscType::Int(3)]),
                Step::AwaitDone {
                    command: "/buffer_alloc".into(),
                    index: Some(3),
                },
                send("/buffer_setRange", vec![]),
            ],
        );
        assert_eq!(addrs(&run.ready()), [(Server::Sound, "/buffer_alloc")]);
        assert_eq!(
            run.reply(Server::Sound, &done("/buffer_alloc", 4)),
            Reply::Unrelated
        );
        assert_eq!(
            run.reply(Server::Sound, &done("/buffer_free", 3)),
            Reply::Unrelated
        );
        assert_eq!(
            run.reply(Server::Sound, &done("/buffer_alloc", 3)),
            Reply::Released
        );
        assert_eq!(addrs(&run.ready()), [(Server::Sound, "/buffer_setRange")]);
    }

    /// **A refusal releases the queue and says why.**
    #[test]
    fn a_refused_command_releases_the_queue_with_its_reason() {
        let mut run = Runner::new();
        run.push(
            Server::Sound,
            [
                send("/transport_play", vec![]),
                Step::AwaitDone {
                    command: "/transport_play".into(),
                    index: None,
                },
                send("/node_set", vec![]),
            ],
        );
        run.ready();
        let fail = msg(
            "/fail",
            vec![
                OscType::String("/transport_play".into()),
                OscType::String("no".into()),
            ],
        );
        assert!(matches!(
            run.reply(Server::Sound, &fail),
            Reply::Refused(args) if args.len() == 2
        ));
        assert_eq!(addrs(&run.ready()), [(Server::Sound, "/node_set")]);
    }

    /// **A join waits for its reads where the samples are, and is attached
    /// where the multitrack sounds**: one queue across two servers, and only the
    /// server a wait was sent to can release it.
    #[test]
    fn one_queue_orders_steps_across_two_servers() {
        let mut run = Runner::new();
        for bufnum in [1, 2] {
            run.push(
                Server::Samples,
                [
                    send("/buffer_allocRead", vec![OscType::Int(bufnum)]),
                    Step::AwaitDone {
                        command: "/buffer_allocRead".into(),
                        index: Some(bufnum),
                    },
                ],
            );
        }
        run.push(
            Server::Samples,
            [
                send("/buffer_stitch", vec![OscType::Int(5)]),
                Step::AwaitDone {
                    command: "/buffer_stitch".into(),
                    index: Some(5),
                },
            ],
        );
        run.push(
            Server::Sound,
            [send("/buffer_attach", vec![OscType::Int(5)])],
        );

        assert_eq!(
            addrs(&run.ready()),
            [(Server::Samples, "/buffer_allocRead")]
        );
        assert_eq!(
            run.reply(Server::Sound, &done("/buffer_allocRead", 1)),
            Reply::Unrelated,
            "the player did not read it"
        );
        assert_eq!(
            run.reply(Server::Samples, &done("/buffer_allocRead", 1)),
            Reply::Released
        );
        assert_eq!(
            addrs(&run.ready()),
            [(Server::Samples, "/buffer_allocRead")]
        );
        run.reply(Server::Samples, &done("/buffer_allocRead", 2));
        assert_eq!(
            addrs(&run.ready()),
            [(Server::Samples, "/buffer_stitch")],
            "the stitch after both reads, never before"
        );
        run.reply(Server::Samples, &done("/buffer_stitch", 5));
        assert_eq!(addrs(&run.ready()), [(Server::Sound, "/buffer_attach")]);
        assert!(run.is_idle());
    }
}
