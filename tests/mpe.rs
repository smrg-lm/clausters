//! MPE: a zone's notes are voices of their own, and a member's expression
//! reaches its note only. Driven through the same byte entry the live input
//! uses (`translate_midi_bytes`), over the built-in `default` def, whose
//! `freq` says what pitch a voice is at.

#![cfg(feature = "synth")]

use clausters::midi::convert;
use clausters::osc::translate::CmdTranslator;
use clausters::rosc::{OscMessage, OscType};
use clausters_midi::mpe::{Side, zone_messages};

const SR: f32 = 48_000.0;

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

fn feed(t: &mut CmdTranslator, bytes: &[u8]) {
    let mut cmds = Vec::new();
    t.translate_midi_bytes(bytes, &mut cmds).unwrap();
}

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

/// The voice sounding the newest note, and its `freq`.
fn newest(t: &CmdTranslator) -> i32 {
    t.midi.zone_voices.values().map(|v| v.id).max().unwrap()
}

fn freq(t: &CmdTranslator, id: i32) -> f32 {
    let (_, controls) = t.mirror.synth_info(id).expect("voice mirrored");
    let fi = t.node_defs.get(&id).unwrap().control_index("freq").unwrap();
    controls[fi as usize]
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
fn a_member_bend_moves_its_voice_and_a_master_bend_moves_the_zone() {
    let mut t = zone(15);
    feed(&mut t, &[0x91, 60, 100]);
    let a = newest(&t);
    feed(&mut t, &[0x92, 67, 100]);
    let b = newest(&t);
    // Channel 1 bent a quarter of its 48 semitones up: 12 semitones.
    feed(&mut t, &clausters_midi::mpe::bend_message(1, 12.0, 48.0));
    assert!((freq(&t, a) - convert::midi2freq(72.0)).abs() < 0.5);
    assert!((freq(&t, b) - convert::midi2freq(67.0)).abs() < 1e-2);
    // The master two semitones down: both move, and the two bends sum.
    feed(&mut t, &[0xE0, 0, 0]);
    assert!((freq(&t, a) - convert::midi2freq(70.0)).abs() < 0.5);
    assert!((freq(&t, b) - convert::midi2freq(65.0)).abs() < 1e-2);
    // A note-off ends the voice on its channel.
    feed(&mut t, &[0x81, 60, 0]);
    assert!(t.mirror.synth_info(a).is_none());
    assert!(t.mirror.synth_info(b).is_some());
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
    for (selector, control) in [("cc6", "amp"), ("bend", "pan")] {
        send(
            &mut t,
            "/midi_map",
            vec![
                OscType::Int(8),
                OscType::String(selector.into()),
                OscType::String(control.into()),
            ],
        )
        .unwrap();
    }
    feed(&mut t, &[0x98, 69, 100]);
    let id = *t.midi.voices.get(&(8, 69)).unwrap();
    // CC 6 is an RPN's data inside a zone; on channel 8 it is a control.
    let mut cmds = Vec::new();
    t.translate_midi_bytes(&[0xB8, 6, 127], &mut cmds).unwrap();
    assert_eq!(cmds.len(), 1, "CC 6 reached the binding that maps it");
    // A bend on a plain channel is the mapped control, not pitch.
    feed(&mut t, &[0xE8, 0x7F, 0x7F]);
    assert!((freq(&t, id) - 440.0).abs() < 1e-2);
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
fn a_zone_of_no_members_plays_its_master_as_a_keyboard() {
    let mut t = zone(0);
    feed(&mut t, &[0x90, 69, 100]);
    assert!((freq(&t, newest(&t)) - 440.0).abs() < 1e-2);
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
            OscType::String("cutoff".into()),
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
    assert_eq!(zone.binding.timbre_control.as_deref(), Some("cutoff"));
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
