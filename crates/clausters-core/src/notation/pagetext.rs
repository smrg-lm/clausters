//! **The text of a page**: where the title, the names and the footnotes are
//! written, and how that is said in MEI.
//!
//! A page has a head and a foot, and each is a grid of three by three: left,
//! centre or right, and top, middle or bottom. A field of the
//! [`super::Header`] sits in one cell of one of them, on the first page or on
//! every page, and two fields in one cell are written one under the other.
//! Where a field sits when nobody moved it is the convention engraving has
//! always had ([`default_place`]): the title centred and large with its
//! subtitle under it, the words' author on the left and the music's on the
//! right, the copyright centred at the foot of the first page.
//!
//! In MEI these are the score definition's **running elements**, `<pgHead>`
//! and `<pgFoot>`, each a list of `<rend>` blocks placed by `@halign` and
//! `@valign` -- which is what the engraver lays out, measured against it
//! rather than assumed. Each block is written with the field's name as its
//! `@label` and under an id of its own ([`field_id`]), so the page it draws
//! names the field a press landed on and the reader gets every field back.

use roxmltree::Node;
use serde::{Deserialize, Serialize};

use super::Header;

/// The head of a page, or its foot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    /// Above the music.
    Head,
    /// Under it.
    Foot,
}

/// A cell's column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Halign {
    Left,
    Center,
    Right,
}

/// A cell's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Valign {
    Top,
    Middle,
    Bottom,
}

/// The pages a field is written on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pages {
    /// The first page alone.
    First,
    /// Every page.
    All,
}

/// **Where a field is written**: the region, the cell and the pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub region: Region,
    pub halign: Halign,
    pub valign: Valign,
    pub pages: Pages,
}

/// The single-line fields, in the order a cell holding several writes them.
pub const FIELDS: &[&str] = &[
    "title",
    "subtitle",
    "lyricist",
    "translator",
    "composer",
    "arranger",
    "copyright",
];

/// The name the footnotes go by, as a field: there are several, and they move
/// together.
pub const NOTE: &str = "note";

/// **Where `field` is written when nobody moved it**, by the convention of the
/// printed page.
#[must_use]
pub fn default_place(field: &str) -> Place {
    let (region, halign, valign) = match field {
        "lyricist" | "translator" => (Region::Head, Halign::Left, Valign::Bottom),
        "composer" | "arranger" => (Region::Head, Halign::Right, Valign::Bottom),
        "copyright" => (Region::Foot, Halign::Center, Valign::Bottom),
        NOTE => (Region::Foot, Halign::Left, Valign::Top),
        // the title, its subtitle, and anything this does not know
        _ => (Region::Head, Halign::Center, Valign::Middle),
    };
    Place {
        region,
        halign,
        valign,
        pages: Pages::First,
    }
}

/// **The id a field's block is written under**: `t-title`, and `t-note-2` for
/// the second footnote. What the page a press lands on names.
#[must_use]
pub fn field_id(field: &str, index: usize) -> String {
    if field == NOTE {
        format!("t-{NOTE}-{}", index + 1)
    } else {
        format!("t-{field}")
    }
}

/// **The field an engraved element is**: its name, and which footnote where
/// it is one (from zero) -- or `None` for an element that is no page text.
/// The emitter's own spelling, read back where it is written.
#[must_use]
pub fn field_of(element_id: &str) -> Option<(&str, usize)> {
    let name = element_id.strip_prefix("t-")?;
    if let Some(n) = name.strip_prefix("note-") {
        let n: usize = n.parse().ok()?;
        return n.checked_sub(1).map(|index| (NOTE, index));
    }
    FIELDS.contains(&name).then_some((name, 0))
}

/// Every text the header holds, as `(field, index, text)`, in writing order.
fn entries(header: &Header) -> Vec<(&'static str, usize, &str)> {
    let mut out: Vec<(&'static str, usize, &str)> = FIELDS
        .iter()
        .filter_map(|field| {
            header
                .text(field)
                .filter(|t| !t.is_empty())
                .map(|t| (*field, 0, t))
        })
        .collect();
    out.extend(
        header
            .notes
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.is_empty())
            .map(|(i, t)| (NOTE, i, t.as_str())),
    );
    out
}

/// The `@fontsize` a field is written in, where it is not the body's.
fn size_of(field: &str) -> &'static str {
    match field {
        "title" => " fontsize=\"x-large\"",
        "subtitle" | "copyright" | NOTE => " fontsize=\"small\"",
        _ => "",
    }
}

fn word<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// **The header as running elements**: what goes inside the score definition,
/// or nothing for a header that writes nothing -- which leaves the engraver's
/// own page head, and a document byte-identical to the one it always was.
///
/// Four blocks at most: the head and the foot, each for the first page and for
/// the rest. A field written on every page is in both of its region's, since
/// the engraver draws the first-page block on the first page *instead of* the
/// other. The page number is the engraver's own when it generates the head;
/// writing the head ourselves takes that over, so it is written here, from the
/// second page on.
#[must_use]
pub fn running_xml(header: &Header, escape: impl Fn(&str) -> String) -> String {
    let entries = entries(header);
    if entries.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (region, tag) in [(Region::Head, "pgHead"), (Region::Foot, "pgFoot")] {
        for (pages, func) in [(Pages::First, "first"), (Pages::All, "all")] {
            let mut rends = String::new();
            for (field, index, text) in &entries {
                let place = header.place(field);
                // the first page's block holds every field of the region, and
                // the other only those written on every page
                let belongs =
                    place.region == region && (pages == Pages::First || place.pages == Pages::All);
                if !belongs {
                    continue;
                }
                rends.push_str(&format!(
                    "<rend xml:id=\"{}\" label=\"{field}\" halign=\"{}\" valign=\"{}\"{}>{}</rend>",
                    field_id(field, *index),
                    word(place.halign),
                    word(place.valign),
                    size_of(field),
                    escape(text),
                ));
            }
            if region == Region::Head && pages == Pages::All {
                rends.push_str(
                    "<rend halign=\"center\" valign=\"top\"><num label=\"page\">#</num></rend>",
                );
            }
            if !rends.is_empty() {
                out.push_str(&format!(
                    "\n\x20\x20\x20<{tag} func=\"{func}\">{rends}</{tag}>"
                ));
            }
        }
    }
    out
}

