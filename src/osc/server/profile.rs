//! `/server_profile`: where the server's time goes, in detail.
//!
//! `/server_load` says which **role** spent the time -- the block, a worker,
//! the serving turn, the NRT queue. A profile says what inside it: which phase
//! of the block, which synth and which def, which command the serving turn
//! was handling, what each reply and each transfer carried, and how long each
//! NRT job ran and waited. It is what is read when a session is slow and the
//! question is *why*, without anybody watching the server while it happens.
//!
//! **Cumulative, like the role table.** Every figure only grows, and a client
//! differences two reports into a window of its own, so two clients profiling
//! at once each read their own interval. The one switch is the per-synth
//! timing (`node::NodeCosts`), which costs the audio thread two clock reads
//! per synth per block and so runs only while some client watches; the rest
//! costs a clock read or a table update per message, per reply and per job,
//! all on threads that are not the audio thread, and is always on.
//!
//! **The network thread's own tables live here**: commands received, replies
//! sent, NRT jobs finished and the synths that left the tree while profiled.
//! The phases come from the role table and the live synths from the node
//! cost table; [`NetProfile::report`] puts the three together.

use std::cell::RefCell;
use std::collections::HashMap;

use rosc::{OscMessage, OscType};

use crate::node::NodeCost;
use crate::osc::ClientId;
use crate::server::meters::Meters;

/// How many synths a report lists, costliest first. Every def is listed, so
/// what the cap leaves out is still in its def's row; the cap keeps the reply
/// inside one datagram for a tree of thousands of synths.
pub(super) const MAX_NODE_ROWS: usize = 256;

/// What one address cost: the time spent on it (commands only), how many
/// times it went by and how many bytes it carried.
#[derive(Clone, Copy, Debug, Default)]
struct Traffic {
    busy_nanos: u64,
    count: u64,
    bytes: u64,
}

/// What one kind of NRT job cost.
#[derive(Clone, Copy, Debug, Default)]
struct JobCost {
    busy_nanos: u64,
    count: u64,
    waited_nanos: u64,
}

/// What one def's synths cost.
#[derive(Clone, Copy, Debug, Default)]
struct DefCost {
    busy_nanos: u64,
    runs: u64,
    nodes: u64,
}

/// The def a synth's cost goes to when the server no longer knows which it
/// was.
const UNKNOWN_DEF: &str = "?";

#[derive(Debug, Default)]
pub(super) struct NetProfile {
    commands: HashMap<String, Traffic>,
    /// Behind a cell because replies leave through `&self`.
    replies: RefCell<HashMap<String, Traffic>>,
    jobs: HashMap<&'static str, JobCost>,
    /// The synths that left the tree having run while profiled, by def.
    retired: HashMap<Box<str>, DefCost>,
    /// The clients that asked for the per-synth timing.
    watchers: Vec<ClientId>,
}

/// Adds to the row of `key`, allocating the key only the first time.
fn row<'a, V: Default>(table: &'a mut HashMap<String, V>, key: &str) -> &'a mut V {
    if !table.contains_key(key) {
        table.insert(key.to_owned(), V::default());
    }
    table.get_mut(key).expect("inserted just above")
}

impl NetProfile {
    /// The serving turn spent `nanos` on a message to `addr` of `bytes`.
    pub(super) fn command(&mut self, addr: &str, nanos: u64, bytes: usize) {
        let traffic = row(&mut self.commands, addr);
        traffic.busy_nanos += nanos;
        traffic.count += 1;
        traffic.bytes += bytes as u64;
    }

    /// A reply to `addr` of `bytes` left.
    pub(super) fn reply(&self, addr: &str, bytes: usize) {
        let mut replies = self.replies.borrow_mut();
        let traffic = row(&mut replies, addr);
        traffic.count += 1;
        traffic.bytes += bytes as u64;
    }

    /// An NRT job for `cmd` finished.
    pub(super) fn job(&mut self, cmd: &'static str, busy_nanos: u64, waited_nanos: u64) {
        let cost = self.jobs.entry(cmd).or_default();
        cost.busy_nanos += busy_nanos;
        cost.count += 1;
        cost.waited_nanos += waited_nanos;
    }

