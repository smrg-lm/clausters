//! **The level of a staff over time**: what its dynamics and hairpins say
//! between the notes, as a curve.
//!
//! [`perform`](super::perform) folds a dynamic and a hairpin into each note's
//! attack, read at its onset -- so a crescendo over a held note does not move
//! inside it, and a sequence rendered from a score would hold velocities and
//! no curve at all. This is the same reading taken **as a function of time**:
//! the dynamic in force, ramped by whatever hairpin is over it, per staff,
//! since that is where both are written. It is what a renderer writes as a
//! lane -- of a voice's channel, or of every channel of the staff when it has
//! several voices -- and it is the level without what is a note's own: the
//! metric stress and the accents stay on the attack.
//!
//! The curve says exactly what the attacks say, at the moments they are read:
//! a step where a dynamic is written, a ramp under a hairpin, and the drop
//! back that [`Hairpin::gain_at`](super::Hairpin) makes after a hairpin that
//! ends on no dynamic.

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;

use super::{Hairpin, Interpretation, dynamic_map, placed_of, prevailing, spanners};
use crate::notation::model::Sheet;
use crate::ratio::Ratio;

/// One break-point of a staff's level.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LevelPoint {
    /// Where, in beats from the start of the score.
    pub t: f64,
    /// The linear amplitude there.
    pub amp: f64,
    /// Whether the stretch that starts here ramps to the next point (under a
    /// hairpin) rather than holding its value until it.
    pub ramps: bool,
}

/// The level of one staff: its points in time order, two at one moment where
/// the level steps.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StaffLevel {
    /// The staff, 0-based from the top.
    pub staff: usize,
    pub points: Vec<LevelPoint>,
}

