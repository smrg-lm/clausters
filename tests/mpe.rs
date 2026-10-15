//! MPE: a zone's notes each have their own identity, and a member's
//! expression reaches its note only. Driven through the same byte entry the
//! live input uses (`translate_midi_bytes`); a zone bound to the built-in
//! `default` writes its notes on the bus its `Voices` node reads, so what a
//! note does is read off the events it writes.

#![cfg(feature = "synth")]

use clausters::osc::translate::CmdTranslator;
use clausters::rosc::{OscMessage, OscType};
use clausters::server::engine::Cmd;
use clausters_core::event_bus::{BusEvent, Expression, Kind};
use clausters_midi::mpe::{Side, zone_messages};

fn send(t: &mut CmdTranslator, addr: &str, args: Vec<OscType>) -> Result<(), String> {
    let mut cmds = Vec::new();
    t.translate(
        &OscMessage {
            addr: addr.into(),
            args,
        },
        &mut cmds,
    )
}

/// The events `bytes` write, as the live input would feed them.
fn feed(t: &mut CmdTranslator, bytes: &[u8]) -> Vec<BusEvent> {
    let mut cmds = Vec::new();
    t.translate_midi_bytes(bytes, &mut cmds).unwrap();
    cmds.iter()
        .filter_map(|c| match c {
            Cmd::BusEvent { event, .. } => Some(*event),
            _ => None,
        })
        .collect()
}

const SR: f32 = 48_000.0;

/// A lower zone of `members` playing `default`.
fn zone(members: i32) -> CmdTranslator {
    let mut t = CmdTranslator::new(SR);
    send(
        &mut t,
        "/midi_bindZone",
        vec![
            OscType::Int(0),
            OscType::Int(members),
            OscType::String("default".into()),
        ],
    )
    .unwrap();
    t
}

/// The note-on among `events`, and its id.
fn note_on(events: &[BusEvent]) -> BusEvent {
    *events
        .iter()
        .find(|e| e.kind == Kind::NoteOn)
        .expect("a note-on")
}

/// The value of expression `which` on note `id` among `events`.
fn expression(events: &[BusEvent], id: i32, which: Expression) -> Option<f64> {
    events
        .iter()
        .find(|e| e.kind == Kind::Expression && e.note_id == id && e.index == which as u32)
        .map(|e| e.value)
}

#[test]
fn a_zone_declared_by_its_device_is_readable_back() {
    let mut t = zone(0);
    for msg in zone_messages(Side::Lower, 5) {
        feed(&mut t, &msg);
    }
    assert_eq!(t.midi.decoder.zone(Side::Lower).map(|z| z.members), Some(5));
}

#[test]
fn a_member_bend_moves_its_note_and_a_master_bend_moves_the_zone() {
    let mut t = zone(15);
    let a = note_on(&feed(&mut t, &[0x91, 60, 100]));
    let b = note_on(&feed(&mut t, &[0x92, 67, 100]));
    assert_ne!(a.note_id, b.note_id, "each note its own identity");
    assert_eq!((a.key, a.channel), (60, 1));
    // Channel 1 bent a quarter of its 48 semitones up: 12 semitones, on its
    // note alone.
    let bent = feed(&mut t, &clausters_midi::mpe::bend_message(1, 12.0, 48.0));
    assert!((expression(&bent, a.note_id, Expression::Tuning).unwrap() - 12.0).abs() < 0.05);
    assert_eq!(expression(&bent, b.note_id, Expression::Tuning), None);
    // The master two semitones down: both move, and the two bends sum.
    let master = feed(&mut t, &[0xE0, 0, 0]);
    assert!((expression(&master, a.note_id, Expression::Tuning).unwrap() - 10.0).abs() < 0.05);
    assert!((expression(&master, b.note_id, Expression::Tuning).unwrap() + 2.0).abs() < 0.05);
    // A note-off ends the note on its channel.
    let off = feed(&mut t, &[0x81, 60, 0]);
    assert!(matches!(off[..], [e] if e.kind == Kind::NoteOff && e.note_id == a.note_id));
}

