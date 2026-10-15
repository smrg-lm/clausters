//! Isolated COM hosting acceptance with an independently loaded repository binary.
#![cfg(all(target_os = "linux", feature = "vst3"))]
use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use clausters_core::plugin_shm::{
    Config, DEFAULT_DEPTH, Event, EventKind, EventList, FRAMES, Job, Span,
};
use clausters_plugin_host::{
    format::ScanEntry,
    runtime::{self, Instance},
};
use std::{path::Path, time::Duration};
#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;
const ID: &str = "6E33225254224A00AA69301AF318797D";
#[path = "../../../tests/common/vst3_fixture.rs"]
mod vst3_fixture;
use vst3_fixture::fixture;
fn helper() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_clausters-plugin-host"))
}
fn config(generation: u64) -> Config {
    Config {
        generation,
        sample_rate: 48000.0,
        sample_origin: 8192,
        inputs: 2,
        outputs: 2,
        depth: DEFAULT_DEPTH,
        reserved: 0,
    }
}
#[test]
fn bundles_class_ids_state_and_independent_instances() {
    let rows = runtime::scan(helper(), &[fixture().to_owned()], Duration::from_secs(3)).unwrap();
    assert_eq!(rows.len(), 1);
    let ScanEntry::Plugin { plugin } = &rows[0] else {
        panic!("{rows:?}")
    };
    assert_eq!(plugin.format, "vst3");
    assert_eq!(plugin.id, ID);
    assert_eq!(plugin.parameters[0].default, 0.5);
    assert_eq!(plugin.note_inputs.len(), 1);
    assert!(plugin.editor);
    assert!(runtime::probe(helper(), fixture(), "bad-id", Duration::from_secs(2)).is_err());
    let mut a =
        Instance::load(helper(), fixture(), ID, config(101), Duration::from_secs(2)).unwrap();
    let mut b =
        Instance::load(helper(), fixture(), ID, config(102), Duration::from_secs(2)).unwrap();
    let image = a.save_state().unwrap();
    assert_eq!(&image[..4], b"VST3");
    assert_eq!(image.len(), 24);
    let mut changed = image.clone();
    changed[8..16].copy_from_slice(&0.75f64.to_ne_bytes());
    changed[16..24].copy_from_slice(&0.75f64.to_ne_bytes());
    a.load_state(&changed).unwrap();
    assert_eq!(a.parameter(0).unwrap(), 0.75);
    assert_eq!(a.metadata().latency, 23);
    assert_eq!(b.parameter(0).unwrap(), 0.5);
    assert!(a.load_state(&[0]).is_err());
    a.load_state(&image).unwrap();
    assert_eq!(a.parameter(0).unwrap(), 0.5);
    let restored = Instance::load_with_state(
        helper(),
        fixture(),
        ID,
        config(103),
        Duration::from_secs(2),
        Some(&changed),
    )
    .unwrap();
    assert_eq!(restored.metadata().latency, 23);
    let path = a.segment_path().to_owned();
    let pid = a.pid();
    drop(a);
    assert!(!path.exists());
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
}
#[test]
fn sample_accurate_parameters_notes_split_transport_and_no_allocation() {
    let mut instance =
        Instance::load(helper(), fixture(), ID, config(201), Duration::from_secs(2)).unwrap();
    let mut producer = instance.producer().unwrap();
    let mut output = [0.0; FRAMES * 2];
    let mut reply = EventList::default();
    for sequence in 0..16 {
        let mut job = Job::full(sequence * FRAMES as i64);
        job.span_count = 2;
        job.spans[0].frames = 13;
        job.spans[1] = Span {
            offset: 13,
            frames: 51,
            active: 1,
            playing: 0,
            position: 4096,
            tempo: 97.0,
        };
        for (offset, kind, value) in [
            (0, EventKind::Parameter, 0.5),
            (3, EventKind::NoteOn, 1.0),
            (5, EventKind::Expression, 1.0),
            (9, EventKind::Expression, 12.0),
            (17, EventKind::Parameter, 0.25),
            (25, EventKind::NoteOff, 0.0),
        ] {
            assert!(job.events.push(Event {
                offset,
                kind: kind as u32,
                id: if offset == 9 { 2 } else { 0 },
                note_id: 123,
                port: 0,
                channel: 2,
                key: 60,
                value,
                ..Event::default()
            }));
        }
        assert_no_alloc(|| producer.process(&job, &[1.0; FRAMES * 2], &mut output, &mut reply));
        if sequence >= DEFAULT_DEPTH as i64 {
            for channel in 0..2 {
                let output = &output[channel * FRAMES..(channel + 1) * FRAMES];
                assert_eq!(output[2], 0.5);
                assert_eq!(output[3], 1.0);
                assert_eq!(output[16], 1.0);
                assert_eq!(output[17], 0.5);
                assert_eq!(output[25], 0.25);
            }
            assert_eq!(reply.count, 4);
            assert_eq!(reply.values[0].offset, 3);
            assert_eq!(reply.values[1].offset, 5);
            assert_eq!(reply.values[1].value, 1.0);
            assert!((reply.values[2].value - 12.0).abs() < 1e-12);
            assert_eq!(reply.values[3].offset, 25);
            assert_eq!(reply.values[0].note_id, 123);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(producer.view().failure(), 0);
}

// This one invokes the adapter locally to put the helper-side conversion and
// COM lists under the allocation guard, in addition to the isolated acceptance.
use clausters_plugin_host::format;
#[path = "../src/vst3.rs"]
mod adapter;
#[test]
fn helper_processor_and_native_fixture_allocate_nothing_on_audio() {
    assert_eq!(adapter::scan(fixture(), 0).unwrap().len(), 1);
    let mut instance = adapter::load(fixture(), ID).unwrap();
    let mut processor = instance.activate(config(301)).unwrap();
    let stopped = std::thread::spawn(move || {
        let mut input = [0.0; FRAMES * 2];
        input[0] = 1.0;
        let mut output = [0.0; FRAMES * 2];
        let mut result = EventList::default();
        assert_no_alloc(|| {
            processor.start().unwrap();
            for _ in 0..20 {
                result.count = 0;
                processor
                    .process(
                        Span {
                            frames: 64,
                            active: 1,
                            playing: 1,
                            tempo: 120.0,
                            ..Span::default()
                        },
                        8192,
                        &input,
                        &mut output,
                        &EventList::default(),
                        &mut result,
                    )
                    .unwrap();
                assert_eq!(output[0], 0.5);
            }
            processor.pause();
        });
        processor.stop()
    })
    .join()
    .unwrap();
    instance.deactivate(stopped).unwrap();
}
#[test]
fn editor_callbacks_reach_audio_and_latency_restart_requests_fail_safely() {
    let mut instance =
        Instance::load(helper(), fixture(), ID, config(401), Duration::from_secs(2)).unwrap();
    let mut producer = instance.producer().unwrap();
    let mut output = [0.0; FRAMES * 2];
    let mut reply = EventList::default();
    let mut callbacks = Vec::new();
    for sequence in 0..40 {
        let mut job = Job::full(sequence * FRAMES as i64);
        if sequence == 0 {
            assert!(job.events.push(Event {
                kind: EventKind::Parameter as u32,
                id: 0,
                value: 0.66,
                note_id: -1,
                port: -1,
                channel: -1,
                key: -1,
                ..Event::default()
            }));
        }
        producer.process(&job, &[1.0; FRAMES * 2], &mut output, &mut reply);
        callbacks.extend_from_slice(&reply.values[..reply.count as usize]);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        callbacks.iter().map(|e| e.kind).collect::<Vec<_>>(),
        vec![
            EventKind::GestureBegin as u32,
            EventKind::Parameter as u32,
            EventKind::GestureEnd as u32
        ]
    );
    assert_eq!(output[0], 0.375);
    assert_eq!(producer.view().failure(), 0);
    for sequence in 0..20 {
        let mut job = Job::full(sequence * FRAMES as i64);
        if sequence == 0 {
            assert!(job.events.push(Event {
                kind: EventKind::Parameter as u32,
                id: 0,
                value: 0.99,
                note_id: -1,
                port: -1,
                channel: -1,
                key: -1,
                ..Event::default()
            }));
        }
        producer.process(&job, &[1.0; FRAMES * 2], &mut output, &mut reply);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_ne!(producer.view().failure(), 0);
    assert_eq!(output, [0.0; FRAMES * 2]);
}
#[test]
fn midi_notes_and_mapped_controllers_keep_offsets_and_queries() {
    let mut instance =
        Instance::load(helper(), fixture(), ID, config(501), Duration::from_secs(2)).unwrap();
    let mut producer = instance.producer().unwrap();
    let mut output = [0.0; FRAMES * 2];
    let mut reply = EventList::default();
    for sequence in 0..16 {
        let mut job = Job::full(sequence * FRAMES as i64);
        for (offset, bytes) in [
            (0, [0xb2u8, 7, 64]),
            (7, [0xb2, 7, 127]),
            (13, [0x92, 60, 127]),
            (27, [0x82, 60, 0]),
        ] {
            assert!(job.events.push(Event {
                offset,
                kind: EventKind::Midi as u32,
                id: u32::from(bytes[0]) | u32::from(bytes[1]) << 8 | u32::from(bytes[2]) << 16,
                port: 0,
                note_id: -1,
                channel: -1,
                key: -1,
                ..Event::default()
            }));
        }
        assert_no_alloc(|| producer.process(&job, &[1.0; FRAMES * 2], &mut output, &mut reply));
        if sequence >= DEFAULT_DEPTH as i64 {
            assert!((output[6] - 64.0 / 127.0).abs() < 1e-6);
            assert_eq!(output[7], 1.0);
            assert_eq!(output[13], 2.0);
            assert_eq!(output[27], 1.0);
            assert_eq!(reply.count, 2);
            assert_eq!(reply.values[0].channel, 2);
            assert_eq!(reply.values[0].offset, 13);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(producer.view().failure(), 0);
    assert_eq!(instance.parameter(0).unwrap(), 1.0);
}

#[path = "../src/editor.rs"]
mod editor;

/// What CI checks of the VST3 editor, with no window.
#[test]
fn native_editor_is_reported_and_starts_closed() {
    use clausters_plugin_host::format::EditorCommand;
    let mut instance =
        Instance::load(helper(), fixture(), ID, config(708), Duration::from_secs(2)).unwrap();
    assert!(instance.metadata().editor);
    assert!(!instance.editor(EditorCommand::Query).unwrap().open);
}

#[test]
#[ignore = "opens a native window; run by hand with --ignored"]
fn native_editor_runs_linux_timer_and_fd_callbacks_while_processing() {
    use clausters_plugin_host::format::EditorCommand;
    let mut instance =
        Instance::load(helper(), fixture(), ID, config(707), Duration::from_secs(2)).unwrap();
    assert!(!instance.editor(EditorCommand::Query).unwrap().open);
    assert!(
        std::env::var_os("DISPLAY").is_some(),
        "this test opens a window: run it on a display"
    );
    let mut admin = instance.administration().unwrap();
    let pid = instance.pid();
    let segment = instance.segment_path().to_owned();
    let mut producer = instance.producer().unwrap();
    let mut output = [0.0; FRAMES * 2];
    let mut result = EventList::default();
    let mut edits = Vec::new();
    for sequence in 0..40 {
        if sequence == 0 || sequence == 20 {
            assert!(admin.editor(EditorCommand::Open).unwrap().open);
        }
        if sequence == 10 {
            let state = admin
                .editor(EditorCommand::Resize {
                    width: 480,
                    height: 280,
                })
                .unwrap();
            assert_eq!((state.width, state.height), (480, 280));
        }
        if sequence == 19 {
            assert!(!admin.editor(EditorCommand::Close).unwrap().open);
        }
        let job = Job::full(sequence * FRAMES as i64);
        assert_no_alloc(|| {
            producer.receive(&mut output, &mut result);
            producer.publish(&job, &[1.0; FRAMES * 2]);
        });
        edits.extend(
            result.values[..result.count as usize]
                .iter()
                .map(|e| (e.kind, e.value)),
        );
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while !producer.last_completed() {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_micros(50));
        }
        assert_eq!(producer.view().failure(), 0);
        std::thread::sleep(Duration::from_millis(3));
    }
    assert!(output.iter().all(|v| *v == 0.75));
    assert!(edits.contains(&(EventKind::Parameter as u32, 0.75)));
    assert!(edits.contains(&(EventKind::GestureBegin as u32, 0.0)));
    assert!(edits.contains(&(EventKind::GestureEnd as u32, 0.0)));
    assert!(admin.editor(EditorCommand::Query).unwrap().open);
    // A helper crash with its window open closes only this instance and leaves
    // its scheduled audio silent once the already completed pipeline drains.
    // SAFETY: pid belongs to the child loaded and still owned by this test.
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0);
    assert!(admin.editor(EditorCommand::Query).is_err());
    for sequence in 40..60 {
        let job = Job::full(sequence * FRAMES as i64);
        assert_no_alloc(|| {
            producer.receive(&mut output, &mut result);
            producer.publish(&job, &[1.0; FRAMES * 2]);
        });
        std::thread::sleep(Duration::from_millis(3));
    }
    assert!(output.iter().all(|v| *v == 0.0));
    assert_ne!(producer.view().failure(), 0);
    drop(instance);
    assert!(!segment.exists());
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert!(admin.editor(EditorCommand::Query).is_err());
}
