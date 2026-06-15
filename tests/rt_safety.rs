//! The design's guardian: `process_block` must never allocate or free,
//! including while applying commands, moving nodes and discarding freed
//! subtrees. If someone sneaks a `Vec::push` or a `format!` into the audio
//! path, this fails.

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use clausters::node::{AddAction, Group, Place, ROOT_NODE_ID, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{compile, default_spec};

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
                usage: Default::default(),
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

/// Same guardian for the M11 mapping path: reading a control bus and sampling
/// an audio bus into mapped controls at the start of every block must not
/// allocate — `map_control` only flips entries in a pre-allocated table.
#[test]
fn mapped_controls_do_not_allocate_on_the_audio_thread() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    let def = Arc::new(compile(default_spec()).unwrap());
    for i in 0..16i32 {
        let synth = Box::new(UGenSynth::new(Arc::clone(&def)));
        handle
            .send(Cmd::AddSynth {
                id: 1000 + i,
                target: ROOT_NODE_ID,
                action: AddAction::Tail,
                synth,
                usage: Default::default(),
            })
            .ok()
            .unwrap();
        // freq follows a control bus, amp samples an audio bus: both branches
        // of the per-block apply get exercised.
        handle
            .send(Cmd::MapControl {
                id: 1000 + i,
                index: 0,
                bus: 5,
                audio: false,
            })
            .ok()
            .unwrap();
        handle
            .send(Cmd::MapControl {
                id: 1000 + i,
                index: 1,
                bus: 6,
                audio: true,
            })
            .ok()
            .unwrap();
    }
    handle
        .send(Cmd::SetControlBus {
            index: 5,
            value: 220.0,
        })
        .ok()
        .unwrap();

    assert_no_alloc(|| {
        for _ in 0..200 {
            engine.process_block(&mut out);
        }
    });
}

/// Same guardian for the scheduler (M6): enqueuing timed bundles, splitting
/// blocks at their offsets and executing them must not allocate — the spent
/// `Vec` shells leave through the garbage FIFO with their capacity intact.
#[test]
fn scheduled_bundles_do_not_allocate_on_the_audio_thread() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // Network side: 16 bundles at odd offsets inside the first blocks, each
    // adding a synth, retuning another and freeing a third.
    let def = Arc::new(compile(default_spec()).unwrap());
    for i in 0..16u64 {
        let mut synth = Box::new(UGenSynth::new(Arc::clone(&def)));
        synth.set_control(1, 0.01);
        let cmds = vec![
            Cmd::AddSynth {
                id: 1000 + i as i32,
                target: ROOT_NODE_ID,
                action: AddAction::Tail,
                synth,
                usage: Default::default(),
            },
            Cmd::SetControl {
                id: 1000 + i.saturating_sub(1) as i32,
                index: 0,
                value: 200.0 + i as f32,
            },
            Cmd::SetControlBus {
                index: 7,
                value: i as f32,
            },
        ];
        handle
            .send(Cmd::Schedule {
                time: i * 37 + 13, // never on a block boundary
                cmds,
            })
            .ok()
            .unwrap();
    }

    // Audio side: enqueue (sorted insert), split, execute — no allocation.
    assert_no_alloc(|| {
        for _ in 0..50 {
            engine.process_block(&mut out);
        }
    });

    // 16 spent shells came back through the garbage FIFO.
    assert_eq!(handle.collect_garbage(), 16);
}

/// Same guardian for the buffer path (M5): installing, replacing and freeing
/// pool buffers, and `PlayBuf` reading them, must not allocate on the audio
/// thread — swapped-out buffers leave as garbage, never dropped there.
#[test]
fn buffer_swaps_do_not_allocate_on_the_audio_thread() {
    use clausters::dsp::buffer::Buffer;
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // Network side: a looping PlayBuf synth plus the first buffer.
    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "player",
            "ugens": [
                {"kind": "PlayBuf", "inputs": [
                    {"const": 0.0}, {"const": 0.0}, {"const": 0.5}, {"const": 1.0}
                ]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
            ]
        }"#,
    )
    .unwrap();
    let def = Arc::new(compile(spec).unwrap());
    let make_buffer = |frames: usize| {
        Some(Arc::new(Buffer::new(
            vec![0.1; frames],
            1,
            frames,
            48_000.0,
        )))
    };
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: make_buffer(4800),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(def)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();

    assert_no_alloc(|| {
        for _ in 0..100 {
            engine.process_block(&mut out);
        }
    });

    // Replacing mid-playback (including a shrink) and emptying the slot are
    // pointer swaps; the old Arcs go out through the garbage FIFO.
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: make_buffer(100),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::SetBuffer { index: 0, buffer: None })
        .ok()
        .unwrap();
    handle.send(Cmd::FreeNode { id: 1000 }).ok().unwrap();
    assert_no_alloc(|| {
        for _ in 0..100 {
            engine.process_block(&mut out);
        }
    });

    // Two buffers plus the synth drop here, on the network side.
    assert_eq!(handle.collect_garbage(), 3);
}

