//! Node id -> slot, in constant time and without allocating.
//!
//! Every command that names a node finds its slot first, on the audio thread,
//! so the lookup is paid once per `/node_set`, map, run, free, move and done
//! action. A scan of the slab grew with the tree -- 0.8 us per `/node_set`
//! over 4000 nodes, so 256 of them took 15% of a block, against 0.06 us through
//! this table (`examples/bench.rs`, "per-command cost") -- so the tree keeps this
//! table beside the slab: open addressing with linear probing, sized once to
//! twice the slab (a power of two), and deletion by backward shift, so a
//! removed id leaves no tombstone and a lookup stays short however long the
//! server runs.

/// A key no node has: `i32::MIN` is never a node id (ids are the client's
/// positive range, the server's ranges above it, and the root's 0).
const EMPTY: i32 = i32::MIN;

pub(super) struct IdIndex {
    keys: Vec<i32>,
    slots: Vec<u32>,
    mask: usize,
}

impl IdIndex {
    /// A table for up to `capacity` ids. Allocates: construction only.
    pub(super) fn with_capacity(capacity: usize) -> Self {
        let size = (capacity.max(1) * 2).next_power_of_two();
        IdIndex {
            keys: vec![EMPTY; size],
            slots: vec![0; size],
            mask: size - 1,
        }
    }

    #[inline]
    fn home(&self, id: i32) -> usize {
        // Fibonacci hashing: consecutive ids -- the usual case -- spread.
        ((id as u32).wrapping_mul(0x9E37_79B9) as usize) & self.mask
    }

    /// The slot `id` lives in.
    #[inline]
    pub(super) fn get(&self, id: i32) -> Option<usize> {
        let mut i = self.home(id);
        loop {
            match self.keys[i] {
                EMPTY => return None,
                k if k == id => return Some(self.slots[i] as usize),
                _ => i = (i + 1) & self.mask,
            }
        }
    }

    /// Records that `id` lives in `slot`. The table is twice the slab, so it
    /// always has room for every node the slab can hold.
    #[inline]
    pub(super) fn insert(&mut self, id: i32, slot: usize) {
        // The translator refuses every id below 1 but the auto `-1`, which it
        // resolves before a node reaches the tree.
        debug_assert_ne!(id, EMPTY, "i32::MIN is the index's empty key");
        let mut i = self.home(id);
        while self.keys[i] != EMPTY && self.keys[i] != id {
            i = (i + 1) & self.mask;
        }
        self.keys[i] = id;
        self.slots[i] = slot as u32;
    }

    /// Forgets `id`, shifting the entries after it back over the hole so no
    /// probe sequence is broken.
    #[inline]
    pub(super) fn remove(&mut self, id: i32) {
        let mut i = self.home(id);
        loop {
            match self.keys[i] {
                EMPTY => return,
                k if k == id => break,
                _ => i = (i + 1) & self.mask,
            }
        }
        let mut hole = i;
        let mut j = i;
        loop {
            j = (j + 1) & self.mask;
            let key = self.keys[j];
            if key == EMPTY {
                break;
            }
            // An entry moves back into the hole unless its home lies
            // cyclically in (hole, j], where the hole is not on its path.
            let home = self.home(key);
            let stays = if hole <= j {
                hole < home && home <= j
            } else {
                hole < home || home <= j
            };
            if !stays {
                self.keys[hole] = key;
                self.slots[hole] = self.slots[j];
                hole = j;
            }
        }
        self.keys[hole] = EMPTY;
    }
}

#[cfg(test)]
mod tests {
    use super::IdIndex;

    #[test]
    fn ids_come_back_after_any_removal_order() {
        let mut index = IdIndex::with_capacity(64);
        for id in 0..64 {
            index.insert(1000 + id, id as usize);
        }
        for id in (0..64).step_by(3) {
            index.remove(1000 + id);
        }
        for id in 0..64 {
            let want = (id % 3 != 0).then_some(id as usize);
            assert_eq!(index.get(1000 + id), want, "id {}", 1000 + id);
        }
        assert_eq!(index.get(5), None);
        index.insert(1000, 7);
        assert_eq!(index.get(1000), Some(7));
    }

    #[test]
    fn colliding_ids_survive_a_removal_between_them() {
        // Same home by construction: the table is small, so many collide.
        let mut index = IdIndex::with_capacity(4);
        let ids: Vec<i32> = (0..4).map(|k| k * 8).collect();
        for (slot, &id) in ids.iter().enumerate() {
            index.insert(id, slot);
        }
        index.remove(ids[1]);
        assert_eq!(index.get(ids[0]), Some(0));
        assert_eq!(index.get(ids[2]), Some(2));
        assert_eq!(index.get(ids[3]), Some(3));
        assert_eq!(index.get(ids[1]), None);
    }
}
