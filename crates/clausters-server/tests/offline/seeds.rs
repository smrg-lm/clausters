//! The server's seed: one seed, the engine's generators, and what follows
//! from them -- a render replays its seed, two synths drawing in turn draw
//! different values, `RandSeed` restarts a sequence, `RandID` chooses an
//! independent one, a parallel group draws what a sequential one does, and
//! `/server_seed` in a score reseeds at its time.
//!
//! Whether the values are scsynth's is `sc_reference`'s question; these are
//! about the model the values come from.

#![cfg(feature = "synth")]

use clausters::clausters_core::rng::RGen;
use clausters::server::render::{OscScore, RenderConfig, render_to_vec};
use rosc::{OscMessage, OscType};
use serde_json::{Value, json};

const SR: f64 = 48_000.0;

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

fn def(name: &str, ugens: Value) -> OscMessage {
    let def = json!({"name": name, "ugens": ugens});
    msg(
        "/def_send",
        vec![
            OscType::String("synth".into()),
            OscType::Blob(def.to_string().into_bytes()),
        ],
    )
}

fn synth(name: &str, id: i32, target: i32) -> OscMessage {
    msg(
        "/synth_new",
        vec![
            OscType::String(name.into()),
            OscType::Int(id),
            OscType::Int(1),
            OscType::Int(target),
        ],
    )
}

/// White noise on `bus`, drawing from generator `id` when one is given.
fn noise(name: &str, bus: f32, id: Option<f32>) -> OscMessage {
    let mut ugens = vec![];
    if let Some(id) = id {
        ugens.push(json!({"kind": "RandID", "rate": "ir", "inputs": [{"const": id}]}));
    }
    let n = ugens.len();
    ugens.push(json!({"kind": "WhiteNoise", "inputs": []}));
    ugens.push(json!({"kind": "Out", "inputs": [{"const": bus}, {"ugen": n}]}));
    def(name, Value::Array(ugens))
}

fn render(events: Vec<(f64, Vec<OscMessage>)>, seed: u64, workers: usize) -> Vec<f32> {
    let cfg = RenderConfig {
        sample_rate: SR,
        channels: 2,
        workers,
        seed: Some(seed),
        ..RenderConfig::default()
    };
    render_to_vec(&OscScore::new(events).unwrap(), &cfg)
        .unwrap()
        .0
}

fn left(x: &[f32]) -> Vec<f32> {
    x.iter().step_by(2).copied().collect()
}

/// **A render replays its seed**, `Rand` included: `Rand` once took its seed
/// from a counter of the process, so the value a synth got depended on how
/// many had been built before it, anywhere in the process.
#[test]
fn a_render_replays_its_seed_and_rand_with_it() {
    let rand = def(
        "rand",
        json!([
            {"kind": "Rand", "rate": "ir", "inputs": [{"const": 0.0}, {"const": 1.0}]},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 0}]}
        ]),
    );
    let score = || {
        vec![
            (0.0, vec![rand.clone(), synth("rand", 1000, 0)]),
            (0.01, vec![]),
        ]
    };
    let a = render(score(), 42, 0);
    let b = render(score(), 42, 0);
    let c = render(score(), 43, 0);
    assert_eq!(a, b, "the same seed replays the same take");
    assert_ne!(a[0], c[0], "another seed is another take");
    // The value is generator 0's first draw, as scsynth's `Rand` draws it.
    let want = RGen::new(42).frand();
    assert_eq!(a[0], want);
}

/// Two synths drawing from one generator draw in turn: two runs of one
/// stream, never the same samples.
#[test]
fn two_synths_on_one_generator_draw_in_turn() {
    let events = vec![
        (
            0.0,
            vec![
                noise("n", 0.0, None),
                synth("n", 1000, 0),
                synth("n", 1001, 0),
            ],
        ),
        (0.01, vec![]),
    ];
    let out = left(&render(events, 7, 0));
    // The two sum on bus 0: their streams are consecutive runs of generator
    // 0's, so the first sample is the first draw plus the 65th.
    let mut g = RGen::new(7);
    let first: Vec<f32> = (0..64).map(|_| g.frand2()).collect();
    let second: Vec<f32> = (0..64).map(|_| g.frand2()).collect();
    assert_eq!(out[0], first[0] + second[0]);
}

