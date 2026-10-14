//! Compile a repository COM binary and wrap it in a standard Linux bundle.
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};
pub fn fixture() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = if manifest.join("tests/fixtures/vst3").is_dir() {
            manifest.to_owned()
        } else {
            manifest.join("crates/clausters-plugin-host")
        };
        let target =
            std::env::temp_dir().join(format!("clausters-vst3-test-{}", std::process::id()));
        let output = Command::new("cargo")
            .args(["build", "--offline", "--manifest-path"])
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
