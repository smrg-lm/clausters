//! The finite-resource id registry: node ids, buses, buffers.

use super::*;
use clausters_core::registry::{self, NodeIdPartition, Registry, ReleaseError};

// --- Finite-resource registry (ABI v10) ---------------------------------
//
// The shared id allocator model (`clausters_core::registry`): node ids, buses
// and buffers are finite boot-time resources, the registry is the occupancy
// map. Handles are internally locked — a client's clock thread allocates
// while its reply thread releases on `/node_end` — and the registry is passive:
// events flow in, nothing calls back out.

/// A registry handle safe to share across the binding's threads.
pub struct FfiRegistry(Mutex<Registry>);

/// A new registry over `[base, base + capacity)`. `capacity` 0 means
/// **unbounded** (the NRT/score mode: allocation never fails, release only
/// keeps the live count). Free with [`clausters_registry_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_registry_new(base: i64, capacity: u64) -> *mut FfiRegistry {
    let reg = if capacity == 0 {
        Registry::unbounded(base)
    } else {
        Registry::new(base, capacity as usize)
    };
    Box::into_raw(Box::new(FfiRegistry(Mutex::new(reg))))
}

/// Frees a registry created by [`clausters_registry_new`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_registry_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_free(h: *mut FfiRegistry) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

fn with_registry<T>(h: *mut FfiRegistry, default: T, f: impl FnOnce(&mut Registry) -> T) -> T {
    // SAFETY: caller guarantees `h` is a live registry handle (or null).
    let Some(reg) = (unsafe { h.as_ref() }) else {
        return default;
    };
    f(&mut reg.0.lock().expect("registry lock poisoned"))
}

/// Allocates `width` contiguous ids; returns the first, or -1 when the space
/// is exhausted (never wraps). `width` 0 counts as 1.
///
/// # Safety
/// `h` must be a live registry handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_alloc(h: *mut FfiRegistry, width: u64) -> i64 {
    with_registry(h, -1, |r| r.alloc(width as usize).unwrap_or(-1))
}

/// Returns `width` ids starting at `first` to the pool. 0 on success, -1 when
/// some id is out of range, -2 when some id is not allocated (double release
/// or foreign id); on error nothing is released.
///
/// # Safety
/// `h` must be a live registry handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_release(
    h: *mut FfiRegistry,
    first: i64,
    width: u64,
) -> i32 {
    with_registry(h, -1, |r| match r.release(first, width as usize) {
        Ok(()) => 0,
        Err(ReleaseError::OutOfRange) => -1,
        Err(ReleaseError::NotAllocated) => -2,
    })
}

/// How many ids are currently allocated.
///
/// # Safety
/// `h` must be a live registry handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_in_use(h: *mut FfiRegistry) -> u64 {
    with_registry(h, 0, |r| r.in_use() as u64)
}

/// The registry's capacity; 0 when unbounded.
///
/// # Safety
/// `h` must be a live registry handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_capacity(h: *mut FfiRegistry) -> u64 {
    with_registry(h, 0, |r| r.capacity().unwrap_or(0) as u64)
}

/// Whether `id` falls inside the registry's space (allocated or not) — the
/// foreign-id filter for `/node_end` handling. 1 yes, 0 no.
///
/// # Safety
/// `h` must be a live registry handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_contains(h: *mut FfiRegistry, id: i64) -> i32 {
    with_registry(h, 0, |r| r.contains(id) as i32)
}

/// Releases everything back to the pool (a client reset).
///
/// # Safety
/// `h` must be a live registry handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_clear(h: *mut FfiRegistry) {
    with_registry(h, (), Registry::clear)
}

