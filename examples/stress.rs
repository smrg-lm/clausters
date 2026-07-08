//! Single-core stress test against a **running** server: how many voices fit
//! in the real audio callback before the sound breaks?
//!
//! Unlike `examples/bench` (offline throughput: how fast `process_block`
//! spins with nobody pacing it), this drives the production path — cpal
//! callback, real-time scheduling, the OSC front door — and watches the
//! server's own CPU meter (`/status.reply`): average and peak per-block load
//! as a percentage of the block budget, plus the late-block counter (blocks
//! that exceeded their budget — the engine-side xrun proxy).
//!
//! Run the server first (single core = the default `--workers 0`; raise the
//! node table for large counts), then ramp:
//!
//! ```sh
//! cargo run --release &                         # or: target/release/clausters
//! cargo run --release --example stress                       # 1-sine nodes
//! cargo run --release --example stress -- --sines 10 --step 20
//! cargo run --release --example stress -- --limit 80 --settle 3
//! ```
//!
//! The default server build runs the callback under real-time scheduling
//! (the `rtprio` feature), which is what makes the numbers measure DSP
//! throughput; against a server built without it, the ceiling is scheduling
//! jitter instead — roughly half the capacity (see BUILD.md).
//!
//! The two axes of the test:
//! - `--sines n`: sinusoids summed **inside one def** (per-node DSP weight);
//! - the ramp: nodes added `--step` at a time (per-node engine overhead).
//!
//! Each step settles, then polls `/status`; the ramp stops when the peak
//! load crosses `--limit` (%), a block runs late, or `/fail` reports a full
//! node table. The last row printed before the stop is the stable capacity.
//! Cross-check xruns externally with `pw-top` (the ERR column) while it runs.
//!
//! Keep `--limit` under 100: past sustained-100% the callback stops sleeping
//! between cycles and RTKit's RLIMIT_RTTIME watchdog raises SIGXCPU — the
//! server's guard then demotes the audio thread back to SCHED_OTHER (the
//! server survives, but the measurement is over: the ramp is no longer
//! testing a real-time thread). Sooner with small quanta
//! (`PIPEWIRE_QUANTUM=64/48000`), where there is less slack per cycle. The
//! default 90 leaves that regime alone.

use std::net::UdpSocket;
use std::time::Duration;

use clausters::osc::server::DEFAULT_PORT;
use clausters::rosc::{OscMessage, OscPacket, OscType, decoder, encoder};

