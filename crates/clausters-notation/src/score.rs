//! The libverovio binding of the editable score: what an [`Engraver`] call is,
//! natively.
//!
//! The score model itself -- the order an edit is made in, when the layout is
//! re-run and reloaded, the undo stack of MEI snapshots -- is
//! [`clausters_core::notation::Score`], because all of it is logic and both
//! clients run it. What is here is the crossing: [`Toolkit`] implements the
//! [`Engraver`] port over libverovio's C wrapper, and [`open`] builds one with
//! a resource path and options before handing it over.
//!
//! The tests below are the other half of that split: they need a real C++
//! engraver to be right about, which is why they are here and not in the core,
//! whose own tests drive the state machine over a fake.

use clausters_core::notation::{AnyEngraver, Engraver, Score as CoreScore};

use crate::verovio::{
    EngraveError, EngraveOptions, Toolkit, default_resource_path, ffi_lock, options_json,
};

pub use clausters_core::notation::{NoteEvent, Page};

/// An editable score over the native engraver -- the type the C ABI hands out.
///
/// Its engraver is the one type an application holds ([`AnyEngraver`]), so the
/// same score a script opened is the one a score editor edits in place.
pub type Score = CoreScore<AnyEngraver>;

impl Engraver for Toolkit {
    /// libverovio has process-wide state, so the guard is the crate's FFI lock
    /// and it covers a whole operation rather than a call. That is the same
    /// discipline [`crate::engrave_svg`] follows, and it is what makes handing a
    /// score to another thread sound.
    type Guard = std::sync::MutexGuard<'static, ()>;

    fn lock(&self) -> Self::Guard {
        ffi_lock()
    }

    fn load_data(&self, data: &str) -> bool {
        Toolkit::load_data(self, data).unwrap_or(false)
    }

    fn render_svg(&self, page: i32) -> String {
        Toolkit::render_svg(self, page)
    }

    fn mei(&self) -> String {
        Toolkit::mei(self, "{}").unwrap_or_default()
    }

    fn edit(&self, action: &str) -> bool {
        Toolkit::edit(self, action).unwrap_or(false)
    }

    fn timemap(&self, options: &str) -> String {
        Toolkit::render_timemap(self, options).unwrap_or_default()
    }

    fn midi_values(&self, xml_id: &str) -> Option<String> {
        Toolkit::midi_values(self, xml_id).ok()
    }

    fn set_options(&self, options: &str) -> bool {
        Toolkit::set_options(self, options).unwrap_or(false)
    }

    fn page_count(&self) -> i32 {
        Toolkit::page_count(self)
    }
}

