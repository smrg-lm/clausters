//! The design's guardian: `process_block` must never allocate or free,
//! including while applying commands and discarding freed nodes. If someone
//! sneaks a `Vec::push` or a `format!` into the audio path, this fails.

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use claudesufa::node::AddAction;
use claudesufa::node::default_synth::DefaultSynth;
use claudesufa::server::engine::{BLOCK_SIZE, Cmd, engine_pair};

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

#[test]
fn audio_thread_does_not_allocate() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // Network side (allowed to allocate): pre-build 32 synths.
    for i in 0..32i32 {
        handle
            .send(Cmd::AddSynth {
                id: 1000 + i,
                synth: Box::new(DefaultSynth::new(100.0 + i as f32, 0.01)),
                action: AddAction::Tail,
            })
            .ok()
            .unwrap();
    }

    // Audio side: inserting the synths and processing must not allocate.
    assert_no_alloc(|| {
        for _ in 0..200 {
            engine.process_block(&mut out);
        }
    });

    // Freeing must not allocate either: nodes leave via the garbage FIFO.
    for i in 0..32i32 {
        handle.send(Cmd::FreeNode { id: 1000 + i }).ok().unwrap();
    }
    assert_no_alloc(|| {
        for _ in 0..200 {
            engine.process_block(&mut out);
        }
    });

    // The network side drops them.
    assert_eq!(handle.collect_garbage(), 32);
}
