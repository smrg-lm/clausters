//! **A music font's outlines, asked of the engraver.**
//!
//! An application that shows a music symbol outside a page -- a tool's icon, a
//! palette's entry -- needs the shape of that symbol, and the shapes are the
//! engraver's: they are the font it draws every page with. It has no door that
//! hands one out, but it draws any codepoint it is asked to as a notehead, so a
//! **specimen** -- a measure holding one note per codepoint, each with that
//! codepoint as its head -- comes back with every one of them in the glyph
//! table a page always carries, as the same path the page would draw.
//!
//! That is what [`specimen`] writes and what [`Score::outlines`] reads: the
//! same call under a native engraver and one compiled to wasm, since it goes
//! through loading and drawing a document and nothing else.
//!
//! A codepoint is named as SMuFL writes it: four hex digits (`E1D5`), which is
//! also the key of the table that comes back.
//!
//! [`Score::outlines`]: super::Score::outlines

/// Whether `code` names a codepoint: hex digits, and no more than a scalar
/// value has.
fn is_code(code: &str) -> bool {
    (4..=6).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_hexdigit())
}

/// **The specimen for `codes`**: an MEI document of one measure, with one
/// quarter note per codepoint whose head is that glyph. A name that is no
/// codepoint is left out, so nothing a caller passes reaches the markup
/// unread.
#[must_use]
pub fn specimen(codes: &[&str]) -> String {
    let notes: String = codes
        .iter()
        .filter(|code| is_code(code))
        .map(|code| {
            format!(
                r##"<note pname="b" oct="4" dur="4" stem.visible="false" head.auth="smufl" head.shape="#x{code}"/>"##
            )
        })
        .collect();
    format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            r#"<mei xmlns="http://www.music-encoding.org/ns/mei" meiversion="5.0">"#,
            "<meiHead><fileDesc><titleStmt><title/></titleStmt><pubStmt/></fileDesc></meiHead>",
            "<music><body><mdiv><score><scoreDef><staffGrp>",
            r#"<staffDef n="1" lines="5" clef.shape="G" clef.line="2"/>"#,
            "</staffGrp></scoreDef><section>",
            r#"<measure n="1"><staff n="1"><layer n="1">{notes}</layer></staff></measure>"#,
            "</section></score></mdiv></body></music></mei>",
        ),
        notes = notes
    )
}

/// The character a codepoint names, for a label that shows the glyph.
#[must_use]
pub fn glyph_char(code: &str) -> Option<char> {
    if !is_code(code) {
        return None;
    }
    u32::from_str_radix(code, 16).ok().and_then(char::from_u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_specimen_holds_one_note_per_codepoint_and_nothing_unread() {
        let mei = specimen(&["E1D5", "e262", r#"E1D7"/><script"#, "E1"]);
        assert_eq!(mei.matches("<note ").count(), 2);
        assert!(mei.contains(r##"head.shape="#xE1D5""##));
        assert!(mei.contains(r##"head.shape="#xe262""##));
        assert!(!mei.contains("script"));
    }

    #[test]
    fn a_codepoint_names_its_character() {
        assert_eq!(glyph_char("E1D5"), Some('\u{E1D5}'));
        assert_eq!(glyph_char("quarter"), None);
        assert_eq!(glyph_char("D800"), None, "a surrogate is no character");
    }
}
