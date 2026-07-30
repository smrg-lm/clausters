//! The design's guardian: `process_block` must never allocate or free,
//! including while applying commands, moving nodes and discarding freed
//! subtrees. If someone sneaks a `Vec::push` or a `format!` into the audio
//! path, this fails.

#![cfg(feature = "synth")]

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use clausters::clausters_core::rng::SEED_STRIDE;
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
        let mut synth = Box::new(UGenSynth::new(Arc::clone(&def), 48_000.0, SEED_STRIDE));
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
        let synth = Box::new(UGenSynth::new(Arc::clone(&def), 48_000.0, SEED_STRIDE));
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
        let mut synth = Box::new(UGenSynth::new(Arc::clone(&def), 48_000.0, SEED_STRIDE));
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
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
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: None,
        })
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

/// The S5 table oscillators and waveshaper (`Osc`/`VOsc`/`Shaper`) read the
/// wavetable pool on the audio thread the same way `PlayBuf` does — pointer
/// lookups and interpolation, never an allocation. The buffers themselves are
/// generated on the NRT thread (`/b_gen`), so the audio thread only reads.
#[test]
fn table_oscillators_do_not_allocate_on_the_audio_thread() {
    use clausters::dsp::buffer::Buffer;
    use clausters::dsp::wavetable::{GenCommand, GenFlags};
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // Two wavetables (built off the audio thread) for VOsc to crossfade, plus a
    // cheby transfer table for Shaper.
    let wt = |amps: Vec<f32>| {
        let flags = GenFlags {
            normalize: true,
            wavetable: true,
            clear: true,
        };
        Some(Arc::new(
            GenCommand::Sine1 { flags, amps }.apply(&Buffer::zeroed(2048, 1, 48_000.0)),
        ))
    };
    let cheby = Some(Arc::new(
        GenCommand::Cheby {
            flags: GenFlags {
                normalize: true,
                wavetable: true,
                clear: true,
            },
            coeffs: vec![0.0, 0.0, 1.0],
        }
        .apply(&Buffer::zeroed(2048, 1, 48_000.0)),
    ));
    for (index, buffer) in [(0, wt(vec![1.0])), (1, wt(vec![0.0, 1.0])), (2, cheby)] {
        handle.send(Cmd::SetBuffer { index, buffer }).ok().unwrap();
    }

    // Osc + VOsc + Shaper all reading the pool, summed to bus 0.
    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "tables",
            "ugens": [
                {"kind": "Osc",  "inputs": [{"const": 0.0}, {"const": 220.0}, {"const": 0.0}]},
                {"kind": "VOsc", "inputs": [{"const": 0.5}, {"const": 110.0}, {"const": 0.0}]},
                {"kind": "Sine", "inputs": [{"const": 55.0}]},
                {"kind": "Shaper", "inputs": [{"const": 2.0}, {"ugen": 2}]},
                {"kind": "Sum3", "inputs": [{"ugen": 0}, {"ugen": 1}, {"ugen": 3}]},
                {"kind": "Out",  "inputs": [{"const": 0.0}, {"ugen": 4}]}
            ]
        }"#,
    )
    .unwrap();
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(
                Arc::new(compile(spec).unwrap()),
                48_000.0,
                SEED_STRIDE,
            )),
            usage: Default::default(),
        })
        .ok()
        .unwrap();

    assert_no_alloc(|| {
        for _ in 0..100 {
            engine.process_block(&mut out);
        }
    });
}

/// Same guardian for the frequency-domain chain (S8): the `FFT` input buffer,
/// the `IFFT` overlap-add tail and the synth-private `SpectralChain` frame are
/// all allocated at build time (network side); the per-hop forward and inverse
/// transforms run in pre-allocated scratch, so the whole chain — including the
/// blocks that cross a hop boundary and run a transform — never allocates.
#[test]
fn spectral_chain_does_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // Sine -> FFT -> PV_BrickWall -> PV_MagAbove -> PV_Kernel -> IFFT -> Out.
    // A 512-point window with a 128-sample hop, so a transform fires every
    // other block. The `PV_Kernel` runs both bin-expression programs (M29) —
    // its pre-allocated eval stack and the polar phase path included.
    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "fftchain",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "FFT", "inputs": [{"ugen": 0}, {"const": 1.0}],
                 "fft_size": 512, "hop": 0.25, "wintype": 0},
                {"kind": "PV_BrickWall", "inputs": [{"ugen": 1}, {"const": 0.3}]},
                {"kind": "PV_MagAbove", "inputs": [{"ugen": 2}, {"const": 0.0}]},
                {"kind": "PV_Kernel", "inputs": [{"ugen": 3}, {"const": 0.5}],
                 "mag_expr": ["mag", "mag", "p0", "ge", "mul"],
                 "phase_expr": ["phase", 0.1, "add"]},
                {"kind": "IFFT", "inputs": [{"ugen": 4}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 5}]}
            ]
        }"#,
    )
    .unwrap();
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(
                Arc::new(compile(spec).unwrap()),
                48_000.0,
                SEED_STRIDE,
            )),
            usage: Default::default(),
        })
        .ok()
        .unwrap();

    assert_no_alloc(|| {
        for _ in 0..100 {
            engine.process_block(&mut out);
        }
    });
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
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

