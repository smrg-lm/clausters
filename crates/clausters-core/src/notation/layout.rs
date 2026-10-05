//! **The page a score is laid out on**: the paper, and the two ways of looking
//! at the music on it.
//!
//! Two things that are easy to take for one. The **page setup** is the
//! document's -- somebody chose the paper, its margins and the size of the
//! staff, and a score opened tomorrow is on that paper still -- so it is a
//! field of the [`super::Sheet`] and travels in the MEI. The **view** is the
//! window's: the same document seen as pages, or as one system that never
//! breaks. Both come to the engraver as its options ([`options`]), one rule for
//! every client, because two clients configuring their engravers differently
//! would draw the same score two ways.
//!
//! Lengths are in **tenths of a millimetre**, the engraver's own unit (its
//! default page, 2100 by 2970, is A4), and whole numbers, so a setup compares
//! and round-trips exactly.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One paper size by name, portrait, in tenths of a millimetre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Paper {
    /// What it is called.
    pub name: &'static str,
    /// Its width, the shorter side.
    pub width: u32,
    /// Its height, the longer side.
    pub height: u32,
}

/// **The paper sizes a score is printed on.** The international sizes; the
/// two North American ones; the two the orchestral libraries' preparation
/// guidelines give for a part (9 by 12 inches as the least, 10 by 13 as the
/// one to prefer); and the octavo choral music is published in.
pub const PAPERS: &[Paper] = &[
    Paper {
        name: "A4",
        width: 2100,
        height: 2970,
    },
    Paper {
        name: "A3",
        width: 2970,
        height: 4200,
    },
    Paper {
        name: "B4",
        width: 2500,
        height: 3530,
    },
    Paper {
        name: "Letter",
        width: 2159,
        height: 2794,
    },
    Paper {
        name: "Tabloid",
        width: 2794,
        height: 4318,
    },
    Paper {
        name: "9x12",
        width: 2286,
        height: 3048,
    },
    Paper {
        name: "10x13",
        width: 2540,
        height: 3302,
    },
    Paper {
        name: "Octavo",
        width: 1715,
        height: 2667,
    },
];

/// The paper called `name`, whatever its letters' case.
#[must_use]
pub fn paper(name: &str) -> Option<&'static Paper> {
    PAPERS.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

/// **A score's page setup**: the paper as it is turned, the margins, and how
/// tall a staff is. All in tenths of a millimetre but the staff, which is in
/// hundredths -- a staff is a few millimetres and its size is chosen finely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageSetup {
    /// The page's width as it is turned: the longer side, for a landscape one.
    pub width: u32,
    /// The page's height as it is turned.
    pub height: u32,
    /// The margins: top, right, bottom, left.
    #[serde(default = "default_margins")]
    pub margins: [u32; 4],
    /// The height of a five-line staff, in hundredths of a millimetre.
    #[serde(default = "default_staff")]
    pub staff: u32,
}

/// Half an inch all round, the margin the preparation guidelines ask of a part.
fn default_margins() -> [u32; 4] {
    [127; 4]
}

/// 7.2 mm: the engraver's own default staff, inside the 7 to 8.5 mm the
/// guidelines give for a part.
fn default_staff() -> u32 {
    720
}

impl Default for PageSetup {
    /// A4, portrait.
    fn default() -> Self {
        Self {
            width: PAPERS[0].width,
            height: PAPERS[0].height,
            margins: default_margins(),
            staff: default_staff(),
        }
    }
}

impl PageSetup {
    /// The setup on `paper`, turned to `landscape` or left portrait, with the
    /// default margins and staff.
    #[must_use]
    pub fn on(paper: &Paper, landscape: bool) -> Self {
        let (width, height) = if landscape {
            (paper.height, paper.width)
        } else {
            (paper.width, paper.height)
        };
        Self {
            width,
            height,
            ..Self::default()
        }
    }

    /// Whether the page is wider than it is tall.
    #[must_use]
    pub fn landscape(&self) -> bool {
        self.width > self.height
    }

    /// The name of the paper this is, when it is one of [`PAPERS`] either way
    /// up.
    #[must_use]
    pub fn paper(&self) -> Option<&'static str> {
        let (short, long) = (self.width.min(self.height), self.width.max(self.height));
        PAPERS
            .iter()
            .find(|p| p.width == short && p.height == long)
            .map(|p| p.name)
    }

    /// Why this setup cannot be engraved, if it cannot: the engraver's own
    /// limits on a page, a margin and a staff, and a page its margins leave
    /// nothing of.
    ///
    /// # Errors
    /// With the reason, in the words a status line can show.
    pub fn check(&self) -> Result<(), String> {
        if !(100..=60_000).contains(&self.height) || !(100..=100_000).contains(&self.width) {
            return Err("a page is between 1 cm and a few metres on a side".into());
        }
        if self.margins.iter().any(|m| *m > 500) {
            return Err("a margin is at most 5 cm".into());
        }
        if !(360..=960).contains(&self.staff) {
            return Err("a staff is between 3.6 and 9.6 mm tall".into());
        }
        let [top, right, bottom, left] = self.margins;
        if left + right >= self.width || top + bottom >= self.height {
            return Err("the margins leave nothing of the page".into());
        }
        Ok(())
    }

    /// The engraver's `unit` for this staff: half the distance between two
    /// staff lines, in tenths of a millimetre. A staff is eight of them tall.
    #[must_use]
    pub fn unit(&self) -> f64 {
        f64::from(self.staff) / 80.0
    }
}

