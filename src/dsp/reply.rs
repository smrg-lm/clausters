//! Side-effect UGens (S9): `SendTrig`, `SendReply` and `Poll` — UGens whose
//! purpose is an OSC reply or a console post, not audio on a bus. Each detects
//! a **trigger** (a signal crossing from `<= 0` up to `> 0`) and buffers one
//! [`ReplyMsg`] per crossing; the synth drains the buffer after the block (see
//! `synthdef::instance`), and the network thread turns each message into `/tr`,
//! a custom-address reply, or a console line (see `osc::server`). A SynthDef may
//! contain only these UGens and no `Out` at all — the server already permits
//! output-less defs.
//!
//! RT-safe: the per-block buffer is a fixed inline array ([`REPLY_BUFFER_LEN`]),
//! so buffering a trigger allocates nothing; extra triggers within one block are
//! dropped, best-effort like the node-event FIFO. The command name / label lives
//! in the UGen as a `String` built on the network thread and only *read* on the
//! audio thread, so no allocation happens while processing.

use crate::dsp::registry::UGenConfig;
use crate::dsp::trig::Edge;
use crate::dsp::{ProcessCtx, REPLY_BUFFER_LEN, ReplyKind, ReplyMsg, UGen, at};

/// A fixed-capacity, allocation-free buffer of the reply messages a UGen
/// accumulates within one block; the synth drains it afterwards.
struct ReplyBuffer {
    msgs: [ReplyMsg; REPLY_BUFFER_LEN],
    len: usize,
}

impl ReplyBuffer {
    fn new() -> Self {
        Self {
            msgs: [ReplyMsg::default(); REPLY_BUFFER_LEN],
            len: 0,
        }
    }

    /// Buffers one message, dropping it once the block's capacity is reached.
    #[inline]
    fn push(&mut self, msg: ReplyMsg) {
        if self.len < REPLY_BUFFER_LEN {
            self.msgs[self.len] = msg;
            self.len += 1;
        }
    }

    /// Emits every buffered message stamped with `node_id`, then clears.
    fn drain(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
        for msg in &mut self.msgs[..self.len] {
            msg.node_id = node_id;
            sink(*msg);
        }
        self.len = 0;
    }
}

/// `SendTrig(in, id, value)` — sends `/tr nodeID id value` on each trigger of
/// `in`. Output is silence (it exists for the side effect, not a signal).
pub struct SendTrig {
    prev: Edge,
    buf: ReplyBuffer,
}

impl SendTrig {
    pub fn new() -> Self {
        Self {
            prev: Edge::default(),
            buf: ReplyBuffer::new(),
        }
    }
}

impl Default for SendTrig {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for SendTrig {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (trig, id, value) = (inputs[0], inputs[1], inputs[2]);
        for (f, &cur) in trig.iter().enumerate() {
            if self.prev.rose(cur) {
                let mut msg = ReplyMsg::new(ReplyKind::Trig, at(id, f) as i32, "");
                msg.push_value(at(value, f));
                self.buf.push(msg);
            }
        }
        output.fill(0.0);
    }

    fn is_reply(&self) -> bool {
        true
    }

    fn drain_replies(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
        self.buf.drain(node_id, sink);
    }
}

/// `SendReply(trig, replyID, values…)` — sends an arbitrary-arity OSC message
/// (`cmdName` from the def, default `/reply`) as `cmdName nodeID replyID
/// value…` on each trigger. Inputs after `trig`, `replyID` are the value list.
/// Output is silence.
pub struct SendReply {
    cmd: String,
    prev: Edge,
    buf: ReplyBuffer,
}

impl SendReply {
    pub fn new(config: &UGenConfig) -> Self {
        let cmd = config
            .label
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/reply".to_string());
        Self {
            cmd,
            prev: Edge::default(),
            buf: ReplyBuffer::new(),
        }
    }
}

impl UGen for SendReply {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        // Variadic: [trig, replyID, value0, value1, ...]. A missing trig (an
        // ill-formed def) simply never fires.
        let Some(&trig) = inputs.first() else {
            output.fill(0.0);
            return;
        };
        let reply_id = inputs.get(1).copied();
        let values = inputs.get(2..).unwrap_or(&[]);
        for (f, &cur) in trig.iter().enumerate() {
            if self.prev.rose(cur) {
                let id = reply_id.map_or(-1, |r| at(r, f) as i32);
                let mut msg = ReplyMsg::new(ReplyKind::Reply, id, &self.cmd);
                for v in values {
                    msg.push_value(at(v, f));
                }
                self.buf.push(msg);
            }
        }
        output.fill(0.0);
    }

    fn is_reply(&self) -> bool {
        true
    }

    fn drain_replies(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
        self.buf.drain(node_id, sink);
    }
}

/// `Poll(trig, in, trigid)` — on each trigger of `trig`, posts `label: value`
/// (the `in` value) to the server console and, when `trigid >= 0`, also sends a
/// `/tr nodeID trigid value` reply. `in` passes through the output unchanged, so
/// `Poll` can sit mid-chain like scsynth.
pub struct Poll {
    label: String,
    prev: Edge,
    buf: ReplyBuffer,
}

impl Poll {
    pub fn new(config: &UGenConfig) -> Self {
        let label = config
            .label
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "poll".to_string());
        Self {
            label,
            prev: Edge::default(),
            buf: ReplyBuffer::new(),
        }
    }
}

impl UGen for Poll {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (trig, sig, trigid) = (inputs[0], inputs[1], inputs[2]);
        for (f, &cur) in trig.iter().enumerate() {
            if self.prev.rose(cur) {
                let mut msg = ReplyMsg::new(ReplyKind::Poll, at(trigid, f) as i32, &self.label);
                msg.push_value(at(sig, f));
                self.buf.push(msg);
            }
        }
        // Pass the polled signal through, so Poll can be inserted inline.
        for (j, o) in output.iter_mut().enumerate() {
            *o = at(sig, j);
        }
    }

    fn is_reply(&self) -> bool {
        true
    }

    fn drain_replies(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
        self.buf.drain(node_id, sink);
    }
}
