//! The operators: what rearranges a score's content and its grid, as against
//! what edits one note of it.
//!
//! Every one of them is a function from a sheet to a sheet, which is what lets
//! them compose — `concat(transpose(a, 2), b)` is a sentence, and the result of
//! composing two operations is the operation on the composed score. None of
//! them mutates anything a caller holds.
//!
//! **The two structures move independently, and that is the invariant to read
//! these against.** An operation on the content leaves the grid alone: stretch
//! a phrase and the barlines stay where they were, so the music re-bars across
//! them and ties where it must. An operation on the grid does not rewrite
//! notes: change the meter and the same notes fall in different measures.
//! Only the three that *add or remove time* — [`insert_measures`],
//! [`remove_measures`] and [`repeat`] — touch both, and they say so.
//!
//! **Where two sheets meet, ids are re-minted.** Two items answering to one id
//! would both answer to one edit, so the incoming sheet's items are renumbered
//! past everything the receiving sheet uses. A caller holding an id into the
//! sheet it passed in cannot use it afterwards, which is the honest outcome:
//! the result is a new score, not either of its parts.

use super::model::{Grid, Item, Meter, Pitch, Sheet, Staff, Step, Voice};
use super::ops::Span;
use crate::ratio::Ratio;

/// Split a run of items at `t`, measured from the run's start.
///
/// An item straddling the cut is divided in two: the first piece keeps the id
/// and, if it sounds, ties into the second, which takes a fresh one. That is
/// the tie a *musical* split makes — the note goes on sounding across the cut —
/// as against the tie an emitter adds at a barline, which is made from the
/// projection and never stored.
fn split_at(items: &[Item], t: Ratio, mint: &mut dyn FnMut() -> u64) -> (Vec<Item>, Vec<Item>) {
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut onset = Ratio::ZERO;
    for item in items {
        let end = onset + item.dur();
        if end <= t {
            before.push(item.clone());
        } else if onset >= t {
            after.push(item.clone());
        } else {
            // the cut falls inside this item
            let head = item.with_dur(t - onset);
            before.push(match head {
                Item::Note {
                    id,
                    pitches,
                    dur,
                    marks,
                    ..
                } => Item::Note {
                    id,
                    pitches,
                    dur,
                    tie: true,
                    marks,
                },
                rest => rest,
            });
            after.push(item.with_dur(end - t).with_id(mint()));
        }
        onset = end;
    }
    (before, after)
}

/// Lengthen a run of items with a rest so it lasts exactly `len`.
fn padded(items: Vec<Item>, len: Ratio, mint: &mut dyn FnMut() -> u64) -> Vec<Item> {
    let have = items.iter().fold(Ratio::ZERO, |acc, i| acc + i.dur());
    let mut out = items;
    if have < len {
        out.push(Item::Rest {
            id: mint(),
            dur: len - have,
        });
    }
    out
}

/// Renumber every item of `sheet` past `from`, moving what points at those
/// items along with them, and return the next free id.
///
/// **A spanner points at ids, so renumbering has to carry it.** Leaving it
/// behind loses a slur or a hairpin silently, which is the worst way for an
/// operation to fail: the music is still there, the mark is not, and nothing
/// said so.
fn renumber(sheet: &mut Sheet, from: u64) -> u64 {
    let mut next = from.max(1);
    let mut moved: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    for voice in sheet.staves.iter_mut().flat_map(|s| s.voices.iter_mut()) {
        for item in &mut voice.items {
            moved.insert(item.id(), next);
            *item = item.with_id(next);
            next += 1;
        }
    }
    for spanner in &mut sheet.spanners {
        if let Some(&id) = moved.get(&spanner.from) {
            spanner.from = id;
        }
        if let Some(&id) = moved.get(&spanner.to) {
            spanner.to = id;
        }
    }
    sheet.next_id = next;
    next
}

/// Drop the spanners whose ends are no longer in the score.
///
/// What removes items has to do this, or the score becomes unwritable: the
/// emitter refuses a spanner pointing at a note that is not there, and rightly
/// — but a caller who deleted a note did not ask for a score that cannot be
/// engraved. The slur over it goes with it.
pub(super) fn prune_spanners(sheet: &mut Sheet) {
    let live: std::collections::HashSet<u64> = sheet
        .voices()
        .flat_map(|v| v.items.iter().map(Item::id))
        .collect();
    sheet
        .spanners
        .retain(|s| live.contains(&s.from) && live.contains(&s.to));
}

