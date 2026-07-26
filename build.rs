//! Linker configuration for the `faust` feature (on by default). Without it
//! this script does nothing and the core builds with no libfaust on the system
//! (`--no-default-features`, plus the other features you want).
//!
//! libfaust must be built with the LLVM backend; it is located through the
//! `FAUST_PREFIX` environment variable, falling back to `~/.local`, then
//! `/usr/local`. Distro packages are not an option on Debian/Ubuntu:
//! `libfaust2t64` ships without the LLVM backend and without headers. BUILD.md
//! has the from-source recipe.
//!
//! The rpath keeps the artifacts **relocatable**: `$ORIGIN` and
//! `$ORIGIN/../_libs` come before the build-time prefix, which is what lets the
//! Python wheel ship libfaust (and the libLLVM it needs) beside the binary and
//! the cdylibs, the way the GUI host binary is already bundled. It is emitted as
//! `DT_RPATH` (`--disable-new-dtags`) rather than `DT_RUNPATH` because only
//! `DT_RPATH` is inherited by *transitive* dependencies: libfaust carries no
//! rpath of its own, so its libLLVM is found through ours.

fn main() {
    println!("cargo:rerun-if-env-changed=FAUST_PREFIX");
    if std::env::var_os("CARGO_FEATURE_FAUST").is_none() {
        return;
    }

    let prefix = std::env::var("FAUST_PREFIX").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        let local = format!("{home}/.local");
        if has_libfaust(&local) {
            local
        } else {
            "/usr/local".into()
        }
    });

    if !has_libfaust(&prefix) {
        println!(
            "cargo:warning=no libfaust under {prefix}/lib: the `faust` feature is on by default \
             and needs libfaust built with the LLVM backend (BUILD.md has the recipe). Point \
             FAUST_PREFIX at it, or build a SynthDef-only server with --no-default-features."
        );
    }

    println!("cargo:rustc-link-search=native={prefix}/lib");
    println!("cargo:rustc-link-lib=dylib=faust");
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../_libs");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
}

/// Whether libfaust is installed under `<prefix>/lib` — telling cargo, on the
/// way, to re-run this script when that directory changes.
///
/// The `rerun-if-changed` is what keeps the answer from going stale. Emitting
/// any `rerun-if-*` turns off cargo's default "re-run when a file in the package
/// changes", so `FAUST_PREFIX` would otherwise be the *only* trigger — and a
/// cached "not found" then survives the very install that fixes it: you build
/// libfaust into the prefix, cargo replays a resolution made before it existed,
/// and the link fails against a prefix that has none. The library appearing in
/// the directory is the event that matters, so that is what we watch.
fn has_libfaust(prefix: &str) -> bool {
    println!("cargo:rerun-if-changed={prefix}/lib");
    std::path::Path::new(&format!("{prefix}/lib/libfaust.so")).exists()
        || std::path::Path::new(&format!("{prefix}/lib/libfaust.a")).exists()
}
