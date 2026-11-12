//! One dry/convolved bus graph shared by the runnable example and assertions.
use clausters::rosc::{OscMessage, OscType};
use clausters::server::render::OscScore;
use serde_json::json;

pub const RATE: f64 = 48000.0;
pub const FRAMES: usize = 2048;

fn message(address: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: address.into(),
        args,
    }
}
fn ints(address: &str, args: &[i32]) -> OscMessage {
    message(address, args.iter().copied().map(OscType::Int).collect())
}
pub fn graph(automatic: bool) -> Vec<OscMessage> {
    let defs = [
        json!({"name":"impulse_source", "ugens":[
            {"kind":"Impulse", "inputs":[{"const":0.0}]},
            {"kind":"Out", "inputs":[{"const":4.0},{"ugen":0}]}]}),
        json!({"name":"dry", "ugens":[
            {"kind":"In", "inputs":[{"const":4.0}]},
            {"kind":"Out", "inputs":[{"const":0.0},{"ugen":0}]}]}),
        json!({"name":"convolved", "ugens":[
            {"kind":"In", "inputs":[{"const":4.0}]},
            {"kind":"Conv", "inputs":[{"ugen":0},{"const":1.0}], "fft_size":512, "partitions":1},
            {"kind":"Out", "inputs":[{"const":0.0},{"ugen":1}]}]}),
    ];
    let mut messages: Vec<_> = defs
        .into_iter()
        .map(|def| {
            message(
                "/def_send",
                vec![
                    OscType::String("synth".into()),
                    OscType::Blob(serde_json::to_vec(&def).unwrap()),
                ],
            )
        })
        .collect();
    messages.push(ints("/group_new", &[1, 1, 0]));
    messages.push(ints("/group_new", &[2, 1, 1]));
    messages.push(ints("/group_new", &[3, 1, 1]));
    for group in [1, 2, 3] {
        messages.push(ints("/group_sortMode", &[group, i32::from(automatic)]));
        messages.push(ints("/group_parallel", &[group, 1]));
    }
    for (name, id, group) in [
        ("impulse_source", 10, 2),
        ("dry", 11, 3),
        ("convolved", 12, 3),
    ] {
        messages.push(message(
            "/synth_new",
            vec![
                OscType::String(name.into()),
                OscType::Int(id),
                OscType::Int(1),
                OscType::Int(group),
            ],
        ));
    }
    messages
}
pub fn score(automatic: bool) -> OscScore {
    let mut setup = vec![
        ints("/buffer_alloc", &[0, 1, 1]),
        message(
            "/buffer_set",
            vec![OscType::Int(0), OscType::Int(0), OscType::Float(1.0)],
        ),
        ints("/buffer_alloc", &[1, 514, 1]),
        message(
            "/buffer_gen",
            vec![
                OscType::Int(1),
                OscType::String("prepare_partconv".into()),
                OscType::Int(512),
                OscType::Int(0),
            ],
        ),
    ];
    setup.extend(graph(automatic));
    OscScore::new([
        (0.0, setup),
        (FRAMES as f64 / RATE, vec![ints("/node_free", &[1])]),
    ])
    .unwrap()
}