/// `RandSeed` restarts the generator, so two synths that each reseed it at
/// their start play the same noise.
#[test]
fn rand_seed_restarts_the_sequence() {
    let reseeded = def(
        "r",
        json!([
            {"kind": "RandSeed", "rate": "ir", "inputs": [{"const": 1.0}, {"const": 1234.0}]},
            {"kind": "WhiteNoise", "inputs": []},
            {"kind": "Out", "inputs": [{"const": 0.0}, {"ugen": 1}]}
        ]),
    );
    let one = left(&render(
        vec![
            (0.0, vec![reseeded.clone(), synth("r", 1000, 0)]),
            (0.01, vec![]),
        ],
        1,
        0,
    ));
    let two = left(&render(
        vec![
            (
                0.0,
                vec![reseeded, synth("r", 1000, 0), synth("r", 1001, 0)],
            ),
            (0.01, vec![]),
        ],
        2,
        0,
    ));
    let mut g = RGen::new(1234);
    let want: Vec<f32> = (0..64).map(|_| g.frand2()).collect();
    assert_eq!(
        &one[..64],
        &want[..],
        "the stream of 1234, whatever the server's seed"
    );
    // Each reseeded at its start, so the second summed the same block again.
    let doubled: Vec<f32> = want.iter().map(|x| x + x).collect();
    assert_eq!(&two[..64], &doubled[..]);
}

/// `RandID` chooses generator `id`, seeded with the server's seed plus `id`.
#[test]
fn rand_id_draws_from_its_own_generator() {
    let out = left(&render(
        vec![
            (0.0, vec![noise("n", 0.0, Some(5.0)), synth("n", 1000, 0)]),
            (0.01, vec![]),
        ],
        100,
        0,
    ));
    let mut g = RGen::new(105);
    let want: Vec<f32> = (0..64).map(|_| g.frand2()).collect();
    assert_eq!(&out[..64], &want[..]);
}

/// **A parallel group draws what a sequential one does.** Synths that write
/// different buses would share a stage, and two that draw from one generator
/// in one stage would race for it; the stage scheduler keeps them apart, so a
/// render with workers is bit-identical. Synths on different generators run
/// side by side and are identical too.
#[test]
fn a_parallel_group_draws_what_a_sequential_one_does() {
    let score = || {
        let mut setup = vec![
            noise("a", 0.0, None),
            noise("b", 1.0, None),
            noise("c", 0.0, Some(1.0)),
            noise("d", 1.0, Some(2.0)),
            msg(
                "/group_new",
                vec![OscType::Int(100), OscType::Int(1), OscType::Int(0)],
            ),
            msg("/group_parallel", vec![OscType::Int(100), OscType::Int(1)]),
        ];
        for (k, name) in ["a", "b", "c", "d", "a", "b"].iter().enumerate() {
            setup.push(synth(name, 1000 + k as i32, 100));
        }
        vec![(0.0, setup), (0.2, vec![])]
    };
    let sequential = render(score(), 9, 0);
    for _ in 0..5 {
        assert!(
            render(score(), 9, 3) == sequential,
            "a render with workers drew another take"
        );
    }
}

/// `/server_seed` in a score reseeds at its time: a synth started after it
/// draws the stream of the new seed.
#[test]
fn a_score_reseeds_at_its_time() {
    let at = 512.0 / SR;
    let out = left(&render(
        vec![
            (0.0, vec![noise("n", 0.0, None)]),
            (
                at,
                vec![
                    msg("/server_seed", vec![OscType::String("77".into())]),
                    synth("n", 1000, 0),
                ],
            ),
            (at + 0.01, vec![]),
        ],
        3,
        0,
    ));
    let mut g = RGen::new(77);
    let want: Vec<f32> = (0..64).map(|_| g.frand2()).collect();
    assert_eq!(&out[512..576], &want[..]);
}
