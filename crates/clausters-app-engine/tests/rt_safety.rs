//! **A take the application reads costs the audio thread nothing it may not
//! spend**: the reader of a fed stream (`TransportDiskIn`) takes its owner's
//! rings once, asks for refills, fades into new contents, fills its ring
//! standing for a cue and reads a loop straight through, and none of it
//! allocates on the audio thread. The owner is the application's own reader
//! (`take::TakeFeed`), reading a part of a file.

#![cfg(feature = "synth")]

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use clausters::clausters_core::rng::SEED_STRIDE;
use clausters::dsp::buffer::Buffer;
use clausters::node::{AddAction, ROOT_NODE_ID};
use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, engine_pair};
use clausters::synthdef::instance::UGenSynth;
use clausters::synthdef::{SynthDefSpec, compile};
use clausters_app_engine::take::{Headers, ReadList, TakeFeed};
use clausters_document::clipboard::SegmentPart;

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

const SR: f64 = 48_000.0;

#[test]
fn a_take_its_application_feeds_does_not_allocate_on_the_audio_thread() {
    let (mut engine, mut handle) = engine_pair(SR as f32, 2);
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let path = std::env::temp_dir().join(format!("clausters_rt_fed_{}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for n in 0..48_000 {
        w.write_sample(((n % 100) as i16 + 1) * 100).unwrap();
    }
    w.finalize().unwrap();
    let path = path.to_str().unwrap().to_string();
    let list = |start: u64| {
        let part = SegmentPart {
            path: path.clone(),
            start,
            frames: 20_000,
            fade_in: 32,
            fade_out: 32,
            fade_in_shape: Default::default(),
            fade_out_shape: Default::default(),
            channels: None,
            source_channels: 1,
            source_frames: 48_000,
            source_rate: SR,
        };
        Arc::new(ReadList::of(&[part], 1, SR, &mut Headers::default()).unwrap())
    };
    let feed = TakeFeed::start(1);
    feed.set(list(1_000));
    handle
        .send(Cmd::SetBuffer {
            index: 0,
            buffer: Some(Arc::new(Buffer::fed_by(
                Arc::clone(feed.stream(0).unwrap()),
                1,
                20_000,
                SR,
            ))),
        })
        .ok()
        .unwrap();
    let spec: SynthDefSpec = serde_json::from_str(
        r#"{"name": "fed", "ugens": [
            {"kind": "TransportDiskIn", "inputs": [{"const": 0.0}, {"const": 0.0},
                {"const": 0.0}, {"const": 0.0}, {"const": 0.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]}"#,
    )
    .unwrap();
    let synth = Box::new(UGenSynth::metered(
        Arc::new(compile(spec).unwrap()),
        SR as f32,
        SEED_STRIDE,
        Arc::clone(handle.meters()),
    ));
    handle
        .send(Cmd::AddSynth {
            id: 1000,
            target: ROOT_NODE_ID,
            action: AddAction::Tail,
            synth,
            usage: Default::default(),
        })
        .ok()
        .unwrap();
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    let mut run = |engine: &mut Engine| {
        let mut heard = false;
        for _ in 0..100 {
            assert_no_alloc(|| engine.process_block(&mut out));
            heard |= out.iter().any(|s| *s != 0.0);
            std::thread::sleep(std::time::Duration::from_micros(1_333));
        }
        heard
    };
    assert!(run(&mut engine), "the fed take played");
    handle
        .send(Cmd::TransportLocate {
            transport: 0,
            position: 12_000,
        })
        .ok()
        .unwrap();
    assert!(run(&mut engine), "and played after a locate");
    feed.set(list(5_000));
    assert!(
        run(&mut engine),
        "and played the application's new contents"
    );
    // A cue walks the governed group and fills the ring standing, and a loop
    // is read straight through: neither allocates.
    for cmd in [
        Cmd::TransportGroup {
            transport: 0,
            id: ROOT_NODE_ID,
        },
        Cmd::TransportRun {
            transport: 0,
            rolling: false,
        },
        Cmd::TransportLoop {
            transport: 0,
            span: Some(2_000..18_000),
        },
        Cmd::TransportLocate {
            transport: 0,
            position: 2_000,
        },
        Cmd::TransportCue { transport: 0 },
    ] {
        handle.send(cmd).ok().unwrap();
    }
    assert!(!run(&mut engine), "a cue is not heard");
    handle
        .send(Cmd::TransportRun {
            transport: 0,
            rolling: true,
        })
        .ok()
        .unwrap();
    assert!(run(&mut engine), "and played a cued loop");
    handle.send(Cmd::FreeNode { id: 1000 }).ok().unwrap();
    assert_no_alloc(|| {
        for _ in 0..10 {
            engine.process_block(&mut out);
        }
    });
    handle.collect_garbage();
    drop(feed);
    std::fs::remove_file(&path).ok();
}
