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

/// **A roll's pass that ends on its own is taken in**, as the audio
/// editor's is: with its end switch on, the pass stops where the last note
/// ends, the roll hears it on the application's turn with nothing asked of
/// it, and the cursor sent back to the start then leaves it stopped -- it
/// does not play the roll again from there. Switched off, a pass rolls on
/// past the notes, as a multitrack's does.
#[test]
fn a_roll_s_pass_that_ends_on_its_own_is_taken_in() {
    use clausters_app_engine::app::App;
    use clausters_apps::editing::Editing;

    let (mut engine, handle) = engine_pair(SR as f32, 2);
    let mut app = App::new(Arc::new(Mutex::new(AppEngine::over(handle))));
    let app_engine = Arc::clone(app.engine());
    let mut editing = Editing::new();
    app.hold(&mut editing);
    let opened = app.call(
        &mut editing,
        &json!({"verb": "openNotes", "key": "roll:ends"}),
    );
    let member = opened["member"].as_u64().expect("a notes member") as u32;
    let act = |app: &mut App, editing: &mut Editing, action: serde_json::Value| {
        let ran = app.act(editing, member, action.clone(), 1, None);
        assert!(
            ran.end.as_ref().is_some_and(Result::is_ok),
            "{action}: {:?}",
            ran.end
        );
    };
    act(&mut app, &mut editing, json!({"verb": "open"}));
    act(
        &mut app,
        &mut editing,
        json!({"verb": "handle", "call": {"verb": "loadrows",
            "keys": ["beat", "midinote", "sustain"], "rows": [[0, 72, 0.25]]}}),
    );
    let rolling = |app: &mut App, editing: &mut Editing| {
        app.query(editing, member, "transport")["rolling"].clone()
    };
    let ends = |app: &mut App, editing: &mut Editing| {
        app.call(editing, &json!({"verb": "actions", "member": member}))["stop_at_end"]["checked"]
            .clone()
    };
    // **a roll rolls on past its notes** until its end switch is turned
    assert_eq!(ends(&mut app, &mut editing), false);
    act(&mut app, &mut editing, json!({"verb": "play", "beat": 0.0}));
    for _ in 0..200 {
        blocks(&mut engine, &app_engine, 1);
        assert!(app.landed(&mut editing).is_empty(), "nothing ends");
    }
    assert_eq!(rolling(&mut app, &mut editing), true, "open: it rolls on");
    act(&mut app, &mut editing, json!({"verb": "stop", "mark": 0.0}));
    act(
        &mut app,
        &mut editing,
        json!({"verb": "end", "end": "contents"}),
    );
    assert_eq!(ends(&mut app, &mut editing), true);
    act(&mut app, &mut editing, json!({"verb": "play", "beat": 0.0}));
    let mut heard_at = None;
    for block in 0..400 {
        blocks(&mut engine, &app_engine, 1);
        let ran: Vec<_> = app
            .landed(&mut editing)
            .into_iter()
            .filter(|ran| ran.member == member)
            .collect();
        if !ran.is_empty() {
            assert!(heard_at.is_none(), "heard twice");
            heard_at = Some(block);
        }
    }
    assert!(heard_at.is_some_and(|b| b > 10), "heard at {heard_at:?}");
    assert_eq!(rolling(&mut app, &mut editing), false, "taken in");
    // back to the start: the transport stands there, and nothing plays --
    // once the note's release has rung out
    blocks(&mut engine, &app_engine, 200);
    act(&mut app, &mut editing, json!({"verb": "locate", "at": 0.0}));
    let peak = blocks(&mut engine, &app_engine, 200);
    assert_eq!(rolling(&mut app, &mut editing), false, "still stopped");
    assert!(peak < 1e-4, "nothing plays again: {peak}");
}
