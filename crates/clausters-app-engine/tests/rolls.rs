//! **Two rolls over one sequence, played on the engine the application
//! holds**: each roll has a playback of its own, and each sounds the whole
//! sequence, its curves with it.

#![cfg(feature = "synth")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use clausters::server::engine::{BLOCK_SIZE, engine_pair};
use clausters_app_engine::{AppEngine, EngineMedium};
use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
use clausters_document::events::{Event, EventSequence};
use clausters_editing::notes_playback::NotesPlayback;
use serde_json::json;

const SR: f64 = 48_000.0;

/// The left side of `n` blocks, the application's turn after each.
fn blocks(engine: &mut clausters::server::engine::Engine, app: &Mutex<AppEngine>, n: usize) -> f32 {
    let mut out = vec![0.0f32; BLOCK_SIZE * 2];
    let mut peak = 0.0f32;
    for _ in 0..n {
        engine.process_block(&mut out);
        app.lock().unwrap().pump();
        for f in out.as_chunks::<2>().0 {
            peak = peak.max(f[0].abs());
        }
        std::thread::sleep(Duration::from_micros(300));
    }
    peak
}

fn sequence(curve: bool) -> EventSequence {
    let mut s = EventSequence::new(vec![
        Event::new(0.0, json!({"midinote": 60, "dur": 1.0, "amp": 0.2})),
        Event::new(1.0, json!({"midinote": 64, "dur": 1.0, "amp": 0.2})),
    ]);
    if curve {
        use clausters_document::events::EventsIntent;
        use clausters_document::multitrack::Automation;
        use clausters_document::{Beat, NodeId, Opaque, Point};
        let mut automation =
            Automation::new(NodeId(0), Opaque(json!({"control": "amp", "channel": 0})));
        automation.points = [(0.0, 0.2), (2.0, 0.4)]
            .map(|(at, value)| Point {
                at: Beat(at),
                value,
                data: Opaque::default(),
            })
            .to_vec();
        s.edit(EventsIntent::Automation { automation }).unwrap();
    }
    s
}

#[test]
fn two_rolls_over_one_sequence_each_sound_it() {
    for curve in [false, true] {
        let s = sequence(curve);
        let (mut engine, handle) = engine_pair(SR as f32, 2);
        let app = Arc::new(Mutex::new(AppEngine::over(handle)));
        let mut ids = IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE);
        let mut first = NotesPlayback::in_medium(EngineMedium::new(Arc::clone(&app)));
        let mut second = NotesPlayback::in_medium(EngineMedium::new(Arc::clone(&app)));
        first.alloc_transport(&mut ids).unwrap();
        second.alloc_transport(&mut ids).unwrap();
        first.play(&s, 0.0, SR, &mut ids).unwrap();
        let heard_first = blocks(&mut engine, &app, 300);
        first.stop(&s, 0.0, SR);
        blocks(&mut engine, &app, 300);
        second.play(&s, 0.0, SR, &mut ids).unwrap();
        let heard_second = blocks(&mut engine, &app, 300);
        assert!(heard_first > 0.01, "curve {curve}: the first roll is heard");
        assert!(
            heard_second > 0.01,
            "curve {curve}: the second roll is heard"
        );
    }
}
