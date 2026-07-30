//! Native side of the wasm parity harness: emits `score.bin` (the binary
//! score) and `native.f32` (the native render of that exact byte stream)
//! into `clients/web/tests/`, for the web package's `tests/parity.html` to
//! render through the wasm build and compare sample by sample. Run:
//! `cargo run -p clausters-web --example gen_parity`.
//!
//! The scene is deterministic (no noise) and the generator asserts the native
//! render contains no subnormal samples: wasm has no flush-to-zero mode, so
//! parity is only guaranteed on a denormal-free render (the recorded FTZ
//! parity policy). The page's bar is a tight tolerance rather than strict
//! bit-identity: native (system libm) and wasm (Rust's libm) round
//! transcendentals a few ULP apart — the posture tests/golden.rs records.

use std::path::Path;

use clausters::rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};

const SAMPLE_RATE: f64 = 48000.0;
const CHANNELS: u32 = 2;
/// Both sides of the comparison pin the same seed. The scene has no noise in
/// it, so today this changes nothing — but a render is a fresh take by
/// default, and a parity fixture is exactly the case that wants the opposite.
/// `tests/parity.html` passes this same number.
const PARITY_SEED: u64 = 0x5041_5249_5459_0001;

fn msg(addr: &str, args: Vec<OscType>) -> OscPacket {
    OscPacket::Message(OscMessage {
        addr: addr.into(),
        args,
    })
}

/// A score timetag: seconds from the start of the render.
fn timetag(secs: f64) -> OscTime {
    OscTime {
        seconds: secs as u32,
        fractional: (secs.fract() * 2f64.powi(32)) as u32,
    }
}

/// One length-prefixed packet appended to the score byte stream.
fn push_packet(score: &mut Vec<u8>, packet: OscPacket) {
    let bytes = encoder::encode(&packet).expect("encodable packet");
    score.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
    score.extend_from_slice(&bytes);
}

fn bundle_at(secs: f64, messages: Vec<OscPacket>) -> OscPacket {
    OscPacket::Bundle(OscBundle {
        timetag: timetag(secs),
        content: messages,
    })
}

/// Two overlapping voices of the built-in "default" def with a mid-block
/// retune — node tree, named controls and the sample-accurate scheduler,
/// kept short (0.5 s stereo) and denormal-free.
fn score_bytes() -> Vec<u8> {
    let s = |v: &str| OscType::String(v.into());
    let i = OscType::Int;
    let f = OscType::Float;
    let mut score = Vec::new();
    push_packet(
        &mut score,
        bundle_at(
            0.0,
            vec![msg(
                "/s_new",
                vec![
                    s("default"),
                    i(1000),
                    i(0),
                    i(0),
                    s("freq"),
                    f(330.0),
                    s("amp"),
                    f(0.3),
                ],
            )],
        ),
    );
    push_packet(
        &mut score,
        bundle_at(
            0.11,
            vec![
                msg(
                    "/s_new",
                    vec![
                        s("default"),
                        i(1001),
                        i(0),
                        i(0),
                        s("freq"),
                        f(440.0),
                        s("amp"),
                        f(0.2),
                    ],
                ),
                msg("/n_set", vec![i(1000), s("freq"), f(220.0)]),
            ],
        ),
    );
    push_packet(
        &mut score,
        bundle_at(0.3, vec![msg("/n_free", vec![i(1000)])]),
    );
    // Final bundle sets the render length.
    push_packet(
        &mut score,
        bundle_at(0.5, vec![msg("/n_free", vec![i(1001)])]),
    );
    score
}

fn main() {
    let score = score_bytes();
    let (samples, _seed) = clausters_web::render(&score, SAMPLE_RATE, CHANNELS, Some(PARITY_SEED))
        .expect("native render succeeds");
    let subnormals = samples.iter().filter(|x| x.is_subnormal()).count();
    assert_eq!(
        subnormals, 0,
        "the parity scene must stay denormal-free (FTZ parity policy)"
    );
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../clients/web/tests");
    std::fs::create_dir_all(&dir).expect("create clients/web/tests/");
    std::fs::write(dir.join("score.bin"), &score).expect("write score.bin");
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for x in &samples {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    std::fs::write(dir.join("native.f32"), &bytes).expect("write native.f32");
    println!(
        "wrote clients/web/tests/score.bin ({} bytes) and native.f32 ({} samples, {} ch, {} Hz)",
        score.len(),
        samples.len(),
        CHANNELS,
        SAMPLE_RATE
    );
}
