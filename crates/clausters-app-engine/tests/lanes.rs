//! **A lane on an engine the application holds**: what a locate does to the
//! notes a transport plays as they are fed from this side.

#![cfg(feature = "synth")]

use std::sync::{Arc, Mutex};

use clausters::server::engine::{BLOCK_SIZE, Cmd, Engine, engine_pair};
use clausters::synthdef::SynthDefSpec;
use clausters_app_engine::AppEngine;

const LANE: i32 = 1;

fn blocks(engine: &mut Engine, app: &Mutex<AppEngine>, n: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut left = Vec::new();
    for _ in 0..n {
        engine.process_block(&mut out);
        app.lock().unwrap().pump();
        left.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
    }
    left
}

/// **A rewind during play sounds the note it lands on**: the position the
/// engine publishes is still the old one when the locate is sent, so a lane fed
/// after it was fed from there, and from past the place on the next turn.
#[test]
fn a_locate_back_while_it_rolls_sounds_the_note_it_lands_on() {
    let (mut engine, handle) = engine_pair(48_000.0, 2);
    let app = Arc::new(Mutex::new(AppEngine::over(handle)));
    {
        let mut held = app.lock().unwrap();
        let dc = r#"{"name": "dc", "controls": [{"name": "level", "default": 0.5}],
            "ugens": [{"kind": "Out", "inputs": [{"const": 0.0}, {"control": 0}]}]}"#;
        let spec: SynthDefSpec = serde_json::from_str(dc).unwrap();
        held.translator().define_synth(spec).unwrap();
        held.lane_new(LANE, 0, 0);
        held.lane_set(
            LANE,
            r#"{"notes": [[0, 3200, "dc", {"level": 0.5}, "free"],
                          [20000, 21000, "dc", {"level": 0.5}, "free"]]}"#,
        );
        assert!(held.push(vec![Cmd::TransportRun {
            transport: 0,
            rolling: true,
        }]));
    }
    let before = blocks(&mut engine, &app, 100);
    assert!(before.iter().any(|&s| s != 0.0), "the first note sounded");
    assert_eq!(*before.last().unwrap(), 0.0, "and is over, at 12800");
    assert!(app.lock().unwrap().push(vec![Cmd::TransportLocate {
        transport: 0,
        position: 0,
    }]));
    let after = blocks(&mut engine, &app, 10);
    assert_ne!(after[0], 0.0, "it sounds from where the locate lands");
}
