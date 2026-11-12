//! Generic server compensation, independent of the def and plugin families.
use clausters::dsp::compensation::{AnalyzedNode, Plan, analyze};
use clausters::dsp::{BusUsage, ProcessCtx, StageMask};
use clausters::node::{AddAction, Group, SynthNode};
use clausters::server::engine::{BLOCK_SIZE, Cmd, engine_pair_with_workers};

#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

struct Processor {
    inputs: Vec<usize>,
    output: usize,
    history: Vec<f32>,
    cursor: usize,
    time: usize,
    replace: bool,
}
impl Processor {
    fn new(inputs: &[usize], output: usize, latency: usize, replace: bool) -> Self {
        Self {
            inputs: inputs.to_vec(),
            output,
            history: vec![0.0; latency],
            cursor: 0,
            time: 0,
            replace,
        }
    }
    fn usage(&self) -> BusUsage {
        let mut result = BusUsage::default();
        for &bus in &self.inputs {
            result.mark_bus(bus, true, false);
        }
        result.mark_bus(self.output, false, true);
        result
    }
}
impl SynthNode for Processor {
    fn process(&mut self, ctx: &mut ProcessCtx) {
        for frame in ctx.offset..ctx.offset + ctx.frames {
            let mut sample = if self.inputs.is_empty() {
                f32::from(self.time.is_multiple_of(256))
            } else {
                self.inputs
                    .iter()
                    .map(|&bus| ctx.buses.audio(bus)[frame])
                    .sum()
            };
            if !self.history.is_empty() {
                std::mem::swap(&mut sample, &mut self.history[self.cursor]);
                self.cursor = (self.cursor + 1) % self.history.len();
            }
            let output = if self.replace {
                unsafe { ctx.buses.audio_replace(self.output) }
            } else {
                unsafe { ctx.buses.audio_mut(self.output) }
            };
            if self.replace {
                output[frame] = sample;
            } else {
                output[frame] += sample;
            }
            self.time += 1;
        }
    }
    fn incarnation(&self) -> u64 {
        42
    }
    fn latency(&self) -> usize {
        self.history.len()
    }
    fn set_control(&mut self, _: u32, _: f32) {}
    fn map_control(&mut self, _: u32, _: i32, _: bool) {}
    fn ugen_count(&self) -> usize {
        1
    }
}