struct Options {
    addr: String,
    /// Sinusoids summed inside the def.
    sines: usize,
    /// Nodes added per ramp step.
    step: usize,
    /// Seconds to let the meter settle after each step (>= 1, the EMA's
    /// time constant, or the average lags the ramp).
    settle: f64,
    /// Peak-load stop threshold, in percent of the block budget.
    limit: f32,
    /// Hard ceiling on nodes (0 = until the server or the limit gives up).
    max: usize,
    /// Linear amplitude of each node (kept tiny: m nodes sum on the bus).
    amp: f32,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let mut o = Options {
            addr: format!("127.0.0.1:{DEFAULT_PORT}"),
            sines: 1,
            step: 50,
            settle: 2.0,
            limit: 90.0,
            max: 0,
            amp: 0.002,
        };
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = |name: &str| {
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            match arg.as_str() {
                "--addr" => o.addr = value("--addr")?,
                "--sines" => o.sines = value("--sines")?.parse().map_err(|e| format!("{e}"))?,
                "--step" => o.step = value("--step")?.parse().map_err(|e| format!("{e}"))?,
                "--settle" => o.settle = value("--settle")?.parse().map_err(|e| format!("{e}"))?,
                "--limit" => o.limit = value("--limit")?.parse().map_err(|e| format!("{e}"))?,
                "--max" => o.max = value("--max")?.parse().map_err(|e| format!("{e}"))?,
                "--amp" => o.amp = value("--amp")?.parse().map_err(|e| format!("{e}"))?,
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        if o.sines == 0 || o.step == 0 {
            return Err("--sines and --step must be >= 1".into());
        }
        Ok(o)
    }
}

/// The stress def: `sines` detuned sinusoids around the `freq` control,
/// summed and scaled to `amp`, written to bus 0. Per sinusoid past the first
/// that is one `Mul` (the detune ratio), one `SinOsc` and one `Add`.
fn stress_def_json(sines: usize, amp: f32) -> String {
    let mut ugens = String::new();
    // ugen 0: SinOsc(freq)
    ugens.push_str(r#"{"kind": "SinOsc", "inputs": [{"control": 0}]}"#);
    let mut acc = 0usize; // index of the running sum
    let mut next = 1usize;
    for k in 1..sines {
        let ratio = 1.0 + k as f64 * 0.03;
        ugens.push_str(&format!(
            r#", {{"kind": "Mul", "inputs": [{{"control": 0}}, {{"const": {ratio}}}]}}"#
        ));
        ugens.push_str(&format!(
            r#", {{"kind": "SinOsc", "inputs": [{{"ugen": {next}}}]}}"#
        ));
        ugens.push_str(&format!(
            r#", {{"kind": "Add", "inputs": [{{"ugen": {acc}}}, {{"ugen": {}}}]}}"#,
            next + 1
        ));
        acc = next + 2;
        next += 3;
    }
    ugens.push_str(&format!(
        r#", {{"kind": "Mul", "inputs": [{{"ugen": {acc}}}, {{"const": {amp}}}]}}"#
    ));
    ugens.push_str(&format!(
        r#", {{"kind": "Out", "inputs": [{{"const": 0.0}}, {{"ugen": {next}}}]}}"#
    ));
    format!(
        r#"{{"name": "stress", "controls": [{{"name": "freq", "default": 220.0}}], "ugens": [{ugens}]}}"#
    )
}

/// One `/status` poll: (avg %, peak %, late blocks since boot). Skips
/// unrelated packets; counts stray `/fail`s through `fails`.
fn poll_status(
    socket: &UdpSocket,
    addr: &str,
    fails: &mut usize,
) -> Result<(f32, f32, i32), Box<dyn std::error::Error>> {
    send(socket, addr, "/status", vec![])?;
    loop {
        match recv(socket, fails)? {
            Some(msg) if msg.addr == "/status.reply" => {
                let avg = match msg.args.get(5) {
                    Some(OscType::Float(v)) => *v,
                    _ => return Err("malformed /status.reply (avg)".into()),
                };
                let peak = match msg.args.get(6) {
                    Some(OscType::Float(v)) => *v,
                    _ => return Err("malformed /status.reply (peak)".into()),
                };
                let late = match msg.args.get(9) {
                    Some(OscType::Int(v)) => *v,
                    _ => {
                        return Err("no late-block counter in /status.reply (older server?)".into());
                    }
                };
                return Ok((avg, peak, late));
            }
            Some(_) => continue,
            None => return Err("no /status.reply (is the server running?)".into()),
        }
    }
}

fn send(
    socket: &UdpSocket,
    addr: &str,
    osc_addr: &str,
    args: Vec<OscType>,
) -> Result<(), Box<dyn std::error::Error>> {
    let packet = OscPacket::Message(OscMessage {
        addr: osc_addr.into(),
        args,
    });
    socket.send_to(&encoder::encode(&packet)?, addr)?;
    Ok(())
}

/// Receives one message (None on timeout), counting `/fail`s on the side.
fn recv(
    socket: &UdpSocket,
    fails: &mut usize,
) -> Result<Option<OscMessage>, Box<dyn std::error::Error>> {
    let mut buf = [0u8; 65536];
    let Ok((len, _)) = socket.recv_from(&mut buf) else {
        return Ok(None);
    };
    if let (_, OscPacket::Message(msg)) = decoder::decode_udp(&buf[..len])? {
        if msg.addr == "/fail" {
            *fails += 1;
        }
        return Ok(Some(msg));
    }
    Ok(None)
}

/// Waits for a reply whose address matches, skipping others (2 s timeout).
/// A `/fail` while waiting is fatal and reported verbatim — e.g. the server
/// rejecting the stress def.
fn expect(
    socket: &UdpSocket,
    addr: &str,
    fails: &mut usize,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        match recv(socket, fails)? {
            Some(msg) if msg.addr == addr => return Ok(()),
            Some(msg) if msg.addr == "/fail" => {
                return Err(format!("server replied /fail {:?}", msg.args).into());
            }
            Some(_) => continue,
            None => return Err(format!("timed out waiting for {addr}").into()),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opts =
        Options::parse().map_err(|e| format!("{e}\nsee the header of examples/stress.rs"))?;
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    let addr = opts.addr.as_str();
    let mut fails = 0usize;

    // Reachability first, so "no server" and "bad def" read differently: a
    // /status round-trip also confirms the server is new enough to report
    // the CPU meter (poll_status errors out on the older field set).
    poll_status(&socket, addr, &mut fails)
        .map_err(|e| format!("{e} — expected a running server on {addr} (cargo run --release)"))?;

    // Ship the def and wait for /done.
    let json = stress_def_json(opts.sines, opts.amp);
    send(
        &socket,
        addr,
        "/d_recv",
        vec![OscType::Blob(json.into_bytes())],
    )?;
    expect(&socket, "/done", &mut fails)?;

    let (_, _, mut late_before) = poll_status(&socket, addr, &mut fails)?;
    println!(
        "stress: {} sinusoid(s) per node, +{} nodes per step, settle {:.1} s, stop at peak > {:.0}% or a late block",
        opts.sines, opts.step, opts.settle, opts.limit
    );
    println!(
        "{:>7} {:>9} {:>8} {:>8} {:>6}",
        "nodes", "sines", "avg%", "peak%", "late"
    );

    let mut nodes = 0usize;
    let mut next_id = 1000i32;
    let mut stable = 0usize;
    let stop_reason;
    loop {
        // One ramp step: /s_new × step, frequencies spread deterministically.
        // Throttled in small batches: hundreds of adds landing on one block
        // boundary make *that* block run late (250 tree inserts inside one
        // 1.33 ms budget) — an artifact of the ramp, not steady-state load.
        for burst in 0..opts.step {
            if burst % 25 == 0 && burst > 0 {
                std::thread::sleep(Duration::from_millis(25));
            }
            let freq = 50.0 + (next_id - 1000) as f32 % 900.0;
            send(
                &socket,
                addr,
                "/s_new",
                vec![
                    OscType::String("stress".into()),
                    OscType::Int(next_id),
                    OscType::Int(1), // tail of
                    OscType::Int(0), // the root group
                    OscType::String("freq".into()),
                    OscType::Float(freq),
                ],
            )?;
            next_id += 1;
        }
        nodes += opts.step;
        std::thread::sleep(Duration::from_secs_f64(opts.settle));

        let fails_before = fails;
        // Two polls: the first closes the peak window holding the node
        // insertion transient; the second reads a clean steady-state window.
        // Only the clean window decides the stop — an insertion-time late
        // block is the ramp's fault, not sustained overload (it is still
        // reported, as a note).
        let (_, _, late_mid) = poll_status(&socket, addr, &mut fails)?;
        std::thread::sleep(Duration::from_secs_f64((opts.settle * 0.5).max(0.3)));
        let (avg, peak, late) = poll_status(&socket, addr, &mut fails)?;
        let late_step = late - late_mid;
        if late_mid > late_before {
            println!(
                "        ({} late block(s) during the ramp step itself)",
                late_mid - late_before
            );
        }
        late_before = late;
        println!(
            "{:>7} {:>9} {:>7.1} {:>7.1} {:>6}",
            nodes,
            nodes * opts.sines,
            avg,
            peak,
            late_step
        );

        if fails > fails_before {
            stop_reason = "/fail from the server (full node table? see --max-nodes)".to_string();
            break;
        }
        if late_step > 0 {
            stop_reason = format!("{late_step} late block(s) — the callback missed its budget");
            break;
        }
        if peak > opts.limit {
            stop_reason = format!("peak {peak:.1}% > limit {:.0}%", opts.limit);
            break;
        }
        stable = nodes;
        if opts.max > 0 && nodes >= opts.max {
            stop_reason = "reached --max".into();
            break;
        }
    }

    println!("\nstopped: {stop_reason}");
    println!(
        "last stable: {stable} node(s) = {} sinusoid(s)",
        stable * opts.sines
    );
    send(&socket, addr, "/g_freeAll", vec![OscType::Int(0)])?;
    Ok(())
}