/// An attribute of `node` read as one of the layout's words.
fn attr<T: serde::de::DeserializeOwned>(node: Node, name: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::from(node.attribute(name)?)).ok()
}

/// **Read the running elements of `score_def` into `header`**: every block
/// written with a field's name, its text, and its place where that is not the
/// convention's. Answers whether there was one -- a document with none keeps
/// whatever its own head says.
pub fn read_running(score_def: Node, header: &mut Header) -> bool {
    let mut found = false;
    // a field in the first-page block and in the other is on every page
    let mut seen: Vec<(String, usize, Place)> = Vec::new();
    for block in score_def.children().filter(Node::is_element) {
        let region = match block.tag_name().name() {
            "pgHead" => Region::Head,
            "pgFoot" => Region::Foot,
            _ => continue,
        };
        let pages = if block.attribute("func") == Some("all") {
            Pages::All
        } else {
            Pages::First
        };
        for rend in block
            .children()
            .filter(|n| n.is_element() && n.has_tag_name("rend"))
        {
            let Some(label) = rend.attribute("label") else {
                continue;
            };
            let Some((field, index)) = rend
                .attribute(("http://www.w3.org/XML/1998/namespace", "id"))
                .and_then(field_of)
                .filter(|(field, _)| *field == label)
            else {
                continue;
            };
            let convention = default_place(field);
            let place = Place {
                region,
                halign: attr(rend, "halign").unwrap_or(convention.halign),
                valign: attr(rend, "valign").unwrap_or(convention.valign),
                pages,
            };
            let text: String = rend
                .descendants()
                .filter(Node::is_text)
                .filter_map(|n| n.text())
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            found = true;
            if let Some(at) = seen.iter().position(|s| s.0 == field && s.1 == index) {
                seen[at].2.pages = Pages::All;
                continue;
            }
            seen.push((field.to_string(), index, place));
            if field == NOTE {
                if header.notes.len() <= index {
                    header.notes.resize(index + 1, String::new());
                }
                header.notes[index] = text;
            } else if let Some(slot) = header.text_mut(field) {
                *slot = text;
            }
        }
    }
    for (field, _, place) in seen {
        if place != default_place(&field) {
            header.places.insert(field, place);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_sits_where_the_printed_page_puts_it() {
        let title = default_place("title");
        assert_eq!((title.region, title.halign), (Region::Head, Halign::Center));
        assert_eq!(default_place("composer").halign, Halign::Right);
        assert_eq!(default_place("lyricist").halign, Halign::Left);
        let copyright = default_place("copyright");
        assert_eq!(
            (copyright.region, copyright.valign),
            (Region::Foot, Valign::Bottom)
        );
        assert_eq!(default_place(NOTE).region, Region::Foot);
    }

    #[test]
    fn a_block_is_named_by_its_field_and_read_back() {
        assert_eq!(field_id("title", 0), "t-title");
        assert_eq!(field_id(NOTE, 1), "t-note-2");
        assert_eq!(field_of("t-title"), Some(("title", 0)));
        assert_eq!(field_of("t-note-2"), Some((NOTE, 1)));
        for not_one in ["n3", "t-", "t-note-0", "t-margin", "title"] {
            assert_eq!(field_of(not_one), None, "{not_one}");
        }
    }

    #[test]
    fn the_page_text_is_written_as_running_elements_and_read_back() {
        use crate::notation::{Sheet, mei_to_sheet, sheet_to_mei};

        let mut header = Header {
            title: "A title".into(),
            subtitle: "and a second line".into(),
            composer: "A. Composer".into(),
            arranger: "arr. B".into(),
            lyricist: "words by C".into(),
            translator: "tr. D".into(),
            copyright: "(c) E & F".into(),
            notes: vec!["* one".into(), "** two".into()],
            ..Header::default()
        };
        // the composer moved to the left, on every page
        header.places.insert(
            "composer".into(),
            Place {
                halign: Halign::Left,
                pages: Pages::All,
                ..default_place("composer")
            },
        );
        let sheet = Sheet {
            header: header.clone(),
            ..Sheet::default()
        };
        let mei = sheet_to_mei(&sheet).expect("writes");
        assert!(mei.contains("<pgHead func=\"first\">"), "{mei}");
        assert!(mei.contains(
            "<rend xml:id=\"t-title\" label=\"title\" halign=\"center\" valign=\"middle\" fontsize=\"x-large\">A title</rend>"
        ));
        // on every page is in both of the head's blocks, and the number is ours
        assert_eq!(mei.matches("xml:id=\"t-composer\"").count(), 2);
        assert!(mei.contains("<num label=\"page\">#</num>"));
        assert!(mei.contains("<pgFoot func=\"first\">"));
        assert!(mei.contains("(c) E &amp; F"));
        assert_eq!(mei_to_sheet(&mei).expect("reads").header, header);
    }

    #[test]
    fn a_header_that_writes_nothing_writes_no_running_element() {
        assert_eq!(running_xml(&Header::default(), str::to_string), "");
    }
}
