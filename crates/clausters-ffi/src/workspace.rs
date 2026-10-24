//! **An editor's working folder** over the C ABI
//! (`clausters_core::workspace`): where a client's audio editor writes its
//! renders, on disk under the user's cache, held by a lock while the handle
//! lives and removed when it is freed, or by the next editor to open when its
//! process ended without freeing it.
//!
//! Three calls: `clausters_core_workspace_create` makes one and answers an
//! opaque handle, `clausters_core_workspace_path` reads its folder, and
//! `clausters_core_workspace_free` removes it.

use std::ffi::{CStr, CString, c_char};

use clausters_core::workspace::Workspace;

/// A working folder and its path as a C string, alive as long as the handle.
pub struct WorkspaceHandle {
    /// Removed with the handle, when it drops.
    _held: Workspace,
    path: CString,
}

/// **Makes a working folder named after `name`**, after removing those no
/// session holds any longer. Null where `name` is not UTF-8 or the folder
/// could not be made. Free it with `clausters_core_workspace_free`.
///
/// # Safety
/// `name` is a NUL-terminated C string (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_workspace_create(
    name: *const c_char,
) -> *mut WorkspaceHandle {
    if name.is_null() {
        return std::ptr::null_mut();
    }
    let Ok(name) = unsafe { CStr::from_ptr(name) }.to_str() else {
        return std::ptr::null_mut();
    };
    let Ok(held) = Workspace::create(name) else {
        return std::ptr::null_mut();
    };
    let Ok(path) = CString::new(held.path().to_string_lossy().into_owned()) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(WorkspaceHandle { _held: held, path }))
}

/// The folder of `handle`, valid until it is freed; null for a null handle.
///
/// # Safety
/// `handle` is null or one `clausters_core_workspace_create` answered and not
/// freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_workspace_path(
    handle: *const WorkspaceHandle,
) -> *const c_char {
    match unsafe { handle.as_ref() } {
        Some(handle) => handle.path.as_ptr(),
        None => std::ptr::null(),
    }
}

/// **Removes the folder of `handle`** and frees it; a null handle is nothing.
///
/// # Safety
/// `handle` is null or one `clausters_core_workspace_create` answered and not
/// freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_core_workspace_free(handle: *mut WorkspaceHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_is_made_read_and_removed() {
        let name = CString::new("ffi-test").unwrap();
        let handle = unsafe { clausters_core_workspace_create(name.as_ptr()) };
        assert!(!handle.is_null());
        let path = unsafe { CStr::from_ptr(clausters_core_workspace_path(handle)) }
            .to_str()
            .unwrap()
            .to_string();
        assert!(std::path::Path::new(&path).is_dir());
        unsafe { clausters_core_workspace_free(handle) };
        assert!(!std::path::Path::new(&path).exists());
        assert!(unsafe { clausters_core_workspace_create(std::ptr::null()) }.is_null());
    }
}