/// **The level each staff of `sheet` is at, over time**, under `interp` -- one
/// curve for every staff that has a dynamic or a hairpin written on it, and
/// none for a staff that has neither, whose level nothing moves.
///
/// # Errors
/// When a spanner names an item that is not on the sheet, as
/// [`perform`](super::perform) refuses it.
pub fn levels(mut sheet: Sheet, interp: &Interpretation) -> Result<Vec<StaffLevel>, String> {
    sheet.assign_ids();
    let placed = placed_of(&sheet);
    let at: HashMap<u64, usize> = placed
        .iter()
        .enumerate()
        .map(|(i, p)| (p.item.id(), i))
        .collect();
    let dynamics = dynamic_map(&placed, interp);
    let (_, hairpins) = spanners(&sheet, &placed, &at, &dynamics, interp)?;
    let beats = interp.beat_unit as f64;

    let mut out = Vec::new();
    for staff in 0..sheet.staves.len() {
        let written = dynamics.get(staff).map(Vec::as_slice).unwrap_or_default();
        let over: Vec<&Hairpin> = hairpins.iter().filter(|h| h.staff == staff).collect();
        if written.is_empty() && over.is_empty() {
            continue;
        }
        // Every moment the level can change at: where it starts, where a
        // dynamic is written, and the two ends of each hairpin.
        let mut moments: BTreeSet<Ratio> = BTreeSet::from([Ratio::ZERO]);
        moments.extend(written.iter().map(|&(t, _)| t));
        moments.extend(over.iter().flat_map(|h| [h.start, h.end]));
        let moments: Vec<Ratio> = moments.into_iter().collect();

        let base = |t: Ratio| prevailing(&dynamics, staff, t).unwrap_or(interp.amp);
        // as a note attacking at `t` hears it
        let at_moment =
            |t: Ratio| base(t) * over.iter().map(|h| h.gain_at(t, staff)).product::<f64>();
        // and just past it, where a hairpin that ended at `t` has let go
        let past_moment = |t: Ratio| {
            base(t)
                * over
                    .iter()
                    .filter(|h| h.end != t)
                    .map(|h| h.gain_at(t, staff))
                    .product::<f64>()
        };

        let mut points: Vec<LevelPoint> = Vec::new();
        for (i, &t) in moments.iter().enumerate() {
            let ramps = moments
                .get(i + 1)
                .is_some_and(|&next| over.iter().any(|h| h.start <= t && next <= h.end));
            let (here, past) = (at_moment(t), past_moment(t));
            points.push(LevelPoint {
                t: t.to_f64() * beats,
                amp: here,
                ramps: ramps && here == past,
            });
            if here != past {
                points.push(LevelPoint {
                    t: t.to_f64() * beats,
                    amp: past,
                    ramps,
                });
            }
        }
        out.push(StaffLevel { staff, points });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notation::model::{Item, Marks, Pitch, Spanner, Staff, Step, Voice};

    fn note(id: u64, dynamic: Option<&str>) -> Item {
        Item::Note {
            id,
            pitches: vec![Pitch {
                step: Step::C,
                alter: 0,
                octave: 4,
                forced: false,
            }],
            dur: Ratio::new(1, 4),
            tie: false,
            marks: Marks {
                dynamic: dynamic.map(str::to_string),
                ..Marks::default()
            },
        }
    }

    fn sheet(items: Vec<Item>, spanners: Vec<Spanner>) -> Sheet {
        Sheet {
            staves: vec![Staff {
                clef: "G2".into(),
                voices: vec![Voice { items }],
            }],
            spanners,
            ..Sheet::default()
        }
    }

    fn hairpin(kind: &str, from: u64, to: u64) -> Spanner {
        Spanner {
            kind: kind.into(),
            from,
            to,
        }
    }

    #[test]
    fn a_staff_with_nothing_written_has_no_level_and_a_dynamic_is_a_step() {
        let interp = Interpretation::default();
        let plain = sheet((1..=4).map(|id| note(id, None)).collect(), Vec::new());
        assert!(levels(plain, &interp).unwrap().is_empty());

        // p on the first note, f on the third: the unmarked level until the
        // first mark is the mark's own, since it is written at the start
        let marked = sheet(
            vec![
                note(1, Some("p")),
                note(2, None),
                note(3, Some("f")),
                note(4, None),
            ],
            Vec::new(),
        );
        let level = &levels(marked, &interp).unwrap()[0];
        assert_eq!(level.staff, 0);
        let said: Vec<(f64, f64, bool)> =
            level.points.iter().map(|p| (p.t, p.amp, p.ramps)).collect();
        assert_eq!(said, vec![(0.0, 0.05, false), (2.0, 0.17, false)]);
    }

    #[test]
    fn a_hairpin_into_a_dynamic_ramps_to_it() {
        let interp = Interpretation::default();
        let crescendo = sheet(
            vec![
                note(1, Some("p")),
                note(2, None),
                note(3, None),
                note(4, Some("f")),
            ],
            vec![hairpin("crescendo", 1, 4)],
        );
        let level = &levels(crescendo.clone(), &interp).unwrap()[0];
        let said: Vec<(f64, f64, bool)> =
            level.points.iter().map(|p| (p.t, p.amp, p.ramps)).collect();
        assert_eq!(said, vec![(0.0, 0.05, true), (3.0, 0.17, false)]);
        // the curve says what the attacks say, where they are read: the note
        // half way up the ramp is half way between the two levels
        let attacks = super::super::perform(crescendo, &interp).unwrap();
        let half_way = 0.05 + (0.17 - 0.05) * (1.0 / 3.0);
        let second = attacks.iter().find(|n| n.id == 2).unwrap();
        let stress = second.amp / half_way;
        assert!(
            (0.5..2.0).contains(&stress),
            "its level times its metric stress: {stress}"
        );
    }

    #[test]
    fn a_hairpin_that_ends_on_no_dynamic_lets_go_after_it() {
        let interp = Interpretation::default();
        let swell = sheet(
            (1..=4)
                .map(|id| note(id, (id == 1).then_some("mf")))
                .collect(),
            vec![hairpin("crescendo", 1, 3)],
        );
        let level = &levels(swell, &interp).unwrap()[0];
        let said: Vec<(f64, f64, bool)> =
            level.points.iter().map(|p| (p.t, p.amp, p.ramps)).collect();
        let peak = 0.12 * interp.crescendo;
        assert_eq!(said.len(), 3);
        assert_eq!(said[0], (0.0, 0.12, true));
        assert!((said[1].1 - peak).abs() < 1e-12 && said[1].0 == 2.0 && !said[1].2);
        assert_eq!(
            said[2],
            (2.0, 0.12, false),
            "back to the level it started from"
        );
    }
}
