//! B1: the pulled-mode server (`ClaustersHeadless`) — no device, no sockets,
//! no threads; the test is the host, driving `process_block` and the ring
//! from one thread, exactly the way the browser's AudioWorklet does.

#![cfg(all(feature = "synth", feature = "embed"))]

use clausters::embed::ClaustersHeadless;
use clausters::osc::server::ServeBudget;
use clausters::rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};
use clausters::server::engine::BLOCK_SIZE;
use clausters::server::nrt::DelegatedKind;

const SR: f64 = 48_000.0;
const CHANNELS: usize = 2;
/// NTP epoch (1900) to Unix epoch (1970), in seconds.
const NTP_UNIX_OFFSET: f64 = 2_208_988_800.0;

fn msg(addr: &str, args: Vec<OscType>) -> Vec<u8> {
    encoder::encode(&OscPacket::Message(OscMessage {
        addr: addr.into(),
        args,
    }))
    .unwrap()
}

/// A bundle whose timetag is `unix` seconds on the server's Unix axis (the
/// tests build servers with `unix_epoch = 0`, so this is seconds from
/// sample 0).
fn bundle_at(unix: f64, messages: Vec<OscMessage>) -> Vec<u8> {
    let ntp = unix + NTP_UNIX_OFFSET;
    let seconds = ntp.trunc();
    encoder::encode(&OscPacket::Bundle(OscBundle {
        timetag: OscTime {
            seconds: seconds as u32,
            fractional: ((ntp - seconds) * 2f64.powi(32)) as u32,
        },
        content: messages.into_iter().map(OscPacket::Message).collect(),
    }))
    .unwrap()
}

/// Renders `blocks` engine blocks and returns the interleaved output.
fn pull(server: &mut ClaustersHeadless, blocks: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; blocks * BLOCK_SIZE * CHANNELS];
    server.process_block(&mut out).unwrap();
    out
}

/// Drains every pending reply, decoded.
fn replies(server: &ClaustersHeadless) -> Vec<OscMessage> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut out = Vec::new();
    while let Some(len) = server.poll_into(&mut buf) {
        if let Ok(OscPacket::Message(m)) = clausters::osc::decode_packet(&buf[..len]) {
            out.push(m);
        }
    }
    out
}

