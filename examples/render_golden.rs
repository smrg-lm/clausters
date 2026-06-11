//! Regenerates the golden reference WAVs in `tests/golden/`:
//!
//! ```sh
//! cargo run --example render_golden
//! ```
//!
//! Never run automatically — a self-regenerating golden detects nothing.
//! Listen to the new files before committing them.

#[path = "../tests/common/scenes.rs"]
mod scenes;

use std::path::Path;

use clausters::server::render::render_to_wav;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    std::fs::create_dir_all(&dir).expect("create tests/golden");

    let source = std::env::temp_dir().join("clausters_render_golden_source.wav");
    scenes::write_playbuf_source(&source);

    let jobs = [
        ("arpeggio.wav", scenes::arpeggio()),
        ("playbuf.wav", scenes::playbuf(&source)),
    ];
    for (name, score) in jobs {
        let path = dir.join(name);
        let stats =
            render_to_wav(&score, &scenes::config(), &path, "float").expect("render the scene");
        println!(
            "wrote {} — {} frames ({:.3} s), {} events",
            path.display(),
            stats.frames,
            stats.frames as f64 / scenes::SAMPLE_RATE,
            stats.events
        );
    }
    let _ = std::fs::remove_file(&source);
    println!("listen to the new goldens before committing (e.g. ffplay/aplay).");
}
