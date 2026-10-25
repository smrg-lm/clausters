//! **The engine's take readers, held by its application**: what an
//! application's members ask of an engine in its own process
//! ([`clausters_apps::editing::held::HeldEngine`]), answered over an
//! [`AppEngine`] with the take reader of [`crate::take`].
//!
//! Each reader a member sounds (an audio editor's file) has a [`TakeFeed`]:
//! a fed stream per channel and the thread that reads its list into them. A
//! list handed over again (an edit, an undo) is put in place in its feed,
//! and the readers fade into it from where they are; a list of another
//! length puts the buffers in the pool again at that length, over the same
//! streams, so the readers keep their rings.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use clausters_apps::editing::held::{HeldEngine, HeldTake};
use clausters_core::ids::{IdSpaces, Space};
use clausters_editing::apply::Medium;

use crate::take::{Counts, Headers, ReadList, TakeFeed};
use crate::{AppEngine, EngineMedium};

/// One reader's reading: its feed, the buffers its readers play, and the list
/// it reads.
struct Reading {
    feed: TakeFeed,
    buffers: Vec<i32>,
    revision: u64,
    frames: u64,
}

/// **The take readers of an engine, in its application's process.**
pub struct Readers {
    engine: Arc<Mutex<AppEngine>>,
    headers: Headers,
    readings: HashMap<u64, Reading>,
    /// What the nodes of the takes it reads fill at its rate, kept across
    /// the takes an edit hands it.
    counts: Arc<Counts>,
}

impl std::fmt::Debug for Readers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Readers")
            .field("files", &self.readings.len())
            .finish()
    }
}

impl Readers {
    /// The readers over `engine`, reading nothing yet.
    pub fn new(engine: Arc<Mutex<AppEngine>>) -> Readers {
        Readers {
            engine,
            headers: Headers::default(),
            readings: HashMap::new(),
            counts: Arc::default(),
        }
    }

    fn engine(&self) -> std::sync::MutexGuard<'_, AppEngine> {
        self.engine.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Frees what `reading` put in the pool and gives its buffers back.
    fn free(&self, reading: Reading, ids: &mut IdSpaces) {
        let cmds = reading
            .buffers
            .iter()
            .map(|&bufnum| clausters::server::engine::Cmd::SetBuffer {
                index: bufnum as usize,
                buffer: None,
            })
            .collect();
        self.engine().push(cmds);
        for bufnum in reading.buffers {
            let _ = ids.release(Space::Buffers, i64::from(bufnum), 1);
        }
        // the feed's thread stops and is joined as it drops
        drop(reading.feed);
    }
}

impl HeldEngine for Readers {
    fn medium(&mut self) -> Box<dyn Medium + Send> {
        Box::new(EngineMedium::new(Arc::clone(&self.engine)))
    }

    fn rate(&self) -> f64 {
        f64::from(self.engine().sample_rate())
    }

    fn take(&mut self, take: &HeldTake, ids: &mut IdSpaces) -> Result<Vec<i32>, String> {
        let channels = take.channels.max(1) as usize;
        // A take of another width is read anew.
        if self
            .readings
            .get(&take.reader)
            .is_some_and(|r| r.feed.channels() != channels)
            && let Some(was) = self.readings.remove(&take.reader)
        {
            self.free(was, ids);
        }
        let list = ReadList::over(&take.list, take.rate, &mut self.headers, &self.counts)
            .inspect_err(|e| tracing::warn!("a take the engine reads: {e}"))?;
        let frames = list.frames();
        let new = !self.readings.contains_key(&take.reader);
        if new {
            let buffers = AppEngine::alloc_take(channels, ids).map_err(|e| e.to_string())?;
            self.readings.insert(
                take.reader,
                Reading {
                    feed: TakeFeed::start(channels),
                    buffers,
                    revision: take.revision,
                    frames,
                },
            );
        }
        let engine = Arc::clone(&self.engine);
        let Some(reading) = self.readings.get_mut(&take.reader) else {
            return Err("the take is not read".into());
        };
        reading.feed.set(Arc::new(list));
        if new || reading.frames != frames {
            let mut engine = engine.lock().unwrap_or_else(|e| e.into_inner());
            engine.install_take(&reading.feed, frames, &reading.buffers);
        }
        reading.revision = take.revision;
        reading.frames = frames;
        Ok(reading.buffers.clone())
    }

    fn revision(&self, reader: u64) -> Option<u64> {
        self.readings.get(&reader).map(|r| r.revision)
    }

    fn close(&mut self, reader: u64, ids: &mut IdSpaces) {
        if let Some(reading) = self.readings.remove(&reader) {
            self.free(reading, ids);
        }
    }

    fn control(&self, bus: usize) -> f32 {
        self.engine().reading().control(bus)
    }

    fn starved(&self) -> u64 {
        self.engine().handle().meters().starved()
    }

    fn rolling(&self, transport: i32) -> bool {
        usize::try_from(transport).is_ok_and(|t| self.engine().rolling(t))
    }
}
