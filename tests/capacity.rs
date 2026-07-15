//! M10: bounded-memory audit — every preallocated structure has a defined,
//! non-fatal behavior when it fills (the table lives in
//! `docs/architecture.md`). These tests overflow each one on purpose and
//! assert the engine neither panics, nor blocks, nor stops processing.

#![cfg(feature = "synth")]

use std::sync::Arc;

use clausters::dsp::Limits;
use clausters::node::{AddAction, Group, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{
    BLOCK_SIZE, Cmd, Engine, EngineHandle, engine_pair, engine_pair_full,
};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{compile, default_spec};

const SR: f32 = 48_000.0;

/// An engine sized by explicit boot-time [`Limits`] (S7), the RT counterpart of
/// launching with `--max-nodes`/`--max-buffers`/`--max-graph-children`.
fn engine_with_limits(limits: Limits) -> (Engine, EngineHandle) {
    engine_pair_full(SR, 2, 0, None, 128, 1024, limits)
}

fn default_synth() -> Box<dyn SynthNode> {
    static DEF: std::sync::OnceLock<Arc<clausters::synthdef::SynthDef>> =
        std::sync::OnceLock::new();
    let def = DEF.get_or_init(|| Arc::new(compile(default_spec()).unwrap()));
    Box::new(UGenSynth::new(Arc::clone(def)))
}

fn add(id: i32, target: i32) -> Cmd {
    Cmd::AddSynth {
        id,
        target,
        action: AddAction::Tail,
        synth: default_synth(),
        usage: Default::default(),
    }
}

fn tick(engine: &mut Engine, blocks: usize) {
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for _ in 0..blocks {
        engine.process_block(&mut out);
    }
}

fn synth_count(handle: &EngineHandle) -> u32 {
    handle
        .counters()
        .synths
        .load(std::sync::atomic::Ordering::Relaxed)
}

/// Garbage FIFO (1024) + holding list (64) overflow: the audio thread must
/// `mem::forget` the excess (a bounded leak) instead of blocking or
/// freeing on the RT path — and keep working.
#[test]
fn garbage_overflow_leaks_instead_of_blocking() {
    // A small node slab keeps the boot-scaled garbage FIFO at its 1024 floor
    // (it grows with `2 * max_nodes`), so the overflow stays reachable.
    let (mut engine, mut handle) = engine_with_limits(Limits {
        max_nodes: 512,
        ..Limits::default()
    });
    // 6 rounds × 250 create+free pairs = 1500 dead synths, never collected:
    // well past 1024 (FIFO) + 64 (holding list).
    for round in 0..6 {
        for i in 0..250 {
            let id = 1000 + round * 250 + i;
            handle.send(add(id, ROOT_NODE_ID)).ok().unwrap();
            handle.send(Cmd::FreeNode { id }).ok().unwrap();
        }
        tick(&mut engine, 1);
    }
    // Still alive and processing: a fresh synth must be audible.
    handle.send(add(9000, ROOT_NODE_ID)).ok().unwrap();
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut heard = false;
    for _ in 0..4 {
        engine.process_block(&mut out);
        heard |= out.iter().any(|s| *s != 0.0);
    }
    assert!(heard, "the engine must keep working after garbage overflow");
    assert_eq!(synth_count(&handle), 1);

    // Collection drains the FIFO; the next ticks flush the holding list.
    let mut collected = handle.collect_garbage();
    for _ in 0..4 {
        tick(&mut engine, 1);
        collected += handle.collect_garbage();
    }
    assert!(
        (1024..1500).contains(&collected),
        "collected {collected}: the FIFO and holding list drain, the leaked rest is gone"
    );
}

/// Event FIFO (2048) overflow: `/n_go`//`/n_end` are best-effort — extra
/// events drop silently, nothing else is affected.
#[test]
fn event_overflow_drops_silently() {
    let (mut engine, mut handle) = engine_pair(SR, 2);
    // 600 lifecycles × 2 events = 2400 > 2048, with nobody draining events.
    for round in 0..3 {
        for i in 0..200 {
            let id = 1000 + round * 200 + i;
            handle.send(add(id, ROOT_NODE_ID)).ok().unwrap();
            handle.send(Cmd::FreeNode { id }).ok().unwrap();
        }
        tick(&mut engine, 1);
        handle.collect_garbage(); // keep the garbage FIFO out of the picture
    }
    handle.send(add(9000, ROOT_NODE_ID)).ok().unwrap();
    tick(&mut engine, 1);
    assert_eq!(synth_count(&handle), 1, "the engine state stays exact");
}

/// A full node slab (configured here at 1024, root included): extra synths
/// come back as `RejectedSynth` garbage; the tree caps exactly at capacity.
#[test]
fn full_node_slab_rejects_gracefully() {
    let (mut engine, mut handle) = engine_with_limits(Limits {
        max_nodes: 1024,
        ..Limits::default()
    });
    // 1100 adds in FIFO-sized chunks; the slab fits 1023 beside the root.
    for chunk in 0..4 {
        for i in 0..275 {
            handle
                .send(add(1000 + chunk * 275 + i, ROOT_NODE_ID))
                .ok()
                .unwrap();
        }
        tick(&mut engine, 1);
    }
    assert_eq!(synth_count(&handle), 1023, "slab capacity, root included");
    let collected = handle.collect_garbage();
    assert_eq!(
        collected,
        1100 - 1023,
        "every rejection rolls back as garbage"
    );
    tick(&mut engine, 2); // still processing
}

/// Non-root groups pre-reserve their child slots (configured here at 256):
/// the 257th add is rejected, never grown (growing would allocate on the
/// audio thread).
#[test]
fn full_group_rejects_extra_children() {
    let (mut engine, mut handle) = engine_pair(SR, 2);
    handle
        .send(Cmd::AddGroup {
            id: 1,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            // The capacity travels with the pre-built group (the translator
            // sizes it from `--max-graph-children`); 256 here.
            group: Group::with_capacity(256),
        })
        .ok()
        .unwrap();
    for chunk in 0..2 {
        for i in 0..150 {
            handle.send(add(1000 + chunk * 150 + i, 1)).ok().unwrap();
        }
        tick(&mut engine, 1);
    }
    assert_eq!(synth_count(&handle), 256, "group child capacity");
    assert_eq!(handle.collect_garbage(), 300 - 256);
}

/// S7: the node slab is boot-time configurable. A small `--max-nodes` overflows
/// exactly at its capacity, root included — same graceful rejection as the
/// default 1024, just sooner.
#[test]
fn small_max_nodes_overflows_predictably() {
    let limits = Limits {
        max_nodes: 16,
        ..Limits::default()
    };
    let (mut engine, mut handle) = engine_with_limits(limits);
    for i in 0..40 {
        handle.send(add(1000 + i, ROOT_NODE_ID)).ok().unwrap();
    }
    tick(&mut engine, 1);
    // 16 slots, one taken by the root: 15 synths fit, the other 25 bounce back.
    assert_eq!(synth_count(&handle), 15, "slab capacity, root included");
    assert_eq!(handle.collect_garbage(), 40 - 15, "the rest roll back");
    tick(&mut engine, 2); // still processing
}

/// S7: `--max-graph-children` sizes each non-root group. A group built with a
/// small child capacity rejects the extra children, exactly like the default.
#[test]
fn custom_group_child_cap_rejects_extra() {
    let limits = Limits {
        max_nodes: 64,
        max_group_children: 8,
        ..Limits::default()
    };
    let (mut engine, mut handle) = engine_with_limits(limits);
    handle
        .send(Cmd::AddGroup {
            id: 1,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            group: Group::with_capacity(limits.max_group_children),
        })
        .ok()
        .unwrap();
    for i in 0..20 {
        handle.send(add(1000 + i, 1)).ok().unwrap();
    }
    tick(&mut engine, 1);
    assert_eq!(synth_count(&handle), 8, "group child capacity");
    assert_eq!(handle.collect_garbage(), 20 - 8);
}

/// The cache-line alignment of `Block` (M10) is a compile-time guarantee.
#[test]
fn blocks_are_cache_line_aligned() {
    use clausters::dsp::{BLOCK_SIZE as B, Block};
    assert_eq!(align_of::<Block>(), 64);
    assert_eq!(size_of::<Block>(), B * 4, "alignment must not add padding");
    let wires = vec![Block::SILENCE; 3];
    for w in &wires {
        assert_eq!((w as *const Block as usize) % 64, 0);
    }
}