/// The rate substrate (S1): the `ir` init pass and the demand pull path must
/// not allocate on the audio thread. `Rand.ir` runs its init once on the first
/// block; `Demand`/`Dseq` step the sub-list every block. Both live entirely in
/// `UGenSynth::process`, so this guards the `step` closure and init skip.
#[test]
fn rate_substrate_does_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // ir: Rand drawn once at init, then held. dr: an Impulse-driven Dseq.
    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "rates",
            "ugens": [
                {"kind": "Rand", "inputs": [{"const": 100.0}, {"const": 200.0}]},
                {"kind": "SampleRate", "inputs": []},
                {"kind": "Impulse", "inputs": [{"const": 750.0}]},
                {"kind": "Dseq", "rate": "dr",
                 "inputs": [{"const": 0.0}, {"const": 1.0}, {"const": 2.0}, {"const": 3.0}]},
                {"kind": "Demand", "inputs": [{"ugen": 2}, {"const": 0.0}, {"ugen": 3}]},
                {"kind": "Mul", "rate": "kr", "inputs": [{"ugen": 4}, {"ugen": 0}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 5}]}
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
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

/// The demand family (U8): every source and both drivers, nested three deep,
/// pulled and reset every block. The recursion runs on the audio thread — this
/// is what says it never allocates a frame of it — and driving all fourteen
/// rows from one def also checks each one's arity against the registry.
#[test]
fn demand_family_does_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // A `Dseq` over every other source (so each is drained and restarted every
    // pass), a `Dstutter` over that, a `Dswitch1` picking between the two
    // drivers' streams, and a reset trigger firing on its own clock.
    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "demandall",
            "ugens": [
                {"kind": "Dseries", "rate": "dr",
                 "inputs": [{"const": 3.0}, {"const": 0.0}, {"const": 1.0}]},
                {"kind": "Dgeom", "rate": "dr",
                 "inputs": [{"const": 3.0}, {"const": 1.0}, {"const": 2.0}]},
                {"kind": "Dwhite", "rate": "dr",
                 "inputs": [{"const": 2.0}, {"const": 0.0}, {"const": 1.0}]},
                {"kind": "Diwhite", "rate": "dr",
                 "inputs": [{"const": 2.0}, {"const": 0.0}, {"const": 9.0}]},
                {"kind": "Dbrown", "rate": "dr",
                 "inputs": [{"const": 2.0}, {"const": 0.0}, {"const": 1.0}, {"const": 0.2}]},
                {"kind": "Dibrown", "rate": "dr",
                 "inputs": [{"const": 2.0}, {"const": 0.0}, {"const": 9.0}, {"const": 2.0}]},
                {"kind": "Drand", "rate": "dr",
                 "inputs": [{"const": 2.0}, {"const": 4.0}, {"const": 5.0}]},
                {"kind": "Dxrand", "rate": "dr",
                 "inputs": [{"const": 2.0}, {"const": 6.0}, {"const": 7.0}]},
                {"kind": "Dshuf", "rate": "dr",
                 "inputs": [{"const": 1.0}, {"const": 8.0}, {"const": 9.0}]},
                {"kind": "Dbufrd", "rate": "dr",
                 "inputs": [{"const": 0.0}, {"const": 0.0}, {"const": 1.0}, {"const": 0.0}]},
                {"kind": "Dseq", "rate": "dr",
                 "inputs": [{"const": 0.0}, {"ugen": 0}, {"ugen": 1}, {"ugen": 2},
                            {"ugen": 3}, {"ugen": 4}, {"ugen": 5}, {"ugen": 6},
                            {"ugen": 7}, {"ugen": 8}, {"ugen": 9}]},
                {"kind": "Dstutter", "rate": "dr", "inputs": [{"const": 2.0}, {"ugen": 10}]},
                {"kind": "Dseries", "rate": "dr",
                 "inputs": [{"const": 0.0}, {"const": 0.0}, {"const": 1.0}]},
                {"kind": "Dswitch1", "rate": "dr",
                 "inputs": [{"ugen": 12}, {"ugen": 11}, {"const": 0.5}]},
                {"kind": "Dseq", "rate": "dr",
                 "inputs": [{"const": 0.0}, {"const": 0.001}, {"const": 0.002}]},
                {"kind": "Impulse", "inputs": [{"const": 7000.0}]},
                {"kind": "Impulse", "inputs": [{"const": 11.0}]},
                {"kind": "Demand", "inputs": [{"ugen": 15}, {"ugen": 16}, {"ugen": 13}]},
                {"kind": "Duty", "inputs": [{"ugen": 14}, {"ugen": 16},
                                            {"ugen": 13}, {"const": 0.0}]},
                {"kind": "TDuty", "inputs": [{"ugen": 14}, {"ugen": 16}, {"ugen": 13},
                                             {"const": 0.0}, {"const": 1.0}]},
                {"kind": "Sum3", "inputs": [{"ugen": 17}, {"ugen": 18}, {"ugen": 19}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 20}]}
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
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

