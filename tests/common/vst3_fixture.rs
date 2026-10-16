//! Compile a repository COM binary and wrap it in a standard Linux bundle.
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};
pub fn fixture() -> &'static Path {
    variant(false)
}
/// The same independently loaded fixture with a native three-value enumeration.
// This shared module is also included by suites that only need the plain variant.
#[allow(dead_code)]
pub fn enumerated_fixture() -> &'static Path {
    variant(true)
}
fn variant(enumerated: bool) -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    static ENUMERATED: OnceLock<PathBuf> = OnceLock::new();
    let path = if enumerated { &ENUMERATED } else { &PATH };
    path.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = if manifest.join("tests/fixtures/vst3").is_dir() {
            manifest.to_owned()
        } else {
            manifest.join("crates/clausters-plugin-host")
        };
        let target = std::env::temp_dir().join(format!(
            "clausters-vst3-test-{}-{enumerated}",
            std::process::id()
        ));
        let mut command = Command::new("cargo");
        if enumerated {
            command.args(["build", "--offline", "--features", "enumerated"]);
        } else {
            command.args(["build", "--offline"]);
        }
        let output = command
            .args(["--manifest-path"])
            .arg(root.join("tests/fixtures/vst3/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bundle = target.join("fixture.vst3");
        let directory = bundle.join(format!("Contents/{}-linux", std::env::consts::ARCH));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::copy(
            target.join("debug/libclausters_vst3_fixture.so"),
            directory.join("fixture.so"),
        )
        .unwrap();
        bundle
    })
    .as_path()
}