    /// A synth of `def` left the tree having cost `cost`. One that never ran
    /// while profiled adds nothing, so it is not counted as a node either.
    pub(super) fn retire(&mut self, def: Option<Box<str>>, cost: NodeCost) {
        if cost.runs == 0 {
            return;
        }
        let def = def.unwrap_or_else(|| UNKNOWN_DEF.into());
        let row = self.retired.entry(def).or_default();
        row.busy_nanos += cost.busy_nanos;
        row.runs += cost.runs;
        row.nodes += 1;
    }

    /// Adds `client` to the watchers. Returns whether anybody watches.
    pub(super) fn watch(&mut self, client: ClientId) -> bool {
        if !self.watchers.contains(&client) {
            self.watchers.push(client);
        }
        true
    }

    /// Drops every watcher `gone` names. Returns whether anybody still
    /// watches.
    pub(super) fn unwatch(&mut self, gone: impl Fn(&ClientId) -> bool) -> bool {
        self.watchers.retain(|c| !gone(c));
        !self.watchers.is_empty()
    }

    /// The arguments of `/server_profile.reply` (`docs/schemas.md`):
    ///
    /// `uptime, profiling,`
    /// `n, n x (role, phase, busy, calls),`
    /// `n, n x (node, def, busy, runs),`
    /// `n, n x (def, busy, runs, nodes),`
    /// `n, n x (address, busy, count, bytes),`
    /// `n, n x (address, count, bytes),`
    /// `n, n x (command, busy, count, waited)`
    ///
    /// -- the phases, the live synths, the defs, the commands received, the
    /// replies sent and the NRT jobs. Seconds since boot, nil where the server
    /// has no clock; counts and bytes since boot.
    pub(super) fn report(
        &self,
        meters: &Meters,
        profiling: bool,
        nodes: &[NodeCost],
        def_of: impl Fn(i32) -> Option<String>,
    ) -> Vec<OscType> {
        let timed = meters.timed();
        let seconds = |nanos: u64| {
            if timed {
                OscType::Double(nanos as f64 / 1e9)
            } else {
                OscType::Nil
            }
        };
        let count = |n: u64| OscType::Long(n as i64);
        let rows = |n: usize| OscType::Int(n as i32);
        let mut args = vec![
            if timed {
                OscType::Double(meters.uptime())
            } else {
                OscType::Nil
            },
            OscType::Int(i32::from(profiling)),
        ];

        let phases = meters.phases();
        args.push(rows(phases.len()));
        for phase in phases {
            args.push(OscType::String(phase.phase.role().as_str().into()));
            args.push(OscType::String(phase.phase.as_str().into()));
            args.push(if timed {
                OscType::Double(phase.busy)
            } else {
                OscType::Nil
            });
            args.push(count(phase.calls));
        }

        // The live synths, named, and their defs' rows beside the retired.
        let mut defs: HashMap<Box<str>, DefCost> = self.retired.clone();
        let mut live: Vec<(NodeCost, String)> = nodes
            .iter()
            .filter(|cost| cost.runs > 0)
            .map(|cost| {
                let def = def_of(cost.id).unwrap_or_else(|| UNKNOWN_DEF.into());
                let row = defs.entry(def.as_str().into()).or_default();
                row.busy_nanos += cost.busy_nanos;
                row.runs += cost.runs;
                row.nodes += 1;
                (*cost, def)
            })
            .collect();
        live.sort_by(|a, b| {
            (b.0.busy_nanos, b.0.runs, a.0.id).cmp(&(a.0.busy_nanos, a.0.runs, b.0.id))
        });
        live.truncate(MAX_NODE_ROWS);
        args.push(rows(live.len()));
        for (cost, def) in live {
            args.push(OscType::Int(cost.id));
            args.push(OscType::String(def));
            args.push(seconds(cost.busy_nanos));
            args.push(count(cost.runs));
        }

        let mut defs: Vec<_> = defs.into_iter().collect();
        defs.sort_by(|a, b| {
            (b.1.busy_nanos, b.1.runs, &a.0).cmp(&(a.1.busy_nanos, a.1.runs, &b.0))
        });
        args.push(rows(defs.len()));
        for (def, cost) in defs {
            args.push(OscType::String(def.into()));
            args.push(seconds(cost.busy_nanos));
            args.push(count(cost.runs));
            args.push(OscType::Int(cost.nodes as i32));
        }

        let mut commands: Vec<_> = self.commands.iter().collect();
        commands.sort_by(|a, b| (b.1.busy_nanos, a.0).cmp(&(a.1.busy_nanos, b.0)));
        args.push(rows(commands.len()));
        for (addr, traffic) in commands {
            args.push(OscType::String(addr.clone()));
            args.push(seconds(traffic.busy_nanos));
            args.push(count(traffic.count));
            args.push(count(traffic.bytes));
        }

        let replies = self.replies.borrow();
        let mut replies: Vec<_> = replies.iter().collect();
        replies.sort_by(|a, b| (b.1.bytes, a.0).cmp(&(a.1.bytes, b.0)));
        args.push(rows(replies.len()));
        for (addr, traffic) in replies {
            args.push(OscType::String(addr.clone()));
            args.push(count(traffic.count));
            args.push(count(traffic.bytes));
        }

        let mut jobs: Vec<_> = self.jobs.iter().collect();
        jobs.sort_by(|a, b| (b.1.busy_nanos, a.0).cmp(&(a.1.busy_nanos, b.0)));
        args.push(rows(jobs.len()));
        for (cmd, cost) in jobs {
            args.push(OscType::String((*cmd).into()));
            args.push(seconds(cost.busy_nanos));
            args.push(count(cost.count));
            args.push(seconds(cost.waited_nanos));
        }
        args
    }
}