fn run(processors: Vec<Processor>, workers: usize, split: bool, nested: bool) -> Vec<f32> {
    let nodes: Vec<_> = processors
        .iter()
        .enumerate()
        .map(|(i, p)| AnalyzedNode {
            id: i as i32 + 10,
            usage: p.usage(),
            latency: p.latency(),
            automatic: true,
            domain: 1,
            runtime_usage: StageMask::of(&p.usage()),
            identity: i as u64 + 1,
        })
        .collect();
    let report = analyze(&nodes);
    assert!(report.excluded.is_empty(), "{:?}", report.excluded);
    let (mut engine, mut handle) = engine_pair_with_workers(48000.0, 1, workers);
    handle
        .send(Cmd::AddGroup {
            id: 1,
            target: 0,
            action: AddAction::Tail,
            group: Group {
                parallel: true,
                ..Group::with_capacity(16)
            },
        })
        .ok()
        .unwrap();
    if nested {
        for id in [2, 3] {
            handle
                .send(Cmd::AddGroup {
                    id,
                    target: 1,
                    action: AddAction::Tail,
                    group: Group {
                        parallel: true,
                        ..Group::with_capacity(16)
                    },
                })
                .ok()
                .unwrap();
        }
    }
    for (i, processor) in processors.into_iter().enumerate() {
        handle
            .send(Cmd::AddSynth {
                id: i as i32 + 10,
                target: if nested { if i < 2 { 2 } else { 3 } } else { 1 },
                action: AddAction::Tail,
                usage: StageMask::of(&processor.usage()),
                synth: Box::new(processor),
            })
            .ok()
            .unwrap();
    }
    handle
        .send(Cmd::SetCompensation(Box::new(Plan::new(&report))))
        .ok()
        .unwrap();
    if split {
        for sample in [13, 39, 71, 117, 263] {
            handle
                .send(Cmd::Schedule {
                    time: sample,
                    cmds: vec![Cmd::SetControlBus {
                        index: 0,
                        value: 0.0,
                    }],
                })
                .ok()
                .unwrap();
        }
    }
    let mut samples = Vec::new();
    let mut output = [0.0; BLOCK_SIZE];
    for _ in 0..12 {
        assert_no_alloc::assert_no_alloc(|| engine.process_block(&mut output));
        samples.extend_from_slice(&output);
        handle.collect_garbage();
    }
    samples
}
fn impulses(samples: &[f32], delay: usize, amplitude: f32) {
    for (i, &sample) in samples.iter().enumerate() {
        let expected = if i >= delay && (i - delay).is_multiple_of(256) {
            amplitude
        } else {
            0.0
        };
        assert!(
            (sample - expected).abs() < 1e-6,
            "sample {i}: {sample} != {expected}"
        );
    }
}
#[test]
fn writer_contributions_align_before_summing() {
    for workers in [0, 2] {
        for split in [false, true] {
            let samples = run(
                vec![
                    Processor::new(&[], 0, 0, false),
                    Processor::new(&[], 0, 91, false),
                    Processor::new(&[], 0, 7, false),
                ],
                workers,
                split,
                false,
            );
            impulses(&samples, 91, 3.0);
        }
    }
}
#[test]
fn chains_main_inputs_and_sidechains_align_across_nested_groups() {
    for workers in [0, 2] {
        for split in [false, true] {
            let samples = run(
                vec![
                    Processor::new(&[], 4, 17, false),
                    Processor::new(&[], 5, 91, false),
                    Processor::new(&[4, 5], 6, 23, false),
                    Processor::new(&[6], 0, 11, false),
                    Processor::new(&[], 0, 0, false),
                ],
                workers,
                split,
                true,
            );
            impulses(&samples, 125, 3.0);
        }
    }
}
#[test]
fn replace_out_overwrites_the_aligned_contribution() {
    let samples = run(
        vec![
            Processor::new(&[], 0, 0, false),
            Processor::new(&[], 0, 91, true),
            Processor::new(&[], 0, 7, false),
        ],
        2,
        true,
        false,
    );
    impulses(&samples, 91, 2.0);
}
#[test]
fn boundaries_and_capacity_are_reported_without_guessing() {
    let make = |id, automatic, domain, input, output| {
        let mut usage = BusUsage::default();
        if let Some(bus) = input {
            usage.mark_bus(bus, true, false);
        }
        usage.mark_bus(output, false, true);
        AnalyzedNode {
            id,
            usage: usage.clone(),
            latency: 7,
            automatic,
            domain,
            runtime_usage: StageMask::of(&usage),
            identity: id as u64,
        }
    };
    let manual = analyze(&[make(1, false, 0, None, 4), make(2, true, 2, Some(4), 0)]);
    assert!(manual.nodes.is_empty());
    assert_eq!(manual.excluded.len(), 2);
    let crossing = analyze(&[make(1, true, 1, None, 4), make(2, true, 2, Some(4), 0)]);
    assert!(crossing.nodes.is_empty());
    let feedback = analyze(&[make(1, true, 1, Some(5), 4), make(2, true, 1, Some(4), 5)]);
    assert!(feedback.nodes.is_empty());
    let mut dynamic = make(3, true, 1, None, 7);
    dynamic.usage.dynamic = true;
    let dynamic = analyze(&[make(1, true, 1, None, 0), dynamic]);
    assert!(dynamic.nodes.is_empty());
    let mut long = make(1, true, 1, None, 0);
    long.latency = usize::MAX;
    let capacity = analyze(&[long, make(2, true, 1, None, 0)]);
    assert!(capacity.nodes.is_empty());
    assert!(
        capacity
            .excluded
            .iter()
            .all(|(_, r)| *r == "delay storage capacity")
    );
}

#[cfg(feature = "synth")]
#[path = "../examples/support/compensation_scene.rs"]
mod scene;

