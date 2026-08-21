//! The libverovio binding: the raw C surface, a safe RAII `Toolkit`, and the
//! one-shot `engrave_svg`.
//!
//! Naming of the raw entry points follows the C wrapper verbatim
//! (`vrvToolkit_*`), hence the `non_snake_case` allowance. Every `const char *`
//! verovio returns points into storage the toolkit owns until its next call, so
//! each is copied out immediately (into an owned `String`).

#![allow(non_snake_case)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::{Mutex, MutexGuard};

// The entry points engraving and editing need, named exactly as the C wrapper
// declares them (`third_party/verovio/tools/c_wrapper.h`).
unsafe extern "C" {
    fn vrvToolkit_constructor() -> *mut c_void;
    fn vrvToolkit_constructorResourcePath(resourcePath: *const c_char) -> *mut c_void;
    fn vrvToolkit_destructor(tkPtr: *mut c_void);
    fn vrvToolkit_getVersion(tkPtr: *mut c_void) -> *const c_char;
    fn vrvToolkit_setOptions(tkPtr: *mut c_void, options: *const c_char) -> bool;
    fn vrvToolkit_loadData(tkPtr: *mut c_void, data: *const c_char) -> bool;
    fn vrvToolkit_renderToSVG(
        tkPtr: *mut c_void,
        page_no: c_int,
        xmlDeclaration: bool,
    ) -> *const c_char;
    fn vrvToolkit_renderToTimemap(tkPtr: *mut c_void, c_options: *const c_char) -> *const c_char;
    fn vrvToolkit_getMEI(tkPtr: *mut c_void, options: *const c_char) -> *const c_char;
    fn vrvToolkit_getMIDIValuesForElement(
        tkPtr: *mut c_void,
        xmlId: *const c_char,
    ) -> *const c_char;
    fn vrvToolkit_edit(tkPtr: *mut c_void, editorAction: *const c_char) -> bool;
    fn vrvToolkit_editInfo(tkPtr: *mut c_void) -> *const c_char;
}

/// Serializes every call into libverovio for the process.
///
/// verovio loads its SMuFL resources into global state on toolkit construction,
/// so concurrent construction is a data race — the Python client was shielded by
/// the GIL, the Rust harness runs tests in parallel and is not. A `Toolkit`'s
/// own methods do **not** lock; the caller brackets a whole sequence with this
/// guard (as `engrave_svg` does), the way `faust::ffi_lock` brackets libfaust.
/// The lock is **not** reentrant, so never take it twice on one thread.
pub fn ffi_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// What can go wrong turning score data into geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngraveError {
    /// verovio could not create a toolkit (typically its resources were not found).
    Toolkit,
    /// verovio could not load the score data (unrecognized or malformed input).
    Load,
    /// The input string held an interior NUL, so it cannot cross as a C string.
    NulByte,
}

impl std::fmt::Display for EngraveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngraveError::Toolkit => f.write_str(
                "verovio could not create a toolkit (are its SMuFL resources installed?)",
            ),
            EngraveError::Load => f.write_str("verovio could not load the score data"),
            EngraveError::NulByte => f.write_str("the score data held an interior NUL byte"),
        }
    }
}

impl std::error::Error for EngraveError {}

/// The layout options passed to verovio, matching the Python client's defaults so
/// the geometry is identical.
#[derive(Debug, Clone)]
pub struct EngraveOptions {
    /// Staff size (verovio `scale`). Default `40`.
    pub scale: i32,
    /// Page width in verovio page units the score wraps into systems at
    /// (`pageWidth`). Default `2100`.
    pub page_width: i32,
    /// The page to render (1-based). Default `1`.
    pub page: i32,
    /// The SMuFL resource directory to construct the toolkit with, or `None` to
    /// take the run-time default (`CLAUSTERS_VEROVIO`, else verovio's baked path).
    pub resource_path: Option<String>,
    /// Extra verovio options merged over the defaults, as a JSON object string
    /// (e.g. `{"unit": 6}`), or `None`.
    pub extra: Option<String>,
}

impl Default for EngraveOptions {
    fn default() -> Self {
        Self {
            scale: 40,
            page_width: 2100,
            page: 1,
            resource_path: None,
            extra: None,
        }
    }
}

/// The SMuFL resource directory to construct a toolkit with, or `None` to use
/// verovio's baked-in path.
///
/// Resolution, in precedence: the `CLAUSTERS_VEROVIO` run-time override (a
/// library file or a prefix, resolved as the Python client's `_resources_for`
/// does — a prefix carries the data under `share/verovio`, the staged wheel
/// layout under `<dir>/verovio`); then the build prefix's `share/verovio`, baked
/// in by `build.rs` as a dev-checkout convenience. verovio's own baked path is
/// unreliable — it points at the configure-time prefix, which need not be where
/// the library ends up — so an explicit path is what the Python client has always
/// passed.
pub fn default_resource_path() -> Option<String> {
    if let Some(root) = std::env::var_os("CLAUSTERS_VEROVIO")
        && let Some(dir) = resources_under(std::path::Path::new(&root))
    {
        return Some(dir);
    }
    match option_env!("CLAUSTERS_VEROVIO_RESOURCES") {
        Some(dir) if std::path::Path::new(dir).is_dir() => Some(dir.to_owned()),
        _ => None,
    }
}

