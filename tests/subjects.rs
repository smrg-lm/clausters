//! The subject table itself, contrasted against the catalog it describes.
//!
//! `tests/common/subjects.json` names the UGens the U suites test and the ear
//! auditions. It is hand-written, so it can drift from the registry — a renamed
//! row, an input added to a family, a milestone that grew a kind and did not
//! grow a subject. These tests are what makes drifting fail loudly instead of
//! quietly shrinking the coverage the other suites believe they have.
//!
//! Three claims: the table describes rows that exist, with the arity they
//! really have; every row of U1–U8 that a suite could test *has* a subject; and
//! every subject actually renders. The rules that measure a signal live in the
//! milestone suites — this file only guards the declaration.

#![cfg(feature = "synth")]

#[path = "common/bench.rs"]
mod bench;

use std::collections::HashSet;

use bench::*;
use clausters::dsp::registry::{Arity, lookup};

/// The milestones the table covers, in track order.
const MILESTONES: [&str; 8] = ["U1", "U2", "U3", "U4", "U5", "U6", "U7", "U8"];

#[test]
fn every_subject_names_a_row_that_exists_with_the_arity_it_has() {
    // Collected rather than asserted one at a time: when a family gains an
    // input, every row of it is wrong at once, and reading that as one list
    // beats rediscovering it a `cargo test` at a time.
    let mut wrong = Vec::new();
    for (milestone, s) in all_subjects() {
        // A multi-channel family is one logical UGen per channel, each row
        // carrying a trailing channel index the bench appends -- so the
        // declared inputs are the row's minus that one.
        let declared = s.inputs.len() + usize::from(s.channels > 1);
        check(&mut wrong, &milestone, &s.name, &s.kind, declared);
        // The prelude rows are real UGens too -- a demand source feeding a
        // driver -- and go stale the same way.
        for row in &s.prelude {
            let kind = row["kind"].as_str().unwrap_or_default();
            let n = row["inputs"].as_array().map_or(0, |a| a.len());
            check(&mut wrong, &milestone, &s.name, kind, n);
        }
    }

    fn check(wrong: &mut Vec<String>, milestone: &str, name: &str, kind: &str, declared: usize) {
        let Some(d) = lookup(kind) else {
            wrong.push(format!("{milestone}/{name}: no catalog row {kind:?}"));
            return;
        };
        let names: Vec<&str> = d.inputs.iter().map(|i| i.name).collect();
        match d.arity {
            Arity::Fixed(n) if declared != n => wrong.push(format!(
                "{milestone}/{name}: {kind} takes {n} inputs {names:?}, the \
                 subject declares {declared}"
            )),
            Arity::Variadic if declared < d.inputs.len() => wrong.push(format!(
                "{milestone}/{name}: {kind} needs at least its fixed head \
                 {names:?}, the subject declares {declared}"
            )),
            _ => {}
        }
    }
    assert!(
        wrong.is_empty(),
        "subjects.json is stale:\n  {}",
        wrong.join("\n  ")
    );
}

#[test]
fn subject_names_are_unique_and_the_handle_the_ear_uses() {
    let mut seen = HashSet::new();
    for (milestone, s) in all_subjects() {
        assert!(
            seen.insert(s.name.clone()),
            "{milestone}: the handle {:?} is used twice; `audition.py <name>` \
             would be ambiguous",
            s.name
        );
        assert!(
            s.name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "{milestone}/{}: a handle is lowercase and digits, so it can be \
             typed without quoting",
            s.name
        );
    }
}

#[test]
fn every_milestone_is_present_and_populated() {
    for m in MILESTONES {
        assert!(
            !subjects(m).is_empty(),
            "{m} has no subjects: the generic rules would silently pass"
        );
    }
}

#[test]
fn every_subject_renders_finite_output() {
    // Four blocks is enough for a NaN from a bad build to appear; the long-run
    // rule is separately driven per milestone.
    for m in MILESTONES {
        assert_renders_finite(m, BLOCK * 4);
    }
}

#[test]
fn every_stateful_subject_survives_a_split_block() {
    // Rule 5, over the whole table at once. A UGen that recomputes something
    // per call rather than per block shows up here and almost nowhere else.
    //
    // The stochastic rows are not skipped for convenience: through the def path
    // their two renders are two instances with two seeds, so there is nothing
    // to compare. `tests/noise.rs` owns their split, seeded, at the struct
    // level -- see `assert_split_agrees` for why the wire cannot pin a seed.
    for m in MILESTONES {
        for s in subjects(m) {
            if s.has("stateful") && !s.has("stochastic") {
                assert_split_agrees(&s, BLOCK * 8, 21);
            }
        }
    }
}

/// `clausters::dsp::BLOCK_SIZE`, spelled once.
const BLOCK: usize = clausters::dsp::BLOCK_SIZE;