/// The boot-derived node-id partition for a node table of `max_nodes` slots.
/// Writes six values into `out`: client base, client capacity, auto base,
/// auto capacity, MIDI base, MIDI capacity. Returns 0, or -1 on a null `out`.
///
/// # Safety
/// `out` must be writable for six `i64`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_registry_node_partition(max_nodes: u64, out: *mut i64) -> i32 {
    if out.is_null() {
        return -1;
    }
    let p = NodeIdPartition::from_max_nodes(max_nodes as usize);
    let vals = [
        p.client_base,
        p.client_capacity as i64,
        p.auto_base,
        p.auto_capacity as i64,
        p.midi_base,
        p.midi_capacity as i64,
    ];
    // SAFETY: caller guarantees `out` is writable for six i64s.
    unsafe { std::slice::from_raw_parts_mut(out, 6) }.copy_from_slice(&vals);
    0
}

/// How much of an audio bus space of `audio_buses` is private to GraphDef
/// instances — what a client subtracts before handing out buses of its own.
///
/// A function of the count and not a constant: the reservation is a *share* of
/// what the server was configured with, so a client asks with the count that
/// server reported rather than assuming the default.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_registry_graph_audio_reserved(audio_buses: usize) -> u64 {
    registry::graph_audio_reserved(audio_buses) as u64
}

/// Control-rate counterpart of [`clausters_registry_graph_audio_reserved`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_registry_graph_control_reserved(control_buses: usize) -> u64 {
    registry::graph_control_reserved(control_buses) as u64
}

// --- The id spaces a client allocates from ---------------------------------
//
// `clausters_core::ids::IdSpaces`: the four spaces sized from the server and
// sliced by a share, the policy every endpoint used to restate over its own
// registries. Internally locked for the same reason a registry is: a client
// allocates on one thread and takes a node back on its reply thread.

use clausters_core::ids::{IdError, IdShare, IdSpaces, ServerShape, Space};

/// The id-spaces handle.
pub struct FfiIdSpaces(Mutex<IdSpaces>);

/// The space a small integer names across the ABI: 0 nodes, 1 audio buses, 2
/// control buses, 3 buffers.
fn space_of(code: i32) -> Option<Space> {
    Some(match code {
        0 => Space::Nodes,
        1 => Space::AudioBuses,
        2 => Space::ControlBuses,
        3 => Space::Buffers,
        _ => return None,
    })
}

/// **A client's id spaces** for a server of this shape, taking share `index`
/// of `of`. `score` non-zero builds the offline variant, whose node space never
/// runs out. Null when the share is not one. Free with
/// [`clausters_ids_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_ids_new(
    max_nodes: u64,
    audio_buses: u64,
    outputs: u64,
    control_buses: u64,
    buffers: u64,
    index: u32,
    of: u32,
    score: i32,
) -> *mut FfiIdSpaces {
    let shape = ServerShape {
        max_nodes: max_nodes as usize,
        audio_buses: audio_buses as usize,
        outputs: outputs as usize,
        control_buses: control_buses as usize,
        buffers: buffers as usize,
    };
    let Ok(share) = IdShare::new(index, of) else {
        return std::ptr::null_mut();
    };
    let spaces = if score != 0 {
        IdSpaces::score(shape)
    } else {
        IdSpaces::new(shape, share)
    };
    Box::into_raw(Box::new(FfiIdSpaces(Mutex::new(spaces))))
}

/// Frees a handle from [`clausters_ids_new`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_ids_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ids_free(h: *mut FfiIdSpaces) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

fn with_ids<T>(h: *mut FfiIdSpaces, default: T, f: impl FnOnce(&mut IdSpaces) -> T) -> T {
    // SAFETY: caller guarantees `h` is a live handle (or null).
    let Some(ids) = (unsafe { h.as_ref() }) else {
        return default;
    };
    f(&mut ids.0.lock().expect("id spaces lock poisoned"))
}

/// A run of `width` ids of `space`; the first, or -1 when the space is
/// exhausted (never wraps) or the space code is unknown.
///
/// # Safety
/// `h` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ids_alloc(h: *mut FfiIdSpaces, space: i32, width: u64) -> i64 {
    let Some(space) = space_of(space) else {
        return -1;
    };
    with_ids(h, -1, |ids| ids.alloc(space, width as usize).unwrap_or(-1))
}