#[cfg(feature = "synth")]
#[test]
fn synthdef_convolution_aligns_in_the_same_live_and_offline_graph() {
    use clausters::dsp::buffer::Buffer;
    use clausters::dsp::conv::layout;
    use clausters::dsp::wavetable::GenCommand;
    use clausters::osc::translate::CmdTranslator;
    use clausters::server::render::{RenderConfig, render_to_vec};
    use std::sync::Arc;
    let kernel = GenCommand::PreparePartConv {
        src: Arc::new(Buffer::new(vec![1.0], 1, 1, scene::RATE)),
        fft_size: 512,
    }
    .apply(&Buffer::zeroed(layout::frames(512, 1), 1, scene::RATE));
    let kernel = Arc::new(kernel);
    let config = RenderConfig {
        sample_rate: scene::RATE,
        channels: 1,
        ..RenderConfig::default()
    };
    for automatic in [false, true] {
        let (offline, _) = render_to_vec(&scene::score(automatic), &config).unwrap();
        for workers in [0, 2] {
            let (mut engine, mut handle) = engine_pair_with_workers(scene::RATE as f32, 1, workers);
            handle
                .send(Cmd::SetBuffer {
                    index: 1,
                    buffer: Some(kernel.clone()),
                })
                .ok()
                .unwrap();
            let mut translator = CmdTranslator::new(scene::RATE as f32);
            let mut cmds = Vec::new();
            for message in scene::graph(automatic) {
                if message.addr == "/def_send" {
                    translator.d_recv(&message.args[1..]).unwrap();
                } else {
                    translator.translate(&message, &mut cmds).unwrap();
                }
            }
            let report = translator.dump_graph(1).unwrap();
            assert!(report.contains("compensation (global, samples)"));
            if automatic {
                assert!(report.contains("(0, 256)"), "{report}");
            }
            handle.send_batch(cmds).ok().unwrap();
            let mut live = Vec::new();
            let mut block = [0.0; BLOCK_SIZE];
            for _ in 0..scene::FRAMES / BLOCK_SIZE {
                assert_no_alloc::assert_no_alloc(|| engine.process_block(&mut block));
                live.extend_from_slice(&block);
                handle.collect_garbage();
            }
            assert_eq!(live, offline);
            for (index, &sample) in live.iter().enumerate() {
                let expected = if index == 256 {
                    if automatic { 2.0 } else { 1.0 }
                } else if index == 0 && !automatic {
                    1.0
                } else {
                    0.0
                };
                assert!(
                    (sample - expected).abs() < 1e-5,
                    "sample {index}: {sample} != {expected}"
                );
            }
        }
    }
}

#[test]
fn changed_latency_and_unrelated_topology_publish_without_rt_allocation() {
    let (mut engine, mut handle) = engine_pair_with_workers(48000.0, 1, 2);
    let mut processors = vec![
        Processor::new(&[], 0, 0, false),
        Processor::new(&[], 0, 91, false),
    ];
    let describe = |processors: &[Processor], identities: &[u64]| {
        analyze(
            &processors
                .iter()
                .enumerate()
                .map(|(i, p)| AnalyzedNode {
                    id: i as i32 + 10,
                    usage: p.usage(),
                    latency: p.latency(),
                    automatic: true,
                    domain: 0,
                    runtime_usage: StageMask::of(&p.usage()),
                    identity: identities[i],
                })
                .collect::<Vec<_>>(),
        )
    };
    let mut report = describe(&processors, &[1, 2]);
    for (i, p) in processors.drain(..).enumerate() {
        handle
            .send(Cmd::AddSynth {
                id: i as i32 + 10,
                target: 0,
                action: AddAction::Tail,
                usage: StageMask::of(&p.usage()),
                synth: Box::new(p),
            })
            .ok()
            .unwrap();
    }
    handle
        .send(Cmd::SetCompensation(Box::new(Plan::new(&report))))
        .ok()
        .unwrap();
    let mut block = [0.0; BLOCK_SIZE];
    assert_no_alloc::assert_no_alloc(|| engine.process_block(&mut block));
    assert!(block.iter().all(|&x| x == 0.0));
    let extra = Processor::new(&[], 8, 0, false);
    report.graph.push((12, 0, StageMask::of(&extra.usage())));
    // No delay spec changes: histories from the first impulse must survive.
    handle
        .send_batch(vec![
            Cmd::AddSynth {
                id: 12,
                target: 0,
                action: AddAction::Tail,
                usage: StageMask::of(&extra.usage()),
                synth: Box::new(extra),
            },
            Cmd::SetCompensation(Box::new(Plan::new(&report))),
        ])
        .ok()
        .unwrap();
    assert_no_alloc::assert_no_alloc(|| engine.process_block(&mut block));
    assert!((block[27] - 2.0).abs() < 1e-6);
    for _ in 0..2 {
        assert_no_alloc::assert_no_alloc(|| engine.process_block(&mut block));
    }
    // A new processor declaration, prepared with the corresponding plan.
    let replacement = Processor::new(&[], 0, 17, false);
    report = describe(
        &[
            Processor::new(&[], 0, 0, false),
            Processor::new(&[], 0, 17, false),
        ],
        &[1, 3],
    );
    report.graph.push((
        12,
        0,
        StageMask::of(&Processor::new(&[], 8, 0, false).usage()),
    ));
    handle
        .send_batch(vec![
            Cmd::RestartSynth {
                id: 11,
                expected: 42,
                synth: Box::new(replacement),
            },
            Cmd::SetCompensation(Box::new(Plan::new(&report))),
        ])
        .ok()
        .unwrap();
    assert_no_alloc::assert_no_alloc(|| engine.process_block(&mut block));
    for (i, &x) in block.iter().enumerate() {
        assert!(
            (x - if i == 17 { 2.0 } else { 0.0 }).abs() < 1e-6,
            "{i}: {x}"
        );
    }
    handle.collect_garbage();
}

