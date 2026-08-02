//! The beat queue behind a client's clock.

use super::*;

// ---- beat-ordered scheduler queue ----
//
// An opaque handle (like `clausters_ws_*`): the host language maps the flat
// `u64` ids back to its routines; only times and ids cross.

/// A new, empty scheduler queue. Free with [`clausters_sched_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_sched_new() -> *mut Scheduler {
    Box::into_raw(Box::new(Scheduler::new()))
}

/// Frees a queue created by [`clausters_sched_new`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_sched_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_free(h: *mut Scheduler) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

/// Queues `id` at beat `time`. Stable for equal times (insertion order).
///
/// # Safety
/// `h` must be a live scheduler handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_push(h: *mut Scheduler, time: f64, id: u64) {
    if let Some(s) = unsafe { h.as_mut() } {
        s.push(time, id);
    }
}

/// Writes the earliest queued beat into `*out_time`; returns 0, or -1 when the
/// queue is empty (out untouched).
///
/// # Safety
/// `h` must be a live scheduler handle and `out_time` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_peek_time(h: *mut Scheduler, out_time: *mut f64) -> i32 {
    let Some(s) = (unsafe { h.as_ref() }) else {
        return -1;
    };
    match s.peek_time() {
        Some(t) if !out_time.is_null() => {
            // SAFETY: caller guarantees `out_time` is writable.
            unsafe { *out_time = t };
            0
        }
        _ => -1,
    }
}

/// Pops the earliest event with time `<= now` into `*out_time`/`*out_id`;
/// returns 0, or -1 when nothing is due.
///
/// # Safety
/// `h` must be a live scheduler handle; `out_time`/`out_id` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_pop_due(
    h: *mut Scheduler,
    now: f64,
    out_time: *mut f64,
    out_id: *mut u64,
) -> i32 {
    let Some(s) = (unsafe { h.as_mut() }) else {
        return -1;
    };
    match s.pop_due(now) {
        Some((t, id)) if !out_time.is_null() && !out_id.is_null() => {
            // SAFETY: caller guarantees the out pointers are writable.
            unsafe {
                *out_time = t;
                *out_id = id;
            }
            0
        }
        _ => -1,
    }
}

/// Removes every queued entry with `id`; returns how many were dropped.
///
/// # Safety
/// `h` must be a live scheduler handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_remove(h: *mut Scheduler, id: u64) -> usize {
    match unsafe { h.as_mut() } {
        Some(s) => s.remove(id),
        None => 0,
    }
}

/// Number of queued entries (0 for a null handle).
///
/// # Safety
/// `h` must be a live scheduler handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_len(h: *mut Scheduler) -> usize {
    unsafe { h.as_ref() }.map_or(0, Scheduler::len)
}

/// Drops every queued entry.
///
/// # Safety
/// `h` must be a live scheduler handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_sched_clear(h: *mut Scheduler) {
    if let Some(s) = unsafe { h.as_mut() } {
        s.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_round_trip_over_the_abi() {
        let h = clausters_sched_new();
        unsafe {
            clausters_sched_push(h, 2.0, 20);
            clausters_sched_push(h, 1.0, 10);
            clausters_sched_push(h, 1.0, 11);
            clausters_sched_push(h, 3.0, 10);
            assert_eq!(clausters_sched_len(h), 4);
            let mut t = 0.0;
            assert_eq!(clausters_sched_peek_time(h, &mut t), 0);
            assert_eq!(t, 1.0);
            assert_eq!(clausters_sched_remove(h, 10), 2);
            let mut id = 0u64;
            assert_eq!(clausters_sched_pop_due(h, 1.0, &mut t, &mut id), 0);
            assert_eq!((t, id), (1.0, 11));
            assert_eq!(clausters_sched_pop_due(h, 1.0, &mut t, &mut id), -1);
            clausters_sched_clear(h);
            assert_eq!(clausters_sched_len(h), 0);
            clausters_sched_free(h);
        }
    }
}
