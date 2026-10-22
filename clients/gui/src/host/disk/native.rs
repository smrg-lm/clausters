//! **The native front's answerer**: a thread of its own that reads the disk
//! as the answerer asks, so a cold read never holds a frame.
//!
//! What a front sends it is a message on a channel ([`NativeDisk`], the
//! front's [`DiskLink`]); what it answers goes back the way a server's reply
//! does, through the callback the front gave it, and is read by the same code.

use std::collections::HashMap;
use std::fs::File;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use clausters_core::osc::OscMessage;

use super::answer::{Answer, Answerer, Got, Need, Stat};
use super::{DiskLink, List};

/// How many rounds of reading one ask may take before it is given up: the
/// shapes, then the blocks, then the answer, with room for a header read
/// again from a larger head.
const ROUNDS: usize = 6;

/// Files kept open at once.
const OPEN_FILES: usize = 32;

enum Order {
    Define(i32, Arc<List>),
    Forget(i32),
    Ask(OscMessage),
}

/// The front's way to the thread.
pub struct NativeDisk {
    orders: Sender<Order>,
}

impl NativeDisk {
    /// Starts the thread; `reply` takes each answer, on the thread.
    pub fn spawn(reply: impl Fn(OscMessage) + Send + 'static) -> Result<Self, String> {
        let (orders, rx) = channel();
        std::thread::Builder::new()
            .name("clausters-gui-disk".into())
            .spawn(move || serve(rx, reply))
            .map_err(|e| e.to_string())?;
        Ok(Self { orders })
    }
}

impl DiskLink for NativeDisk {
    fn define(&self, key: i32, list: Arc<List>) {
        let _ = self.orders.send(Order::Define(key, list));
    }

    fn forget(&self, key: i32) {
        let _ = self.orders.send(Order::Forget(key));
    }

    fn ask(&self, msg: OscMessage) {
        let _ = self.orders.send(Order::Ask(msg));
    }
}

fn serve(orders: Receiver<Order>, reply: impl Fn(OscMessage)) {
    let mut answerer = Answerer::default();
    let mut io = Disk::default();
    while let Ok(order) = orders.recv() {
        match order {
            Order::Define(key, list) => answerer.define(key, list),
            Order::Forget(key) => answerer.forget(key),
            Order::Ask(msg) => {
                if let Some(answer) = answer(&mut answerer, &mut io, &msg) {
                    reply(answer);
                }
            }
        }
    }
}

/// **One ask, read through to its answer**: the answerer is asked, what it
/// needs is read, and it is asked again.
pub fn answer(answerer: &mut Answerer, io: &mut Disk, msg: &OscMessage) -> Option<OscMessage> {
    for _ in 0..ROUNDS {
        match answerer.answer(msg) {
            Answer::Reply(reply) => return Some(reply),
            Answer::None => return None,
            Answer::Need(needs) => {
                for need in needs {
                    let got = io.read(&need);
                    answerer.deliver(&need, got);
                }
            }
        }
    }
    crate::host::diag::warn!(
        "{}: still reading after {ROUNDS} rounds; given up",
        msg.addr
    );
    None
}

/// The disk, read at offsets, with the files last read kept open.
#[derive(Default)]
pub struct Disk {
    open: HashMap<Arc<str>, File>,
}

impl Disk {
    /// Reads what `need` names.
    pub fn read(&mut self, need: &Need) -> Result<Got, String> {
        match need {
            Need::Stat { path } => {
                let meta = std::fs::metadata(&**path).map_err(|e| format!("{path}: {e}"))?;
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64());
                Ok(Got::Stat(Stat {
                    len: meta.len(),
                    modified,
                }))
            }
            Need::Read {
                path, offset, len, ..
            } => {
                if !self.open.contains_key(path) {
                    if self.open.len() >= OPEN_FILES {
                        self.open.clear();
                    }
                    let file = File::open(&**path).map_err(|e| format!("{path}: {e}"))?;
                    self.open.insert(Arc::clone(path), file);
                }
                let file = &self.open[path];
                let mut bytes = vec![0u8; *len];
                read_at(file, &mut bytes, *offset).map_err(|e| format!("{path}: {e}"))?;
                Ok(Got::Bytes(bytes))
            }
        }
    }
}

#[cfg(unix)]
fn read_at(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, bytes, offset)
}

#[cfg(windows)]
fn read_at(file: &File, bytes: &mut [u8], offset: u64) -> std::io::Result<()> {
    let mut done = 0;
    while done < bytes.len() {
        let n = std::os::windows::fs::FileExt::seek_read(
            file,
            &mut bytes[done..],
            offset + done as u64,
        )?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        done += n;
    }
    Ok(())
}