#[test]
fn a_future_plan_cannot_compensate_a_different_live_tree() {
    let (mut engine, mut handle) = engine_pair_with_workers(48000.0, 1, 0);
    let dry = Processor::new(&[], 0, 0, false);
    let wet = Processor::new(&[], 0, 91, false);
    let report = analyze(&[
        AnalyzedNode {
            id: 10,
            usage: dry.usage(),
            latency: 0,
            automatic: true,
            domain: 0,
            runtime_usage: StageMask::of(&dry.usage()),
            identity: 1,
        },
        AnalyzedNode {
            id: 11,
            usage: wet.usage(),
            latency: 91,
            automatic: true,
            domain: 0,
            runtime_usage: StageMask::of(&wet.usage()),
            identity: 2,
        },
    ]);
    handle
        .send_batch(vec![
            Cmd::AddSynth {
                id: 10,
                target: 0,
                action: AddAction::Tail,
                usage: StageMask::of(&dry.usage()),
                synth: Box::new(dry),
            },
            Cmd::SetCompensation(Box::new(Plan::new(&report))),
        ])
        .ok()
        .unwrap();
    let mut block = [0.0; BLOCK_SIZE];
    assert_no_alloc::assert_no_alloc(|| engine.process_block(&mut block));
    assert_eq!(
        block[0], 1.0,
        "missing future writer must not delay an existing source"
    );
}