fn rms(buf: &[f32]) -> f32 {
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

/// Frequency estimate via positive-going zero crossings of one channel.
fn freq(interleaved: &[f32], channel: usize) -> f64 {
    let mono: Vec<f32> = interleaved
        .iter()
        .skip(channel)
        .step_by(CHANNELS)
        .copied()
        .collect();
    let crossings = mono
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count();
    crossings as f64 * SR / mono.len() as f64
}

#[test]
fn d_recv_and_s_new_produce_the_tone() {
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    let json = r#"{
        "name": "beep",
        "controls": [{"name": "freq", "default": 220.0}],
        "ugens": [
            {"kind": "Sine", "inputs": [{"control": 0}]},
            {"kind": "Mul",  "inputs": [{"ugen": 0}, {"const": 0.2}]},
            {"kind": "Out",  "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]
    }"#;
    assert!(server.send(&msg(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(json.as_bytes().to_vec())
        ]
    )));
    assert!(server.send(&msg(
        "/synth_new",
        vec![
            OscType::String("beep".into()),
            OscType::Int(1000),
            OscType::Int(0),
            OscType::Int(0),
            OscType::String("freq".into()),
            OscType::Float(440.0),
        ],
    )));
    // One second of pulled audio; the def lands on the first serving turn.
    let out = pull(&mut server, (SR as usize) / BLOCK_SIZE);
    let f = freq(&out, 0);
    assert!((f - 440.0).abs() < 5.0, "expected ~440 Hz, measured {f}");
    // Amp 0.2 sine on channel 0 only: rms = 0.2 / sqrt(2) / sqrt(2) = 0.1.
    assert!(rms(&out) > 0.09, "expected signal, rms = {}", rms(&out));
    let done: Vec<_> = replies(&server)
        .into_iter()
        .filter(|m| m.addr == "/done")
        .collect();
    assert!(
        done.iter()
            .any(|m| m.args.first() == Some(&OscType::String("/def_send".into()))),
        "missing /def_send synth done: {done:?}"
    );
    assert_eq!(server.clock(), SR as u64, "one second of blocks");
}

#[test]
fn bus_stream_replies_pace_on_the_sample_clock() {
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    server.ctl_set(3, 0.75);
    assert!(server.send(&msg(
        "/bus_stream",
        vec![OscType::Int(100), OscType::Int(3)],
    )));
    // 0.35 s of sample time at a 100 ms period: the immediate snapshot plus
    // three paced ones — deterministic, because headless stream time *is*
    // the sample clock, not the wall.
    pull(&mut server, (0.35 * SR) as usize / BLOCK_SIZE);
    let sets: Vec<_> = replies(&server)
        .into_iter()
        .filter(|m| m.addr == "/bus_stream.reply")
        .collect();
    assert_eq!(sets.len(), 4, "immediate + 3 paced snapshots: {sets:?}");
    for m in &sets {
        assert_eq!(m.args[0], OscType::Int(3));
        assert_eq!(m.args[1], OscType::Float(0.75));
    }
}

#[test]
fn timed_bundle_lands_on_its_exact_sample() {
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    // Mid-block on purpose: frame 4808 = 75 * 64 + 8.
    let onset = 4808.0 / SR;
    assert!(server.send(&bundle_at(
        onset,
        vec![OscMessage {
            addr: "/synth_new".into(),
            args: vec![
                OscType::String("default".into()),
                OscType::Int(1000),
                OscType::Int(0),
                OscType::Int(0),
            ],
        }],
    )));
    let out = pull(&mut server, 100);
    let first = out
        .iter()
        .position(|x| *x != 0.0)
        .expect("the note sounded")
        / CHANNELS;
    // The attack envelope's first sample may still be exactly zero; the
    // sample-accurate bar is "nothing before the target, signal right at it".
    assert!(
        (4808..4810).contains(&first),
        "onset at frame {first}, expected 4808"
    );
}

#[test]
fn buffer_load_installs_and_query_reports() {
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    let data: Vec<f32> = (0..800).map(|i| (i as f32 / 100.0).sin()).collect();
    server.buffer_load(7, 2, 44_100.0, &data).unwrap();
    assert!(server.send(&msg("/buffer_query", vec![OscType::Int(7)])));
    pull(&mut server, 1);
    let info = replies(&server)
        .into_iter()
        .find(|m| m.addr == "/buffer_query.reply")
        .expect("a /buffer_query.reply reply");
    assert_eq!(
        &info.args[..4],
        &[
            OscType::Int(7),
            OscType::Int(400),
            OscType::Int(2),
            OscType::Float(44_100.0),
        ]
    );
}

#[test]
fn inline_nrt_allocates_without_a_thread() {
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    assert!(server.send(&msg(
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(1024), OscType::Int(1)],
    )));
    pull(&mut server, 1);
    let done = replies(&server)
        .into_iter()
        .find(|m| m.addr == "/done")
        .expect("a /done for /buffer_alloc");
    assert_eq!(done.args[0], OscType::String("/buffer_alloc".into()));
}

#[test]
fn quit_is_reported_not_enacted() {
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    assert!(!server.quit_requested());
    assert!(server.send(&msg("/server_quit", vec![])));
    pull(&mut server, 1);
    assert!(server.quit_requested());
}

// ---- B6 step 1: a serving turn drains what fits ----

