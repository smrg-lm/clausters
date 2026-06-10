//! Linker configuration for the optional `faust` feature. Without it this
//! script does nothing and the core builds with no libfaust on the system.
//!
//! With `--features faust`, libfaust (built with the LLVM backend) is located
//! through the `FAUST_PREFIX` environment variable, falling back to
//! `~/.local`, then `/usr/local`. Distro packages are not an option on
//! Debian/Ubuntu: `libfaust2t64` ships without the LLVM backend and without
//! headers.

fn main() {
    println!("cargo:rerun-if-env-changed=FAUST_PREFIX");
    if std::env::var_os("CARGO_FEATURE_FAUST").is_none() {
        return;
    }

    let prefix = std::env::var("FAUST_PREFIX").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        let local = format!("{home}/.local");
        if std::path::Path::new(&format!("{local}/lib/libfaust.so")).exists()
            || std::path::Path::new(&format!("{local}/lib/libfaust.a")).exists()
        {
            local
        } else {
            "/usr/local".into()
        }
    });

    println!("cargo:rustc-link-search=native={prefix}/lib");
    println!("cargo:rustc-link-lib=dylib=faust");
    // Tests and binaries must find libfaust.so at runtime without
    // LD_LIBRARY_PATH gymnastics.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
}