/// One score after another.
///
/// The content joins end to end — each voice of `b` continues the voice of `a`
/// in the same position, with a rest filling any voice of `a` that ran short,
/// so the two do not slide against each other.
///
/// The **grid is `a`'s, continued**. When `a`'s content ends on a barline,
/// `b`'s meters are appended shifted by the measures `a` occupies, so a 4/4
/// section followed by a 3/4 one is exactly that. When it ends *mid-measure*
/// there is no barline for `b`'s grid to start at, so a `b` that names any
/// metric layout of its own is **refused** rather than silently re-barred: the
/// music would come out in bars nobody wrote.
///
/// # Errors
/// When `a` ends mid-measure and `b` carries a grid that is not simply the
/// meter already in force.
pub fn concat(mut a: Sheet, b: &Sheet) -> Result<Sheet, String> {
    a.assign_ids();
    let mut b = b.clone();
    b.assign_ids();
    let next = renumber(&mut b, a.next_id);

    let len = a.len();
    let (measure, offset) = a.grid.position(len);
    let in_force = a.grid.meter_at(measure);
    if offset.is_zero() {
        for meter in &b.grid.meters {
            let shifted = Meter {
                measure: meter.measure + measure,
                ..*meter
            };
            // the first of b's meters is redundant when it is what already plays
            if shifted.measure == measure
                && shifted.count == in_force.count
                && shifted.unit == in_force.unit
            {
                continue;
            }
            a.grid.meters.push(shifted);
        }
        for (m, len) in &b.grid.irregular {
            a.grid.irregular.push((m + measure, *len));
        }
    } else {
        let plain = b.grid.irregular.is_empty()
            && b.grid.meters.len() == 1
            && b.grid.meters[0].count == in_force.count
            && b.grid.meters[0].unit == in_force.unit;
        if !plain {
            return Err(format!(
                "the first score ends inside measure {}, so there is no barline \
                 for the second score's own metric layout to begin at; give the \
                 second score the meter already in force, or pad the first to a \
                 full measure",
                measure + 1
            ));
        }
    }

    let mut mint = {
        let mut n = next;
        move || {
            n += 1;
            n - 1
        }
    };
    while a.staves.len() < b.staves.len() {
        a.staves.push(Staff {
            clef: b.staves[a.staves.len()].clef.clone(),
            voices: Vec::new(),
        });
    }
    for (si, staff) in a.staves.iter_mut().enumerate() {
        let incoming = b.staves.get(si);
        let voices = incoming.map(|s| s.voices.len()).unwrap_or(0);
        while staff.voices.len() < voices {
            staff.voices.push(Voice::default());
        }
        for (vi, voice) in staff.voices.iter_mut().enumerate() {
            voice.items = padded(std::mem::take(&mut voice.items), len, &mut mint);
            if let Some(other) = incoming.and_then(|s| s.voices.get(vi)) {
                voice.items.extend(other.items.iter().cloned());
            }
        }
    }
    a.spanners.extend(b.spanners);
    a.next_id = mint();
    Ok(a)
}

/// Two scores at the same time.
///
/// `as_staff` decides what "at the same time" means on the page: `false` puts
/// `b`'s voices on `a`'s own staves, which is counterpoint on one staff;
/// `true` appends `b`'s staves below `a`'s, which is a second hand or a second
/// instrument. Both are superposition — the difference is where the notes are
/// written, not when they sound.
///
/// # Errors
/// When the two grids differ. Two scores cannot share a moment in time while
/// disagreeing about where the barlines are, and picking one of the two would
/// silently re-bar the other.
pub fn stack(mut a: Sheet, b: &Sheet, as_staff: bool) -> Result<Sheet, String> {
    a.assign_ids();
    let mut b = b.clone();
    b.assign_ids();
    if a.grid != b.grid {
        return Err(
            "the two scores have different metric layouts, so they cannot be \
             stacked; give them the same grid first"
                .to_string(),
        );
    }
    let next = renumber(&mut b, a.next_id);
    a.spanners.extend(std::mem::take(&mut b.spanners));
    if as_staff {
        a.staves.extend(b.staves);
    } else {
        for (si, staff) in b.staves.into_iter().enumerate() {
            match a.staves.get_mut(si) {
                Some(mine) => mine.voices.extend(staff.voices),
                None => a.staves.push(staff),
            }
        }
    }
    a.next_id = next;
    Ok(a)
}