#[test]
fn a_burst_of_buffer_jobs_is_spread_over_turns() {
    // Sixteen allocations arrive at once and the budget starts four per turn:
    // before the budget existed all sixteen ran inside the packet that
    // submitted them, on the thread that owes the next block.
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    server.set_budget(ServeBudget {
        nrt_jobs: 4,
        ..ServeBudget::UNLIMITED
    });
    for i in 0..16 {
        assert!(server.send(&msg(
            "/buffer_alloc",
            vec![OscType::Int(i), OscType::Int(1024), OscType::Int(1)],
        )));
    }
    // The first block's turn takes four and leaves twelve queued.
    pull(&mut server, 1);
    assert_eq!(server.backlog(), 12);
    assert_eq!(
        replies(&server)
            .iter()
            .filter(|m| m.addr == "/done")
            .count(),
        4
    );
    // Three more turns finish them, in order, with nothing dropped.
    pull(&mut server, 3);
    assert_eq!(server.backlog(), 0);
    let done: Vec<i32> = replies(&server)
        .into_iter()
        .filter(|m| m.addr == "/done")
        .map(|m| match m.args[1] {
            OscType::Int(i) => i,
            ref other => panic!("expected the buffer index, got {other:?}"),
        })
        .collect();
    assert_eq!(done, (4..16).collect::<Vec<i32>>());
}

#[test]
fn a_burst_of_packets_is_spread_over_turns() {
    // The ring is the queue: what a turn does not take stays there, in order.
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    server.set_budget(ServeBudget {
        ring_packets: 3,
        ..ServeBudget::UNLIMITED
    });
    for i in 0..9 {
        assert!(server.send(&msg(
            "/buffer_alloc",
            vec![OscType::Int(i), OscType::Int(256), OscType::Int(1)],
        )));
    }
    pull(&mut server, 1);
    assert_eq!(
        replies(&server)
            .iter()
            .filter(|m| m.addr == "/done")
            .count(),
        3
    );
    pull(&mut server, 2);
    assert_eq!(
        replies(&server)
            .iter()
            .filter(|m| m.addr == "/done")
            .count(),
        6
    );
}

#[test]
fn the_default_budget_does_not_bind_on_ordinary_traffic() {
    // A ceiling that binds in normal use would turn every session into
    // latency, so the default is deliberately generous: one command, one turn.
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    assert!(server.send(&msg(
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(1024), OscType::Int(1)],
    )));
    pull(&mut server, 1);
    assert_eq!(server.backlog(), 0);
    assert!(replies(&server).iter().any(|m| m.addr == "/done"));
}

#[test]
fn nothing_runs_before_its_turn() {
    // Submitting queues; the work happens in the turn. Sending without ever
    // pulling a block must leave the job untouched -- that separation is what
    // makes the budget possible at all.
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    assert!(server.send(&msg(
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(1024), OscType::Int(1)],
    )));
    assert!(replies(&server).is_empty());
    pull(&mut server, 1);
    assert!(replies(&server).iter().any(|m| m.addr == "/done"));
}

// ---- B6 step 2a: a long take is loaded in chunks ----

/// Reads the first `count` samples of buffer `index` back through
/// `/buffer_get`, as a client would.
fn buffer_head(server: &mut ClaustersHeadless, index: i32, count: i32) -> Vec<f32> {
    let mut args = vec![OscType::Int(index)];
    args.extend((0..count).map(OscType::Int));
    assert!(server.send(&msg("/buffer_get", args)));
    pull(server, 1);
    replies(server)
        .into_iter()
        .find(|m| m.addr == "/buffer_get.reply")
        .map(|m| {
            m.args
                .iter()
                .filter_map(|a| match a {
                    OscType::Float(f) => Some(*f),
                    _ => None,
                })
                .collect()
        })
        .expect("a /buffer_get.reply carrying the samples")
}

#[test]
fn a_staged_load_matches_the_one_shot_load() {
    // The chunked path must produce exactly the buffer the whole-payload path
    // produces -- otherwise "the same thing, paced" is not what it is.
    let frames = 5_000;
    let channels = 2;
    let data: Vec<f32> = (0..frames * channels)
        .map(|i| (i as f32 / 997.0).sin())
        .collect();

    let mut one = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    one.buffer_load(0, channels, SR, &data).unwrap();

    let mut staged = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    let ticket = staged.buffer_load_begin(0, channels, SR, frames).unwrap();
    // Paced the way a host would: a chunk at a time, from the budget.
    let step = ServeBudget::default().install_frames * channels;
    for (i, run) in data.chunks(step).enumerate() {
        staged.buffer_load_chunk(ticket, i * step, run).unwrap();
    }
    staged.buffer_load_end(ticket).unwrap();

    assert_eq!(
        buffer_head(&mut one, 0, 64),
        buffer_head(&mut staged, 0, 64)
    );
}