/// Returns a run of `space` to the pool: 0 on success, -1 for ids this space
/// never handed out (a double free) or an unknown space code.
///
/// # Safety
/// `h` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ids_release(
    h: *mut FfiIdSpaces,
    space: i32,
    first: i64,
    width: u64,
) -> i32 {
    let Some(space) = space_of(space) else {
        return -1;
    };
    with_ids(h, -1, |ids| {
        match ids.release(space, first, width as usize) {
            Ok(()) => 0,
            Err(IdError::Exhausted(_) | IdError::NotAllocated(_) | IdError::BadShare) => -1,
        }
    })
}

/// A node the server reports gone, taken back if it was this client's: 1 when
/// it was, 0 for the server's own, another client's or one already taken back.
///
/// # Safety
/// `h` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ids_node_ended(h: *mut FfiIdSpaces, node: i64) -> i32 {
    with_ids(h, 0, |ids| i32::from(ids.node_ended(node)))
}

/// Whether `id` falls inside this client's slice of `space` (1) or not (0).
///
/// # Safety
/// `h` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ids_contains(h: *mut FfiIdSpaces, space: i32, id: i64) -> i32 {
    let Some(space) = space_of(space) else {
        return 0;
    };
    with_ids(h, 0, |ids| i32::from(ids.contains(space, id)))
}

/// How many ids of `space` are allocated now.
///
/// # Safety
/// `h` must be a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ids_in_use(h: *mut FfiIdSpaces, space: i32) -> u64 {
    let Some(space) = space_of(space) else {
        return 0;
    };
    with_ids(h, 0, |ids| ids.in_use(space) as u64)
}

/// **The `(base, span)` of share `index` of `of`** within `span` ids at `base`,
/// written into `out[0..2]`: 0 on success, -1 for a share that is not one.
///
/// # Safety
/// `out` must be writable for two `i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_ids_share_of(
    base: i64,
    span: u64,
    index: u32,
    of: u32,
    out: *mut i64,
) -> i32 {
    let Ok(share) = IdShare::new(index, of) else {
        return -1;
    };
    if out.is_null() {
        return -1;
    }
    let (first, width) = clausters_core::ids::share_of(base, span as usize, share);
    // SAFETY: caller guarantees `out` is writable for two i64.
    unsafe {
        *out = first;
        *out.add(1) = width as i64;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_round_trip_over_the_c_surface() {
        let h = clausters_registry_new(1000, 4);
        unsafe {
            assert_eq!(clausters_registry_alloc(h, 2), 1000);
            assert_eq!(clausters_registry_alloc(h, 2), 1002);
            assert_eq!(clausters_registry_alloc(h, 1), -1, "exhausted, no wrap");
            assert_eq!(clausters_registry_in_use(h), 4);
            assert_eq!(clausters_registry_release(h, 1000, 2), 0);
            assert_eq!(clausters_registry_release(h, 1000, 2), -2, "double release");
            assert_eq!(clausters_registry_release(h, 999, 1), -1, "out of range");
            assert_eq!(clausters_registry_alloc(h, 2), 1000, "released ids reused");
            assert_eq!(clausters_registry_contains(h, 1003), 1);
            assert_eq!(clausters_registry_contains(h, 1004), 0);
            assert_eq!(clausters_registry_capacity(h), 4);
            clausters_registry_clear(h);
            assert_eq!(clausters_registry_in_use(h), 0);
            clausters_registry_free(h);
        }
        // Unbounded (NRT): capacity 0, allocation never fails.
        let h = clausters_registry_new(0, 0);
        unsafe {
            assert_eq!(clausters_registry_capacity(h), 0);
            for i in 0..10_000 {
                assert_eq!(clausters_registry_alloc(h, 1), i);
            }
            clausters_registry_free(h);
        }
    }
}
