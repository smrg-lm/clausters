//! **Text shaped before it is drawn**: a string turned into the glyphs that
//! draw it and where each one stands -- what a font's layout tables say and a
//! character-by-character walk cannot.
//!
//! A run of text is not a row of its characters. Arabic joins each letter to
//! its neighbours in a form that depends on them; a combining accent stands
//! over its base and not where the base's advance left the pen; Devanagari
//! writes a vowel sign before the consonant it follows; Latin kerns and
//! ligates; and a right-to-left run reads from the right. So a string goes
//! through three steps before a glyph is placed:
//!
//! 1. **Direction.** The Unicode bidirectional algorithm (`unicode-bidi`)
//!    splits the paragraph into visual runs, each left to right or right to
//!    left, in the order they stand on the line.
//! 2. **Faces.** Each run is cut where the face that draws it changes: a
//!    grapheme cluster -- one perceived character, however many code points
//!    write it -- is drawn by the first face that has every character of it
//!    ([`Atlas::covers`]), so a mark stays with its base. A symbol the chrome
//!    draws as a shape, or a character a window sent the outline of, is a
//!    piece of its own, drawn as it always was.
//! 3. **Shaping.** Each piece goes through the shaper (`rustybuzz`) in its
//!    run's direction, which answers glyph ids, advances and offsets.
//!
//! What comes out ([`Shaped`]) is in **visual order**, every item with its pen
//! position and the byte of the string its cluster starts at, so the drawing,
//! the width and the cluster a caret or a cut lands on all read one thing.
//! **What is measured is what is drawn**: [`super::width`] is a shaped run's
//! width, never a sum of characters.
//!
//! Shaping is display only: nothing here reaches the wire or the core.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use unicode_bidi::BidiInfo;
use unicode_segmentation::UnicodeSegmentation;

use super::atlas::{self, Atlas};
use super::{ADVANCE, outline, symbol};

/// One placed piece of a shaped run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    /// What draws it.
    pub kind: Kind,
    /// The pen, in pixels from the run's left edge, where the item starts.
    pub pen: f32,
    /// Where its glyph stands off the pen: right, and **up** from the
    /// baseline (a mark over its base is raised).
    pub offset: [f32; 2],
    /// How far the pen moves past it.
    pub advance: f32,
    /// The byte of the string the cluster it belongs to starts at.
    pub cluster: usize,
    /// Whether its run reads right to left.
    pub rtl: bool,
}

/// What draws an [`Item`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Glyph `id` of face `face` ([`Atlas::glyph_id`]).
    Glyph { face: u8, id: u16 },
    /// A character drawn as it always was: a symbol the chrome draws as a
    /// shape, or one a window sent the outline of.
    Char(char),
}

/// **A string, shaped**: its items in visual order, left to right on the
/// line, and the width they take.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shaped {
    pub items: Vec<Item>,
    pub width: f32,
}

/// How many shaped strings a thread keeps before it starts again: an
/// interface draws a few hundred, and a document that types thousands of
/// distinct ones is not kept whole.
const KEPT: usize = 4096;

/// The kept runs: by a hash of the string and the size, each beside the
/// string it was shaped from, so a lookup builds nothing.
type Kept = HashMap<(u64, u32), Vec<(Box<str>, Rc<Shaped>)>>;

thread_local! {
    /// The strings shaped at each size, kept until what they were shaped
    /// with changes ([`super::generation`]).
    static SHAPED: RefCell<(u64, usize, Kept)> = RefCell::new((u64::MAX, 0, HashMap::new()));
}

/// **`s` shaped at `scale`**, with the faces the atlas holds: kept once
/// asked, so a frame that measures and draws a label shapes it once. With no
/// face loaded, empty.
pub fn shape(s: &str, scale: f32) -> Rc<Shaped> {
    use std::hash::{Hash, Hasher};
    let now = super::generation();
    let key = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut hasher);
        (hasher.finish(), scale.to_bits())
    };
    let kept = SHAPED.with(|kept| {
        let mut kept = kept.borrow_mut();
        if kept.0 != now {
            *kept = (now, 0, HashMap::new());
        }
        kept.2
            .get(&key)
            .and_then(|same| same.iter().find(|(text, _)| &**text == s))
            .map(|(_, shaped)| shaped.clone())
    });
    if let Some(shaped) = kept {
        return shaped;
    }
    let shaped = Rc::new(atlas::with(|a| shape_with(a, s, scale)));
    SHAPED.with(|kept| {
        let mut kept = kept.borrow_mut();
        if kept.1 >= KEPT {
            kept.1 = 0;
            kept.2.clear();
        }
        kept.1 += 1;
        kept.2
            .entry(key)
            .or_default()
            .push((s.into(), shaped.clone()));
    });
    shaped
}

/// What a piece of a run is drawn with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Piece {
    Face(usize),
    Char(char),
}

