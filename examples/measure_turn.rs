//! What one **pulled serving turn** costs — the measurement the browser
//! engine's serving budget is set from.
//!
//! The pulled server (`ClaustersHeadless`) runs one serving turn before each
//! engine block, on the thread that owes the audio. In a browser that thread is
//! the AudioWorklet's and the deadline is the render quantum: **128 frames,
//! 2.67 ms at 48 kHz**. This prints, against that deadline, what a turn costs
//! when buffer work lands in it. Run it in release mode:
//!
//! ```sh
//! cargo run --release --features embed --example measure_turn
//! ```
//!
//! Read it with two things in mind, because the first table is a trap:
//!
//! - **`/buffer_alloc` looks free and is not.** Linux hands out lazily-zeroed
//!   pages, so allocating 110 MB costs microseconds here and the real price is
//!   paid later, in page faults, on whatever thread first touches the samples —
//!   the audio thread. The number is an artifact of the platform's virtual
//!   memory, not evidence of a cheap operation, and a browser (where the
//!   equivalent is growing wasm linear memory) does not have it.
//! - **`buffer_load` is the honest shape**: host-decoded frames copied into the
//!   engine, which is exactly the path a page takes, with nothing deferred.
//!
//! The last section is what the budget is for: a burst of commands arriving
//! together, with the ceiling off and on.
//!
//! These are *native* numbers on one machine, so they bound the browser's from
//! below — they say which operations cannot fit, never that one does.

use clausters::embed::ClaustersHeadless;
use clausters::rosc::{OscMessage, OscPacket, OscType, encoder};
use clausters::server::engine::BLOCK_SIZE;
use std::time::Instant;

const SR: f64 = 48_000.0;
const CH: usize = 2;
// The render quantum a browser owes: 128 frames. Our engine block is 64.
const QUANTUM_MS: f64 = 128.0 / 48.0;

fn msg(addr: &str, args: Vec<OscType>) -> Vec<u8> {
    encoder::encode(&OscPacket::Message(OscMessage {
        addr: addr.into(),
        args,
    }))
    .unwrap()
}

fn time_one_block(server: &mut ClaustersHeadless) -> f64 {
    let mut out = vec![0.0f32; BLOCK_SIZE * CH];
    let t = Instant::now();
    server.process_block(&mut out).unwrap();
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    println!("quantum budget (128 frames @48k): {QUANTUM_MS:.2} ms\n");

    // 1) One allocation, by size. This is the indivisible job.
    println!("one /buffer_alloc, the turn it lands in:");
    for &(label, frames, ch) in &[
        ("1 s mono", 48_000, 1),
        ("10 s stereo", 480_000, 2),
        ("1 min stereo", 2_880_000, 2),
        ("5 min stereo", 14_400_000, 2),
    ] {
        let mut server = ClaustersHeadless::new(SR, CH, 0.0).unwrap();
        let idle = time_one_block(&mut server);
        server.send(&msg(
            "/buffer_alloc",
            vec![OscType::Int(0), OscType::Int(frames), OscType::Int(ch)],
        ));
        let hit = time_one_block(&mut server);
        let mb = frames as f64 * ch as f64 * 4.0 / 1_048_576.0;
        println!(
            "  {label:14} {mb:7.1} MB  turn {hit:8.3} ms  (idle {idle:.3})  {}",
            if hit > QUANTUM_MS {
                format!("OVER by {:.1}x", hit / QUANTUM_MS)
            } else {
                "fits".into()
            }
        );
    }

    // 1b) The same sizes, as a real copy. `/buffer_alloc` is nearly free above
    // because Linux hands out lazily-zeroed pages: the cost is not absent, it
    // is deferred to the first touch, which lands on the audio thread later.
    // `buffer_load` is the honest shape -- host-decoded frames copied into the
    // engine, which is exactly what the browser path does.
    println!("\nbuffer_load (the copy the browser actually pays):");
    for &(label, frames, ch) in &[
        ("1 s mono", 48_000, 1),
        ("10 s stereo", 480_000, 2),
        ("1 min stereo", 2_880_000, 2),
        ("5 min stereo", 14_400_000, 2),
    ] {
        let mut server = ClaustersHeadless::new(SR, CH, 0.0).unwrap();
        let data = vec![0.5f32; frames * ch];
        let t = Instant::now();
        server.buffer_load(0, ch, SR, &data).unwrap();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let mb = data.len() as f64 * 4.0 / 1_048_576.0;
        println!(
            "  {label:14} {mb:7.1} MB  {ms:8.3} ms  {}",
            if ms > QUANTUM_MS {
                format!("OVER by {:.1}x", ms / QUANTUM_MS)
            } else {
                "fits".into()
            }
        );
    }

    // 2) A burst of small jobs, budgeted vs not.
    println!("\n64 small allocations arriving at once:");
    for &budget in &[usize::MAX, 4] {
        let mut server = ClaustersHeadless::new(SR, CH, 0.0).unwrap();
        server.set_budget(clausters::osc::server::ServeBudget {
            ring_packets: usize::MAX,
            nrt_jobs: budget,
        });
        for i in 0..64 {
            server.send(&msg(
                "/buffer_alloc",
                vec![OscType::Int(i), OscType::Int(48_000), OscType::Int(1)],
            ));
        }
        let mut worst: f64 = 0.0;
        let mut turns = 0;
        loop {
            let t = time_one_block(&mut server);
            worst = worst.max(t);
            turns += 1;
            if server.backlog() == 0 || turns > 200 {
                break;
            }
        }
        let name = if budget == usize::MAX {
            "unbudgeted"
        } else {
            "4 jobs/turn"
        };
        println!(
            "  {name:12} worst turn {worst:7.3} ms over {turns:3} turns  {}",
            if worst > QUANTUM_MS {
                format!("OVER by {:.1}x", worst / QUANTUM_MS)
            } else {
                "fits".into()
            }
        );
    }
}
