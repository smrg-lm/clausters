//! **Sounding a take**: the def that plays a buffer, and the one node the host
//! keeps while it is playing.
//!
//! A buffer is data, and data does not sound: what sounds is an instrument
//! reading it (`docs/decisions.md`). A host that draws a take therefore needs a
//! def of its own to hear one, and it is deliberately the smallest one that
//! could be — read the buffer at its own rate, scale it, out to the first
//! hardware bus. Nothing here is a synthesis surface: a composition's
//! instruments are the client's, and this is the editor's monitor.
//!
//! **One voice, and the host holds its node.** Playing again stops what is
//! playing, because two copies of one take over each other is noise and not a
//! preview. The node is freed rather than gated: the def has no envelope, since
//! a monitor that fades is a monitor lying about the material.

use clausters_core::osc::{OscMessage, OscType};
use serde_json::json;

use super::Host;

/// The def name the host plays a take through. Namespaced, because it is loaded
/// into the same server a composition's own defs live in.
pub const TAKE_DEF: &str = "clausters-gui-take";

/// The node id the take monitor plays on — a **fixed** one, over the voice
/// window's base and outside its wrapping span, because there is only ever one
/// and a fixed id is what makes a stop that arrives after a lost reply still
/// stop the right node.
const TAKE_NODE: i32 = super::voices::ID_BASE + super::voices::ID_SPAN;

/// The `/def_send synth` that loads the take monitor.
///
/// Sent once, by whoever gives a session its server: a def is asynchronous, so
/// loading it at the first press would race the `/synth_new` that wanted it.
pub fn take_def_message() -> OscMessage {
    // `BufRateScale` is what makes the file play at its own pitch without the
    // host knowing either rate — the buffer keeps the file's sample rate and
    // the server never resamples.
    let spec = json!({
        "name": TAKE_DEF,
        "controls": [
            {"name": "bufnum", "default": 0.0},
            {"name": "amp", "default": 1.0},
            {"name": "out", "default": 0.0},
        ],
        "ugens": [
            {"kind": "BufRateScale", "inputs": [{"control": 0}]},
            {"kind": "PlayBuf", "inputs": [
                {"control": 0}, {"const": 0.0}, {"ugen": 0}, {"const": 0.0}]},
            {"kind": "Mul", "inputs": [{"ugen": 1}, {"control": 1}]},
            {"kind": "Out", "inputs": [{"control": 2}, {"ugen": 2}]},
        ],
    });
    OscMessage {
        addr: "/def_send".into(),
        args: vec![
            OscType::String("synth".into()),
            OscType::Blob(spec.to_string().into_bytes()),
        ],
    }
}

impl Host {
    /// **Plays the material a widget draws**, from its first frame, stopping
    /// whatever the monitor was playing. Returns whether anything sounds.
    ///
    /// The widget is named rather than the buffer because that is what the hand
    /// pointed at: the same lookup an edit takes, so what plays is what would
    /// be written.
    pub fn play_material(&mut self, def_id: i32, widget_id: i32) -> bool {
        let Some(bufnum) = self.buffer_of(def_id, widget_id) else {
            return false;
        };
        if self.server.is_none() {
            tracing::warn!("nothing to play this take through: no audio server");
            return false;
        }
        self.stop_material();
        self.send_to_server(OscMessage {
            addr: "/synth_new".into(),
            args: vec![
                OscType::String(TAKE_DEF.into()),
                OscType::Int(TAKE_NODE),
                OscType::Int(0), // add to head…
                OscType::Int(0), // …of the root group
                OscType::String("bufnum".into()),
                OscType::Float(bufnum as f32),
            ],
        });
        self.playing = Some(widget_id);
        true
    }

    /// Stops the take monitor, if it is playing. Returns whether it was.
    pub fn stop_material(&mut self) -> bool {
        if self.playing.take().is_none() {
            return false;
        }
        self.send_to_server(OscMessage {
            addr: "/node_free".into(),
            args: vec![OscType::Int(TAKE_NODE)],
        });
        true
    }

    /// The widget whose material the monitor is playing, if any.
    pub fn playing_material(&self) -> Option<i32> {
        self.playing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The def is the wire's own shape, checked here because nothing else
    /// reads it until a server refuses it out loud on a machine with sound.
    #[test]
    fn the_take_def_is_a_synth_def_spec_playing_a_buffer() {
        let msg = take_def_message();
        assert_eq!(msg.addr, "/def_send");
        assert_eq!(msg.args[0], OscType::String("synth".into()));
        let OscType::Blob(spec) = &msg.args[1] else {
            panic!("the spec rides as a blob")
        };
        let spec: serde_json::Value = serde_json::from_slice(spec).expect("json");
        assert_eq!(spec["name"], TAKE_DEF);
        let kinds: Vec<&str> = spec["ugens"]
            .as_array()
            .expect("ugens")
            .iter()
            .map(|u| u["kind"].as_str().expect("a kind"))
            .collect();
        assert_eq!(kinds, vec!["BufRateScale", "PlayBuf", "Mul", "Out"]);
        assert_eq!(
            spec["ugens"][1]["inputs"][2]["ugen"], 0,
            "played at the buffer's own rate, so a 44.1k file is not sharp"
        );
    }
}