#[test]
fn a_staged_load_is_invisible_until_it_ends() {
    // A half-written take must never be readable: the engine sees the previous
    // buffer until the swap, which is the rule the async path already follows.
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    server.buffer_load(0, 1, SR, &[1.0; 8]).unwrap();

    let ticket = server.buffer_load_begin(0, 1, SR, 8).unwrap();
    server.buffer_load_chunk(ticket, 0, &[9.0; 4]).unwrap();
    assert_eq!(buffer_head(&mut server, 0, 8), vec![1.0; 8]);

    server.buffer_load_chunk(ticket, 4, &[9.0; 4]).unwrap();
    server.buffer_load_end(ticket).unwrap();
    assert_eq!(buffer_head(&mut server, 0, 8), vec![9.0; 8]);
}

#[test]
fn a_cancelled_load_installs_nothing() {
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    server.buffer_load(0, 1, SR, &[1.0; 8]).unwrap();
    let ticket = server.buffer_load_begin(0, 1, SR, 8).unwrap();
    server.buffer_load_chunk(ticket, 0, &[9.0; 8]).unwrap();
    server.buffer_load_cancel(ticket);
    assert!(server.buffer_load_end(ticket).is_err());
    assert_eq!(buffer_head(&mut server, 0, 8), vec![1.0; 8]);
}

#[test]
fn a_chunk_past_the_end_is_refused() {
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    let ticket = server.buffer_load_begin(0, 1, SR, 8).unwrap();
    assert!(server.buffer_load_chunk(ticket, 4, &[0.0; 8]).is_err());
    assert!(server.buffer_load_chunk(999, 0, &[0.0; 1]).is_err());
}

// ---- B6 step 2b: the jobs the host does better leave ----

#[test]
fn a_soundfile_read_leaves_and_comes_back_installed() {
    // The browser's shape: /buffer_allocRead reaches an engine with no
    // filesystem, so it leaves, and the host installs the samples itself
    // through a staged load. The reply is still the command's own /done.
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    server.delegate_jobs();
    assert!(server.send(&msg(
        "/buffer_allocRead",
        vec![OscType::Int(3), OscType::String("take.wav".into())],
    )));
    pull(&mut server, 1);

    // Nothing was answered: the job is out with the host.
    assert!(replies(&server).is_empty());
    let job = server.take_delegated().expect("a job for the host");
    assert_eq!(job.index, 3);
    assert_eq!(
        job.kind,
        DelegatedKind::AllocRead {
            path: "take.wav".into(),
            file_start: 0,
            num_frames: 0,
            channels: Vec::new(),
        }
    );
    // Asking twice does not hand it out twice.
    assert!(server.take_delegated().is_none());

    // The host does the work and installs it the way a page would.
    let ticket = server.buffer_load_begin(3, 1, SR, 4).unwrap();
    server.buffer_load_chunk(ticket, 0, &[0.25; 4]).unwrap();
    server.buffer_load_end(ticket).unwrap();
    server.finish_delegated(job.ticket, Ok(()));

    pull(&mut server, 1);
    let done = replies(&server)
        .into_iter()
        .find(|m| m.addr == "/done")
        .expect("the command's own /done");
    assert_eq!(done.args[0], OscType::String("/buffer_allocRead".into()));
    assert_eq!(buffer_head(&mut server, 3, 4), vec![0.25; 4]);
}

