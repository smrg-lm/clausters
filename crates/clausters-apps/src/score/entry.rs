//! **Note entry**: where the edit cursor stands, how the keys move it, and
//! which element of the page shows it.
//!
//! Note entry is a mode. Outside it a press selects and a drag moves; inside
//! it the window has an **edit cursor** -- a staff, a voice and a time -- and
//! what is entered is written *there*: a letter writes its pitch, of the value
//! in hand, over the stretch it covers (`notation::Op::Enter`), and the cursor
//! goes on by that value. Nothing after it moves. These are the pure parts of
//! that: the cursor's arithmetic over the model, with no window and no
//! engraver.

use clausters_core::notation::{Item, Pitch, Sheet, Step};
use clausters_core::ratio::Ratio;

/// **Where the edit cursor stands**: a staff and a voice of it, from zero,
/// and a time in whole notes from the start of the score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub staff: usize,
    pub voice: usize,
    pub at: Ratio,
}

/// The first beat of the score, on the top staff's first voice.
impl Default for Place {
    fn default() -> Self {
        Self {
            staff: 0,
            voice: 0,
            at: Ratio::ZERO,
        }
    }
}

/// How many voices a staff takes during entry: the four a voice key names.
pub const VOICES: usize = 4;

/// The items of one voice, each with its onset: empty for a voice the staff
/// does not have.
pub fn onsets(sheet: &Sheet, staff: usize, voice: usize) -> Vec<(Ratio, &Item)> {
    let mut onset = Ratio::ZERO;
    sheet
        .staves
        .get(staff)
        .and_then(|s| s.voices.get(voice))
        .map(|v| {
            v.items
                .iter()
                .map(|item| {
                    let at = onset;
                    onset = onset + item.dur();
                    (at, item)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The item of `place`'s voice that **starts** at its time, if one does.
pub fn item_at(sheet: &Sheet, place: Place) -> Option<&Item> {
    onsets(sheet, place.staff, place.voice)
        .into_iter()
        .find(|(at, _)| *at == place.at)
        .map(|(_, item)| item)
}

/// The cursor **one item back**: to where the item before it in its voice
/// starts, or by `value` where the voice holds nothing before it -- never
/// before the start.
pub fn back(sheet: &Sheet, place: Place, value: Ratio) -> Place {
    let before = onsets(sheet, place.staff, place.voice)
        .into_iter()
        .map(|(at, _)| at)
        .filter(|at| *at < place.at)
        .max();
    let at = before.unwrap_or_else(|| (place.at - value).max(Ratio::ZERO));
    Place { at, ..place }
}

/// The cursor **one item on**: to where the next item of its voice starts,
/// or the voice's end, or -- past it -- by `value`.
pub fn on(sheet: &Sheet, place: Place, value: Ratio) -> Place {
    let items = onsets(sheet, place.staff, place.voice);
    let end = items
        .last()
        .map(|(at, item)| *at + item.dur())
        .unwrap_or(Ratio::ZERO);
    let next = items
        .iter()
        .map(|(at, _)| *at)
        .chain([end])
        .filter(|at| *at > place.at)
        .min();
    Place {
        at: next.unwrap_or(place.at + value),
        ..place
    }
}

/// The cursor **a bar back**: to the start of its own measure, or of the one
/// before when it already stands there.
pub fn bar_back(sheet: &Sheet, place: Place) -> Place {
    let (measure, into) = sheet.grid.position(place.at);
    let to = if into.is_positive() || measure == 0 {
        measure
    } else {
        measure - 1
    };
    Place {
        at: sheet.grid.measure_start(to),
        ..place
    }
}

/// The cursor **a bar on**: to the start of the next measure.
pub fn bar_on(sheet: &Sheet, place: Place) -> Place {
    let (measure, _) = sheet.grid.position(place.at);
    Place {
        at: sheet.grid.measure_start(measure + 1),
        ..place
    }
}

/// The cursor on the staff `by` above (negative) or below, kept on the score.
pub fn staff_by(sheet: &Sheet, place: Place, by: i32) -> Place {
    let last = sheet.staves.len().saturating_sub(1) as i32;
    Place {
        staff: (place.staff as i32 + by).clamp(0, last) as usize,
        ..place
    }
}

/// **The pitch a letter writes next**: in the octave nearest the note before
/// the cursor in its voice -- the last one entered, when there is one.
pub fn near(sheet: &Sheet, place: Place, entered: Option<&Item>) -> Option<Pitch> {
    if let Some(pitch) = entered.and_then(|item| item.pitches().first()) {
        return Some(*pitch);
    }
    onsets(sheet, place.staff, place.voice)
        .into_iter()
        .rfind(|(at, item)| *at < place.at && item.sounds())
        .and_then(|(_, item)| item.pitches().first().copied())
}

/// The step a pitch key names: `pitch_c` is C.
pub fn step_of(letter: &str) -> Option<Step> {
    Some(match letter {
        "a" => Step::A,
        "b" => Step::B,
        "c" => Step::C,
        "d" => Step::D,
        "e" => Step::E,
        "f" => Step::F,
        "g" => Step::G,
        _ => return None,
    })
}

/// The written value a value key names.
pub fn value_of(name: &str) -> Option<Ratio> {
    Some(match name {
        "whole" => Ratio::ONE,
        "half" => Ratio::new(1, 2),
        "quarter" => Ratio::new(1, 4),
        "eighth" => Ratio::new(1, 8),
        "16th" => Ratio::new(1, 16),
        "32nd" => Ratio::new(1, 32),
        "64th" => Ratio::new(1, 64),
        _ => return None,
    })
}

/// **Which item shows the cursor, and whether it stands past it**: the item
/// of its voice that starts at its time; else any item of its staff, then of
/// any staff, that starts there -- the column is a time, and every voice and
/// staff draws the same one; else the item of its voice it stands inside;
/// else, past what is written, the last item that has ended by then -- of its
/// own voice first, of its staff's others after -- the cursor standing just
/// after that.
pub fn shown_by(sheet: &Sheet, place: Place) -> Option<(u64, bool)> {
    if let Some(item) = item_at(sheet, place) {
        return Some((item.id(), false));
    }
    let staves: Vec<usize> = std::iter::once(place.staff)
        .chain((0..sheet.staves.len()).filter(|s| *s != place.staff))
        .collect();
    for &staff in &staves {
        let voices = sheet.staves.get(staff).map_or(0, |s| s.voices.len());
        for voice in 0..voices {
            if let Some((_, item)) = onsets(sheet, staff, voice)
                .into_iter()
                .find(|(at, _)| *at == place.at)
            {
                return Some((item.id(), false));
            }
        }
    }
    // inside an item of its own voice: that item's column
    if let Some((_, item)) = onsets(sheet, place.staff, place.voice)
        .into_iter()
        .find(|(at, item)| *at < place.at && place.at < *at + item.dur())
    {
        return Some((item.id(), false));
    }
    let ended = |voice: usize| {
        onsets(sheet, place.staff, voice)
            .into_iter()
            .filter(|(at, item)| *at + item.dur() <= place.at)
            .max_by_key(|(at, item)| *at + item.dur())
    };
    let voices = sheet.staves.get(place.staff).map_or(0, |s| s.voices.len());
    ended(place.voice)
        .or_else(|| {
            (0..voices)
                .filter(|v| *v != place.voice)
                .filter_map(ended)
                .max_by_key(|(at, item)| *at + item.dur())
        })
        .map(|(_, item)| (item.id(), true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_core::notation::{Slot, voice_to_sheet};

    /// Four quarters, then a half, in 4/4: ids 1..5.
    fn tune() -> Sheet {
        let mut sheet = voice_to_sheet(
            &[
                Slot::note(vec![60], 8),
                Slot::note(vec![62], 8),
                Slot::note(vec![64], 8),
                Slot::note(vec![65], 8),
                Slot::note(vec![67], 16),
            ],
            "4/4",
            "G2",
            "C",
        );
        sheet.assign_ids();
        sheet
    }

    fn at(n: i64, d: i64) -> Place {
        Place {
            at: Ratio::new(n, d),
            ..Place::default()
        }
    }

    #[test]
    fn the_arrows_step_by_item_and_past_the_end_by_the_value() {
        let sheet = tune();
        let q = Ratio::new(1, 4);
        assert_eq!(on(&sheet, at(0, 1), q), at(1, 4));
        assert_eq!(on(&sheet, at(1, 1), q), at(3, 2), "over the half");
        assert_eq!(on(&sheet, at(3, 2), q), at(7, 4), "past the end");
        assert_eq!(back(&sheet, at(3, 2), q), at(1, 1));
        assert_eq!(back(&sheet, at(1, 8), q), at(0, 1), "into an item");
        assert_eq!(back(&sheet, at(0, 1), q), at(0, 1), "never before");
        // and an empty voice moves by the value
        let second = Place {
            voice: 1,
            ..at(1, 2)
        };
        assert_eq!(back(&sheet, second, q).at, q);
    }

    #[test]
    fn a_bar_key_goes_to_a_measure_start() {
        let sheet = tune();
        assert_eq!(bar_on(&sheet, at(1, 4)), at(1, 1));
        assert_eq!(bar_back(&sheet, at(5, 4)), at(1, 1));
        assert_eq!(bar_back(&sheet, at(1, 1)), at(0, 1));
        assert_eq!(bar_back(&sheet, at(0, 1)), at(0, 1));
    }

    #[test]
    fn the_cursor_is_shown_by_what_starts_where_it_stands() {
        let sheet = tune();
        assert_eq!(shown_by(&sheet, at(1, 4)), Some((2, false)));
        // a second voice with nothing in it shows the first voice's column
        let second = Place {
            voice: 1,
            ..at(1, 2)
        };
        assert_eq!(shown_by(&sheet, second), Some((3, false)));
        // past the end, just after the last item
        assert_eq!(shown_by(&sheet, at(3, 2)), Some((5, true)));
        // inside an item no column starts: that item
        assert_eq!(shown_by(&sheet, at(5, 4)), Some((5, false)));
        // and past the end of a second voice, its own last item, though the
        // first voice's is still sounding
        let mut two = sheet.clone();
        two.staves[0].voices.push(clausters_core::notation::Voice {
            items: vec![Item::Rest {
                id: 9,
                dur: Ratio::new(3, 2) - Ratio::new(1, 8),
            }],
        });
        let second = Place {
            voice: 1,
            ..at(23, 16)
        };
        assert_eq!(shown_by(&two, second), Some((9, true)));
    }

    #[test]
    fn a_letter_takes_the_octave_of_the_note_before() {
        let sheet = tune();
        let before = near(&sheet, at(1, 2), None).unwrap();
        assert_eq!(before.midi(), 62, "the D before the cursor");
        assert_eq!(near(&sheet, at(0, 1), None), None);
        assert_eq!(step_of("g"), Some(Step::G));
        assert_eq!(value_of("eighth"), Some(Ratio::new(1, 8)));
    }
}