/// A stretch of the score, played `count` times in a row.
///
/// `count` is the **total** number of times it is heard, so `2` is one repeat
/// and `1` changes nothing. The copies go where the original is, pushing
/// everything after it later, and the grid grows by as many measures as the
/// repeated stretch spans — this is one of the three operations that adds time,
/// so both structures move together.
///
/// # Errors
/// When `count` is zero (removing music is [`remove_measures`]'s job, and a
/// silent no-op would be worse than either), or when the span cannot be
/// resolved.
pub fn repeat(mut sheet: Sheet, count: usize, span: &Span) -> Result<Sheet, String> {
    if count == 0 {
        return Err("a stretch played zero times is a deletion, not a repeat".into());
    }
    sheet.assign_ids();
    if count == 1 {
        return Ok(sheet);
    }
    let bounds = span.resolve(&sheet)?;
    let (start, end) = bounds.unwrap_or((Ratio::ZERO, sheet.len()));
    let (first, last) = match *span {
        Span::All => (0, sheet.grid.position(sheet.len()).0),
        Span::Measures(f, l) => (f - 1, l - 1),
    };

    let mut next = sheet.next_id;
    let mut mint = move || {
        next += 1;
        next - 1
    };
    for voice in sheet.staves.iter_mut().flat_map(|s| s.voices.iter_mut()) {
        let items = std::mem::take(&mut voice.items);
        let (head, rest) = split_at(&items, start, &mut mint);
        let (body, tail) = split_at(&rest, end - start, &mut mint);
        let mut out = head;
        for _ in 0..count {
            out.extend(body.iter().map(|i| i.with_id(mint())));
        }
        out.extend(tail);
        voice.items = out;
    }

    // The grid gains `count - 1` more copies of the repeated measures, so the
    // music that follows keeps the barlines it was written against.
    let repeated = last + 1 - first;
    let added = repeated * (count - 1);
    let pattern: Vec<Meter> = (first..=last).map(|m| sheet.grid.meter_at(m)).collect();
    let irregular: Vec<(usize, Ratio)> = (first..=last)
        .filter_map(|m| {
            sheet
                .grid
                .irregular
                .iter()
                .find(|(im, _)| *im == m)
                .map(|(_, len)| (m, *len))
        })
        .collect();
    shift_grid(&mut sheet.grid, last + 1, added as isize);
    for copy in 1..count {
        let base = last + 1 + repeated * (copy - 1);
        for (i, meter) in pattern.iter().enumerate() {
            sheet.grid.meters.push(Meter {
                measure: base + i,
                ..*meter
            });
        }
        for (m, len) in &irregular {
            sheet.grid.irregular.push((base + (m - first), *len));
        }
    }
    tidy(&mut sheet.grid);
    sheet.next_id = mint();
    Ok(sheet)
}

/// The span's items in reverse order, voice by voice.
///
/// The durations come back in the mirrored order, so the stretch lasts exactly
/// as long as it did and the grid is untouched. A tie travels with the pair it
/// joined: what was tied *into* its successor is now tied into what used to
/// precede it.
pub fn retrograde(mut sheet: Sheet, span: &Span) -> Result<Sheet, String> {
    sheet.assign_ids();
    let bounds = span.resolve(&sheet)?;
    let (start, end) = bounds.unwrap_or((Ratio::ZERO, sheet.len()));
    let mut next = sheet.next_id;
    let mut mint = move || {
        next += 1;
        next - 1
    };
    for voice in sheet.staves.iter_mut().flat_map(|s| s.voices.iter_mut()) {
        let items = std::mem::take(&mut voice.items);
        let (head, rest) = split_at(&items, start, &mut mint);
        let (body, tail) = split_at(&rest, end - start, &mut mint);
        // Reverse the items, then move each tie back onto the pair it joined:
        // a tie sits *between* two items, so reversing the items alone would
        // leave every one of them tying into the wrong neighbour.
        let ties: Vec<bool> = body
            .iter()
            .map(|i| matches!(i, Item::Note { tie: true, .. }))
            .collect();
        let mut flipped: Vec<Item> = body.into_iter().rev().collect();
        let n = flipped.len();
        for (i, item) in flipped.iter_mut().enumerate() {
            // item i of the reversal was item n-1-i, whose predecessor tied
            // into it; that tie now runs the other way.
            let tie = n >= 2 && i + 1 < n && ties[n - 2 - i];
            if let Item::Note { tie: t, .. } = item {
                *t = tie;
            }
        }
        let mut out = head;
        out.extend(flipped);
        out.extend(tail);
        voice.items = out;
    }
    sheet.next_id = mint();
    Ok(sheet)
}

