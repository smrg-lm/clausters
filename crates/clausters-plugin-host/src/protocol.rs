//! Administrative plane: framed JSON on a dedicated inherited Unix socket.
use crate::format::{PluginInfo, ScanEntry};
use clausters_core::plugin_shm::{EVENTS, FRAMES, PLUGIN_ABI_VERSION, SLOTS, SPANS};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

pub(crate) const CONTROL_VERSION: u32 = 3;
pub(crate) const MAX_FRAME: usize = 8 * 1024 * 1024;
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Handshake {
    control: u32,
    audio: u32,
    frames: usize,
    slots: usize,
    events: usize,
    spans: usize,
}
impl Handshake {
    pub(crate) fn current() -> Self {
        Self {
            control: CONTROL_VERSION,
            audio: PLUGIN_ABI_VERSION,
            frames: FRAMES,
            slots: SLOTS,
            events: EVENTS,
            spans: SPANS,
        }
    }
}
#[derive(Serialize, Deserialize)]
pub(crate) enum Request {
    Probe {
        path: String,
        id: String,
    },
    Scan {
        path: String,
        offset: usize,
    },
    Load {
        path: String,
        id: String,
        segment: String,
        generation: u64,
        state: Option<Vec<u8>>,
    },
    Metadata,
    SaveState,
    LoadState(Vec<u8>),
    Parameter(u32),
    Shutdown,
}
#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    Scan(Vec<ScanEntry>),
    Metadata(Box<PluginInfo>),
    State(Vec<u8>),
    Parameter(f64),
    Ok,
    Error(String),
}
pub(crate) fn send<T: Serialize>(socket: &mut UnixStream, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FRAME {
        return Err("plugin administrative frame capacity exceeded".into());
    }
    socket
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .map_err(|e| e.to_string())?;
    socket.write_all(&bytes).map_err(|e| e.to_string())
}
pub(crate) fn receive<T: serde::de::DeserializeOwned>(
    socket: &mut UnixStream,
) -> Result<T, String> {
    let mut length = [0; 4];
    socket.read_exact(&mut length).map_err(|e| e.to_string())?;
    let n = u32::from_le_bytes(length) as usize;
    if n == 0 || n > MAX_FRAME {
        return Err("plugin administrative frame capacity exceeded".into());
    }
    let mut bytes = vec![0; n];
    socket.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_handshake_dimension_is_required() {
        for key in ["control", "audio", "frames", "slots", "events", "spans"] {
            let mut value = serde_json::to_value(Handshake::current()).unwrap();
            let n = value[key].as_u64().unwrap();
            value[key] = (n + 1).into();
            let changed: Handshake = serde_json::from_value(value).unwrap();
            assert_ne!(changed, Handshake::current());
        }
    }
}