/// Open `data` (a score in any format verovio auto-detects) on a fresh toolkit
/// and keep the document open. `opts.page` is ignored -- the page to draw is
/// chosen per call in [`Score::display_list`].
///
/// The constructor is here rather than on [`Score`] because building the
/// engraver is exactly what a binding owns: the resource path libverovio needs,
/// and the options JSON it is configured with.
pub fn open(data: &str, opts: &EngraveOptions) -> Result<Score, EngraveError> {
    let options = options_json(opts);
    let resources = opts.resource_path.clone().or_else(default_resource_path);

    let toolkit = {
        let _guard = ffi_lock();
        let tk = Toolkit::new(resources.as_deref())?;
        tk.set_options(&options)?;
        tk
    };
    Score::open(AnyEngraver::new(toolkit), data).ok_or(EngraveError::Load)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// verovio's `keyDown` code for the up arrow (`vrvdef.h`), as the core's
    /// `transpose` sends it -- spelled out here because the raw-hatch test is
    /// what proves the two reach the same editor action.
    const KEY_UP: i32 = 38;
    use clausters_core::notation::{Slot, voice_to_mei};

    /// The two-bar fixture: a C major scale in quarter notes, written by the
    /// core's own encoder. Going in through MEI rather than through typed score
    /// text pins the pitches exactly, so what comes back out of the engraver is
    /// comparable note for note.
    fn phrase() -> String {
        let voice: Vec<Slot> = [60, 62, 64, 65, 67, 69, 71, 72]
            .into_iter()
            .map(|midi| Slot::note(vec![midi], 8)) // a quarter, at 32 ticks to the whole note
            .collect();
        voice_to_mei(&voice, "4/4", "G2", "C")
    }

    fn score() -> Score {
        open(&phrase(), &EngraveOptions::default()).expect("opens")
    }

    /// The ids of the eight noteheads, in draw order.
    fn note_ids(page: &Page) -> Vec<String> {
        page.notes.iter().map(|n| n.id.clone()).collect()
    }

    #[test]
    fn a_page_carries_the_drawing_the_cursors_and_the_notes() {
        let page = score().display_list(1);
        assert!(page.draw.vb[0] > 0.0, "a sized page");
        assert_eq!(page.notes.len(), 8, "eight noteheads across two bars");
        assert_eq!(page.cursors.len(), 8, "one cursor per onset");
        // C major scale from middle C up: MIDI 60..72 diatonically.
        let pitches: Vec<i32> = page.notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![60, 62, 64, 65, 67, 69, 71, 72]);
        // Every cursor lands on a drawn position inside the page.
        for c in &page.cursors {
            assert!(c.x > 0.0 && c.x < page.draw.vb[0], "x within the page");
            assert!(c.y0 < c.y1, "a span, top above bottom");
        }
    }

    #[test]
    fn the_engraved_page_names_what_each_id_is() {
        // the kinds are read off verovio's own SVG, so this is where it is
        // checked that its class names are the ones the table asks about.
        let page = score().display_list(1);
        let kinds = &page.draw.kinds;
        for id in note_ids(&page) {
            assert_eq!(kinds.get(&id).map(String::as_str), Some("note"), "{id}");
        }
        let named: std::collections::BTreeSet<&str> = kinds.values().map(String::as_str).collect();
        for kind in ["staff", "clef", "meterSig", "barLine"] {
            assert!(named.contains(kind), "{kind} in {named:?}");
        }
        assert!(clausters_core::notation::admits("note").pitch);
    }

    /// Two hundred bars of quarters: more than one page of A4 holds.
    fn long_score() -> Score {
        let voice: Vec<Slot> = (0..800).map(|i| Slot::note(vec![60 + i % 12], 8)).collect();
        open(
            &voice_to_mei(&voice, "4/4", "G2", "C"),
            &EngraveOptions::default(),
        )
        .expect("opens")
    }

    #[test]
    fn a_fixed_paper_pages_the_music_and_a_continuous_view_does_not_break_it() {
        use clausters_core::notation::{PageSetup, View, layout_options};

        let mut score = long_score();
        let setup = PageSetup::default();
        assert!(score.relayout(&layout_options(&setup, View::Page)));
        let pages = score.page_count();
        assert!(pages > 1, "a long score takes more than one A4: {pages}");
        let all = score.pages(500.0, true);
        // the paper is the size it was asked to be, whatever the music needs
        assert_eq!(all.draw.vb[0], 21000.0);
        let expected = f64::from(pages) * 29700.0 + f64::from(pages - 1) * 500.0;
        assert_eq!(all.draw.vb[1], expected);
        assert_eq!(
            all.notes.len(),
            800,
            "every page's notes are in the one drawing"
        );
        // a note on a later page is still moved, by a caller that names no page
        let last = all.notes.last().unwrap().id.clone();
        assert!(score.transpose_to_on_any_page(&last, 0));

        assert!(score.relayout(&layout_options(&setup, View::Continuous)));
        assert_eq!(score.page_count(), 1);
        let line = score.pages(0.0, false);
        assert_eq!(
            line.draw.systems.len(),
            1,
            "one system, as long as the music"
        );
        assert!(line.draw.vb[0] > 21000.0);
    }

    #[test]
    fn the_engraver_hands_out_the_outline_of_a_symbol_no_page_holds() {
        let mut score = score();
        let before = score.mei();
        let sheet = score.sheet().cloned();
        // a quarter note with its stem, a sharp, and a codepoint with no glyph
        let found = score.outlines(&["E1D5", "E262", "F8FF"]);
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["E1D5", "E262"]
        );
        assert!(found["E1D5"].starts_with('M') && found["E1D5"].ends_with('z'));
        // and the score is as it was: the document, the model, the page
        assert_eq!(score.mei(), before);
        assert_eq!(score.sheet().cloned(), sheet);
        assert!(!note_ids(&score.display_list(1)).is_empty());
    }

    /// **A system break the writer wrote stands, however short the system it
    /// leaves.** Four bars fit one system of an A4, and a system break written
    /// before the second leaves one bar on the first -- a quarter of the
    /// width, far under what the engraver would call full.
    #[test]
    fn a_written_system_break_stands_on_paper() {
        use clausters_core::notation::{
            PageSetup, Sheet, View, layout_options, sheet_to_mei, voice_to_sheet,
        };

        let voice: Vec<Slot> = (0..16).map(|i| Slot::note(vec![60 + i % 8], 8)).collect();
        let mut sheet: Sheet = voice_to_sheet(&voice, "4/4", "G2", "C");
        let systems = |sheet: &Sheet| {
            let mut score =
                open(&sheet_to_mei(sheet).unwrap(), &EngraveOptions::default()).expect("opens");
            assert!(score.relayout(&layout_options(&PageSetup::default(), View::Page)));
            let page = score.pages(0.0, false);
            let tops: Vec<f64> = page.cursors.iter().map(|c| c.y0).collect();
            (page.draw.systems.len(), tops, score.page_count())
        };
        let (count, tops, pages) = systems(&sheet);
        assert_eq!((count, pages), (1, 1), "four bars fit one system");
        assert!(tops.iter().all(|y| *y == tops[0]));

        sheet.grid.breaks = vec![(1, "system".to_string())];
        let (count, tops, pages) = systems(&sheet);
        assert_eq!((count, pages), (2, 1));
        assert!(tops[..4].iter().all(|y| *y == tops[0]), "the first bar");
        assert!(
            tops[4..].iter().all(|y| *y > tops[0]),
            "the second bar starts the next system: {tops:?}"
        );
    }

    /// **Everything the model grew survives the engraver.** Every edit is
    /// written, loaded and read back from the engraver's own normalized
    /// document, so a mark verovio rewrites in a spelling the reader does not
    /// know is a mark lost on the next edit: each element is put on a score,
    /// opened, and read back out of what verovio hands back.
    #[test]
    fn every_element_survives_the_engravers_round_trip() {
        use clausters_core::notation::{
            Control, Group, Item, Marks, Meter, Sheet, add_spanner, mei_to_sheet, sheet_to_mei,
            stack, voice_to_sheet,
        };
        use clausters_core::ratio::Ratio;

        let quarters = |n: usize| {
            let voice: Vec<Slot> = (0..n)
                .map(|i| Slot::note(vec![60 + (i % 5) as i32], 8))
                .collect();
            let mut sheet = voice_to_sheet(&voice, "4/4", "G2", "C");
            sheet.assign_ids();
            sheet
        };
        let mut sheet: Sheet = stack(quarters(16), &quarters(16), true).unwrap();
        let ids = |sheet: &Sheet, staff: usize| -> Vec<u64> {
            sheet.staves[staff].voices[0]
                .items
                .iter()
                .map(Item::id)
                .collect()
        };
        let (top, low) = (ids(&sheet, 0), ids(&sheet, 1));
        let mark = |sheet: &mut Sheet, id: u64, marks: Marks| {
            for voice in sheet.voices_mut() {
                for item in &mut voice.items {
                    if item.id() == id
                        && let Item::Note { marks: m, .. } = item
                    {
                        *m = marks.clone();
                    }
                }
            }
        };
        mark(
            &mut sheet,
            top[0],
            Marks {
                tremolo: Some(2),
                fingering: Some("3".into()),
                harmony: Some("Cm7".into()),
                lyrics: vec!["Hal-".into(), "lo".into()],
                ..Marks::default()
            },
        );
        mark(
            &mut sheet,
            top[1],
            Marks {
                arpeggio: Some("down".into()),
                breath: Some("breath".into()),
                ring: true,
                ..Marks::default()
            },
        );
        sheet.controls = vec![
            Control {
                kind: "tempo".into(),
                on: top[0],
                text: "Allegro".into(),
                bpm: Some(132.0),
            },
            Control {
                kind: "dir".into(),
                on: low[1],
                text: "dolce".into(),
                bpm: None,
            },
            Control {
                kind: "reh".into(),
                on: top[4],
                text: "A".into(),
                bpm: None,
            },
        ];
        for (kind, from, to) in [
            ("phrase", top[4], top[7]),
            ("gliss", top[5], top[6]),
            ("pedal", low[4], low[7]),
            ("8va", top[8], top[11]),
            ("bracket", low[8], low[10]),
            ("ftrem", low[12], low[13]),
        ] {
            sheet = add_spanner(sheet, kind, from, to).unwrap();
        }
        sheet.grid.keys = vec![(2, "D".into())];
        sheet.grid.meters.push(Meter {
            measure: 3,
            count: 3,
            unit: 4,
        });
        sheet.grid.endings = vec![(1, 1, "1".into()), (2, 2, "2".into())];
        sheet.grid.marks = vec![(0, "segno".into()), (3, "dalsegno".into())];
        sheet.staves[0].label = "Flute".into();
        sheet.staves[1].transpose = -2;
        sheet.staves[1].clefs = vec![(Ratio::from(2), "C3".into())];
        sheet.groups = vec![Group {
            first: 0,
            last: 1,
            symbol: "bracket".into(),
        }];

        let score =
            open(&sheet_to_mei(&sheet).unwrap(), &EngraveOptions::default()).expect("opens");
        let normalized = score.mei();
        let back = mei_to_sheet(&normalized).expect("reads the engraver's document");
        let first = back.staves[0].voices[0].items[0].marks().unwrap().clone();
        assert_eq!(first.tremolo, Some(2), "{normalized}");
        assert_eq!(first.fingering.as_deref(), Some("3"));
        assert_eq!(first.harmony.as_deref(), Some("Cm7"));
        assert_eq!(first.lyrics, vec!["Hal-", "lo"]);
        let second = back.staves[0].voices[0].items[1].marks().unwrap().clone();
        assert_eq!(second.arpeggio.as_deref(), Some("down"));
        assert_eq!(second.breath.as_deref(), Some("breath"));
        assert!(second.ring);
        let mut controls: Vec<(String, String)> = back
            .controls
            .iter()
            .map(|c| (c.kind.clone(), c.text.clone()))
            .collect();
        controls.sort();
        assert_eq!(
            controls,
            vec![
                ("dir".into(), "dolce".into()),
                ("reh".into(), "A".into()),
                ("tempo".into(), "Allegro".into())
            ]
        );
        assert!(back.controls.iter().any(|c| c.bpm == Some(132.0)));
        let mut kinds: Vec<&str> = back.spanners.iter().map(|s| s.kind.as_str()).collect();
        kinds.sort_unstable();
        assert_eq!(
            kinds,
            vec!["8va", "bracket", "ftrem", "gliss", "pedal", "phrase"]
        );
        assert_eq!(back.grid.keys, sheet.grid.keys);
        assert_eq!(back.grid.meters, sheet.grid.meters);
        assert_eq!(back.grid.endings, sheet.grid.endings);
        assert_eq!(back.grid.marks, sheet.grid.marks);
        assert_eq!(back.staves[0].label, "Flute");
        assert_eq!(back.staves[1].transpose, -2);
        assert_eq!(back.staves[1].clefs, sheet.staves[1].clefs);
        assert_eq!(back.groups, sheet.groups);
        assert_eq!(back.len(), sheet.len());
    }

    /// A repeated measure, a repeated beat and a run of empty measures drawn
    /// as one rest come back out of the engraver as what they hold.
    #[test]
    fn the_repeat_signs_and_a_numbered_rest_survive_the_engraver() {
        use clausters_core::notation::{
            Item, Sheet, concat, mei_to_sheet, sheet_to_mei, voice_to_sheet,
        };
        use clausters_core::ratio::Ratio;
        let quarters = |n: usize| {
            let voice: Vec<Slot> = (0..n)
                .map(|i| Slot::note(vec![60 + (i % 5) as i32], 8))
                .collect();
            voice_to_sheet(&voice, "4/4", "G2", "C")
        };
        let mut sheet: Sheet = quarters(8);
        sheet.assign_ids();
        sheet.staves[0].voices[0].items.push(Item::Rest {
            id: 0,
            dur: Ratio::from(2),
        });
        let mut sheet = concat(sheet, &quarters(4)).unwrap();
        sheet.assign_ids();
        // the second beat repeats the first
        let first = sheet.staves[0].voices[0].items[0].clone();
        if let (Item::Note { pitches, marks, .. }, Item::Note { pitches: p, .. }) =
            (&mut sheet.staves[0].voices[0].items[1], &first)
        {
            *pitches = p.clone();
            marks.beat_repeat = true;
        }
        sheet.grid.repeats = vec![1];
        sheet.grid.multirests = true;
        // kept with no run to show it, too
        let mut short = quarters(4);
        short.grid.multirests = true;
        let kept = open(&sheet_to_mei(&short).unwrap(), &EngraveOptions::default()).unwrap();
        assert!(
            mei_to_sheet(&kept.mei()).unwrap().grid.multirests,
            "{}",
            kept.mei()
        );
        let score =
            open(&sheet_to_mei(&sheet).unwrap(), &EngraveOptions::default()).expect("opens");
        let normalized = score.mei();
        let back = mei_to_sheet(&normalized).expect("reads it");
        assert_eq!(back.len(), sheet.len(), "{normalized}");
        assert_eq!(back.grid.repeats, vec![1]);
        assert!(back.grid.multirests);
        let second = &back.staves[0].voices[0].items[1];
        assert!(second.marks().is_some_and(|m| m.beat_repeat));
        assert_eq!(second.pitches(), first.pitches());
    }

    /// **A written page break turns the page**, which the engraver does only
    /// where the paper is full: four bars on one page, then a break before the
    /// third puts it on a second, numbered 2 -- and the cursors still run over
    /// all four bars, read from the whole score.
    #[test]
    fn a_written_page_break_turns_the_page() {
        use clausters_core::notation::{
            Header, PAGE_NUMBER, PageSetup, Prim, Sheet, View, layout_options, sheet_to_mei,
            voice_to_sheet,
        };

        let voice: Vec<Slot> = (0..16).map(|i| Slot::note(vec![60 + i % 8], 8)).collect();
        let mut sheet: Sheet = voice_to_sheet(&voice, "4/4", "G2", "C");
        sheet.header = Header {
            title: "A title".into(),
            ..Header::default()
        };
        let laid = |sheet: &Sheet| {
            let mut score =
                open(&sheet_to_mei(sheet).unwrap(), &EngraveOptions::default()).expect("opens");
            assert!(score.relayout(&layout_options(&PageSetup::default(), View::Page)));
            score.pages(1000.0, true)
        };
        let one = laid(&sheet);
        let texts = |page: &clausters_core::notation::Page| -> Vec<(String, Option<String>)> {
            page.draw
                .prims
                .iter()
                .filter_map(|p| match p {
                    Prim::Text { s, id, .. } => Some((s.clone(), id.clone())),
                    _ => None,
                })
                .collect()
        };
        assert!(
            !texts(&one)
                .iter()
                .any(|(_, id)| id.as_deref() == Some(PAGE_NUMBER)),
            "one page has no number"
        );

        sheet.grid.breaks = vec![(2, "page".to_string())];
        let two = laid(&sheet);
        assert!(
            two.draw.vb[1] > 1.9 * one.draw.vb[1],
            "two pages, one under the other"
        );
        let numbers: Vec<String> = texts(&two)
            .into_iter()
            .filter(|(_, id)| id.as_deref() == Some(PAGE_NUMBER))
            .map(|(s, _)| s)
            .collect();
        assert_eq!(numbers, vec!["2".to_string()], "the second page says it is");
        let titles = texts(&two).iter().filter(|(s, _)| s == "A title").count();
        assert_eq!(titles, 1, "and the title is the first page's alone");
        assert_eq!(
            two.cursors.len(),
            one.cursors.len(),
            "every note is still timed"
        );
    }

    /// How many elements of `kind` a page draws.
    fn drawn(page: &Page, kind: &str) -> usize {
        page.draw.kinds.values().filter(|k| *k == kind).count()
    }

    /// A slur and a hairpin across a written page break are drawn on both
    /// pages, each run of pages holding the part that is its own: the
    /// engraver matches an end written as a beat of the run's last measure,
    /// and one written before the first beat of the run's first.
    #[test]
    fn a_line_across_a_page_break_is_drawn_on_both_pages() {
        use clausters_core::notation::{
            PageSetup, Sheet, View, add_spanner, layout_options, sheet_to_mei, voice_to_sheet,
        };

        let voice: Vec<Slot> = (0..16).map(|i| Slot::note(vec![60 + i % 8], 8)).collect();
        let mut sheet: Sheet = voice_to_sheet(&voice, "4/4", "G2", "C");
        sheet = add_spanner(sheet, "slur", 6, 11).unwrap();
        sheet = add_spanner(sheet, "crescendo", 6, 11).unwrap();
        let laid = |sheet: &Sheet| {
            let mut score =
                open(&sheet_to_mei(sheet).unwrap(), &EngraveOptions::default()).expect("opens");
            assert!(score.relayout(&layout_options(&PageSetup::default(), View::Page)));
            score.pages(1000.0, true)
        };
        let one = laid(&sheet);
        assert_eq!((drawn(&one, "slur"), drawn(&one, "hairpin")), (1, 1));
        sheet.grid.breaks = vec![(2, "page".to_string())];
        let two = laid(&sheet);
        assert_eq!((drawn(&two, "slur"), drawn(&two, "hairpin")), (2, 2));
    }

    /// A slur into a measure drawn as a repeat, and a hairpin out of it, are
    /// drawn: their ends there are beats of the measure, which the engraver
    /// places under the sign.
    #[test]
    fn a_line_into_a_measure_drawn_as_a_repeat_is_drawn() {
        use clausters_core::notation::{Sheet, add_spanner, sheet_to_mei, voice_to_sheet};

        let voice: Vec<Slot> = (0..12).map(|i| Slot::note(vec![60 + i % 8], 8)).collect();
        let mut sheet: Sheet = voice_to_sheet(&voice, "4/4", "G2", "C");
        sheet.grid.repeats = vec![1];
        sheet = add_spanner(sheet, "slur", 2, 6).unwrap();
        sheet = add_spanner(sheet, "crescendo", 7, 10).unwrap();
        let mut score =
            open(&sheet_to_mei(&sheet).unwrap(), &EngraveOptions::default()).expect("opens");
        let page = score.display_list(1);
        assert_eq!(drawn(&page, "mRpt"), 1);
        assert_eq!((drawn(&page, "slur"), drawn(&page, "hairpin")), (1, 1));
        // and the engraver's own document still holds both
        let back = clausters_core::notation::mei_to_sheet(&score.mei()).unwrap();
        let kinds: Vec<&str> = back.spanners.iter().map(|s| s.kind.as_str()).collect();
        assert_eq!(kinds, ["slur", "crescendo"]);
    }

    /// Let it ring is drawn wherever it is written: the engraver draws one
    /// only when it ends inside its own measure, which is how it is written
    /// -- on the last note of a measure, on the last of the score, on each
    /// notehead of a chord, and after a tie across a barline.
    #[test]
    fn let_it_ring_is_drawn_wherever_it_is_written() {
        use clausters_core::notation::{Item, Sheet, mei_to_sheet, sheet_to_mei, voice_to_sheet};

        let mut voice: Vec<Slot> = (0..8).map(|i| Slot::note(vec![60 + i], 8)).collect();
        voice[2] = Slot::note(vec![60, 64], 8);
        // the last quarter of the second bar, held a bar longer
        voice[7] = Slot::note(vec![67], 40);
        let mut sheet: Sheet = voice_to_sheet(&voice, "4/4", "G2", "C");
        // the chord, the last note of the first bar, and the tied one
        let rung = [3u64, 4, 8];
        for item in &mut sheet.staves[0].voices[0].items {
            if let Item::Note { id, marks, .. } = item
                && rung.contains(id)
            {
                marks.ring = true;
            }
        }
        let mut score =
            open(&sheet_to_mei(&sheet).unwrap(), &EngraveOptions::default()).expect("opens");
        let page = score.display_list(1);
        assert_eq!(
            drawn(&page, "lv"),
            4,
            "two of the chord, and one of each note"
        );
        let back = mei_to_sheet(&score.mei()).unwrap();
        let rings: Vec<u64> = back.staves[0].voices[0]
            .items
            .iter()
            .filter(|i| i.marks().is_some_and(|m| m.ring))
            .map(Item::id)
            .collect();
        assert_eq!(rings, rung);
    }

    #[test]
    fn the_page_text_is_drawn_in_its_cells_and_survives_the_engraver() {
        use clausters_core::notation::{
            Halign, Header, PageSetup, Place, Prim, Sheet, View, default_place, layout_options,
            sheet_to_mei, voice_to_sheet,
        };

        let voice: Vec<Slot> = (0..8).map(|i| Slot::note(vec![60 + i], 8)).collect();
        let mut sheet: Sheet = voice_to_sheet(&voice, "4/4", "G2", "C");
        sheet.header = Header {
            title: "A title".into(),
            composer: "A. Composer".into(),
            lyricist: "words by C".into(),
            copyright: "(c) somebody".into(),
            notes: vec!["* a footnote".into()],
            ..Header::default()
        };
        sheet.header.places.insert(
            "lyricist".into(),
            Place {
                halign: Halign::Right,
                ..default_place("lyricist")
            },
        );
        let mut score = open(&sheet_to_mei(&sheet).unwrap(), &EngraveOptions::default()).unwrap();
        assert!(score.relayout(&layout_options(&PageSetup::default(), View::Page)));
        let page = score.pages(0.0, false);
        let text = |id: &str| {
            page.draw.prims.iter().find_map(|p| match p {
                Prim::Text {
                    s,
                    x,
                    y,
                    id: Some(i),
                    ..
                } if i == id => Some((s.clone(), *x, *y)),
                _ => None,
            })
        };
        let (title, title_x, _) = text("t-title").expect("the title is drawn under its id");
        assert_eq!(title, "A title");
        assert!(
            (title_x - 10500.0).abs() < 1.0,
            "centred on an A4: {title_x}"
        );
        // the composer on the right; the words' author moved there too
        let (_, composer_x, _) = text("t-composer").expect("the composer");
        let (_, lyricist_x, _) = text("t-lyricist").expect("the lyricist");
        assert!(composer_x > 15000.0 && lyricist_x > 15000.0);
        // the foot is at the foot of the page
        let (_, _, copyright_y) = text("t-copyright").expect("the copyright");
        let (_, _, note_y) = text("t-note-1").expect("the footnote");
        assert!(copyright_y > 25000.0 && note_y > 25000.0);
        assert_eq!(
            page.draw.kinds.get("t-title").map(String::as_str),
            Some("rend")
        );
        // and what the engraver holds reads back as the header it was written from
        assert_eq!(score.sheet().expect("a model").header, sheet.header);
    }

    #[test]
    fn the_cursor_times_follow_the_note_onsets() {
        let page = score().display_list(1);
        let onsets: Vec<f64> = page.notes.iter().map(|n| n.t).collect();
        let cursors: Vec<f64> = page.cursors.iter().map(|c| c.t).collect();
        assert_eq!(onsets, cursors, "one cursor per onset, same times");
    }

    #[test]
    fn transposing_a_note_moves_only_that_note() {
        let mut sc = score();
        let before = sc.display_list(1);
        let ids = note_ids(&before);
        assert!(sc.transpose(&ids[0], 1), "verovio accepts the step");
        let after = sc.display_list(1);
        let pitches: Vec<i32> = after.notes.iter().map(|n| n.pitch).collect();
        assert_eq!(
            pitches,
            vec![62, 62, 64, 65, 67, 69, 71, 72],
            "the first note is a step up, the rest untouched"
        );
    }

    /// The absolute form lands on the position it names, whatever the note's
    /// own is -- the property that lets an edit cross a wire.
    #[test]
    fn transposing_to_a_position_lands_on_it() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        let target = sc.display_list(1).draw.staff_position(&ids[0]).unwrap() + 2;
        assert!(sc.transpose_to(&ids[0], target, 1), "verovio accepts it");
        assert_eq!(
            sc.display_list(1).draw.staff_position(&ids[0]),
            Some(target),
            "the note is at the position that was named"
        );
    }

    /// The acceptance the absolute form exists for: a resend must be harmless.
    /// A relative payload applied twice would move the note twice.
    #[test]
    fn transposing_to_the_same_position_twice_moves_the_note_once() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        let target = sc.display_list(1).draw.staff_position(&ids[0]).unwrap() + 3;
        assert!(sc.transpose_to(&ids[0], target, 1));
        let once: Vec<i32> = sc.display_list(1).notes.iter().map(|n| n.pitch).collect();

        assert!(
            sc.transpose_to(&ids[0], target, 1),
            "already there is success, not a refusal -- the requested state holds"
        );
        let twice: Vec<i32> = sc.display_list(1).notes.iter().map(|n| n.pitch).collect();
        assert_eq!(once, twice, "the second application changed nothing");
    }

    #[test]
    fn an_unplaceable_transpose_target_is_refused_rather_than_guessed() {
        let mut sc = score();
        assert!(!sc.transpose_to("no-such-id", 0, 1));
    }

    #[test]
    fn an_edit_is_one_undo_step_and_comes_back() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        assert!(!sc.can_undo(), "a fresh score has nothing to undo");
        sc.transpose(&ids[0], 2);
        assert!(sc.can_undo() && !sc.can_redo());

        assert!(sc.undo(), "steps back");
        let pitches: Vec<i32> = sc.display_list(1).notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches[0], 60, "back to the original pitch");
        assert!(!sc.can_undo() && sc.can_redo());

        assert!(sc.redo(), "steps forward again");
        let pitches: Vec<i32> = sc.display_list(1).notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches[0], 64, "two diatonic steps up from C is E");
    }

    #[test]
    fn undo_and_redo_on_an_empty_stack_are_false_not_a_crash() {
        let mut sc = score();
        assert!(!sc.undo());
        assert!(!sc.redo());
    }

    #[test]
    fn a_new_edit_drops_the_redo_branch() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        sc.transpose(&ids[0], 1);
        sc.undo();
        assert!(sc.can_redo());
        sc.transpose(&ids[0], -1);
        assert!(!sc.can_redo(), "the undone branch is gone");
    }

    #[test]
    fn transposing_by_zero_steps_is_not_an_edit() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        assert!(!sc.transpose(&ids[0], 0));
        assert!(!sc.can_undo(), "nothing was pushed");
    }

    #[test]
    fn a_rejected_edit_leaves_the_score_untouched() {
        let mut sc = score();
        let before = sc.mei();
        assert!(
            !sc.transpose("no-such-element", 1),
            "verovio rejects an unknown id"
        );
        assert!(!sc.can_undo(), "a rejected edit is not an undo step");
        assert_eq!(sc.mei(), before, "rolled back to the snapshot");
    }

    #[test]
    fn the_raw_edit_hatch_reaches_the_editor() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        let param = format!(r#"{{"elementId": "{}", "key": {}}}"#, ids[0], KEY_UP);
        assert!(
            sc.edit("keyDown", &param),
            "the same action transpose sends"
        );
        let pitches: Vec<i32> = sc.display_list(1).notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches[0], 62);
    }

    #[test]
    fn two_edits_in_a_row_survive_the_reload() {
        // Every edit reloads, and editing an undrawn document segfaults, so this
        // is the case `ensure_drawn` exists for.
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        assert!(sc.transpose(&ids[0], 1));
        assert!(sc.transpose(&ids[1], 1));
        let pitches: Vec<i32> = sc.display_list(1).notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches[..2], [62, 64]);
    }

    #[test]
    fn the_mei_round_trips_through_a_second_score() {
        let mut sc = score();
        let ids = note_ids(&sc.display_list(1));
        sc.transpose(&ids[0], 1);
        let mei = sc.mei();
        let mut reopened = open(&mei, &EngraveOptions::default()).expect("MEI reloads");
        let pitches: Vec<i32> = reopened
            .display_list(1)
            .notes
            .iter()
            .map(|n| n.pitch)
            .collect();
        assert_eq!(pitches[0], 62, "the edit persisted through MEI");
    }

    #[test]
    fn a_page_serializes_to_the_layers_a_client_sends() {
        let json = serde_json::to_value(score().display_list(1)).expect("serializes");
        for key in ["vb", "glyphs", "prims", "step", "cursors", "notes"] {
            assert!(json.get(key).is_some(), "the `{key}` layer");
        }
    }
}
