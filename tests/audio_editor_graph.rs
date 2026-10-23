//! **The audio editor's node system, heard rather than inspected.**
//!
//! `clausters_core::audio_editor` says what the editor is as nodes, and
//! `clausters_editing::audio_playback` what steps make them; these carry the
//! steps out on an offline server and measure what came out: the take is
//! heard, past its end the output is exactly zero, and neither a play nor a
//! stop is a step -- the transport's ramp and the output's declick see to
//! that -- while the meter reads the level and falls on a pause.

#![cfg(feature = "synth")]

use clausters::rosc::OscType;
use clausters::server::nrtsession::{NrtSession, SessionConfig};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
use clausters_editing::apply::{Endpoint, Step};
use clausters_editing::audio_playback::{AudioEditorPlayback, Pass};

const SR: f64 = 48_000.0;
const BLOCK: usize = 64;

fn session() -> NrtSession {
    NrtSession::open(&SessionConfig {
        sample_rate: SR,
        channels: 2,
        ..Default::default()
    })
    .expect("open")
}

fn send(s: &mut NrtSession, addr: &str, args: Vec<OscType>) {
    assert!(s.send_msg(addr, args).expect("encode"), "ring full");
}

/// Carries the steps out: every message sent, every wait a settle.
fn run(s: &mut NrtSession, steps: Vec<Step>) {
    for step in steps {
        match step {
            Step::Send(m) => send(s, &m.addr, m.args),
            Step::AwaitDone { .. } | Step::Sync(_) => {
                s.settle_for(4);
            }
            Step::Render(r) => panic!("the editor's graph asks for no render ({})", r.path),
        }
    }
    s.settle_for(4);
}

/// Every `/fail` that has arrived, as text.
fn fails(s: &mut NrtSession) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    s.settle_for(4);
    while let Some(len) = s.poll_into(&mut buf) {
        if let Ok(clausters::rosc::OscPacket::Message(m)) =
            clausters::osc::decode_packet(&buf[..len])
            && m.addr == "/fail"
        {
            out.push(format!("{:?}", m.args));
        }
    }
    out
}

/// A mono take of `frames` frames holding a constant: what the chain did to it
/// is arithmetic rather than a spectrum, and a hard edge anywhere is a step of
/// the whole level.
fn take(s: &mut NrtSession, frames: usize, value: f32) {
    send(
        s,
        "/buffer_alloc",
        vec![
            OscType::Int(0),
            OscType::Int(frames as i32),
            OscType::Int(1),
        ],
    );
    s.settle_for(4);
    send(
        s,
        "/buffer_fill",
        vec![
            OscType::Int(0),
            OscType::Int(0),
            OscType::Int(frames as i32),
            OscType::Float(value),
        ],
    );
    s.settle_for(4);
}

/// Renders `blocks` blocks and answers the left and right channels.
fn render(s: &mut NrtSession, blocks: usize) -> (Vec<f32>, Vec<f32>) {
    let out = s
        .run_to_vec((blocks * BLOCK) as u64)
        .expect("the render ran");
    out.as_chunks::<2>().0.iter().map(|f| (f[0], f[1])).unzip()
}

