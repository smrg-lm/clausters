//! Server delay compensation with a SynthDef convolver, no native plugin.
//!
//! Run `cargo run --example delay_compensation`. No device is needed. The dry
//! and one-tap convolved paths share a source but different intrinsic latency.
//! Manual ordering emits two impulses, at 0 and 256 samples; automatic ordering
//! emits their aligned sum at 256, across two nested groups. WAVs are written
//! to examples/out so they can be inspected or played. The same server planner
//! consumes the declared latency of any SynthNode, including isolated plugins.
#[path = "support/compensation_scene.rs"]
mod scene;
use clausters::server::render::{RenderConfig, render_to_wav};
fn main() -> Result<(), String> {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/out");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    for (automatic, name) in [(false, "delay-manual.wav"), (true, "delay-compensated.wav")] {
        let path = directory.join(name);
        let config = RenderConfig {
            sample_rate: scene::RATE,
            channels: 1,
            ..RenderConfig::default()
        };
        let stats = render_to_wav(&scene::score(automatic), &config, &path, "float")?;
        println!(
            "{}: {} frames, peak {:?}",
            path.display(),
            stats.frames,
            stats.peak
        );
    }
    Ok(())
}