/// The SMuFL data directory implied by `root`: a library file names it beside
/// itself, a directory is a staged layout (`<dir>/verovio`) or a build prefix
/// (`<prefix>/share/verovio`).
fn resources_under(root: &std::path::Path) -> Option<String> {
    let dir = if root.is_file() { root.parent()? } else { root };
    for cand in [dir.join("verovio"), dir.join("share").join("verovio")] {
        if cand.is_dir() {
            return cand.to_str().map(str::to_owned);
        }
    }
    None
}

/// One verovio toolkit, freed on drop.
///
/// The methods are lock-free by design: verovio's global resource state makes
/// construction the racy part, so callers serialize whole sequences with
/// [`ffi_lock`] rather than paying a lock per call. Holding a `Toolkit` across
/// threads without that discipline is unsound.
pub struct Toolkit {
    ptr: *mut c_void,
}

impl Toolkit {
    /// Construct a toolkit, optionally pointing it at a SMuFL resource directory
    /// (`None` uses verovio's baked-in path). Serialize with [`ffi_lock`].
    pub fn new(resource_path: Option<&str>) -> Result<Self, EngraveError> {
        let ptr = match resource_path {
            Some(path) => {
                let c = CString::new(path).map_err(|_| EngraveError::NulByte)?;
                // SAFETY: `c` is a valid NUL-terminated string for the call.
                unsafe { vrvToolkit_constructorResourcePath(c.as_ptr()) }
            }
            // SAFETY: no arguments; verovio uses its configured resource path.
            None => unsafe { vrvToolkit_constructor() },
        };
        if ptr.is_null() {
            return Err(EngraveError::Toolkit);
        }
        Ok(Self { ptr })
    }

    /// verovio's version string.
    pub fn version(&self) -> String {
        // SAFETY: `ptr` is a live toolkit; the returned pointer is copied at once.
        unsafe { cstr_to_string(vrvToolkit_getVersion(self.ptr)) }
    }

    /// Set toolkit options from a JSON object string; returns whether verovio
    /// accepted them.
    pub fn set_options(&self, options: &str) -> Result<bool, EngraveError> {
        let c = CString::new(options).map_err(|_| EngraveError::NulByte)?;
        // SAFETY: live toolkit, valid C string.
        Ok(unsafe { vrvToolkit_setOptions(self.ptr, c.as_ptr()) })
    }

    /// Load and lay out score data in any format verovio auto-detects; returns
    /// whether it loaded.
    pub fn load_data(&self, data: &str) -> Result<bool, EngraveError> {
        let c = CString::new(data).map_err(|_| EngraveError::NulByte)?;
        // SAFETY: live toolkit, valid C string.
        Ok(unsafe { vrvToolkit_loadData(self.ptr, c.as_ptr()) })
    }

    /// Render a laid-out page to an SVG string (no XML declaration, matching the
    /// Python client).
    pub fn render_svg(&self, page: i32) -> String {
        // SAFETY: live toolkit; the returned pointer is copied at once.
        unsafe { cstr_to_string(vrvToolkit_renderToSVG(self.ptr, page as c_int, false)) }
    }

    /// The score's timemap as a JSON array string: onset ms -> the ids starting
    /// and stopping there. `options` is a JSON object string.
    pub fn render_timemap(&self, options: &str) -> Result<String, EngraveError> {
        let c = CString::new(options).map_err(|_| EngraveError::NulByte)?;
        // SAFETY: live toolkit, valid C string; the result is copied at once.
        Ok(unsafe { cstr_to_string(vrvToolkit_renderToTimemap(self.ptr, c.as_ptr())) })
    }

    /// The loaded document as MEI, ids and all — the format to persist, and what
    /// an undo snapshot is made of. `options` is a JSON object string.
    pub fn mei(&self, options: &str) -> Result<String, EngraveError> {
        let c = CString::new(options).map_err(|_| EngraveError::NulByte)?;
        // SAFETY: live toolkit, valid C string; the result is copied at once.
        Ok(unsafe { cstr_to_string(vrvToolkit_getMEI(self.ptr, c.as_ptr())) })
    }

    /// The MIDI values verovio computed for one element, as a JSON object string
    /// (`pitch`, `time`, `duration`); empty when the element makes no sound.
    pub fn midi_values(&self, xml_id: &str) -> Result<String, EngraveError> {
        let c = CString::new(xml_id).map_err(|_| EngraveError::NulByte)?;
        // SAFETY: live toolkit, valid C string; the result is copied at once.
        Ok(unsafe { cstr_to_string(vrvToolkit_getMIDIValuesForElement(self.ptr, c.as_ptr())) })
    }

