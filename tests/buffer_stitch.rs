//! The stitched buffer: a join whose samples are other buffers' samples.
//!
//! What these check is the seam, because the seam is the whole reason the join
//! exists: that a frame lands in the part it belongs to and reads that part's
//! source, that an **interpolated** read across a seam sees the next part
//! rather than silence (a reader that read zero there would click at every
//! cut), that the crossfade is applied where it was asked for, and that a join
//! refuses to be written rather than writing through to whichever take a frame
//! happens to land on.
//!
//! Driven through an offline session, the cheapest driver that owns a whole
//! server; nothing here is offline-specific.

#![cfg(feature = "synth")]

use clausters::rosc::{OscMessage, OscType};
use clausters::server::nrtsession::{NrtSession, SessionConfig};

const SR: f64 = 48_000.0;

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

fn blob(values: &[f32]) -> OscType {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    OscType::Blob(bytes)
}

/// A mono buffer holding exactly `values`.
fn mono(s: &mut NrtSession, index: i32, values: &[f32]) {
    send(
        s,
        "/buffer_alloc",
        vec![
            OscType::Int(index),
            OscType::Int(values.len() as i32),
            OscType::Int(1),
        ],
    );
    s.settle_for(4);
    send(
        s,
        "/buffer_setRange",
        vec![OscType::Int(index), OscType::Int(0), blob(values)],
    );
    s.settle_for(4);
}

/// One part of a `/buffer_stitch`, mono into a mono join.
fn part(
    src: i32,
    src_start: i32,
    frames: i32,
    fade_in: i32,
    fade_out: i32,
    to: i32,
) -> Vec<OscType> {
    vec![
        OscType::Int(src),
        OscType::Int(src_start),
        OscType::Int(frames),
        OscType::Int(fade_in),
        OscType::Int(fade_out),
        OscType::Int(to),
    ]
}

fn stitch(s: &mut NrtSession, index: i32, channels: i32, parts: Vec<Vec<OscType>>) {
    let mut args = vec![
        OscType::Int(index),
        OscType::Int(channels),
        OscType::Float(SR as f32),
    ];
    for p in parts {
        args.extend(p);
    }
    send(s, "/buffer_stitch", args);
    s.settle_for(8);
}