/// Typed controls (S2): the trigger reset, the compile-inserted `Lag`, the
/// scalar `/n_set` reject and ordinary sets must all stay allocation-free on
/// the audio thread. The def mixes a `tr`, a lagged `kr` and an `ir` control.
#[test]
fn typed_controls_do_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "typed",
            "controls": [
                {"name": "gate", "default": 0.0, "rate": "tr"},
                {"name": "freq", "default": 200.0, "lag": 0.05},
                {"name": "seed", "default": 1.0, "rate": "ir"}
            ],
            "ugens": [
                {"kind": "Sine", "inputs": [{"control": 1}]},
                {"kind": "Mul", "inputs": [{"ugen": 0}, {"control": 0}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();

    // A trigger set, a lagged set, and an (ignored) scalar set — applied by the
    // engine at the top of the block, all on the audio thread.
    handle
        .send(Cmd::SetControl {
            id: 1000,
            index: 0,
            value: 1.0,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::SetControl {
            id: 1000,
            index: 1,
            value: 800.0,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::SetControl {
            id: 1000,
            index: 2,
            value: 7.0,
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

/// Operator UGens (S3): the generic `UnaryOpUGen`/`BinaryOpUGen` selected by an
/// opcode index and the fused `MulAdd`/`Sum3`/`Sum4` must stay allocation-free
/// on the audio thread. They only call `clausters_core::builtins`, which writes
/// into caller-provided slices, so there is nothing to allocate — this guards
/// that the wiring keeps it so.
#[test]
fn operator_ugens_do_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // Sine -> distort -> *0.3 (mul) -> MulAdd -> Sum3 -> Sum4.
    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "ops",
            "ugens": [
                {"kind": "Sine", "inputs": [{"const": 440.0}]},
                {"kind": "UnaryOpUGen", "op": "distort", "inputs": [{"ugen": 0}]},
                {"kind": "BinaryOpUGen", "op": "mul", "inputs": [{"ugen": 1}, {"const": 0.3}]},
                {"kind": "MulAdd", "inputs": [{"ugen": 2}, {"const": 1.0}, {"const": 0.0}]},
                {"kind": "Sum3", "inputs": [{"ugen": 3}, {"const": 0.0}, {"const": 0.0}]},
                {"kind": "Sum4",
                 "inputs": [{"ugen": 4}, {"const": 0.0}, {"const": 0.0}, {"const": 0.0}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 5}]}
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
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
            client: Some(clausters::osc::ClientId::Udp(
                "127.0.0.1:1".parse().unwrap(),
            )),
            cache: None,
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
        let mut synth = Box::new(
            FaustSynth::new(
                Arc::clone(&def),
                48_000.0,
                &clausters::dsp::buffer::empty_pool(),
            )
            .unwrap(),
        );
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

/// Same guardian for the envelope path: `EnvGen` running its segments, the
/// gate release and — crucially — the `doneAction` free must all leave the
/// audio thread through the garbage FIFO without allocating. The finished node
/// is queued and freed inside `process_block`.
#[test]
fn envgen_free_self_does_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // 16 one-shot envelopes with doneAction = 2: each finishes and frees
    // itself within a few blocks, exercising the queue-and-free path.
    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "env",
            "ugens": [
                {"kind": "EnvGen", "inputs": [
                    {"const": 1.0}, {"const": 1.0}, {"const": 0.0}, {"const": 1.0},
                    {"const": 2.0}, {"const": 0.0}, {"const": 1.0}, {"const": -1.0},
                    {"const": -1.0},
                    {"const": 1.0}, {"const": 0.002}, {"const": 1.0}, {"const": 0.0}
                ]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
            ]
        }"#,
    )
    .unwrap();
    let def = Arc::new(compile(spec).unwrap());
    for i in 0..16i32 {
        handle
            .send(Cmd::AddSynth {
                id: 1000 + i,
                target: ROOT_NODE_ID,
                action: AddAction::Tail,
                synth: Box::new(UGenSynth::new(Arc::clone(&def), 48_000.0, SEED_STRIDE)),
                usage: Default::default(),
            })
            .ok()
            .unwrap();
    }

    // Running the envelopes and freeing every finished node: no allocation.
    assert_no_alloc(|| {
        for _ in 0..200 {
            engine.process_block(&mut out);
        }
    });

    // All 16 freed themselves and came back through the garbage FIFO.
    assert_eq!(handle.collect_garbage(), 16);
}

/// S4: the relative done actions (sibling resolution + free/pause) and `/n_run`
/// (pause/resume toggle) run on the audio thread — during the done drain and
/// the command apply — and must not allocate. The scene fires a
/// `freeSelfAndNext` inside a group and toggles the group's run flag.
#[test]
fn relative_done_actions_and_n_run_do_not_allocate() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // A one-shot envelope with doneAction = 4 (freeSelfAndNext).
    let env: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "env",
            "ugens": [
                {"kind": "EnvGen", "inputs": [
                    {"const": 1.0}, {"const": 1.0}, {"const": 0.0}, {"const": 1.0},
                    {"const": 4.0}, {"const": 0.0}, {"const": 1.0}, {"const": -1.0},
                    {"const": -1.0},
                    {"const": 1.0}, {"const": 0.002}, {"const": 1.0}, {"const": 0.0}
                ]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
            ]
        }"#,
    )
    .unwrap();
    let plain: SynthDefSpec = serde_json::from_str(
        r#"{"name":"p","ugens":[{"kind":"Out","inputs":[{"const":0.0},{"const":0.0}]}]}"#,
    )
    .unwrap();
    let env = Arc::new(compile(env).unwrap());
    let plain = Arc::new(compile(plain).unwrap());

    handle
        .send(Cmd::AddGroup {
            id: 1,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            group: Group::new(),
        })
        .ok()
        .unwrap();
    // Actor (fires freeSelfAndNext) plus neighbours, all inside the group.
    for i in 0..4i32 {
        let def = if i == 0 { &env } else { &plain };
        handle
            .send(Cmd::AddSynth {
                id: 1000 + i,
                target: 1,
                action: AddAction::Tail,
                synth: Box::new(UGenSynth::new(Arc::clone(def), 48_000.0, SEED_STRIDE)),
                usage: Default::default(),
            })
            .ok()
            .unwrap();
    }
    // Toggle the whole group's run flag (pause then resume) — applied on the
    // audio thread in the command loop.
    handle
        .send(Cmd::RunNode { id: 1, run: false })
        .ok()
        .unwrap();
    handle.send(Cmd::RunNode { id: 1, run: true }).ok().unwrap();

    assert_no_alloc(|| {
        for _ in 0..200 {
            engine.process_block(&mut out);
        }
    });
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
                {"kind": "Sine", "inputs": [{"const": 110.0 + i as f64}]},
                {"kind": "Out", "inputs": [{"const": 16.0 + i as f64}, {"ugen": 0}]}
            ]
        }))
        .unwrap();
        let synth = Box::new(UGenSynth::new(
            Arc::new(compile(spec).unwrap()),
            48_000.0,
            SEED_STRIDE,
        ));
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