/// The largest step between two consecutive samples: a click is a step of
/// the whole level, a ramp of five milliseconds a step of a two-hundredth.
fn largest_step(signal: &[f32]) -> f32 {
    signal
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// One control bus's value, over the wire.
fn bus_value(s: &mut NrtSession, index: i32) -> f32 {
    send(s, "/bus_get", vec![OscType::Int(index)]);
    let mut buf = vec![0u8; 1 << 16];
    for _ in 0..64 {
        while let Some(len) = s.poll_into(&mut buf) {
            if let Ok(clausters::rosc::OscPacket::Message(m)) =
                clausters::osc::decode_packet(&buf[..len])
                && m.addr == "/bus_get.reply"
                && let Some(OscType::Float(v)) = m.args.last()
            {
                return *v;
            }
        }
        s.settle();
    }
    panic!("the bus never answered")
}

#[test]
fn eight_channel_meters_read_each_channels_amplitude() {
    let mut s = session();
    let frames = 512;
    send(
        &mut s,
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(frames), OscType::Int(8)],
    );
    s.settle_for(4);
    let expected = [0.125_f32, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0];
    let bytes = (0..frames)
        .flat_map(|_| expected)
        .flat_map(f32::to_le_bytes)
        .collect();
    send(
        &mut s,
        "/buffer_setRange",
        vec![OscType::Int(0), OscType::Int(0), OscType::Blob(bytes)],
    );
    s.settle_for(4);
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut playback = AudioEditorPlayback::new(Endpoint::default());
    run(
        &mut s,
        playback
            .sync(1, 0, 8, frames as u64, SR, SR, &mut ids)
            .unwrap(),
    );
    run(
        &mut s,
        playback.play(
            1,
            0,
            Pass::Loop {
                from: 0,
                to: frames as u64,
            },
        ),
    );
    let (left, right) = render(&mut s, 100);
    assert!((left.last().unwrap() - expected[0]).abs() < 1e-4);
    assert!((right.last().unwrap() - expected[1]).abs() < 1e-4);
    assert!(fails(&mut s).is_empty());
    let (bus, channels) = playback.meters().unwrap();
    assert_eq!(channels, 8);
    for (channel, expected) in expected.iter().enumerate() {
        let level = bus_value(&mut s, bus + channel as i32);
        assert!(
            (level - expected).abs() < 1e-4,
            "channel {channel}: expected {expected}, got {level}"
        );
    }
}

/// **A pass is heard on both sides, ramps in, ends where the take does, and
/// is silent after**: a mono take plays on the left and the right, the play
/// rises over the ramp rather than stepping, the end mark's ramp brings the
/// level to zero on the take's last frame, and past it the output is exactly
/// zero -- not the take's last sample held.
#[test]
fn a_pass_is_declicked_and_ends_where_the_take_does() {
    let mut s = session();
    let frames = 40 * BLOCK;
    take(&mut s, frames, 0.5);
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut playback = AudioEditorPlayback::new(Endpoint::default());
    let steps = playback
        .sync(1, 0, 1, frames as u64, SR, SR, &mut ids)
        .unwrap();
    run(&mut s, steps);
    let steps = playback.play(
        1,
        0,
        Pass::Until {
            end: frames as u64,
            back: 0,
        },
    );
    run(&mut s, steps);
    assert_eq!(fails(&mut s), Vec::<String>::new());

    let (left, right) = render(&mut s, 60);
    let heard = left
        .iter()
        .position(|x| *x > 0.0)
        .expect("the take is heard");
    assert!(
        left[heard + 300] > 0.49 && right[heard + 300] > 0.49,
        "on both sides, at unity: {} {}",
        left[heard + 300],
        right[heard + 300]
    );
    assert!(
        largest_step(&left) < 0.01 && largest_step(&right) < 0.01,
        "neither the play nor the end is a step: {}",
        largest_step(&left)
    );
    let over = heard + frames;
    assert!(
        left[over..].iter().chain(&right[over..]).all(|x| *x == 0.0),
        "and past the take's end the output is exactly zero"
    );
}