#[test]
fn a_delegated_failure_is_the_hosts_message() {
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    server.delegate_jobs();
    assert!(server.send(&msg(
        "/buffer_allocRead",
        vec![OscType::Int(0), OscType::String("missing.wav".into())],
    )));
    pull(&mut server, 1);
    let job = server.take_delegated().unwrap();
    server.finish_delegated(job.ticket, Err("missing.wav: no such file".into()));
    pull(&mut server, 1);
    let fail = replies(&server)
        .into_iter()
        .find(|m| m.addr == "/fail")
        .expect("a /fail carrying the host's message");
    assert_eq!(fail.args[0], OscType::String("/buffer_allocRead".into()));
    assert!(matches!(&fail.args[1], OscType::String(s) if s.contains("no such file")));
}

#[test]
fn a_delegated_job_blocks_the_queue_behind_it() {
    // One queue, submission order: a /buffer_alloc that overtook the read it
    // follows would be a different program, and it is the read's own buffer.
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    server.delegate_jobs();
    assert!(server.send(&msg(
        "/buffer_allocRead",
        vec![OscType::Int(1), OscType::String("take.wav".into())],
    )));
    assert!(server.send(&msg(
        "/buffer_alloc",
        vec![OscType::Int(2), OscType::Int(64), OscType::Int(1)],
    )));
    pull(&mut server, 4);
    let job = server.take_delegated().unwrap();
    // Four turns and the second job has not run: the first is still out.
    assert!(replies(&server).is_empty());

    server
        .buffer_load_begin(1, 1, SR, 2)
        .and_then(|t| server.buffer_load_end(t))
        .unwrap();
    server.finish_delegated(job.ticket, Ok(()));
    pull(&mut server, 2);
    let done: Vec<String> = replies(&server)
        .into_iter()
        .filter(|m| m.addr == "/done")
        .map(|m| match &m.args[0] {
            OscType::String(s) => s.clone(),
            other => panic!("expected the command name, got {other:?}"),
        })
        .collect();
    assert_eq!(done, vec!["/buffer_allocRead", "/buffer_alloc"]);
}

#[test]
fn a_late_answer_to_a_forgotten_job_reports_against_nothing() {
    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    server.delegate_jobs();
    assert!(server.send(&msg(
        "/buffer_allocRead",
        vec![OscType::Int(0), OscType::String("take.wav".into())],
    )));
    pull(&mut server, 1);
    let job = server.take_delegated().unwrap();
    server.finish_delegated(job.ticket, Ok(()));
    pull(&mut server, 1);
    let _ = replies(&server);
    // The same ticket again, and a ticket that never existed.
    server.finish_delegated(job.ticket, Err("late".into()));
    server.finish_delegated(9999, Err("never".into()));
    pull(&mut server, 1);
    assert!(replies(&server).is_empty());
}

#[test]
fn a_soundfile_read_still_runs_here_when_no_host_takes_it() {
    // The other half of delegation, and the one a test was missing: without a
    // host the pulled server does the read itself, as it always did. The gap
    // was real -- a guard written for the delegating runner reached the inline
    // one and stranded every /buffer_allocRead, and no test here saw it.
    let dir = std::env::temp_dir().join(format!("clausters-headless-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("take.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    {
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..16 {
            w.write_sample(i as f32 / 100.0).unwrap();
        }
        w.finalize().unwrap();
    }

    let mut server = ClaustersHeadless::new(SR, CHANNELS, 0.0).unwrap();
    assert!(server.send(&msg(
        "/buffer_allocRead",
        vec![
            OscType::Int(0),
            OscType::String(path.to_string_lossy().into_owned()),
        ],
    )));
    pull(&mut server, 1);
    assert!(server.take_delegated().is_none(), "no host was asked for");
    let done = replies(&server)
        .into_iter()
        .find(|m| m.addr == "/done")
        .expect("the read completed here");
    assert_eq!(done.args[0], OscType::String("/buffer_allocRead".into()));
    assert_eq!(buffer_head(&mut server, 0, 4), vec![0.0, 0.01, 0.02, 0.03]);
    std::fs::remove_file(&path).ok();
}
