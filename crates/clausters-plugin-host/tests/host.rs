//! Real-process acceptance, driven by deterministic repository fixtures, without a device.
#![cfg(all(target_os = "linux", feature = "clap"))]

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use clausters_core::plugin_shm::{
    Config, DEFAULT_DEPTH, EVENTS, Event, EventKind, EventList, FRAMES, Failure, Job, Span,
};
use clausters_plugin_host::format::ScanEntry;
use clausters_plugin_host::runtime::{self, Instance};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;
static FIXTURES: OnceLock<PathBuf> = OnceLock::new();
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(100);
fn fixtures() -> &'static Path {
    FIXTURES
        .get_or_init(|| {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let dir = std::env::temp_dir()
                .join(format!("clausters-plugin-fixtures-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            for (name, source, mode) in [
                (
                    "fixture",
                    root.join("crates/clausters-plugin-host/tests/fixtures/clap.c"),
                    None,
                ),
                (
                    "extended",
                    root.join("crates/clausters-plugin-host/tests/fixtures/extended.c"),
                    None,
                ),
                (
                    "crash",
                    root.join("crates/clausters-plugin-host/tests/fixtures/failure.c"),
                    Some(1),
                ),
                (
                    "hang",
                    root.join("crates/clausters-plugin-host/tests/fixtures/failure.c"),
                    Some(2),
                ),
                (
                    "incompatible",
                    root.join("crates/clausters-plugin-host/tests/fixtures/failure.c"),
                    Some(3),
                ),
            ] {
                let mut cmd = Command::new("cc");
                cmd.args([
                    "-std=c11", "-O2", "-fPIC", "-shared", "-Wall", "-Wextra", "-Werror",
                ])
                .arg("-I")
                .arg(root.join("third_party/clap/include"));
                if let Some(mode) = mode {
                    cmd.arg(format!("-DFAILURE_MODE={mode}"));
                }
                let out = cmd
                    .arg(source)
                    .arg("-o")
                    .arg(dir.join(format!("{name}.clap")))
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            assert!(
                Command::new("cc")
                    .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror"])
                    .arg(root.join("crates/clausters-plugin-host/tests/fixtures/handshake.c"))
                    .arg("-o")
                    .arg(dir.join("bad-helper"))
                    .status()
                    .unwrap()
                    .success()
            );
            dir
        })
        .as_path()
}
fn helper() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_clausters-plugin-host"))
}
fn config(instrument: bool) -> Config {
    Config {
        generation: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        sample_rate: 48000.0,
        sample_origin: 0,
        inputs: u32::from(!instrument),
        outputs: 1,
        depth: DEFAULT_DEPTH,
        reserved: 0,
    }
}
fn load(binary: &str, id: &str, instrument: bool) -> Instance {
    Instance::load(
        helper(),
        &fixtures().join(format!("{binary}.clap")),
        &format!("org.clausters.fixture.{id}"),
        config(instrument),
        Duration::from_secs(2),
    )
    .unwrap()
}
#[test]
fn discovery_isolated_compatible_and_cached_with_failure_diagnostics() {
    let before = Instant::now();
    let entries =
        runtime::scan(helper(), &[fixtures().into()], Duration::from_millis(300)).unwrap();
    assert!(before.elapsed() < Duration::from_secs(3));
    let plugins: Vec<_> = entries
        .iter()
        .filter_map(|e| match e {
            ScanEntry::Plugin { plugin } => Some(plugin),
            _ => None,
        })
        .collect();
    assert_eq!(plugins.len(), 9);
    let gain = plugins.iter().find(|p| p.id.ends_with(".gain")).unwrap();
    assert_eq!(gain.inputs[0].channels, 1);
    assert_eq!(gain.parameters[0].id, 0);
    assert_eq!(gain.parameters[0].default, 0.5);
    assert_eq!(
        plugins
            .iter()
            .find(|p| p.id.ends_with(".delay"))
            .unwrap()
            .latency,
        7
    );
    assert!(
        plugins
            .iter()
            .find(|p| p.id.ends_with(".state"))
            .unwrap()
            .state
    );
    let errors: Vec<_> = entries
        .iter()
        .filter_map(|e| match e {
            ScanEntry::Error { path, error } => Some((path, error)),
            _ => None,
        })
        .collect();
    assert_eq!(errors.len(), 3);
    assert!(
        errors
            .iter()
            .any(|(p, e)| p.ends_with("hang.clap") && e.contains("timeout"))
    );
    assert!(
        errors
            .iter()
            .any(|(p, e)| p.ends_with("incompatible.clap") && e.contains("version"))
    );
    let cache = fixtures().join("cache");
    runtime::cache(&cache, &entries).unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cache.join("plugins.json")).unwrap()).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["entries"].as_array().unwrap().len(), 12);
    assert!(
        Instance::load(
            helper(),
            &fixtures().join("incompatible.clap"),
            "bad",
            config(false),
            Duration::from_secs(1)
        )
        .is_err()
    );
    let mut wrong = config(false);
    wrong.inputs = 2;
    assert!(
        Instance::load(
            helper(),
            &fixtures().join("fixture.clap"),
            "org.clausters.fixture.gain",
            wrong,
            Duration::from_secs(1)
        )
        .is_err()
    );
}
#[test]
fn gain_delay_and_split_samples_keep_declared_pipeline_latency() {
    for kind in ["gain", "delay"] {
        let mut instance = load("fixture", kind, false);
        let path = instance.segment_path().to_owned();
        let pid = instance.pid();
        let mut producer = instance.producer().unwrap();
        let mut output = [0.0; FRAMES];
        let mut events = EventList::default();
        for seq in 0..24 {
            let mut input = [0.0; FRAMES];
            if seq == 0 {
                input[0] = 1.0;
            }
            let mut job = Job::full(seq * FRAMES as i64);
            job.span_count = 2;
            job.spans[0].frames = 13;
            job.spans[1] = Span {
                offset: 13,
                frames: 51,
                active: 1,
                playing: 1,
                position: seq * FRAMES as i64 + 13,
                tempo: 120.0,
            };
            assert_no_alloc(|| {
                producer.process(&job, &input, &mut output, &mut events);
            });
            if seq == DEFAULT_DEPTH as i64 {
                if kind == "gain" {
                    assert_eq!(output[0], 0.5);
                    assert!(output[1..].iter().all(|x| *x == 0.0));
                } else {
                    assert_eq!(output[7], 1.0);
                    assert_eq!(output.iter().sum::<f32>(), 1.0);
                }
            } else {
                assert!(output.iter().all(|x| *x == 0.0));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(producer.view().counters().0, 0);
        drop(instance);
        assert!(!path.exists());
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }
}
#[test]
fn state_is_on_main_and_processing_events_are_on_audio() {
    let mut state = load("extended", "state", false);
    let original = state.save_state().unwrap();
    assert_eq!(original.len(), 8);
    state.load_state(&0.75f64.to_ne_bytes()).unwrap();
    assert_eq!(state.parameter(0).unwrap(), 0.75);
    state.load_state(&original).unwrap();
    assert_eq!(state.parameter(0).unwrap(), 0.5);
    let mut instrument = load("extended", "events", true);
    let mut producer = instrument.producer().unwrap();
    let mut output = [0.0; FRAMES];
    let mut events = EventList::default();
    let mut job = Job::full(1024);
    for (offset, kind) in [(3, EventKind::NoteOn), (17, EventKind::NoteOff)] {
        assert!(job.events.push(Event {
            offset,
            kind: kind as u32,
            note_id: 12,
            port: 0,
            channel: 0,
            key: 60,
            value: 1.0,
            ..Event::default()
        }));
    }
    for seq in 0..12 {
        producer.process(&job, &[], &mut output, &mut events);
        if seq >= DEFAULT_DEPTH {
            assert_eq!(events.count, 2);
            assert_eq!(events.values[0].offset, 3);
            assert_eq!(events.values[1].kind, EventKind::NoteOff as u32);
            assert_eq!(output[2], 0.0);
            assert!(output[3] > 0.25);
            assert_eq!(output[17], 0.0);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(producer.view().failure(), 0);
}
#[test]
fn crash_and_hang_fail_only_their_instance_and_cleanup_is_bounded() {
    for kind in ["crash", "hang", "error"] {
        let mut failed = load("fixture", kind, false);
        let pid = failed.pid();
        let mut healthy = load("fixture", "gain", false);
        let mut producer = failed.producer().unwrap();
        let mut peer = healthy.producer().unwrap();
        let mut a = [0.0; FRAMES];
        let mut b = [0.0; FRAMES];
        let mut events = EventList::default();
        for seq in 0..24 {
            let job = Job::full(seq * FRAMES as i64);
            producer.process(&job, &[1.0; FRAMES], &mut a, &mut events);
            peer.process(&job, &[1.0; FRAMES], &mut b, &mut events);
            if seq >= DEFAULT_DEPTH as i64 {
                assert_eq!(b[0], 0.5);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_ne!(producer.view().failure(), Failure::None as u32);
        assert_eq!(a, [0.0; FRAMES]);
        assert_eq!(peer.view().failure(), 0);
        let before = Instant::now();
        drop(failed);
        assert!(before.elapsed() < Duration::from_secs(1));
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }
}

#[test]
fn incompatible_helper_is_rejected_and_reaped_on_scan_and_load() {
    let bad = fixtures().join("bad-helper");
    let entries = runtime::scan(
        &bad,
        &[fixtures().join("fixture.clap")],
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(matches!(&entries[0], ScanEntry::Error { error, .. } if error.contains("handshake")));
    let pid = std::fs::read_to_string(bad.with_extension("pid")).unwrap();
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    let result = Instance::load(
        &bad,
        &fixtures().join("fixture.clap"),
        "org.clausters.fixture.gain",
        config(false),
        Duration::from_secs(1),
    );
    assert!(matches!(result, Err(error) if error.contains("handshake")));
    let pid = std::fs::read_to_string(bad.with_extension("pid")).unwrap();
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn multiple_ports_keep_native_ids_and_channel_order() {
    let mut cfg = config(false);
    cfg.inputs = 3;
    cfg.outputs = 3;
    let mut instance = Instance::load(
        helper(),
        &fixtures().join("extended.clap"),
        "org.clausters.fixture.ports",
        cfg,
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(
        instance
            .metadata()
            .inputs
            .iter()
            .map(|p| (p.id, p.channels))
            .collect::<Vec<_>>(),
        [(10, 2), (11, 1)]
    );
    let mut producer = instance.producer().unwrap();
    let mut input = [0.0; 3 * FRAMES];
    input[..FRAMES].fill(1.0);
    input[FRAMES..2 * FRAMES].fill(2.0);
    input[2 * FRAMES..].fill(3.0);
    let mut output = [0.0; 3 * FRAMES];
    let mut events = EventList::default();
    for seq in 0..12 {
        assert_no_alloc(|| {
            producer.process(&Job::full(0), &input, &mut output, &mut events);
        });
        if seq >= DEFAULT_DEPTH {
            assert!(input.iter().zip(output).all(|(a, b)| a * 0.5 == b));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(producer.view().failure(), 0);
}

#[test]
fn frozen_events_resume_in_order_and_backlog_overflow_fails() {
    let mut instrument = load("extended", "events", true);
    let mut producer = instrument.producer().unwrap();
    let mut output = [0.0; FRAMES];
    let mut events = EventList::default();
    for seq in 0..12 {
        let mut job = Job::full(1024);
        if seq == 1 {
            job.spans[0].active = 0;
            job.spans[0].playing = 0;
        }
        if seq <= 1 || seq == 3 {
            job.events.push(Event {
                offset: if seq == 1 { 40 } else { 3 },
                kind: if seq == 1 {
                    EventKind::NoteOff
                } else {
                    EventKind::NoteOn
                } as u32,
                note_id: 12,
                port: 0,
                channel: 0,
                key: 60,
                value: 1.0,
                ..Event::default()
            });
        }
        assert_no_alloc(|| {
            producer.process(&job, &[], &mut output, &mut events);
        });
        if seq == DEFAULT_DEPTH {
            assert_eq!(events.values[0].offset, 3);
            assert!(output.iter().any(|x| x.abs() > 0.25));
        }
        if seq == DEFAULT_DEPTH + 1 {
            assert_eq!(events.count, 0);
            assert_eq!(output, [0.0; FRAMES]);
        }
        if seq == DEFAULT_DEPTH + 2 {
            assert_eq!(events.count, 1);
            assert_eq!(events.values[0].offset, 0);
            assert_eq!(output, [0.0; FRAMES]);
        }
        if seq == DEFAULT_DEPTH + 3 {
            assert_eq!(events.values[0].offset, 3);
            assert!(output.iter().any(|x| x.abs() > 0.25));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(producer.view().failure(), 0);
    let mut overflow = load("fixture", "instrument", true);
    let mut producer = overflow.producer().unwrap();
    let mut job = Job::full(0);
    job.spans[0].active = 0;
    for index in 0..EVENTS as u32 {
        assert!(job.events.push(Event {
            offset: index,
            kind: EventKind::NoteOn as u32,
            note_id: index as i32,
            port: 0,
            channel: 0,
            key: 60,
            value: 1.0,
            ..Event::default()
        }));
    }
    for _ in 0..4 {
        assert_no_alloc(|| {
            producer.process(&job, &[], &mut output, &mut events);
        });
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(producer.view().failure(), Failure::Capacity as u32);
    assert_eq!(output, [0.0; FRAMES]);
}

#[cfg(target_arch = "x86_64")]
#[test]
fn producer_hot_path_can_run_with_all_syscalls_trapped() {
    let config = config(false);
    let layout = std::alloc::Layout::from_size_align(config.bytes().unwrap(), 64).unwrap();
    // SAFETY: bounded aligned storage, initialized before either endpoint exists.
    let memory = unsafe { std::alloc::alloc_zeroed(layout) };
    assert!(!memory.is_null());
    unsafe { clausters_core::plugin_shm::View::initialize(memory, layout.size(), config) }.unwrap();
    // SAFETY: the allocation outlives the endpoints and the forked audit child.
    let view = unsafe {
        clausters_core::plugin_shm::View::attach(memory, layout.size(), config.generation)
    }
    .unwrap();
    let mut producer = view.producer();
    // SAFETY: the second endpoint is the sole consumer of the same live allocation.
    let mut consumer = unsafe {
        clausters_core::plugin_shm::View::attach(memory, layout.size(), config.generation)
    }
    .unwrap()
    .consumer();
    let job = Job::full(0);
    let input = [1.0; FRAMES];
    let mut output = [0.0; FRAMES];
    let mut events = EventList::default();
    // Prime the test allocator's thread-local guard before fork/seccomp.
    assert_no_alloc(|| {});
    // SAFETY: after fork the child only uses its copied endpoint and stack,
    // installs seccomp and calls _exit; no thread/runtime locks or destructors.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        let mut filters = [
            libc::sock_filter {
                code: 0x20,
                jt: 0,
                jf: 0,
                k: 0,
            },
            libc::sock_filter {
                code: 0x15,
                jt: 2,
                jf: 0,
                k: libc::SYS_exit as u32,
            },
            libc::sock_filter {
                code: 0x15,
                jt: 1,
                jf: 0,
                k: libc::SYS_exit_group as u32,
            },
            libc::sock_filter {
                code: 0x06,
                jt: 0,
                jf: 0,
                k: libc::SECCOMP_RET_KILL_PROCESS,
            },
            libc::sock_filter {
                code: 0x06,
                jt: 0,
                jf: 0,
                k: libc::SECCOMP_RET_ALLOW,
            },
        ];
        let program = libc::sock_fprog {
            len: filters.len() as u16,
            filter: filters.as_mut_ptr(),
        };
        // SAFETY: valid synchronous prctl filter; _exit skips parent-only cleanup.
        unsafe {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                || libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) != 0
            {
                libc::_exit(2);
            }
        }
        assert_no_alloc(|| {
            // Successful publication and on-time delivery, with no helper scheduling.
            for sequence in 0..=DEFAULT_DEPTH {
                let delivered = producer.process(&job, &input, &mut output, &mut events);
                assert_eq!(delivered, sequence == DEFAULT_DEPTH);
                if delivered {
                    assert_eq!(output, [0.5; FRAMES]);
                }
                if sequence < DEFAULT_DEPTH {
                    let mut processing = consumer.next_job().unwrap();
                    processing.audio().1.fill(0.5);
                    processing.finish(EventList::default());
                }
            }
            // A missed result and a late result never change the authored timeline.
            for _ in 0..DEFAULT_DEPTH {
                producer.process(&job, &input, &mut output, &mut events);
            }
            let mut late = consumer.next_job().unwrap();
            late.audio().1.fill(99.0);
            late.finish(EventList::default());
            let mut due = consumer.next_job().unwrap();
            due.audio().1.fill(0.25);
            due.finish(EventList::default());
            assert!(producer.process(&job, &input, &mut output, &mut events));
            assert_eq!(output, [0.25; FRAMES]);
            assert_eq!(producer.view().counters(), (1, 1));
            // Failure and subsequent silent calls use the same syscall-free path.
            let mut invalid = job;
            invalid.span_count = 9;
            producer.process(&invalid, &input, &mut output, &mut events);
            assert_eq!(producer.view().failure(), Failure::Capacity as u32);
            for _ in 0..64 {
                assert!(!producer.process(&job, &input, &mut output, &mut events));
                assert_eq!(output, [0.0; FRAMES]);
            }
        });
        unsafe {
            libc::_exit(0);
        }
    }
    let mut status = 0;
    // SAFETY: wait for exactly the child created above, outside RT.
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    assert_eq!(
        status, 0,
        "producer attempted a syscall/allocation or seccomp failed"
    );
    // SAFETY: the child is reaped and no endpoint is used after this point.
    unsafe { std::alloc::dealloc(memory, layout) };
}

/// The X11 editor fixture, compiled once per test process.
fn editor_fixture() -> &'static Path {
    static EDITOR: OnceLock<PathBuf> = OnceLock::new();
    EDITOR.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = fixtures().join("editor.clap");
        let compiled = Command::new("cc")
            .args([
                "-std=c11", "-O2", "-fPIC", "-shared", "-Wall", "-Wextra", "-Werror", "-I",
            ])
            .arg(root.join("../../third_party/clap/include"))
            .arg(root.join("tests/fixtures/editor.c"))
            .arg("-o")
            .arg(&path)
            .arg("-lX11")
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        path
    })
}

/// What CI checks of an editor, with no window: the capability is reported
/// and an instance starts with its editor closed.
#[test]
fn native_editor_is_reported_and_starts_closed() {
    use clausters_plugin_host::format::EditorCommand;
    let mut instance = Instance::load(
        helper(),
        editor_fixture(),
        "org.clausters.fixture.gain",
        config(false),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(instance.metadata().editor);
    assert!(!instance.editor(EditorCommand::Query).unwrap().open);
}

#[test]
#[ignore = "opens a native window; run by hand with --ignored"]
fn native_editor_reopens_resizes_and_emits_edits_while_audio_runs() {
    use clausters_plugin_host::format::EditorCommand;
    let path = editor_fixture();
    let mut instance = Instance::load(
        helper(),
        path,
        "org.clausters.fixture.gain",
        config(false),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(instance.metadata().editor);
    assert!(!instance.editor(EditorCommand::Query).unwrap().open);
    assert!(
        std::env::var_os("DISPLAY").is_some(),
        "this test opens a window: run it on a display"
    );
    let mut admin = instance.administration().unwrap();
    let pid = instance.pid();
    let segment = instance.segment_path().to_owned();
    let mut producer = instance.producer().unwrap();
    let mut output = [0.0; FRAMES];
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
            producer.publish(&job, &[1.0; FRAMES]);
        });
        edits.extend(
            result.values[..result.count as usize]
                .iter()
                .map(|e| (e.kind, e.value)),
        );
        let until = Instant::now() + Duration::from_secs(2);
        while !producer.last_completed() {
            assert!(Instant::now() < until);
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
            producer.publish(&job, &[1.0; FRAMES]);
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