/// Mirror every pitch of the span about `axis`.
///
/// The mirror is exact in both dimensions the model keeps separate: the
/// notehead reflects across the axis on the staff, and the sound reflects
/// across it in semitones, with the accidental taking up whatever difference
/// remains. So an inversion of a diatonic line stays diatonic where the
/// interval pattern allows and spells the accidental where it does not, which
/// is what an inversion written by hand looks like.
///
/// Left without an axis, the first sounding pitch of the span is used — the
/// line turns about its own first note.
pub fn invert(mut sheet: Sheet, axis: Option<Pitch>, span: &Span) -> Result<Sheet, String> {
    sheet.assign_ids();
    let bounds = span.resolve(&sheet)?;
    let axis = match axis.or_else(|| first_pitch(&sheet, &bounds)) {
        Some(axis) => axis,
        // Nothing sounds in the span, so there is nothing to mirror and no
        // axis to guess: leaving it alone is the whole of the operation.
        None => return Ok(sheet),
    };
    for_each_item_in(&mut sheet, &bounds, &mut |item| {
        if let Item::Note { pitches, .. } = item {
            for pitch in pitches.iter_mut() {
                *pitch = invert_pitch(pitch, &axis);
            }
        }
    });
    Ok(sheet)
}

/// One pitch mirrored about another.
pub fn invert_pitch(pitch: &Pitch, axis: &Pitch) -> Pitch {
    // Absolute diatonic index: the place on the staff, counting every letter.
    let place = |p: &Pitch| p.octave * 7 + p.step.index();
    let index = 2 * place(axis) - place(pitch);
    let step = Step::ALL[index.rem_euclid(7) as usize];
    let octave = index.div_euclid(7);
    let natural = (octave + 1) * 12 + step.semitones();
    Pitch {
        step,
        alter: 2 * axis.midi() - pitch.midi() - natural,
        octave,
        forced: pitch.forced,
    }
}

/// Multiply every written value in the span by `factor`.
///
/// Augmentation is `2`, diminution is `1/2`, and anything else is the same
/// operation at another ratio. **The grid does not move**, which is the point:
/// the phrase is re-barred against the barlines it already had, tying across
/// them where a value now overruns one — which is what augmentation looks like
/// on a page and what a caller means by it.
///
/// # Errors
/// When `factor` is not positive: a zero-length note sounds nothing and a
/// negative one is not a duration.
pub fn stretch(mut sheet: Sheet, factor: Ratio, span: &Span) -> Result<Sheet, String> {
    if !factor.is_positive() {
        return Err(format!(
            "a written value cannot be scaled by {factor}; the factor has to be \
             greater than zero"
        ));
    }
    sheet.assign_ids();
    let bounds = span.resolve(&sheet)?;
    for_each_item_in(&mut sheet, &bounds, &mut |item| {
        *item = item.with_dur(item.dur() * factor);
    });
    Ok(sheet)
}

/// Put a meter in force from `measure` onward.
///
/// The grid alone changes: the same notes fall in different measures
/// afterwards, which is what changing the meter of a piece means. Setting the
/// meter a measure already carries replaces it rather than stacking a second.
pub fn set_meter(mut sheet: Sheet, measure: usize, count: i64, unit: i64) -> Result<Sheet, String> {
    if measure == 0 {
        return Err("measures are numbered from 1, so there is no measure 0".into());
    }
    if count <= 0 || unit <= 0 {
        return Err(format!("{count}/{unit} is not a meter"));
    }
    let at = measure - 1;
    sheet.grid.meters.retain(|m| m.measure != at);
    sheet.grid.meters.push(Meter {
        measure: at,
        count,
        unit,
    });
    tidy(&mut sheet.grid);
    Ok(sheet)
}