/// OSC command-set completion (S6): the new commands that reach the audio
/// thread must stay allocation-free. `Cmd::MoveNode` with `Place::Head`/`Tail`
/// (`/g_head`/`/g_tail`/`/n_order`), a `/u_cmd` payload routed to a UGen
/// instance, and `Cmd::ClearSched` draining the timed-bundle queue to the
/// garbage FIFO all run inside `process_block` — none may allocate.
#[test]
fn command_set_completion_does_not_allocate_on_the_audio_thread() {
    use clausters::dsp::{UGenCmd, ugen_cmd_selector};

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // A group with two synths inside it.
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
    for id in [1000, 1001] {
        let mut synth = Box::new(UGenSynth::new(Arc::clone(&def), 48_000.0, SEED_STRIDE));
        synth.set_control(1, 0.01);
        handle
            .send(Cmd::AddSynth {
                id,
                target: 1,
                action: AddAction::Tail,
                synth,
                usage: Default::default(),
            })
            .ok()
            .unwrap();
    }

    // A far-future scheduled bundle so ClearSched has something to drain.
    handle
        .send(Cmd::Schedule {
            time: 1_000_000,
            cmds: vec![Cmd::SetControl {
                id: 1000,
                index: 0,
                value: 440.0,
            }],
        })
        .ok()
        .unwrap();

    // Move to head/tail, route a /u_cmd to UGen 0 (default handler ignores it),
    // and flush the schedule queue — all applied at the top of a block.
    handle
        .send(Cmd::MoveNode {
            id: 1001,
            target: 1,
            place: Place::Head,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::MoveNode {
            id: 1001,
            target: 1,
            place: Place::Tail,
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::UGenCommand {
            id: 1000,
            ugen_index: 0,
            command: UGenCmd {
                selector: ugen_cmd_selector("noop"),
                args: [1.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                num_args: 2,
            },
        })
        .ok()
        .unwrap();
    handle.send(Cmd::ClearSched).ok().unwrap();

    assert_no_alloc(|| {
        for _ in 0..100 {
            engine.process_block(&mut out);
        }
    });

    // The drained bundle shell leaves through the garbage FIFO.
    handle.send(Cmd::FreeNode { id: 1 }).ok().unwrap();
    assert_no_alloc(|| {
        for _ in 0..50 {
            engine.process_block(&mut out);
        }
    });
}

/// S7: live input feeds `process_block` at block start by popping a lock-free
/// ring into the input buses — no allocation on the audio thread. Push frames,
/// then process under the alloc guard.
#[test]
fn hardware_input_path_does_not_allocate() {
    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    // 1 input channel -> audio bus 2; an In->Out passthru reads it.
    let mut tx = engine.input_ring(1, BLOCK_SIZE * 16);
    let spec: clausters::synthdef::SynthDefSpec = serde_json::from_value(serde_json::json!({
        "name": "passthru",
        "ugens": [
            {"kind": "In", "inputs": [{"const": 2.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]
    }))
    .unwrap();
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(
                Arc::new(compile(spec).unwrap()),
                48_000.0,
                SEED_STRIDE,
            )),
            usage: Default::default(),
        })
        .ok()
        .unwrap();

    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    assert_no_alloc(|| {
        for _ in 0..200 {
            // Refill the ring (pushing is the "input callback" side, allowed to
            // be exercised here); the pop inside process_block must not alloc.
            for i in 0..BLOCK_SIZE {
                let _ = tx.push(i as f32 * 1e-4);
            }
            engine.process_block(&mut out);
        }
    });
}

/// S9: the side-effect UGens buffer triggers and the engine drains them into
/// the reply FIFO every block — all on the audio thread, with no allocation.
/// An `Impulse` fires the three reply UGens repeatedly; the FIFO is never
/// popped here (no network thread), so it fills and drops, which must also not
/// allocate.
#[test]
fn reply_ugens_do_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // One Impulse drives SendTrig, SendReply and Poll (an output-less def:
    // no Out at all).
    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "sidefx",
            "ugens": [
                {"kind": "Impulse",   "inputs": [{"const": 3000.0}]},
                {"kind": "SendTrig",  "inputs": [{"ugen": 0}, {"const": 7.0}, {"const": 0.5}]},
                {"kind": "SendReply", "label": "/custom",
                 "inputs": [{"ugen": 0}, {"const": 42.0}, {"const": 1.5}, {"const": 2.5}]},
                {"kind": "Poll", "label": "watch",
                 "inputs": [{"ugen": 0}, {"const": 0.25}, {"const": 3.0}]}
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();

    assert_no_alloc(|| {
        for _ in 0..200 {
            engine.process_block(&mut out);
        }
    });
}

/// Audio taps (the `/tap` oscilloscope path): recording live buses into the
/// segment's tap rings every block must not allocate either — the write is
/// one memcpy plus one atomic store per tap.
#[test]
fn tap_writes_do_not_allocate_on_the_audio_thread() {
    use clausters::dsp::Limits;
    use clausters::server::engine::engine_pair_full;
    use clausters::server::ipc::Segment;

    let segment = Segment::in_memory_full(1024, 2, 4096);
    let (mut engine, mut handle) =
        engine_pair_full(48_000.0, 2, 0, Some(segment), 128, 1024, Limits::default());
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    let def = Arc::new(compile(default_spec()).unwrap());
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    handle.send(Cmd::SetTap { tap: 0, bus: 0 }).ok().unwrap();
    handle.send(Cmd::SetTap { tap: 1, bus: 1 }).ok().unwrap();

    assert_no_alloc(|| {
        for _ in 0..200 {
            engine.process_block(&mut out);
        }
    });
}

/// The phase family (U1): every one of them is a pure accumulator plus
/// arithmetic, so none should touch the allocator. `Phasor` is included because
/// it carries trigger state, and the `LF*` shapes because they latch their
/// initial phase on the first block — a lazily built table there would show up
/// here.
#[test]
fn phase_ugens_do_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    // Every kind at once, summed: Saw + Pulse + LFSaw + LFPulse + LFTri +
    // VarSaw, with Phasor driving a modulated pulse width so no input is a
    // plain constant (the scalar fast path must not be the only one covered).
    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "phase",
            "ugens": [
                {"kind": "Phasor", "inputs": [{"const": 0.0}, {"const": 0.0001},
                                              {"const": 0.1}, {"const": 0.9},
                                              {"const": 0.1}]},
                {"kind": "Saw", "inputs": [{"const": 110.0}]},
                {"kind": "Pulse", "inputs": [{"const": 220.0}, {"ugen": 0}]},
                {"kind": "LFSaw", "inputs": [{"const": 3.0}, {"const": 0.25}]},
                {"kind": "LFPulse", "inputs": [{"const": 5.0}, {"const": 0.0}, {"ugen": 0}]},
                {"kind": "LFTri", "inputs": [{"const": 7.0}, {"const": 0.5}]},
                {"kind": "VarSaw", "inputs": [{"const": 2.0}, {"const": 0.0}, {"ugen": 0}]},
                {"kind": "Sum4", "inputs": [{"ugen": 1}, {"ugen": 2},
                                            {"ugen": 3}, {"ugen": 4}]},
                {"kind": "Sum3", "inputs": [{"ugen": 7}, {"ugen": 5}, {"ugen": 6}]},
                {"kind": "BinaryOpUGen", "op": "mul",
                 "inputs": [{"ugen": 8}, {"const": 0.1}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 9}]}
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
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

/// The filter core (U2): the state-variable rows and the one-pole family. The
/// coefficient path is the thing worth guarding — it computes a `tan` and a
/// reciprocal per block, and a lazily built table or a `Vec` of interpolated
/// coefficients there would allocate on the audio thread. Both parameter paths
/// are exercised: constants take the block fast path, the modulated `RLPF`
/// takes the interpolating one.
#[test]
fn filter_ugens_do_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "filters",
            "ugens": [
                {"kind": "Saw", "inputs": [{"const": 110.0}]},
                {"kind": "LFTri", "inputs": [{"const": 2.0}, {"const": 0.0}]},
                {"kind": "MulAdd", "inputs": [{"ugen": 1}, {"const": 900.0}, {"const": 1200.0}]},
                {"kind": "LPF", "inputs": [{"ugen": 0}, {"const": 900.0}]},
                {"kind": "HPF", "inputs": [{"ugen": 3}, {"const": 80.0}]},
                {"kind": "RLPF", "inputs": [{"ugen": 4}, {"ugen": 2}, {"const": 0.2}]},
                {"kind": "BPF", "inputs": [{"ugen": 5}, {"const": 700.0}, {"const": 0.5}]},
                {"kind": "BRF", "inputs": [{"ugen": 6}, {"const": 300.0}, {"const": 0.5}]},
                {"kind": "Svf", "inputs": [{"ugen": 7}, {"ugen": 2}, {"const": 0.4},
                                           {"const": 1.0}, {"ugen": 1}, {"const": 0.2}]},
                {"kind": "OnePole", "inputs": [{"ugen": 8}, {"const": 0.7}]},
                {"kind": "LeakDC", "inputs": [{"ugen": 9}, {"const": 0.995}]},
                {"kind": "BinaryOpUGen", "op": "mul",
                 "inputs": [{"ugen": 10}, {"const": 0.1}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 11}]}
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
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

/// The delay core (U3): the line is allocated in `build`, on the network
/// thread, and never resized — which is exactly the claim worth a guard, since
/// a `Vec` that grew to accommodate a longer delay would allocate here. All
/// three interpolations and both feedback forms are exercised, with a modulated
/// delay time so the per-sample path runs too.
#[test]
fn delay_ugens_do_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "delays",
            "ugens": [
                {"kind": "Saw", "inputs": [{"const": 110.0}]},
                {"kind": "LFTri", "inputs": [{"const": 1.0}, {"const": 0.0}]},
                {"kind": "MulAdd", "inputs": [{"ugen": 1}, {"const": 0.004}, {"const": 0.006}]},
                {"kind": "DelayN", "inputs": [{"ugen": 0}, {"const": 0.01}],
                 "max_delay": 0.05},
                {"kind": "DelayL", "inputs": [{"ugen": 3}, {"ugen": 2}],
                 "max_delay": 0.02},
                {"kind": "DelayC", "inputs": [{"ugen": 4}, {"ugen": 2}],
                 "max_delay": 0.02},
                {"kind": "CombC", "inputs": [{"ugen": 5}, {"ugen": 2}, {"const": 0.5}],
                 "max_delay": 0.02},
                {"kind": "AllpassC", "inputs": [{"ugen": 6}, {"ugen": 2}, {"const": 0.3}],
                 "max_delay": 0.02},
                {"kind": "AllpassN", "inputs": [{"ugen": 7}, {"const": 0.007}, {"const": 0.3}],
                 "max_delay": 0.02},
                {"kind": "BinaryOpUGen", "op": "mul",
                 "inputs": [{"ugen": 8}, {"const": 0.1}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 9}]}
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
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

/// The one-segment ramps and the node-control set (U4). Two claims: `Line`
/// deriving its step on the first sample keeps that state in the UGen and
/// touches no heap, and the `DoneQuery` path — the synth reading one UGen's
/// done flag on behalf of another — adds no allocation to the block either. The
/// ramp is short enough to finish and re-report inside the run, with
/// `doneAction` 0 so the node survives to keep being measured.
#[test]
fn ramp_and_node_control_ugens_do_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "ramps",
            "ugens": [
                {"kind": "Line", "inputs": [{"const": 100.0}, {"const": 900.0},
                                            {"const": 0.001}, {"const": 0.0}]},
                {"kind": "XLine", "rate": "kr", "inputs": [{"const": 0.05},
                                            {"const": 0.4}, {"const": 0.002},
                                            {"const": 0.0}]},
                {"kind": "Done", "rate": "kr", "inputs": [{"ugen": 0}]},
                {"kind": "Saw", "inputs": [{"ugen": 0}]},
                {"kind": "BinaryOpUGen", "op": "mul", "inputs": [{"ugen": 3}, {"ugen": 1}]},
                {"kind": "BinaryOpUGen", "op": "add", "inputs": [{"ugen": 4}, {"ugen": 2}]},
                {"kind": "PauseSelf", "inputs": [{"const": -1.0}]},
                {"kind": "BinaryOpUGen", "op": "mul", "inputs": [{"ugen": 5}, {"ugen": 6}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 7}]}
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
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

/// The trigger family (U5). The interesting claim is not that a comparator
/// allocates — it cannot — but that the *counters* do not: `Stepper`,
/// `PulseCount` and `Timer` all hold running state, and `Decay` recomputes a
/// transcendental whose scalar fast path must not reach for a buffer. The
/// scene runs every state machine in the module at once, driven by a real
/// trigger stream rather than by silence.
#[test]
fn trigger_ugens_do_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "trigs",
            "ugens": [
                {"kind": "Impulse", "inputs": [{"const": 200.0}]},
                {"kind": "Impulse", "inputs": [{"const": 7.0}]},
                {"kind": "Trig", "inputs": [{"ugen": 0}, {"const": 0.001}]},
                {"kind": "Trig1", "inputs": [{"ugen": 0}, {"const": 0.002}]},
                {"kind": "TDelay", "inputs": [{"ugen": 0}, {"const": 0.003}]},
                {"kind": "Latch", "inputs": [{"ugen": 2}, {"ugen": 0}]},
                {"kind": "Gate", "inputs": [{"ugen": 3}, {"ugen": 4}]},
                {"kind": "Schmidt", "inputs": [{"ugen": 5}, {"const": 0.2},
                                               {"const": 0.8}]},
                {"kind": "ToggleFF", "rate": "ar", "inputs": [{"ugen": 0}]},
                {"kind": "SetResetFF", "rate": "ar",
                 "inputs": [{"ugen": 0}, {"ugen": 1}]},
                {"kind": "PulseCount", "rate": "ar",
                 "inputs": [{"ugen": 0}, {"ugen": 1}]},
                {"kind": "PulseDivider", "rate": "ar",
                 "inputs": [{"ugen": 0}, {"const": 3.0}, {"const": 0.0}]},
                {"kind": "Stepper", "rate": "ar", "inputs": [{"ugen": 0}, {"ugen": 1},
                 {"const": 0.0}, {"const": 7.0}, {"const": 1.0}, {"const": 0.0}]},
                {"kind": "Timer", "rate": "ar", "inputs": [{"ugen": 0}]},
                {"kind": "Sweep", "inputs": [{"ugen": 1}, {"const": 2.0}]},
                {"kind": "Changed", "inputs": [{"ugen": 12}, {"const": 0.1}]},
                {"kind": "Decay", "inputs": [{"ugen": 0}, {"const": 0.05}]},
                {"kind": "Decay2", "inputs": [{"ugen": 0}, {"ugen": 13},
                                              {"const": 0.2}]},
                {"kind": "DetectSilence", "inputs": [{"ugen": 17}, {"const": 0.001},
                 {"const": 0.05}, {"const": 0.0}]},
                {"kind": "Sum4", "inputs": [{"ugen": 6}, {"ugen": 7},
                                            {"ugen": 8}, {"ugen": 9}]},
                {"kind": "Sum4", "inputs": [{"ugen": 10}, {"ugen": 11},
                                            {"ugen": 14}, {"ugen": 15}]},
                {"kind": "Sum4", "inputs": [{"ugen": 16}, {"ugen": 17},
                                            {"ugen": 18}, {"ugen": 19}]},
                {"kind": "BinaryOpUGen", "op": "add",
                 "inputs": [{"ugen": 20}, {"ugen": 21}]},
                {"kind": "BinaryOpUGen", "op": "mul",
                 "inputs": [{"ugen": 22}, {"const": 0.01}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 23}]}
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
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

/// The noise family (U6). Nothing here has a buffer to grow, so the claim is
/// narrow — but `PinkNoise` carries a dice table, `LFNoise*` a per-segment
/// state machine and `Dust` a per-sample division, and the scene is what would
/// catch any of them reaching for the heap later.
#[test]
fn noise_ugens_do_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "noise",
            "ugens": [
                {"kind": "WhiteNoise", "inputs": []},
                {"kind": "PinkNoise", "inputs": []},
                {"kind": "BrownNoise", "inputs": []},
                {"kind": "GrayNoise", "inputs": []},
                {"kind": "ClipNoise", "inputs": []},
                {"kind": "LFNoise0", "inputs": [{"const": 300.0}]},
                {"kind": "LFNoise1", "inputs": [{"const": 120.0}]},
                {"kind": "LFNoise2", "inputs": [{"ugen": 5}]},
                {"kind": "LFClipNoise", "inputs": [{"const": 90.0}]},
                {"kind": "Dust", "inputs": [{"const": 400.0}]},
                {"kind": "Dust2", "inputs": [{"ugen": 6}]},
                {"kind": "Crackle", "inputs": [{"const": 1.5}]},
                {"kind": "Sum4", "inputs": [{"ugen": 0}, {"ugen": 1},
                                            {"ugen": 2}, {"ugen": 3}]},
                {"kind": "Sum4", "inputs": [{"ugen": 4}, {"ugen": 5},
                                            {"ugen": 6}, {"ugen": 7}]},
                {"kind": "Sum4", "inputs": [{"ugen": 8}, {"ugen": 9},
                                            {"ugen": 10}, {"ugen": 11}]},
                {"kind": "Sum3", "inputs": [{"ugen": 12}, {"ugen": 13},
                                            {"ugen": 14}]},
                {"kind": "BinaryOpUGen", "op": "mul",
                 "inputs": [{"ugen": 15}, {"const": 0.05}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 16}]}
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
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

/// The pan family (U7). None of these holds a buffer either, so what the scene
/// is really guarding is the shape of the code: eleven rows share one
/// polynomial and one two-by-two product, and every one of them runs its law
/// per sample when its parameter is audio rate. That per-sample path is where a
/// `sin()` call, a temporary or a lazily built table would hide, so the scene
/// drives every row from an audio-rate parameter rather than a constant.
#[test]
fn pan_ugens_do_not_allocate_on_the_audio_thread() {
    use clausters::synthdef::SynthDefSpec;

    let (mut engine, mut handle) = engine_pair(48_000.0, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];

    let spec: SynthDefSpec = serde_json::from_str(
        r#"{
            "name": "pan",
            "ugens": [
                {"kind": "WhiteNoise", "inputs": []},
                {"kind": "Sine", "inputs": [{"const": 220.0}]},
                {"kind": "Sine", "inputs": [{"const": 0.7}]},

                {"kind": "Pan2", "inputs": [{"ugen": 0}, {"ugen": 2},
                                            {"const": 1.0}, {"const": 0.0}]},
                {"kind": "Pan2", "inputs": [{"ugen": 0}, {"ugen": 2},
                                            {"const": 1.0}, {"const": 1.0}]},
                {"kind": "LinPan2", "inputs": [{"ugen": 1}, {"ugen": 2},
                                               {"const": 1.0}, {"const": 0.0}]},
                {"kind": "Balance2", "inputs": [{"ugen": 3}, {"ugen": 4},
                                                {"ugen": 2}, {"const": 1.0},
                                                {"const": 1.0}]},
                {"kind": "Rotate2", "inputs": [{"ugen": 3}, {"ugen": 4},
                                               {"ugen": 2}, {"const": 0.0}]},
                {"kind": "MidSide", "inputs": [{"ugen": 3}, {"ugen": 4},
                                               {"const": 1.0}]},
                {"kind": "StereoWidth", "inputs": [{"ugen": 3}, {"ugen": 4},
                                                   {"ugen": 2}, {"const": 0.0}]},
                {"kind": "PanAz", "inputs": [{"ugen": 1}, {"ugen": 2},
                                             {"const": 1.0}, {"const": 2.0},
                                             {"const": 0.5}, {"const": 4.0},
                                             {"const": 2.0}]},
                {"kind": "XFade2", "inputs": [{"ugen": 0}, {"ugen": 1},
                                              {"ugen": 2}, {"const": 1.0}]},
                {"kind": "LinXFade2", "inputs": [{"ugen": 0}, {"ugen": 1},
                                                 {"ugen": 2}, {"const": 1.0}]},
                {"kind": "Select", "inputs": [{"ugen": 2}, {"ugen": 0},
                                              {"ugen": 1}, {"ugen": 11}]},
                {"kind": "SelectX", "inputs": [{"ugen": 2}, {"ugen": 0},
                                               {"ugen": 1}, {"ugen": 11}]},

                {"kind": "Sum4", "inputs": [{"ugen": 5}, {"ugen": 6},
                                            {"ugen": 7}, {"ugen": 8}]},
                {"kind": "Sum4", "inputs": [{"ugen": 9}, {"ugen": 10},
                                            {"ugen": 12}, {"ugen": 13}]},
                {"kind": "Sum3", "inputs": [{"ugen": 14}, {"ugen": 15},
                                            {"ugen": 16}]},
                {"kind": "BinaryOpUGen", "op": "mul",
                 "inputs": [{"ugen": 17}, {"const": 0.05}]},
                {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 18}]}
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
            synth: Box::new(UGenSynth::new(def, 48_000.0, SEED_STRIDE)),
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