#[test]
fn a_plain_channel_beside_a_zone_keeps_its_controls_and_its_bend() {
    let mut t = zone(3);
    send(
        &mut t,
        "/midi_bind",
        vec![OscType::Int(8), OscType::String("default".into())],
    )
    .unwrap();
    feed(&mut t, &[0x98, 69, 100]);
    // CC 6 is an RPN's data inside a zone; on channel 8 it is a controller.
    let cc = feed(&mut t, &[0xB8, 6, 127]);
    assert!(matches!(cc[..], [e] if e.kind == Kind::Controller && e.index == 6 && e.channel == 8));
    // A bend on a plain channel is the channel's bend, not a note's pitch.
    let bend = feed(&mut t, &[0xE8, 0x7F, 0x7F]);
    assert!(matches!(bend[..], [e] if e.kind == Kind::Bend && e.channel == 8));
}

#[test]
fn a_zone_and_a_binding_over_one_channel_are_refused() {
    let mut t = zone(4);
    assert!(
        send(
            &mut t,
            "/midi_bind",
            vec![OscType::Int(2), OscType::String("default".into())]
        )
        .is_err()
    );
    let mut t = CmdTranslator::new(SR);
    send(
        &mut t,
        "/midi_bind",
        vec![OscType::Int(6), OscType::String("default".into())],
    )
    .unwrap();
    assert!(
        send(
            &mut t,
            "/midi_bindZone",
            vec![
                OscType::Int(0),
                OscType::Int(8),
                OscType::String("default".into())
            ]
        )
        .is_err()
    );
    // The device may not widen a zone over the bound channel either.
    send(
        &mut t,
        "/midi_bindZone",
        vec![
            OscType::Int(0),
            OscType::Int(3),
            OscType::String("default".into()),
        ],
    )
    .unwrap();
    for msg in zone_messages(Side::Lower, 10) {
        feed(&mut t, &msg);
    }
    assert_eq!(t.midi.decoder.zone(Side::Lower).map(|z| z.members), Some(3));
}

#[test]
fn a_member_pressure_and_timbre_are_its_notes_expression() {
    let mut t = zone(7);
    let started = feed(&mut t, &[0x91, 69, 100]);
    let id = note_on(&started).note_id;
    assert_eq!(expression(&started, id, Expression::Pressure), Some(0.0));
    assert!(
        (expression(&started, id, Expression::Brightness).unwrap() - 64.0 / 127.0).abs() < 1e-6,
        "centred (CC 74 at 64) until the controller moves it"
    );
    let pressed = feed(&mut t, &[0xD1, 127]);
    assert!((expression(&pressed, id, Expression::Pressure).unwrap() - 1.0).abs() < 1e-3);
    let dark = feed(&mut t, &[0xB1, 74, 0]);
    assert!(expression(&dark, id, Expression::Brightness).unwrap().abs() < 1e-3);
}

#[test]
fn a_zone_of_no_members_plays_its_master_as_a_keyboard() {
    let mut t = zone(0);
    assert_eq!(note_on(&feed(&mut t, &[0x90, 69, 100])).key, 69);
}

#[test]
fn a_zone_persists_and_restores() {
    let mut a = zone(7);
    send(
        &mut a,
        "/midi_map",
        vec![
            OscType::Int(0),
            OscType::String("timbre".into()),
            OscType::String("pan".into()),
            OscType::Int(71),
        ],
    )
    .unwrap();
    let persisted = a.midi.persist();
    let json = serde_json::to_string(&persisted).unwrap();
    let back: Vec<clausters::midi::PersistedBinding> = serde_json::from_str(&json).unwrap();
    let mut b = CmdTranslator::new(SR);
    let mut cmds = Vec::new();
    for pb in back {
        b.restore_binding(pb, &mut cmds).unwrap();
    }
    let zone = b.midi.zones.get(&0).expect("zone restored");
    assert_eq!(zone.config.members, 7);
    assert_eq!(zone.config.timbre_cc, 71);
    assert_eq!(zone.binding.timbre_control.as_deref(), Some("pan"));
    assert_eq!(
        b.midi.decoder.zone(Side::Lower).map(|z| z.timbre_cc),
        Some(71)
    );
}

#[test]
fn a_file_written_before_zones_still_loads() {
    let old = r#"[{"channel": 2, "binding": {"instrument": "default", "target": 0,
        "action": 0, "gate": false, "freq_control": "freq", "amp_control": "amp",
        "gate_control": "gate", "bend_control": null, "pressure_control": null,
        "poly_control": null, "cc": {}, "programs": {}}}]"#;
    let back: Vec<clausters::midi::PersistedBinding> = serde_json::from_str(old).unwrap();
    assert!(back[0].zone.is_none());
}
