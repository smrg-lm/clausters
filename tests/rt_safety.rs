//! The design's guardian: `process_block` must never allocate or free,
//! including while applying commands, moving nodes and discarding freed
//! subtrees. If someone sneaks a `Vec::push` or a `format!` into the audio
//! path, this fails.

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use claudesufa::node::{AddAction, Group, Place, ROOT_NODE_ID, SynthNode};
use claudesufa::server::engine::{BLOCK_SIZE, Cmd, engine_pair};
use claudesufa::synthdef::instance::UGenSynth;
use claudesufa::synthdef::{compile, default_spec};

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

#[test]
fn audio_thread_does_not_allocate() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // Network side (allowed to allocate): a group plus 32 SynthDef instances
    // inside it, all fully built here.
    let def = Arc::new(compile(default_spec()).unwrap());
    handle
        .send(Cmd::AddGroup {
            id: 1,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            group: Group::new(),
        })
        .ok()
        .unwrap();
    for i in 0..32i32 {
        let mut synth = Box::new(UGenSynth::new(Arc::clone(&def)));
        synth.set_control(0, 100.0 + i as f32);
        synth.set_control(1, 0.01);
        handle
            .send(Cmd::AddSynth {
                id: 1000 + i,
                target: 1,
                action: AddAction::Tail,
                synth,
            })
            .ok()
            .unwrap();
    }

    // Audio side: inserting the group and synths and processing must not
    // allocate.
    assert_no_alloc(|| {
        for _ in 0..200 {
            engine.process_block(&mut out);
        }
    });

    // Reordering must not allocate either.
    handle
        .send(Cmd::MoveNode {
            id: 1000,
            target: 1031,
            place: Place::After,
        })
        .ok()
        .unwrap();
    assert_no_alloc(|| {
        for _ in 0..10 {
            engine.process_block(&mut out);
        }
    });

    // Freeing the group recursively frees its 32 synths: everything leaves
    // through the garbage FIFO without a single allocation or drop here.
    handle.send(Cmd::FreeNode { id: 1 }).ok().unwrap();
    assert_no_alloc(|| {
        for _ in 0..200 {
            engine.process_block(&mut out);
        }
    });

    // The network side drops them: the group plus 32 synths.
    assert_eq!(handle.collect_garbage(), 33);
}
