//! **The server's random generators**: a table of scsynth's generators the
//! random UGens draw from, one table per engine.
//!
//! scsynth's model, kept: the world holds a small table of generators
//! ([`GENERATORS`] of them, scsynth's default count), every random UGen draws
//! from the one its synth selects, `RandID` selects it (0 unless told) and
//! `RandSeed` reseeds it. So the streams are shared rather than owned: two
//! `WhiteNoise` in one synth are two runs of one stream, drawn in turn, which
//! is what keeps them from correlating, and a synth placed at the head of a
//! group can reseed the generator the whole group draws from, which is how a
//! sequence is restarted on purpose.
//!
//! The generator is scsynth's own ([`RGen`], `taus88` with scsynth's seeding),
//! so a seed gives scsynth's stream: a def run from the seed it was run from in
//! SuperCollider draws the same numbers.
//!
//! **One seed seeds the table**: generator `i` is seeded with the seed's low
//! word plus `i`, so the seed a server reports is the whole of its state, and
//! a seed below 2^32 seeds generator 0 exactly as `RandSeed` with that number
//! would. The seed comes from the server's configuration (`--seed`, the
//! `[server]` key), from `/server_seed` at run time, from a render's
//! configuration, or -- when nobody says -- from entropy, and is reported
//! either way.
//!
//! **Order is the seed's other half.** With shared generators a value depends
//! on the order the UGens draw in, and the order is the node tree's, which is
//! sequential and fixed. A `/group_parallel` group keeps it: the stage
//! scheduler treats a generator as a resource, like a bus, so two nodes that
//! draw from the same generator never run in one stage, and a parallel render
//! draws exactly what a sequential one does ([`crate::dsp::StageMask`]).
//! Nodes that should run in parallel and draw randomness select different
//! generators with `RandID`.

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub use clausters_core::rng::RGen;

/// How many generators a table holds: scsynth's default (`-r 64`). It is also
/// the width of the stage scheduler's generator mask, one bit each.
pub const GENERATORS: usize = 64;

/// One engine's generators.
///
/// The generators are read and written from the audio thread, and from a
/// parallel stage's workers; the stage scheduler guarantees that no two nodes
/// of one stage draw from the same generator ([`crate::dsp::StageMask`]'s
/// `generators`), which is the invariant every access below relies on. A
/// reseed from a client reaches the table as an engine command, on the audio
/// thread, between blocks.
pub struct Generators {
    cells: [UnsafeCell<RGen>; GENERATORS],
    /// The seed the table was last seeded from, as reported.
    seed: AtomicU64,
}

// SAFETY: a generator is only ever accessed by the thread running the node
// that draws from it, and the stage scheduler keeps two nodes that draw from
// one generator out of the same stage (see the type's documentation). Every
// access copies the generator in or out; no reference to a cell escapes.
unsafe impl Sync for Generators {}

/// The low word of a 64-bit seed with the high one folded in: the number a
/// table seeds its first generator with. A seed below 2^32 is itself.
#[inline]
fn word(seed: u64) -> u32 {
    (seed ^ (seed >> 32)) as u32
}

impl Generators {
    /// A table seeded from `seed`.
    pub fn seeded(seed: u64) -> Arc<Self> {
        let w = word(seed);
        Arc::new(Self {
            cells: std::array::from_fn(|i| UnsafeCell::new(RGen::new(w.wrapping_add(i as u32)))),
            seed: AtomicU64::new(seed),
        })
    }

    /// Seeds every generator again from `seed`.
    ///
    /// Called where nothing draws at the same time: before the engine runs,
    /// or on the audio thread between blocks (`Cmd::Seed`).
    pub fn reseed(&self, seed: u64) {
        let w = word(seed);
        for (i, cell) in self.cells.iter().enumerate() {
            // SAFETY: see the type's documentation -- nothing draws while the
            // table is reseeded.
            unsafe { *cell.get() = RGen::new(w.wrapping_add(i as u32)) };
        }
        self.seed.store(seed, Ordering::Relaxed);
    }

    /// Records `seed` as the one in force without reseeding: what a network
    /// thread does when it sends the reseed to the engine, so a query that
    /// follows answers the seed asked for rather than the one the audio
    /// thread has not yet applied.
    pub fn note_seed(&self, seed: u64) {
        self.seed.store(seed, Ordering::Relaxed);
    }

    /// The seed the table was last seeded from (or last noted).
    pub fn seed(&self) -> u64 {
        self.seed.load(Ordering::Relaxed)
    }

    #[inline]
    fn get(&self, i: usize) -> RGen {
        // SAFETY: see the type's documentation.
        unsafe { *self.cells[i % GENERATORS].get() }
    }

    #[inline]
    fn put(&self, i: usize, g: RGen) {
        // SAFETY: see the type's documentation.
        unsafe { *self.cells[i % GENERATORS].get() = g }
    }
}

/// A synth's access to its engine's generators: the table, and which of its
/// generators the synth draws from (`RandID`'s choice, 0 unless told).
///
/// Every random UGen of one synth holds a clone, so a `RandID` anywhere in the
/// graph selects for all of them. A draw copies the generator out, draws, and
/// copies it back ([`SynthRand::draw`]), so nothing holds a reference into the
/// table across a nested demand pull.
#[derive(Clone)]
pub struct SynthRand(Arc<Selection>);

struct Selection {
    table: Arc<Generators>,
    index: AtomicU32,
}

impl std::fmt::Debug for SynthRand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SynthRand")
            .field("index", &self.index())
            .field("seed", &self.0.table.seed())
            .finish()
    }
}

impl SynthRand {
    /// A synth's access to `table`, drawing from generator 0.
    pub fn new(table: Arc<Generators>) -> Self {
        Self(Arc::new(Selection {
            table,
            index: AtomicU32::new(0),
        }))
    }

    /// A table of its own, seeded from `seed`: what a synth built outside a
    /// server draws from.
    pub fn detached(seed: u64) -> Self {
        Self::new(Generators::seeded(seed))
    }

    /// Runs `f` on the selected generator, which keeps whatever `f` drew.
    #[inline]
    pub fn draw<R>(&self, f: impl FnOnce(&mut RGen) -> R) -> R {
        let i = self.index();
        let mut g = self.0.table.get(i);
        let r = f(&mut g);
        self.0.table.put(i, g);
        r
    }

    /// Which generator the synth draws from.
    #[inline]
    pub fn index(&self) -> usize {
        self.0.index.load(Ordering::Relaxed) as usize
    }

    /// Selects generator `index` (`RandID`); an index past the table is
    /// ignored, as scsynth ignores it.
    #[inline]
    pub fn select(&self, index: usize) {
        if index < GENERATORS {
            self.0.index.store(index as u32, Ordering::Relaxed);
        }
    }

    /// Reseeds the selected generator (`RandSeed`), as scsynth's
    /// `RGen::init` does.
    #[inline]
    pub fn reseed(&self, seed: u32) {
        self.0.table.put(self.index(), RGen::new(seed));
    }

    /// The table this access draws from.
    pub fn table(&self) -> &Arc<Generators> {
        &self.0.table
    }
}
