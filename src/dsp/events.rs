//! The event buses as the audio thread holds them.
//!
//! One list for every bus, cleared at the start of each block: an event is
//! written when the command that carries it runs -- at the block's start or
//! at a timed bundle's sample -- tagged with its bus and its offset, and a
//! reader takes the ones of its bus that fall in the slice it is
//! processing. The writers are commands, never nodes, so a slice's events are
//! all written before the slice runs, and readers in a parallel stage only
//! ever read. The list is allocated at boot; a block that writes more than it
//! holds drops the rest and counts them.
//!
//! The event's shape and the JSON a client writes it in are the shared
//! core's (`clausters_core::event_bus`).

pub use clausters_core::event_bus::{BusEvent, Kind};

/// Events every bus together holds per block.
pub const EVENTS_PER_BLOCK: usize = 1024;

/// Default number of event buses (`--event-buses`).
pub const DEFAULT_EVENT_BUSES: usize = 64;

/// One block's events, every bus's.
pub struct EventBuses {
    events: Vec<(u32, BusEvent)>,
    count: usize,
    buses: usize,
    dropped: u64,
}

impl EventBuses {
    /// `buses` buses sharing a list of [`EVENTS_PER_BLOCK`]. Allocates.
    pub fn new(buses: usize) -> Self {
        Self {
            events: vec![(0, BusEvent::default()); EVENTS_PER_BLOCK],
            count: 0,
            buses,
            dropped: 0,
        }
    }

    /// None at all, for a view that reads its parent's.
    pub(crate) fn none() -> Self {
        Self {
            events: Vec::new(),
            count: 0,
            buses: 0,
            dropped: 0,
        }
    }

    /// How many buses there are.
    pub fn len(&self) -> usize {
        self.buses
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.buses == 0
    }

    /// Forgets the last block's events.
    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// Writes `event` on `bus` at `offset`. A bus out of range writes
    /// nothing; a full list drops it and counts it. RT-safe.
    pub fn write(&mut self, bus: u32, offset: usize, mut event: BusEvent) -> bool {
        if bus as usize >= self.buses {
            return false;
        }
        if self.count == self.events.len() {
            self.dropped += 1;
            return false;
        }
        event.offset = offset as u32;
        self.events[self.count] = (bus, event);
        self.count += 1;
        true
    }

    /// The events of `bus` in the slice `offset..offset + frames`, in the
    /// order they were written.
    pub fn read(
        &self,
        bus: u32,
        offset: usize,
        frames: usize,
    ) -> impl Iterator<Item = &BusEvent> + '_ {
        let range = offset as u32..(offset + frames) as u32;
        self.events[..self.count]
            .iter()
            .filter(move |(b, e)| *b == bus && range.contains(&e.offset))
            .map(|(_, e)| e)
    }

    /// Events dropped because a block wrote more than the list holds.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_takes_its_bus_and_its_slice() {
        let mut buses = EventBuses::new(4);
        let note = BusEvent::new(Kind::NoteOn, 1);
        assert!(buses.write(1, 0, note));
        assert!(buses.write(2, 0, note));
        assert!(buses.write(1, 32, note));
        assert!(
            !buses.write(9, 0, note),
            "a bus out of range writes nothing"
        );
        assert_eq!(buses.read(1, 0, 32).count(), 1);
        assert_eq!(buses.read(1, 32, 32).count(), 1);
        assert_eq!(buses.read(1, 0, 64).count(), 2);
        assert_eq!(buses.read(1, 32, 32).next().unwrap().offset, 32);
        buses.clear();
        assert_eq!(buses.read(1, 0, 64).count(), 0);
    }

    #[test]
    fn a_full_block_drops_and_counts() {
        let mut buses = EventBuses::new(1);
        for _ in 0..EVENTS_PER_BLOCK {
            assert!(buses.write(0, 0, BusEvent::default()));
        }
        assert!(!buses.write(0, 0, BusEvent::default()));
        assert_eq!(buses.dropped(), 1);
    }
}
