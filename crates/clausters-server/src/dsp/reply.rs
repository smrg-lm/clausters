//! Side-effect UGens: `SendTrig`, `SendReply` and `Poll` -- UGens whose
//! purpose is an OSC reply or a console post, not audio on a bus. Each detects
//! a **trigger** (a signal crossing from `<= 0` up to `> 0`) and buffers one
//! [`ReplyMsg`] per crossing; the synth drains the buffer after the block (see
//! `synthdef::instance`), and the network thread turns each message into `/node_trigger`,
//! a custom-address reply, or a console line (see `osc::server`). A SynthDef may
//! contain only these UGens and no `Out` at all -- the server already permits
//! output-less defs.
//!
//! RT-safe: the per-block buffer is a fixed inline array ([`REPLY_BUFFER_LEN`]),
//! so buffering a trigger allocates nothing; extra triggers within one block are
//! dropped, best-effort like the node-event FIFO. The command name / label lives
//! in the UGen as a `String` built on the network thread and only *read* on the
//! audio thread, so no allocation happens while processing.

use crate::dsp::registry::UGenConfig;
use crate::dsp::trig::Edge;
use crate::dsp::{
    BLOCK_SIZE, ProcessCtx, REPLY_BUFFER_LEN, REPLY_MAX_VALUES, ReplyKind, ReplyMsg, UGen, at,
};

/// A fixed-capacity, allocation-free buffer of the reply messages a UGen
/// accumulates within one block; the synth drains it afterwards.
pub(crate) struct ReplyBuffer {
    msgs: [ReplyMsg; REPLY_BUFFER_LEN],
    len: usize,
}

impl ReplyBuffer {
    pub(crate) fn new() -> Self {
        Self {
            msgs: [ReplyMsg::default(); REPLY_BUFFER_LEN],
            len: 0,
        }
    }

    /// Buffers one message, dropping it once the block's capacity is reached.
    #[inline]
    pub(crate) fn push(&mut self, msg: ReplyMsg) {
        if self.len < REPLY_BUFFER_LEN {
            self.msgs[self.len] = msg;
            self.len += 1;
        }
    }

    /// Emits every buffered message stamped with `node_id`, then clears.
    pub(crate) fn drain(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
        for msg in &mut self.msgs[..self.len] {
            msg.node_id = node_id;
            sink(*msg);
        }
        self.len = 0;
    }
}

/// `SendTrig(in, id, value)` -- sends `/node_trigger nodeID id value` on each trigger of
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
    fn reset(&mut self) {
        // Buffered replies still leave with this block's drain.
        self.prev = Edge::default();
    }

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

/// `SendReply(trig, replyID, values...)` -- sends an arbitrary-arity OSC message
/// (`cmdName` from the def, default `/reply`) as `cmdName nodeID replyID
/// value...` on each trigger. Inputs after `trig`, `replyID` are the value list.
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
    fn reset(&mut self) {
        // Buffered replies still leave with this block's drain.
        self.prev = Edge::default();
    }

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

/// `Poll(trig, in, trigid)` -- on each trigger of `trig`, posts `label: value`
/// (the `in` value) to the server console and, when `trigid >= 0`, also sends a
/// `/node_trigger nodeID trigid value` reply. `in` passes through the output unchanged, so
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
    fn reset(&mut self) {
        // Buffered replies still leave with this block's drain.
        self.prev = Edge::default();
    }

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

/// One channel of a [`SendPeakRMS`]: this tick's peak and sum of squares,
/// and the lagged peak.
#[derive(Clone, Copy, Default)]
struct PeakRms {
    peak: f32,
    squares: f32,
    lagged: f32,
}

/// Inputs of a `SendPeakRMS` before its channels.
const PEAK_RMS_HEAD: usize = 3;

