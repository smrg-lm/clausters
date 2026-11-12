//! A server whose device could not honour the requested rate runs its engine
//! at the rate the device gave. Everything measured in the engine's samples
//! -- a buffer it allocates, a beat placed on the transport -- is measured at
//! that rate, not at the one that was asked for.

use std::sync::Arc;
use std::time::Duration;

use clausters::osc::server::{OscServer, ServerInfo};
use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
use clausters::server::engine::{BLOCK_SIZE, Engine, engine_pair_full};
use clausters::server::ipc::{IpcPeer, Role, Segment};

/// What the device runs at, and what was asked of it.
const ACTUAL: f64 = 48_000.0;
const NOMINAL: f64 = 44_100.0;
const PEER: u32 = 1;

struct Rig {
    engine: Engine,
    server: OscServer,
    client: IpcPeer,
}

fn rig() -> Rig {
    let segment = Segment::in_memory();
    let (engine, handle) = engine_pair_full(
        ACTUAL as f32,
        2,
        0,
        Some(Arc::clone(&segment)),
        128,
        1024,
        clausters::dsp::Limits::default(),
    );
    let info = ServerInfo {
        nominal_sample_rate: NOMINAL,
        actual_sample_rate: ACTUAL,
    };
    let mut server = OscServer::headless(info, handle, 0.0);
    server
        .attach_ipc(IpcPeer::new(Arc::clone(&segment), Role::Server))
        .unwrap();
    Rig {
        engine,
        server,
        client: IpcPeer::new(segment, Role::Client),
    }
}

impl Rig {
    /// Sends `addr args` and drives the server until a reply at `expect`.
    fn ask(&mut self, addr: &str, args: Vec<OscType>, expect: &str) -> OscMessage {
        let packet = encoder::encode(&OscPacket::Message(OscMessage {
            addr: addr.into(),
            args,
        }))
        .unwrap();
        assert!(self.client.push(PEER, &packet));
        let mut buf = vec![0u8; 65536];
        for _ in 0..500 {
            self.server.step();
            self.engine.process_block(&mut vec![0.0f32; BLOCK_SIZE * 2]);
            while let Some((_, len)) = self.client.try_pop(&mut buf) {
                if let Ok(OscPacket::Message(msg)) = clausters::osc::decode_packet(&buf[..len])
                    && msg.addr == expect
                {
                    return msg;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("no {expect} for {addr}");
    }
}

/// A buffer the server allocates holds samples at the engine's rate, so that
/// is the rate it carries -- the one everything comparing it to the engine
/// (a playback rate, a kernel's partitions) reads.
#[test]
fn an_allocated_buffer_carries_the_rate_the_engine_runs_at() {
    let mut rig = rig();
    rig.ask(
        "/buffer_alloc",
        vec![OscType::Int(0), OscType::Int(100), OscType::Int(1)],
        "/done",
    );
    let reply = rig.ask(
        "/buffer_query",
        vec![OscType::Int(0)],
        "/buffer_query.reply",
    );
    assert_eq!(reply.args[3], OscType::Float(ACTUAL as f32), "{reply:?}");
}

/// A beat on the transport is `beat * rate / tempo` of the engine's samples:
/// at tempo 2, beat 2 is one second, which is `ACTUAL` samples.
#[test]
fn a_beat_on_the_transport_is_placed_at_the_rate_the_engine_runs_at() {
    let mut rig = rig();
    rig.ask(
        "/transport_set",
        vec![OscType::Int(0), OscType::Long(0), OscType::Double(2.0)],
        "/done",
    );
    rig.ask(
        "/transport_locate",
        vec![OscType::Int(0), OscType::Double(2.0)],
        "/done",
    );
    let reply = rig.ask(
        "/transport_query",
        vec![OscType::Int(0)],
        "/transport_query.reply",
    );
    // `positionSample` is the eighth field.
    assert_eq!(reply.args[7], OscType::Long(ACTUAL as i64), "{reply:?}");

    // And back: a sample position reads as the beat it is at that rate.
    rig.ask(
        "/transport_locateSample",
        vec![OscType::Int(0), OscType::Long(ACTUAL as i64 / 2)],
        "/done",
    );
    let reply = rig.ask(
        "/transport_query",
        vec![OscType::Int(0)],
        "/transport_query.reply",
    );
    assert_eq!(reply.args[4], OscType::Double(1.0), "{reply:?}");
}