#[cfg(feature = "synth")]
#[test]
fn routing_snapshots_reject_folded_mask_aliases_and_reused_node_ids() {
    use clausters::osc::translate::CmdTranslator;
    use clausters::rosc::{OscMessage, OscType};
    use serde_json::json;
    let mut translator = CmdTranslator::new(48000.0);
    let def = json!({"name":"routed", "controls":[{"name":"out","default":4.0}], "ugens":[{"kind":"Impulse","inputs":[{"const":0.0}]},{"kind":"Out","inputs":[{"control":0},{"ugen":0}]}]});
    translator
        .d_recv(&[OscType::Blob(def.to_string().into_bytes())])
        .unwrap();
    let message = |addr: &str, args| OscMessage {
        addr: addr.into(),
        args,
    };
    let mut cmds = Vec::new();
    translator
        .translate(
            &message("/group_sortMode", vec![OscType::Int(0), OscType::Int(1)]),
            &mut cmds,
        )
        .unwrap();
    translator
        .translate(
            &message(
                "/synth_new",
                vec![
                    OscType::String("routed".into()),
                    OscType::Int(10),
                    OscType::Int(1),
                    OscType::Int(0),
                ],
            ),
            &mut cmds,
        )
        .unwrap();
    let index = cmds
        .iter()
        .position(|c| matches!(c, Cmd::AddSynth { .. }))
        .unwrap();
    let Cmd::AddSynth {
        mut synth, usage, ..
    } = cmds.remove(index)
    else {
        unreachable!()
    };
    let old_plan = cmds
        .into_iter()
        .rev()
        .find_map(|c| {
            if let Cmd::SetCompensation(plan) = c {
                Some(plan)
            } else {
                None
            }
        })
        .unwrap();
    let mut next = Vec::new();
    translator
        .translate(
            &message(
                "/node_set",
                vec![
                    OscType::Int(10),
                    OscType::String("out".into()),
                    OscType::Float(132.0),
                ],
            ),
            &mut next,
        )
        .unwrap();
    let next_plan = next
        .into_iter()
        .find_map(|c| {
            if let Cmd::SetCompensation(plan) = c {
                Some(plan)
            } else {
                None
            }
        })
        .unwrap();
    // 4 and 132 occupy the same worker-mask lane. The exact routing snapshot
    // must still prevent an ahead-of-time plan from being used by this node.
    assert_eq!(
        StageMask::of(&{
            let mut u = BusUsage::default();
            u.mark_bus(132, false, true);
            u
        }),
        usage
    );
    assert!(old_plan.matches_node(0, 10, synth.as_ref(), usage));
    assert!(!next_plan.matches_node(0, 10, synth.as_ref(), usage));
    synth.set_control(0, 132.0);
    assert!(!old_plan.matches_node(0, 10, synth.as_ref(), usage));
    assert!(next_plan.matches_node(0, 10, synth.as_ref(), usage));
    let (other, _) = translator.make_synth("routed").unwrap();
    assert!(
        !old_plan.matches_node(0, 10, other.as_ref(), usage),
        "another object cannot inherit the old plan"
    );
}

#[cfg(feature = "synth")]
#[test]
fn last_bus_clamping_is_shared_by_ordering_parallel_masks_and_compensation() {
    use clausters::osc::translate::CmdTranslator;
    use clausters::rosc::{OscMessage, OscType};
    use clausters::server::engine::engine_pair_full;
    use serde_json::json;
    let mut translator = CmdTranslator::with_buses(48000.0, 32, 32);
    let mut cmds = Vec::new();
    let msg = |addr: &str, args| OscMessage {
        addr: addr.into(),
        args,
    };
    translator
        .translate(
            &msg("/group_sortMode", vec![OscType::Int(0), OscType::Int(1)]),
            &mut cmds,
        )
        .unwrap();
    let source = json!({"name":"source","ugens":[{"kind":"Impulse","inputs":[{"const":0.0}]},{"kind":"Out","inputs":[{"const":37.0},{"ugen":0}]}]});
    let reader = json!({"name":"reader","ugens":[{"kind":"In","inputs":[{"const":31.0}]},{"kind":"Out","inputs":[{"const":0.0},{"ugen":0}]}]});
    for def in [source, reader] {
        translator
            .d_recv(&[OscType::Blob(def.to_string().into_bytes())])
            .unwrap();
    }
    // Reader inserted first; the clamped physical dependency must reorder it.
    for (name, id) in [("reader", 10), ("source", 11)] {
        translator
            .translate(
                &msg(
                    "/synth_new",
                    vec![
                        OscType::String(name.into()),
                        OscType::Int(id),
                        OscType::Int(1),
                        OscType::Int(0),
                    ],
                ),
                &mut cmds,
            )
            .unwrap();
    }
    let dump = translator.dump_graph(0).unwrap();
    assert!(dump.contains("reads -  writes 31"), "{dump}");
    let (mut engine, mut handle) =
        engine_pair_full(48000.0, 1, 2, None, 32, 32, Default::default());
    handle.send_batch(cmds).ok().unwrap();
    let mut output = [0.0; BLOCK_SIZE];
    assert_no_alloc::assert_no_alloc(|| engine.process_block(&mut output));
    assert_eq!(output[0], 1.0);
    assert!(output[1..].iter().all(|&x| x == 0.0));
}
