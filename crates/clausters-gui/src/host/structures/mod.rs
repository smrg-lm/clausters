//! **The structures a hand edits**, and the verbs over them -- apart from what
//! draws them.
//!
//! A note, a box on a time axis, a break-point: each is a shape with a list
//! behind it and a handful of verbs that act on a selection of one. They were
//! written under [`graphics`](super::graphics), beside the code that draws
//! them, because that is where the first one needed them -- so the module whose
//! name says *drawing* held the model, its edits and its picture, in that order
//! of importance and in the wrong place. Nobody looking for where a note is cut
//! looked under `graphics`.
//!
//! # Why it is worth a layer of its own
//!
//! Three applications are being built over one document -- an audio editor, a
//! multitrack editor, a score editor -- and **the structures are the part they
//! share**. What differs between them is the picture and the hand's vocabulary;
//! what does not is that a note is a note and a box is a box. A layer that
//! names them is what makes the next application a composition of things that
//! already exist rather than a fourth copy of them.
//!
//! The boundary is stated negatively, which is what makes it worth separating:
//! **nothing here names a `Rect`, a `View`, a `Draw` or a `Metrics`.** A module
//! that took one would be a drawing again. The mappings between a structure and
//! the pixels it is drawn at stay with the renderer that chose them
//! ([`graphics::pianoroll`](super::graphics::pianoroll) maps a pitch to a row,
//! [`interact::coords`](super::interact) maps a cursor to a sample), and both
//! hand the *numbers* here -- the same rule [`boxes`] has stated since it was
//! the only module in this layer.
//!
//! # What is in it
//!
//! - [`boxes`] -- **a box on a time axis**, which a clip and a note both are: a
//!   span on a row, grabbed by one of three parts, snapped, bounded. The
//!   geometry, and the verbs over a selection of them (cut, drop, put down).
//! - [`notes`] -- **a note**, and the list verbs that are a roll's: insert,
//!   remove, move, resize, velocity, and the clipboard block.
//! - [`clips`] -- **a box with contents on a lane**, plus the lane and the
//!   curve: what a multitrack is made of, and the payloads each list reports as.
//! - [`points`] -- **a break-point**, the shape of an envelope and of an
//!   automation alike: where the points are, what the curve is worth between
//!   two of them, and the edits that move one.

pub mod boxes;
pub mod clips;
pub mod notes;
pub mod points;

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// The drawing's names this layer may not take, and a path into the
    /// drawing itself.
    const DRAWING: [&str; 5] = ["Rect", "View", "Draw", "Metrics", "graphics::"];

    /// Whether `word` stands in `line` as a name of its own -- not inside a
    /// longer one (`TrackView`, `Viewport`).
    fn names(line: &str, word: &str) -> bool {
        if word.ends_with("::") {
            return line.contains(word);
        }
        line.match_indices(word).any(|(at, _)| {
            let ident = |c: char| c.is_alphanumeric() || c == '_';
            let before = line[..at].chars().next_back().is_none_or(|c| !ident(c));
            let after = line[at + word.len()..]
                .chars()
                .next()
                .is_none_or(|c| !ident(c));
            before && after
        })
    }

    /// **The boundary is a rule, so it is checked**: nothing under
    /// `structures` names what draws -- the module docs' one sentence, held
    /// by reading the code (comments aside, which may point at the drawing
    /// to say where a mapping lives, and tests aside). A file that took one
    /// prints where.
    #[test]
    fn no_structure_names_what_draws_it() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/host/structures");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("the layer's directory") {
            let path = entry.expect("an entry").path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("a source file");
            // the tests are not the layer, and this one spells the names
            let layer = source.split("#[cfg(test)]").next().unwrap_or_default();
            for (n, line) in layer.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                for word in DRAWING {
                    if names(code, word) {
                        let file = path.file_name().unwrap_or_default().to_string_lossy();
                        found.push(format!("{file}:{}: {word}", n + 1));
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "a structure names the drawing:\n{}",
            found.join("\n")
        );
    }

    /// The reader finds a name standing alone and not inside a longer one.
    #[test]
    fn a_name_is_found_alone_and_not_inside_another() {
        assert!(names("fn at(r: Rect) {", "Rect"));
        assert!(names("use super::graphics::meters;", "graphics::"));
        assert!(!names("let v: TrackView = x;", "View"));
        assert!(!names("let v = Viewport::new();", "View"));
    }
}