/// [`shape`], against `atlas`.
fn shape_with(atlas: &Atlas, s: &str, scale: f32) -> Shaped {
    let mut out = Shaped::default();
    if s.is_empty() || atlas.faces() == 0 {
        return out;
    }
    let data: Vec<std::sync::Arc<[u8]>> =
        (0..atlas.faces()).filter_map(|i| atlas.data(i)).collect();
    let parsed: Vec<Option<rustybuzz::Face<'_>>> = data
        .iter()
        .map(|bytes| rustybuzz::Face::from_slice(bytes, 0))
        .collect();
    let info = BidiInfo::new(s, None);
    let mut pen = 0.0f32;
    for paragraph in &info.paragraphs {
        let line = paragraph.range.clone();
        let (levels, runs) = info.visual_runs(paragraph, line);
        for run in runs {
            let rtl = levels[run.start].is_rtl();
            // the run cut where what draws it changes, each grapheme by the
            // first face that has all of it
            let mut pieces: Vec<(Piece, std::ops::Range<usize>)> = Vec::new();
            for (at, grapheme) in s[run.clone()].grapheme_indices(true) {
                let at = run.start + at;
                let piece = piece_of(atlas, grapheme);
                match pieces.last_mut() {
                    Some((was, range)) if *was == piece && matches!(piece, Piece::Face(_)) => {
                        range.end = at + grapheme.len();
                    }
                    _ => pieces.push((piece, at..at + grapheme.len())),
                }
            }
            // a right-to-left run's pieces stand right to left on the line
            if rtl {
                pieces.reverse();
            }
            for (piece, range) in pieces {
                match piece {
                    Piece::Char(c) => {
                        let advance = char_advance(c, scale);
                        out.items.push(Item {
                            kind: Kind::Char(c),
                            pen,
                            offset: [0.0, 0.0],
                            advance,
                            cluster: range.start,
                            rtl,
                        });
                        pen += advance;
                    }
                    Piece::Face(face) => {
                        let Some(Some(rb)) = parsed.get(face) else {
                            continue;
                        };
                        let to_px = atlas.units_to_px(face, scale);
                        let mut buffer = rustybuzz::UnicodeBuffer::new();
                        buffer.push_str(&s[range.clone()]);
                        buffer.set_direction(if rtl {
                            rustybuzz::Direction::RightToLeft
                        } else {
                            rustybuzz::Direction::LeftToRight
                        });
                        buffer.guess_segment_properties();
                        let glyphs = rustybuzz::shape(rb, &[], buffer);
                        for (info, at) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions())
                        {
                            let advance = at.x_advance as f32 * to_px;
                            out.items.push(Item {
                                kind: Kind::Glyph {
                                    face: face as u8,
                                    id: info.glyph_id as u16,
                                },
                                pen,
                                offset: [at.x_offset as f32 * to_px, at.y_offset as f32 * to_px],
                                advance,
                                cluster: range.start + info.cluster as usize,
                                rtl,
                            });
                            pen += advance;
                        }
                    }
                }
            }
        }
    }
    out.width = pen;
    out
}

/// **What draws one grapheme**: a symbol or a character a window sent the
/// outline of, alone, is drawn as it always was; anything else by the first
/// face that has every character of it, or by the first that has its first.
fn piece_of(atlas: &Atlas, grapheme: &str) -> Piece {
    let mut chars = grapheme.chars();
    let first = chars.next().unwrap_or(' ');
    if chars.next().is_none() && (symbol::ALL.contains(&first) || outline::has(first)) {
        return Piece::Char(first);
    }
    // the joiners and selectors a face need not have do not decide it
    let decides = |c: char| !matches!(c, '\u{200C}' | '\u{200D}' | '\u{FE00}'..='\u{FE0F}');
    let faces = atlas.faces();
    (0..faces)
        .find(|&face| {
            grapheme
                .chars()
                .filter(|&c| decides(c))
                .all(|c| atlas.covers(face, c))
        })
        .or_else(|| (0..faces).find(|&face| atlas.covers(face, first)))
        .map_or(Piece::Face(0), Piece::Face)
}

