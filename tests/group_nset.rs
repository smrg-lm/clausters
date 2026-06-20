//! scsynth group semantics for `/n_set` and `/n_map`: a command addressed to
//! a **group** transfers the named parameters down its subtree to every
//! synth/faust node that has a control with that name, recursing through
//! subgroups and stopping at each synth. A command addressed to a synth sets
//! only that synth (unchanged). Translator-level: feed OSC, inspect the
//! mirrored node state and the emitted commands.

use clausters::osc::translate::CmdTranslator;
use clausters::rosc::{OscMessage, OscType};
use clausters::server::engine::Cmd;

const SR: f32 = 48_000.0;
const FREQ: usize = 0; // built-in `default` control indices
const AMP: usize = 1;

fn msg(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

fn run(t: &mut CmdTranslator, addr: &str, args: Vec<OscType>) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    t.translate(&msg(addr, args), &mut cmds).unwrap();
    cmds
}

fn s(v: &str) -> OscType {
    OscType::String(v.into())
}

/// freq of a mirrored `default` voice.
fn freq_of(t: &CmdTranslator, id: i32) -> f32 {
    t.mirror.synth_info(id).expect("voice mirrored").1[FREQ]
}

fn new_synth(t: &mut CmdTranslator, id: i32, group: i32) {
    // /s_new default id tail(1) group
    run(
        t,
        "/s_new",
        vec![
            s("default"),
            OscType::Int(id),
            OscType::Int(1),
            OscType::Int(group),
        ],
    );
}

fn new_group(t: &mut CmdTranslator, id: i32, target: i32) {
    // /g_new id head(0) target
    run(
        t,
        "/g_new",
        vec![OscType::Int(id), OscType::Int(0), OscType::Int(target)],
    );
}

#[test]
fn n_set_on_a_group_propagates_to_all_children() {
    let mut t = CmdTranslator::new(SR);
    new_group(&mut t, 10, 0);
    new_synth(&mut t, 101, 10);
    new_synth(&mut t, 102, 10);

    let cmds = run(
        &mut t,
        "/n_set",
        vec![OscType::Int(10), s("freq"), OscType::Float(440.0)],
    );

    // Both children updated, in the mirror and as engine commands.
    assert_eq!(freq_of(&t, 101), 440.0);
    assert_eq!(freq_of(&t, 102), 440.0);
    let sets: Vec<i32> = cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::SetControl { id, index, value } if *index == FREQ as u32 && *value == 440.0 => {
                Some(*id)
            }
            _ => None,
        })
        .collect();
    assert_eq!(sets, vec![101, 102]);
}

#[test]
fn n_set_recurses_through_subgroups_and_stops_at_synths() {
    let mut t = CmdTranslator::new(SR);
    new_group(&mut t, 10, 0);
    new_synth(&mut t, 101, 10);
    new_group(&mut t, 20, 10); // subgroup inside 10
    new_synth(&mut t, 103, 20); // grandchild synth

    run(
        &mut t,
        "/n_set",
        vec![OscType::Int(10), s("freq"), OscType::Float(550.0)],
    );

    assert_eq!(freq_of(&t, 101), 550.0);
    assert_eq!(freq_of(&t, 103), 550.0);
}

#[test]
fn n_set_on_a_synth_sets_only_that_synth() {
    let mut t = CmdTranslator::new(SR);
    new_group(&mut t, 10, 0);
    new_synth(&mut t, 101, 10);
    new_synth(&mut t, 102, 10);

    run(
        &mut t,
        "/n_set",
        vec![OscType::Int(102), s("freq"), OscType::Float(660.0)],
    );

    let default_freq = t.synth_defs.get("default").unwrap().control_defaults[FREQ];
    assert_eq!(freq_of(&t, 101), default_freq); // untouched
    assert_eq!(freq_of(&t, 102), 660.0);
}

#[test]
fn n_set_skips_children_without_that_control() {
    // A name not present on a child is simply not applied there. `default`
    // has no `cutoff`, so the set is a silent no-op (no SetControl emitted).
    let mut t = CmdTranslator::new(SR);
    new_group(&mut t, 10, 0);
    new_synth(&mut t, 101, 10);

    let cmds = run(
        &mut t,
        "/n_set",
        vec![OscType::Int(10), s("cutoff"), OscType::Float(1.0)],
    );
    assert!(!cmds.iter().any(|c| matches!(c, Cmd::SetControl { .. })));
}

#[test]
fn n_set_on_empty_group_is_a_noop_not_an_error() {
    let mut t = CmdTranslator::new(SR);
    new_group(&mut t, 10, 0);
    let cmds = run(
        &mut t,
        "/n_set",
        vec![OscType::Int(10), s("freq"), OscType::Float(440.0)],
    );
    assert!(cmds.is_empty());
}

#[test]
fn n_set_on_unknown_id_fails() {
    let mut t = CmdTranslator::new(SR);
    let mut cmds = Vec::new();
    let err = t.translate(
        &msg(
            "/n_set",
            vec![OscType::Int(999), s("freq"), OscType::Float(440.0)],
        ),
        &mut cmds,
    );
    assert!(err.is_err());
}

#[test]
fn n_map_on_a_group_propagates() {
    // /n_map on a group emits a MapControl per child that has the control.
    let mut t = CmdTranslator::new(SR);
    new_group(&mut t, 10, 0);
    new_synth(&mut t, 101, 10);
    new_synth(&mut t, 102, 10);

    let cmds = run(
        &mut t,
        "/n_map",
        vec![OscType::Int(10), s("amp"), OscType::Int(7)],
    );
    let mapped: Vec<i32> = cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::MapControl {
                id,
                index,
                bus,
                audio,
            } if *index == AMP as u32 && *bus == 7 && !*audio => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(mapped, vec![101, 102]);
}