/// `SendPeakRMS(reply_rate, peak_lag, reply_id, channels...)`: `reply_rate`
/// times a second, a reply (`cmdName` from the def, default `/reply`) of
/// `nodeID replyID` and each channel's peak and RMS over the interval -- the
/// peak held and falling back by `peak_lag` seconds to -60 dB, a meter's
/// ballistics. Channels at audio and at control rate may be mixed; each is
/// measured at its own rate. Up to eight channels, as many as a reply
/// carries.
pub struct SendPeakRMS {
    cmd: String,
    channels: Vec<PeakRms>,
    remain: i64,
    started: bool,
    buf: ReplyBuffer,
}

impl SendPeakRMS {
    pub fn new(config: &UGenConfig) -> Self {
        let cmd = config
            .label
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/reply".to_string());
        let n = config
            .inputs
            .saturating_sub(PEAK_RMS_HEAD)
            .min(REPLY_MAX_VALUES / 2);
        Self {
            cmd,
            channels: vec![PeakRms::default(); n],
            remain: 0,
            started: false,
            buf: ReplyBuffer::new(),
        }
    }

    fn send(&mut self, inputs: &[&[f32]], b1: f32, audio_tick: f32, control_tick: f32) {
        let id = at(inputs[2], 0) as i32;
        let mut msg = ReplyMsg::new(ReplyKind::Reply, id, &self.cmd);
        for (k, ch) in self.channels.iter_mut().enumerate() {
            let audio = inputs[PEAK_RMS_HEAD + k].len() > 1;
            ch.lagged = if ch.peak >= ch.lagged {
                ch.peak
            } else {
                ch.peak + b1 * (ch.lagged - ch.peak)
            };
            let tick = if audio { audio_tick } else { control_tick };
            msg.push_value(ch.lagged);
            msg.push_value((ch.squares / tick).sqrt());
            ch.peak = 0.0;
            ch.squares = 0.0;
        }
        self.buf.push(msg);
    }
}

impl UGen for SendPeakRMS {
    fn reset(&mut self) {
        self.channels.fill(PeakRms::default());
        self.started = false;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
        let rate = at(inputs[0], 0);
        if rate <= 0.0 {
            return;
        }
        let audio_tick = (ctx.full_sample_rate / rate) as i64;
        let control_tick = (ctx.full_sample_rate / (BLOCK_SIZE as f32) / rate) as i64;
        let lag = at(inputs[1], 0);
        let b1 = if lag != 0.0 {
            (0.001f64.ln() / (lag as f64 * rate as f64)).exp() as f32
        } else {
            0.0
        };
        let audio = output.len() > 1;
        let tick = if audio { audio_tick } else { control_tick }.max(1);
        if !self.started {
            self.started = true;
            self.remain = tick;
        }
        let measure = |chans: &mut [PeakRms], from: usize, to: usize| {
            for (k, ch) in chans.iter_mut().enumerate() {
                let x = inputs[PEAK_RMS_HEAD + k];
                if x.len() == 1 {
                    if from == 0 {
                        ch.peak = ch.peak.max(x[0].abs());
                        ch.squares += x[0] * x[0];
                    }
                } else {
                    for &v in &x[from..to] {
                        ch.peak = ch.peak.max(v.abs());
                        ch.squares += v * v;
                    }
                }
            }
        };
        let (at_tick, ct_tick) = (audio_tick.max(1) as f32, control_tick.max(1) as f32);
        if !audio {
            // scsynth's control-rate form: the tick before the block.
            self.remain -= 1;
            if self.remain <= 0 {
                self.remain += tick;
                self.send(inputs, b1, at_tick, ct_tick);
            }
            let len = inputs[PEAK_RMS_HEAD..]
                .iter()
                .map(|x| x.len())
                .max()
                .unwrap_or(1);
            measure(&mut self.channels, 0, len);
            return;
        }
        let n = output.len();
        let mut from = 0;
        while from < n {
            let take = (self.remain as usize).min(n - from);
            measure(&mut self.channels, from, from + take);
            from += take;
            self.remain -= take as i64;
            if self.remain == 0 {
                self.send(inputs, b1, at_tick, ct_tick);
                self.remain = tick;
            }
        }
    }

    fn is_reply(&self) -> bool {
        true
    }

    fn drain_replies(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
        self.buf.drain(node_id, sink);
    }
}