/// **A stop mid-pass is a ramp, and the meter falls**: the output goes to zero
/// with no step, the meter reads the take's level while it plays and falls
/// once it is paused, and closing frees what the editor made.
#[test]
fn a_stop_is_declicked_and_the_meter_falls() {
    let mut s = session();
    let frames = 400 * BLOCK;
    take(&mut s, frames, 0.5);
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut playback = AudioEditorPlayback::new(Endpoint::default());
    let steps = playback
        .sync(1, 0, 1, frames as u64, SR, SR, &mut ids)
        .unwrap();
    run(&mut s, steps);
    let steps = playback.play(
        1,
        0,
        Pass::Loop {
            from: 0,
            to: frames as u64,
        },
    );
    run(&mut s, steps);
    let (playing, _) = render(&mut s, 40);
    assert!(playing.iter().any(|x| *x > 0.49), "the take plays");
    let (bus, channels) = playback.meters().expect("a meter");
    assert_eq!(
        channels, 1,
        "the mono signal defines the visible meter width"
    );
    let level = bus_value(&mut s, bus);
    assert!(level > 0.4, "the meter reads the take: {level}");

    let steps = playback.pause();
    run(&mut s, steps);
    let (stopping, _) = render(&mut s, 40);
    assert!(
        largest_step(
            &playing[playing.len() - 1..]
                .iter()
                .chain(&stopping)
                .copied()
                .collect::<Vec<_>>()
        ) < 0.01,
        "the stop is a ramp, not a step"
    );
    assert!(
        stopping[stopping.len() - BLOCK..].iter().all(|x| *x == 0.0),
        "and then silence"
    );
    // The meter falls at its own ballistics (20 dB a second), since the
    // output is not frozen: two seconds are 40 dB.
    let (_, _) = render(&mut s, 1_600);
    let fallen = bus_value(&mut s, bus);
    assert!(
        fallen < 0.02 * level,
        "the meter falls on a pause: {fallen}"
    );

    let steps = playback.close(&mut ids).unwrap();
    run(&mut s, steps);
    assert_eq!(fails(&mut s), Vec::<String>::new());
    assert_eq!(playback.node_count(), 0);
}

/// The gain insert follows the transport and uses a linear bypass with an
/// exact dry endpoint. Repeated resume addresses the synth paused by EnvGen.
#[test]
fn gain_preview_bypasses_without_a_step_and_resumes() {
    use clausters_document::timebase::Second;
    use clausters_editing::points::point;
    let mut s = session();
    take(&mut s, 48000, 0.5);
    let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
    let mut playback = AudioEditorPlayback::new(Endpoint::default());
    run(
        &mut s,
        playback.sync(1, 0, 1, 48000, SR, SR, &mut ids).unwrap(),
    );
    ids.alloc(clausters_core::ids::Space::Buffers, 1).unwrap();
    let points = vec![
        point(Second(0.0), 0.25, 1.0, 0.0),
        point(Second(1.0), 0.25, 1.0, 0.0),
    ];
    run(
        &mut s,
        playback.gain(1, points.clone(), true, &mut ids).unwrap(),
    );
    run(
        &mut s,
        playback.play(1, 0, Pass::Loop { from: 0, to: 48000 }),
    );
    let (wet, _) = render(&mut s, 20);
    assert!(
        (wet[wet.len() - 1] - 0.125).abs() < 1e-5,
        "last {} fails {:?}",
        wet[wet.len() - 1],
        fails(&mut s)
    );
    for on in [false, true, false, true] {
        run(
            &mut s,
            playback.gain(1, points.clone(), on, &mut ids).unwrap(),
        );
        let (signal, _) = render(&mut s, 20);
        assert!(signal.iter().all(|x| x.is_finite()));
        assert!(
            largest_step(&signal) < 0.003,
            "bypass step {}",
            largest_step(&signal)
        );
        let want = if on { 0.125 } else { 0.5 };
        assert!(
            (signal[signal.len() - 1] - want).abs() < 1e-5,
            "on {on}: {}",
            signal[signal.len() - 1]
        );
    }
    run(&mut s, playback.close(&mut ids).unwrap());
    assert!(fails(&mut s).is_empty());
}