/// The advance of a character drawn as it always was.
fn char_advance(c: char, scale: f32) -> f32 {
    if symbol::ALL.contains(&c) {
        return ADVANCE as f32 * scale;
    }
    outline::advance(c, scale).unwrap_or(ADVANCE as f32 * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `s` shaped at a size with the faces the host ships, and the glyph id
    /// face `face` has for `c`.
    fn shaped(s: &str) -> Rc<Shaped> {
        assert!(atlas::set_embedded());
        shape(s, 2.0)
    }

    fn id_of(face: usize, c: char) -> u16 {
        let bytes = [atlas::EMBEDDED, atlas::DEVANAGARI][face];
        let parsed = rustybuzz::Face::from_slice(bytes, 0).expect("a face");
        parsed.glyph_index(c).expect("the face draws it").0
    }

    fn glyphs(run: &Shaped) -> Vec<(u8, u16)> {
        run.items
            .iter()
            .filter_map(|item| match item.kind {
                Kind::Glyph { face, id } => Some((face, id)),
                Kind::Char(_) => None,
            })
            .collect()
    }

    /// **A ligature is one glyph**: `fi` in the host's face is the face's own
    /// ligature, and it is measured as that one glyph.
    #[test]
    fn a_ligature_is_the_face_s_one_glyph() {
        let run = shaped("fi");
        assert_eq!(glyphs(&run), vec![(0, id_of(0, '\u{FB01}'))]);
        assert_eq!(run.width, run.items[0].advance);
    }

    /// **Arabic joins**: the three letters of a word take their initial,
    /// medial and final forms, none of them the letter standing alone, and the
    /// word reads from the right -- its first letter is the last item on the
    /// line.
    #[test]
    fn an_arabic_word_joins_and_reads_from_the_right() {
        let word = "\u{0628}\u{0633}\u{0645}";
        let run = shaped(word);
        let ids: Vec<u16> = glyphs(&run).into_iter().map(|(_, id)| id).collect();
        assert_eq!(ids.len(), 3, "{run:?}");
        let alone: Vec<u16> = word.chars().map(|c| id_of(0, c)).collect();
        for id in &ids {
            assert!(
                !alone.contains(id),
                "a joined form, not the isolated one: {ids:?}"
            );
        }
        assert!(run.items.iter().all(|item| item.rtl));
        assert_eq!(
            run.items.last().unwrap().cluster,
            0,
            "the first letter on the right"
        );
        assert_eq!(run.items[0].cluster, word.len() - 2, "the last on the left");
    }

    /// **A mark stands over its base**: a combining acute after a letter that
    /// has no precomposed form with it is a second glyph that does not move
    /// the pen, drawn over the base's own width.
    #[test]
    fn a_mark_stands_over_its_base() {
        let run = shaped("q\u{0301}");
        assert_eq!(
            glyphs(&run),
            vec![(0, id_of(0, 'q')), (0, id_of(0, '\u{0301}'))]
        );
        let (base, mark) = (run.items[0], run.items[1]);
        assert_eq!(mark.advance, 0.0, "a mark advances nothing");
        let at = mark.pen + mark.offset[0];
        assert!(
            at >= base.pen - base.advance && at <= base.pen + base.advance,
            "over the base: {base:?} {mark:?}"
        );
        assert_eq!(mark.cluster, 0, "one cluster with its base");
        assert_eq!(run.width, base.advance);
    }

    /// **Devanagari is drawn by the face that has it, and reordered**: the
    /// vowel sign i is written after its consonant and drawn before it, both
    /// glyphs from the Devanagari face the host falls back to.
    #[test]
    fn a_devanagari_vowel_sign_is_drawn_before_its_consonant() {
        let run = shaped("\u{0915}\u{093F}");
        let faces: Vec<u8> = glyphs(&run).iter().map(|(face, _)| *face).collect();
        assert!(
            faces
                .iter()
                .all(|&face| face as usize == atlas::with(|a| a.faces()) - 1)
        );
        let ids: Vec<u16> = glyphs(&run).into_iter().map(|(_, id)| id).collect();
        assert_eq!(ids.len(), 2, "{run:?}");
        assert_eq!(ids[1], id_of(1, '\u{0915}'), "the consonant second");
        assert_ne!(ids[0], id_of(1, '\u{0915}'), "the sign first");
    }

    /// **A mixed line is in visual order**: Latin, then a Hebrew word read
    /// from the right, then Latin -- the Hebrew word's clusters fall from left
    /// to right, everything else rises, and the width is the items'.
    #[test]
    fn a_mixed_line_stands_in_visual_order() {
        let line = "ab \u{05E9}\u{05DC}\u{05D5}\u{05DD} cd";
        let run = shaped(line);
        let hebrew: Vec<usize> = run
            .items
            .iter()
            .filter(|item| item.rtl)
            .map(|item| item.cluster)
            .collect();
        assert!(
            hebrew.windows(2).all(|pair| pair[0] > pair[1]),
            "{hebrew:?}"
        );
        let latin: Vec<usize> = run
            .items
            .iter()
            .filter(|item| !item.rtl)
            .map(|item| item.cluster)
            .collect();
        assert!(latin.windows(2).all(|pair| pair[0] < pair[1]), "{latin:?}");
        let pens: Vec<f32> = run.items.iter().map(|item| item.pen).collect();
        assert!(pens.windows(2).all(|pair| pair[0] <= pair[1]));
        let sum: f32 = run.items.iter().map(|item| item.advance).sum();
        assert!((run.width - sum).abs() < 1e-3);
    }

    /// **A symbol is drawn as the chrome draws it**, in the bitmap's cell,
    /// whatever face is loaded.
    #[test]
    fn a_symbol_stays_a_shape() {
        let run = shaped(&format!("a{}b", super::symbol::PLAY));
        assert_eq!(run.items[1].kind, Kind::Char(super::symbol::PLAY));
        assert_eq!(run.items[1].advance, ADVANCE as f32 * 2.0);
    }
}