/// **How a window looks at a score.**
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    /// As pages of the paper the setup names: fixed, whatever the window's
    /// size.
    #[default]
    Page,
    /// As one system the length of the music, with no page at all.
    Continuous,
}

impl View {
    /// The view a word names (`"page"`, `"continuous"`).
    #[must_use]
    pub fn parse(word: &str) -> Option<View> {
        match word {
            "page" => Some(View::Page),
            "continuous" => Some(View::Continuous),
            _ => None,
        }
    }

    /// The word for this view.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            View::Page => "page",
            View::Continuous => "continuous",
        }
    }
}

/// **The engraver's options for `setup` seen as `view`**, as the JSON object it
/// is configured with.
///
/// A page view fixes the page: its size, its margins, the staff, and breaks
/// the engraver works out wherever the writer wrote none (`auto` honours a
/// written system or page break and fills in the rest -- measured against the
/// engraver, not assumed). A continuous view asks for no break at all and a
/// page fitted to what that leaves: one system, as long as the music.
#[must_use]
pub fn options(setup: &PageSetup, view: View) -> String {
    let [top, right, bottom, left] = setup.margins;
    let mut out = json!({
        "svgViewBox": true,
        // the engraver's `scale` is a zoom of what is laid out, and paper has
        // none: at any other value a page holds more or less music than the
        // staff size says it does
        "scale": 100,
        "unit": setup.unit(),
        "pageMarginTop": top,
        "pageMarginRight": right,
        "pageMarginBottom": bottom,
        "pageMarginLeft": left,
    });
    let more = match view {
        View::Page => json!({
            "breaks": "auto",
            "pageWidth": setup.width,
            "pageHeight": setup.height,
            "adjustPageHeight": false,
            "adjustPageWidth": false,
            "header": "auto",
            "footer": "auto",
        }),
        View::Continuous => json!({
            "breaks": "none",
            "pageWidth": setup.width,
            "pageHeight": setup.height,
            "adjustPageHeight": true,
            "adjustPageWidth": true,
            "header": "none",
            "footer": "none",
        }),
    };
    if let (Value::Object(out), Value::Object(more)) = (&mut out, more) {
        out.extend(more);
    }
    out.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paper_is_found_by_name_and_turned() {
        let a4 = paper("a4").expect("A4");
        assert_eq!((a4.width, a4.height), (2100, 2970));
        let turned = PageSetup::on(a4, true);
        assert_eq!((turned.width, turned.height), (2970, 2100));
        assert!(turned.landscape());
        assert_eq!(turned.paper(), Some("A4"), "the same paper, either way up");
        assert_eq!(PageSetup::default().paper(), Some("A4"));
        assert!(paper("foolscap").is_none());
    }

    #[test]
    fn a_setup_the_engraver_cannot_take_says_why() {
        assert!(PageSetup::default().check().is_ok());
        let tiny = PageSetup {
            staff: 100,
            ..PageSetup::default()
        };
        assert!(tiny.check().unwrap_err().contains("staff"));
        let eaten = PageSetup {
            width: 600,
            margins: [300; 4],
            ..PageSetup::default()
        };
        assert!(eaten.check().unwrap_err().contains("margins"));
    }

    #[test]
    fn a_setup_is_the_documents_and_survives_being_written() {
        use crate::notation::{Op, Sheet, apply, mei_to_sheet, sheet_to_mei};

        let setup = PageSetup {
            staff: 725,
            margins: [150, 100, 200, 127],
            ..PageSetup::on(paper("Letter").unwrap(), true)
        };
        let sheet = apply(Sheet::default(), &Op::SetPage { page: Some(setup) }).expect("applies");
        let mei = sheet_to_mei(&sheet).expect("writes");
        assert!(mei.contains("page.width=\"279.4mm\""), "{mei}");
        assert_eq!(mei_to_sheet(&mei).expect("reads").page, Some(setup));
        // nobody having chosen writes nothing, and reads as nobody having chosen
        let plain = sheet_to_mei(&Sheet::default()).unwrap();
        assert!(!plain.contains("page.width"));
        assert_eq!(mei_to_sheet(&plain).unwrap().page, None);
        // and a setup the engraver could not take is refused, not stored
        let bad = PageSetup {
            staff: 10,
            ..PageSetup::default()
        };
        assert!(apply(Sheet::default(), &Op::SetPage { page: Some(bad) }).is_err());
    }

    #[test]
    fn the_staff_size_is_the_engravers_unit() {
        // the engraver's default unit, 9, is a 7.2 mm staff
        assert_eq!(PageSetup::default().unit(), 9.0);
    }

    #[test]
    fn a_page_view_fixes_the_page_and_a_continuous_one_asks_for_no_break() {
        let setup = PageSetup::default();
        let page: Value = serde_json::from_str(&options(&setup, View::Page)).unwrap();
        assert_eq!(page["pageWidth"], 2100);
        assert_eq!(page["pageHeight"], 2970);
        assert_eq!(page["adjustPageHeight"], false);
        assert_eq!(page["scale"], 100, "paper is laid out at its own size");
        assert_eq!(page["breaks"], "auto");
        let line: Value = serde_json::from_str(&options(&setup, View::Continuous)).unwrap();
        assert_eq!(line["breaks"], "none");
        assert_eq!(line["adjustPageWidth"], true);
        assert_eq!(line["header"], "none");
        assert_eq!(View::parse("continuous"), Some(View::Continuous));
        assert_eq!(View::Page.word(), "page");
    }
}