    /// Apply one editor action, given as a JSON object string
    /// (`{"action": …, "param": {…}}`); returns whether verovio accepted it.
    ///
    /// Editing a document that has been loaded but never rendered **segfaults** —
    /// the editor reaches through drawing state the load does not build — so a
    /// page must be drawn first (see [`Score`](crate::Score), which owns that
    /// discipline).
    pub fn edit(&self, action: &str) -> Result<bool, EngraveError> {
        let c = CString::new(action).map_err(|_| EngraveError::NulByte)?;
        // SAFETY: live toolkit, valid C string.
        Ok(unsafe { vrvToolkit_edit(self.ptr, c.as_ptr()) })
    }

    /// What the editor reported about the last action, as a JSON object string.
    pub fn edit_info(&self) -> String {
        // SAFETY: live toolkit; the returned pointer is copied at once.
        unsafe { cstr_to_string(vrvToolkit_editInfo(self.ptr)) }
    }
}

// SAFETY: a toolkit is a raw pointer into libverovio, which has process-wide
// state, and every operation on one is bracketed by `ffi_lock` -- the score
// model takes that guard through `Engraver::lock`, and `engrave_svg` takes it
// directly -- so no two calls are ever inside the library at once. That is what
// makes handing a score to another thread (a GUI client's usual shape) sound;
// the pointer itself is never shared between them.
unsafe impl Send for Toolkit {}

impl Drop for Toolkit {
    fn drop(&mut self) {
        // SAFETY: `ptr` is a live toolkit constructed here; freed exactly once.
        unsafe { vrvToolkit_destructor(self.ptr) };
    }
}

/// Copy a verovio-owned `const char *` into an owned `String` (lossy on invalid
/// UTF-8, which verovio never emits). A null pointer becomes the empty string.
///
/// # Safety
/// `ptr` must be null or point to a NUL-terminated string valid for the read.
unsafe fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: caller guarantees a valid NUL-terminated string.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Engrave `data` (a score in any format verovio auto-detects) into an SVG
/// string — load, draw, discard, all under one [`ffi_lock`].
///
/// The options mirror the Python client's `engrave` so the geometry is
/// identical: `adjustPageHeight`, `svgViewBox` and `breaks: "auto"` on top of the
/// caller's `scale`/`page_width`, with any `extra` merged over them. Returns the
/// rendered SVG, or an [`EngraveError`] if the toolkit could not be built or the
/// data could not be loaded.
pub fn engrave_svg(data: &str, opts: &EngraveOptions) -> Result<String, EngraveError> {
    let options = options_json(opts);
    let resources = opts.resource_path.clone().or_else(default_resource_path);

    let _guard = ffi_lock();
    let tk = Toolkit::new(resources.as_deref())?;
    tk.set_options(&options)?;
    if !tk.load_data(data)? {
        return Err(EngraveError::Load);
    }
    Ok(tk.render_svg(opts.page))
}

/// The verovio options JSON: the fixed defaults plus the caller's scale/width,
/// with `extra` (a JSON object) merged last so it can override any of them.
pub(crate) fn options_json(opts: &EngraveOptions) -> String {
    use serde_json::{Map, Value, json};
    let mut map: Map<String, Value> = json!({
        "scale": opts.scale,
        "adjustPageHeight": true,
        "svgViewBox": true,
        "breaks": "auto",
        "pageWidth": opts.page_width,
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    if let Some(extra) = &opts.extra {
        // A caller passing non-object JSON gets it ignored, not an error, matching
        // the Python client's `dict.update`.
        if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(extra) {
            map.extend(m);
        }
    }
    Value::Object(map).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The same two-bar Plaine & Easie phrase the Python notation tests use.
    const PHRASE: &str = "@clef:G-2\n@timesig:4/4\n@data:4CDEF/ 4GABc'/";

    #[test]
    fn engraves_the_phrase_to_a_definition_scale_svg() {
        let svg = engrave_svg(PHRASE, &EngraveOptions::default()).expect("engraves");
        assert!(svg.contains("<svg"), "an SVG root");
        assert!(
            svg.contains("definition-scale"),
            "the inner definition-scale group"
        );
        assert!(svg.contains("viewBox"), "a viewBox to fit the page");
        // A notehead per note, each a SMuFL <use> reference, plus clef and meter.
        assert!(svg.matches("<use").count() >= 8, "the placed glyphs");
    }

    #[test]
    fn reports_the_verovio_version() {
        let _guard = ffi_lock();
        let tk = Toolkit::new(default_resource_path().as_deref()).expect("toolkit");
        assert!(!tk.version().is_empty(), "a non-empty version string");
    }

    #[test]
    fn rejects_unloadable_data() {
        let err = engrave_svg("this is not a score", &EngraveOptions::default());
        assert_eq!(err, Err(EngraveError::Load));
    }

    #[test]
    fn the_unit_option_is_honored() {
        // A distinct `unit` changes the drawing, so `extra` reaches verovio.
        let a = engrave_svg(PHRASE, &EngraveOptions::default()).expect("default");
        let b = engrave_svg(
            PHRASE,
            &EngraveOptions {
                extra: Some(r#"{"unit": 6}"#.into()),
                ..Default::default()
            },
        )
        .expect("unit 6");
        assert_ne!(a, b, "the unit option must change the geometry");
    }
}
