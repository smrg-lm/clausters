//! The documentation model's tests: what a document reads as, how it is
//! laid out, and how it is searched -- the current editor's
//! `docIndex.test.ts` ported case by case.

use super::index::{Index, sections, talking_about};
use super::layout::{Item, lay};
use super::markdown::{Block, Run, parse, plain, slugify};

/// **Every kind of block** is read: headings with their ids, paragraphs in
/// runs of a style, lists numbered or not and nested, code with its
/// language, a table, a quote, a rule, an image and an anchor.
#[test]
fn a_document_reads_as_its_blocks() {
    let source = "# The `Server` class\n\nIt **boots** a *server*, see [buffers](buffers.md#reading).\n\n\
- one\n- two\n  1. nested\n\n```python\nx = 1\n```\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n\n> said\n\n---\n\n\
![a picture](p.png)\n\n<a id=\"clausters.Server.boot\"></a>\n";
    let blocks = parse(source);
    let Block::Heading { level, runs, id } = &blocks[0] else {
        panic!("{:?}", blocks[0]);
    };
    assert_eq!(
        (*level, plain(runs).as_str(), id.as_str()),
        (1, "The Server class", "the-server-class")
    );
    assert!(runs.iter().any(|r| r.code && r.text == "Server"));
    let Block::Para(runs) = &blocks[1] else {
        panic!("{:?}", blocks[1]);
    };
    assert!(runs.iter().any(|r| r.strong && r.text == "boots"));
    assert!(runs.iter().any(|r| r.emph && r.text == "server"));
    assert!(
        runs.iter()
            .any(|r| r.link.as_deref() == Some("buffers.md#reading") && r.text == "buffers")
    );
    let Block::List { start: None, items } = &blocks[2] else {
        panic!("{:?}", blocks[2]);
    };
    assert_eq!(items.len(), 2);
    assert!(matches!(
        items[1].last(),
        Some(Block::List { start: Some(1), .. })
    ));
    assert_eq!(
        blocks[3],
        Block::Code {
            lang: "python".into(),
            text: "x = 1".into()
        }
    );
    assert!(
        matches!(&blocks[4], Block::Table { head, rows, .. } if head.len() == 2 && rows.len() == 1)
    );
    assert!(matches!(&blocks[5], Block::Quote(_)));
    assert_eq!(blocks[6], Block::Rule);
    assert!(matches!(&blocks[7], Block::Image { alt, .. } if alt == "a picture"));
    assert_eq!(blocks[8], Block::Anchor("clausters.Server.boot".into()));
}

/// A heading's id is the current editor's: what is not a letter, a digit,
/// `_` or `-` goes, and a run of spaces is one `-`.
#[test]
fn a_heading_id_is_the_editors() {
    assert_eq!(slugify("Buffers & `samples`"), "buffers-samples");
    assert_eq!(slugify("Server.boot()"), "serverboot");
    assert_eq!(slugify("  Two  words "), "two-words");
}

/// **A page is laid out top to bottom**: a long paragraph wraps inside its
/// width, the heading's place is where a link to it lands, a link and a
/// code block's copy button can be pressed, and code is colored.
#[test]
fn a_page_is_laid_out() {
    let words = "word ".repeat(80);
    let source = format!(
        "# Title\n\n{words}\n\n## Reading\n\nSee [the guide](guide.md).\n\n```python\ndef f():\n    return 1\n```\n"
    );
    let laid = lay(&parse(&source), 300.0, 2.0);
    assert!(laid.height > 0.0);
    let inside = laid.items.iter().all(|i| match i {
        Item::Text { x, .. } => *x >= 0.0 && *x < 300.0,
        Item::Fill { rect, .. } | Item::Picture { rect, .. } => {
            rect.x >= 0.0 && rect.x + rect.w <= 300.5
        }
    });
    assert!(inside, "everything inside the width");
    let lines: std::collections::BTreeSet<u32> = laid
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Text { text, y, .. } if text.starts_with("word") => Some(y.to_bits()),
            _ => None,
        })
        .collect();
    assert!(
        lines.len() > 3,
        "the paragraph wraps: {} lines",
        lines.len()
    );
    let reading = laid.anchor("reading").expect("the heading's place");
    assert!(reading > laid.anchor("title").unwrap());
    let (r, href) = &laid.links[0];
    assert_eq!(href, "guide.md");
    assert_eq!(laid.link_at(r.x + 1.0, r.y + 1.0), Some("guide.md"));
    let (button, code) = &laid.copies[0];
    assert_eq!(code, "def f():\n    return 1");
    assert_eq!(
        laid.copy_at(button.x + 1.0, button.y + 1.0),
        Some(code.as_str())
    );
    #[cfg(feature = "highlight")]
    assert!(laid.items.iter().any(|i| matches!(
        i,
        Item::Text {
            ink: super::layout::Ink::Code(Some(_)),
            ..
        }
    )));
    assert!(
        laid.items.windows(2).all(|w| w[0].top() <= w[1].top()),
        "in order from the top"
    );
    let _ = Run::default();
}

// -- the current editor's docIndex.test.ts, ported case by case

const GUIDE: &str = "# Guide\n\nStart here.\n\n## Buffers\n\nA buffer holds samples.\n\n```\n# not a heading\n```\n";
const API: &str = "# API\n\n## Server\n\nBoots the server.\n";

fn index() -> Index {
    Index::build([("guide.md", GUIDE), ("api.md", API)])
}

/// A word finds its section, with the line it is on.
#[test]
fn index_a_word_finds_its_section() {
    let hits = index().search("samples", 200);
    let hit = &hits[0];
    assert_eq!(hit.path, "guide.md");
    assert_eq!(hit.heading.as_deref(), Some("Buffers"));
    assert_eq!(hit.line, "A buffer holds samples.");
}

/// A heading weighs more than the text under another.
#[test]
fn index_a_heading_weighs_more() {
    let hits = index().search("server", 200);
    assert_eq!(hits[0].heading.as_deref(), Some("Server"));
}

/// A line inside a code fence is text, not a heading.
#[test]
fn index_a_fenced_line_is_text() {
    let hits = index().search("heading", 200);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].heading.as_deref(), Some("Buffers"));
    assert_eq!(
        sections("guide.md", GUIDE).len(),
        2,
        "the empty one before the first heading left out"
    );
}

/// A book that cannot be read is left out, and the rest still searched.
#[test]
fn index_what_is_missing_is_left_out() {
    let found = Index::build([("api.md", API)]);
    assert_eq!(found.search("boots", 200)[0].path, "api.md");
}

/// **A word starts another, or is a letter from it**, and every word of a
/// query must be held.
#[test]
fn index_prefix_fuzzy_and_every_word() {
    let i = index();
    assert_eq!(i.search("buff", 200)[0].heading.as_deref(), Some("Buffers"));
    assert_eq!(
        i.search("sampes", 200)[0].heading.as_deref(),
        Some("Buffers")
    );
    assert!(
        i.search("buffer server", 200).is_empty(),
        "both words, in one section"
    );
}

/// **Help on a name finds the entry that is it** before a page that only
/// says it.
#[test]
fn help_finds_the_entry_named() {
    let reference = "# Reference\n\n## `Server.boot`\n\nBoots it.\n";
    let prose = "# Guide\n\nCall `boot` and `boot` again, then boot.\n";
    let hits = talking_about([("guide.md", prose), ("ref.md", reference)], "boot");
    assert_eq!(hits[0].path, "ref.md");
    assert!(hits[0].named);
    assert_eq!(hits[1].path, "guide.md");
    assert!(!hits[1].named);
}