/// Every segment shape is shared with Apply, including held ends. This
/// compares complete node systems, not two copies of envelope arithmetic.
#[test]
fn preview_matches_apply_for_every_gain_shape() {
    use clausters_document::timebase::Second;
    use clausters_editing::points::point;
    for shape in 0..=8 {
        let curve = vec![
            point(Second(0.005), 0.2, shape as f64, -3.0),
            point(Second(0.025), 1.6, 1.0, 0.0),
        ];
        let mut outputs = Vec::new();
        for preview in [true, false] {
            let mut s = session();
            take(&mut s, 4096, 0.5);
            let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
            ids.alloc(clausters_core::ids::Space::Buffers, 1).unwrap();
            let mut playback = AudioEditorPlayback::new(Endpoint::default());
            if !preview {
                run(&mut s, clausters_editing::gain::steps(0, 4096, SR, &curve));
            }
            run(
                &mut s,
                playback.sync(1, 0, 1, 4096, SR, SR, &mut ids).unwrap(),
            );
            if preview {
                run(
                    &mut s,
                    playback.gain(1, curve.clone(), true, &mut ids).unwrap(),
                );
            }
            run(
                &mut s,
                playback.play(1, 0, Pass::Until { end: 4096, back: 0 }),
            );
            let (left, right) = render(&mut s, 70);
            assert!(left.iter().chain(&right).all(|x| x.is_finite()));
            assert_eq!(left, right);
            assert!(fails(&mut s).is_empty());
            outputs.push(left);
        }
        let error = outputs[0]
            .iter()
            .zip(&outputs[1])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        let at = outputs[0]
            .iter()
            .zip(&outputs[1])
            .position(|(a, b)| (a - b).abs() == error)
            .unwrap();
        assert!(
            error < 2e-5,
            "shape {shape}: error {error} at {at}: {} {}",
            outputs[0][at],
            outputs[1][at]
        );
    }
}

/// **An edit with the insert already sounding is heard as Apply would be**:
/// the curve moved under a preview replaces the insert's table, and the
/// output matches the same curve applied to the take, with no step where
/// the first point after zero lands.
#[test]
fn a_curve_edited_under_preview_matches_apply() {
    use clausters_document::timebase::Second;
    use clausters_editing::points::point;
    let before = vec![
        point(Second(0.0), 1.0, 1.0, 0.0),
        point(Second(0.08), 1.0, 1.0, 0.0),
    ];
    let after = vec![
        point(Second(0.0), 1.0, 1.0, 0.0),
        point(Second(0.02), 0.2, 1.0, 0.0),
        point(Second(0.06), 1.0, 1.0, 0.0),
    ];
    let mut outputs = Vec::new();
    for preview in [true, false] {
        let mut s = session();
        take(&mut s, 4096, 0.5);
        let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
        ids.alloc(clausters_core::ids::Space::Buffers, 1).unwrap();
        let mut playback = AudioEditorPlayback::new(Endpoint::default());
        if !preview {
            run(&mut s, clausters_editing::gain::steps(0, 4096, SR, &after));
        }
        run(
            &mut s,
            playback.sync(1, 0, 1, 4096, SR, SR, &mut ids).unwrap(),
        );
        if preview {
            run(
                &mut s,
                playback.gain(1, before.clone(), true, &mut ids).unwrap(),
            );
            run(
                &mut s,
                playback.gain(1, after.clone(), true, &mut ids).unwrap(),
            );
        }
        run(
            &mut s,
            playback.play(1, 0, Pass::Until { end: 4096, back: 0 }),
        );
        let (left, _) = render(&mut s, 70);
        assert!(fails(&mut s).is_empty());
        outputs.push(left);
    }
    let error = outputs[0]
        .iter()
        .zip(&outputs[1])
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    let at = outputs[0]
        .iter()
        .zip(&outputs[1])
        .position(|(a, b)| (a - b).abs() == error)
        .unwrap();
    assert!(
        error < 2e-5,
        "error {error} at {at}: {} {}; steps {} {}",
        outputs[0][at],
        outputs[1][at],
        largest_step(&outputs[0]),
        largest_step(&outputs[1])
    );
}

