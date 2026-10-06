//! What a `table`'s rows cost as they ride the def, and where the line is.
//!
//! A table's rows are a prop: they travel in the `/gui_def` that makes the
//! table and in every `/gui_set rows` after it, as JSON in one OSC argument.
//! That is right for the lists an interface shows and was never measured for
//! the ones it does not -- "a list of tens of thousands of rows wants the bulk
//! path, and which size is the line is not decided". This is the measurement
//! the line was drawn from: what a table of N rows weighs on the wire, what
//! defining it and replacing its rows costs the host, and against what.
//!
//! Three costs meet a different limit each:
//!
//! - **The wire.** A datagram carries 64 KiB; a stream frame 16 MiB by
//!   default. The first is passed at about a thousand rows, which is why a
//!   client's host connects over TCP.
//! - **A set.** The host parses the JSON, keeps it in the registry (what a
//!   `/gui_query` answers from) and builds the typed rows; a frame is 33 ms.
//! - **A frame.** A draw walks every row once to find the ones a fold hides
//!   and draws the ones on screen, so it grows with the rows and not with the
//!   window.
//!
//! Run with `cargo test --release --test table_rows_cost -- --nocapture
//! --ignored`. The timings need `--release` to mean anything.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Instant;

use clausters_core::osc::{OscMessage, OscPacket, OscType};
use clausters_gui::host::graphics::table as table_rows;
use clausters_gui::host::{ClientId, Host};

/// A window of one table of `n` rows, three cells each, as a def's JSON.
fn def_of(n: usize) -> String {
    format!(
        r#"{{"type":"window","children":[{{"id":2,"type":"table","columns":["name","kind","size"],"rows":{}}}]}}"#,
        rows_of(n)
    )
}

/// `n` rows as the JSON a `rows` prop carries: what a file list looks like.
fn rows_of(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| format!(r#"["take-{i:05}.wav","audio","{} kB"]"#, 100 + i % 900))
        .collect();
    format!("[{}]", rows.join(","))
}

fn from() -> ClientId {
    ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 9000)))
}

fn message(addr: &str, args: Vec<OscType>) -> OscPacket {
    OscPacket::Message(OscMessage {
        addr: addr.into(),
        args,
    })
}

/// The median of `runs` timings of `f`, in milliseconds.
fn median_ms(runs: usize, mut f: impl FnMut()) -> f64 {
    let mut took: Vec<f64> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    took.sort_by(f64::total_cmp);
    took[took.len() / 2]
}

/// **The line is where a set costs a frame.** Below
/// [`table_rows::WIRE_ROWS`] a table's rows are replaced inside one frame of
/// the host, on the wire they have always ridden; this prints the three
/// costs either side of it, and checks the two facts the line rests on.
#[test]
#[ignore = "timing: only meaningful under --release"]
fn what_a_tables_rows_cost_by_their_number() {
    println!("rows      wire        def      set rows");
    for n in [100, 1_000, 20_000, table_rows::WIRE_ROWS, 100_000] {
        let def = def_of(n);
        let rows = rows_of(n);
        let defined = median_ms(5, || {
            let mut host = Host::new();
            host.handle_packet(
                message(
                    "/gui_def",
                    vec![OscType::Int(1), OscType::String(def.clone())],
                ),
                from(),
            );
        });
        let mut host = Host::new();
        host.handle_packet(
            message(
                "/gui_def",
                vec![OscType::Int(1), OscType::String(def.clone())],
            ),
            from(),
        );
        let set = median_ms(5, || {
            host.handle_packet(
                message(
                    "/gui_set",
                    vec![
                        OscType::Int(2),
                        OscType::String("rows".into()),
                        OscType::String(rows.clone()),
                    ],
                ),
                from(),
            );
        });
        println!(
            "{n:>6}  {:>7.1} kB  {defined:>7.2} ms  {set:>7.2} ms",
            rows.len() as f64 / 1024.0
        );
        if n == table_rows::WIRE_ROWS {
            assert!(set < 33.0, "at the line a set is inside a frame: {set} ms");
            assert!(
                rows.len() < 16 << 20,
                "and inside a stream frame: {} bytes",
                rows.len()
            );
        }
    }
}