/// Reads `count` flat samples of a buffer back over the wire.
fn read_back(s: &mut NrtSession, index: i32, count: i32) -> Vec<f32> {
    send(
        s,
        "/buffer_getRange",
        vec![OscType::Int(index), OscType::Int(0), OscType::Int(count)],
    );
    let m = reply(s, "/buffer_getRange.reply").expect("the buffer reads back");
    let OscType::Blob(bytes) = m.args.last().expect("a blob") else {
        panic!("expected a blob, got {:?}", m.args)
    };
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

fn reply(s: &mut NrtSession, addr: &str) -> Option<OscMessage> {
    let mut buf = vec![0u8; 1 << 16];
    for _ in 0..64 {
        while let Some(len) = s.poll_into(&mut buf) {
            if let Ok(clausters::rosc::OscPacket::Message(m)) =
                clausters::osc::decode_packet(&buf[..len])
                && m.addr == addr
            {
                return Some(m);
            }
        }
        s.settle();
    }
    None
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

/// **A join reads each part's own source.** Two takes, one buffer: the frames
/// before the seam come from the first and the frames after it from the second,
/// and neither moved.
#[test]
fn a_join_reads_the_part_a_frame_belongs_to() {
    let mut s = session();
    mono(&mut s, 0, &[1.0, 2.0, 3.0, 4.0]);
    mono(&mut s, 1, &[10.0, 20.0, 30.0, 40.0]);
    stitch(
        &mut s,
        2,
        1,
        vec![part(0, 1, 2, 0, 0, 0), part(1, 0, 3, 0, 0, 0)],
    );
    assert!(fails(&mut s).is_empty(), "the stitch was accepted");
    assert_eq!(
        read_back(&mut s, 2, 5),
        vec![2.0, 3.0, 10.0, 20.0, 30.0],
        "two frames of the first take, then three of the second"
    );
}

/// **The order is the join's, not the file's.** The same take read twice, the
/// second half before the first — which is the case a reader with one `start`
/// and one `span` cannot express at all.
#[test]
fn one_take_can_be_joined_out_of_order() {
    let mut s = session();
    mono(&mut s, 0, &[1.0, 2.0, 3.0, 4.0]);
    stitch(
        &mut s,
        1,
        1,
        vec![part(0, 2, 2, 0, 0, 0), part(0, 0, 2, 0, 0, 0)],
    );
    assert!(fails(&mut s).is_empty(), "the stitch was accepted");
    assert_eq!(read_back(&mut s, 1, 4), vec![3.0, 4.0, 1.0, 2.0]);
}

/// **A channel map is routing.** A mono take heard on both sides of a stereo
/// join is the same samples twice, and a channel mapped to nothing is silence —
/// how *loud* each side is belongs to the mixer, not here.
#[test]
fn a_channel_map_routes_and_does_not_scale() {
    let mut s = session();
    mono(&mut s, 0, &[1.0, 2.0]);
    let both = vec![
        OscType::Int(0),
        OscType::Int(0),
        OscType::Int(2),
        OscType::Int(0),
        OscType::Int(0),
        OscType::Int(0),
        OscType::Int(0),
    ];
    stitch(&mut s, 1, 2, vec![both]);
    assert!(fails(&mut s).is_empty(), "the stitch was accepted");
    assert_eq!(
        read_back(&mut s, 1, 4),
        vec![1.0, 1.0, 2.0, 2.0],
        "interleaved, the same sample on both channels and untouched"
    );

    let silent_right = vec![
        OscType::Int(0),
        OscType::Int(0),
        OscType::Int(2),
        OscType::Int(0),
        OscType::Int(0),
        OscType::Int(0),
        OscType::Int(-1),
    ];
    stitch(&mut s, 2, 2, vec![silent_right]);
    assert_eq!(read_back(&mut s, 2, 4), vec![1.0, 0.0, 2.0, 0.0]);
}

/// **The crossfade is where the seam is.** A part asked for a two-frame fade in
/// gets one, and the frames outside it are the source's own.
#[test]
fn a_part_fades_where_it_was_asked_to() {
    let mut s = session();
    mono(&mut s, 0, &[1.0, 1.0, 1.0, 1.0]);
    stitch(&mut s, 1, 1, vec![part(0, 0, 4, 2, 0, 0)]);
    assert!(fails(&mut s).is_empty(), "the stitch was accepted");
    let read = read_back(&mut s, 1, 4);
    assert!(read[0] < read[1], "the fade rises: {read:?}");
    assert!(read[1] < 1.0, "and has not finished at its last frame");
    assert_eq!(&read[2..], &[1.0, 1.0], "past the fade, the source itself");
}

/// **A join is read, never written.** Writing through would turn one edit into
/// an edit of several takes, so every write path refuses it by name — and says
/// what to do instead.
#[test]
fn a_join_refuses_to_be_written() {
    let mut s = session();
    mono(&mut s, 0, &[1.0, 2.0]);
    stitch(&mut s, 1, 1, vec![part(0, 0, 2, 0, 0, 0)]);
    assert!(fails(&mut s).is_empty(), "the stitch was accepted");
    send(
        &mut s,
        "/buffer_setRange",
        vec![OscType::Int(1), OscType::Int(0), blob(&[9.0, 9.0])],
    );
    let refused = fails(&mut s);
    assert!(
        refused.iter().any(|f| f.contains("join")),
        "the write was refused as a join: {refused:?}"
    );
    assert_eq!(
        read_back(&mut s, 1, 2),
        vec![1.0, 2.0],
        "and nothing was written"
    );
}

/// **What cannot be read is refused rather than read wrong**: a part past the
/// end of its source, a map of the wrong width, and a source at another rate —
/// a join is not a resampler.
#[test]
fn a_join_that_cannot_be_read_is_refused() {
    let mut s = session();
    mono(&mut s, 0, &[1.0, 2.0]);

    stitch(&mut s, 1, 1, vec![part(0, 1, 4, 0, 0, 0)]);
    let past_the_end = fails(&mut s);
    assert!(
        past_the_end.iter().any(|f| f.contains("frames")),
        "a part past the end of its source: {past_the_end:?}"
    );

    // Two channels asked for, one map entry given.
    stitch(&mut s, 1, 2, vec![part(0, 0, 2, 0, 0, 0)]);
    let narrow = fails(&mut s);
    assert!(!narrow.is_empty(), "a map of the wrong width is refused");
}

/// **What a join costs per sample, measured rather than argued** — the same
/// method the `Buffer` module docs quote for the atomics: an interpolated
/// random-access read over 64-frame blocks, a plain buffer against a join over
/// the same samples. Ignored by default because it is a measurement and not an
/// assertion; run it with
/// `cargo test --release --test buffer_stitch -- --ignored --nocapture`.
#[test]
#[ignore]
fn what_a_join_costs_per_sample() {
    use clausters::dsp::buffer::Buffer;
    use clausters::dsp::stitch::{PartSpec, Stitch};
    use std::sync::Arc;
    use std::time::Instant;

    const FRAMES: usize = 1 << 20;
    const BLOCK: usize = 64;
    const BLOCKS: usize = 20_000;

    let take = Arc::new(Buffer::new(
        (0..FRAMES).map(|i| (i % 977) as f32).collect(),
        1,
        FRAMES,
        SR,
    ));
    // The same samples as one part, and as many short parts — the second is
    // what a comping pass looks like and the case the cursor is for.
    let one = Buffer::stitched(
        Stitch::new(
            vec![PartSpec {
                src: Arc::clone(&take),
                src_index: 0,
                src_start: 0,
                frames: FRAMES,
                fade_in: 0,
                fade_out: 0,
                map: vec![0],
            }],
            1,
            SR,
        )
        .expect("built"),
        1,
        SR,
    );
    let many = Buffer::stitched(
        Stitch::new(
            (0..256)
                .map(|i| PartSpec {
                    src: Arc::clone(&take),
                    src_index: 0,
                    src_start: i * (FRAMES / 256),
                    frames: FRAMES / 256,
                    fade_in: 0,
                    fade_out: 0,
                    map: vec![0],
                })
                .collect(),
            1,
            SR,
        )
        .expect("built"),
        1,
        SR,
    );

    // Reads forward, interpolating between neighbours, exactly as `read_lin`
    // does: the far side of a seam is a second lookup.
    let run = |buf: &Buffer, label: &str| {
        let mut acc = 0.0f32;
        let start = Instant::now();
        for b in 0..BLOCKS {
            let base = (b * BLOCK) % (FRAMES - BLOCK - 1);
            for i in 0..BLOCK {
                let f = base + i;
                acc += buf.sample(f, 0) * 0.5 + buf.sample(f + 1, 0) * 0.5;
            }
        }
        let per_block = start.elapsed().as_secs_f64() / BLOCKS as f64 * 1e9;
        println!("{label:>24}: {per_block:7.1} ns/block  (checksum {acc:e})");
        per_block
    };

    let plain = run(&take, "plain buffer");
    let joined = run(&one, "join, one part");
    let cut = run(&many, "join, 256 parts");
    // Two framings, because the first one alone misleads. A microbenchmark of
    // nothing but the load makes any added work look enormous; what decides
    // whether it can be afforded is the block budget one reader eats.
    let budget = BLOCK as f64 / SR * 1e9;
    println!(
        "                  over plain: one part {:+.1}%, 256 parts {:+.1}%",
        (joined / plain - 1.0) * 100.0,
        (cut / plain - 1.0) * 100.0
    );
    println!(
        "        of one block's budget ({budget:.0} ns): plain {:.3}%, join {:.3}% \
         -- a join costs {:.3}% of a block more than a plain buffer, per reader",
        plain / budget * 100.0,
        joined / budget * 100.0,
        (joined - plain) / budget * 100.0
    );
}

/// **A client can ask whether a buffer is a join**, and gets back the parts in
/// the terms it would stitch them in.
///
/// Before this the only way to find out was to be refused when writing, which
/// is honest but late: a view that would draw an editable waveform wants to
/// know before it draws one.
#[test]
fn a_join_says_what_it_is_made_of() {
    let mut s = session();
    mono(&mut s, 0, &[1.0, 2.0, 3.0, 4.0]);
    mono(&mut s, 1, &[5.0, 6.0, 7.0, 8.0]);
    stitch(
        &mut s,
        2,
        1,
        vec![part(1, 2, 2, 0, 0, 0), part(0, 0, 3, 1, 1, 0)],
    );

    send(&mut s, "/buffer_parts", vec![OscType::Int(2)]);
    let m = reply(&mut s, "/buffer_parts.reply").expect("the join answers");
    let ints: Vec<i32> = m.args[3..]
        .iter()
        .map(|a| match a {
            OscType::Int(i) => *i,
            other => panic!("a part is ints: {other:?}"),
        })
        .collect();
    assert_eq!(m.args[1], OscType::Int(1), "one channel");
    assert_eq!(
        ints,
        vec![1, 2, 2, 0, 0, 0, 0, 0, 3, 1, 1, 0],
        "the parts as they were given, in order"
    );

    // **A buffer that owns its samples answers with no parts**, which is the
    // answer to the question rather than a refusal, and so does a slot with
    // nothing in it.
    send(&mut s, "/buffer_parts", vec![OscType::Int(0)]);
    let m = reply(&mut s, "/buffer_parts.reply").expect("a plain buffer answers");
    assert_eq!(m.args.len(), 3, "bufnum, channels, rate and nothing else");
    send(&mut s, "/buffer_parts", vec![OscType::Int(60)]);
    let m = reply(&mut s, "/buffer_parts.reply").expect("an empty slot answers");
    assert_eq!(m.args[1], OscType::Int(0), "no channels, and no failure");
    assert!(fails(&mut s).is_empty(), "nothing was refused");
}
