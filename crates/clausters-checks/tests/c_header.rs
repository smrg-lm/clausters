//! The C ABI's header is the one the toolchain reads off `clausters-ffi`.
//!
//! `crates/clausters-ffi/include/clausters_ffi.h` is generated, never written:
//! this test generates it again from the crate's source with cbindgen and
//! fails when the committed one differs, so the file always states the
//! signatures the library exports. It is what the Python client's types are
//! compared against (`clients/python/tests/test_native_parity.py`): a third
//! statement of every signature would be one more thing to keep in step, and
//! a header read back from the crate is the first statement, not a third
//! (`docs/decisions.md`, "The ctypes declarations are checked against a
//! header the crate generates").
//!
//! After a change to the C ABI, regenerate it with
//! `CLAUSTERS_BLESS=1 cargo test -p clausters-checks --test c_header`.

use std::path::PathBuf;

fn ffi() -> PathBuf {
    clausters_checks::root().join("crates/clausters-ffi")
}

/// The header, as cbindgen writes it from the crate's source.
fn generated() -> String {
    let mut config = cbindgen::Config {
        language: cbindgen::Language::C,
        include_guard: Some("CLAUSTERS_FFI_H".into()),
        autogen_warning: Some(
            "/* Generated from crates/clausters-ffi by crates/clausters-checks/tests/c_header.rs; \
             do not edit. */"
                .into(),
        ),
        usize_is_size_t: true,
        documentation: false,
        sys_includes: vec!["stdbool.h".into(), "stddef.h".into(), "stdint.h".into()],
        no_includes: true,
        // A type of the core's that crosses the ABI (the segment's shape) is
        // defined in the header too, so the header compiles on its own.
        parse: cbindgen::ParseConfig {
            parse_deps: true,
            include: Some(vec!["clausters-core".into()]),
            ..Default::default()
        },
        ..Default::default()
    };
    // The two features gate functions; the header names both, so a binding
    // built without one reads the guard rather than a symbol that is not there.
    config
        .defines
        .insert("feature = notation".into(), "CLAUSTERS_FFI_NOTATION".into());
    config
        .defines
        .insert("feature = verovio".into(), "CLAUSTERS_FFI_VEROVIO".into());
    let mut out = Vec::new();
    cbindgen::Builder::new()
        .with_crate(ffi())
        .with_config(config)
        .generate()
        .expect("cbindgen reads crates/clausters-ffi")
        .write(&mut out);
    String::from_utf8(out).expect("the header is text")
}

#[test]
fn the_committed_header_is_the_one_the_crate_generates() {
    let path = ffi().join("include/clausters_ffi.h");
    let fresh = generated();
    if std::env::var_os("CLAUSTERS_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &fresh).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == fresh,
        "{} is not what clausters-ffi generates; regenerate it with \
         CLAUSTERS_BLESS=1 cargo test -p clausters-checks --test c_header",
        path.display()
    );
}

#[test]
fn every_export_is_in_the_header() {
    let header = generated();
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(ffi().join("src")).unwrap() {
        let path = entry.unwrap().path();
        let files: Vec<PathBuf> = if path.is_dir() {
            std::fs::read_dir(&path)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect()
        } else {
            vec![path]
        };
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            for line in text.lines() {
                let Some(rest) = line
                    .strip_prefix("pub extern \"C\" fn ")
                    .or_else(|| line.strip_prefix("pub unsafe extern \"C\" fn "))
                else {
                    continue;
                };
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !header.contains(&format!(" {name}(")) && !header.contains(&format!("*{name}("))
                {
                    missing.push(name);
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "exported by clausters-ffi and absent from the generated header: {}",
        missing.join(", ")
    );
}