// Execute the same prepared-work and acknowledged-cleanup protocol as a client.
fn application_call(
    editing: &mut clausters_apps::editing::Editing,
    request: serde_json::Value,
) -> serde_json::Value {
    serde_json::from_str(&clausters_apps::editing::call_json(
        editing,
        &request.to_string(),
    ))
    .unwrap()
}

fn checked_steps(server: &mut NrtSession, steps: &serde_json::Value) {
    use clausters_editing::run::{Reply, Runner, Server};
    let mut runner = Runner::new();
    runner.push(
        Server::Sound,
        clausters_editing::apply::steps_from_json(steps),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut bytes = vec![0; 65536];
    while !runner.is_idle() {
        for (_, message) in runner.ready() {
            send(server, &message.addr, message.args);
        }
        server.settle_for(4);
        while let Some(len) = server.poll_into(&mut bytes) {
            if let Ok(clausters::rosc::OscPacket::Message(message)) =
                clausters::osc::decode_packet(&bytes[..len])
            {
                assert_ne!(message.addr, "/fail", "{:?}", message.args);
                assert!(!matches!(
                    runner.reply(Server::Sound, &message),
                    Reply::Refused(_)
                ));
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "unsettled step: {:?}",
            runner.awaiting()
        );
    }
}

fn application_cleanup(
    server: &mut NrtSession,
    editing: &mut clausters_apps::editing::Editing,
    resource: serde_json::Value,
) {
    use serde_json::json;
    let release = application_call(
        editing,
        json!({"verb":"cleanup","action":"prepare","resource":resource}),
    );
    assert!(release["ticket"].is_object(), "{release}");
    checked_steps(server, &release["steps"]);
    let confirmed = application_call(
        editing,
        json!({"verb":"cleanup","action":"complete","ticket":release["ticket"],"success":true}),
    );
    assert_eq!(confirmed["accepted"], true);
}

fn application_execute(
    server: &mut NrtSession,
    editing: &mut clausters_apps::editing::Editing,
    mut answer: serde_json::Value,
) -> serde_json::Value {
    use serde_json::json;
    let prepared = answer
        .pointer("/outcome/operation")
        .or_else(|| answer.get("operation"))
        .or_else(|| answer["ticket"].is_object().then_some(&answer))
        .cloned();
    if let Some(mut operation) = prepared {
        while operation["complete"] != true {
            checked_steps(server, &operation["steps"]);
            operation = application_call(
                editing,
                json!({"verb":"operation","action":"complete",
                "ticket":operation["ticket"],"batch":operation["batch"],"result":{"status":"success"}}),
            );
            assert_ne!(operation["accepted"], false, "{operation}");
        }
        assert_eq!(operation["committed"], true, "{operation}");
        if answer["outcome"].is_object() {
            answer["outcome"] = operation["outcome"].clone();
        } else if operation["stepped"].is_object() {
            answer = operation["stepped"].clone();
        } else {
            answer = operation.clone();
        }
        for key in ["freed", "stored", "version"] {
            if let Some(value) = operation.get(key) {
                answer[key] = value.clone();
            }
        }
    }
    for steps in [&answer["steps"], &answer["outcome"]["steps"]] {
        checked_steps(server, steps);
    }
    for effect in answer["effects"].as_array().into_iter().flatten() {
        checked_steps(server, &effect["steps"]);
    }
    for stored in answer["stored"].as_array().into_iter().flatten() {
        application_execute(server, editing, stored["operation"].clone());
    }
    for freed in answer["freed"].as_array().into_iter().flatten() {
        for buffer in freed["buffers"].as_array().into_iter().flatten() {
            application_cleanup(
                server,
                editing,
                json!({"server":{"connection":0,"generation":0},"buffer":buffer}),
            );
        }
    }
    for buffer in answer["freedBuffers"].as_array().into_iter().flatten() {
        application_cleanup(
            server,
            editing,
            json!({"server":{"connection":0,"generation":0},"buffer":buffer}),
        );
    }
    for resource in answer["released"].as_array().into_iter().flatten() {
        application_cleanup(server, editing, resource.clone());
    }
    assert!(fails(server).is_empty());
    answer
}

fn application_run(
    server: &mut NrtSession,
    editing: &mut clausters_apps::editing::Editing,
    request: serde_json::Value,
) -> serde_json::Value {
    let answer = application_call(editing, request);
    application_execute(server, editing, answer)
}

fn application_samples(server: &mut NrtSession, buffer: i32, frames: i32) -> Vec<f32> {
    send(
        server,
        "/buffer_getRange",
        vec![OscType::Int(buffer), OscType::Int(0), OscType::Int(frames)],
    );
    server.settle_for(4);
    let mut bytes = vec![0; 65536];
    while let Some(len) = server.poll_into(&mut bytes) {
        if let Ok(clausters::rosc::OscPacket::Message(message)) =
            clausters::osc::decode_packet(&bytes[..len])
            && message.addr == "/buffer_getRange.reply"
            && let OscType::Blob(values) = &message.args[2]
        {
            return values
                .as_chunks::<4>()
                .0
                .iter()
                .map(|value| f32::from_le_bytes(*value))
                .collect();
        }
    }
    panic!("no sample reply");
}

/// Real NRT sample jobs: WAV spill, restore and acknowledged release.
#[test]
fn application_sources_spill_restore_and_release_without_a_renderer() {
    use clausters_apps::editing::Editing;
    use serde_json::json;
    let directory = std::env::temp_dir().join(format!(
        "clausters-app-services-{}-{}",
        std::process::id(),
        clausters_core::rng::entropy_seed()
    ));
    std::fs::create_dir(&directory).unwrap();
    let mut server = session();
    take(&mut server, 100, 0.125);
    let mut editing = Editing::new();
    let member = application_call(
        &mut editing,
        json!({"verb":"openAudio","key":"take","take":0,
        "frames":100,"channels":1,"rate":SR,"display":1,"buffers":[2,3,4,5,6,7,8]}),
    )["member"]
        .clone();
    application_run(
        &mut server,
        &mut editing,
        json!({"verb":"operation","action":"prepare","member":member,"command":{"kind":"open"}}),
    );
    application_call(
        &mut editing,
        json!({"verb":"member","member":member,"call":{"verb":"sync","scratch":directory.to_str().unwrap()}}),
    );
    application_call(
        &mut editing,
        json!({"verb":"member","member":member,"call":{"verb":"window","widget":10}}),
    );
    editing.set_resident(Some(0));
    let stroke = |server: &mut NrtSession, editing: &mut Editing, seq, value| {
        let version = editing.version();
        application_run(
            server,
            editing,
            json!({"verb":"event","member":member,"addr":"/gui_event","args":[10,seq,version,"draw",0,10,[value],[0]]}),
        )
    };
    stroke(&mut server, &mut editing, 1, 0.5);
    let second = stroke(&mut server, &mut editing, 2, 0.25);
    let path = second["stored"][0]["operation"]["steps"][0]["send"]["args"][1]["s"]
        .as_str()
        .unwrap();
    let mut wav = hound::WavReader::open(path).unwrap();
    assert_eq!(wav.samples::<f32>().next().unwrap().unwrap(), 0.5);
    let undone = application_run(
        &mut server,
        &mut editing,
        json!({"verb":"step","direction":"undo","member":member}),
    );
    assert_eq!(undone["stepped"], true);
    assert_eq!(application_samples(&mut server, 1, 100)[10], 0.5);
    let third = stroke(&mut server, &mut editing, 3, 0.75);
    assert!(!third["freed"][0]["spilled"].as_array().unwrap().is_empty());
    assert!(
        application_call(&mut editing, json!({"verb":"cleanup","action":"pending"}))["tickets"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    drop(wav);
    drop(server);
    std::fs::remove_dir_all(directory).unwrap();
}

/// Clipboard lifetime and disk history are independent over real sample jobs.
#[test]
fn clipboard_paste_mix_and_disk_history_keep_independent_sample_ownership() {
    use clausters_apps::editing::Editing;
    use serde_json::json;
    let directory = std::env::temp_dir().join(format!(
        "clausters-clipboard-{}-{}",
        std::process::id(),
        clausters_core::rng::entropy_seed()
    ));
    std::fs::create_dir(&directory).unwrap();
    let mut server = session();
    take(&mut server, 100, 0.125);
    let mut origin = Editing::new();
    let member = application_call(&mut origin,json!({"verb":"openAudio","key":"origin","take":0,"frames":100,"channels":1,"rate":SR,"display":1,"buffers":[2,3,4,5]}))["member"].clone();
    application_run(
        &mut server,
        &mut origin,
        json!({"verb":"operation","action":"prepare","member":member,"command":{"kind":"open"}}),
    );
    application_call(
        &mut origin,
        json!({"verb":"member","member":member,"call":{"verb":"window","widget":10}}),
    );
    let copied = application_run(
        &mut server,
        &mut origin,
        json!({"verb":"event","member":member,"addr":"/gui_event","args":[10,1,0,"copy",10,20]}),
    );
    let document = copied["outcome"]["clipboard"].clone();
    assert!(document.is_string());
    assert_eq!(copied["outcome"]["clipboardReady"], true);
    application_run(
        &mut server,
        &mut origin,
        json!({"verb":"member","member":member,"call":{"verb":"close_file"}}),
    );
    let mut destination = Editing::new();
    let member = application_call(&mut destination,json!({"verb":"openAudio","key":"destination","take":0,"frames":100,"channels":1,"rate":SR,"display":50,"buffers":[51,52,53,54,55,56]}))["member"].clone();
    application_run(
        &mut server,
        &mut destination,
        json!({"verb":"operation","action":"prepare","member":member,"command":{"kind":"open"}}),
    );
    application_call(
        &mut destination,
        json!({"verb":"member","member":member,"call":{"verb":"window","widget":10}}),
    );
    application_call(
        &mut destination,
        json!({"verb":"member","member":member,"call":{"verb":"sync","scratch":directory.to_str().unwrap()}}),
    );
    destination.set_resident(Some(0));
    for (seq, tag, position) in [(1, "paste", 50), (2, "mix", 50)] {
        let acquired = application_call(
            &mut origin,
            json!({"verb":"exchange","action":"acquire","document":document}),
        );
        assert!(acquired["lease"].is_u64());
        let version = destination.version();
        application_run(
            &mut server,
            &mut destination,
            json!({"verb":"event","member":member,"addr":"/gui_event","args":[10,seq,version,tag,position,"buffer",document]}),
        );
        application_run(
            &mut server,
            &mut origin,
            json!({"verb":"exchange","action":"release","lease":acquired["lease"]}),
        );
    }
    let mixed = application_samples(&mut server, 50, 120);
    assert_eq!(mixed.len(), 120);
    for (i, sample) in mixed.iter().enumerate() {
        assert!((*sample - if (50..70).contains(&i) { 0.25 } else { 0.125 }).abs() < 1e-6);
    }
    let released = application_run(
        &mut server,
        &mut origin,
        json!({"verb":"exchange","action":"clear"}),
    );
    assert_eq!(released["released"].as_array().unwrap().len(), 1);
    let resources = application_call(&mut destination, json!({"verb":"resources"}));
    assert!(
        resources
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["stored"].is_string())
    );
    for frames in [120, 100] {
        application_run(
            &mut server,
            &mut destination,
            json!({"verb":"step","direction":"undo","member":member}),
        );
        let values = application_samples(&mut server, 50, frames);
        assert_eq!(values.len(), frames as usize);
        assert!(values.iter().all(|value| (*value - 0.125).abs() < 1e-6));
    }
    assert!(fails(&mut server).is_empty());
    drop(server);
    std::fs::remove_dir_all(directory).unwrap();
}