/// Open `count` empty measures before measure `at`.
///
/// Time is added, so both structures move: the content is cut at that barline
/// and a rest of the new measures' length is written in, and every meter and
/// irregular bar after the cut slides along. What was in measure `at` is now in
/// measure `at + count`, with nothing rewritten.
pub fn insert_measures(mut sheet: Sheet, at: usize, count: usize) -> Result<Sheet, String> {
    if at == 0 {
        return Err("measures are numbered from 1, so there is no measure 0".into());
    }
    if count == 0 {
        return Ok(sheet);
    }
    sheet.assign_ids();
    let index = at - 1;
    let start = sheet.grid.measure_start(index);
    // The new bars take the meter in force where they are opened.
    let meter = sheet.grid.meter_at(index);
    let added = meter.bar() * Ratio::from(count as i64);

    let mut next = sheet.next_id;
    let mut mint = move || {
        next += 1;
        next - 1
    };
    for voice in sheet.staves.iter_mut().flat_map(|s| s.voices.iter_mut()) {
        let items = std::mem::take(&mut voice.items);
        let (head, tail) = split_at(&items, start, &mut mint);
        if tail.is_empty() && head.is_empty() {
            continue;
        }
        let mut out = head;
        out.push(Item::Rest {
            id: mint(),
            dur: added,
        });
        out.extend(tail);
        voice.items = out;
    }
    shift_grid(&mut sheet.grid, index, count as isize);
    tidy(&mut sheet.grid);
    sheet.next_id = mint();
    Ok(sheet)
}

/// Take measures `first` to `last` out, with whatever was written in them.
///
/// The other half of [`insert_measures`], and the same rule: time is removed,
/// so the content in that stretch goes and everything after it slides back into
/// the barlines that remain.
pub fn remove_measures(mut sheet: Sheet, first: usize, last: usize) -> Result<Sheet, String> {
    let bounds = Span::Measures(first, last).resolve(&sheet)?;
    let (start, end) = bounds.expect("a measure span always resolves to bounds");
    sheet.assign_ids();
    let mut next = sheet.next_id;
    let mut mint = move || {
        next += 1;
        next - 1
    };
    for voice in sheet.staves.iter_mut().flat_map(|s| s.voices.iter_mut()) {
        let items = std::mem::take(&mut voice.items);
        let (head, rest) = split_at(&items, start, &mut mint);
        let (_, tail) = split_at(&rest, end - start, &mut mint);
        let mut out = head;
        out.extend(tail);
        voice.items = out;
    }
    let removed = last + 1 - first;
    sheet
        .grid
        .meters
        .retain(|m| m.measure < first - 1 || m.measure > last - 1 || m.measure == 0);
    sheet
        .grid
        .irregular
        .retain(|(m, _)| *m < first - 1 || *m > last - 1);
    shift_grid(&mut sheet.grid, last, -(removed as isize));
    tidy(&mut sheet.grid);
    sheet.next_id = mint();
    prune_spanners(&mut sheet);
    Ok(sheet)
}

// -- shared machinery ---------------------------------------------------------

/// Move every meter and irregular bar at or after `from` by `by` measures.
fn shift_grid(grid: &mut Grid, from: usize, by: isize) {
    let moved = |m: usize| -> usize {
        if m >= from {
            (m as isize + by).max(0) as usize
        } else {
            m
        }
    };
    for meter in &mut grid.meters {
        meter.measure = moved(meter.measure);
    }
    for (m, _) in &mut grid.irregular {
        *m = moved(*m);
    }
}

/// Put the grid back in order: sorted, one meter per measure, and no meter that
/// restates the one already in force.
fn tidy(grid: &mut Grid) {
    grid.meters.sort_by_key(|m| m.measure);
    grid.meters.dedup_by_key(|m| m.measure);
    let mut kept: Vec<Meter> = Vec::new();
    for meter in grid.meters.drain(..) {
        match kept.last() {
            Some(prev) if prev.count == meter.count && prev.unit == meter.unit => {}
            _ => kept.push(meter),
        }
    }
    if kept.first().map(|m| m.measure) != Some(0) {
        kept.insert(
            0,
            Meter {
                measure: 0,
                count: 4,
                unit: 4,
            },
        );
    }
    grid.meters = kept;
    grid.irregular.sort_by_key(|(m, _)| *m);
    grid.irregular.dedup_by_key(|(m, _)| *m);
}

/// Run `f` over every item whose onset falls inside `bounds`.
fn for_each_item_in(
    sheet: &mut Sheet,
    bounds: &Option<(Ratio, Ratio)>,
    f: &mut dyn FnMut(&mut Item),
) {
    for voice in sheet.staves.iter_mut().flat_map(|s| s.voices.iter_mut()) {
        let mut onset = Ratio::ZERO;
        for item in &mut voice.items {
            let dur = item.dur();
            let inside = match bounds {
                None => true,
                Some((start, end)) => onset >= *start && onset < *end,
            };
            if inside {
                f(item);
            }
            onset = onset + dur;
        }
    }
}

