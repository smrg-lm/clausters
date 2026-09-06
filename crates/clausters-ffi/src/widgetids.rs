//! The widget-id table: a widget id that names what it draws.

use super::*;
use clausters_core::widgetids::WidgetIds;

use crate::document::text;

// --- The widget-id table (ABI v39) --------------------------------------
//
// The GUI namespace has two doors and one occupancy map: an anonymous lease
// for a widget nothing names, and a keyed id for one that draws a structure.
// Handles are internally locked for the same reason the registry's are: a
// client may draw on one thread and answer a host on another.

/// A widget-id table safe to share across the binding's threads.
pub struct FfiWidgetIds(Mutex<WidgetIds>);

/// A new table over `[base, base + capacity)`. `capacity` 0 means
/// **unbounded**. Free with [`clausters_widgetids_free`].
#[unsafe(no_mangle)]
pub extern "C" fn clausters_widgetids_new(base: i64, capacity: u64) -> *mut FfiWidgetIds {
    let ids = if capacity == 0 {
        WidgetIds::unbounded(base)
    } else {
        WidgetIds::new(base, capacity as usize)
    };
    Box::into_raw(Box::new(FfiWidgetIds(Mutex::new(ids))))
}

/// Frees a table created by [`clausters_widgetids_new`] (null is a no-op).
///
/// # Safety
/// `h` must be a pointer from `clausters_widgetids_new`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_free(h: *mut FfiWidgetIds) {
    if !h.is_null() {
        // SAFETY: caller guarantees `h` came from Box::into_raw above.
        drop(unsafe { Box::from_raw(h) });
    }
}

fn with_ids<T>(h: *mut FfiWidgetIds, default: T, f: impl FnOnce(&mut WidgetIds) -> T) -> T {
    // SAFETY: caller guarantees `h` is a live table handle (or null).
    let Some(ids) = (unsafe { h.as_ref() }) else {
        return default;
    };
    f(&mut ids.0.lock().expect("widget id table lock poisoned"))
}

/// A fresh drawer: the owner a begin/retire cycle names. One table serves a
/// whole host, so a drawer is a value the table hands out rather than one a
/// caller invents.
///
/// # Safety
/// `h` must be a live table handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_owner(h: *mut FfiWidgetIds) -> i64 {
    with_ids(h, -1, |ids| ids.owner())
}

/// An id nothing names, or -1 when the space is full.
///
/// # Safety
/// `h` must be a live table handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_alloc(h: *mut FfiWidgetIds) -> i64 {
    with_ids(h, -1, |ids| ids.alloc().unwrap_or(-1))
}

/// Returns an anonymous id to the space. Ids this table never handed out are
/// ignored, so freeing is always safe.
///
/// # Safety
/// `h` must be a live table handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_release(h: *mut FfiWidgetIds, id: i64) {
    with_ids(h, (), |ids| ids.free(id));
}

/// The id that draws `(structure, role, key)`, minted on first ask and the same
/// one after that; -1 when the space is full or a string is not UTF-8.
///
/// # Safety
/// `h` must be a live table handle; `role` and `key` must be valid pointers to
/// `role_len`/`key_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_id_for(
    h: *mut FfiWidgetIds,
    owner: i64,
    structure: i64,
    role: *const u8,
    role_len: usize,
    key: *const u8,
    key_len: usize,
) -> i64 {
    // SAFETY: caller guarantees the two spans.
    let (Some(role), Some(key)) = (unsafe { text(role, role_len) }, unsafe {
        text(key, key_len)
    }) else {
        return -1;
    };
    with_ids(h, -1, |ids| {
        ids.id_for(owner, structure, &role, &key).unwrap_or(-1)
    })
}

/// The id that draws `(structure, role, key)` **if it already has one**, else
/// -1. No minting, and no effect on the draw cycle.
///
/// # Safety
/// As [`clausters_widgetids_id_for`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_id_of(
    h: *mut FfiWidgetIds,
    structure: i64,
    role: *const u8,
    role_len: usize,
    key: *const u8,
    key_len: usize,
) -> i64 {
    // SAFETY: caller guarantees the two spans.
    let (Some(role), Some(key)) = (unsafe { text(role, role_len) }, unsafe {
        text(key, key_len)
    }) else {
        return -1;
    };
    with_ids(h, -1, |ids| ids.id_of(structure, &role, &key).unwrap_or(-1))
}

/// Gives one keyed id back by name, answering the id released or -1.
///
/// # Safety
/// As [`clausters_widgetids_id_for`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_forget(
    h: *mut FfiWidgetIds,
    structure: i64,
    role: *const u8,
    role_len: usize,
    key: *const u8,
    key_len: usize,
) -> i64 {
    // SAFETY: caller guarantees the two spans.
    let (Some(role), Some(key)) = (unsafe { text(role, role_len) }, unsafe {
        text(key, key_len)
    }) else {
        return -1;
    };
    with_ids(h, -1, |ids| {
        ids.forget(structure, &role, &key).unwrap_or(-1)
    })
}

/// Starts `owner`'s draw: every keyed id that owner asks for until the next
/// [`clausters_widgetids_retire`] counts as still drawn.
///
/// # Safety
/// `h` must be a live table handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_begin(h: *mut FfiWidgetIds, owner: i64) {
    with_ids(h, (), |ids| ids.begin(owner));
}

/// Ends `owner`'s draw and takes back every keyed id of that owner's it did not
/// ask for, writing them ascending into `out` and answering how many.
///
/// Answers -1 and retires **nothing** when `cap` is smaller than the number
/// that would be released, so a caller that under-sized its buffer retries
/// rather than losing ids: [`clausters_widgetids_named`] bounds it.
///
/// # Safety
/// `h` must be a live table handle; `out` must be valid for `cap` `i64`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_retire(
    h: *mut FfiWidgetIds,
    owner: i64,
    out: *mut i64,
    cap: usize,
) -> i64 {
    // SAFETY: caller guarantees `h`; the write below is bounded by `cap`.
    let ids = unsafe { h.as_ref() };
    let Some(ids) = ids else { return -1 };
    let mut ids = ids.0.lock().expect("widget id table lock poisoned");
    // Peeked first: `retire` is destructive, so an under-sized buffer has to be
    // refused *before* the ids are gone rather than after.
    if ids.stale(owner) > cap {
        return -1;
    }
    let released = ids.retire(owner);
    if !released.is_empty() {
        if out.is_null() {
            return -1;
        }
        // SAFETY: `released.len() <= cap` was checked above.
        unsafe { std::ptr::copy_nonoverlapping(released.as_ptr(), out, released.len()) };
    }
    released.len() as i64
}

/// How many ids are held, keyed and anonymous together.
///
/// # Safety
/// `h` must be a live table handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_in_use(h: *mut FfiWidgetIds) -> u64 {
    with_ids(h, 0, |ids| ids.in_use() as u64)
}

/// How many of them answer to a name — the bound on a
/// [`clausters_widgetids_retire`] buffer.
///
/// # Safety
/// `h` must be a live table handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_named(h: *mut FfiWidgetIds) -> u64 {
    with_ids(h, 0, |ids| ids.named() as u64)
}

/// Whether `id` falls in this table's space.
///
/// # Safety
/// `h` must be a live table handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_contains(h: *mut FfiWidgetIds, id: i64) -> i32 {
    with_ids(h, 0, |ids| i32::from(ids.contains(id)))
}

/// Drops every name and every id: the table as it was made.
///
/// # Safety
/// `h` must be a live table handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_widgetids_clear(h: *mut FfiWidgetIds) {
    with_ids(h, (), |ids| ids.clear());
}