/// The encoded size of `msg`, in bytes, without encoding it: the address and
/// the type tags each padded to four, then every argument.
pub(super) fn message_size(msg: &OscMessage) -> usize {
    fn pad(n: usize) -> usize {
        (n + 3) & !3
    }
    fn tags(arg: &OscType) -> usize {
        match arg {
            OscType::Array(array) => 2 + array.content.iter().map(tags).sum::<usize>(),
            _ => 1,
        }
    }
    fn body(arg: &OscType) -> usize {
        match arg {
            OscType::Int(_)
            | OscType::Float(_)
            | OscType::Char(_)
            | OscType::Color(_)
            | OscType::Midi(_) => 4,
            OscType::Long(_) | OscType::Double(_) | OscType::Time(_) => 8,
            OscType::String(s) => pad(s.len() + 1),
            OscType::Blob(b) => 4 + pad(b.len()),
            OscType::Array(array) => array.content.iter().map(body).sum(),
            OscType::Bool(_) | OscType::Nil | OscType::Inf => 0,
        }
    }
    let tag_count = 1 + msg.args.iter().map(tags).sum::<usize>();
    pad(msg.addr.len() + 1) + pad(tag_count + 1) + msg.args.iter().map(body).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn udp() -> ClientId {
        ClientId::Udp("127.0.0.1:9".parse().unwrap())
    }

    #[test]
    fn the_size_of_a_message_is_its_encoded_length() {
        let msg = OscMessage {
            addr: "/buffer_setn".into(),
            args: vec![
                OscType::Int(3),
                OscType::String("abc".into()),
                OscType::Blob(vec![1, 2, 3, 4, 5]),
                OscType::Double(0.5),
                OscType::Nil,
            ],
        };
        let encoded = rosc::encoder::encode(&rosc::OscPacket::Message(msg.clone())).unwrap();
        assert_eq!(message_size(&msg), encoded.len());
    }

    #[test]
    fn a_synth_that_never_ran_profiled_is_not_a_node_of_its_def() {
        let mut profile = NetProfile::default();
        profile.retire(Some("sine".into()), NodeCost::default());
        let report = profile.report(&Meters::new(0), false, &[], |_| None);
        // uptime, profiling, the phases, then no nodes and no defs.
        let after_phases = 3 + 4 * crate::server::meters::Phase::ALL.len();
        assert_eq!(report[after_phases], OscType::Int(0), "no node rows");
        assert_eq!(report[after_phases + 1], OscType::Int(0), "no def rows");
    }

    #[test]
    fn a_def_row_adds_its_live_and_its_retired_synths() {
        let mut profile = NetProfile::default();
        let gone = NodeCost {
            id: 1000,
            busy_nanos: 3_000,
            runs: 3,
        };
        profile.retire(Some("sine".into()), gone);
        profile.retire(None, NodeCost { runs: 1, ..gone });
        let live = [
            NodeCost {
                id: 1001,
                busy_nanos: 2_000,
                runs: 2,
            },
            NodeCost {
                id: 1002,
                busy_nanos: 9_000,
                runs: 2,
            },
        ];
        let def_of = |id| {
            (id == 1001)
                .then(|| "sine".to_owned())
                .or(Some("saw".into()))
        };
        let report = profile.report(&Meters::new(0), true, &live, def_of);
        assert_eq!(report[1], OscType::Int(1), "profiling");
        let mut at = 3 + 4 * crate::server::meters::Phase::ALL.len();
        assert_eq!(report[at], OscType::Int(2), "two live synths");
        assert_eq!(report[at + 1], OscType::Int(1002), "costliest first");
        assert_eq!(report[at + 2], OscType::String("saw".into()));
        at += 1 + 2 * 4;
        assert_eq!(report[at], OscType::Int(3), "sine, saw and the unknown");
        let def_rows: Vec<_> = report[at + 1..at + 1 + 3 * 4].chunks(4).collect();
        let sine = def_rows
            .iter()
            .find(|r| r[0] == OscType::String("sine".into()))
            .unwrap();
        assert_eq!(sine[1], OscType::Double(5e-6));
        assert_eq!(sine[2], OscType::Long(5));
        assert_eq!(sine[3], OscType::Int(2), "one live, one retired");
        assert!(def_rows.iter().any(|r| r[0] == OscType::String("?".into())));
    }

    #[test]
    fn commands_replies_and_jobs_each_have_a_section() {
        let mut profile = NetProfile::default();
        profile.command("/node_set", 4_000, 24);
        profile.command("/node_set", 1_000, 24);
        profile.reply("/done", 16);
        profile.job("/buffer_read", 7_000, 2_000);
        let report = profile.report(&Meters::new(0), false, &[], |_| None);
        let at = 3 + 4 * crate::server::meters::Phase::ALL.len() + 2;
        assert_eq!(report[at], OscType::Int(1), "one command");
        assert_eq!(
            report[at + 1..at + 5],
            [
                OscType::String("/node_set".into()),
                OscType::Double(5e-6),
                OscType::Long(2),
                OscType::Long(48),
            ]
        );
        let at = at + 5;
        assert_eq!(
            report[at..at + 4],
            [
                OscType::Int(1),
                OscType::String("/done".into()),
                OscType::Long(1),
                OscType::Long(16),
            ]
        );
        let at = at + 4;
        assert_eq!(
            report[at..at + 5],
            [
                OscType::Int(1),
                OscType::String("/buffer_read".into()),
                OscType::Double(7e-6),
                OscType::Long(1),
                OscType::Double(2e-6),
            ]
        );
    }

    #[test]
    fn a_server_without_a_clock_reports_nil_seconds() {
        let mut profile = NetProfile::default();
        profile.command("/status", 1_000, 8);
        let report = profile.report(&Meters::untimed(0), false, &[], |_| None);
        assert_eq!(report[0], OscType::Nil, "no uptime");
        assert_eq!(report[5], OscType::Nil, "no seconds for a phase");
        let at = 3 + 4 * crate::server::meters::Phase::ALL.len() + 2;
        assert_eq!(report[at + 2], OscType::Nil, "nor for a command");
        assert_eq!(report[at + 3], OscType::Long(1), "and its count is there");
    }

    #[test]
    fn watching_ends_with_the_last_watcher() {
        let mut profile = NetProfile::default();
        assert!(profile.watch(udp()));
        assert!(profile.watch(udp()), "a second watch from one client");
        assert!(!profile.unwatch(|c| *c == udp()), "is still one watcher");
    }
}
