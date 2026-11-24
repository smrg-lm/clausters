//! scsynth's test UGens: [`CheckBadValues`], which says what kind of number a
//! signal carries, and [`Sanitize`], which replaces the kinds a signal should
//! not carry.
//!
//! A bad value is a NaN, an infinity or a denormal. Every processing thread
//! runs in flush-to-zero mode (`dsp::denormals`), so arithmetic here never
//! makes a denormal, and one can only arrive from outside the arithmetic -- a
//! buffer's samples, a bus written by a client. Both UGens still look for it,
//! since that is exactly the case a check is for.

use crate::dsp::reply::ReplyBuffer;
use crate::dsp::{ProcessCtx, ReplyKind, ReplyMsg, UGen, at};

/// The class [`CheckBadValues`] outputs: 0 for an ordinary number (zero
/// included), 1 for a NaN, 2 for an infinity, 3 for a denormal -- scsynth's
/// numbering.
fn class(x: f32) -> u8 {
    if x.is_nan() {
        1
    } else if x.is_infinite() {
        2
    } else if x.is_subnormal() {
        3
    } else {
        0
    }
}

/// The name a class is posted by.
pub fn class_name(class: u8) -> &'static str {
    match class {
        1 => "NaN",
        2 => "infinity",
        3 => "denormal",
        _ => "normal",
    }
}

/// `CheckBadValues(signal, id, post)`: outputs the class of every sample of
/// `signal` (see `class`) and, by `post`, says so on the server's console:
/// `0` never, `1` at every bad sample, `2` (the default) whenever the class
/// changes, with how many samples the previous class lasted. `id` tells one
/// check from another in what it posts.
///
/// A post is a reply, so it leaves the audio thread on the reply path and is
/// written by the network thread; a block holds a few of them, and the rest of
/// a block of bad samples posted one by one is dropped, as replies beyond a
/// block's capacity are.
pub struct CheckBadValues {
    /// The class of the previous sample, and how many samples it has lasted.
    prev: u8,
    count: u32,
    buf: ReplyBuffer,
}

impl CheckBadValues {
    pub fn new() -> Self {
        Self {
            prev: 0,
            count: 0,
            buf: ReplyBuffer::new(),
        }
    }

    fn post(&mut self, id: f32, class: u8, prev: Option<(u8, u32)>) {
        let mut msg = ReplyMsg::new(ReplyKind::BadValue, id as i32, "CheckBadValues");
        msg.push_value(class as f32);
        if let Some((prev, count)) = prev {
            msg.push_value(prev as f32);
            msg.push_value(count as f32);
        }
        self.buf.push(msg);
    }
}

impl Default for CheckBadValues {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for CheckBadValues {
    fn reset(&mut self) {
        // Buffered posts still leave with this block's drain.
        self.prev = 0;
        self.count = 0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (signal, id, post) = (inputs[0], at(inputs[1], 0), at(inputs[2], 0) as i32);
        for (i, out) in output.iter_mut().enumerate() {
            let c = class(at(signal, i));
            *out = c as f32;
            match post {
                1 if c != 0 => self.post(id, c, None),
                2 => {
                    if c != self.prev {
                        let prev = (self.count > 0).then_some((self.prev, self.count));
                        self.post(id, c, prev);
                        self.count = 0;
                    }
                    self.count = self.count.saturating_add(1);
                    self.prev = c;
                }
                _ => {}
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

/// `Sanitize(signal, replace)`: `signal` with every bad value (a NaN, an
/// infinity, a denormal) replaced by `replace` -- a guard before a filter
/// whose state a single NaN would ruin for good.
pub struct Sanitize;

impl UGen for Sanitize {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (signal, replace) = (inputs[0], inputs[1]);
        for (i, out) in output.iter_mut().enumerate() {
            let x = at(signal, i);
            *out = if x.is_finite() && !x.is_subnormal() {
                x
            } else {
                at(replace, i)
            };
        }
    }
}