/// Same guardian for the feedback path (`LocalIn`/`LocalOut`): the per-synth
/// `locals` buffer is allocated at build time (network side); reading and
/// writing it each block is plain slice copies, no allocation.
#[test]
fn local_feedback_does_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // A feedback loop through composed UGens: LocalIn -> *0.9 -> LocalOut,
    // seeded by an impulse, plus the fed-back value summed to bus 0.
    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "fb",
            "ugens": [
                {"kind": "LocalIn",  "inputs": [{"const": 0.0}]},
                {"kind": "Impulse",  "inputs": [{"const": 0.0}]},
                {"kind": "Add",      "inputs": [{"ugen": 0}, {"ugen": 1}]},
                {"kind": "Mul",      "inputs": [{"ugen": 2}, {"const": 0.9}]},
                {"kind": "Out",      "inputs": [{"const": 0.0}, {"ugen": 0}]},
                {"kind": "LocalOut", "inputs": [{"const": 0.0}, {"ugen": 3}]}
            ]
        }"#,
    )
    .unwrap();
    let def = Arc::new(compile(spec).unwrap());
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(def)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();

    assert_no_alloc(|| {
        for _ in 0..200 {
            engine.process_block(&mut out);
        }
    });

    handle.send(Cmd::FreeNode { id: 1000 }).ok().unwrap();
    assert_no_alloc(|| {
        for _ in 0..50 {
            engine.process_block(&mut out);
        }
    });
    assert_eq!(handle.collect_garbage(), 1);
}

/// Same guardian for the Faust path (F3): inserting, processing, recontrolling
/// and freeing `FaustSynth`s must not allocate on the audio thread. This
/// guards our wrapper (staging copies, zone stores, garbage routing) —
/// `compute` itself is JIT code whose C-side mallocs, if any, would bypass
/// the Rust allocator hook; Faust documents it allocation-free after init.
#[cfg(feature = "faust")]
#[test]
fn faust_synths_do_not_allocate_on_the_audio_thread() {
    use clausters::faust::compiler::{CompilePayload, CompileRequest, CompilerThread};
    use clausters::faust::synth::FaustSynth;

    // Compilation and instantiation allocate freely: network/compiler side.
    let compiler = CompilerThread::spawn();
    compiler
        .submit(CompileRequest {
            name: "rt_sine".into(),
            payload: CompilePayload::Source(
                r#"
                wrap(x) = x - floor(x);
                freq = hslider("freq", 440.0, 20.0, 20000.0, 0.01);
                process = sin(6.283185307179586 * ((+(freq/48000.0) : wrap) ~ _)) * 0.01;
                "#
                .into(),
            ),
            client: clausters::osc::ClientId::Udp("127.0.0.1:1".parse().unwrap()),
        })
        .ok()
        .unwrap();
    let def = Arc::new(
        compiler
            .recv_result_timeout(std::time::Duration::from_secs(10))
            .expect("compilation must finish")
            .outcome
            .expect("def must compile"),
    );

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    for i in 0..8i32 {
        let mut synth = Box::new(FaustSynth::new(Arc::clone(&def), 48_000.0).unwrap());
        synth.set_control(0, 100.0 + i as f32);
        handle
            .send(Cmd::AddSynth {
                id: 2000 + i,
                target: ROOT_NODE_ID,
                action: AddAction::Tail,
                synth,
                usage: Default::default(),
            })
            .ok()
            .unwrap();
    }

    assert_no_alloc(|| {
        for _ in 0..200 {
            engine.process_block(&mut out);
        }
    });

    // Zone writes via SetControl are plain stores: still no allocation.
    handle
        .send(Cmd::SetControl {
            id: 2000,
            index: 0,
            value: 880.0,
        })
        .ok()
        .unwrap();
    assert_no_alloc(|| {
        for _ in 0..10 {
            engine.process_block(&mut out);
        }
    });

    // Freeing routes the instances out through the garbage FIFO untouched.
    for i in 0..8i32 {
        handle.send(Cmd::FreeNode { id: 2000 + i }).ok().unwrap();
    }
    assert_no_alloc(|| {
        for _ in 0..100 {
            engine.process_block(&mut out);
        }
    });
    assert_eq!(handle.collect_garbage(), 8);
}

/// M13: the conductor side of parallel dispatch — stage partition (bitops),
/// the publish/steal/wait protocol (atomics, bounded spins, at worst an
/// `unpark` syscall) — must not allocate either. The workers run the same
/// `process` code path verified above; their threads are spawned (and
/// allocate) only at engine creation, outside the audio path.
#[test]
fn parallel_dispatch_does_not_allocate() {
    use clausters::server::engine::engine_pair_with_workers;
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair_with_workers(48_000.0, 2, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // Sources on disjoint buses so stages genuinely fan out to the workers.
    handle
        .send(Cmd::AddGroup {
            id: 1,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            group: Group::new(),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::SetGroupParallel {
            id: 1,
            parallel: true,
        })
        .ok()
        .unwrap();
    for i in 0..16i32 {
        let spec: SynthDefSpec = serde_json::from_value(serde_json::json!({
            "name": format!("p{i}"),
            "ugens": [
                {"kind": "SinOsc", "inputs": [{"const": 110.0 + i as f64}]},
                {"kind": "Out", "inputs": [{"const": 16.0 + i as f64}, {"ugen": 0}]}
            ]
        }))
        .unwrap();
        let synth = Box::new(UGenSynth::new(Arc::new(compile(spec).unwrap())));
        handle
            .send(Cmd::AddSynth {
                id: 3000 + i,
                target: 1,
                action: AddAction::Tail,
                synth,
                usage: {
                    let mut u = clausters::dsp::BusUsage::default();
                    u.mark(16.0 + i as f32, false, true);
                    u
                },
            })
            .ok()
            .unwrap();
    }

    assert_no_alloc(|| {
        for _ in 0..300 {
            engine.process_block(&mut out);
        }
    });
}