/// The first pitch that sounds inside `bounds`, reading the voices in order.
fn first_pitch(sheet: &Sheet, bounds: &Option<(Ratio, Ratio)>) -> Option<Pitch> {
    for voice in sheet.voices() {
        let mut onset = Ratio::ZERO;
        for item in &voice.items {
            let inside = match bounds {
                None => true,
                Some((start, end)) => onset >= *start && onset < *end,
            };
            if inside && let Some(first) = item.pitches().first() {
                return Some(*first);
            }
            onset = onset + item.dur();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notation::mei::sheet_to_mei;
    use crate::notation::ops::{Op, apply};

    /// A one-staff, one-voice sheet of quarter notes on middle C.
    fn quarters(n: usize) -> Sheet {
        let mut sheet = Sheet {
            staves: vec![Staff {
                clef: "G2".into(),
                voices: vec![Voice {
                    items: (0..n)
                        .map(|_| Item::Note {
                            id: 0,
                            pitches: vec![Pitch {
                                step: Step::C,
                                alter: 0,
                                octave: 4,
                                forced: false,
                            }],
                            dur: Ratio::new(1, 4),
                            tie: false,
                            marks: Default::default(),
                        })
                        .collect(),
                }],
            }],
            ..Default::default()
        };
        sheet.assign_ids();
        sheet
    }

    fn durs(sheet: &Sheet) -> Vec<Ratio> {
        sheet.staves[0].voices[0]
            .items
            .iter()
            .map(Item::dur)
            .collect()
    }

    fn steps(sheet: &Sheet) -> Vec<&'static str> {
        sheet.staves[0].voices[0]
            .items
            .iter()
            .map(|i| i.pitches().first().map(|p| p.step.pname()).unwrap_or("r"))
            .collect()
    }

    #[test]
    fn operations_compose() {
        // The acceptance the whole family rests on: composing two operations
        // is the operation on the composed score. Transposing two sheets and
        // then joining them is joining them and then transposing.
        let a = quarters(4);
        let b = quarters(4);
        let up = |s: Sheet| {
            apply(
                s,
                &Op::Transpose {
                    semitones: 2,
                    steps: None,
                    span: Span::All,
                },
            )
            .unwrap()
        };
        let left = up(concat(a.clone(), &b).unwrap());
        let right = concat(up(a), &up(b)).unwrap();
        // The music is the same; only the minted ids can differ, and they are
        // not the music, so the written page is what is compared.
        assert_eq!(sheet_to_mei(&left).unwrap(), sheet_to_mei(&right).unwrap());
    }

    #[test]
    fn joining_two_scores_carries_what_is_written_between_their_notes() {
        use crate::notation::model::Spanner;
        // A slur over the second score's two notes has to survive being joined
        // to the first, ids and all -- losing it silently is the worst way an
        // operation can fail: the music is there and the mark is not.
        let mut b = quarters(2);
        let ids: Vec<u64> = b.staves[0].voices[0].items.iter().map(Item::id).collect();
        b.spanners = vec![Spanner {
            kind: "slur".into(),
            from: ids[0],
            to: ids[1],
        }];
        let joined = concat(quarters(4), &b).unwrap();
        assert_eq!(joined.spanners.len(), 1, "the slur came along");
        // and it points at the notes it was written over, which were renumbered
        let live: Vec<u64> = joined
            .voices()
            .flat_map(|v| v.items.iter().map(Item::id))
            .collect();
        assert!(live.contains(&joined.spanners[0].from));
        assert!(live.contains(&joined.spanners[0].to));
        // the page can be written, which is what proves the ends resolve
        assert!(sheet_to_mei(&joined).is_ok());

        // stacking carries it too
        let stacked = stack(quarters(2), &b, false).unwrap();
        assert_eq!(stacked.spanners.len(), 1);
        assert!(sheet_to_mei(&stacked).is_ok());
    }

    #[test]
    fn removing_the_notes_a_slur_was_over_removes_the_slur() {
        use crate::notation::model::Spanner;
        // Two bars; a slur over the second bar's notes. Dropping that bar has
        // to take the slur, or the score cannot be engraved at all.
        let mut sheet = quarters(8);
        let ids: Vec<u64> = sheet.staves[0].voices[0]
            .items
            .iter()
            .map(Item::id)
            .collect();
        sheet.spanners = vec![Spanner {
            kind: "slur".into(),
            from: ids[4],
            to: ids[7],
        }];
        let cut = remove_measures(sheet, 2, 2).unwrap();
        assert!(cut.spanners.is_empty());
        assert!(sheet_to_mei(&cut).is_ok());
    }

    #[test]
    fn content_and_grid_move_independently() {
        let sheet = quarters(8);
        let grid = sheet.grid.clone();
        // stretching the content does not move a barline
        let stretched = stretch(sheet.clone(), Ratio::from(2), &Span::All).unwrap();
        assert_eq!(stretched.grid, grid);
        assert_eq!(durs(&stretched), vec![Ratio::new(1, 2); 8]);
        // and changing the meter does not rewrite a note
        let remetered = set_meter(sheet.clone(), 2, 3, 4).unwrap();
        assert_eq!(durs(&remetered), durs(&sheet));
        assert_eq!(remetered.grid.meter_at(1).count, 3);
    }

    #[test]
    fn an_augmented_phrase_rebars_against_the_grid_it_was_written_on() {
        // Four quarters in 4/4 is one bar. Doubled, it is two bars, and every
        // value still fits one -- nothing has to tie.
        let doubled = stretch(quarters(4), Ratio::from(2), &Span::All).unwrap();
        let mei = sheet_to_mei(&doubled).unwrap();
        assert_eq!(mei.matches("<measure").count(), 2);
        assert_eq!(mei.matches("tie=").count(), 0);

        // Three quarters tripled is nine quarters: bar 1 takes two of the
        // three-quarter notes and half of the third, which ties across.
        let tripled = stretch(quarters(3), Ratio::from(3), &Span::All).unwrap();
        assert_eq!(durs(&tripled), vec![Ratio::new(3, 4); 3]);
        let mei = sheet_to_mei(&tripled).unwrap();
        assert_eq!(mei.matches("<measure").count(), 3);
        assert!(mei.contains("tie=\"i\"") && mei.contains("tie=\"t\""));
    }

    #[test]
    fn concat_continues_the_grid_when_the_first_score_ends_on_a_barline() {
        let a = quarters(4); // exactly one 4/4 bar
        let mut b = quarters(3);
        b.grid = Grid::uniform(3, 4);
        let joined = concat(a, &b).unwrap();
        assert_eq!(joined.grid.meter_at(0).count, 4);
        assert_eq!(joined.grid.meter_at(1).count, 3);
        assert_eq!(
            sheet_to_mei(&joined).unwrap().matches("<measure").count(),
            2
        );
    }

    #[test]
    fn concat_refuses_a_grid_it_would_have_to_invent_a_barline_for() {
        let a = quarters(3); // three quarters of a 4/4 bar
        let mut b = quarters(3);
        b.grid = Grid::uniform(3, 4);
        let err = concat(a.clone(), &b).unwrap_err();
        assert!(err.contains("no barline"), "{err}");
        // but the same music with the meter already in force simply continues
        let plain = concat(a, &quarters(3)).unwrap();
        assert_eq!(durs(&plain).len(), 6);
    }

    #[test]
    fn stacking_puts_a_line_beside_another_or_below_it() {
        let a = quarters(4);
        let b = quarters(4);
        let voices = stack(a.clone(), &b, false).unwrap();
        assert_eq!(voices.staves.len(), 1);
        assert_eq!(voices.staves[0].voices.len(), 2);
        let staves = stack(a, &b, true).unwrap();
        assert_eq!(staves.staves.len(), 2);
        // ids are re-minted, so no edit can name two notes at once
        let ids: Vec<u64> = staves
            .voices()
            .flat_map(|v| v.items.iter().map(Item::id))
            .collect();
        let unique: std::collections::HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(ids.len(), unique.len());
    }

    #[test]
    fn stacking_refuses_two_different_metric_layouts() {
        let a = quarters(4);
        let mut b = quarters(4);
        b.grid = Grid::uniform(3, 4);
        let err = stack(a, &b, false).unwrap_err();
        assert!(err.contains("different metric layouts"), "{err}");
    }

    #[test]
    fn repeat_adds_time_to_both_structures() {
        // one bar, played twice
        let twice = repeat(quarters(4), 2, &Span::All).unwrap();
        assert_eq!(durs(&twice).len(), 8);
        assert_eq!(sheet_to_mei(&twice).unwrap().matches("<measure").count(), 2);
        // a repeat of nothing is a refusal rather than a quiet deletion
        assert!(repeat(quarters(4), 0, &Span::All).is_err());
        // and once is once
        assert_eq!(durs(&repeat(quarters(4), 1, &Span::All).unwrap()).len(), 4);
    }

    #[test]
    fn repeating_one_measure_pushes_what_follows() {
        // two bars; repeat the first, so the second ends up third
        let sheet = repeat(quarters(8), 2, &Span::Measures(1, 1)).unwrap();
        assert_eq!(durs(&sheet).len(), 12);
        assert_eq!(sheet_to_mei(&sheet).unwrap().matches("<measure").count(), 3);
    }

    #[test]
    fn retrograde_mirrors_the_order_and_keeps_the_length() {
        let mut sheet = quarters(3);
        // give the three notes different values and pitches
        let voice = &mut sheet.staves[0].voices[0];
        voice.items[1] = voice.items[1].with_dur(Ratio::new(1, 2));
        for (i, step) in [Step::C, Step::D, Step::E].iter().enumerate() {
            if let Item::Note { pitches, .. } = &mut voice.items[i] {
                pitches[0].step = *step;
            }
        }
        let before = sheet.len();
        let back = retrograde(sheet, &Span::All).unwrap();
        assert_eq!(steps(&back), vec!["e", "d", "c"]);
        assert_eq!(
            durs(&back),
            vec![Ratio::new(1, 4), Ratio::new(1, 2), Ratio::new(1, 4)]
        );
        assert_eq!(back.len(), before);
    }

    #[test]
    fn inversion_mirrors_on_the_staff_and_in_the_sound() {
        let axis = Pitch {
            step: Step::C,
            alter: 0,
            octave: 4,
            forced: false,
        };
        // a rising major third inverts to a falling one: E4 -> Ab3
        let e = Pitch {
            step: Step::E,
            alter: 0,
            octave: 4,
            forced: false,
        };
        let down = invert_pitch(&e, &axis);
        assert_eq!(down.step, Step::A);
        assert_eq!(down.alter, -1);
        assert_eq!(down.octave, 3);
        assert_eq!(down.midi(), 2 * axis.midi() - e.midi());
        // inverting twice about the same axis is the identity
        assert_eq!(invert_pitch(&down, &axis), e);
    }

    #[test]
    fn inverting_a_line_turns_it_about_its_own_first_note() {
        let mut sheet = quarters(3);
        for (i, step) in [Step::C, Step::E, Step::G].iter().enumerate() {
            if let Item::Note { pitches, .. } = &mut sheet.staves[0].voices[0].items[i] {
                pitches[0].step = *step;
            }
        }
        let turned = invert(sheet, None, &Span::All).unwrap();
        // about C4: C stays, E goes to Ab3, G to F3
        assert_eq!(steps(&turned), vec!["c", "a", "f"]);
    }

    #[test]
    fn measures_open_and_close_and_the_music_moves_with_them() {
        // two bars; open one before the second
        let sheet = insert_measures(quarters(8), 2, 1).unwrap();
        assert_eq!(sheet_to_mei(&sheet).unwrap().matches("<measure").count(), 3);
        assert_eq!(sheet.len(), Ratio::from(3));
        // and taking it back out restores the length
        let back = remove_measures(sheet, 2, 2).unwrap();
        assert_eq!(back.len(), Ratio::from(2));
        assert_eq!(durs(&back).len(), 8);
    }

    #[test]
    fn removing_measures_takes_what_was_written_in_them() {
        // three bars of quarters; drop the middle one
        let sheet = remove_measures(quarters(12), 2, 2).unwrap();
        assert_eq!(sheet.len(), Ratio::from(2));
        assert_eq!(durs(&sheet).len(), 8);
    }

    #[test]
    fn a_meter_change_survives_the_measures_inserted_before_it() {
        let sheet = set_meter(quarters(8), 2, 3, 4).unwrap();
        assert_eq!(sheet.grid.meter_at(1).count, 3);
        let opened = insert_measures(sheet, 1, 1).unwrap();
        // what was measure 2 is measure 3, and the 3/4 went with it
        assert_eq!(opened.grid.meter_at(1).count, 4);
        assert_eq!(opened.grid.meter_at(2).count, 3);
    }
}
