//! **The audio editor's engine, held by its application**: what the
//! coordinator asks of an engine in its own process
//! ([`clausters_apps::editing::held::HeldEngine`]), answered over an
//! [`AppEngine`] with the take reader of [`crate::take`].
//!
//! Each file the editor sounds has a [`TakeFeed`]: a fed stream per channel
//! and the thread that reads the file's list into them. A file's list handed
//! over again (an edit, an undo) is put in place in its feed, and the readers
//! fade into it from where they are; a list of another length puts the
//! buffers in the pool again at that length, over the same streams, so the
//! readers keep their rings.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use clausters_apps::editing::held::{HeldEngine, HeldTake};
use clausters_core::ids::{IdSpaces, Space};
use clausters_editing::apply::Medium;

use crate::take::{Headers, ReadList, TakeFeed};
use crate::{AppEngine, EngineMedium};

/// One file's reading: its feed, the buffers its readers play, and the list
/// it reads.
struct Reading {
    feed: TakeFeed,
    buffers: Vec<i32>,
    revision: u64,
    frames: u64,
}

/// **An audio editor's engine, in its application's process.**
pub struct HeldAudio {
    engine: Arc<Mutex<AppEngine>>,
    headers: Headers,
    readings: HashMap<u64, Reading>,
}

impl std::fmt::Debug for HeldAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldAudio")
            .field("files", &self.readings.len())
            .finish()
    }
}

impl HeldAudio {
    /// The audio editor's engine over `engine`, reading nothing yet.
    pub fn new(engine: Arc<Mutex<AppEngine>>) -> HeldAudio {
        HeldAudio {
            engine,
            headers: Headers::default(),
            readings: HashMap::new(),
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

impl HeldEngine for HeldAudio {
    fn medium(&mut self) -> Box<dyn Medium + Send> {
        Box::new(EngineMedium::new(Arc::clone(&self.engine)))
    }

    fn rate(&self) -> f64 {
        f64::from(self.engine().sample_rate())
    }

    fn take(&mut self, take: &HeldTake, ids: &mut IdSpaces) -> Result<Vec<i32>, String> {
        let channels = take.channels.max(1) as usize;
        // A file of another width is read anew.
        if self
            .readings
            .get(&take.file)
            .is_some_and(|r| r.feed.channels() != channels)
            && let Some(was) = self.readings.remove(&take.file)
        {
            self.free(was, ids);
        }
        let list = ReadList::of(&take.parts, channels, take.rate, &mut self.headers)
            .inspect_err(|e| tracing::warn!("the audio editor's take: {e}"))?;
        let new = !self.readings.contains_key(&take.file);
        if new {
            let buffers = AppEngine::alloc_take(channels, ids).map_err(|e| e.to_string())?;
            self.readings.insert(
                take.file,
                Reading {
                    feed: TakeFeed::start(channels),
                    buffers,
                    revision: take.revision,
                    frames: take.frames,
                },
            );
        }
        let engine = Arc::clone(&self.engine);
        let Some(reading) = self.readings.get_mut(&take.file) else {
            return Err("the take is not read".into());
        };
        reading.feed.set(Arc::new(list));
        if new || reading.frames != take.frames {
            let mut engine = engine.lock().unwrap_or_else(|e| e.into_inner());
            engine.install_take(&reading.feed, take.frames, &reading.buffers);
        }
        reading.revision = take.revision;
        reading.frames = take.frames;
        Ok(reading.buffers.clone())
    }

    fn revision(&self, file: u64) -> Option<u64> {
        self.readings.get(&file).map(|r| r.revision)
    }

    fn close(&mut self, file: u64, ids: &mut IdSpaces) {
        if let Some(reading) = self.readings.remove(&file) {
            self.free(reading, ids);
        }
    }

    fn rolling(&self, transport: i32) -> bool {
        usize::try_from(transport).is_ok_and(|t| self.engine().rolling(t))
    }
}
