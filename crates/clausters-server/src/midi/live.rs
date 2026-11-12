//! Live MIDI input over the OS's standard MIDI, via `midir` (transport).
//!
//! On Linux `midir` speaks the **ALSA sequencer** -- the same system MIDI any
//! controller or DAW uses -- so [`MidiHub::open`] creates a **virtual input
//! port** named for the server; anything routed into it (a keyboard through the
//! kernel, `aconnect`, a DAW) drives the engine. (Network MIDI is a separate
//! idea, deliberately out of scope here.)
//!
//! Threading mirrors the TCP transport ([`clausters_net::tcp`]): `midir` runs the
//! input callback on **its own thread**, which hands each channel-voice
//! message's bytes to the single-threaded command loop over an
//! [`mpsc`](std::sync::mpsc) channel; a **zero-length UDP datagram** to the
//! server's own address wakes the loop so the message is acted on at once,
//! without waiting for the periodic GC tick. The loop translates it on the
//! network thread (`CmdTranslator::translate_midi_bytes`, which runs it through
//! the MPE zones' decoder first); the audio thread is never touched.

use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, channel};

use midir::os::unix::VirtualInput;
use midir::{MidiInput, MidiInputConnection};

/// The server side of the live MIDI transport: the decoded-message stream the
/// command loop drains. Holds the `midir` connection open (dropping it closes
/// the virtual port and stops the input thread).
pub struct MidiHub {
    events: Receiver<[u8; 3]>,
    _conn: MidiInputConnection<()>,
    port_name: String,
}

impl MidiHub {
    /// Opens a virtual MIDI input port named `port_name`. `wake_target` is the
    /// server's own UDP address; the input thread pings it with a zero-length
    /// datagram whenever a message is queued, to wake the command loop.
    pub fn open(port_name: &str, wake_target: SocketAddr) -> Result<Self, String> {
        let input = MidiInput::new("clausters").map_err(|e| e.to_string())?;
        let (tx, events) = channel();
        // A throwaway socket the input thread uses only to send wake bytes.
        let wake = UdpSocket::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        let conn = input
            .create_virtual(
                port_name,
                move |_timestamp, bytes, _| {
                    let Some(&status) = bytes.first() else { return };
                    // Only channel-voice messages reach the loop; system
                    // messages (SysEx, clock, ...) are dropped here.
                    if !(0x80..0xF0).contains(&status) {
                        return;
                    }
                    let d1 = bytes.get(1).copied().unwrap_or(0);
                    let d2 = bytes.get(2).copied().unwrap_or(0);
                    if tx.send([status, d1, d2]).is_ok() {
                        let _ = wake.send_to(&[], wake_target);
                    }
                },
                (),
            )
            .map_err(|e| e.to_string())?;
        Ok(Self {
            events,
            _conn: conn,
            port_name: port_name.to_string(),
        })
    }

    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    /// The next message's bytes, or `None` when the queue is drained.
    pub fn try_next(&self) -> Option<[u8; 3]> {
        self.events.try_recv().ok()
    }
}
