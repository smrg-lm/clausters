//! The arguments every Faust factory is created with, on either backend.
//!
//! Shared because the two compilers must agree on them: `-ftz 2` is the
//! architecture-independent half of the denormal rule (see
//! [`crate::dsp::denormals`]) and a def compiled without it in one place and
//! with it in the other is a def whose tails behave differently. The stdlib
//! search is the one thing that differs, and it differs because the platforms
//! do — see [`FaustArgs::defaults`].

use std::ffi::{CString, c_char, c_int};

/// Compiler arguments handed to libfaust as C `argc`/`argv`.
pub struct FaustArgs {
    storage: Vec<CString>,
    ptrs: Vec<*const c_char>,
}

impl FaustArgs {
    /// The arguments every factory is created with:
    ///
    /// - `-I <dir>` for the Faust stdlib (`stdfaust.lib` and friends), so
    ///   both raw-source defs and `faust` fragments inside JSON can
    ///   `import()` it. The directory comes from `$FAUST_PREFIX/share/faust`,
    ///   falling back to `~/.local`, then `/usr/local` — same search order
    ///   as build.rs. **In a page there is no directory to name**: the
    ///   compiler carries the standard library inside its own virtual
    ///   filesystem, where it is already on the search path.
    /// - `-ftz 2`: the generated code flushes recursive variables below the
    ///   normal float range, so decaying tails cannot strand the audio
    ///   thread in slow subnormal math regardless of the host FPU mode (the
    ///   architecture-independent half of [`crate::dsp::denormals`]).
    pub fn defaults() -> Self {
        // `mut` only where there is a directory to add: a page has none.
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut storage = vec![CString::new("-ftz").unwrap(), CString::new("2").unwrap()];
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(dir) = stdlib_dir()
            && let Ok(dir_c) = CString::new(dir)
        {
            storage.push(CString::new("-I").unwrap());
            storage.push(dir_c);
        }
        // CStrings own their bytes on the heap: moving them (or the Vec)
        // does not invalidate these pointers.
        let ptrs = storage.iter().map(|s| s.as_ptr()).collect();
        Self { storage, ptrs }
    }

    pub fn argc(&self) -> c_int {
        self.storage.len() as c_int
    }

    pub fn argv(&self) -> *const *const c_char {
        if self.ptrs.is_empty() {
            std::ptr::null()
        } else {
            self.ptrs.as_ptr()
        }
    }
}

/// Where the Faust standard library is installed. Native only: the page's
/// compiler ships it inside its own virtual filesystem.
#[cfg(not(target_arch = "wasm32"))]
fn stdlib_dir() -> Option<String> {
    let mut prefixes = Vec::new();
    if let Ok(prefix) = std::env::var("FAUST_PREFIX") {
        prefixes.push(prefix);
    }
    if let Ok(home) = std::env::var("HOME") {
        prefixes.push(format!("{home}/.local"));
    }
    prefixes.push("/usr/local".into());
    prefixes
        .into_iter()
        .map(|prefix| format!("{prefix}/share/faust"))
        .find(|dir| std::path::Path::new(dir).exists())
}
